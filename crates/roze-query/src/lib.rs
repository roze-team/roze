use std::{future::Future, pin::Pin, sync::Arc, time::Duration};

use thiserror::Error;
use tokio::{sync::Semaphore, task::JoinSet};
use tracing::Instrument;

type BoxQueryFuture<T> = Pin<Box<dyn Future<Output = anyhow::Result<T>> + Send + 'static>>;

pub struct QueryTask<T> {
    name: String,
    future: BoxQueryFuture<T>,
}

impl<T> QueryTask<T> {
    pub fn new<F, Fut>(name: impl Into<String>, query: F) -> Self
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = anyhow::Result<T>> + Send + 'static,
    {
        Self {
            name: name.into(),
            future: Box::pin(query()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartialFailurePolicy {
    Strict,
    Allow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueryCompositionConfig {
    pub total_timeout: Duration,
    pub per_call_timeout: Duration,
    pub max_fanout: usize,
    pub max_concurrency: usize,
    pub partial_failure: PartialFailurePolicy,
}

impl Default for QueryCompositionConfig {
    fn default() -> Self {
        Self {
            total_timeout: Duration::from_secs(3),
            per_call_timeout: Duration::from_secs(1),
            max_fanout: 16,
            max_concurrency: 8,
            partial_failure: PartialFailurePolicy::Strict,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryFailureKind {
    Timeout,
    Upstream,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryFailure {
    pub kind: QueryFailureKind,
    pub message: String,
}

#[derive(Debug)]
pub struct QueryOutcome<T> {
    pub name: String,
    pub value: Option<T>,
    pub failure: Option<QueryFailure>,
}

impl<T> QueryOutcome<T> {
    pub fn is_success(&self) -> bool {
        self.failure.is_none()
    }
}

#[derive(Debug)]
pub struct QueryBatch<T> {
    pub outcomes: Vec<QueryOutcome<T>>,
}

impl<T> QueryBatch<T> {
    pub fn is_complete(&self) -> bool {
        self.outcomes.iter().all(QueryOutcome::is_success)
    }

    pub fn successes(&self) -> impl Iterator<Item = (&str, &T)> {
        self.outcomes.iter().filter_map(|outcome| {
            outcome
                .value
                .as_ref()
                .map(|value| (outcome.name.as_str(), value))
        })
    }

    pub fn failures(&self) -> impl Iterator<Item = (&str, &QueryFailure)> {
        self.outcomes.iter().filter_map(|outcome| {
            outcome
                .failure
                .as_ref()
                .map(|failure| (outcome.name.as_str(), failure))
        })
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum QueryCompositionError {
    #[error("query fan-out {actual} exceeds limit {limit}")]
    FanoutLimit { actual: usize, limit: usize },
    #[error("query composition exceeded total timeout after completing {completed} calls")]
    TotalTimeout { completed: usize },
    #[error("strict query composition failed at `{name}`: {message}")]
    StrictFailure { name: String, message: String },
}

#[derive(Debug, Clone)]
pub struct QueryComposer {
    config: QueryCompositionConfig,
}

impl QueryComposer {
    pub fn new(config: QueryCompositionConfig) -> Self {
        Self { config }
    }

    pub async fn execute<T>(
        &self,
        tasks: Vec<QueryTask<T>>,
    ) -> Result<QueryBatch<T>, QueryCompositionError>
    where
        T: Send + 'static,
    {
        if tasks.len() > self.config.max_fanout {
            return Err(QueryCompositionError::FanoutLimit {
                actual: tasks.len(),
                limit: self.config.max_fanout,
            });
        }
        if tasks.is_empty() {
            return Ok(QueryBatch {
                outcomes: Vec::new(),
            });
        }

        let semaphore = Arc::new(Semaphore::new(self.config.max_concurrency.max(1)));
        let mut joins = JoinSet::new();
        for (index, task) in tasks.into_iter().enumerate() {
            let semaphore = semaphore.clone();
            let per_call_timeout = self.config.per_call_timeout;
            joins.spawn(async move {
                let name = task.name;
                let permit = semaphore.acquire_owned().await;
                let outcome = match permit {
                    Ok(_permit) => {
                        let span = tracing::info_span!("roze.query.call", upstream = %name);
                        match tokio::time::timeout(per_call_timeout, task.future)
                            .instrument(span)
                            .await
                        {
                            Ok(Ok(value)) => QueryOutcome {
                                name,
                                value: Some(value),
                                failure: None,
                            },
                            Ok(Err(error)) => QueryOutcome {
                                name,
                                value: None,
                                failure: Some(QueryFailure {
                                    kind: QueryFailureKind::Upstream,
                                    message: error.to_string(),
                                }),
                            },
                            Err(_) => QueryOutcome {
                                name,
                                value: None,
                                failure: Some(QueryFailure {
                                    kind: QueryFailureKind::Timeout,
                                    message: format!(
                                        "upstream call exceeded {} ms",
                                        per_call_timeout.as_millis()
                                    ),
                                }),
                            },
                        }
                    }
                    Err(_) => QueryOutcome {
                        name,
                        value: None,
                        failure: Some(QueryFailure {
                            kind: QueryFailureKind::Cancelled,
                            message: "query composer stopped".to_string(),
                        }),
                    },
                };
                (index, outcome)
            });
        }

        let task_count = joins.len();
        let collect = async {
            let mut ordered: Vec<Option<QueryOutcome<T>>> =
                std::iter::repeat_with(|| None).take(task_count).collect();
            while let Some(result) = joins.join_next().await {
                if let Ok((index, outcome)) = result {
                    ordered[index] = Some(outcome);
                }
            }
            ordered.into_iter().flatten().collect::<Vec<_>>()
        };

        let outcomes = match tokio::time::timeout(self.config.total_timeout, collect).await {
            Ok(outcomes) => outcomes,
            Err(_) => {
                joins.abort_all();
                return Err(QueryCompositionError::TotalTimeout {
                    completed: task_count.saturating_sub(joins.len()),
                });
            }
        };

        if self.config.partial_failure == PartialFailurePolicy::Strict {
            if let Some(outcome) = outcomes.iter().find(|outcome| !outcome.is_success()) {
                let failure = outcome.failure.as_ref().expect("failed outcome has reason");
                return Err(QueryCompositionError::StrictFailure {
                    name: outcome.name.clone(),
                    message: failure.message.clone(),
                });
            }
        }

        Ok(QueryBatch { outcomes })
    }
}

impl Default for QueryComposer {
    fn default() -> Self {
        Self::new(QueryCompositionConfig::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn preserves_task_order_while_running_concurrently() {
        let composer = QueryComposer::default();
        let batch = composer
            .execute(vec![
                QueryTask::new("slow", || async {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                    Ok(1)
                }),
                QueryTask::new("fast", || async { Ok(2) }),
            ])
            .await
            .unwrap();

        assert_eq!(batch.outcomes[0].name, "slow");
        assert_eq!(batch.outcomes[0].value, Some(1));
        assert_eq!(batch.outcomes[1].name, "fast");
        assert_eq!(batch.outcomes[1].value, Some(2));
    }

    #[tokio::test]
    async fn partial_mode_returns_successes_and_failures() {
        let composer = QueryComposer::new(QueryCompositionConfig {
            partial_failure: PartialFailurePolicy::Allow,
            ..Default::default()
        });
        let batch = composer
            .execute(vec![
                QueryTask::new("catalog", || async { Ok(1) }),
                QueryTask::new("inventory", || async { anyhow::bail!("offline") }),
            ])
            .await
            .unwrap();

        assert!(!batch.is_complete());
        assert_eq!(batch.successes().count(), 1);
        assert_eq!(batch.failures().count(), 1);
        assert_eq!(
            batch.outcomes[1].failure.as_ref().unwrap().kind,
            QueryFailureKind::Upstream
        );
    }

    #[tokio::test]
    async fn strict_mode_rejects_failed_upstream() {
        let error = QueryComposer::default()
            .execute::<usize>(vec![QueryTask::new("inventory", || async {
                anyhow::bail!("offline")
            })])
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            QueryCompositionError::StrictFailure { ref name, .. } if name == "inventory"
        ));
    }

    #[tokio::test]
    async fn enforces_fanout_and_per_call_timeout() {
        let limited = QueryComposer::new(QueryCompositionConfig {
            max_fanout: 1,
            ..Default::default()
        });
        let error = limited
            .execute(vec![
                QueryTask::new("one", || async { Ok(1) }),
                QueryTask::new("two", || async { Ok(2) }),
            ])
            .await
            .unwrap_err();
        assert_eq!(
            error,
            QueryCompositionError::FanoutLimit {
                actual: 2,
                limit: 1
            }
        );

        let timeout = QueryComposer::new(QueryCompositionConfig {
            per_call_timeout: Duration::from_millis(5),
            partial_failure: PartialFailurePolicy::Allow,
            ..Default::default()
        });
        let batch = timeout
            .execute(vec![QueryTask::new("slow", || async {
                tokio::time::sleep(Duration::from_millis(100)).await;
                Ok(1)
            })])
            .await
            .unwrap();
        assert_eq!(
            batch.outcomes[0].failure.as_ref().unwrap().kind,
            QueryFailureKind::Timeout
        );
    }
}
