use std::{future::Future, pin::Pin, sync::Arc};

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
            idempotency_key: idempotency_key.into(),
            payload,
            status: OutboxStatus::Pending,
            attempts: 0,
            next_attempt_millis: None,
        }
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

    #[test]
    fn inbox_deduper_marks_once() {
        let mut deduper = InboxDeduper::default();
        assert!(deduper.mark_processed("k1"));
        assert!(!deduper.mark_processed("k1"));
        assert!(deduper.has_processed("k1"));
    }
}
