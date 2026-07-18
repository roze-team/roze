use std::{
    collections::{HashMap, VecDeque},
    future::Future,
    pin::Pin,
    sync::{
        atomic::{AtomicU64, AtomicU8, Ordering},
        Arc, Mutex, OnceLock,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use async_trait::async_trait;
use dashmap::{DashMap, DashSet};
use serde::{Deserialize, Serialize};
use tokio::{sync::broadcast, task::JoinHandle};

use roze_metrics::record_resilience_decision;
use roze_resilience::{
    full_jitter_delay, BreakerDecision, BreakerRegistry, GovernanceBoundary, GovernancePolicy,
    OperationKey, RateLimitRegistry, RetryBudgetRegistry, SheddingRegistry,
};

type DeliveryActionFuture = Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send>>;
type DeliveryAction = Arc<dyn Fn() -> DeliveryActionFuture + Send + Sync + 'static>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Message {
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
    pub producer: String,
    pub occurred_at: i64,
    #[serde(default)]
    pub timestamp_millis: u64,
    #[serde(default)]
    pub partition: Option<i32>,
    #[serde(default)]
    pub offset: Option<i64>,
    #[serde(default)]
    pub group: Option<String>,
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

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeadLetterQuery {
    #[serde(default)]
    pub topic: Option<String>,
    #[serde(default)]
    pub group: Option<String>,
    #[serde(default)]
    pub offset: usize,
    #[serde(default = "default_dead_letter_query_limit")]
    pub limit: usize,
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
        let trace_id = trace_id.into();
        let mut headers = HashMap::new();
        headers.insert(roze_trace::TRACE_ID_HEADER.to_string(), trace_id.clone());
        let mut trace_context = HashMap::new();
        trace_context.insert(roze_trace::TRACE_ID_HEADER.to_string(), trace_id);
        let envelope = roze_eventbus::EventEnvelope::new(topic, payload);
        Self {
            event_id: envelope.event_id,
            event_type: envelope.event_type,
            version: envelope.version,
            schema_revision: envelope.schema_revision,
            topic: envelope.topic,
            key: None,
            headers,
            trace_context,
            tenant_id: None,
            producer: "unknown".to_string(),
            occurred_at: envelope.occurred_at,
            timestamp_millis: current_millis(),
            partition: None,
            offset: None,
            group: None,
            attempt: 0,
            dead_letter_topic: None,
            idempotency_key: None,
            available_at_millis: None,
            payload: envelope.payload,
        }
    }

    pub fn with_context(
        context: &roze_context::Context,
        topic: impl Into<String>,
        payload: serde_json::Value,
    ) -> Self {
        let mut message = Self::new(topic, payload);
        message.headers = context.propagation_headers().into_iter().collect();
        message.trace_context = message.headers.clone();
        message.tenant_id = message.headers.get("x-tenant-id").cloned();
        message
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

    pub fn with_dead_letter_topic(mut self, topic: impl Into<String>) -> Self {
        self.dead_letter_topic = Some(topic.into());
        self
    }

    pub fn with_idempotency_key(mut self, key: impl Into<String>) -> Self {
        self.idempotency_key = Some(key.into());
        self
    }

    pub fn with_group(mut self, group: impl Into<String>) -> Self {
        self.group = Some(group.into());
        self
    }

    pub fn with_event_contract(
        mut self,
        event_type: impl Into<String>,
        version: u32,
        schema_revision: impl Into<String>,
        producer: impl Into<String>,
    ) -> Self {
        self.event_type = event_type.into();
        self.version = version.max(1);
        self.schema_revision = schema_revision.into();
        self.producer = producer.into();
        self
    }

    pub fn with_tenant(mut self, tenant_id: impl Into<String>) -> Self {
        self.tenant_id = Some(tenant_id.into());
        self
    }

    pub fn into_event_envelope(self) -> roze_eventbus::EventEnvelope {
        roze_eventbus::EventEnvelope {
            event_id: self.event_id,
            event_type: self.event_type,
            version: self.version,
            schema_revision: self.schema_revision,
            topic: self.topic,
            key: self.key,
            headers: self.headers,
            trace_context: self.trace_context,
            tenant_id: self.tenant_id,
            idempotency_key: self.idempotency_key,
            producer: self.producer,
            attempt: self.attempt,
            occurred_at: self.occurred_at,
            payload: self.payload,
        }
    }

    pub fn from_event_envelope(event: roze_eventbus::EventEnvelope) -> Self {
        let headers = event.transport_headers();
        Self {
            event_id: event.event_id,
            event_type: event.event_type,
            version: event.version,
            schema_revision: event.schema_revision,
            topic: event.topic,
            key: event.key,
            headers,
            trace_context: event.trace_context,
            tenant_id: event.tenant_id,
            producer: event.producer,
            occurred_at: event.occurred_at,
            timestamp_millis: current_millis(),
            partition: None,
            offset: None,
            group: None,
            attempt: event.attempt,
            dead_letter_topic: None,
            idempotency_key: event.idempotency_key,
            available_at_millis: None,
            payload: event.payload,
        }
    }

    pub fn delay_for(mut self, delay: Duration) -> Self {
        self.available_at_millis = Some(current_millis().saturating_add(delay.as_millis() as u64));
        self
    }

    /// Returns a payload-free identifier suitable for diagnostics.
    pub fn debug_id(&self) -> String {
        format!(
            "{}:{}:{}:{}",
            self.topic,
            self.partition
                .map_or_else(|| "-".to_string(), |value| value.to_string()),
            self.offset
                .map_or_else(|| "-".to_string(), |value| value.to_string()),
            self.timestamp_millis
        )
    }
}

#[derive(Debug)]
struct DeliveryState {
    state: AtomicU8,
}

const DELIVERY_PENDING: u8 = 0;
const DELIVERY_ACKED: u8 = 1;
const DELIVERY_NACKED: u8 = 2;

impl DeliveryState {
    fn new() -> Self {
        Self {
            state: AtomicU8::new(DELIVERY_PENDING),
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
        self.state.state.load(Ordering::SeqCst) == DELIVERY_ACKED
    }

    pub fn is_nacked(&self) -> bool {
        self.state.state.load(Ordering::SeqCst) == DELIVERY_NACKED
    }

    pub async fn ack(&self) -> anyhow::Result<()> {
        let transitioned = self
            .state
            .state
            .compare_exchange(
                DELIVERY_PENDING,
                DELIVERY_ACKED,
                Ordering::SeqCst,
                Ordering::SeqCst,
            )
            .is_ok();
        tracing::debug!(
            protocol = "stream",
            topic = %self.message.topic,
            message_id = %self.message.debug_id(),
            attempt = self.message.attempt,
            decision = if transitioned { "ack" } else { "ack_ignored_already_settled" },
            "stream delivery settlement decided"
        );
        if transitioned {
            (self.ack_fn)().await?;
        }
        Ok(())
    }

    pub async fn nack(&self) -> anyhow::Result<()> {
        let transitioned = self
            .state
            .state
            .compare_exchange(
                DELIVERY_PENDING,
                DELIVERY_NACKED,
                Ordering::SeqCst,
                Ordering::SeqCst,
            )
            .is_ok();
        tracing::debug!(
            protocol = "stream",
            topic = %self.message.topic,
            message_id = %self.message.debug_id(),
            attempt = self.message.attempt,
            decision = if transitioned { "nack" } else { "nack_ignored_already_settled" },
            "stream delivery settlement decided"
        );
        if transitioned {
            (self.nack_fn)().await?;
        }
        Ok(())
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
    async fn dead_letters_query(
        &self,
        query: DeadLetterQuery,
    ) -> anyhow::Result<Vec<DeadLetterRecord>> {
        Ok(self
            .dead_letters(query.offset, query.limit)
            .await?
            .into_iter()
            .filter(|record| {
                query
                    .topic
                    .as_ref()
                    .is_none_or(|topic| &record.original_topic == topic)
            })
            .filter(|record| {
                query
                    .group
                    .as_ref()
                    .is_none_or(|group| record.message.group.as_ref() == Some(group))
            })
            .collect())
    }
    async fn replay_dead_letter(&self, id: u64) -> anyhow::Result<Option<Message>>;
    async fn purge_dead_letter(&self, id: u64) -> anyhow::Result<Option<DeadLetterRecord>>;
    async fn clear_dead_letters(&self) -> anyhow::Result<usize>;
}

#[derive(Debug, Clone)]
pub struct InMemoryBroker {
    topics: Arc<DashMap<String, broadcast::Sender<Delivery>>>,
    topic_offsets: Arc<DashMap<String, i64>>,
    dead_letters: Arc<Mutex<VecDeque<DeadLetterRecord>>>,
    seen_idempotency_keys: Arc<DashSet<String>>,
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
            topics: Arc::new(DashMap::new()),
            topic_offsets: Arc::new(DashMap::new()),
            dead_letters: Arc::new(Mutex::new(VecDeque::new())),
            seen_idempotency_keys: Arc::new(DashSet::new()),
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
        self.topics
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
        self.prepare_delivery_metadata(&mut message);
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
        let duplicate = !self.seen_idempotency_keys.insert(key.to_string());
        if duplicate {
            self.duplicated.fetch_add(1, Ordering::SeqCst);
        }
        duplicate
    }

    fn prepare_delivery_metadata(&self, message: &mut Message) {
        if message.timestamp_millis == 0 {
            message.timestamp_millis = current_millis();
        }
        if message.partition.is_none() {
            message.partition = Some(0);
        }
        if message.offset.is_none() {
            let mut offset = self.topic_offsets.entry(message.topic.clone()).or_insert(0);
            message.offset = Some(*offset);
            *offset = offset.saturating_add(1);
        }
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
            message.idempotency_key = None;
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
    spawn_consumer_with_governance(subscriber, topic, None, handler).await
}

pub async fn spawn_consumer_with_governance<S, F, Fut>(
    subscriber: &S,
    topic: impl Into<String>,
    governance: Option<GovernancePolicy>,
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
    let policy = governance.unwrap_or_default();
    let handle = tokio::spawn(async move {
        loop {
            match receiver.recv().await {
                Ok(delivery) => {
                    let result =
                        execute_governed_delivery(&topic, &policy, handler.as_ref(), &delivery)
                            .await;
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

async fn execute_governed_delivery<F, Fut>(
    topic: &str,
    policy: &GovernancePolicy,
    handler: &F,
    delivery: &Delivery,
) -> anyhow::Result<()>
where
    F: Fn(Delivery) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = anyhow::Result<()>> + Send + 'static,
{
    static RATE_LIMITERS: OnceLock<RateLimitRegistry> = OnceLock::new();
    static BREAKERS: OnceLock<BreakerRegistry> = OnceLock::new();
    static SHEDDERS: OnceLock<SheddingRegistry> = OnceLock::new();
    static RETRY_BUDGETS: OnceLock<RetryBudgetRegistry> = OnceLock::new();

    let key = OperationKey::new("consumer", GovernanceBoundary::Mq, topic).to_string();
    if let Some(config) = policy.rate_limit {
        let allowed = RATE_LIMITERS
            .get_or_init(RateLimitRegistry::new)
            .allow(key.clone(), config);
        record_resilience_decision(
            topic,
            "mq",
            "rate_limit",
            if allowed { "allowed" } else { "rejected" },
        );
        if !allowed {
            anyhow::bail!("message rejected by rate limit");
        }
    }

    let breaker = policy.breaker;
    let breaker_permit = if breaker.is_some() {
        match BREAKERS
            .get_or_init(BreakerRegistry::new)
            .allow(key.clone())
        {
            BreakerDecision::Allow(permit) => {
                record_resilience_decision(topic, "mq", "breaker", "allowed");
                Some(permit)
            }
            BreakerDecision::Reject => {
                record_resilience_decision(topic, "mq", "breaker", "open");
                anyhow::bail!("message rejected by open circuit breaker");
            }
        }
    } else {
        None
    };

    let shedding = policy.shedding;
    if let Some(config) = shedding {
        let allowed = SHEDDERS
            .get_or_init(SheddingRegistry::new)
            .allow(key.clone(), config);
        record_resilience_decision(
            topic,
            "mq",
            "load_shedding",
            if allowed { "allowed" } else { "shed" },
        );
        if !allowed {
            if let (Some(config), Some(permit)) = (breaker, breaker_permit) {
                BREAKERS
                    .get_or_init(BreakerRegistry::new)
                    .cancel(&key, permit, config);
            }
            anyhow::bail!("message rejected by load shedding");
        }
    }

    let started = Instant::now();
    let retry = policy.retry.unwrap_or_default();
    let max_attempts = retry.max_attempts.max(1);
    let budgets = RETRY_BUDGETS.get_or_init(RetryBudgetRegistry::default);
    budgets.record_call(&key);
    let mut attempt = 1;
    let result = loop {
        let future = handler(delivery.clone());
        let result = match policy.timeout {
            Some(timeout) => tokio::time::timeout(timeout.max(Duration::from_millis(1)), future)
                .await
                .map_err(|_| anyhow::anyhow!("message handler timed out"))
                .and_then(|result| result),
            None => future.await,
        };
        if result.is_ok() || attempt >= max_attempts {
            break result;
        }
        if !budgets.allow_retry(&key, retry.budget_percent) {
            record_resilience_decision(topic, "mq", "retry_budget", "exhausted");
            break result;
        }
        record_resilience_decision(topic, "mq", "retry", "scheduled");
        tokio::time::sleep(full_jitter_delay(
            retry.backoff,
            retry.max_backoff,
            attempt as usize,
        ))
        .await;
        attempt += 1;
    };

    let success = result.is_ok();
    if let (Some(config), Some(permit)) = (breaker, breaker_permit) {
        let breakers = BREAKERS.get_or_init(BreakerRegistry::new);
        if success {
            breakers.record_success(key.clone(), permit);
        } else {
            breakers.record_failure(key.clone(), permit, config);
        }
    }
    if let Some(config) = shedding {
        SHEDDERS
            .get_or_init(SheddingRegistry::new)
            .record(key, success, started.elapsed(), config);
    }
    result
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

fn default_dead_letter_query_limit() -> usize {
    100
}

fn delivery_delay(message: &Message) -> Option<Duration> {
    let available_at = message.available_at_millis?;
    let now = current_millis();
    (available_at > now).then(|| Duration::from_millis(available_at - now))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    fn test_delivery(topic: &str) -> Delivery {
        let action: DeliveryAction = Arc::new(|| Box::pin(async { Ok(()) }));
        Delivery::external(
            Message::new(topic, serde_json::json!({"id": 1})),
            action.clone(),
            action,
        )
    }

    #[tokio::test]
    async fn governed_delivery_retries_within_budget() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let handler = {
            let attempts = attempts.clone();
            move |_delivery: Delivery| {
                let attempts = attempts.clone();
                async move {
                    let attempt = attempts.fetch_add(1, Ordering::SeqCst) + 1;
                    if attempt == 1 {
                        anyhow::bail!("transient failure");
                    }
                    Ok(())
                }
            }
        };
        let policy = GovernancePolicy {
            retry: Some(roze_resilience::RetryPolicy {
                max_attempts: 2,
                budget_percent: Some(100),
                ..roze_resilience::RetryPolicy::default()
            }),
            ..GovernancePolicy::default()
        };

        execute_governed_delivery(
            "governed-retry-test",
            &policy,
            &handler,
            &test_delivery("governed-retry-test"),
        )
        .await
        .expect("second attempt succeeds");
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn governed_delivery_enforces_timeout() {
        let policy = GovernancePolicy {
            timeout: Some(Duration::from_millis(1)),
            ..GovernancePolicy::default()
        };
        let error = execute_governed_delivery(
            "governed-timeout-test",
            &policy,
            &|_delivery| async {
                tokio::time::sleep(Duration::from_secs(1)).await;
                Ok(())
            },
            &test_delivery("governed-timeout-test"),
        )
        .await
        .expect_err("handler must time out");
        assert!(error.to_string().contains("timed out"));
    }

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
        assert!(received.message().timestamp_millis > 0);
        assert_eq!(received.message().partition, Some(0));
        assert_eq!(received.message().offset, Some(0));
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

    #[test]
    fn message_round_trips_context_headers() {
        let ctx =
            roze_context::Context::background_with_request_id_and_trace_id("request-1", "trace-1")
                .with_locale("zh-CN");
        let message = Message::with_context(&ctx, "events", serde_json::json!({"ok": true}));
        let restored = message.context();

        assert_eq!(restored.request_id(), "request-1");
        assert_eq!(restored.trace_id(), "trace-1");
        assert_eq!(restored.locale().as_deref(), Some("zh-CN"));
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
    async fn ack_is_idempotent_and_blocks_later_nack() {
        let broker = InMemoryBroker::new();
        let mut rx = broker.subscribe("orders").await.expect("subscribe");
        broker
            .publish(Message::new("orders", serde_json::json!({"id": 1})))
            .await
            .expect("publish");

        let delivery = rx.recv().await.expect("delivery");
        delivery.ack().await.expect("first ack");
        delivery.ack().await.expect("second ack");
        delivery.nack().await.expect("late nack");

        assert!(delivery.is_acked());
        assert!(!delivery.is_nacked());
        let stats = broker.stats().await.expect("stats");
        assert_eq!(stats.acked, 1);
        assert_eq!(stats.nacked, 0);
        assert_eq!(stats.dead_lettered, 0);
    }

    #[tokio::test]
    async fn nack_is_idempotent_and_blocks_later_ack() {
        let broker = InMemoryBroker::with_dead_letter("dead", 1);
        let mut dead_rx = broker.subscribe("dead").await.expect("subscribe dead");
        let mut rx = broker.subscribe("orders").await.expect("subscribe orders");
        broker
            .publish(
                Message::new("orders", serde_json::json!({"id": 1})).with_dead_letter_topic("dead"),
            )
            .await
            .expect("publish");

        let delivery = rx.recv().await.expect("delivery");
        delivery.nack().await.expect("first nack");
        delivery.nack().await.expect("second nack");
        delivery.ack().await.expect("late ack");

        assert!(delivery.is_nacked());
        assert!(!delivery.is_acked());
        let dead = dead_rx.recv().await.expect("dead letter");
        assert_eq!(dead.message().topic, "dead");
        let stats = broker.stats().await.expect("stats");
        assert_eq!(stats.acked, 0);
        assert_eq!(stats.nacked, 1);
        assert_eq!(stats.dead_lettered, 1);
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

    #[test]
    fn message_and_event_envelope_round_trip_all_business_metadata() {
        let message = Message::new("orders", serde_json::json!({"id": 7}))
            .with_event_contract("order.created", 2, "2026-07", "order-service")
            .with_tenant("acme")
            .with_idempotency_key("order-7");
        let expected_id = message.event_id.clone();
        let round_trip = Message::from_event_envelope(message.into_event_envelope());
        assert_eq!(round_trip.event_id, expected_id);
        assert_eq!(round_trip.event_type, "order.created");
        assert_eq!(round_trip.version, 2);
        assert_eq!(round_trip.schema_revision, "2026-07");
        assert_eq!(round_trip.tenant_id.as_deref(), Some("acme"));
        assert_eq!(round_trip.idempotency_key.as_deref(), Some("order-7"));
        assert_eq!(round_trip.producer, "order-service");
    }

    #[tokio::test]
    async fn admin_filters_dead_letters_by_topic_and_group() {
        let broker = InMemoryBroker::with_dead_letter("dead", 1);
        let mut orders = broker.subscribe("orders").await.expect("subscribe orders");
        let mut invoices = broker
            .subscribe("invoices")
            .await
            .expect("subscribe invoices");

        broker
            .publish(Message::new("orders", serde_json::json!({"id": 1})).with_group("billing"))
            .await
            .expect("publish order");
        orders
            .recv()
            .await
            .expect("order")
            .nack()
            .await
            .expect("nack");

        broker
            .publish(Message::new("invoices", serde_json::json!({"id": 2})).with_group("billing"))
            .await
            .expect("publish invoice");
        invoices
            .recv()
            .await
            .expect("invoice")
            .nack()
            .await
            .expect("nack");

        let records = broker
            .dead_letters_query(DeadLetterQuery {
                topic: Some("orders".to_string()),
                group: Some("billing".to_string()),
                offset: 0,
                limit: 10,
            })
            .await
            .expect("dead letters");

        assert_eq!(records.len(), 1);
        assert_eq!(records[0].original_topic, "orders");
        assert_eq!(records[0].message.group.as_deref(), Some("billing"));
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

    #[tokio::test]
    #[ignore = "production-soak: set ROZE_MQ_SOAK_SECONDS/ROZE_MQ_SOAK_MESSAGES for long runs"]
    async fn production_soak_in_memory_broker() {
        let seconds = std::env::var("ROZE_MQ_SOAK_SECONDS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(5);
        let max_messages = std::env::var("ROZE_MQ_SOAK_MESSAGES")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(1_000);
        let broker = InMemoryBroker::with_dead_letter("dead", 1);
        let mut orders = broker.subscribe("orders").await.expect("subscribe orders");
        let mut dead = broker.subscribe("dead").await.expect("subscribe dead");
        let started = std::time::Instant::now();
        let deadline = started + std::time::Duration::from_secs(seconds);
        let mut sent = 0u64;
        let mut acked = 0u64;
        let mut nacked = 0u64;
        let mut delivery_latency = roze_metrics::LatencyHistogram::new();

        while std::time::Instant::now() < deadline && sent < max_messages {
            let delivery_started = std::time::Instant::now();
            let message = Message::new("orders", serde_json::json!({ "id": sent }))
                .with_group("soak")
                .with_dead_letter_topic("dead")
                .with_idempotency_key(format!("soak-{sent}"));
            broker.publish(message.clone()).await.expect("publish");
            if sent.is_multiple_of(97) {
                broker.publish(message).await.expect("publish duplicate");
            }

            let delivery = tokio::time::timeout(std::time::Duration::from_secs(1), orders.recv())
                .await
                .expect("delivery timeout")
                .expect("delivery");
            if sent.is_multiple_of(13) {
                delivery.nack().await.expect("nack");
                let dead_delivery =
                    tokio::time::timeout(std::time::Duration::from_secs(1), dead.recv())
                        .await
                        .expect("dead letter timeout")
                        .expect("dead letter delivery");
                dead_delivery.ack().await.expect("ack dead letter");
                nacked += 1;
            } else {
                delivery.ack().await.expect("ack");
                acked += 1;
            }
            delivery_latency.observe(delivery_started.elapsed());
            sent += 1;
        }

        let records = broker
            .dead_letters_query(DeadLetterQuery {
                topic: Some("orders".to_string()),
                group: Some("soak".to_string()),
                offset: 0,
                limit: 500,
            })
            .await
            .expect("dead letters");
        let mut replayed = 0_u64;
        let mut replay_recovery_us = 0_u128;
        if let Some(record) = records.first() {
            let replay_started = std::time::Instant::now();
            let mut replay_rx = broker.subscribe("orders").await.expect("subscribe replay");
            broker
                .replay_dead_letter(record.id)
                .await
                .expect("replay")
                .expect("replayed message");
            let replayed_delivery =
                tokio::time::timeout(std::time::Duration::from_secs(1), replay_rx.recv())
                    .await
                    .expect("replay timeout")
                    .expect("replayed delivery");
            replayed_delivery.ack().await.expect("ack replay");
            replayed = 1;
            replay_recovery_us = replay_started.elapsed().as_micros().max(1);
        }

        let stats = broker.stats().await.expect("stats");
        let elapsed_ms = started.elapsed().as_millis().max(1);
        let messages_per_second_milli = u128::from(sent).saturating_mul(1_000_000) / elapsed_ms;
        let p50_delivery_us = delivery_latency
            .percentile_upper_bound_micros(50)
            .expect("delivery latency");
        let p95_delivery_us = delivery_latency
            .percentile_upper_bound_micros(95)
            .expect("delivery latency");
        let p99_delivery_us = delivery_latency
            .percentile_upper_bound_micros(99)
            .expect("delivery latency");
        println!(
            "roze_mq_soak elapsed_ms={elapsed_ms} sent={sent} acked={acked} nacked={nacked} messages_per_second_milli={messages_per_second_milli} p50_delivery_us={p50_delivery_us} p95_delivery_us={p95_delivery_us} p99_delivery_us={p99_delivery_us} replayed={replayed} replay_recovery_us={replay_recovery_us} published={} duplicated={} dead_lettered={}",
            stats.published, stats.duplicated, stats.dead_lettered
        );

        assert!(sent > 0, "soak must send at least one message");
        assert_eq!(delivery_latency.count(), sent);
        assert_eq!(replayed, 1);
        assert_eq!(stats.published, sent + ((sent.saturating_sub(1)) / 97 + 1));
        assert_eq!(stats.acked, acked + nacked + u64::from(!records.is_empty()));
        assert_eq!(stats.nacked, nacked);
        assert_eq!(stats.dead_lettered, nacked);
        assert!(stats.duplicated > 0);
    }
}
