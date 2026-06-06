use std::sync::Arc;
use std::time::Instant;

#[derive(Debug, Clone)]
pub struct AppState<C> {
    name: Arc<str>,
    config: Arc<C>,
    started_at: Instant,
}

impl<C> AppState<C> {
    pub fn new(name: impl Into<Arc<str>>, config: C) -> Self {
        Self {
            name: name.into(),
            config: Arc::new(config),
            started_at: Instant::now(),
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
}

#[derive(Debug, Clone)]
pub struct ServiceBuilder<C> {
    name: Arc<str>,
    config: C,
}

impl<C> ServiceBuilder<C> {
    pub fn new(name: impl Into<Arc<str>>, config: C) -> Self {
        Self {
            name: name.into(),
            config,
        }
    }

    pub fn build(self) -> AppState<C> {
        AppState {
            name: self.name,
            config: Arc::new(self.config),
            started_at: Instant::now(),
        }
    }
}

pub fn service_state<C>(name: impl Into<Arc<str>>, config: C) -> AppState<C> {
    AppState::new(name, config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct Config(u32);

    #[test]
    fn builds_state() {
        let state = service_state("demo", Config(7));
        assert_eq!(state.name(), "demo");
        assert_eq!(state.config(), &Config(7));
        let _ = state.uptime();
    }
}
