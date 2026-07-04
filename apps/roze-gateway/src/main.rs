mod config;

use std::sync::Arc;

use axum::{
    body::Body,
    extract::State,
    http::{Request, Response, StatusCode},
    routing::any,
    Router,
};
use tokio::sync::{mpsc, RwLock};
use tower::ServiceExt;
use tracing::{info, warn};

struct DynamicGatewayRouter {
    current: Arc<RwLock<Router>>,
}

impl Clone for DynamicGatewayRouter {
    fn clone(&self) -> Self {
        Self {
            current: self.current.clone(),
        }
    }
}

impl DynamicGatewayRouter {
    fn new(initial: Router) -> Self {
        Self {
            current: Arc::new(RwLock::new(initial)),
        }
    }

    async fn set_route(&self, route: Router) {
        let mut guard = self.current.write().await;
        *guard = route;
    }
}

async fn dynamic_gateway_handler(
    State(dynamic): State<DynamicGatewayRouter>,
    req: Request<Body>,
) -> Response<Body> {
    let router = dynamic.current.read().await.clone();
    router.oneshot(req).await.unwrap_or_else(|err| {
        Response::builder()
            .status(StatusCode::INTERNAL_SERVER_ERROR)
            .body(Body::from(err.to_string()))
            .expect("gateway dispatch error response")
    })
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let (config, center) = config::load_with_config_center_with_center(config_path()).await?;
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
    let initial_gateway_signature = route_signature(&gateway, &config.governance);
    let registry = roze_rpc::registry::build_service_registry(&config)?;
    let config_history = roze_admin::ConfigReloadHistory::new(128);

    let initial_router = roze_gateway::build_router_with_registry_governance_and_auth(
        gateway.clone(),
        jwt.clone(),
        api_keys.clone(),
        registry.clone(),
        Some(config.governance.clone()),
    );
    let dynamic_gateway = DynamicGatewayRouter::new(initial_router);
    let admin_router = build_admin_router(registry.clone(), config_history.clone());
    let dynamic_router = admin_router.merge(
        Router::new()
            .fallback(any(dynamic_gateway_handler))
            .with_state(dynamic_gateway.clone()),
    );
    let signature_guard = Arc::new(RwLock::new(initial_gateway_signature));

    if let Some(center) = center.clone() {
        let (config_tx, mut config_rx) = mpsc::unbounded_channel::<config::Config>();
        let center_router = dynamic_gateway.clone();
        let center_registry = registry.clone();
        let center_signature = signature_guard.clone();
        let center_listen = listen;

        center
            .add_listener(move |updated| {
                let _ = config_tx.send(updated.clone());
            })
            .await;

        tokio::spawn(async move {
            while let Some(updated_config) = config_rx.recv().await {
                let Some(updated_gateway) = updated_config.gateway else {
                    warn!(
                        event = "gateway.config.reload",
                        message = "missing gateway config in hot-reload payload"
                    );
                    continue;
                };

                let next_signature = route_signature(&updated_gateway, &updated_config.governance);
                let mut current_signature = center_signature.write().await;
                if *current_signature == next_signature {
                    tracing::debug!(
                        event = "gateway.config.hot_reloaded.skipped",
                        listen = %center_listen,
                        signature = %next_signature,
                        reason = "gateway signature unchanged",
                        "gateway route table unchanged, skip rebuild"
                    );
                    continue;
                }

                let next_api_keys = updated_config
                    .auth
                    .as_ref()
                    .and_then(|auth| auth.api_keys.clone());
                let next_jwt = updated_config.auth.as_ref().map(roze_jwt::JwtConfig::from);
                let next = roze_gateway::build_router_with_registry_governance_and_auth(
                    updated_gateway,
                    next_jwt,
                    next_api_keys,
                    center_registry.clone(),
                    Some(updated_config.governance),
                );
                center_router.set_route(next).await;
                *current_signature = next_signature.clone();

                tracing::info!(
                    event = "gateway.config.hot_reloaded",
                    listen = %center_listen,
                    signature = %next_signature,
                    "gateway route table refreshed"
                );
            }
        });
    }

    if let Some(center) = center {
        let reload_history = config_history.clone();
        center
            .add_reload_listener(move |result| {
                reload_history.record(result);
                let diff_paths = result
                    .diff
                    .iter()
                    .map(|entry| entry.path.as_str())
                    .collect::<Vec<_>>()
                    .join(",");
                if let Some(error) = &result.error {
                    tracing::warn!(
                        event = "gateway.config.reload.failed",
                        version = result.version,
                        old_version = result.old_version,
                        hash = %result.hash,
                        old_hash = %result.old_hash,
                        source = %result.source,
                        diff_paths = %diff_paths,
                        error = %error,
                        "gateway config reload failed"
                    );
                } else {
                    tracing::info!(
                        event = "gateway.config.reload.applied",
                        version = result.version,
                        old_version = result.old_version,
                        hash = %result.hash,
                        old_hash = %result.old_hash,
                        source = %result.source,
                        changed = result.changed,
                        diff_paths = %diff_paths,
                        "gateway config reload applied"
                    );
                    for event in result.change_events() {
                        tracing::info!(
                            event = "gateway.config_updated",
                            version = event.version,
                            old_version = event.old_version,
                            source = %event.source,
                            section = %event.section,
                            section_hash = event.section_hash.as_deref().unwrap_or_default(),
                            paths = %event.paths.join(","),
                            changed = event.changed,
                            "gateway config section updated"
                        );
                    }
                }
            })
            .await;
    }

    info!(
        addr = %listen,
        "start roze-gateway with dynamic route table"
    );
    roze_http::rest::RestServer::new(listen, dynamic_router)
        .serve()
        .await?;
    Ok(())
}

fn build_admin_router(
    registry: Option<Arc<dyn roze_rpc::registry::Registry>>,
    history: roze_admin::ConfigReloadHistory,
) -> Router {
    let mut state = roze_admin::AdminState::new().with_config_history(history);
    if let Some(registry) = registry {
        state = state.with_registry(roze_admin::RegistryAdmin::new(registry));
    }
    if let Some(auth) = roze_admin::AdminAuthConfig::from_env() {
        state = state.with_auth(auth);
    }
    roze_admin::admin_router(state)
}

fn route_signature(
    gateway: &roze_config::GatewayConfig,
    governance: &roze_config::GovernanceConfig,
) -> String {
    serde_json::to_string(&(gateway, governance)).unwrap_or_else(|_| String::new())
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
