use std::{
    any::{type_name, Any, TypeId},
    collections::{BTreeMap, HashMap},
    fmt,
    future::Future,
    pin::Pin,
    sync::{
        atomic::{AtomicU8, AtomicUsize, Ordering},
        Arc, Mutex, RwLock,
    },
    time::{Duration, Instant},
};

use anyhow::Context;
use roze_shutdown::{channel, ShutdownHandle, ShutdownListener};
use tokio::task::JoinSet;

type ExtensionValue = Arc<dyn Any + Send + Sync>;

/// Cloneable, type-safe storage for application resources bound to a service context.
///
/// Clones share the same values. Inserting through one clone is immediately visible
/// through every other clone, which makes the store suitable for generated
/// `ServiceContext` values shared by REST handlers, RPC methods, and background tasks.
#[derive(Clone, Default)]
pub struct ApplicationExtensions {
    values: Arc<RwLock<HashMap<TypeId, ExtensionValue>>>,
}

impl fmt::Debug for ApplicationExtensions {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ApplicationExtensions")
            .field("len", &self.len())
            .finish_non_exhaustive()
    }
}

impl ApplicationExtensions {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert<T>(&self, value: T) -> Option<Arc<T>>
    where
        T: Send + Sync + 'static,
    {
        self.insert_arc(Arc::new(value))
    }

    pub fn insert_arc<T>(&self, value: Arc<T>) -> Option<Arc<T>>
    where
        T: Send + Sync + 'static,
    {
        self.values
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(TypeId::of::<T>(), value)
            .and_then(|previous| previous.downcast::<T>().ok())
    }

    pub fn get<T>(&self) -> Option<Arc<T>>
    where
        T: Send + Sync + 'static,
    {
        self.values
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(&TypeId::of::<T>())
            .cloned()
            .and_then(|value| value.downcast::<T>().ok())
    }

    pub fn require<T>(&self) -> anyhow::Result<Arc<T>>
    where
        T: Send + Sync + 'static,
    {
        self.get::<T>().ok_or_else(|| {
            anyhow::anyhow!(
                "application extension `{}` is not configured",
                type_name::<T>()
            )
        })
    }

    pub fn contains<T>(&self) -> bool
    where
        T: Send + Sync + 'static,
    {
        self.values
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .contains_key(&TypeId::of::<T>())
    }

    pub fn remove<T>(&self) -> Option<Arc<T>>
    where
        T: Send + Sync + 'static,
    {
        self.values
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&TypeId::of::<T>())
            .and_then(|value| value.downcast::<T>().ok())
    }

    pub fn len(&self) -> usize {
        self.values
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

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

    fn order(&self) -> i32 {
        0
    }

    fn start(&self, shutdown: ShutdownListener) -> ServiceFuture<'_>;

    fn ready(&self) -> ServiceFuture<'_> {
        Box::pin(async { Ok(()) })
    }

    fn drain(&self) -> ServiceFuture<'_> {
        Box::pin(async { Ok(()) })
    }

    fn stop(&self) -> ServiceFuture<'_> {
        Box::pin(async { Ok(()) })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecyclePhase {
    Starting,
    Ready,
    Draining,
    Stopped,
    Failed,
}

const PHASE_STARTING: u8 = 0;
const PHASE_READY: u8 = 1;
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
        phase_from_code(self.phase.load(Ordering::SeqCst))
    }

    fn mark(&self, phase: LifecyclePhase) {
        let previous = phase_from_code(self.phase.swap(phase_code(phase), Ordering::SeqCst));
        if previous != phase {
            tracing::debug!(from = ?previous, to = ?phase, "service group lifecycle changed");
        }
    }
}

fn phase_from_code(code: u8) -> LifecyclePhase {
    match code {
        PHASE_READY => LifecyclePhase::Ready,
        PHASE_DRAINING => LifecyclePhase::Draining,
        PHASE_STOPPED => LifecyclePhase::Stopped,
        PHASE_FAILED => LifecyclePhase::Failed,
        _ => LifecyclePhase::Starting,
    }
}

impl LifecycleState {
    pub fn is_ready(&self) -> bool {
        matches!(self.phase(), LifecyclePhase::Ready)
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
}

fn phase_code(phase: LifecyclePhase) -> u8 {
    match phase {
        LifecyclePhase::Starting => PHASE_STARTING,
        LifecyclePhase::Ready => PHASE_READY,
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
    pub startup_timeout: Duration,
    pub drain_timeout: Duration,
    pub shutdown_timeout: Duration,
    pub stop_on_first_error: bool,
}

impl Default for ServiceGroupConfig {
    fn default() -> Self {
        Self {
            startup_timeout: Duration::from_secs(30),
            drain_timeout: Duration::from_secs(30),
            shutdown_timeout: Duration::from_secs(30),
            stop_on_first_error: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceGroupSnapshot {
    pub phase: LifecyclePhase,
    pub service_count: usize,
    pub startup_timeout: Duration,
    pub drain_timeout: Duration,
    pub shutdown_timeout: Duration,
    pub stop_on_first_error: bool,
    pub failure: Option<String>,
}

#[derive(Clone)]
pub struct ServiceGroupHandle {
    shutdown_request: ShutdownHandle,
    lifecycle: LifecycleState,
    config: ServiceGroupConfig,
    service_count: Arc<AtomicUsize>,
    failure: Arc<Mutex<Option<String>>>,
}

impl ServiceGroupHandle {
    pub fn shutdown(&self) {
        tracing::debug!(
            service_count = self.service_count.load(Ordering::SeqCst),
            "service group shutdown requested"
        );
        self.lifecycle.mark(LifecyclePhase::Draining);
        self.shutdown_request.trigger();
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
            startup_timeout: self.config.startup_timeout,
            drain_timeout: self.config.drain_timeout,
            shutdown_timeout: self.config.shutdown_timeout,
            stop_on_first_error: self.config.stop_on_first_error,
            failure: self.failure.lock().expect("service failure lock").clone(),
        }
    }
}

pub struct ServiceGroup {
    config: ServiceGroupConfig,
    services: Vec<Arc<dyn RuntimeService>>,
    service_count: Arc<AtomicUsize>,
    shutdown_request: ShutdownHandle,
    shutdown_request_listener: ShutdownListener,
    service_shutdown: ShutdownHandle,
    service_shutdown_listener: ShutdownListener,
    lifecycle: LifecycleState,
    failure: Arc<Mutex<Option<String>>>,
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
        let (shutdown_request, shutdown_request_listener) = channel();
        let (service_shutdown, service_shutdown_listener) = channel();
        Self {
            config,
            services: Vec::new(),
            service_count: Arc::new(AtomicUsize::new(0)),
            shutdown_request,
            shutdown_request_listener,
            service_shutdown,
            service_shutdown_listener,
            lifecycle: LifecycleState::new(),
            failure: Arc::new(Mutex::new(None)),
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
            shutdown_request: self.shutdown_request.clone(),
            lifecycle: self.lifecycle.clone(),
            config: self.config.clone(),
            service_count: self.service_count.clone(),
            failure: self.failure.clone(),
        }
    }

    pub fn shutdown_listener(&self) -> ShutdownListener {
        self.service_shutdown_listener.clone()
    }

    pub fn lifecycle(&self) -> LifecycleState {
        self.lifecycle.clone()
    }

    pub fn snapshot(&self) -> ServiceGroupSnapshot {
        ServiceGroupSnapshot {
            phase: self.lifecycle.phase(),
            service_count: self.service_count.load(Ordering::SeqCst),
            startup_timeout: self.config.startup_timeout,
            drain_timeout: self.config.drain_timeout,
            shutdown_timeout: self.config.shutdown_timeout,
            stop_on_first_error: self.config.stop_on_first_error,
            failure: self.failure.lock().expect("service failure lock").clone(),
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
        let service_names = self
            .services
            .iter()
            .map(|service| service.name())
            .collect::<Vec<_>>();
        tracing::debug!(
            service_count = service_names.len(),
            services = ?service_names,
            shutdown_timeout_ms = self.config.shutdown_timeout.as_millis(),
            startup_timeout_ms = self.config.startup_timeout.as_millis(),
            drain_timeout_ms = self.config.drain_timeout.as_millis(),
            stop_on_first_error = self.config.stop_on_first_error,
            "service group starting"
        );
        if self.services.is_empty() {
            self.lifecycle.mark(LifecyclePhase::Stopped);
            return Ok(());
        }

        let mut tasks = spawn_services(&self.services, &self.service_shutdown_listener);
        let mut errors = run_lifecycle_hooks(
            &self.services,
            LifecycleHook::Ready,
            self.config.startup_timeout,
        )
        .await;
        if !errors.is_empty() {
            self.lifecycle.mark(LifecyclePhase::Failed);
            self.service_shutdown.trigger();
            wait_for_tasks(
                &mut tasks,
                self.services.len(),
                self.config.shutdown_timeout,
                &mut errors,
            )
            .await;
            let failure = errors.join("; ");
            *self.failure.lock().expect("service failure lock") = Some(failure.clone());
            return Err(anyhow::anyhow!(failure));
        }
        self.lifecycle.mark(LifecyclePhase::Ready);
        let mut active = self.services.len();
        tokio::pin!(shutdown);

        while active > 0 {
            tokio::select! {
                _ = &mut shutdown => {
                    tracing::debug!(source = "external_signal", "service group draining");
                    self.lifecycle.mark(LifecyclePhase::Draining);
                    break;
                }
                _ = self.shutdown_request_listener.clone().wait() => {
                    tracing::debug!(source = "shutdown_handle", "service group draining");
                    self.lifecycle.mark(LifecyclePhase::Draining);
                    break;
                }
                joined = tasks.join_next() => {
                    let Some(joined) = joined else {
                        break;
                    };
                    active -= 1;
                    if handle_service_exit(joined, &mut errors) && self.config.stop_on_first_error {
                        tracing::debug!(source = "service_error", "service group draining");
                        self.lifecycle.mark(LifecyclePhase::Draining);
                        self.shutdown_request.trigger();
                        break;
                    }
                }
            }
        }

        if self.shutdown_request_listener.is_triggered() || active > 0 {
            errors.extend(
                run_lifecycle_hooks(
                    &self.services,
                    LifecycleHook::Drain,
                    self.config.drain_timeout,
                )
                .await,
            );
            self.service_shutdown.trigger();
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
            let failure = errors.join("; ");
            *self.failure.lock().expect("service failure lock") = Some(failure.clone());
            Err(anyhow::anyhow!(failure))
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum LifecycleHook {
    Ready,
    Drain,
}

async fn run_lifecycle_hooks(
    services: &[Arc<dyn RuntimeService>],
    hook: LifecycleHook,
    timeout: Duration,
) -> Vec<String> {
    let future = async {
        let mut errors = Vec::new();
        for service in ordered_services(services, matches!(hook, LifecycleHook::Drain)) {
            let result = match hook {
                LifecycleHook::Ready => service.ready().await,
                LifecycleHook::Drain => service.drain().await,
            };
            if let Err(error) = result {
                errors.push(format!(
                    "service {} {hook:?} hook failed: {error:#}",
                    service.name()
                ));
            }
        }
        errors
    };
    match tokio::time::timeout(timeout, future).await {
        Ok(errors) => errors,
        Err(_) => vec![format!(
            "service group {hook:?} hooks timed out after {timeout:?}"
        )],
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
    for service in ordered_services(services, false) {
        let service = Arc::clone(service);
        let listener = shutdown.clone();
        let name = service.name().to_string();
        tracing::debug!(service = %name, "service task spawning");
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
            Ok(()) => {
                tracing::debug!(service = %exit.name, outcome = "completed", "service task exited");
                false
            }
            Err(error) => {
                tracing::debug!(service = %exit.name, outcome = "failed", "service task exited");
                errors.push(format!("service {} failed: {error:#}", exit.name));
                true
            }
        },
        Err(error) => {
            tracing::debug!(outcome = "join_failed", "service task exited");
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
    let mut errors = Vec::new();
    for service in ordered_services(services, true) {
        let name = service.name().to_string();
        tracing::debug!(service = %name, "service stop hook starting");
        if let Err(error) = service
            .stop()
            .await
            .with_context(|| format!("service {name} stop hook failed"))
        {
            errors.push(format!("{error:#}"));
        }
    }
    errors
}

fn ordered_services(
    services: &[Arc<dyn RuntimeService>],
    reverse: bool,
) -> Vec<&Arc<dyn RuntimeService>> {
    let mut ordered = services.iter().collect::<Vec<_>>();
    ordered.sort_by_key(|service| service.order());
    if reverse {
        ordered.reverse();
    }
    ordered
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
    fn application_extensions_are_typed_and_shared_across_clones() {
        let extensions = ApplicationExtensions::new();
        let clone = extensions.clone();

        assert!(extensions.insert(String::from("captcha")).is_none());
        assert_eq!(clone.require::<String>().unwrap().as_str(), "captcha");
        assert!(clone.contains::<String>());
        assert_eq!(extensions.len(), 1);

        let previous = clone.insert(String::from("merchant")).unwrap();
        assert_eq!(previous.as_str(), "captcha");
        assert_eq!(extensions.get::<String>().unwrap().as_str(), "merchant");
        assert_eq!(extensions.remove::<String>().unwrap().as_str(), "merchant");
        assert!(clone.is_empty());
        assert!(extensions.require::<String>().is_err());
    }

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
            ..ServiceGroupConfig::default()
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
    async fn service_group_exposes_ready_and_draining_phases() {
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
                    while handle.phase() != LifecyclePhase::Ready {
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
            while handle.phase() != LifecyclePhase::Ready {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("service group should enter ready phase");

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
            cloned.mark(LifecyclePhase::Ready);
        });

        assert!(
            lifecycle
                .wait_for_phase(LifecyclePhase::Ready, Duration::from_millis(50))
                .await
        );
    }

    #[tokio::test]
    async fn service_group_snapshot_tracks_phase_and_service_count() {
        let mut group = ServiceGroup::with_config(ServiceGroupConfig {
            shutdown_timeout: Duration::from_millis(75),
            stop_on_first_error: false,
            ..ServiceGroupConfig::default()
        });
        let handle = group.handle();

        assert_eq!(
            group.snapshot(),
            ServiceGroupSnapshot {
                phase: LifecyclePhase::Starting,
                service_count: 0,
                startup_timeout: Duration::from_secs(30),
                drain_timeout: Duration::from_secs(30),
                shutdown_timeout: Duration::from_millis(75),
                stop_on_first_error: false,
                failure: None,
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
                .wait_for_phase(LifecyclePhase::Ready, Duration::from_millis(50))
                .await
        );
        assert_eq!(handle.snapshot().phase, LifecyclePhase::Ready);
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
        let started = Instant::now();
        let deadline = started + Duration::from_secs(seconds);
        let mut cycles = 0_u64;
        let mut worker_exits = 0_u64;
        let mut stop_hooks = 0_u64;
        let mut running_snapshots = 0_u64;
        let mut stopped_snapshots = 0_u64;
        let mut max_service_count = 0_usize;
        let mut cycle_latency = roze_metrics::LatencyHistogram::new();
        let mut fault_detection_latency = roze_metrics::LatencyHistogram::new();
        let mut failed_task_detections = 0_u64;
        let mut drain_timeout_detections = 0_u64;

        struct TimeoutDrainService;

        impl RuntimeService for TimeoutDrainService {
            fn name(&self) -> &str {
                "timeout-drain"
            }

            fn start(&self, shutdown: ShutdownListener) -> ServiceFuture<'_> {
                Box::pin(async move {
                    shutdown.wait().await;
                    Ok(())
                })
            }

            fn drain(&self) -> ServiceFuture<'_> {
                Box::pin(std::future::pending())
            }
        }

        while Instant::now() < deadline && cycles < max_cycles {
            let cycle_started = Instant::now();
            let mut group = ServiceGroup::with_config(ServiceGroupConfig {
                shutdown_timeout: Duration::from_millis(250),
                stop_on_first_error: true,
                ..ServiceGroupConfig::default()
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
                    .wait_for_phase(LifecyclePhase::Ready, Duration::from_secs(1))
                    .await,
                "service group did not enter ready phase"
            );
            let running_snapshot = handle.snapshot();
            assert_eq!(running_snapshot.phase, LifecyclePhase::Ready);
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
            cycle_latency.observe(cycle_started.elapsed());

            if cycles == 1 || cycles.is_multiple_of(128) {
                let fault_started = Instant::now();
                let mut failed_group = ServiceGroup::with_config(ServiceGroupConfig {
                    shutdown_timeout: Duration::from_millis(50),
                    stop_on_first_error: true,
                    ..ServiceGroupConfig::default()
                });
                let failed_handle = failed_group.handle();
                failed_group.add_fn("failing", |_| async {
                    Err(anyhow::anyhow!("injected lifecycle failure"))
                });
                failed_group.add_fn("peer", |shutdown| async move {
                    shutdown.wait().await;
                    Ok(())
                });
                let error = failed_group
                    .start_with_shutdown(std::future::pending())
                    .await
                    .expect_err("injected service failure must fail the group");
                assert!(error.to_string().contains("injected lifecycle failure"));
                assert_eq!(failed_handle.phase(), LifecyclePhase::Failed);
                failed_task_detections += 1;
                fault_detection_latency.observe(fault_started.elapsed());
            }

            if cycles == 1 || cycles.is_multiple_of(256) {
                let fault_started = Instant::now();
                let mut timeout_group = ServiceGroup::with_config(ServiceGroupConfig {
                    drain_timeout: Duration::from_millis(1),
                    shutdown_timeout: Duration::from_millis(50),
                    stop_on_first_error: true,
                    ..ServiceGroupConfig::default()
                });
                let timeout_handle = timeout_group.handle();
                timeout_group.add(TimeoutDrainService);
                let timeout_lifecycle = timeout_handle.lifecycle();
                let timeout_join =
                    tokio::spawn(timeout_group.start_with_shutdown(std::future::pending()));
                assert!(
                    timeout_lifecycle
                        .wait_for_phase(LifecyclePhase::Ready, Duration::from_secs(1))
                        .await,
                    "timeout fault group did not enter ready phase"
                );
                timeout_handle.shutdown();
                let error = timeout_join
                    .await
                    .expect("timeout fault group task should join")
                    .expect_err("drain timeout must fail the group");
                assert!(error.to_string().contains("Drain hooks timed out"));
                assert_eq!(timeout_handle.phase(), LifecyclePhase::Failed);
                drain_timeout_detections += 1;
                fault_detection_latency.observe(fault_started.elapsed());
            }
        }

        let elapsed_ms = started.elapsed().as_millis().max(1);
        let cycles_per_second_milli = u128::from(cycles).saturating_mul(1_000_000) / elapsed_ms;
        let p50_cycle_us = cycle_latency
            .percentile_upper_bound_micros(50)
            .expect("cycle latency");
        let p95_cycle_us = cycle_latency
            .percentile_upper_bound_micros(95)
            .expect("cycle latency");
        let p99_cycle_us = cycle_latency
            .percentile_upper_bound_micros(99)
            .expect("cycle latency");
        let p99_fault_detection_us = fault_detection_latency
            .percentile_upper_bound_micros(99)
            .expect("fault detection latency");
        println!(
            "roze_lifecycle_soak elapsed_ms={elapsed_ms} cycles={cycles} cycles_per_second_milli={cycles_per_second_milli} p50_cycle_us={p50_cycle_us} p95_cycle_us={p95_cycle_us} p99_cycle_us={p99_cycle_us} failed_task_detections={failed_task_detections} drain_timeout_detections={drain_timeout_detections} p99_fault_detection_us={p99_fault_detection_us} worker_exits={worker_exits} stop_hooks={stop_hooks} running_snapshots={running_snapshots} stopped_snapshots={stopped_snapshots} max_service_count={max_service_count}"
        );
        assert!(cycles > 0, "soak must run at least one lifecycle cycle");
        assert_eq!(cycle_latency.count(), cycles);
        assert!(failed_task_detections > 0);
        assert!(drain_timeout_detections > 0);
        assert_eq!(
            fault_detection_latency.count(),
            failed_task_detections + drain_timeout_detections
        );
    }
}
