use std::{
    future::Future,
    sync::{Arc, Mutex as StdMutex, OnceLock},
    time::{Duration, Instant},
};

use async_trait::async_trait;
use roze_resilience::{
    full_jitter_delay, BreakerDecision, BreakerRegistry, GovernancePolicy, RateLimitRegistry,
    RetryBudgetRegistry, SheddingRegistry,
};
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
    policy: Option<GovernancePolicy>,
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
                policy: None,
            });
        self
    }

    pub fn add_governed<J>(
        &mut self,
        job: J,
        schedule: Schedule,
        policy: GovernancePolicy,
    ) -> &mut Self
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
                policy: Some(policy),
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
            .push(ScheduledJob {
                job,
                schedule,
                policy: None,
            });
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
                    .spawn_with_governance(scheduled.job, scheduled.schedule, scheduled.policy)
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
        self.spawn_once_with_governance(job, None).await
    }

    pub async fn spawn_once_with_governance<J>(
        &self,
        job: J,
        policy: Option<GovernancePolicy>,
    ) -> anyhow::Result<()>
    where
        J: Job,
    {
        execute_governed_job(&job, policy.as_ref()).await
    }

    pub async fn spawn<J>(&self, job: J, schedule: Schedule) -> anyhow::Result<()>
    where
        J: Job,
    {
        self.spawn_with_governance(job, schedule, None).await
    }

    pub async fn spawn_with_governance<J>(
        &self,
        job: J,
        schedule: Schedule,
        policy: Option<GovernancePolicy>,
    ) -> anyhow::Result<()>
    where
        J: Job,
    {
        match schedule {
            Schedule::Once => self.spawn_once_with_governance(job, policy).await,
            Schedule::After(delay) => {
                let job = Arc::new(job);
                let handle = tokio::spawn(async move {
                    tokio::time::sleep(delay).await;
                    if let Err(err) = execute_governed_job(job.as_ref(), policy.as_ref()).await {
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
                        if let Err(err) = execute_governed_job(job.as_ref(), policy.as_ref()).await
                        {
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

async fn execute_governed_job(
    job: &dyn Job,
    policy: Option<&GovernancePolicy>,
) -> anyhow::Result<()> {
    static RATE_LIMITERS: OnceLock<RateLimitRegistry> = OnceLock::new();
    static BREAKERS: OnceLock<BreakerRegistry> = OnceLock::new();
    static SHEDDERS: OnceLock<SheddingRegistry> = OnceLock::new();
    static RETRY_BUDGETS: OnceLock<RetryBudgetRegistry> = OnceLock::new();

    let empty = GovernancePolicy::default();
    let policy = policy.unwrap_or(&empty);
    let name = job.name();
    let key = format!("job:{name}");
    if let Some(config) = policy.rate_limit {
        let allowed = RATE_LIMITERS
            .get_or_init(RateLimitRegistry::new)
            .allow(key.clone(), config);
        roze_metrics::record_resilience_decision(
            name,
            "job",
            "rate_limit",
            if allowed { "allowed" } else { "rejected" },
        );
        if !allowed {
            anyhow::bail!("job rejected by rate limit");
        }
    }

    let breaker_permit = if policy.breaker.is_some() {
        match BREAKERS
            .get_or_init(BreakerRegistry::new)
            .allow(key.clone())
        {
            BreakerDecision::Allow(permit) => Some(permit),
            BreakerDecision::Reject => {
                roze_metrics::record_resilience_decision(name, "job", "breaker", "open");
                anyhow::bail!("job rejected by open circuit breaker");
            }
        }
    } else {
        None
    };

    if let Some(config) = policy.shedding {
        if !SHEDDERS
            .get_or_init(SheddingRegistry::new)
            .allow(key.clone(), config)
        {
            if let (Some(config), Some(permit)) = (policy.breaker, breaker_permit) {
                BREAKERS
                    .get_or_init(BreakerRegistry::new)
                    .cancel(&key, permit, config);
            }
            roze_metrics::record_resilience_decision(name, "job", "load_shedding", "shed");
            anyhow::bail!("job rejected by load shedding");
        }
    }

    let started = Instant::now();
    let retry = policy.retry.unwrap_or_default();
    let budgets = RETRY_BUDGETS.get_or_init(RetryBudgetRegistry::default);
    budgets.record_call(&key);
    let mut attempt = 1;
    let result = loop {
        let result = match policy.timeout {
            Some(timeout) => tokio::time::timeout(timeout.max(Duration::from_millis(1)), job.run())
                .await
                .map_err(|_| anyhow::anyhow!("job timed out"))
                .and_then(|result| result),
            None => job.run().await,
        };
        if result.is_ok() || attempt >= retry.max_attempts.max(1) {
            break result;
        }
        if !budgets.allow_retry(&key, retry.budget_percent) {
            roze_metrics::record_resilience_decision(name, "job", "retry_budget", "exhausted");
            break result;
        }
        roze_metrics::record_resilience_decision(name, "job", "retry", "scheduled");
        tokio::time::sleep(full_jitter_delay(
            retry.backoff,
            retry.max_backoff,
            attempt as usize,
        ))
        .await;
        attempt += 1;
    };

    let success = result.is_ok();
    if let (Some(config), Some(permit)) = (policy.breaker, breaker_permit) {
        let breakers = BREAKERS.get_or_init(BreakerRegistry::new);
        if success {
            breakers.record_success(key.clone(), permit);
        } else {
            breakers.record_failure(key.clone(), permit, config);
        }
    }
    if let Some(config) = policy.shedding {
        SHEDDERS
            .get_or_init(SheddingRegistry::new)
            .record(key, success, started.elapsed(), config);
    }
    result
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

    struct FlakyJob {
        attempts: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl Job for FlakyJob {
        fn name(&self) -> &str {
            "flaky-governed-job"
        }

        async fn run(&self) -> anyhow::Result<()> {
            let attempt = self.attempts.fetch_add(1, Ordering::SeqCst) + 1;
            if attempt == 1 {
                anyhow::bail!("transient failure");
            }
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
    async fn governed_job_retries_within_budget() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let policy = GovernancePolicy {
            retry: Some(roze_resilience::RetryPolicy {
                max_attempts: 2,
                budget_percent: Some(100),
                ..roze_resilience::RetryPolicy::default()
            }),
            ..GovernancePolicy::default()
        };
        JobScheduler::new()
            .spawn_once_with_governance(
                FlakyJob {
                    attempts: attempts.clone(),
                },
                Some(policy),
            )
            .await
            .expect("second attempt succeeds");
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn governed_job_enforces_timeout() {
        let job = boxed_job("governed-timeout-job", || async {
            tokio::time::sleep(Duration::from_secs(1)).await;
            Ok(())
        });
        let policy = GovernancePolicy {
            timeout: Some(Duration::from_millis(1)),
            ..GovernancePolicy::default()
        };
        let error = JobScheduler::new()
            .spawn_once_with_governance(job, Some(policy))
            .await
            .expect_err("job must time out");
        assert!(error.to_string().contains("timed out"));
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
