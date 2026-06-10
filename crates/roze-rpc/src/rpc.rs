use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, Instant},
};

use crate::{
    balance::Balancer,
    registry::{CachedRegistryResolver, Registry, ServiceInstance},
};
use roze_auth::principal_from_claims;
use roze_context::Context;
use roze_grpc::transport::{Channel, Code, Endpoint, MetadataValue, Request, Server, Status};
use roze_jwt::{extract_bearer_token, verify_token, JwtConfig};
use roze_metrics::record_rpc_method;
use roze_trace::{generate_trace_id, TRACE_ID_HEADER};
use tokio::time::sleep;
use tracing::info;

static METHOD_RATE_LIMITS: OnceLock<Mutex<HashMap<String, MethodRateLimitState>>> = OnceLock::new();
static METHOD_BREAKERS: OnceLock<Mutex<HashMap<String, MethodBreakerState>>> = OnceLock::new();

#[derive(Debug, Clone)]
pub struct RpcConfig {
    pub addr: SocketAddr,
}

pub struct RpcServer {
    config: RpcConfig,
}

pub struct ServiceRegistrationGuard {
    registry: Arc<dyn Registry>,
    service_name: String,
    addr: String,
    active: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct RpcClientOptions {
    pub connect_timeout: Duration,
    pub request_timeout: Duration,
    pub keepalive_time: Duration,
    pub max_retries: usize,
    pub retry_backoff: Duration,
    pub non_block: bool,
    pub trace: bool,
    pub stat: bool,
    pub prometheus: bool,
    pub breaker: bool,
}

impl Default for RpcClientOptions {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(5),
            request_timeout: Duration::from_millis(2_000),
            keepalive_time: Duration::from_secs(20),
            max_retries: 1,
            retry_backoff: Duration::from_millis(100),
            non_block: false,
            trace: true,
            stat: true,
            prometheus: true,
            breaker: true,
        }
    }
}

impl RpcClientOptions {
    pub fn from_config(config: &roze_config::RpcClientConfig) -> Self {
        Self {
            request_timeout: Duration::from_millis(config.timeout_ms),
            keepalive_time: Duration::from_secs(config.keepalive_time_secs),
            non_block: config.non_block,
            trace: config.middlewares.trace,
            stat: config.middlewares.stat,
            prometheus: config.middlewares.prometheus,
            breaker: config.middlewares.breaker,
            ..Self::default()
        }
    }
}

impl RpcServer {
    pub fn new(addr: SocketAddr) -> Self {
        Self {
            config: RpcConfig { addr },
        }
    }

    pub fn builder(&self) -> Server {
        info!(addr = %self.config.addr, "building RPC server");
        Server::builder()
    }
}

impl ServiceRegistrationGuard {
    pub async fn start(
        registry: Arc<dyn Registry>,
        service_name: impl Into<String>,
        addr: SocketAddr,
    ) -> anyhow::Result<Self> {
        let service_name = service_name.into();
        let addr = addr.to_string();
        registry
            .register(ServiceInstance::new(service_name.clone(), addr.clone()))
            .await?;
        Ok(Self {
            registry,
            service_name,
            addr,
            active: true,
        })
    }

    pub async fn shutdown(&mut self) -> anyhow::Result<()> {
        if self.active {
            self.registry
                .deregister(&self.service_name, &self.addr)
                .await?;
            self.active = false;
        }
        Ok(())
    }
}

impl Drop for ServiceRegistrationGuard {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        self.active = false;
        let registry = Arc::clone(&self.registry);
        let service_name = self.service_name.clone();
        let addr = self.addr.clone();
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                let _ = registry.deregister(&service_name, &addr).await;
            });
        }
    }
}

pub fn auth_interceptor(
    config: JwtConfig,
) -> impl FnMut(Request<()>) -> Result<Request<()>, Status> + Clone {
    move |mut req: Request<()>| {
        let trace_id = trace_id_from_metadata(&req).unwrap_or_else(generate_trace_id);
        let header_value = req
            .metadata()
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            .ok_or_else(|| Status::unauthenticated("missing authorization header"))?;
        let token = extract_bearer_token(header_value)
            .ok_or_else(|| Status::unauthenticated("missing bearer token"))?;
        let claims =
            verify_token(token, &config).map_err(|err| Status::unauthenticated(err.to_string()))?;
        let subject = MetadataValue::try_from(claims.sub.as_str())
            .map_err(|_| Status::unauthenticated("invalid subject"))?;
        req.metadata_mut().insert("x-subject", subject);
        req.extensions_mut().insert(principal_from_claims(&claims));
        req.metadata_mut().insert(
            TRACE_ID_HEADER,
            MetadataValue::try_from(trace_id.as_str())
                .map_err(|_| Status::unauthenticated("invalid trace id"))?,
        );
        req.extensions_mut()
            .insert(Context::background_with_trace_id(trace_id));
        Ok(req)
    }
}

pub async fn connect_channel(addr: impl AsRef<str>) -> anyhow::Result<Channel> {
    connect_channel_with_options(addr, RpcClientOptions::default()).await
}

pub async fn connect_channel_with_options(
    addr: impl AsRef<str>,
    options: RpcClientOptions,
) -> anyhow::Result<Channel> {
    let url = normalize_endpoint(addr.as_ref())?;
    let channel = Endpoint::from_shared(url)?
        .connect_timeout(options.connect_timeout)
        .timeout(options.request_timeout)
        .http2_keep_alive_interval(options.keepalive_time)
        .connect()
        .await?;
    Ok(channel)
}

pub async fn connect_channel_from_config(
    config: &roze_config::RpcClientConfig,
) -> anyhow::Result<Channel> {
    let target = rpc_client_target(config)?;
    connect_channel_with_options(target, RpcClientOptions::from_config(config)).await
}

pub fn rpc_client_target(config: &roze_config::RpcClientConfig) -> anyhow::Result<&str> {
    if let Some(target) = config.target.as_deref().filter(|target| !target.is_empty()) {
        return Ok(target);
    }
    if let Some(endpoint) = config
        .endpoints
        .iter()
        .map(String::as_str)
        .find(|endpoint| !endpoint.is_empty())
    {
        return Ok(endpoint);
    }
    if config.etcd.is_some() {
        anyhow::bail!(
            "rpc client etcd discovery config requires generated client registry resolver connection"
        );
    }
    anyhow::bail!("rpc client config must set target or at least one endpoint")
}

pub async fn connect_via_registry<R, B>(
    service: &str,
    registry: &R,
    balancer: &B,
) -> anyhow::Result<Channel>
where
    R: Registry,
    B: Balancer,
{
    connect_via_registry_with_options(service, registry, balancer, RpcClientOptions::default())
        .await
}

pub async fn connect_via_registry_with_options<R, B>(
    service: &str,
    registry: &R,
    balancer: &B,
    options: RpcClientOptions,
) -> anyhow::Result<Channel>
where
    R: Registry,
    B: Balancer,
{
    let instances = registry.discover(service).await?;
    let instance = balancer
        .pick(&instances)
        .ok_or_else(|| anyhow::anyhow!("no available instances for service `{service}`"))?;
    connect_channel_with_options(instance.addr, options).await
}

pub async fn connect_via_cached_registry_with_options<R, B>(
    service: &str,
    resolver: &CachedRegistryResolver<R, B>,
    options: RpcClientOptions,
) -> anyhow::Result<Channel>
where
    R: Registry,
    B: Balancer,
{
    let instance = resolver
        .pick(service)
        .await?
        .ok_or_else(|| anyhow::anyhow!("no available instances for service `{service}`"))?;
    connect_channel_with_options(instance.addr, options).await
}

pub fn normalize_endpoint(addr: &str) -> anyhow::Result<String> {
    if addr.starts_with("http://") || addr.starts_with("https://") {
        Ok(addr.to_string())
    } else {
        Ok(format!("http://{addr}"))
    }
}

pub fn should_retry_status(status: &Status) -> bool {
    matches!(
        status.code(),
        Code::Unavailable | Code::DeadlineExceeded | Code::Unknown
    )
}

pub fn trace_id_from_metadata(request: &Request<()>) -> Option<String> {
    request
        .metadata()
        .get(TRACE_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

pub fn request_context<T>(request: &Request<T>) -> Context {
    request
        .extensions()
        .get::<Context>()
        .cloned()
        .unwrap_or_else(|| {
            let trace_id = request
                .metadata()
                .get(TRACE_ID_HEADER)
                .and_then(|value| value.to_str().ok())
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
                .unwrap_or_else(generate_trace_id);
            let ctx = Context::background_with_trace_id(trace_id);
            match request
                .metadata()
                .get(roze_context::TIMEOUT_HEADER)
                .and_then(|value| value.to_str().ok())
                .and_then(|raw| raw.parse::<u64>().ok())
                .map(Duration::from_millis)
            {
                Some(timeout) => ctx.with_timeout(timeout),
                None => ctx,
            }
        })
}

pub fn apply_request_context<T>(request: &mut Request<T>, context: &Context) {
    let trace_id = context.trace_id();
    if let Ok(value) = MetadataValue::try_from(trace_id.as_str()) {
        request.metadata_mut().insert(TRACE_ID_HEADER, value);
    }
    if let Some(timeout) = context.remaining_timeout() {
        let timeout_ms = timeout.as_millis().to_string();
        if let Ok(value) = MetadataValue::try_from(timeout_ms.as_str()) {
            request
                .metadata_mut()
                .insert(roze_context::TIMEOUT_HEADER, value);
        }
    }
}

pub fn apply_client_auth<T>(
    request: &mut Request<T>,
    options: &RpcClientOptions,
    config: Option<&roze_config::RpcClientConfig>,
) {
    if !options.trace {
        request.metadata_mut().remove(TRACE_ID_HEADER);
    }
    let Some(config) = config else {
        return;
    };
    if let Some(app) = config.app.as_deref().filter(|app| !app.is_empty()) {
        if let Ok(value) = MetadataValue::try_from(app) {
            request.metadata_mut().insert("x-app", value);
        }
    }
    if let Some(token) = config.token.as_deref().filter(|token| !token.is_empty()) {
        let authorization = format!("Bearer {token}");
        if let Ok(value) = MetadataValue::try_from(authorization.as_str()) {
            request.metadata_mut().insert("authorization", value);
        }
    }
}

#[derive(Debug, Clone)]
pub struct MethodPolicy {
    pub timeout: Option<Duration>,
    pub rate_limit: Option<MethodRateLimitConfig>,
    pub breaker: Option<MethodBreakerConfig>,
}

#[derive(Debug, Clone, Copy)]
pub struct MethodRateLimitConfig {
    pub burst: u32,
    pub refill: Duration,
}

#[derive(Debug, Clone, Copy)]
pub struct MethodBreakerConfig {
    pub failure_threshold: u32,
    pub reset_timeout: Duration,
}

#[derive(Debug, Clone)]
pub struct MethodGuard {
    key: String,
    service: String,
    method: String,
    started_at: Instant,
    breaker: Option<MethodBreakerConfig>,
}

#[derive(Debug)]
struct MethodRateLimitState {
    tokens: f64,
    last_refill: Instant,
}

#[derive(Debug)]
struct MethodBreakerState {
    failures: u32,
    open_until: Option<Instant>,
}

pub fn method_policy(
    governance: Option<&roze_config::GovernanceConfig>,
    method: &str,
) -> MethodPolicy {
    let Some(governance) = governance else {
        return MethodPolicy {
            timeout: None,
            rate_limit: None,
            breaker: None,
        };
    };
    let method_config = governance.routes.get(method);
    MethodPolicy {
        timeout: method_config
            .and_then(|route| route.timeout_ms)
            .or(governance.timeout_ms)
            .map(Duration::from_millis),
        rate_limit: method_config
            .and_then(|route| route.rate_limit)
            .or(governance.rate_limit)
            .map(|config| MethodRateLimitConfig {
                burst: config.burst,
                refill: Duration::from_millis(config.refill_ms),
            }),
        breaker: method_config
            .and_then(|route| route.breaker)
            .or(governance.breaker)
            .map(|config| MethodBreakerConfig {
                failure_threshold: config.failure_threshold,
                reset_timeout: Duration::from_millis(config.reset_timeout_ms),
            }),
    }
}

pub fn begin_method(
    service: impl Into<String>,
    method: impl Into<String>,
    request_ctx: Context,
    governance: Option<&roze_config::GovernanceConfig>,
) -> Result<(Context, MethodGuard), Status> {
    let service = service.into();
    let method = method.into();
    let policy = method_policy(governance, &method);
    let key = format!("{service}:{method}");
    if let Some(config) = &policy.rate_limit {
        enforce_method_rate_limit(&key, config)?;
    }
    if policy
        .breaker
        .as_ref()
        .is_some_and(|_| method_breaker_is_open(&key))
    {
        return Err(Status::unavailable("circuit open"));
    }
    let request_ctx = match policy.timeout {
        Some(timeout) => request_ctx.with_timeout(timeout),
        None => request_ctx,
    };
    Ok((
        request_ctx,
        MethodGuard {
            key,
            service,
            method,
            started_at: Instant::now(),
            breaker: policy.breaker,
        },
    ))
}

pub fn finish_method(guard: MethodGuard, code: impl Into<String>) {
    let code = code.into();
    let success = code == "ok";
    record_rpc_method(
        guard.service,
        guard.method,
        code,
        guard.started_at.elapsed(),
    );
    if let Some(config) = guard.breaker {
        method_breaker_record(&guard.key, success, &config);
    }
}

pub async fn retry_status<F, Fut, T>(mut call: F, options: RpcClientOptions) -> Result<T, Status>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, Status>>,
{
    let mut attempt = 0usize;
    loop {
        let response = call().await;
        match response {
            Ok(value) => return Ok(value),
            Err(status) if attempt < options.max_retries && should_retry_status(&status) => {
                attempt += 1;
                sleep(retry_delay(options.retry_backoff, attempt)).await;
            }
            Err(status) => return Err(status),
        }
    }
}

fn retry_delay(base: Duration, attempt: usize) -> Duration {
    let factor = attempt.max(1) as u32;
    base.saturating_mul(factor)
}

fn enforce_method_rate_limit(key: &str, config: &MethodRateLimitConfig) -> Result<(), Status> {
    let mut states = METHOD_RATE_LIMITS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .expect("method rate limit lock poisoned");
    let state = states
        .entry(key.to_string())
        .or_insert_with(|| MethodRateLimitState {
            tokens: config.burst as f64,
            last_refill: Instant::now(),
        });
    refill_method_tokens(state, config);
    if state.tokens >= 1.0 {
        state.tokens -= 1.0;
        Ok(())
    } else {
        Err(Status::resource_exhausted("rate limited"))
    }
}

fn refill_method_tokens(state: &mut MethodRateLimitState, config: &MethodRateLimitConfig) {
    let refill_secs = config.refill.as_secs_f64();
    if refill_secs <= 0.0 {
        state.tokens = config.burst as f64;
        state.last_refill = Instant::now();
        return;
    }
    let now = Instant::now();
    let elapsed = now.duration_since(state.last_refill).as_secs_f64();
    let tokens_to_add = elapsed / refill_secs;
    if tokens_to_add > 0.0 {
        state.tokens = (state.tokens + tokens_to_add).min(config.burst as f64);
        state.last_refill = now;
    }
}

fn method_breaker_is_open(key: &str) -> bool {
    let mut states = METHOD_BREAKERS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .expect("method breaker lock poisoned");
    let state = states
        .entry(key.to_string())
        .or_insert_with(|| MethodBreakerState {
            failures: 0,
            open_until: None,
        });
    if let Some(open_until) = state.open_until {
        if Instant::now() < open_until {
            return true;
        }
        state.open_until = None;
        state.failures = 0;
    }
    false
}

fn method_breaker_record(key: &str, success: bool, config: &MethodBreakerConfig) {
    let mut states = METHOD_BREAKERS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .expect("method breaker lock poisoned");
    let state = states
        .entry(key.to_string())
        .or_insert_with(|| MethodBreakerState {
            failures: 0,
            open_until: None,
        });
    if success {
        state.failures = 0;
        state.open_until = None;
        return;
    }
    state.failures = state.failures.saturating_add(1);
    if state.failures >= config.failure_threshold.max(1) {
        state.failures = 0;
        state.open_until = Some(Instant::now() + config.reset_timeout);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    use super::*;

    #[test]
    fn retry_status_targets_transient_errors() {
        assert!(should_retry_status(&Status::unavailable("down")));
        assert!(should_retry_status(&Status::deadline_exceeded("slow")));
        assert!(should_retry_status(&Status::new(Code::Unknown, "unknown")));
        assert!(!should_retry_status(&Status::invalid_argument(
            "bad request"
        )));
    }

    #[tokio::test]
    async fn retry_status_retries_once() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let attempts_clone = attempts.clone();
        let result = retry_status(
            move || {
                let attempts = attempts_clone.clone();
                async move {
                    let current = attempts.fetch_add(1, Ordering::SeqCst);
                    if current == 0 {
                        Err(Status::unavailable("temporary"))
                    } else {
                        Ok("ok")
                    }
                }
            },
            RpcClientOptions {
                connect_timeout: Duration::from_secs(1),
                request_timeout: Duration::from_secs(1),
                max_retries: 1,
                retry_backoff: Duration::from_millis(0),
                ..RpcClientOptions::default()
            },
        )
        .await
        .expect("retry should succeed");

        assert_eq!(result, "ok");
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn request_context_prefers_metadata_trace_id() {
        let mut request = Request::new(());
        request.metadata_mut().insert(
            TRACE_ID_HEADER,
            MetadataValue::try_from("trace-abc").unwrap(),
        );

        let context = request_context(&request);
        assert_eq!(context.trace_id(), "trace-abc");
    }

    #[test]
    fn apply_request_context_sets_trace_id_metadata() {
        let mut request = Request::new(());
        let context = Context::background_with_trace_id("trace-xyz");

        apply_request_context(&mut request, &context);

        let trace_id = request
            .metadata()
            .get(TRACE_ID_HEADER)
            .and_then(|value| value.to_str().ok())
            .expect("trace id metadata");
        assert_eq!(trace_id, "trace-xyz");
    }

    #[test]
    fn rpc_client_options_follow_config_defaults() {
        let config = roze_config::RpcClientConfig {
            etcd: None,
            endpoints: vec!["127.0.0.1:4000".to_string()],
            target: None,
            app: None,
            token: None,
            non_block: true,
            timeout_ms: 2_500,
            keepalive_time_secs: 30,
            middlewares: roze_config::RpcClientMiddlewaresConfig {
                trace: false,
                ..Default::default()
            },
        };

        let options = RpcClientOptions::from_config(&config);
        assert_eq!(options.request_timeout, Duration::from_millis(2_500));
        assert_eq!(options.keepalive_time, Duration::from_secs(30));
        assert!(options.non_block);
        assert!(!options.trace);
        assert_eq!(
            rpc_client_target(&config).expect("target"),
            "127.0.0.1:4000"
        );
    }

    #[test]
    fn apply_client_auth_sets_app_and_token_metadata() {
        let config = roze_config::RpcClientConfig {
            etcd: None,
            endpoints: Vec::new(),
            target: Some("dns:///user.rpc".to_string()),
            app: Some("admin".to_string()),
            token: Some("secret".to_string()),
            non_block: false,
            timeout_ms: 2_000,
            keepalive_time_secs: 20,
            middlewares: Default::default(),
        };
        let options = RpcClientOptions::from_config(&config);
        let mut request = Request::new(());

        apply_client_auth(&mut request, &options, Some(&config));

        assert_eq!(
            request
                .metadata()
                .get("x-app")
                .and_then(|value| value.to_str().ok()),
            Some("admin")
        );
        assert_eq!(
            request
                .metadata()
                .get("authorization")
                .and_then(|value| value.to_str().ok()),
            Some("Bearer secret")
        );
    }

    #[test]
    fn method_policy_prefers_method_override() {
        let mut governance = roze_config::GovernanceConfig::default();
        governance.timeout_ms = Some(1000);
        governance.routes.insert(
            "GetUser".into(),
            roze_config::RouteGovernanceConfig {
                timeout_ms: Some(50),
                rate_limit: None,
                breaker: None,
            },
        );

        let policy = method_policy(Some(&governance), "GetUser");

        assert_eq!(policy.timeout, Some(Duration::from_millis(50)));
    }

    #[tokio::test]
    async fn registration_guard_registers_and_shutdown_deregisters() {
        let registry = Arc::new(crate::registry::MemoryRegistry::default());
        let mut guard = ServiceRegistrationGuard::start(
            registry.clone(),
            "svc",
            "127.0.0.1:9000".parse().unwrap(),
        )
        .await
        .expect("start");

        assert_eq!(registry.discover("svc").await.expect("discover").len(), 1);
        guard.shutdown().await.expect("shutdown");
        assert!(registry.discover("svc").await.expect("discover").is_empty());
    }
}
