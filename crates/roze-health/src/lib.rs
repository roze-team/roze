use std::{
    fmt::{self, Display},
    future::Future,
    panic::AssertUnwindSafe,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, RwLock,
    },
    time::Duration,
};

use futures_util::{future::join_all, FutureExt};
use serde::{Deserialize, Serialize};

const DEFAULT_CHECK_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HealthStatus {
    Healthy,
    Degraded,
    Unhealthy,
}

impl HealthStatus {
    pub fn is_ready(self) -> bool {
        matches!(self, HealthStatus::Healthy)
    }

    pub fn is_alive(self) -> bool {
        !matches!(self, HealthStatus::Unhealthy)
    }
}

impl Display for HealthStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HealthStatus::Healthy => f.write_str("healthy"),
            HealthStatus::Degraded => f.write_str("degraded"),
            HealthStatus::Unhealthy => f.write_str("unhealthy"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthCheck {
    pub name: String,
    pub status: HealthStatus,
    pub message: Option<String>,
}

type CheckFuture = Pin<Box<dyn Future<Output = HealthCheck> + Send>>;
type CheckFn = Arc<dyn Fn() -> CheckFuture + Send + Sync>;

#[derive(Clone)]
struct RegisteredCheck {
    name: String,
    check: CheckFn,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServicePhase {
    Starting,
    Ready,
    Draining,
}

impl Display for ServicePhase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ServicePhase::Starting => f.write_str("starting"),
            ServicePhase::Ready => f.write_str("ready"),
            ServicePhase::Draining => f.write_str("draining"),
        }
    }
}

#[derive(Clone)]
pub struct HealthRegistry {
    checks: Arc<RwLock<Vec<RegisteredCheck>>>,
    startup_complete: Arc<AtomicBool>,
    draining: Arc<AtomicBool>,
    check_timeout: Duration,
}

impl Default for HealthRegistry {
    fn default() -> Self {
        Self {
            checks: Arc::default(),
            startup_complete: Arc::default(),
            draining: Arc::default(),
            check_timeout: DEFAULT_CHECK_TIMEOUT,
        }
    }
}

impl fmt::Debug for HealthRegistry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HealthRegistry")
            .field("phase", &self.phase())
            .field(
                "checks",
                &self
                    .checks
                    .read()
                    .map(|checks| checks.len())
                    .unwrap_or_default(),
            )
            .finish()
    }
}

impl HealthRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_check_timeout(mut self, timeout: Duration) -> Self {
        assert!(!timeout.is_zero(), "health check timeout must be positive");
        self.check_timeout = timeout;
        self
    }

    pub fn check_timeout(&self) -> Duration {
        self.check_timeout
    }

    pub fn mark_started(&self) {
        let previous = self.phase();
        self.startup_complete.store(true, Ordering::SeqCst);
        self.log_phase_change(previous);
    }

    pub fn mark_draining(&self) {
        let previous = self.phase();
        self.draining.store(true, Ordering::SeqCst);
        self.log_phase_change(previous);
    }

    pub fn mark_ready(&self) {
        let previous = self.phase();
        self.startup_complete.store(true, Ordering::SeqCst);
        self.draining.store(false, Ordering::SeqCst);
        self.log_phase_change(previous);
    }

    pub fn phase(&self) -> ServicePhase {
        if self.draining.load(Ordering::SeqCst) {
            ServicePhase::Draining
        } else if self.startup_complete.load(Ordering::SeqCst) {
            ServicePhase::Ready
        } else {
            ServicePhase::Starting
        }
    }

    pub fn register_static(&self, check: HealthCheck) {
        self.register_check(check.name.clone(), move || {
            let check = check.clone();
            async move { check }
        });
    }

    pub fn register_dependency<F, Fut>(&self, name: impl Into<String>, check: F)
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = anyhow::Result<()>> + Send + 'static,
    {
        let name = name.into();
        let report_name = name.clone();
        self.register_check(name, move || {
            let fut = check();
            let report_name = report_name.clone();
            async move {
                match fut.await {
                    Ok(()) => HealthCheck::healthy(report_name),
                    Err(err) => HealthCheck::unhealthy(report_name, err.to_string()),
                }
            }
        });
    }

    pub fn register_check<F, Fut>(&self, name: impl Into<String>, check: F)
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = HealthCheck> + Send + 'static,
    {
        let name = name.into();
        let registered = RegisteredCheck {
            name: name.clone(),
            check: Arc::new(move || Box::pin(check())),
        };
        let mut checks = self.checks.write().expect("health registry lock poisoned");
        checks.push(registered);
        tracing::debug!(check = %name, check_count = checks.len(), "health check registered");
    }

    pub async fn liveness_report(&self) -> HealthReport {
        let mut checks = Vec::new();
        checks.push(HealthCheck::healthy("process"));
        if self.draining.load(Ordering::SeqCst) {
            checks.push(HealthCheck::degraded(
                "phase",
                ServicePhase::Draining.to_string(),
            ));
        }
        HealthReport::new(checks)
    }

    pub async fn readiness_report(&self) -> HealthReport {
        let mut checks = self.phase_checks();
        checks.extend(self.run_registered_checks().await);
        let report = HealthReport::new(checks);
        tracing::debug!(
            phase = ?self.phase(),
            status = %report.overall_status(),
            check_count = report.checks.len(),
            "readiness evaluated"
        );
        report
    }

    pub async fn startup_report(&self) -> HealthReport {
        let mut checks = Vec::new();
        match self.phase() {
            ServicePhase::Starting => checks.push(HealthCheck::unhealthy(
                "startup",
                ServicePhase::Starting.to_string(),
            )),
            ServicePhase::Ready => checks.push(HealthCheck::healthy("startup")),
            ServicePhase::Draining => checks.push(HealthCheck::degraded(
                "startup",
                ServicePhase::Draining.to_string(),
            )),
        }
        HealthReport::new(checks)
    }

    async fn run_registered_checks(&self) -> Vec<HealthCheck> {
        let checks = self
            .checks
            .read()
            .expect("health registry lock poisoned")
            .clone();
        let timeout = self.check_timeout;
        join_all(checks.into_iter().map(|registered| async move {
            let name = registered.name;
            let check = AssertUnwindSafe((registered.check)()).catch_unwind();
            match tokio::time::timeout(timeout, check).await {
                Ok(Ok(mut check)) => {
                    if check.name.is_empty() {
                        check.name = name;
                    }
                    check
                }
                Ok(Err(_)) => HealthCheck::unhealthy(name, "health check panicked"),
                Err(_) => HealthCheck::unhealthy(
                    name,
                    format!("health check timed out after {}ms", timeout.as_millis()),
                ),
            }
        }))
        .await
    }

    fn phase_checks(&self) -> Vec<HealthCheck> {
        match self.phase() {
            ServicePhase::Starting => vec![HealthCheck::unhealthy(
                "phase",
                ServicePhase::Starting.to_string(),
            )],
            ServicePhase::Ready => vec![HealthCheck::healthy("phase")],
            ServicePhase::Draining => vec![HealthCheck::degraded(
                "phase",
                ServicePhase::Draining.to_string(),
            )],
        }
    }

    fn log_phase_change(&self, previous: ServicePhase) {
        let current = self.phase();
        if previous != current {
            tracing::debug!(from = ?previous, to = ?current, "health phase changed");
        }
    }
}

impl HealthCheck {
    pub fn healthy(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status: HealthStatus::Healthy,
            message: None,
        }
    }

    pub fn degraded(name: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status: HealthStatus::Degraded,
            message: Some(message.into()),
        }
    }

    pub fn unhealthy(name: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status: HealthStatus::Unhealthy,
            message: Some(message.into()),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthReport {
    pub checks: Vec<HealthCheck>,
}

impl HealthReport {
    pub fn new(checks: Vec<HealthCheck>) -> Self {
        Self { checks }
    }

    pub fn overall_status(&self) -> HealthStatus {
        if self
            .checks
            .iter()
            .any(|check| matches!(check.status, HealthStatus::Unhealthy))
        {
            HealthStatus::Unhealthy
        } else if self
            .checks
            .iter()
            .any(|check| matches!(check.status, HealthStatus::Degraded))
        {
            HealthStatus::Degraded
        } else {
            HealthStatus::Healthy
        }
    }

    pub fn is_ready(&self) -> bool {
        self.overall_status().is_ready()
    }

    pub fn is_alive(&self) -> bool {
        self.overall_status().is_alive()
    }

    pub fn render_text(&self) -> String {
        let mut out = format!("status={}\n", self.overall_status());
        for check in &self.checks {
            out.push_str(&format!(
                "{}={}",
                escape_text_field(&check.name),
                check.status
            ));
            if let Some(message) = &check.message {
                out.push_str(&format!(" message={}", escape_text_field(message)));
            }
            out.push('\n');
        }
        out
    }

    pub fn probe(&self, probe: ProbeKind) -> ProbeReport {
        ProbeReport {
            probe,
            status: self.overall_status(),
            ready: self.is_ready(),
            alive: self.is_alive(),
            checks: self.checks.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProbeKind {
    Liveness,
    Readiness,
    Startup,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbeReport {
    pub probe: ProbeKind,
    pub status: HealthStatus,
    pub ready: bool,
    pub alive: bool,
    pub checks: Vec<HealthCheck>,
}

fn escape_text_field(value: &str) -> String {
    value.replace('\\', r"\\").replace('\n', r"\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aggregates_statuses() {
        let report = HealthReport::new(vec![
            HealthCheck::healthy("db"),
            HealthCheck::degraded("cache", "warmup"),
        ]);

        assert_eq!(report.overall_status(), HealthStatus::Degraded);
        assert!(report.is_alive());
        assert!(!report.is_ready());
        assert!(report.render_text().contains("cache=degraded"));
        let probe = report.probe(ProbeKind::Readiness);
        assert_eq!(probe.probe, ProbeKind::Readiness);
        assert!(!probe.ready);
    }

    #[test]
    fn render_text_escapes_multiline_fields() {
        let report = HealthReport::new(vec![HealthCheck::degraded(
            "cache\nprimary",
            "warming\nslowly",
        )]);

        let rendered = report.render_text();

        assert!(rendered.contains(r"cache\nprimary=degraded"));
        assert!(rendered.contains(r"message=warming\nslowly"));
    }

    #[tokio::test]
    async fn registry_reports_phase_and_dependencies() {
        let registry = HealthRegistry::new();
        registry.register_dependency("db", || async { Ok(()) });

        let starting = registry.readiness_report().await;
        assert!(!starting.is_ready());
        assert_eq!(starting.checks[0].status, HealthStatus::Unhealthy);

        registry.mark_ready();
        let ready = registry.readiness_report().await;
        assert!(ready.is_ready());
        assert!(ready.checks.iter().any(|check| check.name == "db"));

        registry.mark_draining();
        let draining = registry.readiness_report().await;
        assert!(!draining.is_ready());
        assert_eq!(draining.checks[0].status, HealthStatus::Degraded);
        assert!(draining.is_alive());
    }

    #[tokio::test]
    async fn dependency_checks_run_concurrently_and_preserve_order() {
        let registry = HealthRegistry::new().with_check_timeout(Duration::from_secs(1));
        registry.mark_ready();
        let barrier = Arc::new(tokio::sync::Barrier::new(3));
        let first_barrier = barrier.clone();
        registry.register_dependency("first", move || {
            let barrier = first_barrier.clone();
            async move {
                barrier.wait().await;
                Ok(())
            }
        });
        let second_barrier = barrier.clone();
        registry.register_dependency("second", move || {
            let barrier = second_barrier.clone();
            async move {
                barrier.wait().await;
                Ok(())
            }
        });

        let report_task = tokio::spawn(async move { registry.readiness_report().await });
        tokio::time::timeout(Duration::from_millis(200), barrier.wait())
            .await
            .expect("both checks entered concurrently");
        let report = report_task.await.expect("readiness task");

        assert_eq!(report.checks[1].name, "first");
        assert_eq!(report.checks[2].name, "second");
    }

    #[tokio::test]
    async fn dependency_timeout_is_reported_as_unhealthy() {
        let registry = HealthRegistry::new().with_check_timeout(Duration::from_millis(20));
        registry.mark_ready();
        registry.register_dependency("stalled", || async {
            std::future::pending::<anyhow::Result<()>>().await
        });

        let report = registry.readiness_report().await;

        assert!(!report.is_ready());
        assert_eq!(report.checks[1].name, "stalled");
        assert_eq!(report.checks[1].status, HealthStatus::Unhealthy);
        assert_eq!(
            report.checks[1].message.as_deref(),
            Some("health check timed out after 20ms")
        );
    }

    #[tokio::test]
    async fn dependency_panic_is_isolated_as_unhealthy() {
        let registry = HealthRegistry::new();
        registry.mark_ready();
        registry.register_dependency("broken", || async {
            panic!("dependency check failed");
            #[allow(unreachable_code)]
            Ok(())
        });
        registry.register_dependency("healthy", || async { Ok(()) });

        let report = registry.readiness_report().await;

        assert!(!report.is_ready());
        assert_eq!(report.checks[1].name, "broken");
        assert_eq!(
            report.checks[1].message.as_deref(),
            Some("health check panicked")
        );
        assert_eq!(report.checks[2].status, HealthStatus::Healthy);
    }

    #[test]
    #[should_panic(expected = "health check timeout must be positive")]
    fn rejects_zero_check_timeout() {
        let _ = HealthRegistry::new().with_check_timeout(Duration::ZERO);
    }
}
