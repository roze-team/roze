use std::time::Duration;

use roze_kafka::Publisher;
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
use tokio::{sync::watch, task::JoinHandle};

#[derive(Debug)]
enum PipelineFailureReason {
    ProducerCreate,
    EmptyBrokers,
    Disabled,
    ConsumerSpawn { worker: u32, err: String },
    Unknown,
}

impl std::fmt::Display for PipelineFailureReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ProducerCreate => write!(f, "producer_create"),
            Self::EmptyBrokers => write!(f, "empty_brokers"),
            Self::Disabled => write!(f, "disabled"),
            Self::ConsumerSpawn { worker, err } => {
                write!(f, "consumer_spawn_worker_{worker}:{err}")
            }
            Self::Unknown => write!(f, "unknown"),
        }
    }
}

const USER_TOPIC: &str = "events";

struct RunningKafkaPipeline {
    producer: Option<roze_kafka::RdkafkaProducer>,
    consumer_handles: Vec<tokio::task::JoinHandle<()>>,
}

impl RunningKafkaPipeline {
    async fn stop(&mut self, app_name: &str, version: u64, signature: &str) {
        if self.producer.is_none() && self.consumer_handles.is_empty() {
            tracing::debug!(
                app = %app_name,
                event = "kafka.pipeline.stop_skipped",
                version = version,
                signature = %signature,
                "kafka pipeline stop skipped"
            );
            return;
        }

        tracing::info!(
            app = %app_name,
            event = "kafka.pipeline.stopping",
            version = version,
            signature = %signature,
            handles = self.consumer_handles.len(),
            "stopping existing kafka pipeline"
        );

        let mut handles = std::mem::take(&mut self.consumer_handles);
        for handle in handles.drain(..) {
            handle.abort();
            let _ = tokio::time::timeout(Duration::from_millis(500), async move {
                let _ = handle.await;
            })
            .await;
        }

        if let Some(producer) = self.producer.take() {
            if let Err(err) = producer.flush() {
                tracing::warn!(
                    app = %app_name,
                    event = "kafka.producer.flush_failed",
                    version = version,
                    signature = %signature,
                    error = %err,
                    "kafka producer flush failed"
                );
            }
            if let Err(err) = producer.close() {
                tracing::warn!(
                    app = %app_name,
                    event = "kafka.producer.close_failed",
                    version = version,
                    signature = %signature,
                    error = %err,
                    "kafka producer close failed"
                );
            }
        }

        tracing::info!(
            app = %app_name,
            event = "kafka.pipeline.stopped",
            version = version,
            signature = %signature,
            "kafka pipeline stopped"
        );
    }
}

impl Default for RunningKafkaPipeline {
    fn default() -> Self {
        Self {
            producer: None,
            consumer_handles: Vec::new(),
        }
    }
}

pub fn start_center_driven_kafka(
    mut config_rx: watch::Receiver<crate::config::Config>,
    reload_version: Arc<AtomicU64>,
    app_name: String,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut running = RunningKafkaPipeline::default();
        let mut last_signature: Option<String> = None;

        loop {
            let config = config_rx.borrow().clone();
            let version = reload_version.load(Ordering::SeqCst);
            let kafka_config = map_kafka_config(&config);
            let kafka_signature = kafka_config
                .as_ref()
                .map(serialize_signature)
                .unwrap_or_default();
            let bootstrap = kafka_config
                .as_ref()
                .map(|k| k.brokers_csv())
                .unwrap_or_default();

            if Some(&kafka_signature) != last_signature.as_ref() {
                let previous_signature = last_signature.clone().unwrap_or_default();
                last_signature = Some(kafka_signature.clone());

                tracing::info!(
                    app = %app_name,
                    event = "kafka.signature.changed",
                    version = version,
                    previous_signature = %previous_signature,
                    signature = %kafka_signature,
                    bootstrap = %bootstrap,
                    "kafka config signature changed, restart pipeline"
                );
                tracing::info!(
                    app = %app_name,
                    event = "kafka.pipeline.restarting",
                    version = version,
                    bootstrap = %bootstrap,
                    previous_signature = %previous_signature,
                    signature = %kafka_signature,
                    topic = %USER_TOPIC,
                    "kafka pipeline restart started"
                );
                restart_kafka_pipeline(
                    app_name.clone(),
                    version,
                    &bootstrap,
                    &kafka_signature,
                    &previous_signature,
                    &config,
                    &mut running,
                )
                .await;
            } else {
                tracing::debug!(
                    app = %app_name,
                    event = "kafka.signature.unchanged",
                    version = version,
                    signature = %kafka_signature,
                    bootstrap = %bootstrap,
                    "kafka config signature unchanged, skip restart"
                );
            }

            if config_rx.changed().await.is_err() {
                break;
            }
        }

        running
            .stop(
                &app_name,
                reload_version.load(Ordering::SeqCst),
                &last_signature.unwrap_or_default(),
            )
            .await;
        tracing::info!(
            app = %app_name,
            event = "kafka.runtime.stopped",
            "kafka runtime stopped"
        );
    })
}

fn serialize_signature(config: &roze_kafka::KafkaConfig) -> String {
    format!(
        "{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}",
        config.brokers_csv(),
        config.client_id_or_default(),
        config.group_id_or_default(),
        config.topic_prefix,
        config.acks,
        config.auto_offset_reset,
        config.enable_auto_commit,
        config.enable_manual_ack,
        config.linger_ms,
        config.batch_size,
        config.session_timeout_ms,
        config.heartbeat_interval_ms,
        config.max_poll_interval_ms,
        config.flush_timeout_ms,
        config.max_retries,
        config.retry_backoff_ms,
        config.retry_topic.clone().unwrap_or_default(),
        config.dead_letter_topic.clone().unwrap_or_default(),
        config.topic_regex.clone().unwrap_or_default(),
        config.consumer_workers,
    )
}

fn map_kafka_config(config: &crate::config::Config) -> Option<roze_kafka::KafkaConfig> {
    config.kafka.as_ref().map(|k| roze_kafka::KafkaConfig {
        brokers: k.brokers.clone(),
        bootstrap: k.bootstrap.clone(),
        bootstrap_servers: k.bootstrap_servers.clone(),
        client_id: Some(k.client_id.clone()),
        group_id: k.group_id.clone(),
        group: None,
        topic_prefix: k.topic_prefix.clone(),
        acks: k.acks.clone(),
        auto_offset_reset: k.auto_offset_reset.clone(),
        enable_auto_commit: k.enable_auto_commit,
        enable_manual_ack: k.enable_manual_ack,
        session_timeout_ms: k.session_timeout_ms,
        heartbeat_interval_ms: k.heartbeat_interval_ms,
        max_poll_interval_ms: k.max_poll_interval_ms,
        flush_timeout_ms: k.flush_timeout_ms,
        linger_ms: k.linger_ms,
        batch_size: k.batch_size,
        max_retries: k.max_retries,
        retry_backoff_ms: k.retry_backoff_ms,
        retry_topic: k.retry_topic.clone(),
        dead_letter_topic: k.dead_letter_topic.clone(),
        topic_regex: k.topic_regex.clone(),
        consumer_workers: k.consumer_workers,
    })
}

async fn restart_kafka_pipeline(
    app_name: String,
    version: u64,
    bootstrap: &str,
    signature: &str,
    previous_signature: &str,
    app_config: &crate::config::Config,
    running: &mut RunningKafkaPipeline,
) {
    let started_at = tokio::time::Instant::now();

    running.stop(&app_name, version, previous_signature).await;

    let bootstrap = bootstrap.to_string();

    let Some(kafka_config) = map_kafka_config(app_config) else {
        let reason = PipelineFailureReason::Disabled;
        tracing::info!(
            app = %app_name,
            event = "kafka.pipeline.disabled",
            version = version,
            bootstrap = %bootstrap,
            signature = signature,
            topic = %USER_TOPIC,
            elapsed_ms = elapsed_ms(started_at),
            reason = %format!("{reason}"),
            "kafka config missing, pipeline stopped"
        );
        return;
    };

    let group = kafka_config.group_id_or_default();
    let topic = kafka_config.topic_name(USER_TOPIC);

    if kafka_config.normalized_brokers().is_empty() {
        let reason = PipelineFailureReason::EmptyBrokers;
        tracing::warn!(
            app = %app_name,
            event = "kafka.pipeline.empty_brokers",
            version = version,
            bootstrap = %bootstrap,
            group = %group,
            signature = %signature,
            topic = %topic,
            elapsed_ms = elapsed_ms(started_at),
            reason = %format!("{reason}"),
            "kafka config exists but brokers empty, skip"
        );
        return;
    }

    let producer = match roze_kafka::RdkafkaProducer::new(kafka_config.clone()) {
        Ok(producer) => producer,
        Err(err) => {
            let err = err.to_string();
            let reason = PipelineFailureReason::ProducerCreate;
            tracing::error!(
                app = %app_name,
                error = %err,
                event = "kafka.pipeline.create_failed",
                version = version,
                bootstrap = %bootstrap,
                group = %group,
                topic = %topic,
                auto_commit = %kafka_config.enable_auto_commit,
                manual_ack = %kafka_config.enable_manual_ack,
                signature = %signature,
                elapsed_ms = elapsed_ms(started_at),
                "create rdkafka producer failed"
            );
            tracing::warn!(
                app = %app_name,
                event = "kafka.pipeline.restart_failed",
                version = version,
                bootstrap = %bootstrap,
                previous_signature = %previous_signature,
                signature = %signature,
                topic = %topic,
                error = %err,
                reason = %format!("{reason}"),
                elapsed_ms = elapsed_ms(started_at),
                "kafka pipeline failed to rebuild"
            );
            return;
        }
    };

    let auto_commit = kafka_config.enable_auto_commit;
    let manual_ack = kafka_config.enable_manual_ack;

    let subscriber = roze_kafka::RdkafkaSubscriber::new(kafka_config.clone());

    let kafka_config_for_loop = kafka_config.clone();
    let mut handles = Vec::new();
    let mut restart_failure: Option<PipelineFailureReason> = None;
    let mut spawn_failed_count = 0u32;
    let workers = kafka_config.consumer_workers.max(1);
    for worker_id in 0..workers {
        let subscriber = subscriber.clone();
        let config = kafka_config_for_loop.clone();
        let worker_app_name = app_name.clone();
        let worker_producer = producer.clone();
        let worker_topic = topic.clone();
        let worker_signature = signature.to_string();
        let worker_group = group.clone();

        let spawn_result = if config.enable_manual_ack {
            let handler_app_name = worker_app_name.clone();
            let handler_producer = worker_producer.clone();
            let handler_topic = worker_topic.clone();
            let handler_signature = worker_signature.clone();
            let handler_group = worker_group.clone();
            roze_kafka::spawn_consumer_with_auto_ack(
                &subscriber,
                worker_topic.clone(),
                move |delivery| {
                    let config = config.clone();
                    let app_name = handler_app_name.clone();
                    let producer = handler_producer.clone();
                    let topic = handler_topic.clone();
                    let signature = handler_signature.clone();
                    let group = handler_group.clone();
                    async move {
                        tracing::info!(
                            app = %app_name,
                            event = "kafka.message.received",
                            worker = worker_id,
                            topic = %delivery.message().topic,
                            key = ?delivery.message().key,
                            attempt = %delivery.message().attempt,
                            group = %group,
                            auto_commit = %config.enable_auto_commit,
                            manual_ack = %config.enable_manual_ack,
                            version = %version,
                            signature = %signature,
                            "received kafka message in user service (manual ack)"
                        );

                        if should_nack(&delivery.message().payload) {
                            tracing::warn!(
                                app = %app_name,
                                event = "kafka.message.nack",
                                worker = worker_id,
                                topic = %topic,
                                attempt = %delivery.message().attempt,
                                group = %group,
                                auto_commit = %config.enable_auto_commit,
                                manual_ack = %config.enable_manual_ack,
                                signature = %signature,
                                "business rule requires nack"
                            );

                            if let Err(err) = delivery.nack().await {
                                tracing::warn!(
                                    app = %app_name,
                                    event = "kafka.message.nack_failed",
                                    worker = worker_id,
                                    topic = %delivery.message().topic,
                                    attempt = %delivery.message().attempt,
                                    group = %group,
                                    signature = %signature,
                                    error = %err,
                                    "kafka message nack failed"
                                );
                                return Err(err);
                            }

                            tracing::warn!(
                                app = %app_name,
                                event = "kafka.message.nack_recovered",
                                worker = worker_id,
                                topic = %delivery.message().topic,
                                attempt = %delivery.message().attempt,
                                group = %group,
                                signature = %signature,
                                "kafka message nack handled and recovery path executed"
                            );
                            return Ok(());
                        }

                        let reply = json!({
                            "source": app_name,
                            "type": "processed",
                            "topic": topic,
                            "status": "ok"
                        });

                        producer
                            .publish(roze_kafka::KafkaRecord::new(
                                config.topic_name("events.processed"),
                                reply,
                            ))
                            .await?;

                        if let Err(err) = delivery.ack().await {
                            tracing::warn!(
                                app = %app_name,
                                event = "kafka.message.ack_failed",
                                worker = worker_id,
                                topic = %delivery.message().topic,
                                attempt = %delivery.message().attempt,
                                group = %group,
                                signature = %signature,
                                error = %err,
                                "kafka message manual ack failed"
                            );
                            return Err(err);
                        }

                        tracing::info!(
                            app = %app_name,
                            event = "kafka.message.acked",
                            worker = worker_id,
                            topic = %delivery.message().topic,
                            attempt = %delivery.message().attempt,
                            group = %group,
                            signature = %signature,
                            "kafka message manual ack done"
                        );
                        Ok(())
                    }
                },
                false,
            )
            .await
        } else {
            let handler_app_name = worker_app_name.clone();
            let handler_signature = worker_signature.clone();
            let handler_group = worker_group.clone();
            roze_kafka::spawn_consumer(&subscriber, worker_topic.clone(), move |delivery| {
                let config = config.clone();
                let app_name = handler_app_name.clone();
                let signature = handler_signature.clone();
                let group = handler_group.clone();
                async move {
                    tracing::info!(
                        app = %app_name,
                        event = "kafka.message.received",
                        worker = worker_id,
                        topic = %delivery.message().topic,
                        key = ?delivery.message().key,
                        attempt = %delivery.message().attempt,
                        group = %group,
                        auto_commit = %config.enable_auto_commit,
                        signature = %signature,
                        "received kafka message in user service"
                    );
                    delivery.ack().await?;
                    tracing::info!(
                        app = %app_name,
                        event = "kafka.message.acked",
                        worker = worker_id,
                        topic = %delivery.message().topic,
                        attempt = %delivery.message().attempt,
                        group = %group,
                        signature = %signature,
                        "kafka message auto ack done"
                    );
                    Ok(())
                }
            })
            .await
        };

        match spawn_result {
            Ok(handle) => handles.push(handle),
            Err(err) => {
                let err = err.to_string();
                let reason = PipelineFailureReason::ConsumerSpawn {
                    worker: worker_id,
                    err: err.clone(),
                };
                let reason_text = format!("{reason}");
                restart_failure = Some(reason);
                spawn_failed_count = spawn_failed_count.saturating_add(1);
                tracing::error!(
                    app = %worker_app_name,
                    worker = worker_id,
                    error = %err,
                    event = "kafka.consumer.spawn_failed",
                    version = version,
                    signature = %worker_signature,
                    topic = %worker_topic,
                    group = %worker_group,
                    auto_commit = %auto_commit,
                    manual_ack = %manual_ack,
                    bootstrap = %bootstrap,
                    reason = %reason_text,
                    elapsed_ms = elapsed_ms(started_at),
                    "start consumer failed"
                );
            }
        }
    }

    if handles.is_empty() {
        let reason = restart_failure.unwrap_or(PipelineFailureReason::Unknown);
        tracing::warn!(
            app = %app_name,
            event = "kafka.pipeline.restart_failed",
            version = version,
            bootstrap = %bootstrap,
            previous_signature = %previous_signature,
            signature = %signature,
            topic = %topic,
            group = %group,
            workers = %kafka_config.consumer_workers,
            reason = %format!("{reason}"),
            elapsed_ms = elapsed_ms(started_at),
            "kafka pipeline restart completed with no consumers"
        );
        return;
    }

    if spawn_failed_count > 0 {
        tracing::warn!(
            app = %app_name,
            event = "kafka.pipeline.startup_degraded",
            version = version,
            bootstrap = %bootstrap,
            topic = %topic,
            group = %group,
            signature = %signature,
            workers = %kafka_config.consumer_workers,
            workers_started = handles.len(),
            spawn_failed = spawn_failed_count,
            reason = %format!(
                "{}",
                restart_failure
                    .as_ref()
                    .map(|value| format!("{value}"))
                    .unwrap_or_else(|| "partial_failure".to_string())
            ),
            elapsed_ms = elapsed_ms(started_at),
            "kafka pipeline started with partial consumer spawn failures"
        );
    }

    running.producer = Some(producer.clone());
    running.consumer_handles = handles;

    tracing::info!(
        app = %app_name,
        version = version,
        signature = %signature,
        previous_signature = %previous_signature,
        event = "kafka.pipeline.started",
        bootstrap = %bootstrap,
        topic = %topic,
        group = %group,
        auto_commit = %auto_commit,
        manual_ack = %manual_ack,
        workers = %kafka_config.consumer_workers,
        elapsed_ms = elapsed_ms(started_at),
        "kafka pipeline started"
    );

    tracing::info!(
        app = %app_name,
        event = "kafka.pipeline.restarted",
        version = version,
        bootstrap = %bootstrap,
        previous_signature = %previous_signature,
        signature = %signature,
        topic = %topic,
        group = %group,
        workers = %kafka_config.consumer_workers,
        elapsed_ms = elapsed_ms(started_at),
        "kafka pipeline restart completed"
    );

    let topic_for_push = topic.clone();
    let producer_for_push = producer.clone();
    let app_name_for_push = app_name.clone();
    let signature_for_push = signature.to_string();
    let version_for_push = version;
    let startup_auto_commit = auto_commit;
    let startup_manual_ack = manual_ack;
    let startup_bootstrap = bootstrap;
    let startup_group = group.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(200)).await;
        let payload =
            json!({"source": app_name_for_push, "type": "startup", "topic": topic_for_push});
        if let Err(err) = producer_for_push
            .publish(roze_kafka::KafkaRecord::new(
                topic_for_push.clone(),
                payload,
            ))
            .await
        {
            tracing::warn!(
                app = %app_name_for_push,
                event = "kafka.startup_publish_failed",
                version = version_for_push,
                topic = %topic_for_push,
                group = %startup_group,
                auto_commit = %startup_auto_commit,
                manual_ack = %startup_manual_ack,
                bootstrap = %startup_bootstrap,
                signature = %signature_for_push,
                error = %err,
                "publish kafka startup sample message failed"
            );
        } else {
            tracing::info!(
                app = %app_name_for_push,
                event = "kafka.startup_publish_ok",
                version = version_for_push,
                topic = %topic_for_push,
                group = %startup_group,
                auto_commit = %startup_auto_commit,
                manual_ack = %startup_manual_ack,
                bootstrap = %startup_bootstrap,
                signature = %signature_for_push,
                "kafka startup sample message published"
            );
        }
    });
}

fn elapsed_ms(started_at: tokio::time::Instant) -> u128 {
    tokio::time::Instant::now()
        .duration_since(started_at)
        .as_millis()
}

fn should_nack(payload: &Value) -> bool {
    match payload.get("should_fail") {
        Some(value) => value.as_bool().unwrap_or(false),
        None => payload
            .get("type")
            .and_then(|v| v.as_str())
            .is_some_and(|v| v == "nack"),
    }
}
