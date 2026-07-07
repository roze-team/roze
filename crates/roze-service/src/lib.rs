use std::{
    collections::BTreeMap,
    future::Future,
    pin::Pin,
    sync::{
        atomic::{AtomicU8, AtomicUsize, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use anyhow::Context;
use roze_shutdown::{channel, ShutdownHandle, ShutdownListener};
use tokio::task::JoinSet;

#[derive(Debug, Clone)]
pub struct AppState<C> {
    name: Arc<str>,
    config: Arc<C>,
    started_at: Instant,
    metadata: BTreeMap<String, String>,
}

impl<C> AppState<C> {
    pub fn new(name: impl Into<Arc<str>>, config: C) -> Self {
        Self {
            name: name.into(),
            config: Arc::new(config),
            started_at: Instant::now(),
            metadata: BTreeMap::new(),
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn config(&self) -> &C {
        &self.config
    }

    pub fn started_at(&self) -> Instant {
        self.started_at
    }

    pub fn uptime(&self) -> std::time::Duration {
        self.started_at.elapsed()
    }

    pub fn metadata(&self) -> &BTreeMap<String, String> {
        &self.metadata
    }

    pub fn metadata_value(&self, key: impl AsRef<str>) -> Option<&str> {
        self.metadata.get(key.as_ref()).map(String::as_str)
    }

    pub fn with_metadata(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.metadata.insert(key.into(), value.into());
        self
    }
}

#[derive(Debug, Clone)]
pub struct ServiceBuilder<C> {
    name: Arc<str>,
    config: C,
    metadata: BTreeMap<String, String>,
}

impl<C> ServiceBuilder<C> {
    pub fn new(name: impl Into<Arc<str>>, config: C) -> Self {
        Self {
            name: name.into(),
            config,
            metadata: BTreeMap::new(),
        }
    }

    pub fn with_metadata(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.metadata.insert(key.into(), value.into());
        self
    }

    pub fn build(self) -> AppState<C> {
        AppState {
            name: self.name,
            config: Arc::new(self.config),
            started_at: Instant::now(),
            metadata: self.metadata,
        }
    }
}

pub fn service_state<C>(name: impl Into<Arc<str>>, config: C) -> AppState<C> {
    AppState::new(name, config)
}

pub fn service_builder<C>(name: impl Into<Arc<str>>, config: C) -> ServiceBuilder<C> {
    ServiceBuilder::new(name, config)
}

pub type ServiceFuture<'a> = Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'a>>;

pub trait RuntimeService: Send + Sync + 'static {
    fn name(&self) -> &str;

    fn start(&self, shutdown: ShutdownListener) -> ServiceFuture<'_>;

    fn stop(&self) -> ServiceFuture<'_> {
        Box::pin(async { Ok(()) })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecyclePhase {
    Starting,
    Running,
    Draining,
    Stopped,
    Failed,
}

const PHASE_STARTING: u8 = 0;
const PHASE_RUNNING: u8 = 1;
const PHASE_DRAINING: u8 = 2;
const PHASE_STOPPED: u8 = 3;
const PHASE_FAILED: u8 = 4;

#[derive(Debug, Clone)]
pub struct LifecycleState {
    phase: Arc<AtomicU8>,
}

impl Default for LifecycleState {
    fn default() -> Self {
        Self::new()
    }
}

impl LifecycleState {
    pub fn new() -> Self {
        Self {
            phase: Arc::new(AtomicU8::new(PHASE_STARTING)),
        }
    }

    pub fn phase(&self) -> LifecyclePhase {
        match self.phase.load(Ordering::SeqCst) {
            PHASE_RUNNING => LifecyclePhase::Running,
            PHASE_DRAINING => LifecyclePhase::Draining,
            PHASE_STOPPED => LifecyclePhase::Stopped,
            PHASE_FAILED => LifecyclePhase::Failed,
            _ => LifecyclePhase::Starting,
        }
    }

    pub fn is_running(&self) -> bool {
        matches!(self.phase(), LifecyclePhase::Running)
    }

    pub fn is_draining(&self) -> bool {
        matches!(self.phase(), LifecyclePhase::Draining)
    }

    pub fn is_terminal(&self) -> bool {
        matches!(
            self.phase(),
            LifecyclePhase::Stopped | LifecyclePhase::Failed
        )
    }

    pub async fn wait_for_phase(&self, phase: LifecyclePhase, timeout: Duration) -> bool {
        tokio::time::timeout(timeout, async {
            while self.phase() != phase {
                tokio::task::yield_now().await;
            }
        })
        .await
        .is_ok()
    }

    fn mark(&self, phase: LifecyclePhase) {
        self.phase.store(phase_code(phase), Ordering::SeqCst);
    }
}

fn phase_code(phase: LifecyclePhase) -> u8 {
    match phase {
        LifecyclePhase::Starting => PHASE_STARTING,
        LifecyclePhase::Running => PHASE_RUNNING,
        LifecyclePhase::Draining => PHASE_DRAINING,
        LifecyclePhase::Stopped => PHASE_STOPPED,
        LifecyclePhase::Failed => PHASE_FAILED,
    }
}

pub struct FnService<F> {
    name: Arc<str>,
    start: F,
}

impl<F> FnService<F> {
    pub fn new(name: impl Into<Arc<str>>, start: F) -> Self {
        Self {
            name: name.into(),
            start,
        }
    }
}

impl<F, Fut> RuntimeService for FnService<F>
where
    F: Fn(ShutdownListener) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = anyhow::Result<()>> + Send + 'static,
{
    fn name(&self) -> &str {
        &self.name
    }

    fn start(&self, shutdown: ShutdownListener) -> ServiceFuture<'_> {
        Box::pin((self.start)(shutdown))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceGroupConfig {
    pub shutdown_timeout: Duration,
    pub stop_on_first_error: bool,
}

impl Default for ServiceGroupConfig {
    fn default() -> Self {
        Self {
            shutdown_timeout: Duration::from_secs(30),
            stop_on_first_error: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceGroupSnapshot {
    pub phase: LifecyclePhase,
    pub service_count: usize,
    pub shutdown_timeout: Duration,
    pub stop_on_first_error: bool,
}

#[derive(Clone)]
pub struct ServiceGroupHandle {
    shutdown: ShutdownHandle,
    lifecycle: LifecycleState,
    config: ServiceGroupConfig,
    service_count: Arc<AtomicUsize>,
}

impl ServiceGroupHandle {
    pub fn shutdown(&self) {
        self.lifecycle.mark(LifecyclePhase::Draining);
        self.shutdown.trigger();
    }

    pub fn phase(&self) -> LifecyclePhase {
        self.lifecycle.phase()
    }

    pub fn lifecycle(&self) -> LifecycleState {
        self.lifecycle.clone()
    }

    pub fn snapshot(&self) -> ServiceGroupSnapshot {
        ServiceGroupSnapshot {
            phase: self.phase(),
            service_count: self.service_count.load(Ordering::SeqCst),
            shutdown_timeout: self.config.shutdown_timeout,
            stop_on_first_error: self.config.stop_on_first_error,
        }
    }
}

pub struct ServiceGroup {
    config: ServiceGroupConfig,
    services: Vec<Arc<dyn RuntimeService>>,
    service_count: Arc<AtomicUsize>,
    shutdown: ShutdownHandle,
    listener: ShutdownListener,
    lifecycle: LifecycleState,
}

impl Default for ServiceGroup {
    fn default() -> Self {
        Self::new()
    }
}

impl ServiceGroup {
    pub fn new() -> Self {
        Self::with_config(ServiceGroupConfig::default())
    }

    pub fn with_config(config: ServiceGroupConfig) -> Self {
        let (shutdown, listener) = channel();
        Self {
            config,
            services: Vec::new(),
            service_count: Arc::new(AtomicUsize::new(0)),
            shutdown,
            listener,
            lifecycle: LifecycleState::new(),
        }
    }

    pub fn add<S>(&mut self, service: S) -> &mut Self
    where
        S: RuntimeService,
    {
        self.services.push(Arc::new(service));
        self.service_count.fetch_add(1, Ordering::SeqCst);
        self
    }

    pub fn add_arc<S>(&mut self, service: Arc<S>) -> &mut Self
    where
        S: RuntimeService,
    {
        self.services.push(service);
        self.service_count.fetch_add(1, Ordering::SeqCst);
        self
    }

    pub fn add_fn<F, Fut>(&mut self, name: impl Into<Arc<str>>, start: F) -> &mut Self
    where
        F: Fn(ShutdownListener) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = anyhow::Result<()>> + Send + 'static,
    {
        self.add(FnService::new(name, start))
    }

    pub fn handle(&self) -> ServiceGroupHandle {
        ServiceGroupHandle {
            shutdown: self.shutdown.clone(),
            lifecycle: self.lifecycle.clone(),
            config: self.config.clone(),
            service_count: self.service_count.clone(),
        }
    }

    pub fn shutdown_listener(&self) -> ShutdownListener {
        self.listener.clone()
    }

    pub fn lifecycle(&self) -> LifecycleState {
        self.lifecycle.clone()
    }

    pub fn snapshot(&self) -> ServiceGroupSnapshot {
        ServiceGroupSnapshot {
            phase: self.lifecycle.phase(),
            service_count: self.service_count.load(Ordering::SeqCst),
            shutdown_timeout: self.config.shutdown_timeout,
            stop_on_first_error: self.config.stop_on_first_error,
        }
    }

    pub async fn start(self) -> anyhow::Result<()> {
        self.start_with_shutdown(roze_shutdown::listen_for_ctrl_c())
            .await
    }

    pub async fn start_with_shutdown<F>(self, shutdown: F) -> anyhow::Result<()>
    where
        F: Future<Output = ()> + Send,
    {
        if self.services.is_empty() {
            self.lifecycle.mark(LifecyclePhase::Stopped);
            return Ok(());
        }

        let mut tasks = spawn_services(&self.services, &self.listener);
        self.lifecycle.mark(LifecyclePhase::Running);
        let mut active = self.services.len();
        let mut errors = Vec::new();
        tokio::pin!(shutdown);

        while active > 0 {
            tokio::select! {
                _ = &mut shutdown => {
                    self.lifecycle.mark(LifecyclePhase::Draining);
                    self.shutdown.trigger();
                    break;
                }
                _ = self.listener.clone().wait() => {
                    self.lifecycle.mark(LifecyclePhase::Draining);
                    break;
                }
                joined = tasks.join_next() => {
                    let Some(joined) = joined else {
                        break;
                    };
                    active -= 1;
                    if handle_service_exit(joined, &mut errors) && self.config.stop_on_first_error {
                        self.lifecycle.mark(LifecyclePhase::Draining);
                        self.shutdown.trigger();
                        break;
                    }
                }
            }
        }

        if self.listener.is_triggered() || active > 0 {
            self.shutdown.trigger();
            stop_services(&self.services, self.config.shutdown_timeout, &mut errors).await;
            wait_for_tasks(
                &mut tasks,
                active,
                self.config.shutdown_timeout,
                &mut errors,
            )
            .await;
        }

        if errors.is_empty() {
            self.lifecycle.mark(LifecyclePhase::Stopped);
            Ok(())
        } else {
            self.lifecycle.mark(LifecyclePhase::Failed);
            Err(anyhow::anyhow!("{}", errors.join("; ")))
        }
    }
}

struct ServiceTaskExit {
    name: String,
    result: anyhow::Result<()>,
}

fn spawn_services(
    services: &[Arc<dyn RuntimeService>],
    shutdown: &ShutdownListener,
) -> JoinSet<ServiceTaskExit> {
    let mut tasks = JoinSet::new();
    for service in services {
        let service = Arc::clone(service);
        let listener = shutdown.clone();
        let name = service.name().to_string();
        tasks.spawn(async move {
            let result = service.start(listener).await;
            ServiceTaskExit { name, result }
        });
    }
    tasks
}

fn handle_service_exit(
    joined: Result<ServiceTaskExit, tokio::task::JoinError>,
    errors: &mut Vec<String>,
) -> bool {
    match joined {
        Ok(exit) => match exit.result {
            Ok(()) => false,
            Err(error) => {
                errors.push(format!("service {} failed: {error:#}", exit.name));
                true
            }
        },
        Err(error) => {
            errors.push(format!("service task failed: {error}"));
            true
        }
    }
}

async fn stop_services(
    services: &[Arc<dyn RuntimeService>],
    timeout: Duration,
    errors: &mut Vec<String>,
) {
    match tokio::time::timeout(timeout, run_stop_hooks(services)).await {
        Ok(stop_errors) => errors.extend(stop_errors),
        Err(_) => errors.push(format!(
            "service group stop hooks timed out after {timeout:?}"
        )),
    }
}

async fn run_stop_hooks(services: &[Arc<dyn RuntimeService>]) -> Vec<String> {
    let mut tasks = JoinSet::new();
    for service in services.iter().rev() {
        let service = Arc::clone(service);
        let name = service.name().to_string();
        tasks.spawn(async move {
            service
                .stop()
                .await
                .with_context(|| format!("service {name} stop hook failed"))
        });
    }

    let mut errors = Vec::new();
    while let Some(joined) = tasks.join_next().await {
        match joined {
            Ok(Ok(())) => {}
            Ok(Err(error)) => errors.push(format!("{error:#}")),
            Err(error) => errors.push(format!("service stop task failed: {error}")),
        }
    }
    errors
}

async fn wait_for_tasks(
    tasks: &mut JoinSet<ServiceTaskExit>,
    active: usize,
    timeout: Duration,
    errors: &mut Vec<String>,
) {
    let result = tokio::time::timeout(timeout, async {
        for _ in 0..active {
            match tasks.join_next().await {
                Some(joined) => {
                    handle_service_exit(joined, errors);
                }
                None => break,
            }
        }
    })
    .await;

    if result.is_err() {
        errors.push(format!(
            "service group tasks timed out after shutdown timeout {timeout:?}"
        ));
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    };

    use super::*;

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct Config(u32);

    #[test]
    fn builds_state() {
        let state = service_state("demo", Config(7)).with_metadata("env", "test");
        assert_eq!(state.name(), "demo");
        assert_eq!(state.config(), &Config(7));
        assert_eq!(state.metadata_value("env"), Some("test"));
        let _ = state.uptime();
    }

    #[tokio::test]
    async fn service_group_runs_function_service_until_shutdown() {
        let mut group = ServiceGroup::new();
        let started = Arc::new(AtomicBool::new(false));
        let handle = group.handle();

        group.add_fn("worker", {
            let started = started.clone();
            move |shutdown| {
                let started = started.clone();
                async move {
                    started.store(true, Ordering::SeqCst);
                    shutdown.wait().await;
                    Ok(())
                }
            }
        });

        let join = tokio::spawn(group.start_with_shutdown(std::future::pending()));
        tokio::time::timeout(Duration::from_millis(50), async {
            while !started.load(Ordering::SeqCst) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("service should start");

        handle.shutdown();
        join.await
            .expect("group task should join")
            .expect("group should stop cleanly");
        assert_eq!(handle.phase(), LifecyclePhase::Stopped);
    }

    #[tokio::test]
    async fn service_group_runs_stop_hooks_on_external_shutdown() {
        struct Stoppable {
            stopped: Arc<AtomicBool>,
        }

        impl RuntimeService for Stoppable {
            fn name(&self) -> &str {
                "stoppable"
            }

            fn start(&self, shutdown: ShutdownListener) -> ServiceFuture<'_> {
                Box::pin(async move {
                    shutdown.wait().await;
                    Ok(())
                })
            }

            fn stop(&self) -> ServiceFuture<'_> {
                Box::pin(async move {
                    self.stopped.store(true, Ordering::SeqCst);
                    Ok(())
                })
            }
        }

        let stopped = Arc::new(AtomicBool::new(false));
        let mut group = ServiceGroup::new();
        let handle = group.handle();
        group.add(Stoppable {
            stopped: stopped.clone(),
        });

        group
            .start_with_shutdown(async {})
            .await
            .expect("shutdown should be clean");
        assert!(stopped.load(Ordering::SeqCst));
        assert_eq!(handle.phase(), LifecyclePhase::Stopped);
    }

    #[tokio::test]
    async fn service_group_stops_peers_after_service_error() {
        let mut group = ServiceGroup::with_config(ServiceGroupConfig {
            shutdown_timeout: Duration::from_millis(100),
            stop_on_first_error: true,
        });
        let handle = group.handle();
        let peer_stopped = Arc::new(AtomicUsize::new(0));

        group.add_fn("failing", |_| async { Err(anyhow::anyhow!("boom")) });
        group.add_fn("peer", {
            let peer_stopped = peer_stopped.clone();
            move |shutdown| {
                let peer_stopped = peer_stopped.clone();
                async move {
                    shutdown.wait().await;
                    peer_stopped.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                }
            }
        });

        let error = group
            .start_with_shutdown(std::future::pending())
            .await
            .expect_err("failing service should fail group");

        assert!(error.to_string().contains("service failing failed"));
        assert_eq!(peer_stopped.load(Ordering::SeqCst), 1);
        assert_eq!(handle.phase(), LifecyclePhase::Failed);
    }

    #[tokio::test]
    async fn service_group_exposes_running_and_draining_phases() {
        let mut group = ServiceGroup::new();
        let handle = group.handle();
        let observed_draining = Arc::new(AtomicBool::new(false));

        group.add_fn("worker", {
            let handle = handle.clone();
            let observed_draining = observed_draining.clone();
            move |shutdown| {
                let handle = handle.clone();
                let observed_draining = observed_draining.clone();
                async move {
                    while handle.phase() != LifecyclePhase::Running {
                        tokio::task::yield_now().await;
                    }
                    shutdown.wait().await;
                    observed_draining
                        .store(handle.phase() == LifecyclePhase::Draining, Ordering::SeqCst);
                    Ok(())
                }
            }
        });

        let join = tokio::spawn(group.start_with_shutdown(std::future::pending()));
        tokio::time::timeout(Duration::from_millis(50), async {
            while handle.phase() != LifecyclePhase::Running {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("service group should enter running phase");

        handle.shutdown();
        join.await
            .expect("group task should join")
            .expect("group should stop cleanly");

        assert!(observed_draining.load(Ordering::SeqCst));
        assert_eq!(handle.phase(), LifecyclePhase::Stopped);
    }

    #[tokio::test]
    async fn lifecycle_wait_for_phase_observes_transition() {
        let lifecycle = LifecycleState::new();
        let cloned = lifecycle.clone();
        tokio::spawn(async move {
            tokio::task::yield_now().await;
            cloned.mark(LifecyclePhase::Running);
        });

        assert!(
            lifecycle
                .wait_for_phase(LifecyclePhase::Running, Duration::from_millis(50))
                .await
        );
    }

    #[tokio::test]
    async fn service_group_snapshot_tracks_phase_and_service_count() {
        let mut group = ServiceGroup::with_config(ServiceGroupConfig {
            shutdown_timeout: Duration::from_millis(75),
            stop_on_first_error: false,
        });
        let handle = group.handle();

        assert_eq!(
            group.snapshot(),
            ServiceGroupSnapshot {
                phase: LifecyclePhase::Starting,
                service_count: 0,
                shutdown_timeout: Duration::from_millis(75),
                stop_on_first_error: false,
            }
        );

        group.add_fn("worker", |shutdown| async move {
            shutdown.wait().await;
            Ok(())
        });
        assert_eq!(handle.snapshot().service_count, 1);

        let join = tokio::spawn(group.start_with_shutdown(std::future::pending()));
        assert!(
            handle
                .lifecycle()
                .wait_for_phase(LifecyclePhase::Running, Duration::from_millis(50))
                .await
        );
        assert_eq!(handle.snapshot().phase, LifecyclePhase::Running);
        assert_eq!(
            handle.snapshot().shutdown_timeout,
            Duration::from_millis(75)
        );
        assert!(!handle.snapshot().stop_on_first_error);

        handle.shutdown();
        join.await
            .expect("service group task should join")
            .expect("service group should stop cleanly");
        assert_eq!(handle.snapshot().phase, LifecyclePhase::Stopped);
    }

    #[tokio::test]
    #[ignore = "production-soak: set ROZE_LIFECYCLE_SOAK_SECONDS/ROZE_LIFECYCLE_SOAK_CYCLES for long runs"]
    async fn production_soak_service_group_lifecycle() {
        let seconds = std::env::var("ROZE_LIFECYCLE_SOAK_SECONDS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(300);
        let max_cycles = std::env::var("ROZE_LIFECYCLE_SOAK_CYCLES")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(u64::MAX);
        let deadline = Instant::now() + Duration::from_secs(seconds);
        let mut cycles = 0_u64;
        let mut worker_exits = 0_u64;
        let mut stop_hooks = 0_u64;
        let mut running_snapshots = 0_u64;
        let mut stopped_snapshots = 0_u64;
        let mut max_service_count = 0_usize;

        while Instant::now() < deadline && cycles < max_cycles {
            let mut group = ServiceGroup::with_config(ServiceGroupConfig {
                shutdown_timeout: Duration::from_millis(250),
                stop_on_first_error: true,
            });
            let handle = group.handle();
            let exits = Arc::new(AtomicUsize::new(0));
            let stops = Arc::new(AtomicUsize::new(0));

            for index in 0..4 {
                struct SoakService {
                    name: String,
                    exits: Arc<AtomicUsize>,
                    stops: Arc<AtomicUsize>,
                }

                impl RuntimeService for SoakService {
                    fn name(&self) -> &str {
                        &self.name
                    }

                    fn start(&self, shutdown: ShutdownListener) -> ServiceFuture<'_> {
                        Box::pin(async move {
                            shutdown.wait().await;
                            self.exits.fetch_add(1, Ordering::SeqCst);
                            Ok(())
                        })
                    }

                    fn stop(&self) -> ServiceFuture<'_> {
                        Box::pin(async move {
                            self.stops.fetch_add(1, Ordering::SeqCst);
                            Ok(())
                        })
                    }
                }

                group.add(SoakService {
                    name: format!("worker-{index}"),
                    exits: exits.clone(),
                    stops: stops.clone(),
                });
            }

            let lifecycle = handle.lifecycle();
            let join = tokio::spawn(group.start_with_shutdown(std::future::pending()));
            assert!(
                lifecycle
                    .wait_for_phase(LifecyclePhase::Running, Duration::from_secs(1))
                    .await,
                "service group did not enter running phase"
            );
            let running_snapshot = handle.snapshot();
            assert_eq!(running_snapshot.phase, LifecyclePhase::Running);
            assert_eq!(running_snapshot.service_count, 4);
            assert_eq!(
                running_snapshot.shutdown_timeout,
                Duration::from_millis(250)
            );
            assert!(running_snapshot.stop_on_first_error);
            running_snapshots += 1;
            max_service_count = max_service_count.max(running_snapshot.service_count);

            handle.shutdown();
            join.await
                .expect("service group task should join")
                .expect("service group should stop cleanly");

            let stopped_snapshot = handle.snapshot();
            assert_eq!(stopped_snapshot.phase, LifecyclePhase::Stopped);
            assert_eq!(stopped_snapshot.service_count, 4);
            assert_eq!(exits.load(Ordering::SeqCst), 4);
            assert_eq!(stops.load(Ordering::SeqCst), 4);
            stopped_snapshots += 1;

            cycles += 1;
            worker_exits += exits.load(Ordering::SeqCst) as u64;
            stop_hooks += stops.load(Ordering::SeqCst) as u64;
        }

        println!(
            "roze_lifecycle_soak cycles={cycles} worker_exits={worker_exits} stop_hooks={stop_hooks} running_snapshots={running_snapshots} stopped_snapshots={stopped_snapshots} max_service_count={max_service_count}"
        );
        assert!(cycles > 0, "soak must run at least one lifecycle cycle");
    }
}
