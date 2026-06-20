use roze_trace::generate_trace_id as trace_generate_trace_id;
use std::{
    any::Any,
    collections::{BTreeMap, HashMap},
    marker::PhantomData,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

pub const REQUEST_ID_HEADER: &str = "x-request-id";
pub const TRACE_ID_HEADER: &str = roze_trace::TRACE_ID_HEADER;
pub const TIMEOUT_HEADER: &str = "x-roze-timeout-ms";
pub const SUBJECT_HEADER: &str = "x-roze-subject";
pub const TENANT_HEADER: &str = "x-roze-tenant";
pub const ROLES_HEADER: &str = "x-roze-roles";
pub const METADATA_HEADER_PREFIX: &str = "x-roze-meta-";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthContext {
    pub subject: String,
    pub roles: Vec<String>,
    pub tenant: Option<String>,
}

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
    request_id: Mutex<String>,
    trace_id: Mutex<String>,
    auth: Mutex<Option<AuthContext>>,
    metadata: Mutex<BTreeMap<String, String>>,
    deadline: Mutex<Option<Instant>>,
    cancelled: AtomicBool,
    cancel_reason: Mutex<Option<CancelReason>>,
    error: Mutex<Option<String>>,
    values: Mutex<HashMap<&'static str, Arc<dyn Any + Send + Sync>>>,
}

impl ContextInner {
    fn new(request_id: String, trace_id: String) -> Self {
        Self {
            request_id: Mutex::new(request_id),
            trace_id: Mutex::new(trace_id),
            auth: Mutex::new(None),
            metadata: Mutex::new(BTreeMap::new()),
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
                "request_id",
                &self
                    .request_id
                    .lock()
                    .expect("context request_id mutex poisoned")
                    .clone(),
            )
            .field(
                "trace_id",
                &self
                    .trace_id
                    .lock()
                    .expect("context trace_id mutex poisoned")
                    .clone(),
            )
            .field(
                "auth",
                &self
                    .auth
                    .lock()
                    .expect("context auth mutex poisoned")
                    .clone(),
            )
            .field(
                "metadata",
                &self
                    .metadata
                    .lock()
                    .expect("context metadata mutex poisoned")
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
        let request_id = trace_generate_trace_id();
        Self::background_with_request_id_and_trace_id(request_id.clone(), request_id)
    }

    pub fn background_with_trace_id(trace_id: impl Into<String>) -> Self {
        let trace_id = trace_id.into();
        Self::background_with_request_id_and_trace_id(trace_id.clone(), trace_id)
    }

    pub fn background_with_request_id_and_trace_id(
        request_id: impl Into<String>,
        trace_id: impl Into<String>,
    ) -> Self {
        Self {
            inner: Arc::new(ContextInner::new(request_id.into(), trace_id.into())),
        }
    }

    pub fn request_id(&self) -> String {
        self.inner
            .request_id
            .lock()
            .expect("context request_id mutex poisoned")
            .clone()
    }

    pub fn with_request_id(&self, request_id: impl Into<String>) -> Self {
        let next = self.fork();
        *next
            .inner
            .request_id
            .lock()
            .expect("context request_id mutex poisoned") = request_id.into();
        next
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

    pub fn auth(&self) -> Option<AuthContext> {
        self.inner
            .auth
            .lock()
            .expect("context auth mutex poisoned")
            .clone()
    }

    pub fn with_auth(&self, auth: AuthContext) -> Self {
        let next = self.fork();
        *next.inner.auth.lock().expect("context auth mutex poisoned") = Some(auth);
        next
    }

    pub fn subject(&self) -> Option<String> {
        self.auth().map(|auth| auth.subject)
    }

    pub fn tenant(&self) -> Option<String> {
        self.auth().and_then(|auth| auth.tenant)
    }

    pub fn roles(&self) -> Vec<String> {
        self.auth().map(|auth| auth.roles).unwrap_or_default()
    }

    pub fn metadata(&self) -> BTreeMap<String, String> {
        self.inner
            .metadata
            .lock()
            .expect("context metadata mutex poisoned")
            .clone()
    }

    pub fn metadata_value(&self, key: impl AsRef<str>) -> Option<String> {
        self.inner
            .metadata
            .lock()
            .expect("context metadata mutex poisoned")
            .get(key.as_ref())
            .cloned()
    }

    pub fn with_metadata(&self, key: impl Into<String>, value: impl Into<String>) -> Self {
        let next = self.fork();
        next.inner
            .metadata
            .lock()
            .expect("context metadata mutex poisoned")
            .insert(key.into(), value.into());
        next
    }

    pub fn with_metadata_map(&self, metadata: BTreeMap<String, String>) -> Self {
        let next = self.fork();
        *next
            .inner
            .metadata
            .lock()
            .expect("context metadata mutex poisoned") = metadata;
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
        let request_id = self.request_id();
        let trace_id = self.trace_id();
        let auth = self.auth();
        let metadata = self.metadata();
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
                request_id: Mutex::new(request_id),
                trace_id: Mutex::new(trace_id),
                auth: Mutex::new(auth),
                metadata: Mutex::new(metadata),
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
        let ctx = Context::background_with_request_id_and_trace_id("request-123", "trace-123")
            .with_timeout(Duration::from_millis(20))
            .with_auth(AuthContext {
                subject: "user-1".to_string(),
                roles: vec!["admin".to_string()],
                tenant: Some("tenant-1".to_string()),
            })
            .with_metadata("locale", "zh-CN")
            .with_value(key, TestValue(42));

        assert_eq!(ctx.request_id(), "request-123");
        assert_eq!(ctx.trace_id(), "trace-123");
        assert_eq!(ctx.subject().as_deref(), Some("user-1"));
        assert_eq!(ctx.tenant().as_deref(), Some("tenant-1"));
        assert_eq!(ctx.roles(), vec!["admin"]);
        assert_eq!(ctx.metadata_value("locale").as_deref(), Some("zh-CN"));
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
