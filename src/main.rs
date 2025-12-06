mod param;
mod service;

use crate::service::PdflensService;
use eyre::Result;
use rmcp::ServiceExt;
use tracing_subscriber::{EnvFilter, prelude::*};

#[tokio::main]
async fn main() -> Result<()> {
    color_eyre::install()?;
    tracing_subscriber::registry()
        .with(tracing_error::ErrorLayer::default())
        .with({
            let mut filter = EnvFilter::new(concat!("warn,", env!("CARGO_CRATE_NAME"), "=info"));
            if let Some(env) = std::env::var_os(EnvFilter::DEFAULT_ENV) {
                for segment in env.to_string_lossy().split(',') {
                    if let Ok(directive) = segment.parse() {
                        filter = filter.add_directive(directive);
                    }
                }
            }
            filter
        })
        .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
        .init();

    let service = PdflensService::new()
        .serve(rmcp::transport::stdio())
        .await?;
    service.waiting().await?;

    Ok(())
}
