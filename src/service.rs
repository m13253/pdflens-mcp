use std::borrow::Cow;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock};

use base64::prelude::*;
use eyre::{Result, bail, eyre};
use hayro::{InterpreterSettings, Pdf, RenderSettings};
use pdf_extract::extract_text_from_mem_by_pages;
use percent_encoding::{AsciiSet, CONTROLS, percent_decode_str, utf8_percent_encode};
use regex::Regex;
use rmcp::handler::server::tool::{IntoCallToolResult, ToolRouter, cached_schema_for_type};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CallToolResult, Content, Implementation, ProgressNotificationParam, Role, ServerCapabilities,
    ServerInfo,
};
use rmcp::service::{NotificationContext, RequestContext};
use rmcp::{Json, Peer, RoleServer, ServerHandler};
use tokio::sync::RwLock;
use tokio::task::spawn_blocking;
use tracing::instrument;

use crate::param::{
    GetPdfNumPagesParams, GetPdfNumPagesResult, ListWorkspaceDirsResults, ReadPdfAsImagesParams,
    ReadPdfAsTextParams,
};

pub struct PdflensService {
    tool_router: ToolRouter<Self>,
    roots: RwLock<Vec<PathBuf>>,
}

#[rmcp::tool_router]
impl PdflensService {
    pub fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
            roots: RwLock::new(Vec::from_iter(std::env::current_dir().ok())),
        }
    }

    #[tracing::instrument(skip_all)]
    async fn update_roots(&self, peer: &Peer<RoleServer>) {
        static FILE_URI_REGEX: LazyLock<Regex> =
            LazyLock::new(|| Regex::new("(?is)^file:/{0,2}(.*)").unwrap());

        let roots = match peer.list_roots().await {
            Ok(roots) => roots.roots,
            Err(err) => {
                tracing::error!("Failed to request root paths, keeping the old list: {err}");
                return;
            }
        };
        let mut roots = roots
            .into_iter()
            .filter_map(|root| {
                FILE_URI_REGEX
                    .captures(&root.uri)
                    .and_then(|captures| {
                        percent_decode_str(captures.get(1).unwrap().as_str())
                            .decode_utf8()
                            .ok()
                    })
                    .map(|path| PathBuf::from(path.as_ref()))
            })
            .collect::<Vec<_>>();
        for root in &mut roots {
            if let Ok(path) = tokio::fs::canonicalize(&root).await {
                *root = path;
            }
        }
        *self.roots.write().await = roots;
    }

    #[rmcp::tool(
        description = "List the user’s workspace directories, also known as MCP root directories.",
        annotations(read_only_hint = true)
    )]
    pub async fn list_workspace_dirs(
        &self,
    ) -> Result<Json<ListWorkspaceDirsResults>, rmcp::ErrorData> {
        Ok(self.list_workspace_dirs_handler().await)
    }

    #[rmcp::tool(
        description = "Get the number of pages in a PDF.",
        annotations(read_only_hint = true),
        output_schema = cached_schema_for_type::<GetPdfNumPagesResult>()
    )]
    pub async fn get_pdf_num_pages(
        &self,
        Parameters(params): Parameters<GetPdfNumPagesParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        self.get_pdf_num_pages_handler(&params).await.map_or_else(
            |err| {
                tracing::error!("{err:?}");
                Ok(CallToolResult::error(vec![
                    Content::text(format!("{err:#}")).with_audience(vec![Role::Assistant]),
                ]))
            },
            |ok| ok.into_call_tool_result(),
        )
    }

    #[rmcp::tool(
        description = "Read a PDF in plain text format. The output separates each page with “\x0c” (U+000C). Performance recommendation: if numPages < 1000, omit `fromPage` and `toPage` to read the whole PDF; otherwise, read in chunks of 1000 pages.",
        annotations(read_only_hint = true)
    )]
    pub async fn read_pdf_as_text(
        &self,
        Parameters(params): Parameters<ReadPdfAsTextParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        self.read_pdf_as_text_handler(&params)
            .await
            .or_else(|err: eyre::Error| {
                tracing::error!("{err:?}");
                Ok(CallToolResult::error(vec![
                    Content::text(format!("{err:#}")).with_audience(vec![Role::Assistant]),
                ]))
            })
    }

    #[rmcp::tool(
        description = "Read pages of a PDF as images. The output contains one image per page. Performance recommendation: Only use this tool on specific pages after reading the text version.",
        annotations(read_only_hint = true)
    )]
    pub async fn read_pdf_as_images(
        &self,
        Parameters(params): Parameters<ReadPdfAsImagesParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        self.read_pdf_as_images_handler(&params, &context)
            .await
            .or_else(|err| {
                tracing::error!("{err:?}");
                Ok(CallToolResult::error(vec![
                    Content::text(format!("{err:#}")).with_audience(vec![Role::Assistant]),
                ]))
            })
    }

    #[instrument(skip_all)]
    async fn list_workspace_dirs_handler(&self) -> Json<ListWorkspaceDirsResults> {
        const ESCAPE_SET: &AsciiSet = &CONTROLS.add(b' ').add(b'#').add(b'?');

        Json(ListWorkspaceDirsResults {
            dirs: self
                .roots
                .read()
                .await
                .iter()
                .map(|path| {
                    format!(
                        "file://{}",
                        utf8_percent_encode(&path.to_string_lossy(), ESCAPE_SET)
                    )
                })
                .collect::<Vec<_>>(),
        })
    }

    #[instrument(skip_all)]
    async fn load_file(&self, uri: &str) -> Result<Vec<u8>> {
        static MAYBE_URI_REGEX: LazyLock<Regex> =
            LazyLock::new(|| Regex::new("(?s)^(?:([A-Za-z][0-9A-Za-z]*):/{0,2})?(.*)").unwrap());

        let captures = MAYBE_URI_REGEX.captures(uri).unwrap();
        let filename = if let Some(schema) = captures.get(1) {
            if schema.as_str().eq_ignore_ascii_case("file") {
                percent_decode_str(&captures[2]).decode_utf8()?
            } else {
                bail!(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!(
                        "Invalid file path: {uri:?}\nAbsolute paths start with `file:///`. Relative paths are relative to the root of any opened workspaces."
                    )
                ));
            }
        } else {
            Cow::from(&captures[2])
        };

        let roots = self.roots.read().await.clone();
        let roots_hashset = HashSet::<_>::from_iter(roots.iter());

        if filename.starts_with(&['/', '\\']) {
            let path = match tokio::fs::canonicalize(filename.as_ref()).await {
                Ok(path) => path,
                Err(err) => {
                    if err.kind() == std::io::ErrorKind::NotFound {
                        bail!(std::io::Error::new(
                            std::io::ErrorKind::NotFound,
                            format!(
                                "File not found: {uri:?}\nPlease use folder browsing tools to confirm the correct path."
                            )
                        ));
                    } else {
                        bail!(err);
                    }
                }
            };
            if !roots_hashset.into_iter().any(|root| path.starts_with(root)) {
                bail!(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    format!(
                        "Access is denied, because the file is outside the user’s workspace directories: {path:?}\nUse `list_workspace_dirs` to show current workspace directories."
                    )
                ))
            }
            let file_data = tokio::fs::read(path).await?;
            Ok(file_data)
        } else {
            for root in roots {
                let path = match tokio::fs::canonicalize(root.join(filename.as_ref())).await {
                    Ok(path) => path,
                    Err(err) => {
                        if err.kind() == std::io::ErrorKind::NotFound {
                            continue;
                        } else {
                            bail!(err);
                        }
                    }
                };
                if !path.starts_with(root) {
                    // Treat as not found
                    continue;
                }
                let file_data = tokio::fs::read(path).await?;
                return Ok(file_data);
            }
            bail!(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!(
                    "File not found: {uri:?}\nPlease use folder browsing tools to confirm the correct path. Relative paths are searched from the workspace directories listed in `list_workspace_dirs`."
                )
            ));
        }
    }

    #[instrument(skip_all)]
    async fn get_pdf_num_pages_handler(
        &self,
        params: &GetPdfNumPagesParams,
    ) -> Result<Json<GetPdfNumPagesResult>> {
        let file_data = Arc::new(self.load_file(&params.path).await?);
        let pdf = spawn_blocking(|| {
            Pdf::new(file_data).map_err(|err| eyre!("Failed to load PDF: {err:?}"))
        })
        .await??;
        let num_pages = pdf.pages().len();
        Ok(Json(GetPdfNumPagesResult { num_pages }))
    }

    #[instrument(skip_all)]
    async fn read_pdf_as_text_handler(
        &self,
        params: &ReadPdfAsTextParams,
    ) -> Result<CallToolResult> {
        let file_data = self.load_file(&params.path).await?;
        let mut pages =
            spawn_blocking(move || extract_text_from_mem_by_pages(&file_data)).await??;

        // Convert to 0-based, half-closed half-open indices
        let num_pages = pages.len();
        let from_page_idx = params.from_page.saturating_sub(1).min(num_pages);
        let to_page_idx = params
            .to_page
            .map(|x| x.clamp(from_page_idx, num_pages))
            .unwrap_or(num_pages);

        pages.truncate(to_page_idx);
        pages.drain(..from_page_idx);

        Ok(CallToolResult::success(vec![
            Content::text(pages.join("\x0c")).with_audience(vec![Role::Assistant]),
        ]))
    }

    #[instrument(skip_all)]
    async fn read_pdf_as_images_handler(
        &self,
        params: &ReadPdfAsImagesParams,
        context: &RequestContext<RoleServer>,
    ) -> Result<CallToolResult> {
        let file_data = Arc::new(self.load_file(&params.path).await?);
        let pdf = spawn_blocking(|| match hayro::Pdf::new(file_data) {
            Ok(ok) => Ok(Arc::new(ok)),
            Err(err) => bail!("Failed to load PDF: {err:?}"),
        })
        .await??;
        let interpreter_settings = InterpreterSettings::default();

        // Convert to 0-based, half-closed half-open indices
        let num_pages = pdf.pages().len();
        let from_page_idx = params.from_page.saturating_sub(1).min(num_pages);
        let to_page_idx = params
            .to_page
            .map(|x| x.clamp(from_page_idx, num_pages))
            .unwrap_or(num_pages);
        let page_count = to_page_idx - from_page_idx;

        let progress_token = context.meta.get_progress_token();
        let mut content = Vec::with_capacity(page_count);
        for (i, page_idx) in (from_page_idx..to_page_idx)
            .enumerate()
            .take_while(|_| !context.ct.is_cancelled())
        {
            if let Some(progress_token) = &progress_token {
                context
                    .peer
                    .notify_progress(ProgressNotificationParam {
                        progress_token: progress_token.clone(),
                        progress: i as f64,
                        total: Some(page_count as f64),
                        message: None,
                    })
                    .await?;
            };

            let pdf = pdf.clone();
            let image_dimension = params.image_dimension;
            let interpreter_settings = interpreter_settings.clone();

            let image = spawn_blocking(move || {
                let page = &pdf.pages()[page_idx];

                let (orig_width, orig_height) = page.render_dimensions();
                let render_settings = if orig_width >= orig_height {
                    let width = image_dimension.max(1);
                    let height = ((image_dimension as f64 * orig_height as f64 / orig_width as f64)
                        .round() as u16)
                        .max(1);
                    RenderSettings {
                        x_scale: width as f32 / orig_width as f32,
                        y_scale: height as f32 / orig_height as f32,
                        width: Some(width),
                        height: Some(height),
                    }
                } else {
                    let width = ((image_dimension as f64 * orig_width as f64 / orig_height as f64)
                        .round() as u16)
                        .max(1);
                    let height = image_dimension.max(1);
                    RenderSettings {
                        x_scale: width as f32 / orig_width as f32,
                        y_scale: height as f32 / orig_height as f32,
                        width: Some(width),
                        height: Some(height),
                    }
                };

                BASE64_STANDARD.encode(
                    hayro::render(&page, &interpreter_settings, &render_settings).take_png(),
                )
            })
            .await?;

            content.push(Content::image(image, "image/png").with_audience(vec![Role::Assistant]));
        }

        if let Some(progress_token) = &progress_token {
            context
                .peer
                .notify_progress(ProgressNotificationParam {
                    progress_token: progress_token.clone(),
                    progress: page_count as f64,
                    total: Some(page_count as f64),
                    message: None,
                })
                .await?;
        };

        Ok(CallToolResult::success(content))
    }
}

#[rmcp::tool_handler]
impl ServerHandler for PdflensService {
    #[tracing::instrument(skip_all)]
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            server_info: Implementation {
                name: "pdflens".to_owned(),
                title: Some("pdflens".to_owned()),
                version: env!("CARGO_PKG_VERSION").to_owned(),
                website_url: Some("https://github.com/m13253/pdflens-mcp".to_owned()),
                ..Default::default()
            },
            instructions: Some("A tool for reading PDF files".to_owned()),
            ..Default::default()
        }
    }

    #[tracing::instrument(skip_all)]
    async fn on_roots_list_changed(&self, context: NotificationContext<RoleServer>) {
        self.update_roots(&context.peer).await;
    }
}
