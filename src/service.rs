use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use base64::prelude::*;
use eyre::{Result, bail, eyre};
use hayro::{InterpreterSettings, Pdf, RenderSettings};
use indexmap::IndexSet;
use pdf_extract::extract_text_from_mem_by_pages;
use rmcp::handler::server::tool::{IntoCallToolResult, ToolRouter, schema_for_type};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CallToolResult, Content, Implementation, ProgressNotificationParam, Role, ServerCapabilities,
    ServerInfo,
};
use rmcp::service::RequestContext;
use rmcp::{Json, Peer, RoleServer, ServerHandler};
use tokio::task::spawn_blocking;
use tracing::instrument;
use url::Url;

use crate::param::{
    GetPdfNumPagesParams, GetPdfNumPagesResult, ReadPdfAsImagesParams, ReadPdfAsTextParams,
    ReadPdfPageAsImageParams,
};

pub struct PdflensService {
    tool_router: ToolRouter<Self>,
}

impl PdflensService {
    pub fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
        }
    }

    #[tracing::instrument(skip_all)]
    pub async fn get_roots(peer: &Peer<RoleServer>) -> IndexSet<PathBuf> {
        if !peer
            .peer_info()
            .map(|peer_info| peer_info.capabilities.roots.is_some())
            .unwrap_or_default()
        {
            let roots = Self::get_roots_fallback().await;
            tracing::warn!(
                "MCP client does not support root path capability, falling back to current working directory: {roots:?}"
            );
            return roots;
        }

        let uris = match peer.list_roots().await {
            Ok(roots) => roots.roots,
            Err(err) => {
                let roots = Self::get_roots_fallback().await;
                tracing::error!(
                    "Failed to request MCP root paths: {err}. Falling back to current working directory: {roots:?}"
                );
                return roots;
            }
        };
        tracing::info!("Resolving MCP root paths: {uris:?}");

        let mut roots = IndexSet::new();
        for root in uris {
            let Some(path) = Url::parse(&root.uri)
                .ok()
                .filter(|uri| uri.scheme().eq_ignore_ascii_case("file"))
                .and_then(|uri| uri.to_file_path().ok())
            else {
                tracing::error!("Ignored invalid MCP root path: {:?}", root.uri);
                continue;
            };
            roots.insert(match tokio::fs::canonicalize(&path).await {
                Ok(path) => {
                    tracing::info!("Resolved MCP root path: {:?} → {path:?}", root.uri);
                    path
                }
                Err(err) => {
                    tracing::error!(
                        "Failed to resolve MCP root path, keeping as is: {:?} → {err}",
                        root.uri
                    );
                    path
                }
            });
        }
        if roots.is_empty() {
            let roots = Self::get_roots_fallback().await;
            tracing::error!(
                "MCP client returned no valid root paths, Falling back to current working directory: {roots:?}"
            );
            return roots;
        }

        tracing::info!("Finalized MCP root paths: {roots:?}");
        roots
    }

    #[tracing::instrument(skip_all)]
    async fn get_roots_fallback() -> IndexSet<PathBuf> {
        if let Ok(cwd) = std::env::current_dir() {
            IndexSet::from([match tokio::fs::canonicalize(&cwd).await {
                Ok(path) => {
                    tracing::info!("Resolved MCP root path: {cwd:?} → {path:?}");
                    path
                }
                Err(err) => {
                    tracing::error!(
                        "Failed to resolve MCP root path, keeping as is: {cwd:?} → {err}",
                    );
                    cwd
                }
            }])
        } else {
            IndexSet::new()
        }
    }

    #[instrument(skip_all)]
    fn format_roots_as_uri(roots: impl IntoIterator<Item = impl AsRef<Path>>) -> String {
        let mut builder = String::new();
        for root in roots {
            builder.push_str(if builder.is_empty() { "* " } else { "\n* " });
            if let Ok(uri) = Url::from_directory_path(&root) {
                builder.push_str(uri.as_str());
            } else {
                builder.push_str(&root.as_ref().to_string_lossy());
            }
        }
        if builder.is_empty() {
            return "(empty)".to_owned();
        }
        builder
    }

    #[instrument(skip_all)]
    async fn load_file(&self, uri: &str, peer: &Peer<RoleServer>) -> Result<Vec<u8>> {
        let parse_as_uri = Url::parse(uri)
            .ok()
            .filter(|uri| uri.scheme().eq_ignore_ascii_case("file"))
            .and_then(|uri| uri.to_file_path().ok());
        let parse_as_path = Path::new(uri);
        let path = parse_as_uri.as_deref().unwrap_or(parse_as_path);

        let roots = Self::get_roots(peer).await;

        if parse_as_uri.is_some() || path.is_absolute() {
            let real_path = match tokio::fs::canonicalize(path).await {
                Ok(real_path) => real_path,
                Err(err) => {
                    if err.kind() == std::io::ErrorKind::NotFound {
                        bail!(std::io::Error::new(
                            std::io::ErrorKind::NotFound,
                            format!(
                                "File not found: {uri:?}\nPlease check the directory listing to confirm the correct path."
                            )
                        ));
                    } else {
                        bail!(err);
                    }
                }
            };
            if !roots.iter().any(|root| real_path.starts_with(root)) {
                let real_path_uri = Url::from_file_path(&real_path);
                let real_path_str = if let Ok(uri) = &real_path_uri {
                    Cow::from(uri.as_str())
                } else {
                    real_path.to_string_lossy()
                };
                bail!(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    format!(
                        "Access denied: {real_path_str:?}\nThe file is outside the user’s current workspace directories:\n{}",
                        Self::format_roots_as_uri(roots)
                    )
                ))
            }
            let file_data = tokio::fs::read(real_path).await?;
            Ok(file_data)
        } else {
            for root in &roots {
                let real_path = match tokio::fs::canonicalize(root.join(path)).await {
                    Ok(real_path) => real_path,
                    Err(err) => {
                        if err.kind() == std::io::ErrorKind::NotFound {
                            continue;
                        } else {
                            bail!(err);
                        }
                    }
                };
                if !real_path.starts_with(root) {
                    // Treat as not found
                    continue;
                }
                let file_data = tokio::fs::read(real_path).await?;
                return Ok(file_data);
            }
            bail!(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!(
                    "File not found: {path:?}\nPlease check the directory listing to confirm the correct path. The path should be either absolute or relative to any of the user’s current workspace directories:\n{}",
                    Self::format_roots_as_uri(roots)
                )
            ));
        }
    }

    #[instrument(skip_all)]
    async fn get_pdf_num_pages_handler(
        &self,
        params: GetPdfNumPagesParams,
        context: RequestContext<RoleServer>,
    ) -> Result<Json<GetPdfNumPagesResult>> {
        let file_data = Arc::new(self.load_file(&params.path, &context.peer).await?);
        let pdf = spawn_blocking(|| {
            Pdf::new(file_data).map_err(|err| eyre!("Failed to load PDF: {err:?}"))
        })
        .await??;
        let num_pages = pdf.pages().len();
        Ok(Json(GetPdfNumPagesResult { num_pages }))
    }

    #[allow(dead_code)]
    #[instrument(skip_all)]
    async fn read_pdf_as_images_handler(
        &self,
        params: ReadPdfAsImagesParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult> {
        let file_data = Arc::new(self.load_file(&params.path, &context.peer).await?);
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
                        x_scale: width as f32 / orig_width,
                        y_scale: height as f32 / orig_height,
                        width: Some(width),
                        height: Some(height),
                    }
                } else {
                    let width = ((image_dimension as f64 * orig_width as f64 / orig_height as f64)
                        .round() as u16)
                        .max(1);
                    let height = image_dimension.max(1);
                    RenderSettings {
                        x_scale: width as f32 / orig_width,
                        y_scale: height as f32 / orig_height,
                        width: Some(width),
                        height: Some(height),
                    }
                };

                BASE64_STANDARD
                    .encode(hayro::render(page, &interpreter_settings, &render_settings).take_png())
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

    #[instrument(skip_all)]
    async fn read_pdf_as_text_handler(
        &self,
        params: ReadPdfAsTextParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult> {
        let file_data = self.load_file(&params.path, &context.peer).await?;
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
    async fn read_pdf_page_as_image_handler(
        &self,
        params: ReadPdfPageAsImageParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult> {
        let file_data = Arc::new(self.load_file(&params.path, &context.peer).await?);
        let pdf = spawn_blocking(|| match hayro::Pdf::new(file_data) {
            Ok(ok) => Ok(Arc::new(ok)),
            Err(err) => bail!("Failed to load PDF: {err:?}"),
        })
        .await??;

        let page_num = params.page;
        let image_dimension = params.image_dimension;

        let image = spawn_blocking(move || {
            let pages = pdf.pages();
            let Some(page) = page_num.checked_sub(1).and_then(|x| pages.get(x)) else {
                bail!(
                    "Page number {} is out of range (1–{})",
                    page_num,
                    pages.len()
                );
            };

            let interpreter_settings = InterpreterSettings::default();

            let (orig_width, orig_height) = page.render_dimensions();
            let render_settings = if orig_width >= orig_height {
                let width = image_dimension.max(1);
                let height = ((image_dimension as f64 * orig_height as f64 / orig_width as f64)
                    .round() as u16)
                    .max(1);
                RenderSettings {
                    x_scale: width as f32 / orig_width,
                    y_scale: height as f32 / orig_height,
                    width: Some(width),
                    height: Some(height),
                }
            } else {
                let width = ((image_dimension as f64 * orig_width as f64 / orig_height as f64)
                    .round() as u16)
                    .max(1);
                let height = image_dimension.max(1);
                RenderSettings {
                    x_scale: width as f32 / orig_width,
                    y_scale: height as f32 / orig_height,
                    width: Some(width),
                    height: Some(height),
                }
            };

            Ok(BASE64_STANDARD
                .encode(hayro::render(page, &interpreter_settings, &render_settings).take_png()))
        })
        .await??;

        Ok(CallToolResult::success(vec![
            Content::image(image, "image/png").with_audience(vec![Role::Assistant]),
        ]))
    }
}

#[rmcp::tool_router]
impl PdflensService {
    #[rmcp::tool(
        description = "Get the number of pages in a PDF.",
        annotations(read_only_hint = true),
        output_schema = schema_for_type::<GetPdfNumPagesResult>()
    )]
    pub async fn get_pdf_num_pages(
        &self,
        Parameters(params): Parameters<GetPdfNumPagesParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        self.get_pdf_num_pages_handler(params, context)
            .await
            .map_or_else(
                |err| {
                    tracing::error!("{err}");
                    Ok(CallToolResult::error(vec![
                        Content::text(format!("{err:#}")).with_audience(vec![Role::Assistant]),
                    ]))
                },
                |ok| ok.into_call_tool_result(),
            )
    }

    #[cfg_attr(not(feature = "enable_multi_images"), allow(dead_code))]
    #[cfg_attr(
        feature = "enable_multi_images",
        rmcp::tool(
            description = "Read one page of a PDF as an image. The output contains one image per page. Performance recommendation: Only use this tool on specific pages after reading the text version.",
            annotations(read_only_hint = true)
        )
    )]
    pub async fn read_pdf_as_images(
        &self,
        Parameters(params): Parameters<ReadPdfAsImagesParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        self.read_pdf_as_images_handler(params, context)
            .await
            .or_else(|err| {
                tracing::error!("{err}");
                Ok(CallToolResult::error(vec![
                    Content::text(format!("{err:#}")).with_audience(vec![Role::Assistant]),
                ]))
            })
    }

    #[rmcp::tool(
        description = "Read a PDF in plain text format. The output separates each page with “\x0c” (U+000C). Performance recommendation: if numPages < 1000, read from first page to last page; otherwise, read in chunks of 1000 pages.",
        annotations(read_only_hint = true)
    )]
    pub async fn read_pdf_as_text(
        &self,
        Parameters(params): Parameters<ReadPdfAsTextParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        self.read_pdf_as_text_handler(params, context)
            .await
            .or_else(|err| {
                tracing::error!("{err}");
                Ok(CallToolResult::error(vec![
                    Content::text(format!("{err:#}")).with_audience(vec![Role::Assistant]),
                ]))
            })
    }

    #[rmcp::tool(
        description = "Read one page of a PDF as an image. You may call this tool multiple times in parallel to read multiple pages.",
        annotations(read_only_hint = true)
    )]
    pub async fn read_pdf_page_as_image(
        &self,
        Parameters(params): Parameters<ReadPdfPageAsImageParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        self.read_pdf_page_as_image_handler(params, context)
            .await
            .or_else(|err| {
                tracing::error!("{err}");
                Ok(CallToolResult::error(vec![
                    Content::text(format!("{err:#}")).with_audience(vec![Role::Assistant]),
                ]))
            })
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
}
