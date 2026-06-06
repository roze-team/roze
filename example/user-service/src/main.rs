mod client;
mod config;
mod handler;
mod logic;
mod pb;
mod rpc;
mod svc;
mod types;

use roze_core::rest::RestServer;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    roze_core::init_tracing();

    let config = config::load("config.yaml")?;
    let rest = config
        .rest
        .clone()
        .ok_or_else(|| anyhow::anyhow!("missing rest config"))?;
    let ctx = svc::ServiceContext::new(config).await?;
    let app = roze_core::middleware::apply_common(handler::router(ctx));
    RestServer::new(rest.addr, app).serve().await?;

    Ok(())
}
