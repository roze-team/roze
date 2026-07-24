use serde::{Deserialize, Serialize};
#[cfg(feature = "rskafka")]
use std::collections::BTreeMap;
use std::fmt;
#[cfg(feature = "rdkafka")]
use std::time::Duration;
use std::{
    collections::{HashMap, VecDeque},
    future::Future,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::{SystemTime, UNIX_EPOCH},
};
#[cfg(feature = "rdkafka")]
use tokio::sync::{mpsc, oneshot};
use tokio::{sync::broadcast, task::JoinHandle};

#[cfg(feature = "rdkafka")]
use rdkafka::{
    config::ClientConfig,
    consumer::{CommitMode, Consumer, StreamConsumer},
    message::{Header, Headers, Message},
    producer::{FutureProducer, FutureRecord, Producer},
    util::Timeout,
    Offset, TopicPartitionList,
};
#[cfg(feature = "rdkafka")]
use regex::Regex;

#[cfg(feature = "rdkafka")]
use rdkafka::message::OwnedHeaders;
#[cfg(feature = "rskafka")]
use rskafka::{
    client::{
        partition::{Compression, UnknownTopicHandling},
        Client, ClientBuilder,
    },
    record::Record as RustNativeRecord,
};

type DeliveryActionFuture = Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send>>;
type DeliveryAction = Arc<dyn Fn() -> DeliveryActionFuture + Send + Sync + 'static>;

/// Selects the Kafka implementation used by the framework runtime.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum KafkaProvider {
    #[default]
    Memory,
    Rdkafka,
    #[serde(alias = "rskafka")]
    RustNative,
}

impl fmt::Display for KafkaProvider {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Memory => "memory",
            Self::Rdkafka => "rdkafka",
            Self::RustNative => "rust-native",
        })
    }
}

/// Declares provider behavior that applications may validate before startup.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct KafkaCapabilities {
    pub publish: bool,
    pub subscribe: bool,
    pub consumer_groups: bool,
    pub manual_ack: bool,
    pub offset_commit: bool,
    pub rebalance: bool,
    pub transactions: bool,
}

impl KafkaProvider {
    pub const fn capabilities(self) -> KafkaCapabilities {
        match self {
            Self::Memory => KafkaCapabilities {
                publish: true,
                subscribe: true,
                consumer_groups: false,
                manual_ack: true,
                offset_commit: false,
                rebalance: false,
                transactions: false,
            },
            Self::Rdkafka => KafkaCapabilities {
                publish: true,
                subscribe: true,
                consumer_groups: true,
                manual_ack: true,
                offset_commit: true,
                rebalance: true,
                transactions: false,
            },
            Self::RustNative => KafkaCapabilities {
                publish: true,
                subscribe: false,
                consumer_groups: false,
                manual_ack: false,
                offset_commit: false,
                rebalance: false,
                transactions: false,
            },
        }
    }
}

/// Fail-fast errors returned while selecting or constructing a Kafka provider.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum KafkaRuntimeError {
    #[error(
        "Kafka provider `{provider}` requires Cargo feature `{feature}`; enable it on `roze-kafka`"
    )]
    FeatureDisabled {
        provider: KafkaProvider,
        feature: &'static str,
    },
    #[error("Kafka provider `{provider}` does not support required capability `{capability}`")]
    UnsupportedCapability {
        provider: KafkaProvider,
        capability: &'static str,
    },
    #[error("Kafka provider `{provider}` failed to initialize: {message}")]
    Initialization {
        provider: KafkaProvider,
        message: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct KafkaConfig {
    /// Explicit provider selection. When omitted, enabled Cargo features retain
    /// the legacy memory-first selection behavior.
    #[serde(default)]
    pub provider: Option<KafkaProvider>,
    #[serde(default)]
    pub brokers: Vec<String>,
    #[serde(default, alias = "bootstrap")]
    pub bootstrap: Option<String>,
    #[serde(default)]
    pub bootstrap_servers: Option<Vec<String>>,
    #[serde(default)]
    pub client_id: Option<String>,
    #[serde(default)]
    pub group_id: Option<String>,
    #[serde(default, alias = "group")]
    pub group: Option<String>,
    #[serde(default)]
    pub topic_prefix: String,
    #[serde(default = "default_acks")]
    pub acks: String,
    #[serde(default = "default_auto_offset_reset")]
    pub auto_offset_reset: String,
    #[serde(default = "default_enable_manual_ack")]
    pub enable_manual_ack: bool,
    #[serde(default = "default_enable_auto_commit", alias = "auto_commit")]
    pub enable_auto_commit: bool,
    #[serde(default = "default_session_timeout_ms")]
    pub session_timeout_ms: u64,
    #[serde(default = "default_heartbeat_interval_ms")]
    pub heartbeat_interval_ms: u64,
    #[serde(default = "default_max_poll_interval_ms")]
    pub max_poll_interval_ms: u64,
    #[serde(default = "default_flush_timeout_ms")]
    pub flush_timeout_ms: u64,
    #[serde(default = "default_message_timeout_ms")]
    pub message_timeout_ms: u64,
    #[serde(default = "default_linger_ms")]
    pub linger_ms: u64,
    #[serde(default = "default_batch_size")]
    pub batch_size: usize,
    #[serde(default = "default_retry_backoff_ms")]
    pub retry_backoff_ms: u64,
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,
    #[serde(default)]
    pub retry_topic: Option<String>,
    #[serde(default)]
    pub dead_letter_topic: Option<String>,
    #[serde(default)]
    pub topic_regex: Option<String>,
    #[serde(default = "default_consumer_workers")]
    pub consumer_workers: u32,
}

impl Default for KafkaConfig {
    fn default() -> Self {
        Self {
            provider: None,
            brokers: Vec::new(),
            bootstrap: None,
            bootstrap_servers: None,
            client_id: None,
            group_id: None,
            group: None,
            topic_prefix: String::new(),
            acks: default_acks(),
            auto_offset_reset: default_auto_offset_reset(),
            enable_manual_ack: default_enable_manual_ack(),
            enable_auto_commit: default_enable_auto_commit(),
            session_timeout_ms: default_session_timeout_ms(),
            heartbeat_interval_ms: default_heartbeat_interval_ms(),
            max_poll_interval_ms: default_max_poll_interval_ms(),
            flush_timeout_ms: default_flush_timeout_ms(),
            message_timeout_ms: default_message_timeout_ms(),
            linger_ms: default_linger_ms(),
            batch_size: default_batch_size(),
            retry_backoff_ms: default_retry_backoff_ms(),
            max_retries: default_max_retries(),
            retry_topic: None,
            dead_letter_topic: None,
            topic_regex: None,
            consumer_workers: default_consumer_workers(),
        }
    }
}

fn default_acks() -> String {
    "all".into()
}

fn default_auto_offset_reset() -> String {
    "earliest".into()
}

fn default_enable_manual_ack() -> bool {
    false
}

fn default_enable_auto_commit() -> bool {
    false
}

fn default_session_timeout_ms() -> u64 {
    10_000
}

fn default_heartbeat_interval_ms() -> u64 {
    3_000
}

fn default_max_poll_interval_ms() -> u64 {
    300_000
}

fn default_flush_timeout_ms() -> u64 {
    5_000
}

fn default_message_timeout_ms() -> u64 {
    30_000
}

fn default_linger_ms() -> u64 {
    10
}

fn default_batch_size() -> usize {
    16_384
}

fn default_retry_backoff_ms() -> u64 {
    1000
}

fn default_max_retries() -> u32 {
    3
}

fn default_consumer_workers() -> u32 {
    1
}

fn split_csv(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(ToString::to_string)
        .collect()
}

impl KafkaConfig {
    pub fn resolved_provider(&self) -> KafkaProvider {
        self.provider.unwrap_or_else(default_compiled_provider)
    }

    pub fn normalized_brokers(&self) -> Vec<String> {
        if !self.brokers.is_empty() {
            return self.brokers.clone();
        }
        if let Some(values) = &self.bootstrap_servers {
            if !values.is_empty() {
                return values.clone();
            }
        }
        if let Some(value) = &self.bootstrap {
            return split_csv(value);
        }
        Vec::new()
    }

    pub fn brokers_csv(&self) -> String {
        self.normalized_brokers().join(",")
    }

    pub fn topic_name(&self, topic: impl AsRef<str>) -> String {
        if self.topic_prefix.is_empty() {
            topic.as_ref().to_string()
        } else {
            format!("{}.{}", self.topic_prefix, topic.as_ref())
        }
    }

    pub fn client_id_or_default(&self) -> String {
        self.client_id
            .clone()
            .unwrap_or_else(|| "roze-kafka".to_string())
    }

    pub fn group_id_or_default(&self) -> String {
        self.group
            .clone()
            .or_else(|| self.group_id.clone())
            .unwrap_or_else(|| "roze-kafka-group".to_string())
    }

    pub fn should_retry(&self, message_attempt: u32) -> bool {
        self.max_retries != 0 && message_attempt < self.max_retries
    }
}

fn default_compiled_provider() -> KafkaProvider {
    if cfg!(feature = "rdkafka") {
        KafkaProvider::Rdkafka
    } else if cfg!(feature = "rskafka") {
        KafkaProvider::RustNative
    } else {
        KafkaProvider::Memory
    }
}

const fn provider_feature(provider: KafkaProvider) -> (&'static str, bool) {
    match provider {
        KafkaProvider::Memory => ("memory", cfg!(feature = "memory")),
        KafkaProvider::Rdkafka => ("rdkafka", cfg!(feature = "rdkafka")),
        KafkaProvider::RustNative => ("rskafka", cfg!(feature = "rskafka")),
    }
}

#[cfg(feature = "rskafka")]
fn stable_partition_hash(key: &[u8]) -> u64 {
    key.iter().fold(0xcbf29ce484222325_u64, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
    })
}

#[cfg(any(feature = "rdkafka", test))]
#[derive(Debug, Clone, PartialEq, Eq)]
enum RecoverAction {
    Retry {
        topic: String,
        attempt: u32,
        backoff_ms: u64,
    },
    DeadLetter {
        topic: String,
    },
    Drop {
        reason: &'static str,
    },
}

#[cfg(any(feature = "rdkafka", test))]
fn recover_action(cfg: &KafkaConfig, message: &KafkaRecord) -> RecoverAction {
    if cfg.max_retries == 0 {
        return cfg
            .dead_letter_topic
            .clone()
            .map(|topic| RecoverAction::DeadLetter { topic })
            .unwrap_or(RecoverAction::Drop {
                reason: "recover_dropped",
            });
    }

    if cfg.should_retry(message.attempt) {
        return cfg
            .retry_topic
            .clone()
            .map(|topic| RecoverAction::Retry {
                topic,
                attempt: message.attempt.saturating_add(1),
                backoff_ms: cfg.retry_backoff_ms,
            })
            .unwrap_or(RecoverAction::Drop {
                reason: "retry_topic_missing",
            });
    }

    cfg.dead_letter_topic
        .clone()
        .map(|topic| RecoverAction::DeadLetter { topic })
        .unwrap_or(RecoverAction::Drop {
            reason: "dead_letter_missing",
        })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KafkaRecord {
    pub topic: String,
    #[serde(default)]
    pub key: Option<String>,
    #[serde(default)]
    pub headers: HashMap<String, String>,
    #[serde(default)]
    pub timestamp_millis: u64,
    #[serde(default)]
    pub partition: Option<i32>,
    #[serde(default)]
    pub offset: Option<i64>,
    #[serde(default)]
    pub group: Option<String>,
    pub payload: serde_json::Value,
    #[serde(default)]
    pub attempt: u32,
    #[serde(default)]
    pub dead_letter_topic: Option<String>,
}

impl KafkaRecord {
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
            timestamp_millis: current_millis(),
            partition: None,
            offset: None,
            group: None,
            payload,
            attempt: 0,
            dead_letter_topic: None,
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

    pub fn to_event(self) -> roze_eventbus::EventEnvelope {
        roze_eventbus::EventEnvelope::from_transport(
            self.topic,
            self.payload,
            self.key,
            self.headers,
            self.attempt,
        )
    }

    pub fn from_event(event: roze_eventbus::EventEnvelope) -> Self {
        let headers = event.transport_headers();
        Self {
            topic: event.topic,
            key: event.key,
            headers,
            timestamp_millis: current_millis(),
            partition: None,
            offset: None,
            group: None,
            payload: event.payload,
            attempt: event.attempt,
            dead_letter_topic: None,
        }
    }

    pub fn to_mq_message(&self) -> roze_mq::Message {
        let mut message = roze_mq::Message::from_event_envelope(self.clone().to_event());
        message.timestamp_millis = self.timestamp_millis;
        message.partition = self.partition;
        message.offset = self.offset;
        message.group = self.group.clone();
        message.dead_letter_topic = self.dead_letter_topic.clone();
        message
    }

    pub fn from_mq_message(message: roze_mq::Message) -> Self {
        let timestamp_millis = message.timestamp_millis;
        let partition = message.partition;
        let offset = message.offset;
        let group = message.group.clone();
        let dead_letter_topic = message.dead_letter_topic.clone();
        let mut record = Self::from_event(message.into_event_envelope());
        record.timestamp_millis = timestamp_millis;
        record.partition = partition;
        record.offset = offset;
        record.group = group;
        record.dead_letter_topic = dead_letter_topic;
        record
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PublishResult {
    pub topic: String,
    pub partition: Option<i32>,
    pub offset: Option<i64>,
    pub timestamp_millis: u64,
}

impl PublishResult {
    fn from_record(record: &KafkaRecord) -> Self {
        Self {
            topic: record.topic.clone(),
            partition: record.partition,
            offset: record.offset,
            timestamp_millis: record.timestamp_millis,
        }
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
    message: KafkaRecord,
    state: Arc<DeliveryState>,
    ack_fn: DeliveryAction,
    nack_fn: DeliveryAction,
}

impl Delivery {
    pub fn message(&self) -> &KafkaRecord {
        &self.message
    }

    pub fn into_message(self) -> KafkaRecord {
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

    fn new(message: KafkaRecord, ack_fn: DeliveryAction, nack_fn: DeliveryAction) -> Self {
        Self {
            message,
            state: Arc::new(DeliveryState::new()),
            ack_fn,
            nack_fn,
        }
    }
}

#[async_trait::async_trait]
pub trait Publisher: Send + Sync + 'static {
    async fn publish(&self, message: KafkaRecord) -> anyhow::Result<()>;

    async fn publish_with_result(&self, message: KafkaRecord) -> anyhow::Result<PublishResult> {
        let result = PublishResult::from_record(&message);
        self.publish(message).await?;
        Ok(result)
    }
}

#[async_trait::async_trait]
pub trait Subscriber: Send + Sync + 'static {
    async fn subscribe(&self, topic: &str) -> anyhow::Result<broadcast::Receiver<Delivery>>;
}

fn bridge_mq_deliveries(
    mut receiver: broadcast::Receiver<Delivery>,
) -> broadcast::Receiver<roze_mq::Delivery> {
    let (sender, output) = broadcast::channel(256);
    tokio::spawn(async move {
        loop {
            match receiver.recv().await {
                Ok(delivery) => {
                    let message = delivery.message().to_mq_message();
                    let ack_delivery = delivery.clone();
                    let nack_delivery = delivery;
                    let adapted = roze_mq::Delivery::from_handlers(
                        message,
                        move || {
                            let delivery = ack_delivery.clone();
                            async move { delivery.ack().await }
                        },
                        move || {
                            let delivery = nack_delivery.clone();
                            async move { delivery.nack().await }
                        },
                    );
                    if sender.send(adapted).is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    tracing::warn!(protocol = "kafka", skipped, "Kafka delivery adapter lagged");
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });
    output
}

#[derive(Debug, Clone)]
pub struct InMemoryKafkaBroker {
    topics: Arc<Mutex<HashMap<String, broadcast::Sender<Delivery>>>>,
    topic_offsets: Arc<Mutex<HashMap<String, i64>>>,
    dead_letters: Arc<Mutex<VecDeque<roze_mq::DeadLetterRecord>>>,
    dead_letter_topic: Option<String>,
    max_attempts: u32,
    next_dead_letter_id: Arc<AtomicU64>,
    published: Arc<AtomicU64>,
    delivered: Arc<AtomicU64>,
    acked: Arc<AtomicU64>,
    nacked: Arc<AtomicU64>,
    dead_lettered: Arc<AtomicU64>,
    replayed: Arc<AtomicU64>,
}

impl InMemoryKafkaBroker {
    pub fn new() -> Self {
        Self {
            topics: Arc::new(Mutex::new(HashMap::new())),
            topic_offsets: Arc::new(Mutex::new(HashMap::new())),
            dead_letters: Arc::new(Mutex::new(VecDeque::new())),
            dead_letter_topic: None,
            max_attempts: 3,
            next_dead_letter_id: Arc::new(AtomicU64::new(1)),
            published: Arc::new(AtomicU64::new(0)),
            delivered: Arc::new(AtomicU64::new(0)),
            acked: Arc::new(AtomicU64::new(0)),
            nacked: Arc::new(AtomicU64::new(0)),
            dead_lettered: Arc::new(AtomicU64::new(0)),
            replayed: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn with_dead_letter(dead_letter_topic: impl Into<String>, max_attempts: u32) -> Self {
        Self {
            dead_letter_topic: Some(dead_letter_topic.into()),
            max_attempts: max_attempts.max(1),
            ..Self::new()
        }
    }

    pub fn dead_letters(&self) -> Vec<KafkaRecord> {
        self.dead_letters
            .lock()
            .expect("broker lock poisoned")
            .iter()
            .map(|record| KafkaRecord::from_mq_message(record.message.clone()))
            .collect()
    }

    pub fn dead_letter_records(&self) -> Vec<roze_mq::DeadLetterRecord> {
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
                let (sender, _receiver) = broadcast::channel(256);
                sender
            })
            .clone()
    }

    fn push_dead_letter(&self, message: KafkaRecord, reason: impl Into<String>) {
        record_kafka_event(&message.topic, message.group.as_deref(), "dead_lettered");
        let record = roze_mq::DeadLetterRecord {
            id: self.next_dead_letter_id.fetch_add(1, Ordering::SeqCst),
            original_topic: message.topic.clone(),
            reason: reason.into(),
            failed_at_millis: current_millis(),
            replay_count: 0,
            message: message.to_mq_message(),
        };
        self.dead_letters
            .lock()
            .expect("broker lock poisoned")
            .push_back(record);
        self.dead_lettered.fetch_add(1, Ordering::SeqCst);
    }

    fn make_delivery(&self, message: KafkaRecord) -> Delivery {
        let broker = self.clone();
        let message_for_nack = message.clone();
        let ack_topic = message.topic.clone();
        let ack_group = message.group.clone();

        let ack_broker = self.clone();
        let ack_fn: DeliveryAction = Arc::new(move || {
            let broker = ack_broker.clone();
            let topic = ack_topic.clone();
            let group = ack_group.clone();
            Box::pin(async move {
                broker.acked.fetch_add(1, Ordering::SeqCst);
                record_kafka_event(&topic, group.as_deref(), "acked");
                Ok(())
            })
        });
        let nack_fn: DeliveryAction = Arc::new(move || {
            let broker = broker.clone();
            let message = message_for_nack.clone();
            Box::pin(async move {
                broker.nacked.fetch_add(1, Ordering::SeqCst);
                record_kafka_event(&message.topic, message.group.as_deref(), "nacked");
                broker.requeue_or_dead_letter(message).await;
                Ok(())
            })
        });

        Delivery::new(message, ack_fn, nack_fn)
    }

    fn route_or_dead_letter(&self, mut message: KafkaRecord) -> PublishResult {
        self.prepare_delivery_metadata(&mut message);
        let result = PublishResult::from_record(&message);
        message.attempt = message.attempt.saturating_add(1);
        if message.attempt > self.max_attempts {
            self.push_dead_letter(message.clone(), "max_attempts_exceeded");
            if let Some(dead_letter_topic) = message
                .dead_letter_topic
                .clone()
                .or_else(|| self.dead_letter_topic.clone())
            {
                let mut dead = message;
                dead.topic = dead_letter_topic;
                dead.attempt = 0;
                let _ = self.sender_for(&dead.topic).send(self.make_delivery(dead));
            }
            return result;
        }

        let topic = message.topic.clone();
        let group = message.group.clone();
        let _ = self.sender_for(&topic).send(self.make_delivery(message));
        self.delivered.fetch_add(1, Ordering::SeqCst);
        record_kafka_event(&topic, group.as_deref(), "delivered");
        record_kafka_offset(&result);
        result
    }

    fn prepare_delivery_metadata(&self, message: &mut KafkaRecord) {
        if message.timestamp_millis == 0 {
            message.timestamp_millis = current_millis();
        }
        if message.partition.is_none() {
            message.partition = Some(0);
        }
        if message.offset.is_none() {
            let mut offsets = self.topic_offsets.lock().expect("broker lock poisoned");
            let offset = offsets.entry(message.topic.clone()).or_insert(0);
            message.offset = Some(*offset);
            *offset = offset.saturating_add(1);
        }
    }

    async fn route(&self, message: KafkaRecord) -> PublishResult {
        self.route_or_dead_letter(message)
    }

    async fn requeue_or_dead_letter(&self, message: KafkaRecord) {
        self.route_or_dead_letter(message);
    }
}

impl Default for InMemoryKafkaBroker {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Publisher for InMemoryKafkaBroker {
    async fn publish(&self, message: KafkaRecord) -> anyhow::Result<()> {
        self.publish_with_result(message).await.map(|_| ())
    }

    async fn publish_with_result(&self, mut message: KafkaRecord) -> anyhow::Result<PublishResult> {
        message.ensure_trace_id();
        record_kafka_event(&message.topic, message.group.as_deref(), "published");
        self.published.fetch_add(1, Ordering::SeqCst);
        Ok(self.route(message).await)
    }
}

#[async_trait::async_trait]
impl Subscriber for InMemoryKafkaBroker {
    async fn subscribe(&self, topic: &str) -> anyhow::Result<broadcast::Receiver<Delivery>> {
        Ok(self.sender_for(topic).subscribe())
    }
}

#[async_trait::async_trait]
impl roze_mq::Publisher for InMemoryKafkaBroker {
    async fn publish(&self, message: roze_mq::Message) -> anyhow::Result<()> {
        <Self as Publisher>::publish(self, KafkaRecord::from_mq_message(message)).await
    }
}

#[async_trait::async_trait]
impl roze_mq::Subscriber for InMemoryKafkaBroker {
    async fn subscribe(
        &self,
        topic: &str,
    ) -> anyhow::Result<broadcast::Receiver<roze_mq::Delivery>> {
        let receiver = <Self as Subscriber>::subscribe(self, topic).await?;
        Ok(bridge_mq_deliveries(receiver))
    }
}

#[async_trait::async_trait]
impl roze_mq::MqAdmin for InMemoryKafkaBroker {
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
                .expect("broker lock poisoned")
                .len() as u64,
        })
    }

    async fn dead_letters(
        &self,
        offset: usize,
        limit: usize,
    ) -> anyhow::Result<Vec<roze_mq::DeadLetterRecord>> {
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

    async fn replay_dead_letter(&self, id: u64) -> anyhow::Result<Option<roze_mq::Message>> {
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
        record_kafka_event(&message.topic, message.group.as_deref(), "replayed");
        self.route(KafkaRecord::from_mq_message(message.clone()))
            .await;
        Ok(Some(message))
    }

    async fn purge_dead_letter(
        &self,
        id: u64,
    ) -> anyhow::Result<Option<roze_mq::DeadLetterRecord>> {
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

#[cfg(feature = "rdkafka")]
#[derive(Clone)]
pub struct RdkafkaProducer {
    producer: FutureProducer,
    config: KafkaConfig,
}

#[cfg(feature = "rdkafka")]
impl RdkafkaProducer {
    pub fn new(config: impl Into<KafkaConfig>) -> anyhow::Result<Self> {
        let config = config.into();
        if config.normalized_brokers().is_empty() {
            return Err(anyhow::anyhow!("kafka broker list is empty"));
        }

        let producer: FutureProducer = ClientConfig::new()
            .set("bootstrap.servers", config.brokers_csv())
            .set("client.id", config.client_id_or_default())
            .set("acks", config.acks.as_str())
            .set("batch.size", config.batch_size.to_string())
            .set("linger.ms", config.linger_ms.to_string())
            .set("message.send.max.retries", config.max_retries.to_string())
            .set("retry.backoff.ms", config.retry_backoff_ms.to_string())
            .set("message.timeout.ms", config.message_timeout_ms.to_string())
            .create()?;

        Ok(Self { producer, config })
    }

    pub fn flush(&self) -> anyhow::Result<()> {
        self.producer
            .flush(Timeout::After(Duration::from_millis(
                self.config.flush_timeout_ms,
            )))
            .map_err(|err| anyhow::anyhow!(err.to_string()))
    }

    pub fn close(&self) -> anyhow::Result<()> {
        self.flush()
    }

    pub fn config(&self) -> &KafkaConfig {
        &self.config
    }
}

#[cfg(feature = "rdkafka")]
#[async_trait::async_trait]
impl Publisher for RdkafkaProducer {
    async fn publish(&self, message: KafkaRecord) -> anyhow::Result<()> {
        self.publish_with_result(message).await.map(|_| ())
    }

    async fn publish_with_result(&self, mut message: KafkaRecord) -> anyhow::Result<PublishResult> {
        message.ensure_trace_id();
        let metric_topic = self.config.topic_name(&message.topic);
        let topic = metric_topic.clone();
        let timestamp_millis = if message.timestamp_millis == 0 {
            current_millis()
        } else {
            message.timestamp_millis
        };
        let payload = serde_json::to_vec(&message.payload)?;
        let mut headers = OwnedHeaders::new();
        for (key, value) in &message.headers {
            headers = headers.insert(Header {
                key: key.as_str(),
                value: Some(value.as_bytes()),
            });
        }
        let attempt = message.attempt.to_string();
        headers = headers.insert(Header {
            key: "roze-attempt",
            value: Some(attempt.as_bytes()),
        });

        let mut record = FutureRecord::to(&topic).payload(&payload).headers(headers);
        if let Some(key) = message.key.as_ref() {
            record = record.key(key);
        }

        match self
            .producer
            .send(
                record,
                Timeout::After(Duration::from_millis(self.config.flush_timeout_ms)),
            )
            .await
        {
            Ok((_partition, _offset)) => {
                record_kafka_event(&metric_topic, message.group.as_deref(), "published");
                let result = PublishResult {
                    topic: metric_topic,
                    partition: Some(_partition),
                    offset: Some(_offset),
                    timestamp_millis,
                };
                record_kafka_offset(&result);
                Ok(result)
            }
            Err((error, _)) => {
                record_kafka_event(&metric_topic, message.group.as_deref(), "publish_failed");
                Err(anyhow::anyhow!(error.to_string()))
            }
        }
    }
}

#[cfg(feature = "rdkafka")]
#[async_trait::async_trait]
impl roze_mq::Publisher for RdkafkaProducer {
    async fn publish(&self, message: roze_mq::Message) -> anyhow::Result<()> {
        <Self as Publisher>::publish(self, KafkaRecord::from_mq_message(message)).await
    }
}

#[cfg(feature = "rdkafka")]
#[derive(Debug)]
enum RdkafkaAckCmd {
    Commit {
        metadata: TopicOffsetMetadata,
        result: oneshot::Sender<Result<(), String>>,
    },
}

#[cfg(feature = "rdkafka")]
#[derive(Debug, Clone)]
struct TopicOffsetMetadata {
    topic: String,
    partition: i32,
    next_offset: i64,
}

#[cfg(feature = "rdkafka")]
fn parse_attempt<H>(headers: Option<&H>) -> u32
where
    H: Headers,
{
    if let Some(headers) = headers {
        for index in 0..headers.count() {
            let item = headers.get(index);
            if item.key == "roze-attempt" {
                if let Some(value) = item.value {
                    if let Ok(raw) = std::str::from_utf8(value) {
                        if let Ok(value) = raw.parse::<u32>() {
                            return value;
                        }
                    }
                }
            }
        }
    }
    0
}

#[cfg(feature = "rdkafka")]
fn collect_headers<H>(headers: Option<&H>) -> HashMap<String, String>
where
    H: Headers,
{
    let mut output = HashMap::new();
    if let Some(headers) = headers {
        for index in 0..headers.count() {
            let item = headers.get(index);
            output.insert(
                item.key.to_string(),
                String::from_utf8_lossy(item.value.unwrap_or(&[])).into_owned(),
            );
        }
    }
    output
}

#[cfg(feature = "rdkafka")]
async fn publish_recover(cfg: KafkaConfig, mut message: KafkaRecord) -> anyhow::Result<()> {
    match recover_action(&cfg, &message) {
        RecoverAction::Retry {
            topic,
            attempt,
            backoff_ms,
        } => {
            message.topic = topic;
            message.attempt = attempt;
            let metric_topic = cfg.topic_name(&message.topic);
            record_kafka_event(&metric_topic, message.group.as_deref(), "retry_scheduled");
            tracing::warn!(
                event = "kafka.message.requeue_retry",
                topic = %metric_topic,
                attempt = %message.attempt,
                retry_backoff_ms = backoff_ms,
                max_retries = cfg.max_retries,
                "kafka message retry topic configured, requeueing"
            );
            tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
            RdkafkaProducer::new(cfg)?.publish(message).await?;
            Ok(())
        }
        RecoverAction::DeadLetter { topic } => {
            message.topic = topic;
            message.attempt = 0;
            let log_topic = cfg.topic_name(&message.topic);
            let log_attempt = message.attempt;
            let max_retries = cfg.max_retries;
            record_kafka_event(&log_topic, message.group.as_deref(), "dead_lettered");
            RdkafkaProducer::new(cfg)?.publish(message).await?;
            tracing::warn!(
                event = "kafka.message.dead_lettered",
                topic = %log_topic,
                attempt = %log_attempt,
                max_retries = max_retries,
                "kafka message moved to dead letter"
            );
            Ok(())
        }
        RecoverAction::Drop { reason } => {
            record_kafka_event(&message.topic, message.group.as_deref(), reason);
            tracing::warn!(
                event = %format!("kafka.message.{reason}"),
                topic = %message.topic,
                attempt = %message.attempt,
                max_retries = cfg.max_retries,
                "kafka message recovery path dropped message"
            );
            Ok(())
        }
    }
}

#[cfg(feature = "rdkafka")]
#[derive(Debug, Clone)]
pub struct RdkafkaSubscriber {
    config: KafkaConfig,
}

#[cfg(feature = "rdkafka")]
impl RdkafkaSubscriber {
    pub fn new(config: impl Into<KafkaConfig>) -> Self {
        Self {
            config: config.into(),
        }
    }

    fn build_consumer(&self) -> anyhow::Result<StreamConsumer> {
        if self.config.normalized_brokers().is_empty() {
            return Err(anyhow::anyhow!("kafka broker list is empty"));
        }

        let consumer = ClientConfig::new()
            .set("bootstrap.servers", self.config.brokers_csv())
            .set("client.id", self.config.client_id_or_default())
            .set("group.id", self.config.group_id_or_default())
            .set("auto.offset.reset", self.config.auto_offset_reset.as_str())
            .set(
                "session.timeout.ms",
                self.config.session_timeout_ms.to_string(),
            )
            .set(
                "heartbeat.interval.ms",
                self.config.heartbeat_interval_ms.to_string(),
            )
            .set(
                "max.poll.interval.ms",
                self.config.max_poll_interval_ms.to_string(),
            )
            .set(
                "enable.auto.commit",
                if self.config.enable_auto_commit && !self.config.enable_manual_ack {
                    "true"
                } else {
                    "false"
                },
            )
            .create()?;

        Ok(consumer)
    }

    fn topics_for_subscription(
        &self,
        consumer: &StreamConsumer,
        requested_topic: &str,
    ) -> anyhow::Result<Vec<String>> {
        if let Some(regex_str) = &self.config.topic_regex {
            let regex = Regex::new(regex_str)
                .map_err(|err| anyhow::anyhow!("invalid topic_regex: {err}"))?;
            let metadata = consumer.fetch_metadata(None, Timeout::After(Duration::from_secs(3)))?;
            let mut topics = Vec::new();
            for topic in metadata.topics() {
                if regex.is_match(topic.name()) {
                    topics.push(topic.name().to_string());
                }
            }
            return if topics.is_empty() {
                Err(anyhow::anyhow!("no topic matches configured topic_regex"))
            } else {
                Ok(topics)
            };
        }

        Ok(vec![requested_topic.to_string()])
    }

    fn commit_with_consumer(
        consumer: &StreamConsumer,
        meta: &TopicOffsetMetadata,
    ) -> anyhow::Result<()> {
        let mut offsets = TopicPartitionList::new();
        offsets.add_partition_offset(
            &meta.topic,
            meta.partition,
            Offset::Offset(meta.next_offset),
        )?;
        consumer.commit(&offsets, CommitMode::Sync)?;
        Ok(())
    }
}

#[cfg(feature = "rdkafka")]
#[async_trait::async_trait]
impl Subscriber for RdkafkaSubscriber {
    async fn subscribe(&self, topic: &str) -> anyhow::Result<broadcast::Receiver<Delivery>> {
        let requested_topic = self.config.topic_name(topic);
        let consumer = self.build_consumer()?;
        let topics = self.topics_for_subscription(&consumer, &requested_topic)?;

        let topic_refs: Vec<&str> = topics.iter().map(String::as_str).collect();
        consumer.subscribe(&topic_refs)?;

        let (sender, _receiver) = broadcast::channel(256);
        let (ack_tx, mut ack_rx) = mpsc::unbounded_channel::<RdkafkaAckCmd>();
        let cfg = self.config.clone();
        let sender_for_task = sender.clone();

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    maybe_msg = consumer.recv() => {
                        match maybe_msg {
                            Ok(msg) => {
                                let payload_bytes = msg.payload().unwrap_or(&[]).to_vec();
                                let payload = serde_json::from_slice::<serde_json::Value>(&payload_bytes)
                                    .unwrap_or_else(|_| serde_json::json!(String::from_utf8_lossy(&payload_bytes)));
                                let message = KafkaRecord {
                                    topic: msg.topic().to_string(),
                                    key: msg.key().map(|value| String::from_utf8_lossy(value).into_owned()),
                                    headers: collect_headers(msg.headers()),
                                    timestamp_millis: current_millis(),
                                    partition: Some(msg.partition()),
                                    offset: Some(msg.offset()),
                                    group: Some(cfg.group_id_or_default()),
                                    payload: payload.clone(),
                                    attempt: parse_attempt(msg.headers()),
                                    dead_letter_topic: cfg.dead_letter_topic.clone(),
                                };
                                if let (Some(partition), Some(offset)) = (message.partition, message.offset) {
                                    roze_metrics::record_queue_offset(
                                        "kafka",
                                        &message.topic,
                                        message.group.as_deref().unwrap_or_default(),
                                        partition,
                                        offset,
                                    );
                                }
                                let commit_meta = TopicOffsetMetadata {
                                    topic: msg.topic().to_string(),
                                    partition: msg.partition(),
                                    next_offset: msg.offset() + 1,
                                };
                                let cfg_for_nack = cfg.clone();
                                let message_for_nack = message.clone();
                                let ack_tx = ack_tx.clone();
                                let delivery = Delivery::new(
                                    message,
                                    Arc::new(move || {
                                        let ack_tx = ack_tx.clone();
                                        let meta = commit_meta.clone();
                                        Box::pin(async move {
                                            let (result_tx, result_rx) = oneshot::channel();
                                            ack_tx
                                                .send(RdkafkaAckCmd::Commit {
                                                    metadata: meta,
                                                    result: result_tx,
                                                })
                                                .map_err(|_| anyhow::anyhow!("ack channel closed"))?;
                                            result_rx
                                                .await
                                                .map_err(|_| anyhow::anyhow!("ack result channel closed"))?
                                                .map_err(anyhow::Error::msg)
                                        })
                                    }),
                                    Arc::new(move || {
                                        let cfg = cfg_for_nack.clone();
                                        let message = message_for_nack.clone();
                                        Box::pin(async move {
                                            record_kafka_event(
                                                &message.topic,
                                                message.group.as_deref(),
                                                "nacked",
                                            );
                                            publish_recover(cfg, message).await
                                        })
                                    }),
                                );
                                let _ = sender_for_task.send(delivery);
                            }
                            Err(err) => {
                                tracing::warn!(error=%err, "kafka receive failed");
                            }
                        }
                    }
                    maybe_ack = ack_rx.recv() => {
                        if let Some(RdkafkaAckCmd::Commit { metadata, result }) = maybe_ack {
                            if let Err(err) = RdkafkaSubscriber::commit_with_consumer(&consumer, &metadata) {
                                let message = err.to_string();
                                let _ = result.send(Err(message.clone()));
                                record_kafka_event(&metadata.topic, Some(&cfg.group_id_or_default()), "commit_failed");
                                tracing::warn!(topic=%metadata.topic, error=%message, "kafka commit failed");
                            } else {
                                let _ = result.send(Ok(()));
                                record_kafka_event(&metadata.topic, Some(&cfg.group_id_or_default()), "acked");
                            }
                        }
                    }
                }
            }
        });

        Ok(sender.subscribe())
    }
}

#[cfg(feature = "rdkafka")]
#[async_trait::async_trait]
impl roze_mq::Subscriber for RdkafkaSubscriber {
    async fn subscribe(
        &self,
        topic: &str,
    ) -> anyhow::Result<broadcast::Receiver<roze_mq::Delivery>> {
        let receiver = <Self as Subscriber>::subscribe(self, topic).await?;
        Ok(bridge_mq_deliveries(receiver))
    }
}

#[cfg(feature = "rskafka")]
/// Experimental pure-Rust Kafka publisher backed by rskafka.
///
/// rskafka does not provide consumer groups or offset commits, so this type is
/// intentionally publisher-only.
#[derive(Clone)]
pub struct RustNativeProducer {
    client: Arc<Client>,
    config: KafkaConfig,
}

#[cfg(feature = "rskafka")]
impl RustNativeProducer {
    pub async fn connect(config: impl Into<KafkaConfig>) -> anyhow::Result<Self> {
        let config = config.into();
        let brokers = config.normalized_brokers();
        if brokers.is_empty() {
            return Err(anyhow::anyhow!("kafka broker list is empty"));
        }
        let client = ClientBuilder::new(brokers)
            .client_id(config.client_id_or_default())
            .build()
            .await?;
        Ok(Self {
            client: Arc::new(client),
            config,
        })
    }

    pub fn config(&self) -> &KafkaConfig {
        &self.config
    }

    async fn resolve_partition(&self, topic: &str, message: &KafkaRecord) -> anyhow::Result<i32> {
        let topics = self.client.list_topics().await?;
        let metadata = topics
            .into_iter()
            .find(|metadata| metadata.name == topic)
            .ok_or_else(|| anyhow::anyhow!("Kafka topic `{topic}` was not found"))?;
        if metadata.partitions.is_empty() {
            return Err(anyhow::anyhow!("Kafka topic `{topic}` has no partitions"));
        }
        if let Some(partition) = message.partition {
            if metadata.partitions.contains(&partition) {
                return Ok(partition);
            }
            return Err(anyhow::anyhow!(
                "Kafka topic `{topic}` does not contain partition {partition}"
            ));
        }
        if let Some(key) = message.key.as_deref() {
            let index = stable_partition_hash(key.as_bytes()) as usize % metadata.partitions.len();
            return metadata
                .partitions
                .iter()
                .nth(index)
                .copied()
                .ok_or_else(|| anyhow::anyhow!("Kafka topic `{topic}` has no partitions"));
        }
        metadata
            .partitions
            .iter()
            .next()
            .copied()
            .ok_or_else(|| anyhow::anyhow!("Kafka topic `{topic}` has no partitions"))
    }
}

#[cfg(feature = "rskafka")]
#[async_trait::async_trait]
impl Publisher for RustNativeProducer {
    async fn publish(&self, message: KafkaRecord) -> anyhow::Result<()> {
        self.publish_with_result(message).await.map(|_| ())
    }

    async fn publish_with_result(&self, mut message: KafkaRecord) -> anyhow::Result<PublishResult> {
        message.ensure_trace_id();
        let topic = self.config.topic_name(&message.topic);
        let partition = self.resolve_partition(&topic, &message).await?;
        let timestamp_millis = if message.timestamp_millis == 0 {
            current_millis()
        } else {
            message.timestamp_millis
        };
        let timestamp = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(
            timestamp_millis.min(i64::MAX as u64) as i64,
        )
        .unwrap_or_else(chrono::Utc::now);
        let mut headers = message
            .headers
            .into_iter()
            .map(|(key, value)| (key, value.into_bytes()))
            .collect::<BTreeMap<_, _>>();
        headers.insert(
            "roze-attempt".to_string(),
            message.attempt.to_string().into_bytes(),
        );
        let record = RustNativeRecord {
            key: message.key.map(String::into_bytes),
            value: Some(serde_json::to_vec(&message.payload)?),
            headers,
            timestamp,
        };
        let partition_client = self
            .client
            .partition_client(topic.clone(), partition, UnknownTopicHandling::Retry)
            .await?;
        match partition_client
            .produce(vec![record], Compression::NoCompression)
            .await
        {
            Ok(offsets) => {
                let offset = offsets.into_iter().next();
                let result = PublishResult {
                    topic,
                    partition: Some(partition),
                    offset,
                    timestamp_millis,
                };
                record_kafka_event(&result.topic, message.group.as_deref(), "published");
                record_kafka_offset(&result);
                Ok(result)
            }
            Err(error) => {
                record_kafka_event(&topic, message.group.as_deref(), "publish_failed");
                Err(anyhow::Error::new(error))
            }
        }
    }
}

#[cfg(feature = "rskafka")]
#[async_trait::async_trait]
impl roze_mq::Publisher for RustNativeProducer {
    async fn publish(&self, message: roze_mq::Message) -> anyhow::Result<()> {
        <Self as Publisher>::publish(self, KafkaRecord::from_mq_message(message)).await
    }
}

/// A complete Kafka publisher/subscriber pair using stable `roze_mq` traits.
pub struct KafkaRuntime {
    pub provider: KafkaProvider,
    pub capabilities: KafkaCapabilities,
    pub publisher: Arc<dyn roze_mq::Publisher>,
    pub subscriber: Arc<dyn roze_mq::Subscriber>,
}

/// Builds a provider runtime that satisfies publish, subscribe, and settlement semantics.
///
/// Publish-only providers are rejected with [`KafkaRuntimeError::UnsupportedCapability`].
pub async fn build_runtime(config: &KafkaConfig) -> Result<KafkaRuntime, KafkaRuntimeError> {
    let provider = config.resolved_provider();
    let (feature, enabled) = provider_feature(provider);
    if !enabled {
        return Err(KafkaRuntimeError::FeatureDisabled { provider, feature });
    }
    let capabilities = provider.capabilities();
    if !capabilities.subscribe {
        return Err(KafkaRuntimeError::UnsupportedCapability {
            provider,
            capability: "consumer-groups-and-offset-commit",
        });
    }

    match provider {
        KafkaProvider::Memory => {
            #[cfg(feature = "memory")]
            {
                let broker = Arc::new(InMemoryKafkaBroker::new());
                Ok(KafkaRuntime {
                    provider,
                    capabilities,
                    publisher: broker.clone(),
                    subscriber: broker,
                })
            }
            #[cfg(not(feature = "memory"))]
            {
                Err(KafkaRuntimeError::FeatureDisabled {
                    provider,
                    feature: "memory",
                })
            }
        }
        KafkaProvider::Rdkafka => {
            #[cfg(feature = "rdkafka")]
            {
                let publisher = RdkafkaProducer::new(config.clone()).map_err(|error| {
                    KafkaRuntimeError::Initialization {
                        provider,
                        message: error.to_string(),
                    }
                })?;
                let subscriber = RdkafkaSubscriber::new(config.clone());
                Ok(KafkaRuntime {
                    provider,
                    capabilities,
                    publisher: Arc::new(publisher),
                    subscriber: Arc::new(subscriber),
                })
            }
            #[cfg(not(feature = "rdkafka"))]
            {
                Err(KafkaRuntimeError::FeatureDisabled {
                    provider,
                    feature: "rdkafka",
                })
            }
        }
        KafkaProvider::RustNative => Err(KafkaRuntimeError::UnsupportedCapability {
            provider,
            capability: "consumer-groups-and-offset-commit",
        }),
    }
}

/// Builds only the configured publisher.
///
/// This is the supported entrypoint for the experimental `rust-native` provider.
pub async fn build_publisher(
    config: &KafkaConfig,
) -> Result<Arc<dyn roze_mq::Publisher>, KafkaRuntimeError> {
    let provider = config.resolved_provider();
    match provider {
        KafkaProvider::Memory => {
            #[cfg(feature = "memory")]
            {
                Ok(Arc::new(InMemoryKafkaBroker::new()))
            }
            #[cfg(not(feature = "memory"))]
            {
                Err(KafkaRuntimeError::FeatureDisabled {
                    provider,
                    feature: "memory",
                })
            }
        }
        KafkaProvider::Rdkafka => {
            #[cfg(feature = "rdkafka")]
            {
                RdkafkaProducer::new(config.clone())
                    .map(|producer| Arc::new(producer) as Arc<dyn roze_mq::Publisher>)
                    .map_err(|error| KafkaRuntimeError::Initialization {
                        provider,
                        message: error.to_string(),
                    })
            }
            #[cfg(not(feature = "rdkafka"))]
            {
                Err(KafkaRuntimeError::FeatureDisabled {
                    provider,
                    feature: "rdkafka",
                })
            }
        }
        KafkaProvider::RustNative => {
            #[cfg(feature = "rskafka")]
            {
                RustNativeProducer::connect(config.clone())
                    .await
                    .map(|producer| Arc::new(producer) as Arc<dyn roze_mq::Publisher>)
                    .map_err(|error| KafkaRuntimeError::Initialization {
                        provider,
                        message: error.to_string(),
                    })
            }
            #[cfg(not(feature = "rskafka"))]
            {
                Err(KafkaRuntimeError::FeatureDisabled {
                    provider,
                    feature: "rskafka",
                })
            }
        }
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
    publisher.publish(KafkaRecord::new(topic, payload)).await
}

pub async fn publish_json_with_result<P>(
    publisher: &P,
    topic: impl Into<String>,
    payload: serde_json::Value,
) -> anyhow::Result<PublishResult>
where
    P: Publisher,
{
    publisher
        .publish_with_result(KafkaRecord::new(topic, payload))
        .await
}

fn current_millis() -> u64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(elapsed) => elapsed.as_millis() as u64,
        Err(_) => 0,
    }
}

fn record_kafka_event(topic: &str, group: Option<&str>, outcome: &str) {
    roze_metrics::record_queue_event("kafka", topic, group.unwrap_or_default(), outcome);
}

fn record_kafka_offset(result: &PublishResult) {
    if let (Some(partition), Some(offset)) = (result.partition, result.offset) {
        roze_metrics::record_queue_offset("kafka", &result.topic, "", partition, offset);
    }
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
    spawn_consumer_with_auto_ack(subscriber, topic, handler, true).await
}

pub async fn spawn_consumer_with_auto_ack<S, F, Fut>(
    subscriber: &S,
    topic: impl Into<String>,
    handler: F,
    auto_ack: bool,
) -> anyhow::Result<JoinHandle<()>>
where
    S: Subscriber,
    F: Fn(Delivery) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = anyhow::Result<()>> + Send + 'static,
{
    let topic = topic.into();
    let mut receiver = subscriber.subscribe(&topic).await?;
    let handler = std::sync::Arc::new(handler);
    let handle = tokio::spawn(async move {
        loop {
            match receiver.recv().await {
                Ok(delivery) => {
                    let result = handler(delivery.clone()).await;
                    match (result, auto_ack) {
                        (Ok(_), true) => {
                            if !delivery.is_acked() && !delivery.is_nacked() {
                                let _ = delivery.ack().await;
                            }
                        }
                        (Ok(_), false) => {}
                        (Err(err), _) => {
                            tracing::warn!(topic = %topic, error = %err, "kafka handler failed, nacking message");
                            let _ = delivery.nack().await;
                        }
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                    tracing::warn!(topic = %topic, skipped = skipped, "kafka consumer lagged");
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });
    Ok(handle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use roze_mq::MqAdmin;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    #[test]
    fn formats_brokers_and_topics() {
        let cfg = KafkaConfig {
            brokers: vec!["k1:9092".into(), "k2:9092".into()],
            client_id: Some("roze".into()),
            group_id: Some("group".into()),
            topic_prefix: "app".into(),
            acks: "all".into(),
            auto_offset_reset: "earliest".into(),
            enable_auto_commit: false,
            session_timeout_ms: 10000,
            consumer_workers: 2,
            ..Default::default()
        };
        assert_eq!(cfg.brokers_csv(), "k1:9092,k2:9092");
        assert_eq!(cfg.topic_name("orders"), "app.orders");
    }

    #[test]
    fn message_timeout_is_bounded_by_default() {
        assert_eq!(KafkaConfig::default().message_timeout_ms, 30_000);
    }

    #[test]
    fn provider_configuration_and_capabilities_are_explicit() {
        let native: KafkaConfig =
            serde_json::from_value(serde_json::json!({"provider": "rust-native"}))
                .expect("deserialize rust-native provider");
        assert_eq!(native.resolved_provider(), KafkaProvider::RustNative);
        assert_eq!(
            native.resolved_provider().capabilities(),
            KafkaCapabilities {
                publish: true,
                subscribe: false,
                consumer_groups: false,
                manual_ack: false,
                offset_commit: false,
                rebalance: false,
                transactions: false,
            }
        );

        let alias: KafkaConfig = serde_json::from_value(serde_json::json!({"provider": "rskafka"}))
            .expect("deserialize rskafka alias");
        assert_eq!(alias.resolved_provider(), KafkaProvider::RustNative);
        assert!(KafkaProvider::Rdkafka.capabilities().consumer_groups);
        assert!(KafkaProvider::Rdkafka.capabilities().offset_commit);
        assert!(!KafkaProvider::Rdkafka.capabilities().transactions);
    }

    #[test]
    #[cfg(feature = "rskafka")]
    fn rust_native_partition_hash_is_stable() {
        assert_eq!(
            stable_partition_hash(b"order-42"),
            9_015_620_992_513_762_004
        );
        assert_eq!(
            stable_partition_hash(b"order-42"),
            stable_partition_hash(b"order-42")
        );
    }

    #[tokio::test]
    #[cfg(feature = "memory")]
    async fn memory_runtime_implements_stable_mq_contract() {
        let runtime = build_runtime(&KafkaConfig {
            provider: Some(KafkaProvider::Memory),
            ..Default::default()
        })
        .await
        .expect("build memory runtime");
        let mut receiver = runtime
            .subscriber
            .subscribe("orders")
            .await
            .expect("subscribe through roze-mq");
        runtime
            .publisher
            .publish(roze_mq::Message::new(
                "orders",
                serde_json::json!({"id": 42}),
            ))
            .await
            .expect("publish through roze-mq");

        let delivery = receiver.recv().await.expect("receive adapted delivery");
        assert_eq!(delivery.message().payload["id"], 42);
        delivery.ack().await.expect("ack adapted delivery");
        assert!(delivery.is_acked());
    }

    #[tokio::test]
    #[cfg(feature = "rskafka")]
    async fn rust_native_stream_runtime_fails_before_connecting() {
        let result = build_runtime(&KafkaConfig {
            provider: Some(KafkaProvider::RustNative),
            brokers: vec!["127.0.0.1:9092".to_string()],
            ..Default::default()
        })
        .await;
        let Err(error) = result else {
            panic!("rskafka cannot satisfy stream consumer semantics");
        };
        assert_eq!(
            error,
            KafkaRuntimeError::UnsupportedCapability {
                provider: KafkaProvider::RustNative,
                capability: "consumer-groups-and-offset-commit",
            }
        );
    }

    #[tokio::test]
    #[ignore = "integration: requires ROZE_KAFKA_BROKERS and an existing ROZE_KAFKA_TOPIC"]
    #[cfg(feature = "rskafka")]
    async fn rust_native_publish_round_trips_against_real_broker() {
        let brokers = std::env::var("ROZE_KAFKA_BROKERS")
            .expect("ROZE_KAFKA_BROKERS is required")
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        let topic = std::env::var("ROZE_KAFKA_TOPIC").expect("ROZE_KAFKA_TOPIC is required");
        let producer = RustNativeProducer::connect(KafkaConfig {
            provider: Some(KafkaProvider::RustNative),
            brokers,
            ..Default::default()
        })
        .await
        .expect("connect rskafka producer");
        let payload = serde_json::json!({
            "provider": "rust-native",
            "test": uuid::Uuid::now_v7().to_string()
        });
        let result = producer
            .publish_with_result(KafkaRecord::new(topic.clone(), payload.clone()))
            .await
            .expect("publish through rskafka");
        let offset = result.offset.expect("published offset");
        let partition = result.partition.expect("published partition");
        let partition_client = producer
            .client
            .partition_client(topic, partition, UnknownTopicHandling::Error)
            .await
            .expect("open published partition");
        let (records, _) = partition_client
            .fetch_records(offset, 1..1_000_000, 1_000)
            .await
            .expect("fetch published record");
        let record = records
            .into_iter()
            .find(|record| record.offset == offset)
            .expect("find published offset");
        let restored: serde_json::Value =
            serde_json::from_slice(record.record.value.as_deref().expect("record value"))
                .expect("decode published JSON");
        assert_eq!(restored, payload);
    }

    #[tokio::test]
    #[cfg(not(feature = "rskafka"))]
    async fn selecting_uncompiled_provider_reports_required_feature() {
        let result = build_runtime(&KafkaConfig {
            provider: Some(KafkaProvider::RustNative),
            ..Default::default()
        })
        .await;
        let Err(error) = result else {
            panic!("uncompiled provider must fail");
        };
        assert_eq!(
            error,
            KafkaRuntimeError::FeatureDisabled {
                provider: KafkaProvider::RustNative,
                feature: "rskafka",
            }
        );
    }

    #[tokio::test]
    async fn in_memory_kafka_round_trips() {
        let broker = InMemoryKafkaBroker::new();
        let mut receiver = broker.subscribe("orders").await.expect("subscribe");

        broker
            .publish(KafkaRecord::new("orders", serde_json::json!({"id": 1})))
            .await
            .expect("publish");

        let delivery = receiver.recv().await.expect("delivery");
        assert_eq!(delivery.message().topic, "orders");
        assert_eq!(delivery.message().payload["id"], 1);
        assert!(delivery.message().timestamp_millis > 0);
        assert_eq!(delivery.message().partition, Some(0));
        assert_eq!(delivery.message().offset, Some(0));
        let trace_id = delivery
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
    async fn publish_with_result_returns_broker_metadata() {
        let broker = InMemoryKafkaBroker::new();
        let mut receiver = broker.subscribe("orders").await.expect("subscribe");

        let result = broker
            .publish_with_result(KafkaRecord::new("orders", serde_json::json!({"id": 7})))
            .await
            .expect("publish");

        assert_eq!(result.topic, "orders");
        assert_eq!(result.partition, Some(0));
        assert_eq!(result.offset, Some(0));
        assert!(result.timestamp_millis > 0);

        let delivery = receiver.recv().await.expect("delivery");
        assert_eq!(delivery.message().offset, result.offset);
    }

    #[test]
    fn kafka_record_preserves_standard_mq_metadata() {
        let mut record = KafkaRecord::new("orders", serde_json::json!({"id": 1}));
        record
            .headers
            .insert("x-roze-idempotency-key".to_string(), "order-1".to_string());
        record.timestamp_millis = 42;
        record.partition = Some(3);
        record.offset = Some(99);
        record.group = Some("workers".to_string());

        let mq = record.to_mq_message();
        assert_eq!(mq.timestamp_millis, 42);
        assert_eq!(mq.partition, Some(3));
        assert_eq!(mq.offset, Some(99));
        assert_eq!(mq.group.as_deref(), Some("workers"));

        let restored = KafkaRecord::from_mq_message(mq);
        assert_eq!(restored.timestamp_millis, 42);
        assert_eq!(restored.partition, Some(3));
        assert_eq!(restored.offset, Some(99));
        assert_eq!(restored.group.as_deref(), Some("workers"));
        assert_eq!(
            restored.to_mq_message().idempotency_key.as_deref(),
            Some("order-1")
        );
    }

    #[test]
    fn recover_action_retries_to_raw_retry_topic() {
        let cfg = KafkaConfig {
            topic_prefix: "app".to_string(),
            retry_topic: Some("orders.retry".to_string()),
            dead_letter_topic: Some("orders.dlq".to_string()),
            retry_backoff_ms: 250,
            max_retries: 3,
            ..Default::default()
        };
        let mut record = KafkaRecord::new("orders", serde_json::json!({"id": 1}));
        record.attempt = 1;

        let action = recover_action(&cfg, &record);

        assert_eq!(
            action,
            RecoverAction::Retry {
                topic: "orders.retry".to_string(),
                attempt: 2,
                backoff_ms: 250,
            }
        );
        if let RecoverAction::Retry { topic, .. } = action {
            assert_eq!(cfg.topic_name(topic), "app.orders.retry");
        }
    }

    #[test]
    fn recover_action_dead_letters_after_retry_limit() {
        let cfg = KafkaConfig {
            topic_prefix: "app".to_string(),
            retry_topic: Some("orders.retry".to_string()),
            dead_letter_topic: Some("orders.dlq".to_string()),
            max_retries: 3,
            ..Default::default()
        };
        let mut record = KafkaRecord::new("orders", serde_json::json!({"id": 1}));
        record.attempt = 3;

        let action = recover_action(&cfg, &record);

        assert_eq!(
            action,
            RecoverAction::DeadLetter {
                topic: "orders.dlq".to_string(),
            }
        );
        if let RecoverAction::DeadLetter { topic } = action {
            assert_eq!(cfg.topic_name(topic), "app.orders.dlq");
        }
    }

    #[test]
    fn recover_action_drops_when_recovery_topic_missing() {
        let cfg = KafkaConfig {
            max_retries: 3,
            retry_topic: None,
            dead_letter_topic: Some("orders.dlq".to_string()),
            ..Default::default()
        };
        let record = KafkaRecord::new("orders", serde_json::json!({"id": 1}));

        assert_eq!(
            recover_action(&cfg, &record),
            RecoverAction::Drop {
                reason: "retry_topic_missing",
            }
        );
    }

    #[tokio::test]
    async fn nack_routes_to_dead_letter() {
        let broker = InMemoryKafkaBroker::with_dead_letter("dead", 1);
        let mut dead = broker.subscribe("dead").await.expect("subscribe dead");
        let mut orders = broker.subscribe("orders").await.expect("subscribe orders");

        broker
            .publish(
                KafkaRecord::new("orders", serde_json::json!({"id": 1}))
                    .with_dead_letter_topic("dead"),
            )
            .await
            .expect("publish");

        let first = orders.recv().await.expect("delivery");
        first.nack().await.expect("nack");

        let dead_delivery = dead.recv().await.expect("dead");
        assert_eq!(dead_delivery.message().topic, "dead");
        assert_eq!(dead_delivery.message().payload["id"], 1);

        let metrics = roze_metrics::http_metrics();
        assert!(metrics.contains("roze_queue_events_total"));
        assert!(metrics.contains(r#"system="kafka""#));
        assert!(metrics.contains(r#"topic="orders""#));
        assert!(metrics.contains(r#"outcome="published""#));
        assert!(metrics.contains(r#"outcome="nacked""#));
        assert!(metrics.contains(r#"outcome="dead_lettered""#));
    }

    #[tokio::test]
    async fn admin_replays_kafka_dead_letters() {
        let broker = InMemoryKafkaBroker::with_dead_letter("dead", 1);
        let mut orders = broker.subscribe("orders").await.expect("subscribe orders");
        let mut replay = broker.subscribe("orders").await.expect("subscribe replay");

        broker
            .publish(KafkaRecord::new("orders", serde_json::json!({"id": 9})))
            .await
            .expect("publish");
        let first = orders.recv().await.expect("delivery");
        first.nack().await.expect("nack");

        let records = MqAdmin::dead_letters(&broker, 0, 10)
            .await
            .expect("dead letters");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].original_topic, "orders");

        let replayed = broker
            .replay_dead_letter(records[0].id)
            .await
            .expect("replay")
            .expect("message");
        assert_eq!(replayed.topic, "orders");
        let delivery = replay.recv().await.expect("replayed");
        assert_eq!(delivery.message().payload["id"], 9);

        let stats = broker.stats().await.expect("stats");
        assert_eq!(stats.dead_lettered, 1);
        assert_eq!(stats.replayed, 1);
        assert_eq!(broker.clear_dead_letters().await.expect("clear"), 1);
    }

    #[tokio::test]
    async fn spawn_consumer_auto_ack() {
        let broker = InMemoryKafkaBroker::new();
        let seen = Arc::new(AtomicUsize::new(0));
        let seen_for_thread = seen.clone();

        let handle = spawn_consumer(&broker, "tasks", move |delivery| {
            let seen_for_thread = seen_for_thread.clone();
            async move {
                seen_for_thread.fetch_add(1, Ordering::SeqCst);
                assert_eq!(delivery.message().topic, "tasks");
                Ok(())
            }
        })
        .await
        .expect("spawn");

        broker
            .publish(KafkaRecord::new("tasks", serde_json::json!({"id": 2})))
            .await
            .expect("publish");

        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        handle.abort();
        assert_eq!(seen.load(Ordering::SeqCst), 1);
    }

    #[cfg(feature = "rdkafka")]
    #[tokio::test]
    #[ignore = "requires ROZE_TEST_KAFKA_BROKERS and an externally managed Kafka restart cycle"]
    async fn production_soak_rdkafka_disconnect_recovery() {
        let brokers =
            std::env::var("ROZE_TEST_KAFKA_BROKERS").expect("ROZE_TEST_KAFKA_BROKERS is required");
        let probe_broker = split_csv(&brokers)
            .into_iter()
            .next()
            .expect("ROZE_TEST_KAFKA_BROKERS must contain one endpoint");
        let probe_address = probe_broker
            .parse::<std::net::SocketAddr>()
            .expect("ROZE_TEST_KAFKA_BROKERS must contain host:port");
        let seconds = std::env::var("ROZE_KAFKA_SOAK_SECONDS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(300);
        let require_disconnect =
            std::env::var("ROZE_KAFKA_REQUIRE_DISCONNECT").is_ok_and(|value| value == "1");
        let suffix = format!("{}-{}", std::process::id(), current_millis());
        let topic = format!("events-{suffix}");
        let config = KafkaConfig {
            brokers: split_csv(&brokers),
            client_id: Some(format!("roze-soak-{suffix}")),
            group_id: Some(format!("roze-soak-{suffix}")),
            topic_prefix: format!("roze.soak.{suffix}"),
            enable_manual_ack: true,
            enable_auto_commit: false,
            flush_timeout_ms: 2_000,
            message_timeout_ms: 2_000,
            linger_ms: 0,
            retry_backoff_ms: 100,
            max_retries: 3,
            ..Default::default()
        };
        let producer = RdkafkaProducer::new(config.clone()).expect("create Kafka producer");
        let subscriber = RdkafkaSubscriber::new(config);
        let mut receiver = subscriber.subscribe(&topic).await.expect("subscribe");
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
            if require_disconnect {
                if std::net::TcpStream::connect_timeout(&probe_address, Duration::from_millis(500))
                    .is_err()
                {
                    disconnect_observations = disconnect_observations.saturating_add(1);
                    recovery_started.get_or_insert_with(std::time::Instant::now);
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    continue;
                }
                let metadata_result = producer
                    .producer
                    .client()
                    .fetch_metadata(None, Timeout::After(Duration::from_millis(500)));
                if metadata_result.is_err() {
                    disconnect_observations = disconnect_observations.saturating_add(1);
                    recovery_started.get_or_insert_with(std::time::Instant::now);
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    continue;
                }
                if let Some(recovery_started_at) = recovery_started.take() {
                    recoveries = recoveries.saturating_add(1);
                    recovery_latency.observe(recovery_started_at.elapsed());
                }
            }
            let operation_started = std::time::Instant::now();
            let mut message = KafkaRecord::new(
                &topic,
                serde_json::json!({"sequence": attempts, "sent_at_ms": current_millis()}),
            );
            message.key = Some(format!("{suffix}-{attempts}"));
            let publish_result =
                tokio::time::timeout(std::time::Duration::from_secs(5), producer.publish(message))
                    .await;
            if !matches!(publish_result, Ok(Ok(()))) {
                disconnect_observations = disconnect_observations.saturating_add(1);
                recovery_started.get_or_insert_with(std::time::Instant::now);
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                delivery_latency.observe(operation_started.elapsed());
                continue;
            }

            let receive_result =
                tokio::time::timeout(std::time::Duration::from_secs(5), receiver.recv()).await;
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

        let _ = producer.flush();
        let elapsed_ms = started.elapsed().as_millis().max(1);
        let messages_per_second_milli =
            u128::from(delivered).saturating_mul(1_000_000) / elapsed_ms;
        let p99_delivery_us = delivery_latency
            .percentile_upper_bound_micros(99)
            .expect("Kafka delivery latency");
        let p99_recovery_us = recovery_latency
            .percentile_upper_bound_micros(99)
            .unwrap_or(0);
        println!(
            "roze_kafka_soak kafka_elapsed_ms={elapsed_ms} kafka_attempts={attempts} kafka_delivered={delivered} kafka_disconnect_observations={disconnect_observations} kafka_recoveries={recoveries} kafka_messages_per_second_milli={messages_per_second_milli} kafka_p99_delivery_us={p99_delivery_us} kafka_p99_recovery_us={p99_recovery_us}"
        );

        assert!(attempts > 0);
        assert!(delivered > 0);
        if require_disconnect {
            assert!(disconnect_observations > 0);
            assert!(recoveries > 0);
            assert!(p99_recovery_us > 0);
            assert!(
                recovery_started.is_none(),
                "Kafka did not recover before the soak ended"
            );
        }
    }
}
