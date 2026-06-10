use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::{
    future::Future,
    time::{Duration, Instant},
};

use poem::{
    http::{
        header::{HeaderName, HeaderValue},
        Method,
    },
    middleware::{CatchPanic, Cors},
    Endpoint, EndpointExt, Middleware, Request, Result,
};
use tracing::Instrument;

use roze_auth::principal_from_claims;
use roze_config::{GovernanceConfig, RouteGovernanceConfig};
use roze_context::Context;
use roze_error::RozeError;
use roze_jwt::{extract_bearer_token, verify_token, JwtConfig};
use roze_metrics::{record_http_request, record_http_route};
use roze_trace::{generate_trace_id, request_span, TRACE_ID_HEADER};

static ROUTE_RATE_LIMITS: OnceLock<Mutex<HashMap<String, RateLimitState>>> = OnceLock::new();
static ROUTE_BREAKERS: OnceLock<Mutex<HashMap<String, BreakerState>>> = OnceLock::new();

#[derive(Debug, Clone, Copy, Default)]
pub struct TraceMiddleware;

impl<E> Middleware<E> for TraceMiddleware
where
    E: Endpoint,
{
    type Output = TraceEndpoint<E>;

    fn transform(&self, ep: E) -> Self::Output {
        TraceEndpoint { ep }
    }
}

pub struct TraceEndpoint<E> {
    ep: E,
}

impl<E> Endpoint for TraceEndpoint<E>
where
    E: Endpoint,
{
    type Output = E::Output;

    fn call(&self, req: Request) -> impl Future<Output = Result<Self::Output>> + Send {
        let mut req = req;
        let method = req.method().clone();
        let uri = req.uri().clone();
        let trace_id = ensure_trace_id_header(&mut req);
        let span = request_span(method.as_str(), uri.path(), &trace_id);

        async move {
            let start = std::time::Instant::now();
            let response = self.ep.call(req).await;
            record_http_request(response.is_ok(), start.elapsed());
            match &response {
                Ok(_) => {
                    tracing::info!(
                        elapsed_ms = start.elapsed().as_millis(),
                        "request completed"
                    );
                }
                Err(err) => {
                    tracing::warn!(
                        error = %err,
                        elapsed_ms = start.elapsed().as_millis(),
                        "request failed"
                    );
                }
            }
            response
        }
        .instrument(span)
    }
}

#[derive(Debug, Clone)]
pub struct AuthMiddleware {
    config: JwtConfig,
}

impl AuthMiddleware {
    pub fn new(config: JwtConfig) -> Self {
        Self { config }
    }
}

impl<E> Middleware<E> for AuthMiddleware
where
    E: Endpoint,
{
    type Output = AuthEndpoint<E>;

    fn transform(&self, ep: E) -> Self::Output {
        AuthEndpoint {
            ep,
            config: self.config.clone(),
        }
    }
}

pub struct AuthEndpoint<E> {
    ep: E,
    config: JwtConfig,
}

impl<E> Endpoint for AuthEndpoint<E>
where
    E: Endpoint,
{
    type Output = E::Output;

    fn call(&self, mut req: Request) -> impl Future<Output = Result<Self::Output>> + Send {
        let config = self.config.clone();
        let ep = &self.ep;

        async move {
            let header_value = req
                .headers()
                .get("authorization")
                .and_then(|value| value.to_str().ok())
                .ok_or_else(|| RozeError::Unauthorized)?;
            let token =
                extract_bearer_token(header_value).ok_or_else(|| RozeError::Unauthorized)?;
            let claims =
                verify_token(token, &config).map_err(|err| RozeError::Internal(err.to_string()))?;
            req.extensions_mut().insert(principal_from_claims(&claims));
            ep.call(req).await
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct RequestIdMiddleware;

impl<E> Middleware<E> for RequestIdMiddleware
where
    E: Endpoint,
{
    type Output = RequestIdEndpoint<E>;

    fn transform(&self, ep: E) -> Self::Output {
        RequestIdEndpoint { ep }
    }
}

pub struct RequestIdEndpoint<E> {
    ep: E,
}

impl<E> Endpoint for RequestIdEndpoint<E>
where
    E: Endpoint,
{
    type Output = E::Output;

    fn call(&self, mut req: Request) -> impl Future<Output = Result<Self::Output>> + Send {
        let request_id = next_request_id();
        let trace_id = incoming_trace_id(&req).unwrap_or_else(|| request_id.clone());
        let context = incoming_timeout(&req)
            .map(|timeout| {
                Context::background_with_trace_id(trace_id.clone()).with_timeout(timeout)
            })
            .unwrap_or_else(|| Context::background_with_trace_id(trace_id.clone()));
        req.extensions_mut().insert(context);
        req.extensions_mut().insert(RequestContext {
            request_id: request_id.clone(),
            trace_id,
        });
        let ep = &self.ep;

        async move { ep.call(req).await }
    }
}

pub fn trace() -> TraceMiddleware {
    TraceMiddleware
}

pub fn auth(config: JwtConfig) -> AuthMiddleware {
    AuthMiddleware::new(config)
}

pub fn rate_limit(config: RateLimitConfig) -> RateLimitMiddleware {
    RateLimitMiddleware::new(config)
}

pub fn breaker(config: BreakerConfig) -> BreakerMiddleware {
    BreakerMiddleware::new(config)
}

pub fn apply_common<E>(endpoint: E) -> impl Endpoint
where
    E: Endpoint,
{
    endpoint
        .with(RequestIdMiddleware)
        .with(CatchPanic::new())
        .with(Cors::new().allow_origin_regex(".*").allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::DELETE,
            Method::OPTIONS,
        ]))
        .with(TraceMiddleware)
}

pub fn apply_auth<E>(endpoint: E, config: JwtConfig) -> impl Endpoint
where
    E: Endpoint,
{
    endpoint.with(AuthMiddleware::new(config))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltInMiddleware {
    Auth,
    Timeout,
    RateLimit,
    Breaker,
}

impl BuiltInMiddleware {
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "auth" => Some(Self::Auth),
            "timeout" => Some(Self::Timeout),
            "rate_limit" | "ratelimit" => Some(Self::RateLimit),
            "breaker" | "circuit_breaker" => Some(Self::Breaker),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MiddlewarePlan {
    pub builtins: Vec<BuiltInMiddleware>,
    pub custom: Vec<String>,
}

pub fn resolve_middleware_plan(names: &[String]) -> MiddlewarePlan {
    let mut plan = MiddlewarePlan::default();
    for name in names {
        match BuiltInMiddleware::parse(name) {
            Some(builtin) if !plan.builtins.contains(&builtin) => plan.builtins.push(builtin),
            Some(_) => {}
            None if !plan.custom.contains(name) => plan.custom.push(name.clone()),
            None => {}
        }
    }
    plan
}

#[derive(Debug, Clone)]
pub struct RoutePolicy {
    pub timeout: Option<Duration>,
    pub rate_limit: Option<RateLimitConfig>,
    pub breaker: Option<BreakerConfig>,
}

#[derive(Debug, Clone)]
pub struct RouteGuard {
    key: String,
    service: String,
    route: String,
    method: String,
    started_at: Instant,
    breaker: Option<BreakerConfig>,
}

pub fn route_policy(governance: Option<&GovernanceConfig>, route: &str) -> RoutePolicy {
    let Some(governance) = governance else {
        return RoutePolicy {
            timeout: None,
            rate_limit: None,
            breaker: None,
        };
    };
    let route_config = governance.routes.get(route);
    RoutePolicy {
        timeout: route_config
            .and_then(|route| route.timeout_ms)
            .or(governance.timeout_ms)
            .map(Duration::from_millis),
        rate_limit: effective_rate_limit(governance, route_config),
        breaker: effective_breaker(governance, route_config),
    }
}

pub fn begin_route(
    service: impl Into<String>,
    route: impl Into<String>,
    method: impl Into<String>,
    request_ctx: Context,
    governance: Option<&GovernanceConfig>,
) -> Result<(Context, RouteGuard), RozeError> {
    let service = service.into();
    let route = route.into();
    let method = method.into();
    let policy = route_policy(governance, &route);
    let key = format!("{service}:{method}:{route}");

    if let Some(config) = &policy.rate_limit {
        enforce_rate_limit(&key, config)?;
    }
    if policy
        .breaker
        .as_ref()
        .is_some_and(|_| route_breaker_is_open(&key))
    {
        return Err(RozeError::Internal("circuit open".to_string()));
    }

    let request_ctx = match policy.timeout {
        Some(timeout) => request_ctx.with_timeout(timeout),
        None => request_ctx,
    };

    Ok((
        request_ctx,
        RouteGuard {
            key,
            service,
            route,
            method,
            started_at: Instant::now(),
            breaker: policy.breaker,
        },
    ))
}

pub fn finish_route(guard: RouteGuard, success: bool, status: impl Into<String>) {
    record_http_route(
        guard.service,
        guard.route,
        guard.method,
        status,
        guard.started_at.elapsed(),
    );
    if let Some(config) = guard.breaker {
        route_breaker_record(&guard.key, success, &config);
    }
}

#[derive(Debug, Clone)]
pub struct RateLimitConfig {
    pub burst: u32,
    pub refill: Duration,
}

#[derive(Debug, Clone)]
pub struct BreakerConfig {
    pub failure_threshold: u32,
    pub reset_timeout: Duration,
}

#[derive(Debug, Clone)]
pub struct RequestContext {
    pub request_id: String,
    pub trace_id: String,
}

#[derive(Debug, Clone)]
pub struct RateLimitMiddleware {
    config: RateLimitConfig,
    state: Arc<Mutex<RateLimitState>>,
}

#[derive(Debug)]
struct RateLimitState {
    tokens: f64,
    last_refill: Instant,
}

#[derive(Debug, Clone)]
pub struct RateLimitEndpoint<E> {
    ep: E,
    config: RateLimitConfig,
    state: Arc<Mutex<RateLimitState>>,
}

#[derive(Debug, Clone)]
pub struct BreakerMiddleware {
    config: BreakerConfig,
    state: Arc<Mutex<BreakerState>>,
}

#[derive(Debug)]
struct BreakerState {
    failures: u32,
    open_until: Option<Instant>,
}

#[derive(Debug, Clone)]
pub struct BreakerEndpoint<E> {
    ep: E,
    config: BreakerConfig,
    state: Arc<Mutex<BreakerState>>,
}

impl RateLimitMiddleware {
    pub fn new(config: RateLimitConfig) -> Self {
        Self {
            state: Arc::new(Mutex::new(RateLimitState {
                tokens: config.burst as f64,
                last_refill: Instant::now(),
            })),
            config,
        }
    }
}

impl<E> Middleware<E> for RateLimitMiddleware
where
    E: Endpoint,
{
    type Output = RateLimitEndpoint<E>;

    fn transform(&self, ep: E) -> Self::Output {
        RateLimitEndpoint {
            ep,
            config: self.config.clone(),
            state: Arc::clone(&self.state),
        }
    }
}

impl<E> Endpoint for RateLimitEndpoint<E>
where
    E: Endpoint,
{
    type Output = E::Output;

    fn call(&self, req: Request) -> impl Future<Output = Result<Self::Output>> + Send {
        let allowed = {
            let mut state = self.state.lock().expect("rate limit lock poisoned");
            refill_tokens(&mut state, &self.config);
            if state.tokens >= 1.0 {
                state.tokens -= 1.0;
                true
            } else {
                false
            }
        };

        async move {
            if !allowed {
                return Err(RozeError::Internal("rate limited".to_string()).into());
            }
            self.ep.call(req).await
        }
    }
}

impl BreakerMiddleware {
    pub fn new(config: BreakerConfig) -> Self {
        Self {
            state: Arc::new(Mutex::new(BreakerState {
                failures: 0,
                open_until: None,
            })),
            config,
        }
    }
}

impl<E> Middleware<E> for BreakerMiddleware
where
    E: Endpoint,
{
    type Output = BreakerEndpoint<E>;

    fn transform(&self, ep: E) -> Self::Output {
        BreakerEndpoint {
            ep,
            config: self.config.clone(),
            state: Arc::clone(&self.state),
        }
    }
}

impl<E> Endpoint for BreakerEndpoint<E>
where
    E: Endpoint,
{
    type Output = E::Output;

    fn call(&self, req: Request) -> impl Future<Output = Result<Self::Output>> + Send {
        let open = {
            let mut state = self.state.lock().expect("breaker lock poisoned");
            breaker_is_open(&mut state)
        };

        async move {
            if open {
                return Err(RozeError::Internal("circuit open".to_string()).into());
            }

            let response = self.ep.call(req).await;
            let mut state = self.state.lock().expect("breaker lock poisoned");
            match &response {
                Ok(_) => breaker_record_success(&mut state),
                Err(_) => breaker_record_failure(&mut state, &self.config),
            }
            response
        }
    }
}

fn next_request_id() -> String {
    generate_trace_id()
}

fn incoming_trace_id(req: &Request) -> Option<String> {
    req.headers()
        .get(TRACE_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn incoming_timeout(req: &Request) -> Option<Duration> {
    let raw = req
        .headers()
        .get(roze_context::TIMEOUT_HEADER)
        .and_then(|value| value.to_str().ok())?;
    let millis = raw.parse::<u64>().ok()?;
    Some(Duration::from_millis(millis))
}

fn ensure_trace_id_header(req: &mut Request) -> String {
    if let Some(trace_id) = incoming_trace_id(req) {
        return trace_id;
    }

    let trace_id = generate_trace_id();
    let value = HeaderValue::from_str(&trace_id)
        .unwrap_or_else(|_| HeaderValue::from_static("trace-invalid"));
    req.headers_mut()
        .insert(HeaderName::from_static(TRACE_ID_HEADER), value);
    trace_id
}

fn refill_tokens(state: &mut RateLimitState, config: &RateLimitConfig) {
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

fn breaker_is_open(state: &mut BreakerState) -> bool {
    if let Some(open_until) = state.open_until {
        if Instant::now() < open_until {
            return true;
        }
        state.open_until = None;
        state.failures = 0;
    }

    false
}

fn breaker_record_success(state: &mut BreakerState) {
    state.failures = 0;
    state.open_until = None;
}

fn breaker_record_failure(state: &mut BreakerState, config: &BreakerConfig) {
    state.failures = state.failures.saturating_add(1);
    if state.failures >= config.failure_threshold.max(1) {
        state.failures = 0;
        state.open_until = Some(Instant::now() + config.reset_timeout);
    }
}

fn effective_rate_limit(
    governance: &GovernanceConfig,
    route: Option<&RouteGovernanceConfig>,
) -> Option<RateLimitConfig> {
    route
        .and_then(|route| route.rate_limit)
        .or(governance.rate_limit)
        .map(|config| RateLimitConfig {
            burst: config.burst,
            refill: Duration::from_millis(config.refill_ms),
        })
}

fn effective_breaker(
    governance: &GovernanceConfig,
    route: Option<&RouteGovernanceConfig>,
) -> Option<BreakerConfig> {
    route
        .and_then(|route| route.breaker)
        .or(governance.breaker)
        .map(|config| BreakerConfig {
            failure_threshold: config.failure_threshold,
            reset_timeout: Duration::from_millis(config.reset_timeout_ms),
        })
}

fn enforce_rate_limit(key: &str, config: &RateLimitConfig) -> Result<(), RozeError> {
    let mut states = ROUTE_RATE_LIMITS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .expect("route rate limit lock poisoned");
    let state = states
        .entry(key.to_string())
        .or_insert_with(|| RateLimitState {
            tokens: config.burst as f64,
            last_refill: Instant::now(),
        });
    refill_tokens(state, config);
    if state.tokens >= 1.0 {
        state.tokens -= 1.0;
        Ok(())
    } else {
        Err(RozeError::Internal("rate limited".to_string()))
    }
}

fn route_breaker_is_open(key: &str) -> bool {
    let mut states = ROUTE_BREAKERS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .expect("route breaker lock poisoned");
    let state = states
        .entry(key.to_string())
        .or_insert_with(|| BreakerState {
            failures: 0,
            open_until: None,
        });
    breaker_is_open(state)
}

fn route_breaker_record(key: &str, success: bool, config: &BreakerConfig) {
    let mut states = ROUTE_BREAKERS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .expect("route breaker lock poisoned");
    let state = states
        .entry(key.to_string())
        .or_insert_with(|| BreakerState {
            failures: 0,
            open_until: None,
        });
    if success {
        breaker_record_success(state);
    } else {
        breaker_record_failure(state, config);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_limit_refills_burst_capacity() {
        let config = RateLimitConfig {
            burst: 3,
            refill: Duration::from_millis(10),
        };
        let mut state = RateLimitState {
            tokens: 0.0,
            last_refill: Instant::now() - Duration::from_millis(50),
        };

        refill_tokens(&mut state, &config);

        assert_eq!(state.tokens, 3.0);
    }

    #[test]
    fn breaker_opens_and_resets() {
        let config = BreakerConfig {
            failure_threshold: 2,
            reset_timeout: Duration::from_millis(10),
        };
        let mut state = BreakerState {
            failures: 0,
            open_until: None,
        };

        assert!(!breaker_is_open(&mut state));
        breaker_record_failure(&mut state, &config);
        assert!(!breaker_is_open(&mut state));
        breaker_record_failure(&mut state, &config);
        assert!(breaker_is_open(&mut state));

        state.open_until = Some(Instant::now() - Duration::from_millis(1));
        assert!(!breaker_is_open(&mut state));
        assert_eq!(state.failures, 0);
    }

    #[test]
    fn resolves_builtin_and_custom_middleware() {
        let plan = resolve_middleware_plan(&[
            "auth".to_string(),
            "breaker".to_string(),
            "audit".to_string(),
            "auth".to_string(),
        ]);

        assert_eq!(
            plan.builtins,
            vec![BuiltInMiddleware::Auth, BuiltInMiddleware::Breaker]
        );
        assert_eq!(plan.custom, vec!["audit"]);
    }

    #[test]
    fn route_policy_prefers_route_over_global() {
        let mut governance = GovernanceConfig::default();
        governance.timeout_ms = Some(1000);
        governance.rate_limit = Some(roze_config::RateLimitConfig {
            burst: 10,
            refill_ms: 20,
        });
        governance.routes.insert(
            "login".into(),
            RouteGovernanceConfig {
                timeout_ms: Some(50),
                rate_limit: Some(roze_config::RateLimitConfig {
                    burst: 2,
                    refill_ms: 5,
                }),
                breaker: None,
            },
        );

        let policy = route_policy(Some(&governance), "login");

        assert_eq!(policy.timeout, Some(Duration::from_millis(50)));
        assert_eq!(policy.rate_limit.expect("rate limit").burst, 2);
    }
}
