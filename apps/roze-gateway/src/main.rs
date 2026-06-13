mod config;

use std::sync::Arc;

use poem::{Request, Response, Result};
use tokio::sync::{mpsc, RwLock};
use tracing::{info, warn};

struct DynamicGatewayEndpoint<E> {
    current: Arc<RwLock<RouteEntry<E>>>,
}

impl<E> Clone for DynamicGatewayEndpoint<E> {
    fn clone(&self) -> Self {
        Self {
            current: self.current.clone(),
        }
    }
}

struct RouteEntry<E> {
    route: E,
}

impl<E> DynamicGatewayEndpoint<E> {
    fn new(initial: E) -> Self {
        Self {
            current: Arc::new(RwLock::new(RouteEntry { route: initial })),
        }
    }

    async fn set_route(&self, route: E) {
        let mut guard = self.current.write().await;
        *guard = RouteEntry { route };
    }
}

impl<E> poem::Endpoint for DynamicGatewayEndpoint<E>
where
    E: poem::Endpoint<Output = Response> + Send + Sync + 'static,
{
    type Output = Response;

    fn call(&self, req: Request) -> impl std::future::Future<Output = Result<Self::Output>> + Send {
        let this = self.current.clone();
        async move {
            let guard = this.read().await;
            guard.route.call(req).await
        }
    }
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
    let listen = gateway
        .listen
        .unwrap_or_else(|| "127.0.0.1:8081".parse().expect("default addr"));
    let initial_gateway_signature = route_signature(&gateway);

    let initial_router = roze_gateway::build_router(gateway.clone(), jwt.clone());
    let dynamic_router = DynamicGatewayEndpoint::new(initial_router);
    let signature_guard = Arc::new(RwLock::new(initial_gateway_signature));

    if let Some(center) = center.clone() {
        let (config_tx, mut config_rx) = mpsc::unbounded_channel::<config::Config>();
        let center_router = dynamic_router.clone();
        let center_jwt = jwt.clone();
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

                let next_signature = route_signature(&updated_gateway);
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

                let next = roze_gateway::build_router(updated_gateway, center_jwt.clone());
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
        center
            .add_reload_listener(|result| {
                if let Some(error) = &result.error {
                    tracing::warn!(
                        event = "gateway.config.reload.failed",
                        version = result.version,
                        old_version = result.old_version,
                        hash = %result.hash,
                        old_hash = %result.old_hash,
                        source = %result.source,
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
                        "gateway config reload applied"
                    );
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

fn route_signature(config: &roze_config::GatewayConfig) -> String {
    serde_json::to_string(config).unwrap_or_else(|_| String::new())
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
