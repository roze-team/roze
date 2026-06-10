use std::{collections::BTreeMap, sync::Arc, time::Instant};

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

#[cfg(test)]
mod tests {
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
}
