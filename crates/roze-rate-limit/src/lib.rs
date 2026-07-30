use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    sync::Arc,
    time::{Duration, Instant},
};

use async_trait::async_trait;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum RateLimitStoreKind {
    #[default]
    Auto,
    Memory,
    Redis,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum RateLimitUnavailablePolicy {
    FailOpen,
    #[default]
    FailClosed,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RateLimiterConfig {
    #[serde(default)]
    pub store: RateLimitStoreKind,
    #[serde(default)]
    pub redis_url: Option<String>,
    #[serde(default)]
    pub redis_cluster_urls: Vec<String>,
    #[serde(default = "default_key_prefix")]
    pub key_prefix: String,
    #[serde(default)]
    pub namespace: Option<String>,
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
    #[serde(default)]
    pub unavailable_policy: RateLimitUnavailablePolicy,
}

impl fmt::Debug for RateLimiterConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RateLimiterConfig")
            .field("store", &self.store)
            .field("redis_url", &self.redis_url.as_ref().map(|_| "[REDACTED]"))
            .field(
                "redis_cluster_urls",
                &vec!["[REDACTED]"; self.redis_cluster_urls.len()],
            )
            .field("key_prefix", &self.key_prefix)
            .field("namespace", &self.namespace)
            .field("timeout_ms", &self.timeout_ms)
            .field("unavailable_policy", &self.unavailable_policy)
            .finish()
    }
}

impl Default for RateLimiterConfig {
    fn default() -> Self {
        Self {
            store: RateLimitStoreKind::Auto,
            redis_url: None,
            redis_cluster_urls: Vec::new(),
            key_prefix: default_key_prefix(),
            namespace: None,
            timeout_ms: default_timeout_ms(),
            unavailable_policy: RateLimitUnavailablePolicy::FailClosed,
        }
    }
}

impl RateLimiterConfig {
    pub fn resolved_store_kind(&self) -> RateLimitStoreKind {
        match self.store {
            RateLimitStoreKind::Auto
                if self.redis_url.as_deref().is_some_and(non_empty)
                    || !self.redis_cluster_urls.is_empty() =>
            {
                RateLimitStoreKind::Redis
            }
            RateLimitStoreKind::Auto => RateLimitStoreKind::Memory,
            explicit => explicit,
        }
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(self.timeout_ms > 0, "rate limit timeout must be positive");
        anyhow::ensure!(
            !self.key_prefix.trim().is_empty(),
            "rate limit key prefix must not be empty"
        );
        if let Some(namespace) = &self.namespace {
            anyhow::ensure!(
                !namespace.trim().is_empty(),
                "rate limit namespace must not be empty when configured"
            );
        }
        anyhow::ensure!(
            self.redis_cluster_urls
                .iter()
                .all(|url| !url.trim().is_empty()),
            "redis_cluster_urls cannot contain empty seed URLs"
        );
        if self.store == RateLimitStoreKind::Redis {
            anyhow::ensure!(
                self.redis_url.as_deref().is_some_and(non_empty)
                    || !self.redis_cluster_urls.is_empty(),
                "Redis rate limiting requires redis_url or redis_cluster_urls"
            );
        }
        Ok(())
    }
}

fn non_empty(value: &str) -> bool {
    !value.trim().is_empty()
}

fn default_key_prefix() -> String {
    "roze:rate-limit:v1".to_string()
}

const fn default_timeout_ms() -> u64 {
    100
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum RateLimitDimension {
    Route,
    ClientIp,
    Subject,
    Tenant,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum MissingDimensionPolicy {
    #[default]
    Reject,
    Omit,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RateLimitKeyPolicy {
    #[serde(default = "default_dimensions")]
    pub dimensions: Vec<RateLimitDimension>,
    #[serde(default)]
    pub headers: Vec<String>,
    #[serde(default)]
    pub missing: MissingDimensionPolicy,
}

impl Default for RateLimitKeyPolicy {
    fn default() -> Self {
        Self {
            dimensions: default_dimensions(),
            headers: Vec::new(),
            missing: MissingDimensionPolicy::Reject,
        }
    }
}

impl RateLimitKeyPolicy {
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            !self.dimensions.is_empty() || !self.headers.is_empty(),
            "rate limit key policy must select at least one dimension or header"
        );
        let mut dimensions = BTreeSet::new();
        for dimension in &self.dimensions {
            anyhow::ensure!(
                dimensions.insert(*dimension),
                "rate limit key dimensions must not contain duplicates"
            );
        }
        let mut headers = BTreeSet::new();
        for header in &self.headers {
            let normalized = header.trim().to_ascii_lowercase();
            anyhow::ensure!(
                valid_header_name(&normalized),
                "rate limit header name `{header}` is invalid"
            );
            anyhow::ensure!(
                headers.insert(normalized),
                "rate limit header names must not contain duplicates"
            );
        }
        Ok(())
    }
}

fn default_dimensions() -> Vec<RateLimitDimension> {
    vec![RateLimitDimension::Route]
}

#[derive(Debug, Clone, Copy)]
pub struct RateLimit {
    pub burst: u32,
    pub refill: Duration,
}

impl RateLimit {
    pub fn validate(self) -> anyhow::Result<()> {
        anyhow::ensure!(self.burst > 0, "rate limit burst must be positive");
        anyhow::ensure!(
            !self.refill.is_zero(),
            "rate limit refill duration must be positive"
        );
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RateLimitIdentity {
    pub service: String,
    pub boundary: String,
    pub operation: String,
    pub client_ip: Option<String>,
    pub subject: Option<String>,
    pub tenant: Option<String>,
    headers: BTreeMap<String, String>,
}

impl RateLimitIdentity {
    pub fn new(
        service: impl Into<String>,
        boundary: impl Into<String>,
        operation: impl Into<String>,
    ) -> Self {
        Self {
            service: service.into(),
            boundary: boundary.into(),
            operation: operation.into(),
            client_ip: None,
            subject: None,
            tenant: None,
            headers: BTreeMap::new(),
        }
    }

    pub fn with_client_ip(mut self, client_ip: Option<impl ToString>) -> Self {
        self.client_ip = client_ip.map(|value| value.to_string());
        self
    }

    pub fn with_subject(mut self, subject: Option<impl Into<String>>) -> Self {
        self.subject = subject.map(Into::into);
        self
    }

    pub fn with_tenant(mut self, tenant: Option<impl Into<String>>) -> Self {
        self.tenant = tenant.map(Into::into);
        self
    }

    pub fn with_headers<I, K, V>(mut self, headers: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: AsRef<str>,
        V: Into<String>,
    {
        for (name, value) in headers {
            self.headers
                .insert(name.as_ref().to_ascii_lowercase(), value.into());
        }
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateLimitDecision {
    pub allowed: bool,
    pub retry_after: Duration,
    pub degraded: bool,
}

impl RateLimitDecision {
    fn allowed(degraded: bool) -> Self {
        Self {
            allowed: true,
            retry_after: Duration::ZERO,
            degraded,
        }
    }

    fn denied(retry_after: Duration) -> Self {
        Self {
            allowed: false,
            retry_after,
            degraded: false,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RateLimitError {
    #[error("rate limit key policy has no dimensions")]
    EmptyKeyPolicy,
    #[error("rate limit key dimension `{0}` is unavailable")]
    MissingDimension(&'static str),
    #[error("rate limit header name `{0}` is invalid")]
    InvalidHeader(String),
    #[error("rate limit store is unavailable")]
    StoreUnavailable,
}

#[async_trait]
pub trait RateLimitStore: fmt::Debug + Send + Sync + 'static {
    async fn check(&self, key: &str, limit: RateLimit) -> anyhow::Result<RateLimitDecision>;

    async fn health_check(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

#[derive(Clone)]
pub struct RateLimiter {
    store: Arc<dyn RateLimitStore>,
    timeout: Duration,
    unavailable_policy: RateLimitUnavailablePolicy,
    store_kind: RateLimitStoreKind,
}

impl fmt::Debug for RateLimiter {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RateLimiter")
            .field("timeout", &self.timeout)
            .field("unavailable_policy", &self.unavailable_policy)
            .field("store_kind", &self.store_kind)
            .finish_non_exhaustive()
    }
}

impl RateLimiter {
    pub fn from_config(config: &RateLimiterConfig) -> anyhow::Result<Self> {
        config.validate()?;
        let store_kind = config.resolved_store_kind();
        let key_prefix = match config.namespace.as_deref().filter(|value| non_empty(value)) {
            Some(namespace) => format!(
                "{}:{}",
                config.key_prefix.trim_end_matches(':'),
                namespace.trim_matches(':')
            ),
            None => config.key_prefix.clone(),
        };
        let store: Arc<dyn RateLimitStore> = match store_kind {
            RateLimitStoreKind::Auto => unreachable!("auto store kind must resolve before use"),
            RateLimitStoreKind::Memory => Arc::new(InMemoryRateLimitStore::default()),
            RateLimitStoreKind::Redis => Arc::new(RedisRateLimitStore::connect_topology(
                config.redis_url.as_deref().unwrap_or_default(),
                &config.redis_cluster_urls,
                &key_prefix,
            )?),
        };
        Ok(Self {
            store,
            timeout: Duration::from_millis(config.timeout_ms),
            unavailable_policy: config.unavailable_policy,
            store_kind,
        })
    }

    pub fn with_store(
        store: Arc<dyn RateLimitStore>,
        timeout: Duration,
        unavailable_policy: RateLimitUnavailablePolicy,
    ) -> anyhow::Result<Self> {
        anyhow::ensure!(!timeout.is_zero(), "rate limit timeout must be positive");
        Ok(Self {
            store,
            timeout,
            unavailable_policy,
            store_kind: RateLimitStoreKind::Memory,
        })
    }

    pub fn store_kind(&self) -> RateLimitStoreKind {
        self.store_kind
    }

    pub async fn check(
        &self,
        policy: &RateLimitKeyPolicy,
        identity: &RateLimitIdentity,
        limit: RateLimit,
    ) -> Result<RateLimitDecision, RateLimitError> {
        if limit.burst == 0 || limit.refill.is_zero() {
            return Ok(RateLimitDecision::denied(
                limit.refill.max(Duration::from_secs(1)),
            ));
        }
        let key = build_key(policy, identity)?;
        match tokio::time::timeout(self.timeout, self.store.check(&key, limit)).await {
            Ok(Ok(decision)) => Ok(decision),
            Ok(Err(_)) | Err(_) => match self.unavailable_policy {
                RateLimitUnavailablePolicy::FailOpen => Ok(RateLimitDecision::allowed(true)),
                RateLimitUnavailablePolicy::FailClosed => Err(RateLimitError::StoreUnavailable),
            },
        }
    }

    pub async fn health_check(&self) -> anyhow::Result<()> {
        tokio::time::timeout(self.timeout, self.store.health_check())
            .await
            .map_err(|_| anyhow::anyhow!("rate limit health check timed out"))?
    }
}

fn build_key(
    policy: &RateLimitKeyPolicy,
    identity: &RateLimitIdentity,
) -> Result<String, RateLimitError> {
    if policy.dimensions.is_empty() && policy.headers.is_empty() {
        return Err(RateLimitError::EmptyKeyPolicy);
    }
    let mut material = String::from("roze-rate-limit-key-v1");
    for dimension in &policy.dimensions {
        match dimension {
            RateLimitDimension::Route => {
                push_component(
                    &mut material,
                    "service",
                    Some(&identity.service),
                    policy.missing,
                )?;
                push_component(
                    &mut material,
                    "boundary",
                    Some(&identity.boundary),
                    policy.missing,
                )?;
                push_component(
                    &mut material,
                    "operation",
                    Some(&identity.operation),
                    policy.missing,
                )?;
            }
            RateLimitDimension::ClientIp => push_component(
                &mut material,
                "client_ip",
                identity.client_ip.as_deref(),
                policy.missing,
            )?,
            RateLimitDimension::Subject => push_component(
                &mut material,
                "subject",
                identity.subject.as_deref(),
                policy.missing,
            )?,
            RateLimitDimension::Tenant => push_component(
                &mut material,
                "tenant",
                identity.tenant.as_deref(),
                policy.missing,
            )?,
        }
    }
    for header in &policy.headers {
        let normalized = header.trim().to_ascii_lowercase();
        if !valid_header_name(&normalized) {
            return Err(RateLimitError::InvalidHeader(header.clone()));
        }
        push_component(
            &mut material,
            "header",
            identity.headers.get(&normalized).map(String::as_str),
            policy.missing,
        )?;
        push_component(
            &mut material,
            "header_name",
            Some(&normalized),
            policy.missing,
        )?;
    }
    let digest = Sha256::digest(material.as_bytes());
    let mut key = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        write!(&mut key, "{byte:02x}").expect("writing to String cannot fail");
    }
    Ok(key)
}

fn push_component(
    material: &mut String,
    name: &'static str,
    value: Option<&str>,
    missing: MissingDimensionPolicy,
) -> Result<(), RateLimitError> {
    let value = value.filter(|value| !value.is_empty());
    let Some(value) = value else {
        return match missing {
            MissingDimensionPolicy::Reject => Err(RateLimitError::MissingDimension(name)),
            MissingDimensionPolicy::Omit => Ok(()),
        };
    };
    material.push('|');
    material.push_str(name);
    material.push(':');
    material.push_str(&value.len().to_string());
    material.push(':');
    material.push_str(value);
    Ok(())
}

fn valid_header_name(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_' | b'.')
        })
}

#[derive(Debug, Clone, Copy)]
struct MemoryState {
    tokens: f64,
    last_refill: Instant,
}

#[derive(Debug, Default)]
pub struct InMemoryRateLimitStore {
    states: DashMap<String, MemoryState>,
}

#[async_trait]
impl RateLimitStore for InMemoryRateLimitStore {
    async fn check(&self, key: &str, limit: RateLimit) -> anyhow::Result<RateLimitDecision> {
        let now = Instant::now();
        let mut state = self.states.entry(key.to_string()).or_insert(MemoryState {
            tokens: f64::from(limit.burst),
            last_refill: now,
        });
        let refill_units =
            now.duration_since(state.last_refill).as_secs_f64() / limit.refill.as_secs_f64();
        state.tokens = (state.tokens + refill_units).min(f64::from(limit.burst));
        state.last_refill = now;
        if state.tokens >= 1.0 {
            state.tokens -= 1.0;
            return Ok(RateLimitDecision::allowed(false));
        }
        let retry = Duration::from_secs_f64((1.0 - state.tokens) * limit.refill.as_secs_f64());
        Ok(RateLimitDecision::denied(
            retry.max(Duration::from_millis(1)),
        ))
    }
}

#[derive(Clone)]
pub struct RedisRateLimitStore {
    client: roze_redis::RedisClient,
    key_prefix: String,
}

impl fmt::Debug for RedisRateLimitStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RedisRateLimitStore")
            .field("key_prefix", &self.key_prefix)
            .finish_non_exhaustive()
    }
}

impl RedisRateLimitStore {
    pub fn connect(url: &str, key_prefix: &str) -> anyhow::Result<Self> {
        Self::connect_topology(url, &[], key_prefix)
    }

    pub fn connect_topology(
        url: &str,
        cluster_urls: &[String],
        key_prefix: &str,
    ) -> anyhow::Result<Self> {
        anyhow::ensure!(
            !key_prefix.trim().is_empty(),
            "key prefix must not be empty"
        );
        Ok(Self {
            client: roze_redis::RedisClient::open_topology(url, cluster_urls)?,
            key_prefix: key_prefix.trim_end_matches(':').to_string(),
        })
    }

    async fn connection(&self) -> anyhow::Result<roze_redis::RedisConnection> {
        self.client.connection().await
    }
}

const REDIS_TOKEN_BUCKET: &str = r#"
local now = redis.call('TIME')
local now_ms = tonumber(now[1]) * 1000 + math.floor(tonumber(now[2]) / 1000)
local burst = tonumber(ARGV[1])
local refill_ms = tonumber(ARGV[2])
local tokens = tonumber(redis.call('HGET', KEYS[1], 'tokens') or burst)
local last_ms = tonumber(redis.call('HGET', KEYS[1], 'last_ms') or now_ms)
local elapsed = math.max(0, now_ms - last_ms)
tokens = math.min(burst, tokens + elapsed / refill_ms)
local allowed = 0
local retry_after_ms = 0
if tokens >= 1 then
  tokens = tokens - 1
  allowed = 1
else
  retry_after_ms = math.max(1, math.ceil((1 - tokens) * refill_ms))
end
redis.call('HSET', KEYS[1], 'tokens', tokens, 'last_ms', now_ms)
redis.call('PEXPIRE', KEYS[1], math.max(refill_ms * burst * 2, refill_ms + 1000))
return {allowed, retry_after_ms}
"#;

#[async_trait]
impl RateLimitStore for RedisRateLimitStore {
    async fn check(&self, key: &str, limit: RateLimit) -> anyhow::Result<RateLimitDecision> {
        let mut connection = self.connection().await?;
        let refill_ms = limit.refill.as_millis().max(1).min(u128::from(u64::MAX)) as u64;
        let (allowed, retry_after_ms): (i64, u64) =
            roze_redis::redis::Script::new(REDIS_TOKEN_BUCKET)
                .key(format!("{}:{key}", self.key_prefix))
                .arg(limit.burst)
                .arg(refill_ms)
                .invoke_async(&mut connection)
                .await?;
        Ok(if allowed == 1 {
            RateLimitDecision::allowed(false)
        } else {
            RateLimitDecision::denied(Duration::from_millis(retry_after_ms.max(1)))
        })
    }

    async fn health_check(&self) -> anyhow::Result<()> {
        let mut connection = self.connection().await?;
        let response: String = roze_redis::redis::cmd("PING")
            .query_async(&mut connection)
            .await?;
        anyhow::ensure!(
            response.eq_ignore_ascii_case("PONG"),
            "Redis rate limit health check returned an unexpected response"
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn route_identity(subject: &str) -> RateLimitIdentity {
        RateLimitIdentity::new("auth", "rest", "POST:/login")
            .with_client_ip(Some("203.0.113.8"))
            .with_subject(Some(subject.to_string()))
    }

    #[tokio::test]
    async fn different_subjects_do_not_share_memory_buckets() {
        let limiter = RateLimiter::from_config(&RateLimiterConfig::default()).unwrap();
        let policy = RateLimitKeyPolicy {
            dimensions: vec![RateLimitDimension::Route, RateLimitDimension::Subject],
            ..Default::default()
        };
        let limit = RateLimit {
            burst: 1,
            refill: Duration::from_secs(60),
        };
        assert!(
            limiter
                .check(&policy, &route_identity("alice"), limit)
                .await
                .unwrap()
                .allowed
        );
        assert!(
            limiter
                .check(&policy, &route_identity("bob"), limit)
                .await
                .unwrap()
                .allowed
        );
        assert!(
            !limiter
                .check(&policy, &route_identity("alice"), limit)
                .await
                .unwrap()
                .allowed
        );
    }

    #[test]
    fn raw_identity_values_are_not_present_in_storage_keys() {
        let policy = RateLimitKeyPolicy {
            dimensions: vec![
                RateLimitDimension::Route,
                RateLimitDimension::ClientIp,
                RateLimitDimension::Subject,
            ],
            ..Default::default()
        };
        let key = build_key(&policy, &route_identity("alice@example.com")).unwrap();
        assert_eq!(key.len(), 64);
        assert!(!key.contains("alice"));
        assert!(!key.contains("203.0.113.8"));
    }

    #[test]
    fn missing_required_dimension_is_rejected() {
        let policy = RateLimitKeyPolicy {
            dimensions: vec![RateLimitDimension::Tenant],
            ..Default::default()
        };
        assert!(matches!(
            build_key(&policy, &route_identity("alice")),
            Err(RateLimitError::MissingDimension("tenant"))
        ));
    }

    #[test]
    fn auto_store_and_key_policy_validation_are_deterministic() {
        let mut config = RateLimiterConfig::default();
        assert_eq!(config.resolved_store_kind(), RateLimitStoreKind::Memory);
        config.redis_url = Some("redis://127.0.0.1:6379".to_string());
        config.namespace = Some("production".to_string());
        assert_eq!(config.resolved_store_kind(), RateLimitStoreKind::Redis);
        config.validate().expect("valid auto Redis config");

        let duplicate_dimensions = RateLimitKeyPolicy {
            dimensions: vec![RateLimitDimension::Route, RateLimitDimension::Route],
            ..Default::default()
        };
        assert!(duplicate_dimensions.validate().is_err());
        let duplicate_headers = RateLimitKeyPolicy {
            dimensions: Vec::new(),
            headers: vec!["X-API-Key".to_string(), "x-api-key".to_string()],
            ..Default::default()
        };
        assert!(duplicate_headers.validate().is_err());
    }

    #[derive(Debug)]
    struct FailingStore;

    #[async_trait]
    impl RateLimitStore for FailingStore {
        async fn check(&self, _key: &str, _limit: RateLimit) -> anyhow::Result<RateLimitDecision> {
            anyhow::bail!("simulated store failure")
        }
    }

    #[tokio::test]
    async fn store_failures_follow_open_and_closed_policy() {
        let identity = route_identity("alice");
        let limit = RateLimit {
            burst: 1,
            refill: Duration::from_secs(1),
        };
        let open = RateLimiter::with_store(
            Arc::new(FailingStore),
            Duration::from_millis(100),
            RateLimitUnavailablePolicy::FailOpen,
        )
        .unwrap();
        let decision = open
            .check(&RateLimitKeyPolicy::default(), &identity, limit)
            .await
            .unwrap();
        assert!(decision.allowed);
        assert!(decision.degraded);

        let closed = RateLimiter::with_store(
            Arc::new(FailingStore),
            Duration::from_millis(100),
            RateLimitUnavailablePolicy::FailClosed,
        )
        .unwrap();
        assert!(matches!(
            closed
                .check(&RateLimitKeyPolicy::default(), &identity, limit)
                .await,
            Err(RateLimitError::StoreUnavailable)
        ));
    }

    #[tokio::test]
    #[ignore = "requires ROZE_TEST_REDIS_URL"]
    async fn two_instances_share_atomic_redis_quota_across_restart() {
        let Ok(url) = std::env::var("ROZE_TEST_REDIS_URL") else {
            return;
        };
        let unique = format!(
            "roze:test:rate-limit:{}:{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let config = RateLimiterConfig {
            store: RateLimitStoreKind::Redis,
            redis_url: Some(url),
            redis_cluster_urls: Vec::new(),
            key_prefix: unique,
            namespace: None,
            timeout_ms: 1_000,
            unavailable_policy: RateLimitUnavailablePolicy::FailClosed,
        };
        let first = Arc::new(RateLimiter::from_config(&config).unwrap());
        let second = Arc::new(RateLimiter::from_config(&config).unwrap());
        first.health_check().await.unwrap();
        let policy = RateLimitKeyPolicy {
            dimensions: vec![RateLimitDimension::Route, RateLimitDimension::ClientIp],
            ..Default::default()
        };
        let identity = route_identity("ignored");
        let limit = RateLimit {
            burst: 8,
            refill: Duration::from_secs(60),
        };
        let allowed = Arc::new(AtomicUsize::new(0));
        let mut tasks = Vec::new();
        for index in 0..32 {
            let limiter = if index % 2 == 0 {
                first.clone()
            } else {
                second.clone()
            };
            let policy = policy.clone();
            let identity = identity.clone();
            let allowed = allowed.clone();
            tasks.push(tokio::spawn(async move {
                if limiter
                    .check(&policy, &identity, limit)
                    .await
                    .unwrap()
                    .allowed
                {
                    allowed.fetch_add(1, Ordering::Relaxed);
                }
            }));
        }
        for task in tasks {
            task.await.unwrap();
        }
        assert_eq!(allowed.load(Ordering::Relaxed), 8);

        drop(first);
        let restarted = RateLimiter::from_config(&config).unwrap();
        assert!(
            !restarted
                .check(&policy, &identity, limit)
                .await
                .unwrap()
                .allowed,
            "recreating one instance must not reset shared quota"
        );
    }
}
