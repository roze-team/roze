mod config;

use tracing::info;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let (config, _center) = config::load_with_config_center_with_center(config_path()).await?;
    roze_log::init_tracing_with_config(&config)?;

    let gateway = config
        .gateway
        .clone()
        .ok_or_else(|| anyhow::anyhow!("missing gateway config"))?;
    let jwt = config.auth.as_ref().map(roze_jwt::JwtConfig::from);
    let api_keys = config.auth.as_ref().and_then(|auth| auth.api_keys.clone());
    let listen = gateway
        .listen
        .unwrap_or_else(|| "127.0.0.1:8081".parse().expect("default addr"));
    let registry = roze_rpc::registry::build_service_registry(&config)?;
    let service = roze_gateway::build_router_with_registry_governance_and_auth(
        gateway,
        jwt,
        api_keys,
        registry,
        Some(config.governance),
    );

    info!(addr = %listen, "start roze-gateway native HTTP service");
    roze_http::rest::RestServer::new(listen, service)
        .serve()
        .await?;
    Ok(())
}

fn config_path() -> std::path::PathBuf {
    if let Ok(path) = std::env::var("ROZE_GATEWAY_CONFIG_FILE") {
        return std::path::PathBuf::from(path);
    }

    let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let manifest_config = manifest_dir.join("config.yaml");
    if manifest_config.exists() {
        manifest_config
    } else {
        std::path::PathBuf::from("config.yaml")
    }
}
