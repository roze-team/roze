use roze_service::{service_state, AppState};

pub struct Bootstrap<C> {
    state: AppState<C>,
}

impl<C> Bootstrap<C> {
    pub fn new(name: impl Into<std::sync::Arc<str>>, config: C) -> Self {
        Self {
            state: service_state(name, config),
        }
    }

    pub fn state(&self) -> &AppState<C> {
        &self.state
    }

    pub fn into_state(self) -> AppState<C> {
        self.state
    }
}

pub fn bootstrap<C>(name: impl Into<std::sync::Arc<str>>, config: C) -> AppState<C> {
    service_state(name, config)
}

