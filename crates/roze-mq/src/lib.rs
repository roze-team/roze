use std::{
    collections::{HashMap, HashSet, VecDeque},
    future::Future,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::{sync::broadcast, task::JoinHandle};

type DeliveryActionFuture = Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send>>;
type DeliveryAction = Arc<dyn Fn() -> DeliveryActionFuture + Send + Sync + 'static>;

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
    #[serde(default)]
    pub idempotency_key: Option<String>,
    #[serde(default)]
    pub available_at_millis: Option<u64>,
    pub payload: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeadLetterRecord {
    pub id: u64,
    pub original_topic: String,
    pub reason: String,
    pub failed_at_millis: u64,
    pub replay_count: u32,
    pub message: Message,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct MqStats {
    pub published: u64,
    pub delivered: u64,
    pub acked: u64,
    pub nacked: u64,
    pub duplicated: u64,
    pub dead_lettered: u64,
    pub replayed: u64,
    pub dead_letter_pending: u64,
}

impl Message {
    pub fn new(topic: impl Into<String>, payload: serde_json::Value) -> Self {
        Self::with_trace_id(topic, payload, roze_trace::generate_trace_id())
    }

    pub fn with_trace_id(
        topic: impl Into<String>,
        payload: serde_json::Value,
        trace_id: impl Into<String>,
    ) -> Self {
        let mut headers = HashMap::new();
        headers.insert(roze_trace::TRACE_ID_HEADER.to_string(), trace_id.into());
        Self {
            topic: topic.into(),
            key: None,
            headers,
            attempt: 0,
            dead_letter_topic: None,
            idempotency_key: None,
            available_at_millis: None,
            payload,
        }
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

    pub fn with_dead_letter_topic(mut self, topic: impl Into<String>) -> Self {
        self.dead_letter_topic = Some(topic.into());
        self
    }

    pub fn with_idempotency_key(mut self, key: impl Into<String>) -> Self {
        self.idempotency_key = Some(key.into());
        self
    }

    pub fn delay_for(mut self, delay: Duration) -> Self {
        self.available_at_millis = Some(current_millis().saturating_add(delay.as_millis() as u64));
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

#[derive(Clone)]
pub struct Delivery {
    message: Message,
    state: Arc<DeliveryState>,
    ack_fn: DeliveryAction,
    nack_fn: DeliveryAction,
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
        (self.ack_fn)().await
    }

    pub async fn nack(&self) -> anyhow::Result<()> {
        self.state.nacked.store(true, Ordering::SeqCst);
        (self.nack_fn)().await
    }

    pub fn external(message: Message, ack_fn: DeliveryAction, nack_fn: DeliveryAction) -> Self {
        Self {
            message,
            state: Arc::new(DeliveryState::new()),
            ack_fn,
            nack_fn,
        }
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

#[async_trait]
pub trait MqAdmin: Send + Sync + 'static {
    async fn stats(&self) -> anyhow::Result<MqStats>;
    async fn dead_letters(
        &self,
        offset: usize,
        limit: usize,
    ) -> anyhow::Result<Vec<DeadLetterRecord>>;
    async fn replay_dead_letter(&self, id: u64) -> anyhow::Result<Option<Message>>;
    async fn purge_dead_letter(&self, id: u64) -> anyhow::Result<Option<DeadLetterRecord>>;
    async fn clear_dead_letters(&self) -> anyhow::Result<usize>;
}

#[derive(Debug, Clone)]
pub struct InMemoryBroker {
    topics: Arc<Mutex<HashMap<String, broadcast::Sender<Delivery>>>>,
    dead_letters: Arc<Mutex<VecDeque<DeadLetterRecord>>>,
    seen_idempotency_keys: Arc<Mutex<HashSet<String>>>,
    dead_letter_topic: Option<String>,
    max_attempts: u32,
    next_dead_letter_id: Arc<AtomicU64>,
    published: Arc<AtomicU64>,
    delivered: Arc<AtomicU64>,
    acked: Arc<AtomicU64>,
    nacked: Arc<AtomicU64>,
    duplicated: Arc<AtomicU64>,
    dead_lettered: Arc<AtomicU64>,
    replayed: Arc<AtomicU64>,
}

impl InMemoryBroker {
    pub fn new() -> Self {
        Self {
            topics: Arc::new(Mutex::new(HashMap::new())),
            dead_letters: Arc::new(Mutex::new(VecDeque::new())),
            seen_idempotency_keys: Arc::new(Mutex::new(HashSet::new())),
            dead_letter_topic: None,
            max_attempts: 3,
            next_dead_letter_id: Arc::new(AtomicU64::new(1)),
            published: Arc::new(AtomicU64::new(0)),
            delivered: Arc::new(AtomicU64::new(0)),
            acked: Arc::new(AtomicU64::new(0)),
            nacked: Arc::new(AtomicU64::new(0)),
            duplicated: Arc::new(AtomicU64::new(0)),
            dead_lettered: Arc::new(AtomicU64::new(0)),
            replayed: Arc::new(AtomicU64::new(0)),
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
        self.dead_letters
            .lock()
            .expect("broker lock poisoned")
            .iter()
            .map(|record| record.message.clone())
            .collect()
    }

    pub fn dead_letter_records(&self) -> Vec<DeadLetterRecord> {
        self.dead_letters
            .lock()
            .expect("broker lock poisoned")
            .iter()
            .cloned()
            .collect()
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
        if self.is_duplicate(&message) {
            return Ok(());
        }
        if let Some(delay) = delivery_delay(&message) {
            let broker = self.clone();
            tokio::spawn(async move {
                tokio::time::sleep(delay).await;
                message.available_at_millis = None;
                let _ = broker.route_delivery_now(message).await;
            });
            return Ok(());
        }
        self.route_delivery_now(message).await
    }

    async fn route_delivery_now(&self, mut message: Message) -> anyhow::Result<()> {
        message.attempt = message.attempt.saturating_add(1);
        if message.attempt > self.max_attempts {
            self.push_dead_letter(message.clone(), "max_attempts_exceeded");
            if let Some(dead_letter_topic) = &message.dead_letter_topic {
                let mut routed = message.clone();
                routed.topic = dead_letter_topic.clone();
                routed.attempt = 0;
                let _ = self
                    .sender_for(dead_letter_topic)
                    .send(Delivery::new(routed, self.clone()));
            }
            return Ok(());
        }

        let sender = self.sender_for(&message.topic);
        let _ = sender.send(Delivery::new(message, self.clone()));
        self.delivered.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn is_duplicate(&self, message: &Message) -> bool {
        let Some(key) = message.idempotency_key.as_deref() else {
            return false;
        };
        let mut seen = self
            .seen_idempotency_keys
            .lock()
            .expect("broker lock poisoned");
        let duplicate = !seen.insert(key.to_string());
        if duplicate {
            self.duplicated.fetch_add(1, Ordering::SeqCst);
        }
        duplicate
    }

    async fn requeue_or_dead_letter(&self, message: Message) -> anyhow::Result<()> {
        self.nacked.fetch_add(1, Ordering::SeqCst);
        let mut next = message.clone();
        next.attempt = next.attempt.saturating_add(1);
        if next.attempt > self.max_attempts {
            self.push_dead_letter(next.clone(), "nack_max_attempts_exceeded");
            if let Some(dead_letter_topic) = next
                .dead_letter_topic
                .clone()
                .or_else(|| self.dead_letter_topic.clone())
            {
                let mut routed = next;
                routed.topic = dead_letter_topic.clone();
                routed.attempt = 0;
                let _ = self
                    .sender_for(&dead_letter_topic)
                    .send(Delivery::new(routed, self.clone()));
            }
            return Ok(());
        }

        let _ = self
            .sender_for(&next.topic)
            .send(Delivery::new(next, self.clone()));
        self.delivered.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn push_dead_letter(&self, message: Message, reason: impl Into<String>) {
        let record = DeadLetterRecord {
            id: self.next_dead_letter_id.fetch_add(1, Ordering::SeqCst),
            original_topic: message.topic.clone(),
            reason: reason.into(),
            failed_at_millis: current_millis(),
            replay_count: 0,
            message,
        };
        self.dead_letters
            .lock()
            .expect("broker lock poisoned")
            .push_back(record);
        self.dead_lettered.fetch_add(1, Ordering::SeqCst);
    }
}

impl Default for InMemoryBroker {
    fn default() -> Self {
        Self::new()
    }
}

impl Delivery {
    fn new(message: Message, broker: InMemoryBroker) -> Self {
        let ack_broker = broker.clone();
        let ack_fn: DeliveryAction = Arc::new(move || {
            let broker = ack_broker.clone();
            Box::pin(async move {
                broker.acked.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
        });
        let nack_broker = broker.clone();
        let message_for_nack = message.clone();
        let nack_fn: DeliveryAction = Arc::new(move || {
            let broker = nack_broker.clone();
            let message = message_for_nack.clone();
            Box::pin(async move { broker.requeue_or_dead_letter(message).await })
        });
        Self {
            message,
            state: Arc::new(DeliveryState::new()),
            ack_fn,
            nack_fn,
        }
    }
}

#[async_trait]
impl Publisher for InMemoryBroker {
    async fn publish(&self, mut message: Message) -> anyhow::Result<()> {
        message.ensure_trace_id();
        self.published.fetch_add(1, Ordering::SeqCst);
        self.route_delivery(message).await
    }
}

#[async_trait]
impl Subscriber for InMemoryBroker {
    async fn subscribe(&self, topic: &str) -> anyhow::Result<broadcast::Receiver<Delivery>> {
        Ok(self.sender_for(topic).subscribe())
    }
}

#[async_trait]
impl MqAdmin for InMemoryBroker {
    async fn stats(&self) -> anyhow::Result<MqStats> {
        Ok(MqStats {
            published: self.published.load(Ordering::SeqCst),
            delivered: self.delivered.load(Ordering::SeqCst),
            acked: self.acked.load(Ordering::SeqCst),
            nacked: self.nacked.load(Ordering::SeqCst),
            duplicated: self.duplicated.load(Ordering::SeqCst),
            dead_lettered: self.dead_lettered.load(Ordering::SeqCst),
            replayed: self.replayed.load(Ordering::SeqCst),
            dead_letter_pending: self
                .dead_letters
                .lock()
                .expect("broker lock poisoned")
                .len() as u64,
        })
    }

    async fn dead_letters(
        &self,
        offset: usize,
        limit: usize,
    ) -> anyhow::Result<Vec<DeadLetterRecord>> {
        let limit = limit.clamp(1, 500);
        Ok(self
            .dead_letters
            .lock()
            .expect("broker lock poisoned")
            .iter()
            .skip(offset)
            .take(limit)
            .cloned()
            .collect())
    }

    async fn replay_dead_letter(&self, id: u64) -> anyhow::Result<Option<Message>> {
        let mut message = {
            let mut dead_letters = self.dead_letters.lock().expect("broker lock poisoned");
            let Some(record) = dead_letters.iter_mut().find(|record| record.id == id) else {
                return Ok(None);
            };
            record.replay_count = record.replay_count.saturating_add(1);
            let mut message = record.message.clone();
            message.topic = record.original_topic.clone();
            message.attempt = 0;
            message.available_at_millis = None;
            message
        };
        message.ensure_trace_id();
        self.replayed.fetch_add(1, Ordering::SeqCst);
        self.route_delivery(message.clone()).await?;
        Ok(Some(message))
    }

    async fn purge_dead_letter(&self, id: u64) -> anyhow::Result<Option<DeadLetterRecord>> {
        let mut dead_letters = self.dead_letters.lock().expect("broker lock poisoned");
        let Some(index) = dead_letters.iter().position(|record| record.id == id) else {
            return Ok(None);
        };
        Ok(dead_letters.remove(index))
    }

    async fn clear_dead_letters(&self) -> anyhow::Result<usize> {
        let mut dead_letters = self.dead_letters.lock().expect("broker lock poisoned");
        let len = dead_letters.len();
        dead_letters.clear();
        Ok(len)
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

fn current_millis() -> u64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(elapsed) => elapsed.as_millis() as u64,
        Err(_) => 0,
    }
}

fn delivery_delay(message: &Message) -> Option<Duration> {
    let available_at = message.available_at_millis?;
    let now = current_millis();
    (available_at > now).then(|| Duration::from_millis(available_at - now))
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
        let trace_id = received
            .message()
            .headers
            .get(roze_trace::TRACE_ID_HEADER)
            .expect("trace header");
        assert_eq!(
            uuid::Uuid::parse_str(trace_id).unwrap().get_version_num(),
            7
        );
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
            .publish(
                Message::new("orders", serde_json::json!({"id": 1})).with_dead_letter_topic("dead"),
            )
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

    #[tokio::test]
    async fn admin_lists_replays_and_purges_dead_letters() {
        let broker = InMemoryBroker::with_dead_letter("dead", 1);
        let mut rx = broker.subscribe("orders").await.expect("subscribe orders");
        let mut replay_rx = broker.subscribe("orders").await.expect("subscribe replay");
        broker
            .publish(Message::new("orders", serde_json::json!({"id": 7})))
            .await
            .expect("publish");

        let first = rx.recv().await.expect("first");
        first.nack().await.expect("nack");

        let records = MqAdmin::dead_letters(&broker, 0, 10)
            .await
            .expect("dead letters");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].original_topic, "orders");
        assert_eq!(records[0].reason, "nack_max_attempts_exceeded");

        let replayed = broker
            .replay_dead_letter(records[0].id)
            .await
            .expect("replay")
            .expect("message");
        assert_eq!(replayed.topic, "orders");
        let delivery = replay_rx.recv().await.expect("replayed delivery");
        assert_eq!(delivery.message().payload["id"], 7);

        let stats = broker.stats().await.expect("stats");
        assert_eq!(stats.published, 1);
        assert_eq!(stats.nacked, 1);
        assert_eq!(stats.dead_lettered, 1);
        assert_eq!(stats.replayed, 1);
        assert_eq!(stats.dead_letter_pending, 1);

        let purged = broker
            .purge_dead_letter(records[0].id)
            .await
            .expect("purge")
            .expect("record");
        assert_eq!(purged.id, records[0].id);
        assert_eq!(broker.clear_dead_letters().await.expect("clear"), 0);
    }

    #[tokio::test]
    async fn idempotency_key_deduplicates_messages() {
        let broker = InMemoryBroker::new();
        let mut rx = broker.subscribe("events").await.expect("subscribe");
        let first =
            Message::new("events", serde_json::json!({"id": 1})).with_idempotency_key("event-1");
        broker.publish(first.clone()).await.expect("publish first");
        broker.publish(first).await.expect("publish duplicate");

        let received = rx.recv().await.expect("message");
        assert_eq!(received.message().payload["id"], 1);
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn delayed_message_is_not_delivered_immediately() {
        let broker = InMemoryBroker::new();
        let mut rx = broker.subscribe("events").await.expect("subscribe");
        broker
            .publish(
                Message::new("events", serde_json::json!({"id": 1}))
                    .delay_for(std::time::Duration::from_millis(30)),
            )
            .await
            .expect("publish");

        assert!(rx.try_recv().is_err());
        let received = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
            .await
            .expect("message should arrive")
            .expect("message");
        assert_eq!(received.message().payload["id"], 1);
    }
}
