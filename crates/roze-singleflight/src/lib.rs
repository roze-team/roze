use std::{
    any::Any,
    sync::{Arc, Mutex},
};

use dashmap::DashMap;
use tokio::sync::Notify;

#[derive(Debug, Clone, Default)]
pub struct SingleFlightGroup {
    entries: Arc<DashMap<String, Arc<FlightEntry>>>,
}

#[derive(Debug)]
struct FlightEntry {
    state: Mutex<FlightState>,
    notify: Notify,
}

#[derive(Debug, Default)]
struct FlightState {
    running: bool,
    result: Option<Result<Arc<dyn Any + Send + Sync>, String>>,
}

impl FlightEntry {
    fn new() -> Self {
        Self {
            state: Mutex::new(FlightState::default()),
            notify: Notify::new(),
        }
    }
}

impl SingleFlightGroup {
    pub fn new() -> Self {
        Self {
            entries: Arc::new(DashMap::new()),
        }
    }

    fn entry_for(&self, key: &str) -> Arc<FlightEntry> {
        self.entries
            .entry(key.to_string())
            .or_insert_with(|| Arc::new(FlightEntry::new()))
            .clone()
    }

    pub async fn do_call<T, F, Fut>(&self, key: impl AsRef<str>, loader: F) -> Result<T, String>
    where
        T: Clone + Send + Sync + 'static,
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<T, String>>,
    {
        let entry = self.entry_for(key.as_ref());
        let mut loader = Some(loader);

        loop {
            enum Action {
                Load,
                Wait,
            }

            let action = {
                let mut state = entry
                    .state
                    .lock()
                    .expect("singleflight entry lock poisoned");
                if let Some(result) = &state.result {
                    return match result {
                        Ok(value) => value
                            .downcast_ref::<T>()
                            .cloned()
                            .ok_or_else(|| "singleflight type mismatch".to_string()),
                        Err(err) => Err(err.clone()),
                    };
                }

                if !state.running {
                    state.running = true;
                    Action::Load
                } else {
                    Action::Wait
                }
            };

            match action {
                Action::Load => {
                    let output =
                        loader.take().expect("singleflight loader already consumed")().await;
                    let mut state = entry
                        .state
                        .lock()
                        .expect("singleflight entry lock poisoned");
                    state.result = Some(match output {
                        Ok(value) => Ok(Arc::new(value) as Arc<dyn Any + Send + Sync>),
                        Err(err) => Err(err),
                    });
                    state.running = false;
                    entry.notify.notify_waiters();
                }
                Action::Wait => {
                    entry.notify.notified().await;
                }
            }
        }
    }

    pub async fn reset(&self, key: impl AsRef<str>) {
        self.entries.remove(key.as_ref());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn deduplicates_same_key() {
        let group = Arc::new(SingleFlightGroup::new());
        let calls = Arc::new(AtomicUsize::new(0));
        let a_group = group.clone();
        let a_calls = calls.clone();
        let b_group = group.clone();
        let b_calls = calls.clone();

        let a = tokio::spawn(async move {
            a_group
                .do_call("user:1", || async {
                    a_calls.fetch_add(1, Ordering::SeqCst);
                    Ok::<_, String>(42u32)
                })
                .await
                .expect("result")
        });
        let b = tokio::spawn(async move {
            b_group
                .do_call("user:1", || async {
                    b_calls.fetch_add(1, Ordering::SeqCst);
                    Ok::<_, String>(42u32)
                })
                .await
                .expect("result")
        });

        assert_eq!(a.await.expect("join"), 42);
        assert_eq!(b.await.expect("join"), 42);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}
