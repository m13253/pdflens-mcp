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
use rmcp::handler::server::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CallToolResult, Content, ProgressNotificationParam, Role, ServerCapabilities, ServerInfo,
};
use rmcp::service::{NotificationContext, RequestContext};
use rmcp::{Json, Peer, RoleServer, ServerHandler};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::instrument;

pub struct PdflensService {
    tool_router: ToolRouter<Self>,
    roots: RwLock<Vec<PathBuf>>,
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[schemars(title = "get_pdf_num_pages")]
pub struct GetPdfNumPagesParams {
    #[schemars(
        description = "Either an absolute path starting with file:/// or a path relative to any MCP root paths"
    )]
    pub filename: String,
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[schemars(title = "read_pdf_as_text")]
pub struct PdfToTextParams {
    #[schemars(
        description = "Either an absolute path starting with file:/// or a path relative to any MCP root paths"
    )]
    pub filename: String,
    #[schemars(description = "If omitted, reads from the beginning")]
    pub from_page: Option<usize>,
    #[schemars(description = "If omitted or larger than the total pages, reads until the end")]
    pub to_page: Option<usize>,
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[schemars(title = "read_pdf_as_images")]
pub struct PdfToImagesParams {
    #[schemars(
        description = "Either an absolute path starting with file:/// or a path relative to any MCP root paths"
    )]
    pub filename: String,
    #[schemars(description = "If omitted, reads from the beginning")]
    pub from_page: Option<usize>,
    #[schemars(description = "If omitted or larger than the total pages, reads until the end")]
    pub to_page: Option<usize>,
    #[schemars(description = "Number of pixels on the longer side of the output image")]
    pub image_dimension: u16,
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[repr(transparent)]
#[schemars(title = "list_mcp_root_paths_results")]
pub struct ListMcpRootPathsResults {
    pub roots: Vec<String>,
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
        description = "Use this tool to diagnose file-not-found errors. MCP root paths are usually the user’s workspace paths"
    )]
    pub async fn list_mcp_root_paths(
        &self,
    ) -> Result<Json<ListMcpRootPathsResults>, rmcp::ErrorData> {
        Ok(self.list_mcp_root_paths_handler().await)
    }

    #[rmcp::tool(description = "Gets the number of pages in a PDF")]
    pub async fn get_pdf_num_pages(
        &self,
        Parameters(params): Parameters<GetPdfNumPagesParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        self.get_pdf_num_pages_handler(&params)
            .await
            .or_else(|err| {
                tracing::error!("{err:?}");
                Ok(CallToolResult::error(vec![
                    Content::text(format!("{err:#}")).with_audience(vec![Role::Assistant]),
                ]))
            })
    }

    #[rmcp::tool(
        description = "Prefer this tool to read PDFs. If the PDF is short, please read it in whole. If the PDF has thousands of pages, please successively use this tool every 10–100 pages. You decide the amount regarding your processing capabilities"
    )]
    pub async fn read_pdf_as_text(
        &self,
        Parameters(params): Parameters<PdfToTextParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        self.read_pdf_as_text_handler(&params).await.or_else(|err: eyre::Error| {
            tracing::error!("{err:?}");
            Ok(CallToolResult::error(vec![
                Content::text(format!("{err:#}")).with_audience(vec![Role::Assistant]),
            ]))
        })
    }

    #[rmcp::tool(
        description = "Use this tool to understand the page layout or to see a specific figure. You need vision capabilities to use this tool. Please only use for pages of interest because it’s slower"
    )]
    pub async fn read_pdf_as_images(
        &self,
        Parameters(params): Parameters<PdfToImagesParams>,
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
    async fn list_mcp_root_paths_handler(&self) -> Json<ListMcpRootPathsResults> {
        const ESCAPE_SET: &AsciiSet = &CONTROLS.add(b' ').add(b'#').add(b'?');

        Json(ListMcpRootPathsResults {
            roots: self
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
                    format!("File not found: {uri:?}\nUse `list_mcp_root_paths` to diagnose")
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
                                "File not found: {filename:?}\nUse `list_mcp_root_paths` to diagnose"
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
                        "File isn’t within any MCP root paths: {path:?}\nUse `list_mcp_root_paths` to diagnose"
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
                format!("File not found: {uri:?}\nUse `list_mcp_root_paths` to diagnose")
            ));
        }
    }

    #[instrument(skip_all)]
    async fn get_pdf_num_pages_handler(
        &self,
        params: &GetPdfNumPagesParams,
    ) -> Result<CallToolResult> {
        let file_data = Arc::new(self.load_file(&params.filename).await?);
        let pdf = Pdf::new(file_data).map_err(|err| eyre!("Failed to load PDF: {err:?}"))?;
        let page_count = pdf.pages().len();
        Ok(CallToolResult::success(vec![
            Content::text(page_count.to_string()).with_audience(vec![Role::Assistant]),
        ]))
    }

    #[instrument(skip_all)]
    async fn read_pdf_as_text_handler(&self, params: &PdfToTextParams) -> Result<CallToolResult> {
        let file_data = self.load_file(&params.filename).await?;
        let mut text = extract_text_from_mem_by_pages(&file_data)?;

        let from_page_idx = params
            .from_page
            .map(|x| x.saturating_sub(1).min(text.len()))
            .unwrap_or_default();
        let to_page_idx = params
            .to_page
            .map(|x| x.clamp(from_page_idx, text.len()))
            .unwrap_or(text.len());

        text.truncate(to_page_idx);
        text.drain(..from_page_idx);

        Ok(CallToolResult::success(vec![
            Content::text(text.join("\x0c")).with_audience(vec![Role::Assistant]),
        ]))
    }

    #[instrument(skip_all)]
    async fn read_pdf_as_images_handler(
        &self,
        params: &PdfToImagesParams,
        context: &RequestContext<RoleServer>,
    ) -> Result<CallToolResult> {
        let file_data = Arc::new(self.load_file(&params.filename).await?);
        let pdf = hayro::Pdf::new(file_data).map_err(|err| eyre!("Failed to load PDF: {err:?}"))?;
        let pages = pdf.pages();

        // 0-based indices, half-closed half-open
        let from_page_idx = params
            .from_page
            .map(|x| x.saturating_sub(1).min(pages.len()))
            .unwrap_or_default();
        let to_page_idx = params
            .to_page
            .map(|x| x.clamp(from_page_idx, pages.len()))
            .unwrap_or(pages.len());
        let page_count = to_page_idx - from_page_idx;

        let interpreter_settings = InterpreterSettings::default();

        let progress_token = context.meta.get_progress_token();
        let mut content = Vec::with_capacity(page_count);
        for (i, page) in pdf.pages()[from_page_idx..to_page_idx]
            .into_iter()
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

            let (orig_width, orig_height) = page.render_dimensions();
            let render_settings = if orig_width >= orig_height {
                let width = params.image_dimension.max(1);
                let height = ((params.image_dimension as f64 * orig_height as f64
                    / orig_width as f64)
                    .round() as u16)
                    .max(1);
                RenderSettings {
                    x_scale: width as f32 / orig_width as f32,
                    y_scale: height as f32 / orig_height as f32,
                    width: Some(width),
                    height: Some(height),
                }
            } else {
                let width = ((params.image_dimension as f64 * orig_width as f64
                    / orig_height as f64)
                    .round() as u16)
                    .max(1);
                let height = params.image_dimension.max(1);
                RenderSettings {
                    x_scale: width as f32 / orig_width as f32,
                    y_scale: height as f32 / orig_height as f32,
                    width: Some(width),
                    height: Some(height),
                }
            };

            let pixmap = hayro::render(&page, &interpreter_settings, &render_settings);
            content.push(
                Content::image(BASE64_STANDARD.encode(pixmap.take_png()), "image/png")
                    .with_audience(vec![Role::Assistant]),
            );
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
            instructions: Some("A tool for reading PDF files".to_string()),
            ..Default::default()
        }
    }

    #[tracing::instrument(skip_all)]
    async fn on_roots_list_changed(&self, context: NotificationContext<RoleServer>) {
        self.update_roots(&context.peer).await;
    }
}
