mod config;
mod handler;
mod kafka;
mod logic;
mod middleware;
mod openapi;
mod svc;
mod types;

use roze_http::rest::RestServer;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
use tokio::time::{sleep, timeout, Duration};

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
    let config_history = roze_admin::ConfigReloadHistory::new(128);
    let registry = roze_rpc::registry::build_service_registry(&config)?;
    if let Some(center) = center {
        let config_tx_for_reload = config_tx.clone();
        center
            .add_listener(move |updated| {
                let _ = config_tx_for_reload.send(updated.clone());
            })
            .await;

        let reload_version_for_listener = reload_version.clone();
        let reload_history = config_history.clone();
        center
            .add_reload_listener(move |result| {
                reload_version_for_listener.store(result.version, Ordering::SeqCst);
                reload_history.record(result);
                let diff_paths = result
                    .diff
                    .iter()
                    .map(|entry| entry.path.as_str())
                    .collect::<Vec<_>>()
                    .join(",");
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
                        diff_paths = %diff_paths,
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
                        diff_paths = %diff_paths,
                        success = result.success,
                        ts_millis = result.ts_millis,
                        "config center reload applied"
                    );
                    for event in result.change_events() {
                        tracing::info!(
                            event = "config_updated",
                            version = event.version,
                            old_version = event.old_version,
                            source = %event.source,
                            section = %event.section,
                            section_hash = event.section_hash.as_deref().unwrap_or_default(),
                            paths = %event.paths.join(","),
                            changed = event.changed,
                            "config center section updated"
                        );
                    }
                }
            })
            .await;
    }

    let app_name = config.name.clone();
    let mut kafka_task = kafka::start_center_driven_kafka(
        config_rx.clone(),
        reload_version.clone(),
        app_name.clone(),
    );

    let mut registration = if rest.register {
        let registry = registry
            .clone()
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
    let ctx = svc::ServiceContext::new(config.clone()).await?;
    let _config_history = config_history;
    let app = roze_middleware::apply_common(handler::router(ctx));
    RestServer::new(rest.addr, app).serve().await?;
    if let Some(registration) = registration.as_mut() {
        registration.shutdown().await?;
    }

    drop(config_tx);
    tracing::info!(
        app = %app_name,
        event = "kafka.runtime.shutdown_requested",
        "kafka background runtime shutdown requested"
    );

    tokio::select! {
        result = &mut kafka_task => {
            if let Err(err) = result {
                tracing::warn!(
                    app = %app_name,
                    event = "kafka.runtime.stop_error",
                    error = %err,
                    "kafka background runtime stopped with error"
                );
            }
        }
        _ = sleep(Duration::from_secs(5)) => {
            tracing::warn!(
                app = %app_name,
                event = "kafka.runtime.stop_timeout",
                "kafka background runtime stop timeout, forcing abort"
            );
            kafka_task.abort();
            let _ = timeout(Duration::from_secs(2), &mut kafka_task).await;
        }
    }

    Ok(())
}

fn config_path() -> std::path::PathBuf {
    roze_config::service_config_path(env!("CARGO_MANIFEST_DIR"))
}
