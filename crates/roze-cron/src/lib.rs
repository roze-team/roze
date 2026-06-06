use std::{future::Future, sync::Arc, time::Duration};

use anyhow::Result;
use tokio::time::sleep;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CronSchedule {
    interval: Duration,
    initial_delay: Duration,
}

impl CronSchedule {
    pub fn every(interval: Duration) -> Self {
        Self {
            interval,
            initial_delay: Duration::ZERO,
        }
    }

    pub fn with_initial_delay(mut self, delay: Duration) -> Self {
        self.initial_delay = delay;
        self
    }

    pub fn interval(self) -> Duration {
        self.interval
    }
}

#[derive(Debug, Clone)]
pub struct CronJob {
    pub name: Arc<str>,
    pub schedule: CronSchedule,
}

impl CronJob {
    pub fn new(name: impl Into<Arc<str>>, schedule: CronSchedule) -> Self {
        Self {
            name: name.into(),
            schedule,
        }
    }

    pub async fn run<F, Fut>(&self, mut handler: F, iterations: Option<usize>) -> Result<()>
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = Result<()>>,
    {
        if !self.schedule.initial_delay.is_zero() {
            sleep(self.schedule.initial_delay).await;
        }

        let mut completed = 0usize;
        loop {
            if let Some(limit) = iterations {
                if completed >= limit {
                    break;
                }
            }

            handler().await?;
            completed += 1;

            if let Some(limit) = iterations {
                if completed >= limit {
                    break;
                }
            }

            sleep(self.schedule.interval).await;
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
    async fn runs_repeatedly() {
        let hits = Arc::new(AtomicUsize::new(0));
        let job = CronJob::new("demo", CronSchedule::every(Duration::from_millis(1)));
        let hits_clone = hits.clone();
        job.run(
            move || {
                let hits_clone = hits_clone.clone();
                async move {
                    let _ = hits_clone.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                }
            },
            Some(3),
        )
        .await
        .expect("run");
        assert_eq!(hits.load(Ordering::SeqCst), 3);
    }
}
