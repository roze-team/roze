mod config;

use tracing::info;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let (config, center) = config::load_with_config_center_with_center(config_path()).await?;
    roze_log::init_tracing_with_config(&config)?;

    let gateway = config
        .gateway
        .clone()
        .ok_or_else(|| anyhow::anyhow!("missing gateway config"))?;
    roze_gateway::validate_gateway_config(&gateway)?;
    let jwt = config.auth.as_ref().map(roze_jwt::JwtConfig::from);
    let api_keys = config.auth.as_ref().and_then(|auth| auth.api_keys.clone());
    let listen = gateway
        .listen
        .unwrap_or_else(|| "127.0.0.1:8081".parse().expect("default addr"));
    let registry = roze_rpc::registry::build_service_registry(&config)?;
    let service = roze_gateway::try_build_router_with_registry_governance_and_auth(
        gateway,
        jwt,
        api_keys,
        registry,
        Some(config.governance),
    )?;
    if let Some(center) = center {
        let reload_service = service.clone();
        center
            .add_reload_listener(move |result| {
                if !result.success {
                    roze_gateway::record_reload_outcome(roze_gateway::GatewayReloadOutcome::Failed);
                    tracing::warn!(
                        event = "gateway.config.reload.failed",
                        version = result.version,
                        old_version = result.old_version,
                        hash = %result.hash,
                        error = %result.error.as_deref().unwrap_or("config reload failed"),
                        "gateway keeps the last valid runtime snapshot"
                    );
                    return;
                }
                if !gateway_reload_relevant(&result.diff) {
                    roze_gateway::record_reload_outcome(
                        roze_gateway::GatewayReloadOutcome::Skipped,
                    );
                    tracing::info!(
                        event = "gateway.config.hot_reloaded.skipped",
                        version = result.version,
                        hash = %result.hash,
                        "gateway runtime sections are unchanged"
                    );
                    return;
                }
                let Some(updated) = result.config.as_ref() else {
                    roze_gateway::record_reload_outcome(roze_gateway::GatewayReloadOutcome::Failed);
                    tracing::warn!(
                        event = "gateway.config.reload.failed",
                        version = result.version,
                        "successful reload result has no config snapshot"
                    );
                    return;
                };
                let Some(gateway) = updated.gateway.clone() else {
                    roze_gateway::record_reload_outcome(roze_gateway::GatewayReloadOutcome::Failed);
                    tracing::warn!(
                        event = "gateway.config.reload.failed",
                        version = result.version,
                        "reloaded config removed the gateway section"
                    );
                    return;
                };
                let updated_listen = gateway.listen.unwrap_or(listen);
                if updated_listen != listen {
                    roze_gateway::record_reload_outcome(roze_gateway::GatewayReloadOutcome::Failed);
                    tracing::warn!(
                        event = "gateway.config.reload.failed",
                        version = result.version,
                        current_addr = %listen,
                        requested_addr = %updated_listen,
                        "listen address changes require a process restart"
                    );
                    return;
                }
                let registry = match roze_rpc::registry::build_service_registry(updated) {
                    Ok(registry) => registry,
                    Err(error) => {
                        roze_gateway::record_reload_outcome(
                            roze_gateway::GatewayReloadOutcome::Failed,
                        );
                        tracing::warn!(
                            event = "gateway.config.reload.failed",
                            version = result.version,
                            error = %error,
                            "gateway registry reload failed"
                        );
                        return;
                    }
                };
                let jwt = updated.auth.as_ref().map(roze_jwt::JwtConfig::from);
                let api_keys = updated.auth.as_ref().and_then(|auth| auth.api_keys.clone());
                match reload_service.reload(
                    gateway,
                    jwt,
                    api_keys,
                    registry,
                    Some(updated.governance.clone()),
                ) {
                    Ok(()) => {
                        roze_gateway::record_reload_outcome(
                            roze_gateway::GatewayReloadOutcome::Applied,
                        );
                        tracing::info!(
                            event = "gateway.config.hot_reloaded",
                            version = result.version,
                            old_version = result.old_version,
                            hash = %result.hash,
                            old_hash = %result.old_hash,
                            "gateway runtime snapshot atomically replaced"
                        );
                    }
                    Err(error) => {
                        roze_gateway::record_reload_outcome(
                            roze_gateway::GatewayReloadOutcome::Failed,
                        );
                        tracing::warn!(
                            event = "gateway.config.reload.failed",
                            version = result.version,
                            error = %error,
                            "gateway keeps the last valid runtime snapshot"
                        );
                    }
                }
            })
            .await;
    }

    info!(addr = %listen, "start roze-gateway native HTTP service");
    roze_http::rest::RestServer::new(listen, service)
        .serve()
        .await?;
    Ok(())
}

fn gateway_reload_relevant(diff: &[roze_config::ConfigDiffEntry]) -> bool {
    diff.iter().any(|entry| {
        let section = entry
            .path
            .trim_start_matches(['$', '.'])
            .split(['.', '['])
            .next()
            .unwrap_or_default();
        matches!(section, "gateway" | "auth" | "governance" | "registry")
    })
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
