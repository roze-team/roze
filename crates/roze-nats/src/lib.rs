use std::{
    collections::VecDeque,
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NatsConfig {
    pub servers: Vec<String>,
    #[serde(default)]
    pub client_name: Option<String>,
    #[serde(default)]
    pub subject_prefix: String,
    #[serde(default)]
    pub jetstream: JetStreamConfig,
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
        let mut event = roze_eventbus::EventEnvelope::new(self.subject, self.payload);
        event.headers = self.headers;
        event
    }

    pub fn from_event(event: roze_eventbus::EventEnvelope) -> Self {
        Self {
            subject: event.topic,
            reply_to: None,
            headers: event.headers,
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
        roze_mq::Message {
            topic: message.subject,
            key: None,
            headers: message.headers,
            attempt,
            dead_letter_topic: None,
            idempotency_key: None,
            available_at_millis: None,
            payload: message.payload,
        }
    }

    fn from_mq_message(message: roze_mq::Message) -> NatsMessage {
        NatsMessage {
            subject: message.topic,
            reply_to: None,
            headers: message.headers,
            payload: message.payload,
        }
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
                .with_locale("zh-CN");
        let msg = NatsMessage::with_context(&ctx, "orders", serde_json::json!({"id": 1}));
        let restored = msg.context();

        assert_eq!(restored.request_id(), "request-1");
        assert_eq!(restored.trace_id(), "trace-1");
        assert_eq!(restored.locale().as_deref(), Some("zh-CN"));
    }

    #[test]
    fn jetstream_defaults_are_production_safe() {
        let cfg = JetStreamConfig::default();
        assert_eq!(cfg.stream, "ROZE");
        assert_eq!(cfg.durable, "roze");
        assert_eq!(cfg.max_retries, 3);
    }
}
