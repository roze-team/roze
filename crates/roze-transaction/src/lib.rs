use std::{
    collections::BTreeMap,
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
};

use anyhow::Result;
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OutboxStatus {
    Pending,
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
            attempt: self.attempts,
            dead_letter_topic: None,
            idempotency_key: Some(self.idempotency_key.clone()),
            available_at_millis: self.next_attempt_millis,
            payload: self.payload.clone(),
        };
        message.ensure_trace_id();
        message
    }

    pub fn mark_published(&mut self) {
        self.status = OutboxStatus::Published;
    }

    pub fn mark_failed(&mut self, next_attempt_millis: Option<u64>) {
        self.status = OutboxStatus::Failed;
        self.attempts = self.attempts.saturating_add(1);
        self.next_attempt_millis = next_attempt_millis;
    }
}

#[derive(Debug, Clone, Default)]
pub struct InMemoryOutbox {
    messages: Arc<Mutex<BTreeMap<String, OutboxMessage>>>,
}

impl InMemoryOutbox {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn enqueue(&self, message: OutboxMessage) -> bool {
        self.messages
            .lock()
            .expect("outbox lock poisoned")
            .insert(message.id.clone(), message)
            .is_none()
    }

    pub fn get(&self, id: &str) -> Option<OutboxMessage> {
        self.messages
            .lock()
            .expect("outbox lock poisoned")
            .get(id)
            .cloned()
    }

    pub fn pending(&self, now_millis: u64, limit: usize) -> Vec<OutboxMessage> {
        self.messages
            .lock()
            .expect("outbox lock poisoned")
            .values()
            .filter(|message| {
                matches!(message.status, OutboxStatus::Pending | OutboxStatus::Failed)
                    && message
                        .next_attempt_millis
                        .map(|next| next <= now_millis)
                        .unwrap_or(true)
            })
            .take(limit.max(1))
            .cloned()
            .collect()
    }

    pub fn mark_published(&self, id: &str) {
        if let Some(message) = self
            .messages
            .lock()
            .expect("outbox lock poisoned")
            .get_mut(id)
        {
            message.mark_published();
        }
    }

    pub fn mark_failed(&self, id: &str, next_attempt_millis: Option<u64>) {
        if let Some(message) = self
            .messages
            .lock()
            .expect("outbox lock poisoned")
            .get_mut(id)
        {
            message.mark_failed(next_attempt_millis);
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OutboxRelayReport {
    pub published: usize,
    pub failed: usize,
}

pub async fn relay_outbox_batch<P>(
    outbox: &InMemoryOutbox,
    publisher: &P,
    now_millis: u64,
    limit: usize,
) -> OutboxRelayReport
where
    P: roze_mq::Publisher,
{
    let mut report = OutboxRelayReport::default();
    for message in outbox.pending(now_millis, limit) {
        match publisher.publish(message.to_mq_message()).await {
            Ok(()) => {
                outbox.mark_published(&message.id);
                report.published += 1;
            }
            Err(_) => {
                outbox.mark_failed(
                    &message.id,
                    Some(next_attempt_millis(now_millis, message.attempts)),
                );
                report.failed += 1;
            }
        }
    }
    report
}

fn next_attempt_millis(now_millis: u64, attempts: u32) -> u64 {
    let delay = 1_000u64.saturating_mul(2u64.saturating_pow(attempts.min(6)));
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

    #[test]
    fn outbox_tracks_status_and_attempts() {
        let mut message = OutboxMessage::new(
            "1",
            "orders",
            "order-created-1",
            serde_json::json!({"id": 1}),
        );
        message.mark_failed(Some(42));
        assert_eq!(message.status, OutboxStatus::Failed);
        assert_eq!(message.attempts, 1);
        message.mark_published();
        assert_eq!(message.status, OutboxStatus::Published);
    }

    #[tokio::test]
    async fn outbox_relay_publishes_to_mq_with_context() {
        let outbox = InMemoryOutbox::new();
        let broker = roze_mq::InMemoryBroker::new();
        let mut rx = broker.subscribe("orders").await.expect("subscribe");
        let ctx =
            roze_context::Context::background_with_request_id_and_trace_id("request-1", "trace-1")
                .with_locale("zh-CN");

        outbox.enqueue(OutboxMessage::with_context(
            &ctx,
            "msg-1",
            "orders",
            "order-1",
            serde_json::json!({"id": 1}),
        ));

        let report = relay_outbox_batch(&outbox, &broker, 1, 10).await;
        let delivered = rx.recv().await.expect("delivery");

        assert_eq!(report.published, 1);
        assert_eq!(
            outbox.get("msg-1").expect("message").status,
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

    #[test]
    fn inbox_deduper_marks_once() {
        let mut deduper = InboxDeduper::default();
        assert!(deduper.mark_processed("k1"));
        assert!(!deduper.mark_processed("k1"));
        assert!(deduper.has_processed("k1"));
    }
}
