mod config;
mod handler;
mod logic;
mod middleware;
mod openapi;
mod svc;
mod types;

use roze_http::rest::RestServer;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = config::load(config_path())?;
    roze_log::init_tracing_with_config(&config)?;
    let rest = config
        .rest
        .clone()
        .ok_or_else(|| anyhow::anyhow!("missing rest config"))?;
    let mut registration = if rest.register {
        let registry = roze_rpc::registry::build_service_registry(&config)?
            .ok_or_else(|| anyhow::anyhow!("missing registry config"))?;
        Some(
            roze_rpc::rpc::ServiceRegistrationGuard::start(
                registry,
                config.name.clone(),
                rest.addr,
            )
            .await?,
        )
    } else {
        None
    };
    let ctx = svc::ServiceContext::new(config).await?;
    let app = roze_middleware::apply_common(handler::router(ctx));
    RestServer::new(rest.addr, app).serve().await?;
    if let Some(registration) = registration.as_mut() {
        registration.shutdown().await?;
    }

    Ok(())
}

fn config_path() -> std::path::PathBuf {
    let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let manifest_config = manifest_dir.join("config.yaml");
    if manifest_config.exists() {
        manifest_config
    } else {
        std::path::PathBuf::from("config.yaml")
    }
}
