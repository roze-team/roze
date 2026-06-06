use std::{future::Future, pin::Pin, sync::Arc};

use anyhow::Result;

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
}
