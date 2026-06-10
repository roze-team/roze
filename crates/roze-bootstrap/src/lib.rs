use std::future::Future;

use roze_health::{HealthCheck, HealthReport};
use roze_service::{service_state, AppState};
use roze_shutdown::{channel, ShutdownHandle, ShutdownListener};

pub struct Bootstrap<C> {
    state: AppState<C>,
    checks: Vec<HealthCheck>,
}

impl<C> Bootstrap<C> {
    pub fn new(name: impl Into<std::sync::Arc<str>>, config: C) -> Self {
        Self {
            state: service_state(name, config),
            checks: Vec::new(),
        }
    }

    pub fn add_check(mut self, check: HealthCheck) -> Self {
        self.checks.push(check);
        self
    }

    pub fn state(&self) -> &AppState<C> {
        &self.state
    }

    pub fn health_report(&self) -> HealthReport {
        HealthReport::new(self.checks.clone())
    }

    pub fn into_state(self) -> AppState<C> {
        self.state
    }
}

pub fn bootstrap<C>(name: impl Into<std::sync::Arc<str>>, config: C) -> AppState<C> {
    service_state(name, config)
}

pub fn bootstrap_with_health<C>(
    name: impl Into<std::sync::Arc<str>>,
    config: C,
    checks: Vec<HealthCheck>,
) -> (AppState<C>, HealthReport) {
    let state = service_state(name, config);
    let report = HealthReport::new(checks);
    (state, report)
}

pub struct BootstrapRuntime<C> {
    state: AppState<C>,
    report: HealthReport,
    shutdown: ShutdownHandle,
    listener: ShutdownListener,
}

impl<C> BootstrapRuntime<C> {
    pub fn new(name: impl Into<std::sync::Arc<str>>, config: C) -> Self {
        let (shutdown, listener) = channel();
        Self {
            state: service_state(name, config),
            report: HealthReport::default(),
            shutdown,
            listener,
        }
    }

    pub fn state(&self) -> &AppState<C> {
        &self.state
    }

    pub fn health_report(&self) -> &HealthReport {
        &self.report
    }

    pub fn health_report_mut(&mut self) -> &mut HealthReport {
        &mut self.report
    }

    pub fn shutdown_handle(&self) -> ShutdownHandle {
        self.shutdown.clone()
    }

    pub fn shutdown_listener(&self) -> ShutdownListener {
        self.listener.clone()
    }

    pub fn into_parts(self) -> (AppState<C>, HealthReport, ShutdownHandle, ShutdownListener) {
        (self.state, self.report, self.shutdown, self.listener)
    }

    pub async fn run<F, Fut>(self, serve: F) -> anyhow::Result<()>
    where
        F: FnOnce(AppState<C>, HealthReport, ShutdownHandle) -> Fut,
        Fut: Future<Output = anyhow::Result<()>>,
    {
        let (state, report, shutdown, listener) = self.into_parts();
        tokio::select! {
            result = serve(state, report, shutdown.clone()) => {
                result
            }
            _ = listener.wait() => {
                Ok(())
            }
        }
    }
}

pub fn bootstrap_runtime<C>(
    name: impl Into<std::sync::Arc<str>>,
    config: C,
) -> BootstrapRuntime<C> {
    BootstrapRuntime::new(name, config)
}
