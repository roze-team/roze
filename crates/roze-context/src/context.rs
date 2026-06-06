use std::{
    any::Any,
    collections::HashMap,
    marker::PhantomData,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use roze_trace::generate_trace_id as trace_generate_trace_id;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelReason {
    Canceled,
    DeadlineExceeded,
}

#[derive(Debug)]
pub struct ContextKey<T: 'static> {
    name: &'static str,
    _marker: PhantomData<fn() -> T>,
}

impl<T: 'static> Copy for ContextKey<T> {}

impl<T: 'static> Clone for ContextKey<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: 'static> ContextKey<T> {
    pub const fn new(name: &'static str) -> Self {
        Self {
            name,
            _marker: PhantomData,
        }
    }
}

#[derive(Clone)]
pub struct Context {
    inner: Arc<ContextInner>,
}

struct ContextInner {
    trace_id: Mutex<String>,
    deadline: Mutex<Option<Instant>>,
    cancelled: AtomicBool,
    cancel_reason: Mutex<Option<CancelReason>>,
    error: Mutex<Option<String>>,
    values: Mutex<HashMap<&'static str, Arc<dyn Any + Send + Sync>>>,
}

impl ContextInner {
    fn new(trace_id: String) -> Self {
        Self {
            trace_id: Mutex::new(trace_id),
            deadline: Mutex::new(None),
            cancelled: AtomicBool::new(false),
            cancel_reason: Mutex::new(None),
            error: Mutex::new(None),
            values: Mutex::new(HashMap::new()),
        }
    }
}

impl std::fmt::Debug for ContextInner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ContextInner")
            .field(
                "trace_id",
                &self
                    .trace_id
                    .lock()
                    .expect("context trace_id mutex poisoned")
                    .clone(),
            )
            .field(
                "deadline",
                &self
                    .deadline
                    .lock()
                    .expect("context deadline mutex poisoned")
                    .map(|deadline| format!("{deadline:?}")),
            )
            .field("cancelled", &self.cancelled.load(Ordering::SeqCst))
            .field(
                "cancel_reason",
                &self
                    .cancel_reason
                    .lock()
                    .expect("context cancel mutex poisoned")
                    .clone(),
            )
            .field(
                "error",
                &self
                    .error
                    .lock()
                    .expect("context error mutex poisoned")
                    .clone(),
            )
            .field(
                "values_len",
                &self
                    .values
                    .lock()
                    .expect("context values mutex poisoned")
                    .len(),
            )
            .finish()
    }
}

impl Context {
    pub fn background() -> Self {
        Self::background_with_trace_id(trace_generate_trace_id())
    }

    pub fn background_with_trace_id(trace_id: impl Into<String>) -> Self {
        Self {
            inner: Arc::new(ContextInner::new(trace_id.into())),
        }
    }

    pub fn trace_id(&self) -> String {
        self.inner
            .trace_id
            .lock()
            .expect("context trace_id mutex poisoned")
            .clone()
    }

    pub fn with_trace_id(&self, trace_id: impl Into<String>) -> Self {
        let next = self.fork();
        *next
            .inner
            .trace_id
            .lock()
            .expect("context trace_id mutex poisoned") = trace_id.into();
        next
    }

    pub fn with_timeout(&self, timeout: Duration) -> Self {
        self.with_deadline(Instant::now() + timeout)
    }

    pub fn with_deadline(&self, deadline: Instant) -> Self {
        let next = self.fork();
        *next
            .inner
            .deadline
            .lock()
            .expect("context deadline mutex poisoned") = Some(deadline);
        next
    }

    pub fn deadline(&self) -> Option<Instant> {
        *self
            .inner
            .deadline
            .lock()
            .expect("context deadline mutex poisoned")
    }

    pub fn remaining_timeout(&self) -> Option<Duration> {
        let deadline = self.deadline()?;
        deadline.checked_duration_since(Instant::now())
    }

    pub fn cancel(&self) {
        self.inner.cancelled.store(true, Ordering::SeqCst);
        *self
            .inner
            .cancel_reason
            .lock()
            .expect("context cancel mutex poisoned") = Some(CancelReason::Canceled);
    }

    pub fn cancel_with_reason(&self, reason: CancelReason) {
        self.inner.cancelled.store(true, Ordering::SeqCst);
        *self
            .inner
            .cancel_reason
            .lock()
            .expect("context cancel mutex poisoned") = Some(reason);
    }

    pub fn cancelled(&self) -> bool {
        self.inner.cancelled.load(Ordering::SeqCst)
    }

    pub fn cancel_reason(&self) -> Option<CancelReason> {
        *self
            .inner
            .cancel_reason
            .lock()
            .expect("context cancel mutex poisoned")
    }

    pub fn with_error(&self, error: impl Into<String>) -> Self {
        let next = self.fork();
        *next
            .inner
            .error
            .lock()
            .expect("context error mutex poisoned") = Some(error.into());
        next
    }

    pub fn error(&self) -> Option<String> {
        self.inner
            .error
            .lock()
            .expect("context error mutex poisoned")
            .clone()
    }

    pub fn with_value<T>(&self, key: ContextKey<T>, value: T) -> Self
    where
        T: Send + Sync + 'static,
    {
        let next = self.fork();
        next.inner
            .values
            .lock()
            .expect("context values mutex poisoned")
            .insert(key.name, Arc::new(value));
        next
    }

    pub fn value<T>(&self, key: ContextKey<T>) -> Option<T>
    where
        T: Clone + Send + Sync + 'static,
    {
        let value = self
            .inner
            .values
            .lock()
            .expect("context values mutex poisoned")
            .get(key.name)?
            .clone();
        value.downcast_ref::<T>().cloned()
    }

    pub fn has_value<T>(&self, key: ContextKey<T>) -> bool
    where
        T: Send + Sync + 'static,
    {
        self.inner
            .values
            .lock()
            .expect("context values mutex poisoned")
            .contains_key(key.name)
    }

    pub fn is_expired(&self) -> bool {
        self.deadline()
            .map(|deadline| Instant::now() >= deadline)
            .unwrap_or(false)
    }

    pub fn with_expiration_reason(&self) -> Self {
        let next = self.fork();
        if next.is_expired() {
            next.cancel_with_reason(CancelReason::DeadlineExceeded);
        }
        next
    }

    fn fork(&self) -> Self {
        let trace_id = self.trace_id();
        let deadline = *self
            .inner
            .deadline
            .lock()
            .expect("context deadline mutex poisoned");
        let error = self.error();
        let values = self
            .inner
            .values
            .lock()
            .expect("context values mutex poisoned")
            .iter()
            .map(|(key, value)| (*key, Arc::clone(value)))
            .collect::<HashMap<_, _>>();
        let cancelled = self.cancelled();
        let cancel_reason = self.cancel_reason();

        Self {
            inner: Arc::new(ContextInner {
                trace_id: Mutex::new(trace_id),
                deadline: Mutex::new(deadline),
                cancelled: AtomicBool::new(cancelled),
                cancel_reason: Mutex::new(cancel_reason),
                error: Mutex::new(error),
                values: Mutex::new(values),
            }),
        }
    }
}

impl Default for Context {
    fn default() -> Self {
        Self::background()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct TestValue(u32);

    #[test]
    fn context_carries_trace_id_and_values() {
        let key = ContextKey::<TestValue>::new("test-value");
        let ctx = Context::background_with_trace_id("trace-123")
            .with_timeout(Duration::from_millis(20))
            .with_value(key, TestValue(42));

        assert_eq!(ctx.trace_id(), "trace-123");
        assert!(ctx.remaining_timeout().is_some());
        assert_eq!(ctx.value(key), Some(TestValue(42)));
    }

    #[test]
    fn context_cancellation_is_observable() {
        let ctx = Context::background();
        assert!(!ctx.cancelled());
        ctx.cancel();
        assert!(ctx.cancelled());
        assert_eq!(ctx.cancel_reason(), Some(CancelReason::Canceled));
    }
}
