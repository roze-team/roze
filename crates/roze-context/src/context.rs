use roze_trace::generate_trace_id as trace_generate_trace_id;
use std::{
    any::Any,
    collections::{BTreeMap, HashMap},
    marker::PhantomData,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

pub const REQUEST_ID_HEADER: &str = "x-request-id";
pub const TRACE_ID_HEADER: &str = roze_trace::TRACE_ID_HEADER;
pub const TIMEOUT_HEADER: &str = "x-roze-timeout-ms";
pub const RETRY_BUDGET_HEADER: &str = "x-roze-retry-budget-remaining";
pub const IDEMPOTENCY_KEY_HEADER: &str = "idempotency-key";
pub const LOCALE_HEADER: &str = "x-roze-locale";
pub const ACCEPT_LANGUAGE_HEADER: &str = "accept-language";
pub const SUBJECT_HEADER: &str = "x-roze-subject";
pub const TENANT_HEADER: &str = "x-roze-tenant";
pub const ROLES_HEADER: &str = "x-roze-roles";
pub const PERMISSIONS_HEADER: &str = "x-roze-meta-permissions";
pub const SCOPE_HEADER: &str = "x-roze-meta-scope";
pub const METADATA_HEADER_PREFIX: &str = "x-roze-meta-";
pub const LOCALE_METADATA_KEY: &str = "locale";
pub const USER_ID_METADATA_KEY: &str = "uid";
pub const DEVICE_ID_METADATA_KEY: &str = "device_id";
pub const SCOPE_METADATA_KEY: &str = "scope";
pub const PERMISSIONS_METADATA_KEY: &str = "permissions";
pub const IDEMPOTENCY_KEY_METADATA_KEY: &str = "idempotency_key";
pub const MAX_PROPAGATED_RETRY_BUDGET: usize = 64;

pub const HULA_TENANT_ID_HEADER: &str = "x-hula-tenant-id";
pub const HULA_UID_HEADER: &str = "x-hula-uid";
pub const HULA_DEVICE_ID_HEADER: &str = "x-hula-device-id";
pub const HULA_TRACE_ID_HEADER: &str = "x-hula-trace-id";
pub const HULA_REQUEST_ID_HEADER: &str = "x-hula-request-id";
pub const HULA_ROLE_HEADER: &str = "x-hula-role";
pub const HULA_SCOPE_HEADER: &str = "x-hula-scope";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeaderAliases {
    pub request_id: &'static [&'static str],
    pub trace_id: &'static [&'static str],
    pub subject: &'static [&'static str],
    pub tenant: &'static [&'static str],
    pub roles: &'static [&'static str],
    pub metadata: &'static [(&'static str, &'static str)],
}

pub const EMPTY_HEADER_ALIASES: HeaderAliases = HeaderAliases {
    request_id: &[],
    trace_id: &[],
    subject: &[],
    tenant: &[],
    roles: &[],
    metadata: &[],
};

pub const HULA_HEADER_ALIASES: HeaderAliases = HeaderAliases {
    request_id: &[HULA_REQUEST_ID_HEADER],
    trace_id: &[HULA_TRACE_ID_HEADER],
    subject: &[HULA_UID_HEADER],
    tenant: &[HULA_TENANT_ID_HEADER],
    roles: &[HULA_ROLE_HEADER],
    metadata: &[
        (USER_ID_METADATA_KEY, HULA_UID_HEADER),
        (DEVICE_ID_METADATA_KEY, HULA_DEVICE_ID_HEADER),
        (SCOPE_METADATA_KEY, HULA_SCOPE_HEADER),
    ],
};

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
    retry_budget: Mutex<Option<Arc<AtomicUsize>>>,
    cancelled: Arc<AtomicBool>,
    cancel_reason: Arc<Mutex<Option<CancelReason>>>,
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
            retry_budget: Mutex::new(None),
            cancelled: Arc::new(AtomicBool::new(false)),
            cancel_reason: Arc::new(Mutex::new(None)),
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
            .field(
                "retry_budget_remaining",
                &self
                    .retry_budget
                    .lock()
                    .expect("context retry budget mutex poisoned")
                    .as_ref()
                    .map(|budget| budget.load(Ordering::Acquire)),
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

    pub fn permissions(&self) -> Vec<String> {
        self.metadata_value(PERMISSIONS_METADATA_KEY)
            .map(|value| split_context_list(&value))
            .unwrap_or_default()
    }

    pub fn with_permissions<I, S>(&self, permissions: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let permissions = permissions
            .into_iter()
            .map(|permission| permission.as_ref().trim().to_string())
            .filter(|permission| !permission.is_empty())
            .collect::<Vec<_>>();
        self.with_metadata(PERMISSIONS_METADATA_KEY, permissions.join(","))
    }

    pub fn has_permissions<I, S>(&self, required: I) -> bool
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let permissions = self.permissions();
        required
            .into_iter()
            .all(|permission| permissions.iter().any(|owned| owned == permission.as_ref()))
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

    pub fn locale(&self) -> Option<String> {
        self.metadata_value(LOCALE_METADATA_KEY)
    }

    pub fn with_locale(&self, locale: impl Into<String>) -> Self {
        self.with_metadata(LOCALE_METADATA_KEY, locale)
    }

    pub fn idempotency_key(&self) -> Option<String> {
        self.metadata_value(IDEMPOTENCY_KEY_METADATA_KEY)
    }

    pub fn with_idempotency_key(&self, key: impl Into<String>) -> Self {
        self.with_metadata(IDEMPOTENCY_KEY_METADATA_KEY, key)
    }

    pub fn with_retry_budget(&self, remaining: usize) -> Self {
        let next = self.fork();
        *next
            .inner
            .retry_budget
            .lock()
            .expect("context retry budget mutex poisoned") = Some(Arc::new(AtomicUsize::new(
            remaining.min(MAX_PROPAGATED_RETRY_BUDGET),
        )));
        next
    }

    pub fn ensure_retry_budget(&self, remaining: usize) {
        let mut budget = self
            .inner
            .retry_budget
            .lock()
            .expect("context retry budget mutex poisoned");
        if budget.is_none() {
            *budget = Some(Arc::new(AtomicUsize::new(
                remaining.min(MAX_PROPAGATED_RETRY_BUDGET),
            )));
        }
    }

    pub fn retry_budget_remaining(&self) -> Option<usize> {
        self.inner
            .retry_budget
            .lock()
            .expect("context retry budget mutex poisoned")
            .as_ref()
            .map(|budget| budget.load(Ordering::Acquire))
    }

    pub fn try_consume_retry_budget(&self) -> bool {
        let budget = self
            .inner
            .retry_budget
            .lock()
            .expect("context retry budget mutex poisoned")
            .clone();
        let Some(budget) = budget else {
            return true;
        };
        budget
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |remaining| {
                remaining.checked_sub(1)
            })
            .is_ok()
    }

    pub fn take_retry_budget_up_to(&self, maximum: usize) -> usize {
        let budget = self
            .inner
            .retry_budget
            .lock()
            .expect("context retry budget mutex poisoned")
            .clone();
        let Some(budget) = budget else {
            return 0;
        };
        let previous = budget
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                Some(current.saturating_sub(current.min(maximum)))
            })
            .expect("retry budget allocation update is infallible");
        previous.min(maximum)
    }

    pub fn restore_retry_budget(&self, amount: usize) {
        if amount == 0 {
            return;
        }
        let budget = self
            .inner
            .retry_budget
            .lock()
            .expect("context retry budget mutex poisoned")
            .clone();
        match budget {
            Some(budget) => {
                let _ = budget.fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                    Some(
                        current
                            .saturating_add(amount)
                            .min(MAX_PROPAGATED_RETRY_BUDGET),
                    )
                });
            }
            None => self.ensure_retry_budget(amount),
        }
    }

    pub fn limit_retry_budget(&self, remaining: usize) {
        let remaining = remaining.min(MAX_PROPAGATED_RETRY_BUDGET);
        let budget = self
            .inner
            .retry_budget
            .lock()
            .expect("context retry budget mutex poisoned")
            .clone();
        match budget {
            Some(budget) => {
                let _ = budget.fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                    Some(current.min(remaining))
                });
            }
            None => self.ensure_retry_budget(remaining),
        }
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

    pub fn with_merged_metadata(&self, metadata: BTreeMap<String, String>) -> Self {
        let next = self.fork();
        next.inner
            .metadata
            .lock()
            .expect("context metadata mutex poisoned")
            .extend(metadata);
        next
    }

    pub fn propagation_headers(&self) -> BTreeMap<String, String> {
        let mut headers = BTreeMap::new();
        headers.insert(REQUEST_ID_HEADER.to_string(), self.request_id());
        headers.insert(TRACE_ID_HEADER.to_string(), self.trace_id());
        if let Some(timeout) = self.remaining_timeout() {
            headers.insert(
                TIMEOUT_HEADER.to_string(),
                timeout.as_millis().max(1).to_string(),
            );
        }
        if let Some(remaining) = self.retry_budget_remaining() {
            headers.insert(RETRY_BUDGET_HEADER.to_string(), remaining.to_string());
        }
        if let Some(auth) = self.auth() {
            headers.insert(SUBJECT_HEADER.to_string(), auth.subject);
            if let Some(tenant) = auth.tenant {
                headers.insert(TENANT_HEADER.to_string(), tenant);
            }
            if !auth.roles.is_empty() {
                headers.insert(ROLES_HEADER.to_string(), auth.roles.join(","));
            }
        }
        if let Some(idempotency_key) = self.idempotency_key() {
            headers.insert(IDEMPOTENCY_KEY_HEADER.to_string(), idempotency_key);
        }
        for (key, value) in self.metadata() {
            headers.insert(format!("{METADATA_HEADER_PREFIX}{key}"), value);
        }
        headers
    }

    pub fn from_propagation_headers(headers: &BTreeMap<String, String>) -> Self {
        Self::from_propagation_headers_with_aliases(headers, HULA_HEADER_ALIASES)
    }

    pub fn from_propagation_headers_with_aliases(
        headers: &BTreeMap<String, String>,
        aliases: HeaderAliases,
    ) -> Self {
        let request_id = header_value_with_aliases(headers, REQUEST_ID_HEADER, aliases.request_id)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .unwrap_or_else(trace_generate_trace_id);
        let trace_id = header_value_with_aliases(headers, TRACE_ID_HEADER, aliases.trace_id)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| request_id.clone());
        let mut ctx = Self::background_with_request_id_and_trace_id(request_id, trace_id)
            .with_metadata_map(metadata_from_headers_with_aliases(headers, aliases));
        if let Some(auth) = auth_from_headers_with_aliases(headers, aliases) {
            ctx = ctx.with_auth(auth);
        }
        if let Some(timeout) = header_value(headers, TIMEOUT_HEADER)
            .and_then(|value| value.parse::<u64>().ok())
            .map(Duration::from_millis)
        {
            ctx = ctx.with_timeout(timeout);
        }
        if let Some(remaining) = retry_budget_from_headers(headers) {
            ctx = ctx.with_retry_budget(remaining);
        }
        ctx
    }

    pub fn with_propagation_headers(&self, headers: &BTreeMap<String, String>) -> Self {
        self.with_propagation_headers_and_aliases(headers, HULA_HEADER_ALIASES)
    }

    pub fn with_propagation_headers_and_aliases(
        &self,
        headers: &BTreeMap<String, String>,
        aliases: HeaderAliases,
    ) -> Self {
        let mut ctx = self.clone();
        if let Some(request_id) =
            header_value_with_aliases(headers, REQUEST_ID_HEADER, aliases.request_id)
        {
            ctx = ctx.with_request_id(request_id);
        }
        if let Some(trace_id) =
            header_value_with_aliases(headers, TRACE_ID_HEADER, aliases.trace_id)
        {
            ctx = ctx.with_trace_id(trace_id);
        }
        if let Some(auth) = auth_from_headers_with_aliases(headers, aliases) {
            ctx = ctx.with_auth(auth);
        }
        let metadata = metadata_from_headers_with_aliases(headers, aliases);
        if !metadata.is_empty() {
            ctx = ctx.with_merged_metadata(metadata);
        }
        if let Some(timeout) = header_value(headers, TIMEOUT_HEADER)
            .and_then(|value| value.parse::<u64>().ok())
            .map(Duration::from_millis)
        {
            ctx = ctx.with_timeout(timeout);
        }
        if let Some(remaining) = retry_budget_from_headers(headers) {
            ctx = ctx.with_retry_budget(remaining);
        }
        ctx
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
        let retry_budget = self
            .inner
            .retry_budget
            .lock()
            .expect("context retry budget mutex poisoned")
            .clone();
        let error = self.error();
        let values = self
            .inner
            .values
            .lock()
            .expect("context values mutex poisoned")
            .iter()
            .map(|(key, value)| (*key, Arc::clone(value)))
            .collect::<HashMap<_, _>>();
        let cancelled = Arc::clone(&self.inner.cancelled);
        let cancel_reason = Arc::clone(&self.inner.cancel_reason);

        Self {
            inner: Arc::new(ContextInner {
                request_id: Mutex::new(request_id),
                trace_id: Mutex::new(trace_id),
                auth: Mutex::new(auth),
                metadata: Mutex::new(metadata),
                deadline: Mutex::new(deadline),
                retry_budget: Mutex::new(retry_budget),
                cancelled,
                cancel_reason,
                error: Mutex::new(error),
                values: Mutex::new(values),
            }),
        }
    }
}

fn header_value<'a>(headers: &'a BTreeMap<String, String>, key: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(key))
        .map(|(_, value)| value.as_str())
}

fn header_value_with_aliases<'a>(
    headers: &'a BTreeMap<String, String>,
    key: &str,
    aliases: &[&str],
) -> Option<&'a str> {
    header_value(headers, key).or_else(|| {
        aliases
            .iter()
            .find_map(|alias| header_value(headers, alias))
    })
}

fn retry_budget_from_headers(headers: &BTreeMap<String, String>) -> Option<usize> {
    header_value(headers, RETRY_BUDGET_HEADER)
        .and_then(|value| value.parse::<usize>().ok())
        .map(|remaining| remaining.min(MAX_PROPAGATED_RETRY_BUDGET))
}

fn metadata_from_headers_with_aliases(
    headers: &BTreeMap<String, String>,
    aliases: HeaderAliases,
) -> BTreeMap<String, String> {
    let prefix = METADATA_HEADER_PREFIX.to_ascii_lowercase();
    let mut metadata = headers
        .iter()
        .filter_map(|(key, value)| {
            let lower = key.to_ascii_lowercase();
            lower
                .strip_prefix(&prefix)
                .map(|key| (key.to_string(), value.clone()))
        })
        .collect::<BTreeMap<_, _>>();
    for (metadata_key, header) in aliases.metadata {
        if metadata.contains_key(*metadata_key) {
            continue;
        }
        if let Some(value) = header_value(headers, header).filter(|value| !value.is_empty()) {
            metadata.insert((*metadata_key).to_string(), value.to_string());
        }
    }
    if !metadata.contains_key(IDEMPOTENCY_KEY_METADATA_KEY) {
        if let Some(value) =
            header_value(headers, IDEMPOTENCY_KEY_HEADER).filter(|value| !value.is_empty())
        {
            metadata.insert(IDEMPOTENCY_KEY_METADATA_KEY.to_string(), value.to_string());
        }
    }
    metadata
}

fn split_context_list(raw: &str) -> Vec<String> {
    raw.split(|ch: char| ch == ',' || ch.is_whitespace())
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn auth_from_headers_with_aliases(
    headers: &BTreeMap<String, String>,
    aliases: HeaderAliases,
) -> Option<AuthContext> {
    let subject = header_value_with_aliases(headers, SUBJECT_HEADER, aliases.subject)
        .filter(|value| !value.is_empty())?
        .to_string();
    let tenant = header_value_with_aliases(headers, TENANT_HEADER, aliases.tenant)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    let roles = header_value_with_aliases(headers, ROLES_HEADER, aliases.roles)
        .map(|value| {
            value
                .split(',')
                .map(str::trim)
                .filter(|role| !role.is_empty())
                .map(ToOwned::to_owned)
                .collect()
        })
        .unwrap_or_default();
    Some(AuthContext {
        subject,
        roles,
        tenant,
    })
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
        let forked = ctx.with_locale("zh-CN");
        assert!(!ctx.cancelled());
        assert!(!forked.cancelled());
        ctx.cancel();
        assert!(ctx.cancelled());
        assert!(forked.cancelled());
        assert_eq!(ctx.cancel_reason(), Some(CancelReason::Canceled));
        assert_eq!(forked.cancel_reason(), Some(CancelReason::Canceled));
    }

    #[test]
    fn context_round_trips_propagation_headers() {
        let ctx = Context::background_with_request_id_and_trace_id("request-1", "trace-1")
            .with_auth(AuthContext {
                subject: "user-1".to_string(),
                roles: vec!["admin".to_string(), "ops".to_string()],
                tenant: Some("tenant-1".to_string()),
            })
            .with_locale("zh-CN")
            .with_idempotency_key("checkout-42")
            .with_retry_budget(3);

        let restored = Context::from_propagation_headers(&ctx.propagation_headers());

        assert_eq!(restored.request_id(), "request-1");
        assert_eq!(restored.trace_id(), "trace-1");
        assert_eq!(restored.subject().as_deref(), Some("user-1"));
        assert_eq!(restored.tenant().as_deref(), Some("tenant-1"));
        assert_eq!(restored.roles(), vec!["admin", "ops"]);
        assert_eq!(restored.locale().as_deref(), Some("zh-CN"));
        assert_eq!(restored.idempotency_key().as_deref(), Some("checkout-42"));
        assert_eq!(restored.retry_budget_remaining(), Some(3));
        assert_eq!(
            ctx.propagation_headers()
                .get(IDEMPOTENCY_KEY_HEADER)
                .map(String::as_str),
            Some("checkout-42")
        );
    }

    #[test]
    fn standard_idempotency_header_enters_context_metadata() {
        let headers = BTreeMap::from([("Idempotency-Key".to_string(), "payment-17".to_string())]);

        let restored = Context::from_propagation_headers(&headers);

        assert_eq!(restored.idempotency_key().as_deref(), Some("payment-17"));
    }

    #[test]
    fn retry_budget_is_shared_by_clones_and_forks_and_never_underflows() {
        let context = Context::background().with_retry_budget(2);
        let cloned = context.clone();
        let forked = context.with_locale("zh-CN");

        assert!(cloned.try_consume_retry_budget());
        assert!(forked.try_consume_retry_budget());
        assert!(!context.try_consume_retry_budget());
        assert_eq!(context.retry_budget_remaining(), Some(0));
        assert_eq!(forked.retry_budget_remaining(), Some(0));
    }

    #[test]
    fn retry_budget_header_is_bounded_and_only_tightens() {
        let headers = BTreeMap::from([(RETRY_BUDGET_HEADER.to_string(), usize::MAX.to_string())]);
        let context = Context::from_propagation_headers(&headers);

        assert_eq!(
            context.retry_budget_remaining(),
            Some(MAX_PROPAGATED_RETRY_BUDGET)
        );
        context.limit_retry_budget(5);
        context.limit_retry_budget(9);
        assert_eq!(context.retry_budget_remaining(), Some(5));
    }

    #[test]
    fn retry_budget_allocation_is_atomic_and_restoration_is_bounded() {
        let context = Context::background().with_retry_budget(4);
        let first = context.take_retry_budget_up_to(3);
        let second = context.take_retry_budget_up_to(3);

        assert_eq!(first, 3);
        assert_eq!(second, 1);
        assert_eq!(context.retry_budget_remaining(), Some(0));

        context.restore_retry_budget(first);
        context.restore_retry_budget(second);
        context.restore_retry_budget(usize::MAX);
        assert_eq!(
            context.retry_budget_remaining(),
            Some(MAX_PROPAGATED_RETRY_BUDGET)
        );
    }

    #[test]
    fn context_permissions_round_trip_and_require_every_permission() {
        let ctx = Context::background().with_permissions(["users:read", "users:write"]);

        assert_eq!(ctx.permissions(), vec!["users:read", "users:write"]);
        assert!(ctx.has_permissions(["users:read"]));
        assert!(ctx.has_permissions(["users:read", "users:write"]));
        assert!(!ctx.has_permissions(["users:delete"]));

        let restored = Context::from_propagation_headers(&ctx.propagation_headers());
        assert_eq!(restored.permissions(), vec!["users:read", "users:write"]);
    }

    #[test]
    fn context_restores_hula_header_aliases() {
        let headers = BTreeMap::from([
            (
                HULA_REQUEST_ID_HEADER.to_string(),
                "request-hula".to_string(),
            ),
            (HULA_TRACE_ID_HEADER.to_string(), "trace-hula".to_string()),
            (HULA_TENANT_ID_HEADER.to_string(), "tenant-hula".to_string()),
            (HULA_UID_HEADER.to_string(), "user-hula".to_string()),
            (HULA_DEVICE_ID_HEADER.to_string(), "device-hula".to_string()),
            (HULA_ROLE_HEADER.to_string(), "admin,ops".to_string()),
            (HULA_SCOPE_HEADER.to_string(), "message:write".to_string()),
        ]);

        let restored = Context::from_propagation_headers(&headers);

        assert_eq!(restored.request_id(), "request-hula");
        assert_eq!(restored.trace_id(), "trace-hula");
        assert_eq!(restored.subject().as_deref(), Some("user-hula"));
        assert_eq!(restored.tenant().as_deref(), Some("tenant-hula"));
        assert_eq!(restored.roles(), vec!["admin", "ops"]);
        assert_eq!(
            restored.metadata_value(USER_ID_METADATA_KEY).as_deref(),
            Some("user-hula")
        );
        assert_eq!(
            restored.metadata_value(DEVICE_ID_METADATA_KEY).as_deref(),
            Some("device-hula")
        );
        assert_eq!(
            restored.metadata_value(SCOPE_METADATA_KEY).as_deref(),
            Some("message:write")
        );
    }

    #[test]
    fn standard_headers_take_precedence_over_hula_aliases() {
        let headers = BTreeMap::from([
            (REQUEST_ID_HEADER.to_string(), "request-roze".to_string()),
            (TRACE_ID_HEADER.to_string(), "trace-roze".to_string()),
            (SUBJECT_HEADER.to_string(), "user-roze".to_string()),
            (
                HULA_REQUEST_ID_HEADER.to_string(),
                "request-hula".to_string(),
            ),
            (HULA_TRACE_ID_HEADER.to_string(), "trace-hula".to_string()),
            (HULA_UID_HEADER.to_string(), "user-hula".to_string()),
        ]);

        let restored = Context::from_propagation_headers(&headers);

        assert_eq!(restored.request_id(), "request-roze");
        assert_eq!(restored.trace_id(), "trace-roze");
        assert_eq!(restored.subject().as_deref(), Some("user-roze"));
    }

    #[test]
    fn propagation_headers_merge_with_existing_metadata() {
        let ctx = Context::background_with_request_id_and_trace_id("request-old", "trace-old")
            .with_metadata("existing", "keep")
            .with_metadata("scope", "old-scope");
        let headers = BTreeMap::from([
            (TRACE_ID_HEADER.to_string(), "trace-new".to_string()),
            (HULA_SCOPE_HEADER.to_string(), "new-scope".to_string()),
        ]);

        let restored = ctx.with_propagation_headers(&headers);

        assert_eq!(restored.request_id(), "request-old");
        assert_eq!(restored.trace_id(), "trace-new");
        assert_eq!(restored.metadata_value("existing").as_deref(), Some("keep"));
        assert_eq!(
            restored.metadata_value(SCOPE_METADATA_KEY).as_deref(),
            Some("new-scope")
        );
    }
}
