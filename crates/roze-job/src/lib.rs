use std::{future::Future, sync::Arc, time::Duration};

use async_trait::async_trait;
use tokio::{sync::Mutex, task::JoinHandle};

#[async_trait]
pub trait Job: Send + Sync + 'static {
    fn name(&self) -> &str;

    async fn run(&self) -> anyhow::Result<()>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Schedule {
    Once,
    After(Duration),
    Every(Duration),
}

#[derive(Default)]
pub struct JobScheduler {
    handles: Arc<Mutex<Vec<JoinHandle<()>>>>,
}

impl JobScheduler {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn spawn_once<J>(&self, job: J) -> anyhow::Result<()>
    where
        J: Job,
    {
        job.run().await
    }

    pub async fn spawn<J>(&self, job: J, schedule: Schedule) -> anyhow::Result<()>
    where
        J: Job,
    {
        match schedule {
            Schedule::Once => self.spawn_once(job).await,
            Schedule::After(delay) => {
                let job = Arc::new(job);
                let handle = tokio::spawn(async move {
                    tokio::time::sleep(delay).await;
                    if let Err(err) = job.run().await {
                        tracing::warn!(job = job.name(), error = %err, "job execution failed");
                    }
                });
                self.handles.lock().await.push(handle);
                Ok(())
            }
            Schedule::Every(interval) => {
                let job = Arc::new(job);
                let handle = tokio::spawn(async move {
                    let mut ticker = tokio::time::interval(interval);
                    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                    loop {
                        ticker.tick().await;
                        if let Err(err) = job.run().await {
                            tracing::warn!(job = job.name(), error = %err, "job execution failed");
                        }
                    }
                });
                self.handles.lock().await.push(handle);
                Ok(())
            }
        }
    }

    pub async fn shutdown(&self) {
        let mut handles = self.handles.lock().await;
        for handle in handles.drain(..) {
            handle.abort();
        }
    }
}

pub async fn run_once<J: Job>(job: &J) -> anyhow::Result<()> {
    job.run().await
}

pub fn boxed_job<F, Fut>(name: &'static str, func: F) -> impl Job
where
    F: Fn() -> Fut + Send + Sync + 'static,
    Fut: Future<Output = anyhow::Result<()>> + Send + 'static,
{
    struct FnJob<F> {
        name: &'static str,
        func: F,
    }

    #[async_trait]
    impl<F, Fut> Job for FnJob<F>
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = anyhow::Result<()>> + Send + 'static,
    {
        fn name(&self) -> &str {
            self.name
        }

        async fn run(&self) -> anyhow::Result<()> {
            (self.func)().await
        }
    }

    FnJob { name, func }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct EchoJob;

    #[async_trait]
    impl Job for EchoJob {
        fn name(&self) -> &str {
            "echo"
        }

        async fn run(&self) -> anyhow::Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn runs_once() {
        let scheduler = JobScheduler::new();
        scheduler.spawn_once(EchoJob).await.expect("run");
    }

    #[tokio::test]
    async fn boxed_job_executes() {
        let job = boxed_job("boxed", || async { Ok(()) });
        run_once(&job).await.expect("run");
    }

    #[tokio::test]
    async fn delayed_job_is_schedulable() {
        let scheduler = JobScheduler::new();
        scheduler
            .spawn(EchoJob, Schedule::After(Duration::from_millis(1)))
            .await
            .expect("schedule");
        scheduler.shutdown().await;
    }
}
