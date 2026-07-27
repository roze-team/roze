use std::{
    collections::VecDeque,
    fmt,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::{SystemTime, UNIX_EPOCH},
};

use async_trait::async_trait;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NatsConfig {
    pub servers: Vec<String>,
    #[serde(default)]
    pub client_name: Option<String>,
    #[serde(default)]
    pub subject_prefix: String,
    #[serde(default)]
    pub jetstream: JetStreamConfig,
}

impl fmt::Debug for NatsConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NatsConfig")
            .field("server_count", &self.servers.len())
            .field("client_name", &self.client_name)
            .field("subject_prefix", &self.subject_prefix)
            .field("jetstream", &self.jetstream)
            .finish()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct JetStreamConfig {
    #[serde(default = "default_stream_name")]
    pub stream: String,
    #[serde(default)]
    pub subjects: Vec<String>,
    #[serde(default = "default_durable_name")]
    pub durable: String,
    #[serde(default = "default_max_messages")]
    pub max_messages: i64,
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,
    #[serde(default)]
    pub retry_subject: Option<String>,
    #[serde(default)]
    pub dead_letter_subject: Option<String>,
    #[serde(default = "default_consumer_buffer")]
    pub consumer_buffer: usize,
}

impl Default for JetStreamConfig {
    fn default() -> Self {
        Self {
            stream: default_stream_name(),
            subjects: Vec::new(),
            durable: default_durable_name(),
            max_messages: default_max_messages(),
            max_retries: default_max_retries(),
            retry_subject: None,
            dead_letter_subject: None,
            consumer_buffer: default_consumer_buffer(),
        }
    }
}

fn default_stream_name() -> String {
    "ROZE".to_string()
}

fn default_durable_name() -> String {
    "roze".to_string()
}

fn default_max_messages() -> i64 {
    10_000
}

fn default_max_retries() -> u32 {
    3
}

fn default_consumer_buffer() -> usize {
    256
}

impl NatsConfig {
    pub fn subject_name(&self, subject: impl AsRef<str>) -> String {
        if self.subject_prefix.is_empty() {
            subject.as_ref().to_string()
        } else {
            format!("{}.{}", self.subject_prefix, subject.as_ref())
        }
    }

    pub fn servers_csv(&self) -> String {
        self.servers.join(",")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NatsMessage {
    pub subject: String,
    #[serde(default)]
    pub reply_to: Option<String>,
    #[serde(default)]
    pub headers: std::collections::HashMap<String, String>,
    pub payload: serde_json::Value,
}

impl NatsMessage {
    pub fn new(subject: impl Into<String>, payload: serde_json::Value) -> Self {
        Self::with_trace_id(subject, payload, roze_trace::generate_trace_id())
    }

    pub fn with_trace_id(
        subject: impl Into<String>,
        payload: serde_json::Value,
        trace_id: impl Into<String>,
    ) -> Self {
        let mut headers = std::collections::HashMap::new();
        headers.insert(roze_trace::TRACE_ID_HEADER.to_string(), trace_id.into());
        Self {
            subject: subject.into(),
            reply_to: None,
            headers,
            payload,
        }
    }

    pub fn with_context(
        context: &roze_context::Context,
        subject: impl Into<String>,
        payload: serde_json::Value,
    ) -> Self {
        Self {
            subject: subject.into(),
            reply_to: None,
            headers: context.propagation_headers().into_iter().collect(),
            payload,
        }
    }

    pub fn context(&self) -> roze_context::Context {
        roze_context::Context::from_propagation_headers(
            &self
                .headers
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
        )
    }

    pub fn ensure_trace_id(&mut self) -> String {
        if let Some((_, value)) = self.headers.iter().find(|(key, value)| {
            key.eq_ignore_ascii_case(roze_trace::TRACE_ID_HEADER) && !value.trim().is_empty()
        }) {
            return value.clone();
        }
        let trace_id = roze_trace::generate_trace_id();
        self.headers
            .insert(roze_trace::TRACE_ID_HEADER.to_string(), trace_id.clone());
        trace_id
    }

    pub fn to_event(self) -> roze_eventbus::EventEnvelope {
        roze_eventbus::EventEnvelope::from_transport(
            self.subject,
            self.payload,
            None,
            self.headers,
            0,
        )
    }

    pub fn from_event(event: roze_eventbus::EventEnvelope) -> Self {
        let headers = event.transport_headers();
        Self {
            subject: event.topic,
            reply_to: None,
            headers,
            payload: event.payload,
        }
    }
}

#[derive(Debug, Clone)]
pub struct NatsJetStream {
    config: NatsConfig,
    jetstream: async_nats::jetstream::Context,
    dead_letters: Arc<Mutex<VecDeque<roze_mq::DeadLetterRecord>>>,
    next_dead_letter_id: Arc<AtomicU64>,
    published: Arc<AtomicU64>,
    delivered: Arc<AtomicU64>,
    acked: Arc<AtomicU64>,
    nacked: Arc<AtomicU64>,
    dead_lettered: Arc<AtomicU64>,
    replayed: Arc<AtomicU64>,
}

impl NatsJetStream {
    pub async fn connect(config: NatsConfig) -> anyhow::Result<Self> {
        let client = async_nats::connect(config.servers_csv()).await?;
        let jetstream = async_nats::jetstream::new(client);
        let broker = Self {
            config,
            jetstream,
            dead_letters: Arc::new(Mutex::new(VecDeque::new())),
            next_dead_letter_id: Arc::new(AtomicU64::new(1)),
            published: Arc::new(AtomicU64::new(0)),
            delivered: Arc::new(AtomicU64::new(0)),
            acked: Arc::new(AtomicU64::new(0)),
            nacked: Arc::new(AtomicU64::new(0)),
            dead_lettered: Arc::new(AtomicU64::new(0)),
            replayed: Arc::new(AtomicU64::new(0)),
        };
        broker.ensure_stream().await?;
        Ok(broker)
    }

    pub fn config(&self) -> &NatsConfig {
        &self.config
    }

    pub async fn health_check(&self) -> anyhow::Result<()> {
        self.jetstream.query_account().await?;
        Ok(())
    }

    async fn ensure_stream(&self) -> anyhow::Result<async_nats::jetstream::stream::Stream> {
        let subjects = if self.config.jetstream.subjects.is_empty() {
            vec![format!(
                "{}.*",
                self.config.subject_prefix.trim_end_matches('.')
            )]
        } else {
            self.config
                .jetstream
                .subjects
                .iter()
                .map(|subject| self.config.subject_name(subject))
                .collect()
        };
        Ok(self
            .jetstream
            .get_or_create_stream(async_nats::jetstream::stream::Config {
                name: self.config.jetstream.stream.clone(),
                subjects,
                max_messages: self.config.jetstream.max_messages,
                ..Default::default()
            })
            .await?)
    }

    fn to_mq_message(message: NatsMessage, attempt: u32) -> roze_mq::Message {
        let event = roze_eventbus::EventEnvelope::from_transport(
            message.subject,
            message.payload,
            None,
            message.headers,
            attempt,
        );
        roze_mq::Message::from_event_envelope(event)
    }

    fn from_mq_message(message: roze_mq::Message) -> NatsMessage {
        NatsMessage::from_event(message.into_event_envelope())
    }

    async fn publish_nats(&self, mut message: NatsMessage, attempt: u32) -> anyhow::Result<()> {
        message.ensure_trace_id();
        message
            .headers
            .insert("roze-attempt".to_string(), attempt.to_string());
        let subject = self.config.subject_name(&message.subject);
        let payload = serde_json::to_vec(&message)?;
        self.jetstream.publish(subject, payload.into()).await?;
        self.published.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn recover_message(&self, message: roze_mq::Message) -> anyhow::Result<()> {
        self.nacked.fetch_add(1, Ordering::SeqCst);
        let next_attempt = message.attempt.saturating_add(1);
        if next_attempt <= self.config.jetstream.max_retries {
            if let Some(subject) = self.config.jetstream.retry_subject.clone() {
                let mut retry = message.clone();
                retry.topic = subject;
                retry.attempt = next_attempt;
                return self
                    .publish_nats(Self::from_mq_message(retry), next_attempt)
                    .await;
            }
        }

        self.push_dead_letter(message.clone(), "nack_max_attempts_exceeded");
        if let Some(subject) = self.config.jetstream.dead_letter_subject.clone() {
            let mut dead = message;
            dead.topic = subject;
            dead.attempt = 0;
            self.publish_nats(Self::from_mq_message(dead), 0).await?;
        }
        Ok(())
    }

    fn push_dead_letter(&self, message: roze_mq::Message, reason: impl Into<String>) {
        let record = roze_mq::DeadLetterRecord {
            id: self.next_dead_letter_id.fetch_add(1, Ordering::SeqCst),
            original_topic: message.topic.clone(),
            reason: reason.into(),
            failed_at_millis: current_millis(),
            replay_count: 0,
            message,
        };
        self.dead_letters
            .lock()
            .expect("nats dlq lock poisoned")
            .push_back(record);
        self.dead_lettered.fetch_add(1, Ordering::SeqCst);
    }
}

#[async_trait]
impl roze_mq::Publisher for NatsJetStream {
    async fn publish(&self, message: roze_mq::Message) -> anyhow::Result<()> {
        self.publish_nats(Self::from_mq_message(message.clone()), message.attempt)
            .await
    }
}

#[async_trait]
impl roze_mq::Subscriber for NatsJetStream {
    async fn subscribe(
        &self,
        topic: &str,
    ) -> anyhow::Result<broadcast::Receiver<roze_mq::Delivery>> {
        let subject = self.config.subject_name(topic);
        let stream = self.ensure_stream().await?;
        let consumer = stream
            .get_or_create_consumer(
                &self.config.jetstream.durable,
                async_nats::jetstream::consumer::pull::Config {
                    durable_name: Some(self.config.jetstream.durable.clone()),
                    filter_subject: subject,
                    ..Default::default()
                },
            )
            .await?;
        let mut messages = consumer.messages().await?;
        let (sender, receiver) = broadcast::channel(self.config.jetstream.consumer_buffer.max(1));
        let broker = self.clone();
        tokio::spawn(async move {
            while let Some(next) = messages.next().await {
                let Ok(js_message) = next else {
                    continue;
                };
                let mut nats_message: NatsMessage = serde_json::from_slice(&js_message.payload)
                    .unwrap_or_else(|_| NatsMessage::new("unknown", serde_json::json!(null)));
                nats_message.ensure_trace_id();
                let attempt = nats_message
                    .headers
                    .get("roze-attempt")
                    .and_then(|value| value.parse::<u32>().ok())
                    .unwrap_or_default();
                let mq_message = NatsJetStream::to_mq_message(nats_message, attempt);
                broker.delivered.fetch_add(1, Ordering::SeqCst);

                let ack_broker = broker.clone();
                let ack_message = js_message.clone();
                let ack_fn = Arc::new(move || {
                    let broker = ack_broker.clone();
                    let message = ack_message.clone();
                    Box::pin(async move {
                        message
                            .ack()
                            .await
                            .map_err(|err| anyhow::anyhow!(err.to_string()))?;
                        broker.acked.fetch_add(1, Ordering::SeqCst);
                        Ok(())
                    })
                        as std::pin::Pin<
                            Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send>,
                        >
                });

                let nack_broker = broker.clone();
                let nack_message = js_message.clone();
                let mq_for_nack = mq_message.clone();
                let nack_fn = Arc::new(move || {
                    let broker = nack_broker.clone();
                    let message = nack_message.clone();
                    let mq_message = mq_for_nack.clone();
                    Box::pin(async move {
                        message
                            .ack_with(async_nats::jetstream::AckKind::Nak(None))
                            .await
                            .map_err(|err| anyhow::anyhow!(err.to_string()))?;
                        broker.recover_message(mq_message).await
                    })
                        as std::pin::Pin<
                            Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send>,
                        >
                });
                let _ = sender.send(roze_mq::Delivery::external(mq_message, ack_fn, nack_fn));
            }
        });
        Ok(receiver)
    }
}

#[async_trait]
impl roze_mq::MqAdmin for NatsJetStream {
    async fn stats(&self) -> anyhow::Result<roze_mq::MqStats> {
        Ok(roze_mq::MqStats {
            published: self.published.load(Ordering::SeqCst),
            delivered: self.delivered.load(Ordering::SeqCst),
            acked: self.acked.load(Ordering::SeqCst),
            nacked: self.nacked.load(Ordering::SeqCst),
            duplicated: 0,
            dead_lettered: self.dead_lettered.load(Ordering::SeqCst),
            replayed: self.replayed.load(Ordering::SeqCst),
            dead_letter_pending: self
                .dead_letters
                .lock()
                .expect("nats dlq lock poisoned")
                .len() as u64,
        })
    }

    async fn dead_letters(
        &self,
        offset: usize,
        limit: usize,
    ) -> anyhow::Result<Vec<roze_mq::DeadLetterRecord>> {
        Ok(self
            .dead_letters
            .lock()
            .expect("nats dlq lock poisoned")
            .iter()
            .skip(offset)
            .take(limit.clamp(1, 500))
            .cloned()
            .collect())
    }

    async fn replay_dead_letter(&self, id: u64) -> anyhow::Result<Option<roze_mq::Message>> {
        let mut message = {
            let mut records = self.dead_letters.lock().expect("nats dlq lock poisoned");
            let Some(record) = records.iter_mut().find(|record| record.id == id) else {
                return Ok(None);
            };
            record.replay_count = record.replay_count.saturating_add(1);
            let mut message = record.message.clone();
            message.topic = record.original_topic.clone();
            message.attempt = 0;
            message
        };
        message.ensure_trace_id();
        self.replayed.fetch_add(1, Ordering::SeqCst);
        self.publish_nats(Self::from_mq_message(message.clone()), message.attempt)
            .await?;
        Ok(Some(message))
    }

    async fn purge_dead_letter(
        &self,
        id: u64,
    ) -> anyhow::Result<Option<roze_mq::DeadLetterRecord>> {
        let mut records = self.dead_letters.lock().expect("nats dlq lock poisoned");
        let Some(index) = records.iter().position(|record| record.id == id) else {
            return Ok(None);
        };
        Ok(records.remove(index))
    }

    async fn clear_dead_letters(&self) -> anyhow::Result<usize> {
        let mut records = self.dead_letters.lock().expect("nats dlq lock poisoned");
        let len = records.len();
        records.clear();
        Ok(len)
    }
}

fn current_millis() -> u64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(elapsed) => elapsed.as_millis() as u64,
        Err(_) => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use roze_mq::{Publisher, Subscriber};

    #[test]
    fn formats_servers_and_subjects() {
        let cfg = NatsConfig {
            servers: vec!["n1:4222".into(), "n2:4222".into()],
            client_name: Some("roze".into()),
            subject_prefix: "app".into(),
            jetstream: JetStreamConfig::default(),
        };
        assert_eq!(cfg.servers_csv(), "n1:4222,n2:4222");
        assert_eq!(cfg.subject_name("orders"), "app.orders");
    }

    #[test]
    fn nats_message_carries_uuid_v7_trace_id() {
        let msg = NatsMessage::new("orders", serde_json::json!({"id": 1}));
        let trace_id = msg.headers.get(roze_trace::TRACE_ID_HEADER).unwrap();
        assert_eq!(
            uuid::Uuid::parse_str(trace_id).unwrap().get_version_num(),
            7
        );
    }

    #[test]
    fn nats_message_round_trips_context_headers() {
        let ctx =
            roze_context::Context::background_with_request_id_and_trace_id("request-1", "trace-1")
                .with_locale("zh-CN")
                .with_auth(roze_context::AuthContext {
                    subject: "user-1".to_string(),
                    roles: vec!["buyer".to_string()],
                    tenant: Some("tenant-1".to_string()),
                })
                .with_idempotency_key("order-1")
                .with_retry_budget(2)
                .with_timeout(std::time::Duration::from_secs(1));
        let msg = NatsMessage::with_context(&ctx, "orders", serde_json::json!({"id": 1}));
        let restored = msg.context();

        assert_eq!(restored.request_id(), "request-1");
        assert_eq!(restored.trace_id(), "trace-1");
        assert_eq!(restored.locale().as_deref(), Some("zh-CN"));
        assert_eq!(restored.subject().as_deref(), Some("user-1"));
        assert_eq!(restored.tenant().as_deref(), Some("tenant-1"));
        assert_eq!(restored.idempotency_key().as_deref(), Some("order-1"));
        assert_eq!(restored.retry_budget_remaining(), Some(2));
        assert!(restored.remaining_timeout().is_some());
    }

    #[test]
    fn jetstream_defaults_are_production_safe() {
        let cfg = JetStreamConfig::default();
        assert_eq!(cfg.stream, "ROZE");
        assert_eq!(cfg.durable, "roze");
        assert_eq!(cfg.max_retries, 3);
    }

    #[test]
    fn nats_transport_preserves_idempotency_metadata() {
        let message = roze_mq::Message::new("orders", serde_json::json!({"order_id": "order-1"}))
            .with_idempotency_key("order-1");

        let wire = NatsJetStream::from_mq_message(message);
        let restored = NatsJetStream::to_mq_message(wire, 0);

        assert_eq!(restored.idempotency_key.as_deref(), Some("order-1"));
        assert_eq!(restored.payload["order_id"], "order-1");
    }

    #[tokio::test]
    #[ignore = "requires ROZE_TEST_NATS_URL, for example nats://127.0.0.1:4222"]
    async fn jetstream_round_trip_against_real_service() {
        let server = std::env::var("ROZE_TEST_NATS_URL").expect("ROZE_TEST_NATS_URL is required");
        let suffix = format!("{}-{}", std::process::id(), current_millis());
        let topic = format!("events-{suffix}");
        let broker = NatsJetStream::connect(NatsConfig {
            servers: vec![server],
            client_name: Some(format!("roze-reference-{suffix}")),
            subject_prefix: format!("roze.reference.{suffix}"),
            jetstream: JetStreamConfig {
                stream: format!("ROZE_REFERENCE_{suffix}"),
                subjects: vec![topic.clone()],
                durable: format!("roze-reference-{suffix}"),
                max_messages: 100,
                max_retries: 1,
                retry_subject: None,
                dead_letter_subject: None,
                consumer_buffer: 8,
            },
        })
        .await
        .expect("connect NATS JetStream");
        let mut receiver = broker.subscribe(&topic).await.expect("subscribe");
        broker
            .publish(
                roze_mq::Message::new(&topic, serde_json::json!({"order_id": "order-1"}))
                    .with_idempotency_key("order-1"),
            )
            .await
            .expect("publish");

        let delivery = tokio::time::timeout(std::time::Duration::from_secs(10), receiver.recv())
            .await
            .expect("NATS delivery timeout")
            .expect("receive delivery");
        assert_eq!(delivery.message().payload["order_id"], "order-1");
        assert_eq!(
            delivery.message().idempotency_key.as_deref(),
            Some("order-1")
        );
        delivery.ack().await.expect("ack delivery");
    }

    #[tokio::test]
    #[ignore = "requires ROZE_TEST_NATS_URL and an externally managed NATS restart cycle"]
    async fn production_soak_jetstream_disconnect_recovery() {
        let server = std::env::var("ROZE_TEST_NATS_URL").expect("ROZE_TEST_NATS_URL is required");
        let seconds = std::env::var("ROZE_NATS_SOAK_SECONDS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(300);
        let require_disconnect =
            std::env::var("ROZE_NATS_REQUIRE_DISCONNECT").is_ok_and(|value| value == "1");
        let suffix = format!("{}-{}", std::process::id(), current_millis());
        let topic = format!("events-{suffix}");
        let broker = NatsJetStream::connect(NatsConfig {
            servers: vec![server],
            client_name: Some(format!("roze-soak-{suffix}")),
            subject_prefix: format!("roze.soak.{suffix}"),
            jetstream: JetStreamConfig {
                stream: format!("ROZE_SOAK_{suffix}"),
                subjects: vec![topic.clone()],
                durable: format!("roze-soak-{suffix}"),
                max_messages: 100_000,
                max_retries: 3,
                retry_subject: None,
                dead_letter_subject: None,
                consumer_buffer: 1_024,
            },
        })
        .await
        .expect("connect NATS JetStream");
        let mut receiver = broker.subscribe(&topic).await.expect("subscribe");
        let started = std::time::Instant::now();
        let deadline = started + std::time::Duration::from_secs(seconds);
        let mut attempts = 0_u64;
        let mut delivered = 0_u64;
        let mut disconnect_observations = 0_u64;
        let mut recoveries = 0_u64;
        let mut recovery_started = None;
        let mut delivery_latency = roze_metrics::LatencyHistogram::new();
        let mut recovery_latency = roze_metrics::LatencyHistogram::new();

        while std::time::Instant::now() < deadline {
            attempts = attempts.saturating_add(1);
            let operation_started = std::time::Instant::now();
            let message = roze_mq::Message::new(
                &topic,
                serde_json::json!({"sequence": attempts, "sent_at_ms": current_millis()}),
            )
            .with_idempotency_key(format!("{suffix}-{attempts}"));
            let publish_result =
                tokio::time::timeout(std::time::Duration::from_secs(2), broker.publish(message))
                    .await;
            if !matches!(publish_result, Ok(Ok(()))) {
                disconnect_observations = disconnect_observations.saturating_add(1);
                recovery_started.get_or_insert_with(std::time::Instant::now);
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                delivery_latency.observe(operation_started.elapsed());
                continue;
            }

            let receive_result =
                tokio::time::timeout(std::time::Duration::from_secs(2), receiver.recv()).await;
            let Ok(Ok(delivery)) = receive_result else {
                disconnect_observations = disconnect_observations.saturating_add(1);
                recovery_started.get_or_insert_with(std::time::Instant::now);
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                delivery_latency.observe(operation_started.elapsed());
                continue;
            };
            let ack_result =
                tokio::time::timeout(std::time::Duration::from_secs(2), delivery.ack()).await;
            if !matches!(ack_result, Ok(Ok(()))) {
                disconnect_observations = disconnect_observations.saturating_add(1);
                recovery_started.get_or_insert_with(std::time::Instant::now);
                delivery_latency.observe(operation_started.elapsed());
                continue;
            }

            delivered = delivered.saturating_add(1);
            delivery_latency.observe(operation_started.elapsed());
            if let Some(recovery_started) = recovery_started.take() {
                recoveries = recoveries.saturating_add(1);
                recovery_latency.observe(recovery_started.elapsed());
            }
        }

        let elapsed_ms = started.elapsed().as_millis().max(1);
        let messages_per_second_milli =
            u128::from(delivered).saturating_mul(1_000_000) / elapsed_ms;
        let p99_delivery_us = delivery_latency
            .percentile_upper_bound_micros(99)
            .expect("NATS delivery latency");
        let p99_recovery_us = recovery_latency
            .percentile_upper_bound_micros(99)
            .unwrap_or(0);
        println!(
            "roze_nats_soak nats_elapsed_ms={elapsed_ms} nats_attempts={attempts} nats_delivered={delivered} nats_disconnect_observations={disconnect_observations} nats_recoveries={recoveries} nats_messages_per_second_milli={messages_per_second_milli} nats_p99_delivery_us={p99_delivery_us} nats_p99_recovery_us={p99_recovery_us}"
        );

        assert!(attempts > 0);
        assert!(delivered > 0);
        if require_disconnect {
            assert!(disconnect_observations > 0);
            assert!(recoveries > 0);
            assert!(p99_recovery_us > 0);
            assert!(
                recovery_started.is_none(),
                "NATS did not recover before the soak ended"
            );
        }
    }
}
