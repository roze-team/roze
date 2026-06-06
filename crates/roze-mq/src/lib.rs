use std::{
    collections::HashMap,
    future::Future,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::{sync::broadcast, task::JoinHandle};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Message {
    pub topic: String,
    #[serde(default)]
    pub key: Option<String>,
    #[serde(default)]
    pub headers: HashMap<String, String>,
    #[serde(default)]
    pub attempt: u32,
    #[serde(default)]
    pub dead_letter_topic: Option<String>,
    pub payload: serde_json::Value,
}

impl Message {
    pub fn new(topic: impl Into<String>, payload: serde_json::Value) -> Self {
        Self {
            topic: topic.into(),
            key: None,
            headers: HashMap::new(),
            attempt: 0,
            dead_letter_topic: None,
            payload,
        }
    }

    pub fn with_dead_letter_topic(mut self, topic: impl Into<String>) -> Self {
        self.dead_letter_topic = Some(topic.into());
        self
    }
}

#[derive(Debug)]
struct DeliveryState {
    acked: AtomicBool,
    nacked: AtomicBool,
}

impl DeliveryState {
    fn new() -> Self {
        Self {
            acked: AtomicBool::new(false),
            nacked: AtomicBool::new(false),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Delivery {
    message: Message,
    state: Arc<DeliveryState>,
    broker: InMemoryBroker,
}

impl Delivery {
    pub fn message(&self) -> &Message {
        &self.message
    }

    pub fn into_message(self) -> Message {
        self.message
    }

    pub fn is_acked(&self) -> bool {
        self.state.acked.load(Ordering::SeqCst)
    }

    pub fn is_nacked(&self) -> bool {
        self.state.nacked.load(Ordering::SeqCst)
    }

    pub async fn ack(&self) -> anyhow::Result<()> {
        self.state.acked.store(true, Ordering::SeqCst);
        Ok(())
    }

    pub async fn nack(&self) -> anyhow::Result<()> {
        self.state.nacked.store(true, Ordering::SeqCst);
        self.broker.requeue_or_dead_letter(self.message.clone()).await
    }
}

#[async_trait]
pub trait Publisher: Send + Sync + 'static {
    async fn publish(&self, message: Message) -> anyhow::Result<()>;
}

#[async_trait]
pub trait Subscriber: Send + Sync + 'static {
    async fn subscribe(&self, topic: &str) -> anyhow::Result<broadcast::Receiver<Delivery>>;
}

#[derive(Debug, Clone)]
pub struct InMemoryBroker {
    topics: Arc<Mutex<HashMap<String, broadcast::Sender<Delivery>>>>,
    dead_letters: Arc<Mutex<Vec<Message>>>,
    dead_letter_topic: Option<String>,
    max_attempts: u32,
}

impl InMemoryBroker {
    pub fn new() -> Self {
        Self {
            topics: Arc::new(Mutex::new(HashMap::new())),
            dead_letters: Arc::new(Mutex::new(Vec::new())),
            dead_letter_topic: None,
            max_attempts: 3,
        }
    }

    pub fn with_dead_letter(dead_letter_topic: impl Into<String>, max_attempts: u32) -> Self {
        Self {
            dead_letter_topic: Some(dead_letter_topic.into()),
            max_attempts: max_attempts.max(1),
            ..Self::default()
        }
    }

    pub fn dead_letters(&self) -> Vec<Message> {
        self.dead_letters.lock().expect("broker lock poisoned").clone()
    }

    fn sender_for(&self, topic: &str) -> broadcast::Sender<Delivery> {
        let mut topics = self.topics.lock().expect("broker lock poisoned");
        topics
            .entry(topic.to_string())
            .or_insert_with(|| {
                let (sender, _receiver) = broadcast::channel(128);
                sender
            })
            .clone()
    }

    async fn route_delivery(&self, mut message: Message) -> anyhow::Result<()> {
        message.attempt = message.attempt.saturating_add(1);
        if message.attempt > self.max_attempts {
            self.dead_letters
                .lock()
                .expect("broker lock poisoned")
                .push(message.clone());
            if let Some(dead_letter_topic) = &message.dead_letter_topic {
                let mut routed = message.clone();
                routed.topic = dead_letter_topic.clone();
                routed.attempt = 0;
                let _ = self.sender_for(dead_letter_topic).send(Delivery::new(routed, self.clone()));
            }
            return Ok(());
        }

        let sender = self.sender_for(&message.topic);
        let _ = sender.send(Delivery::new(message, self.clone()));
        Ok(())
    }

    async fn requeue_or_dead_letter(&self, message: Message) -> anyhow::Result<()> {
        let mut next = message.clone();
        next.attempt = next.attempt.saturating_add(1);
        if next.attempt > self.max_attempts {
            self.dead_letters
                .lock()
                .expect("broker lock poisoned")
                .push(next.clone());
            if let Some(dead_letter_topic) = next
                .dead_letter_topic
                .clone()
                .or_else(|| self.dead_letter_topic.clone())
            {
                let mut routed = next;
                routed.topic = dead_letter_topic.clone();
                routed.attempt = 0;
                let _ = self.sender_for(&dead_letter_topic).send(Delivery::new(routed, self.clone()));
            }
            return Ok(());
        }

        let _ = self.sender_for(&next.topic).send(Delivery::new(next, self.clone()));
        Ok(())
    }
}

impl Default for InMemoryBroker {
    fn default() -> Self {
        Self::new()
    }
}

impl Delivery {
    fn new(message: Message, broker: InMemoryBroker) -> Self {
        Self {
            message,
            state: Arc::new(DeliveryState::new()),
            broker,
        }
    }
}

#[async_trait]
impl Publisher for InMemoryBroker {
    async fn publish(&self, message: Message) -> anyhow::Result<()> {
        self.route_delivery(message).await
    }
}

#[async_trait]
impl Subscriber for InMemoryBroker {
    async fn subscribe(&self, topic: &str) -> anyhow::Result<broadcast::Receiver<Delivery>> {
        Ok(self.sender_for(topic).subscribe())
    }
}

pub async fn publish_json<P>(
    publisher: &P,
    topic: impl Into<String>,
    payload: serde_json::Value,
) -> anyhow::Result<()>
where
    P: Publisher,
{
    publisher.publish(Message::new(topic, payload)).await
}

pub async fn spawn_consumer<S, F, Fut>(
    subscriber: &S,
    topic: impl Into<String>,
    handler: F,
) -> anyhow::Result<JoinHandle<()>>
where
    S: Subscriber,
    F: Fn(Delivery) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = anyhow::Result<()>> + Send + 'static,
{
    let topic = topic.into();
    let mut receiver = subscriber.subscribe(&topic).await?;
    let handler = Arc::new(handler);
    let handle = tokio::spawn(async move {
        loop {
            match receiver.recv().await {
                Ok(delivery) => {
                    let result = handler(delivery.clone()).await;
                    match result {
                        Ok(()) => {
                            if !delivery.is_acked() && !delivery.is_nacked() {
                                let _ = delivery.ack().await;
                            }
                        }
                        Err(err) => {
                            tracing::warn!(topic = %topic, error = %err, "message handler failed");
                            let _ = delivery.nack().await;
                        }
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                    tracing::warn!(topic = %topic, skipped = skipped, "message consumer lagged");
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });

    Ok(handle)
}

pub fn runtime_name() -> &'static str {
    "roze-mq"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn in_memory_broker_round_trips() {
        let broker = InMemoryBroker::new();
        let mut rx = broker.subscribe("events").await.expect("subscribe");
        broker
            .publish(Message::new("events", serde_json::json!({"ok": true})))
            .await
            .expect("publish");

        let received = rx.recv().await.expect("message");
        assert_eq!(received.message().topic, "events");
        assert_eq!(received.message().payload["ok"], true);
    }

    #[tokio::test]
    async fn publish_json_helper_works() {
        let broker = InMemoryBroker::new();
        let mut rx = broker.subscribe("jobs").await.expect("subscribe");
        publish_json(&broker, "jobs", serde_json::json!({"id": 1}))
            .await
            .expect("publish");

        let received = rx.recv().await.expect("message");
        assert_eq!(received.message().payload["id"], 1);
    }

    #[tokio::test]
    async fn consumer_loop_processes_messages() {
        let broker = InMemoryBroker::new();
        let (tx, rx) = tokio::sync::oneshot::channel();
        let tx = Arc::new(Mutex::new(Some(tx)));
        let tx_clone = tx.clone();

        let _handle = spawn_consumer(&broker, "tasks", move |delivery| {
            let tx = tx_clone.clone();
            async move {
                if let Some(sender) = tx.lock().expect("oneshot lock poisoned").take() {
                    let _ = sender.send(delivery.message().topic.clone());
                }
                Ok(())
            }
        })
        .await
        .expect("consumer");

        tokio::task::yield_now().await;
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        broker
            .publish(Message::new("tasks", serde_json::json!({"id": 2})))
            .await
            .expect("publish");

        let handled = tokio::time::timeout(std::time::Duration::from_secs(1), rx)
            .await
            .expect("consumer should process")
            .expect("oneshot");
        assert_eq!(handled.as_str(), "tasks");
    }

    #[tokio::test]
    async fn nack_routes_to_dead_letter() {
        let broker = InMemoryBroker::with_dead_letter("dead", 1);
        let mut dead_rx = broker.subscribe("dead").await.expect("subscribe");
        let mut rx = broker.subscribe("orders").await.expect("subscribe");
        broker
            .publish(Message::new("orders", serde_json::json!({"id": 1})).with_dead_letter_topic("dead"))
            .await
            .expect("publish");

        let first = rx.recv().await.expect("first delivery");
        assert_eq!(first.message().attempt, 1);
        first.nack().await.expect("nack");

        let dead = dead_rx.recv().await.expect("dead letter");
        assert_eq!(dead.message().topic, "dead");
        assert_eq!(dead.message().attempt, 0);
        assert_eq!(broker.dead_letters().len(), 1);
    }
}
