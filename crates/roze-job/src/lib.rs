use std::{
    future::Future,
    sync::{Arc, Mutex as StdMutex},
    time::Duration,
};

use async_trait::async_trait;
use roze_service::{RuntimeService, ServiceFuture};
use tokio::{sync::Mutex, task::JoinHandle};

#[async_trait]
pub trait Job: Send + Sync + 'static {
    fn name(&self) -> &str;

    async fn run(&self) -> anyhow::Result<()>;
}

#[async_trait]
impl<J> Job for Arc<J>
where
    J: Job + ?Sized,
{
    fn name(&self) -> &str {
        (**self).name()
    }

    async fn run(&self) -> anyhow::Result<()> {
        (**self).run().await
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Schedule {
    Once,
    After(Duration),
    Every(Duration),
}

#[derive(Clone)]
struct ScheduledJob {
    job: Arc<dyn Job>,
    schedule: Schedule,
}

pub struct JobService {
    name: String,
    scheduler: JobScheduler,
    jobs: StdMutex<Option<Vec<ScheduledJob>>>,
}

impl JobService {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            scheduler: JobScheduler::new(),
            jobs: StdMutex::new(Some(Vec::new())),
        }
    }

    pub fn add<J>(&mut self, job: J, schedule: Schedule) -> &mut Self
    where
        J: Job,
    {
        self.jobs
            .get_mut()
            .expect("job service mutex")
            .as_mut()
            .expect("job service already started")
            .push(ScheduledJob {
                job: Arc::new(job),
                schedule,
            });
        self
    }

    pub fn add_arc<J>(&mut self, job: Arc<J>, schedule: Schedule) -> &mut Self
    where
        J: Job,
    {
        self.jobs
            .get_mut()
            .expect("job service mutex")
            .as_mut()
            .expect("job service already started")
            .push(ScheduledJob { job, schedule });
        self
    }
}

impl RuntimeService for JobService {
    fn name(&self) -> &str {
        &self.name
    }

    fn start(&self, shutdown: roze_shutdown::ShutdownListener) -> ServiceFuture<'_> {
        Box::pin(async move {
            let jobs = self
                .jobs
                .lock()
                .expect("job service mutex")
                .take()
                .ok_or_else(|| anyhow::anyhow!("job service {} already started", self.name))?;
            for scheduled in jobs {
                self.scheduler
                    .spawn(scheduled.job, scheduled.schedule)
                    .await?;
            }
            shutdown.wait().await;
            self.scheduler.shutdown().await;
            Ok(())
        })
    }

    fn stop(&self) -> ServiceFuture<'_> {
        Box::pin(async move {
            self.scheduler.shutdown().await;
            Ok(())
        })
    }
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
    use roze_service::ServiceGroup;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

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

    #[tokio::test]
    async fn job_service_runs_inside_service_group() {
        let hits = Arc::new(AtomicUsize::new(0));
        let mut jobs = JobService::new("jobs");
        jobs.add(
            boxed_job("tick", {
                let hits = hits.clone();
                move || {
                    let hits = hits.clone();
                    async move {
                        hits.fetch_add(1, Ordering::SeqCst);
                        Ok(())
                    }
                }
            }),
            Schedule::Every(Duration::from_millis(5)),
        );

        let mut group = ServiceGroup::new();
        let handle = group.handle();
        group.add(jobs);

        let join = tokio::spawn(group.start_with_shutdown(std::future::pending()));
        tokio::time::timeout(Duration::from_millis(100), async {
            while hits.load(Ordering::SeqCst) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("job should run");

        handle.shutdown();
        join.await
            .expect("service group should join")
            .expect("service group should stop cleanly");
    }
}
