use serde::{Deserialize, Serialize};
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
use tokio::sync::mpsc;
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

type DeliveryActionFuture = Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send>>;
type DeliveryAction = Arc<dyn Fn() -> DeliveryActionFuture + Send + Sync + 'static>;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct KafkaConfig {
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
        let mut event = roze_eventbus::EventEnvelope::new(self.topic, self.payload);
        event.key = self.key;
        event.headers = self.headers;
        event.attempt = self.attempt;
        event
    }

    pub fn from_event(event: roze_eventbus::EventEnvelope) -> Self {
        Self {
            topic: event.topic,
            key: event.key,
            headers: event.headers,
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
        roze_mq::Message {
            topic: self.topic.clone(),
            key: self.key.clone(),
            headers: self.headers.clone(),
            timestamp_millis: self.timestamp_millis,
            partition: self.partition,
            offset: self.offset,
            group: self.group.clone(),
            attempt: self.attempt,
            dead_letter_topic: self.dead_letter_topic.clone(),
            idempotency_key: None,
            available_at_millis: None,
            payload: self.payload.clone(),
        }
    }

    pub fn from_mq_message(message: roze_mq::Message) -> Self {
        Self {
            topic: message.topic,
            key: message.key,
            headers: message.headers,
            timestamp_millis: message.timestamp_millis,
            partition: message.partition,
            offset: message.offset,
            group: message.group,
            payload: message.payload,
            attempt: message.attempt,
            dead_letter_topic: message.dead_letter_topic,
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
}

#[async_trait::async_trait]
pub trait Subscriber: Send + Sync + 'static {
    async fn subscribe(&self, topic: &str) -> anyhow::Result<broadcast::Receiver<Delivery>>;
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

        let ack_broker = self.clone();
        let ack_fn: DeliveryAction = Arc::new(move || {
            let broker = ack_broker.clone();
            Box::pin(async move {
                broker.acked.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
        });
        let nack_fn: DeliveryAction = Arc::new(move || {
            let broker = broker.clone();
            let message = message_for_nack.clone();
            Box::pin(async move {
                broker.nacked.fetch_add(1, Ordering::SeqCst);
                broker.requeue_or_dead_letter(message).await;
                Ok(())
            })
        });

        Delivery::new(message, ack_fn, nack_fn)
    }

    fn route_or_dead_letter(&self, mut message: KafkaRecord) {
        self.prepare_delivery_metadata(&mut message);
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
            return;
        }

        let _ = self
            .sender_for(&message.topic)
            .send(self.make_delivery(message));
        self.delivered.fetch_add(1, Ordering::SeqCst);
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

    async fn route(&self, message: KafkaRecord) {
        self.route_or_dead_letter(message);
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
    async fn publish(&self, mut message: KafkaRecord) -> anyhow::Result<()> {
        message.ensure_trace_id();
        self.published.fetch_add(1, Ordering::SeqCst);
        self.route(message).await;
        Ok(())
    }
}

#[async_trait::async_trait]
impl Subscriber for InMemoryKafkaBroker {
    async fn subscribe(&self, topic: &str) -> anyhow::Result<broadcast::Receiver<Delivery>> {
        Ok(self.sender_for(topic).subscribe())
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
    async fn publish(&self, mut message: KafkaRecord) -> anyhow::Result<()> {
        message.ensure_trace_id();
        let topic = self.config.topic_name(message.topic);
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
            Ok((_partition, _offset)) => Ok(()),
            Err((error, _)) => Err(anyhow::anyhow!(error.to_string())),
        }
    }
}

#[cfg(feature = "rdkafka")]
#[derive(Debug)]
enum RdkafkaAckCmd {
    Commit(TopicOffsetMetadata),
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
    if cfg.max_retries == 0 {
        if let Some(topic) = cfg.dead_letter_topic.clone() {
            message.topic = cfg.topic_name(topic);
            message.attempt = 0;
            let log_topic = message.topic.clone();
            let log_attempt = message.attempt;
            RdkafkaProducer::new(cfg)?.publish(message).await?;
            tracing::warn!(
                event = "kafka.message.dead_lettered",
                topic = %log_topic,
                attempt = %log_attempt,
                "kafka message dead letter handled (max_retries=0)"
            );
            return Ok(());
        }
        tracing::warn!(
            event = "kafka.message.recover_dropped",
            topic = %message.topic,
            attempt = %message.attempt,
            "kafka message no retry or dead-letter topic configured, message dropped"
        );
        return Ok(());
    }

    if cfg.should_retry(message.attempt) {
        if let Some(topic) = cfg.retry_topic.clone() {
            message.topic = cfg.topic_name(topic);
            message.attempt = message.attempt.saturating_add(1);
            let backoff_ms = cfg.retry_backoff_ms;
            tracing::warn!(
                event = "kafka.message.requeue_retry",
                topic = %message.topic,
                attempt = %message.attempt,
                retry_backoff_ms = backoff_ms,
                max_retries = cfg.max_retries,
                "kafka message retry topic configured, requeueing"
            );
            tokio::time::sleep(Duration::from_millis(cfg.retry_backoff_ms)).await;
            RdkafkaProducer::new(cfg)?.publish(message).await?;
        } else {
            tracing::warn!(
                event = "kafka.message.retry_topic_missing",
                topic = %message.topic,
                attempt = %message.attempt,
                "kafka message retry topic not configured, message dropped"
            );
        }
        return Ok(());
    }

    if let Some(topic) = cfg.dead_letter_topic.clone() {
        message.topic = cfg.topic_name(topic);
        message.attempt = 0;
        let log_topic = message.topic.clone();
        let log_attempt = message.attempt;
        RdkafkaProducer::new(cfg.clone())?.publish(message).await?;
        tracing::warn!(
            event = "kafka.message.dead_lettered",
            topic = %log_topic,
            attempt = %log_attempt,
            max_retries = cfg.max_retries,
            "kafka message moved to dead letter"
        );
        return Ok(());
    }

    tracing::warn!(
        event = "kafka.message.dead_letter_missing",
        topic = %message.topic,
        attempt = %message.attempt,
        max_retries = cfg.max_retries,
        "kafka message dead-letter topic not configured, message dropped"
    );
    Ok(())
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
        consumer.commit(&offsets, CommitMode::Async)?;
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
                                            ack_tx
                                                .send(RdkafkaAckCmd::Commit(meta))
                                                .map_err(|_| anyhow::anyhow!("ack channel closed"))
                                                .map(|_| ())
                                        })
                                    }),
                                    Arc::new(move || {
                                        let cfg = cfg_for_nack.clone();
                                        let message = message_for_nack.clone();
                                        Box::pin(async move {
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
                        if let Some(RdkafkaAckCmd::Commit(meta)) = maybe_ack {
                            if let Err(err) = RdkafkaSubscriber::commit_with_consumer(&consumer, &meta) {
                                tracing::warn!(topic=%meta.topic, error=%err, "kafka commit failed");
                            }
                        }
                    }
                }
            }
        });

        Ok(sender.subscribe())
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

fn current_millis() -> u64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(elapsed) => elapsed.as_millis() as u64,
        Err(_) => 0,
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

    #[test]
    fn kafka_record_preserves_standard_mq_metadata() {
        let mut record = KafkaRecord::new("orders", serde_json::json!({"id": 1}));
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
}
