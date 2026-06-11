mod config;
mod handler;
mod logic;
mod middleware;
mod openapi;
mod kafka;
mod svc;
mod types;

use roze_http::rest::RestServer;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
use tokio::time::{timeout, Duration};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let (config, center) = config::load_with_config_center_with_center(config_path()).await?;
    roze_log::init_tracing_with_config(&config)?;
    let rest = config
        .rest
        .clone()
        .ok_or_else(|| anyhow::anyhow!("missing rest config"))?;
    let (config_tx, config_rx) = tokio::sync::watch::channel(config.clone());
    let reload_version = Arc::new(AtomicU64::new(0));
    if let Some(center) = center {
        let config_tx_for_reload = config_tx.clone();
        center
            .add_listener(move |updated| {
                let _ = config_tx_for_reload.send(updated.clone());
            })
            .await;

        let reload_version_for_listener = reload_version.clone();
        center
            .add_reload_listener(move |result| {
                reload_version_for_listener.store(result.version, Ordering::SeqCst);
                if let Some(error) = &result.error {
                    tracing::warn!(
                        event = "config.reload.failed",
                        version = result.version,
                        old_version = result.old_version,
                        hash = %result.hash,
                        old_hash = %result.old_hash,
                        source = %result.source,
                        namespace = result.namespace.as_deref().unwrap_or_default(),
                        app = result.app.as_deref().unwrap_or_default(),
                        key = result.key.as_deref().unwrap_or_default(),
                        changed = result.changed,
                        ts_millis = result.ts_millis,
                        error = %error,
                        "config center reload failed"
                    );
                } else {
                    tracing::info!(
                        event = "config.reload.applied",
                        version = result.version,
                        old_version = result.old_version,
                        hash = %result.hash,
                        old_hash = %result.old_hash,
                        source = %result.source,
                        namespace = result.namespace.as_deref().unwrap_or_default(),
                        app = result.app.as_deref().unwrap_or_default(),
                        key = result.key.as_deref().unwrap_or_default(),
                        changed = result.changed,
                        success = result.success,
                        ts_millis = result.ts_millis,
                        "config center reload applied"
                    );
                }
            })
            .await;
    }

    let kafka_task = kafka::start_center_driven_kafka(
        config_rx.clone(),
        reload_version.clone(),
        config.name.clone(),
    );

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

    drop(config_tx);
    tracing::info!(
        app = %config.name,
        event = "kafka.runtime.shutdown_requested",
        "kafka background runtime shutdown requested"
    );

    match timeout(Duration::from_secs(5), kafka_task).await {
        Ok(result) => {
            if let Err(err) = result {
                tracing::warn!(
                    app = %config.name,
                    event = "kafka.runtime.stop_error",
                    error = %err,
                    "kafka background runtime stopped with error"
                );
            }
        }
        Err(_) => {
            tracing::warn!(
                app = %config.name,
                event = "kafka.runtime.stop_timeout",
                "kafka background runtime stop timeout, forcing abort"
            );
            kafka_task.abort();
            let _ = timeout(Duration::from_secs(2), kafka_task).await;
        }
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
