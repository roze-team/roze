use std::{
    collections::HashMap,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use async_trait::async_trait;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EventEnvelope {
    pub topic: String,
    #[serde(default)]
    pub key: Option<String>,
    #[serde(default)]
    pub headers: HashMap<String, String>,
    #[serde(default)]
    pub trace_id: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub attempt: u32,
    pub occurred_at: i64,
    pub payload: serde_json::Value,
}

impl EventEnvelope {
    pub fn new(topic: impl Into<String>, payload: serde_json::Value) -> Self {
        Self {
            topic: topic.into(),
            key: None,
            headers: HashMap::new(),
            trace_id: None,
            source: None,
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

    pub fn with_trace_id(mut self, trace_id: impl Into<String>) -> Self {
        self.trace_id = Some(trace_id.into());
        self
    }

    pub fn with_source(mut self, source: impl Into<String>) -> Self {
        self.source = Some(source.into());
        self
    }
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
}
