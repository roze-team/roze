use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{SystemTime, UNIX_EPOCH},
};

use async_trait::async_trait;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EventEnvelope {
    pub event_id: String,
    pub event_type: String,
    pub version: u32,
    pub schema_revision: String,
    pub topic: String,
    #[serde(default)]
    pub key: Option<String>,
    #[serde(default)]
    pub headers: HashMap<String, String>,
    pub trace_context: HashMap<String, String>,
    pub tenant_id: Option<String>,
    pub idempotency_key: Option<String>,
    pub producer: String,
    pub attempt: u32,
    pub occurred_at: i64,
    pub payload: serde_json::Value,
}

impl EventEnvelope {
    pub fn new(topic: impl Into<String>, payload: serde_json::Value) -> Self {
        let topic = topic.into();
        Self {
            event_id: next_event_id(),
            event_type: topic.clone(),
            version: 1,
            schema_revision: "1".to_string(),
            topic,
            key: None,
            headers: HashMap::new(),
            trace_context: HashMap::new(),
            tenant_id: None,
            idempotency_key: None,
            producer: "unknown".to_string(),
            attempt: 0,
            occurred_at: unix_millis_now(),
            payload,
        }
    }

    pub fn with_key(mut self, key: impl Into<String>) -> Self {
        self.key = Some(key.into());
        self
    }

    pub fn with_header(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.insert(key.into(), value.into());
        self
    }

    pub fn with_event_type(mut self, event_type: impl Into<String>) -> Self {
        self.event_type = event_type.into();
        self
    }

    pub fn with_version(mut self, version: u32, schema_revision: impl Into<String>) -> Self {
        self.version = version.max(1);
        self.schema_revision = schema_revision.into();
        self
    }

    pub fn with_trace_context(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.trace_context.insert(key.into(), value.into());
        self
    }

    pub fn with_tenant(mut self, tenant_id: impl Into<String>) -> Self {
        self.tenant_id = Some(tenant_id.into());
        self
    }

    pub fn with_idempotency_key(mut self, idempotency_key: impl Into<String>) -> Self {
        self.idempotency_key = Some(idempotency_key.into());
        self
    }

    pub fn with_producer(mut self, producer: impl Into<String>) -> Self {
        self.producer = producer.into();
        self
    }

    pub fn from_transport(
        topic: impl Into<String>,
        payload: serde_json::Value,
        key: Option<String>,
        headers: HashMap<String, String>,
        attempt: u32,
    ) -> Self {
        let mut event = Self::new(topic, payload);
        event.event_id = required_header(&headers, "x-roze-event-id").unwrap_or_else(next_event_id);
        event.event_type =
            required_header(&headers, "x-roze-event-type").unwrap_or_else(|| event.topic.clone());
        event.version = required_header(&headers, "x-roze-event-version")
            .and_then(|value| value.parse().ok())
            .unwrap_or(1);
        event.schema_revision =
            required_header(&headers, "x-roze-schema-revision").unwrap_or_else(|| "1".to_string());
        event.tenant_id = required_header(&headers, "x-roze-tenant-id");
        event.idempotency_key = required_header(&headers, "x-roze-idempotency-key");
        event.producer =
            required_header(&headers, "x-roze-producer").unwrap_or_else(|| "unknown".to_string());
        event.trace_context = headers
            .iter()
            .filter(|(name, _)| is_trace_context_header(name))
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect();
        event.key = key;
        event.headers = headers;
        event.attempt = attempt;
        event
    }

    pub fn transport_headers(&self) -> HashMap<String, String> {
        let mut headers = self.headers.clone();
        headers.insert("x-roze-event-id".to_string(), self.event_id.clone());
        headers.insert("x-roze-event-type".to_string(), self.event_type.clone());
        headers.insert("x-roze-event-version".to_string(), self.version.to_string());
        headers.insert(
            "x-roze-schema-revision".to_string(),
            self.schema_revision.clone(),
        );
        headers.insert("x-roze-producer".to_string(), self.producer.clone());
        if let Some(tenant_id) = &self.tenant_id {
            headers.insert("x-roze-tenant-id".to_string(), tenant_id.clone());
        }
        if let Some(idempotency_key) = &self.idempotency_key {
            headers.insert(
                "x-roze-idempotency-key".to_string(),
                idempotency_key.clone(),
            );
        }
        headers.extend(self.trace_context.clone());
        headers
    }
}

fn required_header(headers: &HashMap<String, String>, name: &str) -> Option<String> {
    headers
        .iter()
        .find(|(key, value)| key.eq_ignore_ascii_case(name) && !value.trim().is_empty())
        .map(|(_, value)| value.clone())
}

fn is_trace_context_header(name: &str) -> bool {
    ["traceparent", "tracestate", "baggage", "x-trace-id"]
        .iter()
        .any(|expected| name.eq_ignore_ascii_case(expected))
}

static EVENT_ID_SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn next_event_id() -> String {
    format!(
        "evt-{}-{}",
        unix_millis_now(),
        EVENT_ID_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventTopic {
    pub name: String,
}

impl EventTopic {
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }
}

#[async_trait]
pub trait EventPublisher: Send + Sync + 'static {
    async fn publish(&self, event: EventEnvelope) -> anyhow::Result<()>;
}

#[async_trait]
pub trait EventSubscriber: Send + Sync + 'static {
    async fn subscribe(&self, topic: &str) -> anyhow::Result<broadcast::Receiver<EventEnvelope>>;
}

#[derive(Debug, Clone)]
pub struct InMemoryEventBus {
    topics: Arc<DashMap<String, broadcast::Sender<EventEnvelope>>>,
    capacity: usize,
}

impl InMemoryEventBus {
    pub fn new() -> Self {
        Self {
            topics: Arc::new(DashMap::new()),
            capacity: 256,
        }
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            topics: Arc::new(DashMap::new()),
            capacity: capacity.max(1),
        }
    }

    fn sender_for(&self, topic: &str) -> broadcast::Sender<EventEnvelope> {
        self.topics
            .entry(topic.to_string())
            .or_insert_with(|| broadcast::channel(self.capacity).0)
            .clone()
    }

    pub fn topic_count(&self) -> usize {
        self.topics.len()
    }
}

impl Default for InMemoryEventBus {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl EventPublisher for InMemoryEventBus {
    async fn publish(&self, event: EventEnvelope) -> anyhow::Result<()> {
        let _ = self.sender_for(&event.topic).send(event)?;
        Ok(())
    }
}

#[async_trait]
impl EventSubscriber for InMemoryEventBus {
    async fn subscribe(&self, topic: &str) -> anyhow::Result<broadcast::Receiver<EventEnvelope>> {
        Ok(self.sender_for(topic).subscribe())
    }
}

pub async fn publish_json<P>(
    publisher: &P,
    topic: impl Into<String>,
    payload: serde_json::Value,
) -> anyhow::Result<()>
where
    P: EventPublisher,
{
    publisher.publish(EventEnvelope::new(topic, payload)).await
}

pub async fn spawn_consumer<S, F, Fut>(
    subscriber: &S,
    topic: impl Into<String>,
    handler: F,
) -> anyhow::Result<tokio::task::JoinHandle<()>>
where
    S: EventSubscriber,
    F: Fn(EventEnvelope) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = anyhow::Result<()>> + Send + 'static,
{
    let topic = topic.into();
    let mut receiver = subscriber.subscribe(&topic).await?;
    let handler = Arc::new(handler);
    let handle = tokio::spawn(async move {
        loop {
            match receiver.recv().await {
                Ok(event) => {
                    if let Err(err) = handler(event.clone()).await {
                        tracing::warn!(topic = %topic, error = %err, "event handler failed");
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                    tracing::warn!(topic = %topic, skipped, "event consumer lagged");
                }
            }
        }
    });
    Ok(handle)
}

pub fn unix_millis_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    #[tokio::test]
    async fn publish_and_consume_event() {
        let bus = InMemoryEventBus::new();
        let seen = Arc::new(AtomicUsize::new(0));
        let consumer_seen = seen.clone();
        let handle = spawn_consumer(&bus, "orders", move |event| {
            let consumer_seen = consumer_seen.clone();
            async move {
                if event.topic == "orders" {
                    consumer_seen.fetch_add(1, Ordering::SeqCst);
                }
                Ok(())
            }
        })
        .await
        .expect("consumer");

        publish_json(&bus, "orders", serde_json::json!({"id": 1}))
            .await
            .expect("publish");
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert_eq!(seen.load(Ordering::SeqCst), 1);
        handle.abort();
    }

    #[test]
    fn envelope_requires_versioned_reliable_event_metadata() {
        let event = EventEnvelope::new("orders.created", serde_json::json!({"id": 1}))
            .with_event_type("order.created")
            .with_version(2, "order-v2")
            .with_trace_context("traceparent", "00-trace-span-01")
            .with_tenant("tenant-1")
            .with_idempotency_key("order-1")
            .with_producer("order-service");

        assert!(event.event_id.starts_with("evt-"));
        assert_eq!(event.event_type, "order.created");
        assert_eq!(event.version, 2);
        assert_eq!(event.schema_revision, "order-v2");
        assert_eq!(event.tenant_id.as_deref(), Some("tenant-1"));
        assert_eq!(event.idempotency_key.as_deref(), Some("order-1"));
        assert_eq!(event.producer, "order-service");
    }
}
