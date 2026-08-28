use crate::svc::ServiceContext;

#[path = "config_center.rs"]
mod config_center;
#[path = "kafka.rs"]
mod kafka;

/// Stable application-owned hook for attaching data sources and other resources.
///
/// This file is preserved by `rozectl ... generate --update`.
pub async fn configure_context(ctx: ServiceContext) -> anyhow::Result<ServiceContext> {
    Ok(ctx)
}

/// Registers application-owned workers and background services.
///
/// Every registered service shares Roze's shutdown signal and failure propagation.
/// This file and the hook body are preserved by `rozectl ... generate --update`.
pub fn register_services(
    group: &mut roze_service::ServiceGroup,
    ctx: &ServiceContext,
) -> anyhow::Result<()> {
    let initial_config = ctx.config.clone();
    let service_name = ctx.config.name.clone();
    let config_path = roze_config::service_config_path(env!("CARGO_MANIFEST_DIR"));

    group.add_fn("config-center-kafka", move |shutdown| {
        let initial_config = initial_config.clone();
        let service_name = service_name.clone();
        let config_path = config_path.clone();
        async move {
            let (config_tx, config_rx) = tokio::sync::watch::channel(initial_config);
            let reload_version = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
            let kafka_task = kafka::start_center_driven_kafka(
                config_rx,
                reload_version.clone(),
                service_name.clone(),
                {
                    let shutdown = shutdown.clone();
                    async move { shutdown.wait().await }
                },
            );

            let center = config_center::open(config_path).await?;
            if let Some(center) = center.as_ref() {
                let current = center.get_config().await;
                let _ = config_tx.send(current);
                let reload_tx = config_tx.clone();
                let reload_version = reload_version.clone();
                center
                    .add_reload_listener(move |result| {
                        if !result.success {
                            tracing::warn!(
                                event = "config.reload.failed",
                                version = result.version,
                                error = result.error.as_deref().unwrap_or("unknown"),
                                "configuration reload rejected"
                            );
                            return;
                        }
                        let Some(config) = result.config.as_ref() else {
                            tracing::warn!(
                                event = "config.reload.failed",
                                version = result.version,
                                error_kind = "missing_snapshot",
                                "successful reload has no configuration snapshot"
                            );
                            return;
                        };
                        reload_version.store(result.version, std::sync::atomic::Ordering::SeqCst);
                        if reload_tx.send(config.clone()).is_err() {
                            tracing::debug!(
                                event = "config.reload.receiver_closed",
                                version = result.version,
                                "configuration reload receiver is closed"
                            );
                        }
                    })
                    .await;
                tracing::info!(
                    event = "config.center.started",
                    service = %service_name,
                    "configuration center watcher started"
                );
            } else {
                tracing::info!(
                    event = "config.center.disabled",
                    service = %service_name,
                    "configuration center is disabled"
                );
            }

            shutdown.wait().await;
            drop(center);
            drop(config_tx);
            match tokio::time::timeout(std::time::Duration::from_secs(5), kafka_task).await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => anyhow::bail!("kafka runtime task failed: {error}"),
                Err(_) => anyhow::bail!("kafka runtime did not stop within 5 seconds"),
            }
            Ok(())
        }
    });
    Ok(())
}
