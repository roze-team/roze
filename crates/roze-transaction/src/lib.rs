use std::{
    collections::BTreeMap,
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
};

use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

type BoxFutureResult = Pin<Box<dyn Future<Output = Result<()>> + Send + 'static>>;

#[derive(Clone)]
pub struct TransactionAction {
    pub name: Arc<str>,
    apply: Arc<dyn Fn() -> BoxFutureResult + Send + Sync>,
    rollback: Arc<dyn Fn() -> BoxFutureResult + Send + Sync>,
}

impl TransactionAction {
    pub fn new<F, Fut, R, RFut>(name: impl Into<Arc<str>>, apply: F, rollback: R) -> Self
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<()>> + Send + 'static,
        R: Fn() -> RFut + Send + Sync + 'static,
        RFut: Future<Output = Result<()>> + Send + 'static,
    {
        Self {
            name: name.into(),
            apply: Arc::new(move || Box::pin(apply())),
            rollback: Arc::new(move || Box::pin(rollback())),
        }
    }

    pub async fn apply(&self) -> Result<()> {
        (self.apply)().await
    }

    pub async fn rollback(&self) -> Result<()> {
        (self.rollback)().await
    }
}

#[derive(Default, Clone)]
pub struct TransactionPlan {
    steps: Vec<TransactionAction>,
}

pub type Saga = TransactionPlan;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SagaRetryPolicy {
    pub max_attempts: u32,
    pub initial_backoff_millis: u64,
    pub max_backoff_millis: u64,
}

impl Default for SagaRetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            initial_backoff_millis: 100,
            max_backoff_millis: 5_000,
        }
    }
}

impl TransactionPlan {
    pub fn new() -> Self {
        Self { steps: Vec::new() }
    }

    pub fn push(&mut self, action: TransactionAction) {
        self.steps.push(action);
    }

    pub fn len(&self) -> usize {
        self.steps.len()
    }

    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }

    pub async fn execute(&self) -> Result<()> {
        let mut applied: Vec<&TransactionAction> = Vec::new();
        for action in &self.steps {
            if let Err(err) = action.apply().await {
                for previous in applied.iter().rev() {
                    let _ = previous.rollback().await;
                }
                return Err(err);
            }
            applied.push(action);
        }
        Ok(())
    }

    pub async fn execute_with_retry(&self, policy: SagaRetryPolicy) -> Result<()> {
        let attempts = policy.max_attempts.max(1);
        for attempt in 0..attempts {
            match self.execute().await {
                Ok(()) => return Ok(()),
                Err(err) if attempt + 1 == attempts => return Err(err),
                Err(_) => {
                    let multiplier = 2u64.saturating_pow(attempt.min(6));
                    let delay = policy
                        .initial_backoff_millis
                        .saturating_mul(multiplier)
                        .min(policy.max_backoff_millis);
                    if delay > 0 {
                        tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
                    }
                }
            }
        }
        unreachable!("at least one saga execution attempt is required")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OutboxStatus {
    Pending,
    Publishing,
    Published,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutboxMessage {
    pub id: String,
    pub topic: String,
    pub key: Option<String>,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    pub idempotency_key: String,
    pub payload: serde_json::Value,
    pub status: OutboxStatus,
    pub attempts: u32,
    pub next_attempt_millis: Option<u64>,
    pub lease_until_millis: Option<u64>,
    pub last_error: Option<String>,
}

impl OutboxMessage {
    pub fn new(
        id: impl Into<String>,
        topic: impl Into<String>,
        idempotency_key: impl Into<String>,
        payload: serde_json::Value,
    ) -> Self {
        Self {
            id: id.into(),
            topic: topic.into(),
            key: None,
            headers: BTreeMap::new(),
            idempotency_key: idempotency_key.into(),
            payload,
            status: OutboxStatus::Pending,
            attempts: 0,
            next_attempt_millis: None,
            lease_until_millis: None,
            last_error: None,
        }
    }

    pub fn with_context(
        context: &roze_context::Context,
        id: impl Into<String>,
        topic: impl Into<String>,
        idempotency_key: impl Into<String>,
        payload: serde_json::Value,
    ) -> Self {
        let mut message = Self::new(id, topic, idempotency_key, payload);
        message.headers = context.propagation_headers();
        message
    }

    pub fn to_mq_message(&self) -> roze_mq::Message {
        let mut message = roze_mq::Message {
            topic: self.topic.clone(),
            key: self.key.clone(),
            headers: self.headers.clone().into_iter().collect(),
            timestamp_millis: 0,
            partition: None,
            offset: None,
            group: None,
            attempt: self.attempts,
            dead_letter_topic: None,
            idempotency_key: Some(self.idempotency_key.clone()),
            available_at_millis: self.next_attempt_millis,
            payload: self.payload.clone(),
        };
        message.ensure_trace_id();
        message
    }

    pub fn mark_publishing(&mut self, lease_until_millis: u64) {
        self.status = OutboxStatus::Publishing;
        self.lease_until_millis = Some(lease_until_millis);
    }

    pub fn mark_published(&mut self) {
        self.status = OutboxStatus::Published;
        self.next_attempt_millis = None;
        self.lease_until_millis = None;
        self.last_error = None;
    }

    pub fn mark_failed(&mut self, error: impl Into<String>, next_attempt_millis: Option<u64>) {
        self.status = OutboxStatus::Failed;
        self.attempts = self.attempts.saturating_add(1);
        self.next_attempt_millis = next_attempt_millis;
        self.lease_until_millis = None;
        self.last_error = Some(error.into());
    }
}

#[async_trait]
pub trait OutboxStore: std::fmt::Debug + Send + Sync + 'static {
    async fn enqueue(&self, message: OutboxMessage) -> Result<bool>;
    async fn get(&self, id: &str) -> Result<Option<OutboxMessage>>;
    async fn claim_pending(
        &self,
        now_millis: u64,
        limit: usize,
        lease_until_millis: u64,
    ) -> Result<Vec<OutboxMessage>>;
    async fn mark_published(&self, id: &str) -> Result<()>;
    async fn mark_failed(
        &self,
        id: &str,
        error: &str,
        next_attempt_millis: Option<u64>,
    ) -> Result<()>;
}

#[async_trait]
pub trait TransactionalOutbox<Tx>: Send + Sync
where
    Tx: Send,
{
    async fn enqueue_in_transaction(
        &self,
        transaction: &mut Tx,
        messages: &[OutboxMessage],
    ) -> Result<()>;
}

#[derive(Debug, Clone, Default)]
pub struct InMemoryOutbox {
    messages: Arc<Mutex<BTreeMap<String, OutboxMessage>>>,
}

impl InMemoryOutbox {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl OutboxStore for InMemoryOutbox {
    async fn enqueue(&self, message: OutboxMessage) -> Result<bool> {
        Ok(self
            .messages
            .lock()
            .expect("outbox lock poisoned")
            .insert(message.id.clone(), message)
            .is_none())
    }

    async fn get(&self, id: &str) -> Result<Option<OutboxMessage>> {
        Ok(self
            .messages
            .lock()
            .expect("outbox lock poisoned")
            .get(id)
            .cloned())
    }

    async fn claim_pending(
        &self,
        now_millis: u64,
        limit: usize,
        lease_until_millis: u64,
    ) -> Result<Vec<OutboxMessage>> {
        let mut messages = self.messages.lock().expect("outbox lock poisoned");
        let mut claimed = Vec::new();
        for message in messages.values_mut() {
            let retry_ready = message
                .next_attempt_millis
                .map(|next| next <= now_millis)
                .unwrap_or(true);
            let lease_expired = message
                .lease_until_millis
                .map(|lease| lease <= now_millis)
                .unwrap_or(true);
            let claimable = match message.status {
                OutboxStatus::Pending | OutboxStatus::Failed => retry_ready,
                OutboxStatus::Publishing => lease_expired,
                OutboxStatus::Published => false,
            };
            if claimable {
                message.mark_publishing(lease_until_millis);
                claimed.push(message.clone());
                if claimed.len() >= limit.max(1) {
                    break;
                }
            }
        }
        Ok(claimed)
    }

    async fn mark_published(&self, id: &str) -> Result<()> {
        if let Some(message) = self
            .messages
            .lock()
            .expect("outbox lock poisoned")
            .get_mut(id)
        {
            message.mark_published();
        }
        Ok(())
    }

    async fn mark_failed(
        &self,
        id: &str,
        error: &str,
        next_attempt_millis: Option<u64>,
    ) -> Result<()> {
        if let Some(message) = self
            .messages
            .lock()
            .expect("outbox lock poisoned")
            .get_mut(id)
        {
            message.mark_failed(error, next_attempt_millis);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OutboxRelayReport {
    pub published: usize,
    pub failed: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutboxRelayConfig {
    pub limit: usize,
    pub lease_millis: u64,
    pub initial_backoff_millis: u64,
    pub max_backoff_millis: u64,
}

impl Default for OutboxRelayConfig {
    fn default() -> Self {
        Self {
            limit: 100,
            lease_millis: 30_000,
            initial_backoff_millis: 1_000,
            max_backoff_millis: 60_000,
        }
    }
}

pub async fn relay_outbox_batch<S, P>(
    outbox: &S,
    publisher: &P,
    now_millis: u64,
    config: OutboxRelayConfig,
) -> Result<OutboxRelayReport>
where
    S: OutboxStore + ?Sized,
    P: roze_mq::Publisher,
{
    let mut report = OutboxRelayReport::default();
    let lease_until = now_millis.saturating_add(config.lease_millis);
    for message in outbox
        .claim_pending(now_millis, config.limit, lease_until)
        .await?
    {
        match publisher.publish(message.to_mq_message()).await {
            Ok(()) => {
                outbox.mark_published(&message.id).await?;
                report.published += 1;
            }
            Err(error) => {
                let next_attempt = next_attempt_millis(now_millis, message.attempts, config);
                outbox
                    .mark_failed(&message.id, &error.to_string(), Some(next_attempt))
                    .await?;
                report.failed += 1;
            }
        }
    }
    Ok(report)
}

fn next_attempt_millis(now_millis: u64, attempts: u32, config: OutboxRelayConfig) -> u64 {
    let delay = config
        .initial_backoff_millis
        .saturating_mul(2u64.saturating_pow(attempts.min(6)))
        .min(config.max_backoff_millis);
    now_millis.saturating_add(delay)
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InboxDeduper {
    processed: std::collections::BTreeSet<String>,
}

impl InboxDeduper {
    pub fn has_processed(&self, idempotency_key: impl AsRef<str>) -> bool {
        self.processed.contains(idempotency_key.as_ref())
    }

    pub fn mark_processed(&mut self, idempotency_key: impl Into<String>) -> bool {
        self.processed.insert(idempotency_key.into())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InboxStatus {
    Processing,
    Processed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InboxMessage {
    pub idempotency_key: String,
    pub topic: String,
    pub group: Option<String>,
    pub status: InboxStatus,
    pub attempts: u32,
    pub first_seen_millis: u64,
    pub updated_at_millis: u64,
    pub next_attempt_millis: Option<u64>,
    #[serde(default)]
    pub last_error: Option<String>,
}

impl InboxMessage {
    pub fn new(
        idempotency_key: impl Into<String>,
        topic: impl Into<String>,
        group: Option<String>,
        now_millis: u64,
    ) -> Self {
        Self {
            idempotency_key: idempotency_key.into(),
            topic: topic.into(),
            group,
            status: InboxStatus::Processing,
            attempts: 1,
            first_seen_millis: now_millis,
            updated_at_millis: now_millis,
            next_attempt_millis: None,
            last_error: None,
        }
    }

    pub fn mark_processed(&mut self, now_millis: u64) {
        self.status = InboxStatus::Processed;
        self.updated_at_millis = now_millis;
        self.next_attempt_millis = None;
        self.last_error = None;
    }

    pub fn mark_failed(
        &mut self,
        now_millis: u64,
        error: impl Into<String>,
        next_attempt_millis: Option<u64>,
    ) {
        self.status = InboxStatus::Failed;
        self.updated_at_millis = now_millis;
        self.last_error = Some(error.into());
        self.next_attempt_millis = next_attempt_millis;
    }

    pub fn begin_retry(&mut self, now_millis: u64) {
        self.status = InboxStatus::Processing;
        self.attempts = self.attempts.saturating_add(1);
        self.updated_at_millis = now_millis;
        self.next_attempt_millis = None;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InboxBegin {
    Started,
    DuplicateProcessed,
    AlreadyProcessing,
    RetryStarted,
}

#[derive(Debug, Clone, Default)]
pub struct InMemoryInbox {
    messages: Arc<Mutex<BTreeMap<String, InboxMessage>>>,
}

impl InMemoryInbox {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn begin(
        &self,
        idempotency_key: impl Into<String>,
        topic: impl Into<String>,
        group: Option<String>,
        now_millis: u64,
    ) -> InboxBegin {
        let idempotency_key = idempotency_key.into();
        let topic = topic.into();
        let mut messages = self.messages.lock().expect("inbox lock poisoned");
        match messages.get_mut(&idempotency_key) {
            Some(message) if message.status == InboxStatus::Processed => {
                InboxBegin::DuplicateProcessed
            }
            Some(message) if message.status == InboxStatus::Processing => {
                InboxBegin::AlreadyProcessing
            }
            Some(message)
                if message.status == InboxStatus::Failed
                    && message
                        .next_attempt_millis
                        .map(|next| next <= now_millis)
                        .unwrap_or(true) =>
            {
                message.begin_retry(now_millis);
                InboxBegin::RetryStarted
            }
            Some(_) => InboxBegin::AlreadyProcessing,
            None => {
                messages.insert(
                    idempotency_key.clone(),
                    InboxMessage::new(idempotency_key, topic, group, now_millis),
                );
                InboxBegin::Started
            }
        }
    }

    pub fn mark_processed(&self, idempotency_key: &str, now_millis: u64) -> bool {
        let mut messages = self.messages.lock().expect("inbox lock poisoned");
        let Some(message) = messages.get_mut(idempotency_key) else {
            return false;
        };
        message.mark_processed(now_millis);
        true
    }

    pub fn mark_failed(
        &self,
        idempotency_key: &str,
        now_millis: u64,
        error: impl Into<String>,
        next_attempt_millis: Option<u64>,
    ) -> bool {
        let mut messages = self.messages.lock().expect("inbox lock poisoned");
        let Some(message) = messages.get_mut(idempotency_key) else {
            return false;
        };
        message.mark_failed(now_millis, error, next_attempt_millis);
        true
    }

    pub fn get(&self, idempotency_key: &str) -> Option<InboxMessage> {
        self.messages
            .lock()
            .expect("inbox lock poisoned")
            .get(idempotency_key)
            .cloned()
    }

    pub fn pending_retry(&self, now_millis: u64, limit: usize) -> Vec<InboxMessage> {
        self.messages
            .lock()
            .expect("inbox lock poisoned")
            .values()
            .filter(|message| {
                message.status == InboxStatus::Failed
                    && message
                        .next_attempt_millis
                        .map(|next| next <= now_millis)
                        .unwrap_or(true)
            })
            .take(limit.max(1))
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use roze_mq::Subscriber;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    #[tokio::test]
    async fn rolls_back_on_failure() {
        let applied = Arc::new(AtomicUsize::new(0));
        let rolled_back = Arc::new(AtomicUsize::new(0));

        let mut plan = TransactionPlan::new();
        for idx in 0..3 {
            let applied_clone = applied.clone();
            let rolled_back_clone = rolled_back.clone();
            plan.push(TransactionAction::new(
                format!("step-{idx}"),
                move || {
                    let applied_clone = applied_clone.clone();
                    async move {
                        let _ = applied_clone.fetch_add(1, Ordering::SeqCst);
                        if idx == 2 {
                            anyhow::bail!("boom");
                        }
                        Ok(())
                    }
                },
                move || {
                    let rolled_back_clone = rolled_back_clone.clone();
                    async move {
                        let _ = rolled_back_clone.fetch_add(1, Ordering::SeqCst);
                        Ok(())
                    }
                },
            ));
        }

        assert!(plan.execute().await.is_err());
        assert_eq!(applied.load(Ordering::SeqCst), 3);
        assert_eq!(rolled_back.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn retries_only_after_a_failed_attempt_has_been_compensated() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let rollbacks = Arc::new(AtomicUsize::new(0));
        let mut plan = TransactionPlan::new();
        let attempts_for_apply = attempts.clone();
        let rollbacks_for_rollback = rollbacks.clone();
        plan.push(TransactionAction::new(
            "local-write",
            || async { Ok(()) },
            move || {
                let rollbacks = rollbacks_for_rollback.clone();
                async move {
                    rollbacks.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                }
            },
        ));
        plan.push(TransactionAction::new(
            "remote-write",
            move || {
                let attempts = attempts_for_apply.clone();
                async move {
                    if attempts.fetch_add(1, Ordering::SeqCst) == 0 {
                        anyhow::bail!("transient failure");
                    }
                    Ok(())
                }
            },
            || async { Ok(()) },
        ));

        plan.execute_with_retry(SagaRetryPolicy {
            max_attempts: 2,
            initial_backoff_millis: 0,
            max_backoff_millis: 0,
        })
        .await
        .expect("second attempt succeeds");

        assert_eq!(attempts.load(Ordering::SeqCst), 2);
        assert_eq!(rollbacks.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn outbox_tracks_status_attempts_and_errors() {
        let mut message = OutboxMessage::new(
            "1",
            "orders",
            "order-created-1",
            serde_json::json!({"id": 1}),
        );
        message.mark_failed("broker timeout", Some(42));
        assert_eq!(message.status, OutboxStatus::Failed);
        assert_eq!(message.attempts, 1);
        assert_eq!(message.last_error.as_deref(), Some("broker timeout"));
        message.mark_published();
        assert_eq!(message.status, OutboxStatus::Published);
        assert!(message.last_error.is_none());
    }

    #[tokio::test]
    async fn outbox_relay_publishes_to_mq_with_context() {
        let outbox = InMemoryOutbox::new();
        let broker = roze_mq::InMemoryBroker::new();
        let mut rx = broker.subscribe("orders").await.expect("subscribe");
        let ctx =
            roze_context::Context::background_with_request_id_and_trace_id("request-1", "trace-1")
                .with_locale("zh-CN");

        outbox
            .enqueue(OutboxMessage::with_context(
                &ctx,
                "msg-1",
                "orders",
                "order-1",
                serde_json::json!({"id": 1}),
            ))
            .await
            .expect("enqueue");

        let report = relay_outbox_batch(
            &outbox,
            &broker,
            1,
            OutboxRelayConfig {
                limit: 10,
                ..Default::default()
            },
        )
        .await
        .expect("relay");
        let delivered = rx.recv().await.expect("delivery");

        assert_eq!(report.published, 1);
        assert_eq!(
            outbox
                .get("msg-1")
                .await
                .expect("get")
                .expect("message")
                .status,
            OutboxStatus::Published
        );
        assert_eq!(
            delivered.message().idempotency_key.as_deref(),
            Some("order-1")
        );
        assert_eq!(delivered.message().context().trace_id(), "trace-1");
        assert_eq!(
            delivered.message().context().locale().as_deref(),
            Some("zh-CN")
        );
    }

    #[tokio::test]
    async fn outbox_claim_lease_prevents_parallel_delivery_and_recovers() {
        let outbox = InMemoryOutbox::new();
        outbox
            .enqueue(OutboxMessage::new(
                "msg-1",
                "orders",
                "order-1",
                serde_json::json!({"id": 1}),
            ))
            .await
            .expect("enqueue");

        let first = outbox
            .claim_pending(100, 10, 200)
            .await
            .expect("first claim");
        let concurrent = outbox
            .claim_pending(150, 10, 250)
            .await
            .expect("concurrent claim");
        let recovered = outbox
            .claim_pending(200, 10, 300)
            .await
            .expect("recovered claim");

        assert_eq!(first.len(), 1);
        assert!(concurrent.is_empty());
        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].lease_until_millis, Some(300));
    }

    #[test]
    fn inbox_deduper_marks_once() {
        let mut deduper = InboxDeduper::default();
        assert!(deduper.mark_processed("k1"));
        assert!(!deduper.mark_processed("k1"));
        assert!(deduper.has_processed("k1"));
    }

    #[test]
    fn inbox_tracks_processing_processed_and_duplicates() {
        let inbox = InMemoryInbox::new();

        assert_eq!(
            inbox.begin("order-1", "orders", Some("workers".to_string()), 100),
            InboxBegin::Started
        );
        assert_eq!(
            inbox.begin("order-1", "orders", Some("workers".to_string()), 101),
            InboxBegin::AlreadyProcessing
        );
        assert!(inbox.mark_processed("order-1", 110));
        assert_eq!(
            inbox.begin("order-1", "orders", Some("workers".to_string()), 120),
            InboxBegin::DuplicateProcessed
        );

        let message = inbox.get("order-1").expect("inbox message");
        assert_eq!(message.status, InboxStatus::Processed);
        assert_eq!(message.attempts, 1);
        assert_eq!(message.group.as_deref(), Some("workers"));
    }

    #[test]
    fn inbox_failed_messages_become_retryable_after_delay() {
        let inbox = InMemoryInbox::new();

        assert_eq!(
            inbox.begin("order-1", "orders", None, 100),
            InboxBegin::Started
        );
        assert!(inbox.mark_failed("order-1", 110, "db timeout", Some(200)));
        assert!(inbox.pending_retry(150, 10).is_empty());
        assert_eq!(inbox.pending_retry(200, 10).len(), 1);
        assert_eq!(
            inbox.begin("order-1", "orders", None, 200),
            InboxBegin::RetryStarted
        );

        let message = inbox.get("order-1").expect("inbox message");
        assert_eq!(message.status, InboxStatus::Processing);
        assert_eq!(message.attempts, 2);
        assert_eq!(message.last_error.as_deref(), Some("db timeout"));
    }
}
