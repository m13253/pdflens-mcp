use eyre::Result;
use rmcp::handler::server::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, Root, ServerCapabilities, ServerInfo};
use rmcp::service::NotificationContext;
use rmcp::{Peer, RoleServer, ServerHandler};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

pub struct PdflensService {
    tool_router: ToolRouter<Self>,
    roots: RwLock<Vec<Root>>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct ReadPdfParams {
    pub filename: String,
    pub page_from: u64,
    pub page_to: u64,
}

#[rmcp::tool_router]
impl PdflensService {
    pub fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
            roots: RwLock::new(Vec::new()),
        }
    }

    #[tracing::instrument(skip_all)]
    async fn update_roots(&self, peer: &Peer<RoleServer>) {
        tracing::info!("Updating roots");
        let Some(peer_info) = peer.peer_info() else {
            return;
        };
        if peer_info.capabilities.roots.is_none() {
            return;
        }
        let roots = match peer.list_roots().await {
            Ok(roots) => roots.roots,
            Err(err) => {
                tracing::error!("{}", err);
                return;
            }
        };
        tracing::info!("roots: {:?}", roots);
        *self.roots.write().await = roots;
    }

    #[rmcp::tool(description = "Convert PDF to text")]
    pub async fn pdf_to_text(
        &self,
        Parameters(params): Parameters<ReadPdfParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        todo!()
    }

    #[rmcp::tool(description = "Convert PDF to images")]
    pub async fn pdf_to_images(
        &self,
        Parameters(params): Parameters<ReadPdfParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        todo!()
    }
}

#[rmcp::tool_handler]
impl ServerHandler for PdflensService {
    #[tracing::instrument(skip_all)]
    fn get_info(&self) -> ServerInfo {
        tracing::info!("get_info");
        ServerInfo {
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            instructions: Some("A tool to read PDF files".to_string()),
            ..Default::default()
        }
    }

    #[tracing::instrument(skip_all)]
    async fn on_roots_list_changed(&self, context: NotificationContext<RoleServer>) {
        self.update_roots(&context.peer).await;
    }
}
