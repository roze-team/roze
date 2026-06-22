use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use axum::{
    extract::{Request as AxumRequest, State},
    http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response as AxumResponse},
    Router,
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tower_http::{
    catch_panic::CatchPanicLayer,
    cors::{Any, CorsLayer},
    decompression::RequestDecompressionLayer,
    limit::RequestBodyLimitLayer,
    timeout::TimeoutLayer,
};
use tracing::Instrument;

use roze_auth::{principal_from_claims, AuthPrincipal};
use roze_config::{GovernanceConfig, RouteGovernanceConfig};
use roze_context::{AuthContext, Context};
use roze_error::RozeError;
use roze_jwt::{extract_bearer_token, verify_token, JwtConfig};
use roze_metrics::{record_http_request, record_http_route};
use roze_trace::{generate_trace_id, request_span};

static ROUTE_RATE_LIMITS: OnceLock<Mutex<HashMap<String, RateLimitState>>> = OnceLock::new();
static ROUTE_BREAKERS: OnceLock<Mutex<HashMap<String, BreakerState>>> = OnceLock::new();

pub fn apply_common<S>(router: Router<S>) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    apply_common_with_config(router, CommonMiddlewareConfig::default())
}

pub fn apply_common_with_config<S>(router: Router<S>, config: CommonMiddlewareConfig) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    let state = CommonMiddlewareState::from(&config);
    let mut router = router.layer(axum::middleware::from_fn(axum_request_context));
    if config.trace || config.stat || config.prometheus {
        router = router.layer(axum::middleware::from_fn(axum_trace));
    }
    router = router.layer(axum::middleware::from_fn_with_state(
        state,
        axum_capacity_guard,
    ));
    if let Some(limit) = config.request_body_limit_bytes {
        router = router.layer(RequestBodyLimitLayer::new(limit));
    }
    if config.gunzip {
        router = router.layer(RequestDecompressionLayer::new().gzip(true));
    }
    if config.cors {
        router = router.layer(build_cors_layer(config.cors_config.as_ref()));
    }
    if config.recover {
        router = router.layer(CatchPanicLayer::new());
    }
    router
}

pub fn apply_timeout<S>(router: Router<S>, timeout: Duration) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    router.layer(TimeoutLayer::with_status_code(
        StatusCode::REQUEST_TIMEOUT,
        timeout,
    ))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommonMiddlewareConfig {
    pub recover: bool,
    pub trace: bool,
    pub stat: bool,
    pub prometheus: bool,
    pub cors: bool,
    pub cors_config: Option<CorsConfig>,
    pub timeout: bool,
    pub max_conns: Option<usize>,
    pub shedding: Option<SheddingConfig>,
    pub gunzip: bool,
    pub request_body_limit_bytes: Option<usize>,
}

impl Default for CommonMiddlewareConfig {
    fn default() -> Self {
        Self {
            recover: true,
            trace: true,
            stat: true,
            prometheus: true,
            cors: true,
            cors_config: None,
            timeout: true,
            max_conns: None,
            shedding: None,
            gunzip: false,
            request_body_limit_bytes: None,
        }
    }
}

impl From<&roze_config::HttpMiddlewaresConfig> for CommonMiddlewareConfig {
    fn from(config: &roze_config::HttpMiddlewaresConfig) -> Self {
        Self {
            recover: config.recover,
            trace: config.trace,
            stat: config.stat,
            prometheus: config.prometheus,
            cors: config.cors,
            cors_config: config.cors_config.as_ref().map(CorsConfig::from),
            timeout: config.timeout,
            max_conns: config.max_conns,
            shedding: config.shedding.map(|shedding| SheddingConfig {
                concurrency: shedding.concurrency,
                window: Duration::from_millis(shedding.window_ms),
                min_samples: shedding.min_samples,
                max_avg_latency: Duration::from_millis(shedding.max_avg_latency_ms),
                max_failure_ratio_per_mille: shedding.max_failure_ratio_per_mille,
                cool_down: Duration::from_millis(shedding.cool_down_ms),
            }),
            gunzip: config.gunzip,
            request_body_limit_bytes: config.request_body_limit_bytes,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CorsConfig {
    pub allow_origins: Vec<String>,
    pub allow_methods: Vec<String>,
    pub allow_headers: Vec<String>,
    pub expose_headers: Vec<String>,
    pub allow_credentials: bool,
    pub max_age: Option<Duration>,
}

impl From<&roze_config::HttpCorsConfig> for CorsConfig {
    fn from(config: &roze_config::HttpCorsConfig) -> Self {
        Self {
            allow_origins: config.allow_origins.clone(),
            allow_methods: config.allow_methods.clone(),
            allow_headers: config.allow_headers.clone(),
            expose_headers: config.expose_headers.clone(),
            allow_credentials: config.allow_credentials,
            max_age: config.max_age_seconds.map(Duration::from_secs),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SheddingConfig {
    pub concurrency: usize,
    pub window: Duration,
    pub min_samples: u64,
    pub max_avg_latency: Duration,
    pub max_failure_ratio_per_mille: u32,
    pub cool_down: Duration,
}

#[derive(Clone)]
struct CommonMiddlewareState {
    max_conns: Option<Arc<Semaphore>>,
    shedding: Option<Arc<AdaptiveShedding>>,
}

impl From<&CommonMiddlewareConfig> for CommonMiddlewareState {
    fn from(config: &CommonMiddlewareConfig) -> Self {
        Self {
            max_conns: config
                .max_conns
                .filter(|limit| *limit > 0)
                .map(|limit| Arc::new(Semaphore::new(limit))),
            shedding: config
                .shedding
                .filter(|config| config.concurrency > 0)
                .map(AdaptiveShedding::new)
                .map(Arc::new),
        }
    }
}

struct AdaptiveShedding {
    config: SheddingConfig,
    concurrency: Arc<Semaphore>,
    state: Mutex<AdaptiveSheddingState>,
}

#[derive(Debug)]
struct AdaptiveSheddingState {
    window_started: Instant,
    requests: u64,
    failures: u64,
    total_latency: Duration,
    overloaded_until: Option<Instant>,
}

struct SheddingPermit {
    runtime: Arc<AdaptiveShedding>,
    permit: OwnedSemaphorePermit,
    started: Instant,
}

impl AdaptiveShedding {
    fn new(config: SheddingConfig) -> Self {
        Self {
            config,
            concurrency: Arc::new(Semaphore::new(config.concurrency)),
            state: Mutex::new(AdaptiveSheddingState {
                window_started: Instant::now(),
                requests: 0,
                failures: 0,
                total_latency: Duration::ZERO,
                overloaded_until: None,
            }),
        }
    }

    fn try_begin(self: &Arc<Self>) -> Result<SheddingPermit, SheddingRejectReason> {
        let now = Instant::now();
        if self.should_shed(now) {
            return Err(SheddingRejectReason::Overloaded);
        }
        let permit = self
            .concurrency
            .clone()
            .try_acquire_owned()
            .map_err(|_| SheddingRejectReason::Concurrency)?;
        Ok(SheddingPermit {
            runtime: self.clone(),
            permit,
            started: now,
        })
    }

    fn should_shed(&self, now: Instant) -> bool {
        let mut state = self.state.lock().expect("adaptive shedding lock poisoned");
        self.rotate_window_if_needed(&mut state, now);
        if state
            .overloaded_until
            .is_some_and(|overloaded_until| now < overloaded_until)
        {
            return true;
        }
        if state.requests < self.config.min_samples.max(1) {
            return false;
        }

        let avg_latency = avg_latency(&state);
        let failure_ratio = failure_ratio_per_mille(&state);
        let latency_overloaded = avg_latency > self.config.max_avg_latency;
        let failure_overloaded = failure_ratio > self.config.max_failure_ratio_per_mille;
        if latency_overloaded || failure_overloaded {
            state.overloaded_until = Some(now + self.config.cool_down);
            true
        } else {
            false
        }
    }

    fn record(&self, success: bool, elapsed: Duration) {
        let now = Instant::now();
        let mut state = self.state.lock().expect("adaptive shedding lock poisoned");
        self.rotate_window_if_needed(&mut state, now);
        state.requests = state.requests.saturating_add(1);
        if !success {
            state.failures = state.failures.saturating_add(1);
        }
        state.total_latency = state.total_latency.saturating_add(elapsed);
    }

    fn rotate_window_if_needed(&self, state: &mut AdaptiveSheddingState, now: Instant) {
        let window = self.config.window.max(Duration::from_millis(1));
        if now.duration_since(state.window_started) < window {
            return;
        }
        state.window_started = now;
        state.requests = 0;
        state.failures = 0;
        state.total_latency = Duration::ZERO;
        if state
            .overloaded_until
            .is_some_and(|overloaded_until| now >= overloaded_until)
        {
            state.overloaded_until = None;
        }
    }
}

impl SheddingPermit {
    fn record(self, success: bool) {
        self.runtime.record(success, self.started.elapsed());
        drop(self.permit);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SheddingRejectReason {
    Concurrency,
    Overloaded,
}

pub fn apply_auth<S>(router: Router<S>, config: JwtConfig) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    router.layer(axum::middleware::from_fn_with_state(config, axum_auth))
}

pub fn apply_auth_policy<S>(router: Router<S>, config: AuthPolicyConfig) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    router.layer(axum::middleware::from_fn_with_state(
        config,
        axum_auth_policy,
    ))
}

pub fn apply_idempotency_key<S>(router: Router<S>) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    router.layer(axum::middleware::from_fn(axum_idempotency_key))
}

#[deprecated(note = "use apply_common; the middleware crate is Axum-only")]
pub fn apply_common_axum<S>(router: Router<S>) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    apply_common(router)
}

pub async fn axum_request_context(mut req: AxumRequest, next: Next) -> AxumResponse {
    let header_map = incoming_axum_headers(&req);
    let mut context = Context::from_propagation_headers(&header_map);
    if let Some(locale) = incoming_axum_locale(&req) {
        context = context.with_locale(locale);
    }
    if let Some(key) = idempotency_key_from_request(&req) {
        context = context.with_metadata(roze_context::IDEMPOTENCY_KEY_METADATA_KEY, key);
    }
    insert_axum_request_header(
        &mut req,
        roze_context::REQUEST_ID_HEADER,
        &context.request_id(),
    );
    insert_axum_request_header(&mut req, roze_context::TRACE_ID_HEADER, &context.trace_id());
    req.extensions_mut().insert(context.clone());

    let locale = context.locale();
    let mut response = roze_error::scope_error_context(
        locale,
        Some(context.request_id()),
        Some(context.trace_id()),
        next.run(req),
    )
    .await;
    insert_axum_response_header(
        response.headers_mut(),
        roze_context::REQUEST_ID_HEADER,
        &context.request_id(),
    );
    insert_axum_response_header(
        response.headers_mut(),
        roze_context::TRACE_ID_HEADER,
        &context.trace_id(),
    );
    response
}

pub async fn axum_trace(mut req: AxumRequest, next: Next) -> AxumResponse {
    let method = req.method().clone();
    let uri = req.uri().clone();
    let trace_id = ensure_axum_header_with_aliases(
        &mut req,
        roze_context::TRACE_ID_HEADER,
        roze_context::HULA_HEADER_ALIASES.trace_id,
    );
    let span = request_span(method.as_str(), uri.path(), &trace_id);

    async move {
        let start = Instant::now();
        let response = next.run(req).await;
        let elapsed = start.elapsed();
        let success = response.status().is_success();
        record_http_request(success, elapsed);
        if success {
            tracing::info!(elapsed_ms = elapsed.as_millis(), "request completed");
        } else {
            tracing::warn!(
                status = response.status().as_u16(),
                elapsed_ms = elapsed.as_millis(),
                "request failed"
            );
        }
        response
    }
    .instrument(span)
    .await
}

pub async fn axum_auth(
    State(config): State<JwtConfig>,
    mut req: AxumRequest,
    next: Next,
) -> std::result::Result<AxumResponse, RozeError> {
    let header_value = req
        .headers()
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .ok_or(RozeError::Unauthorized)?;
    let token = extract_bearer_token(header_value).ok_or(RozeError::Unauthorized)?;
    let claims =
        verify_token(token, &config).map_err(|err| RozeError::Internal(err.to_string()))?;
    let principal = principal_from_claims(&claims);
    if let Some(context) = req.extensions().get::<Context>().cloned() {
        req.extensions_mut()
            .insert(context.with_auth(auth_context_from_principal(&principal)));
    }
    req.extensions_mut().insert(principal);
    Ok(next.run(req).await)
}

pub async fn axum_auth_policy(
    State(config): State<AuthPolicyConfig>,
    req: AxumRequest,
    next: Next,
) -> std::result::Result<AxumResponse, RozeError> {
    let decision = config.decision_for_path(req.uri().path());
    if decision == AuthDecision::Public {
        return Ok(next.run(req).await);
    }
    let Some(context) = req.extensions().get::<Context>() else {
        return Err(RozeError::Unauthorized);
    };
    enforce_auth_decision(context, &decision)?;
    Ok(next.run(req).await)
}

pub async fn axum_idempotency_key(mut req: AxumRequest, next: Next) -> AxumResponse {
    if let Some(key) = idempotency_key_from_request(&req) {
        if let Some(context) = req.extensions().get::<Context>().cloned() {
            req.extensions_mut()
                .insert(context.with_metadata(roze_context::IDEMPOTENCY_KEY_METADATA_KEY, key));
        }
    }
    next.run(req).await
}

async fn axum_capacity_guard(
    State(state): State<CommonMiddlewareState>,
    req: AxumRequest,
    next: Next,
) -> AxumResponse {
    let max_conns_permit = match state.max_conns {
        Some(semaphore) => match semaphore.try_acquire_owned() {
            Ok(permit) => Some(permit),
            Err(_) => {
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    "maximum connections exceeded",
                )
                    .into_response();
            }
        },
        None => None,
    };
    let shedding_permit = match state.shedding {
        Some(shedding) => match shedding.try_begin() {
            Ok(permit) => Some(permit),
            Err(SheddingRejectReason::Concurrency) => {
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    "shedding concurrency exceeded",
                )
                    .into_response();
            }
            Err(SheddingRejectReason::Overloaded) => {
                return (StatusCode::SERVICE_UNAVAILABLE, "service overloaded").into_response();
            }
        },
        None => None,
    };

    let response = next.run(req).await;
    let success = response.status().is_success();
    if let Some(permit) = shedding_permit {
        permit.record(success);
    }
    drop(max_conns_permit);
    response
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltInMiddleware {
    Auth,
    Trace,
    Recover,
    Stat,
    Prometheus,
    Cors,
    Timeout,
    RateLimit,
    Breaker,
    MaxConns,
    Shedding,
    Gunzip,
    BodyLimit,
    Idempotency,
}

impl BuiltInMiddleware {
    pub fn parse(name: &str) -> Option<Self> {
        match normalize_middleware_name(name).as_str() {
            "auth" | "jwt" => Some(Self::Auth),
            "trace" | "tracing" => Some(Self::Trace),
            "recover" | "recovery" | "panic_recover" => Some(Self::Recover),
            "stat" | "stats" => Some(Self::Stat),
            "prometheus" | "metrics" | "metric" => Some(Self::Prometheus),
            "cors" => Some(Self::Cors),
            "timeout" => Some(Self::Timeout),
            "rate_limit" | "ratelimit" | "rate" => Some(Self::RateLimit),
            "breaker" | "circuit_breaker" => Some(Self::Breaker),
            "max_conns" | "max_connections" | "max_conn" | "max_connection" => Some(Self::MaxConns),
            "shedding" | "load_shed" | "load_shedding" => Some(Self::Shedding),
            "gunzip" | "gzip" | "request_gunzip" => Some(Self::Gunzip),
            "body_limit" | "request_body_limit" | "max_bytes" | "max_body_bytes" => {
                Some(Self::BodyLimit)
            }
            "idempotency" | "idempotency_key" => Some(Self::Idempotency),
            _ => None,
        }
    }
}

fn normalize_middleware_name(name: &str) -> String {
    name.trim()
        .chars()
        .flat_map(|ch| match ch {
            '-' | ' ' => ['_'].into_iter().collect::<Vec<_>>(),
            ch if ch.is_ascii_uppercase() => ['_', ch.to_ascii_lowercase()]
                .into_iter()
                .collect::<Vec<_>>(),
            ch => [ch].into_iter().collect::<Vec<_>>(),
        })
        .collect::<String>()
        .trim_start_matches('_')
        .to_string()
}

fn avg_latency(state: &AdaptiveSheddingState) -> Duration {
    if state.requests == 0 {
        Duration::ZERO
    } else {
        state.total_latency / state.requests as u32
    }
}

fn failure_ratio_per_mille(state: &AdaptiveSheddingState) -> u32 {
    state
        .failures
        .saturating_mul(1000)
        .checked_div(state.requests)
        .unwrap_or(0) as u32
}

fn build_cors_layer(config: Option<&CorsConfig>) -> CorsLayer {
    let Some(config) = config else {
        return CorsLayer::permissive();
    };

    let mut cors = CorsLayer::new();
    if config.allow_origins.is_empty() || config.allow_origins.iter().any(|origin| origin == "*") {
        cors = cors.allow_origin(Any);
    } else {
        let origins = config
            .allow_origins
            .iter()
            .filter_map(|origin| origin.parse::<HeaderValue>().ok())
            .collect::<Vec<_>>();
        if !origins.is_empty() {
            cors = cors.allow_origin(origins);
        }
    }

    if config.allow_methods.is_empty() || config.allow_methods.iter().any(|method| method == "*") {
        cors = cors.allow_methods(Any);
    } else {
        let methods = config
            .allow_methods
            .iter()
            .filter_map(|method| method.parse::<Method>().ok())
            .collect::<Vec<_>>();
        if !methods.is_empty() {
            cors = cors.allow_methods(methods);
        }
    }

    if config.allow_headers.is_empty() || config.allow_headers.iter().any(|header| header == "*") {
        cors = cors.allow_headers(Any);
    } else {
        let headers = config
            .allow_headers
            .iter()
            .filter_map(|header| header.parse::<HeaderName>().ok())
            .collect::<Vec<_>>();
        if !headers.is_empty() {
            cors = cors.allow_headers(headers);
        }
    }

    let expose_headers = config
        .expose_headers
        .iter()
        .filter_map(|header| header.parse::<HeaderName>().ok())
        .collect::<Vec<_>>();
    if !expose_headers.is_empty() {
        cors = cors.expose_headers(expose_headers);
    }

    if config.allow_credentials {
        cors = cors.allow_credentials(true);
    }
    if let Some(max_age) = config.max_age {
        cors = cors.max_age(max_age);
    }
    cors
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthDecision {
    Public,
    User,
    Role(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthPolicyConfig {
    pub public_paths: Vec<String>,
    pub user_paths: Vec<String>,
    pub role_paths: Vec<RolePathPolicy>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RolePathPolicy {
    pub path: String,
    pub role: String,
}

impl AuthPolicyConfig {
    pub fn decision_for_path(&self, path: &str) -> AuthDecision {
        if self
            .public_paths
            .iter()
            .any(|pattern| path_matches(pattern, path))
        {
            return AuthDecision::Public;
        }
        if let Some(policy) = self
            .role_paths
            .iter()
            .find(|policy| path_matches(&policy.path, path))
        {
            return AuthDecision::Role(policy.role.clone());
        }
        if self
            .user_paths
            .iter()
            .any(|pattern| path_matches(pattern, path))
        {
            return AuthDecision::User;
        }
        AuthDecision::Public
    }
}

pub fn default_hula_auth_policy() -> AuthPolicyConfig {
    AuthPolicyConfig {
        public_paths: vec![
            "/register".to_string(),
            "/login".to_string(),
            "/captcha".to_string(),
            "/healthz".to_string(),
        ],
        user_paths: vec![
            "/message/*".to_string(),
            "/conversation/*".to_string(),
            "/friend/*".to_string(),
            "/group/*".to_string(),
        ],
        role_paths: vec![RolePathPolicy {
            path: "/admin/*".to_string(),
            role: "admin".to_string(),
        }],
    }
}

pub fn enforce_auth_decision(context: &Context, decision: &AuthDecision) -> Result<(), RozeError> {
    match decision {
        AuthDecision::Public => Ok(()),
        AuthDecision::User => context
            .subject()
            .filter(|subject| !subject.is_empty())
            .map(|_| ())
            .ok_or(RozeError::Unauthorized),
        AuthDecision::Role(required) => {
            if context
                .subject()
                .filter(|subject| !subject.is_empty())
                .is_none()
            {
                return Err(RozeError::Unauthorized);
            }
            if context.roles().iter().any(|role| role == required) {
                Ok(())
            } else {
                Err(RozeError::Forbidden)
            }
        }
    }
}

pub fn idempotency_key_from_headers(headers: &HeaderMap) -> Option<String> {
    ["x-idempotency-key", "idempotency-key"]
        .iter()
        .find_map(|name| {
            headers
                .get(*name)
                .and_then(|value| value.to_str().ok())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
        })
}

pub fn idempotency_key_from_query(query: Option<&str>) -> Option<String> {
    query?.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        matches!(key, "client_msg_id" | "idempotency_key")
            .then(|| value.trim())
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
    })
}

pub fn idempotency_key_from_request(req: &AxumRequest) -> Option<String> {
    idempotency_key_from_headers(req.headers())
        .or_else(|| idempotency_key_from_query(req.uri().query()))
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
) -> std::result::Result<(Context, RouteGuard), RozeError> {
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

#[derive(Debug)]
struct RateLimitState {
    tokens: f64,
    last_refill: Instant,
}

#[derive(Debug)]
struct BreakerState {
    failures: u32,
    open_until: Option<Instant>,
}

fn next_request_id() -> String {
    generate_trace_id()
}

fn incoming_axum_header(req: &AxumRequest, key: &str) -> Option<String> {
    req.headers()
        .get(key)
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn incoming_axum_headers(req: &AxumRequest) -> BTreeMap<String, String> {
    req.headers()
        .iter()
        .filter_map(|(name, value)| {
            Some((name.as_str().to_string(), value.to_str().ok()?.to_string()))
        })
        .collect()
}

fn incoming_axum_locale(req: &AxumRequest) -> Option<String> {
    incoming_axum_header(req, roze_context::LOCALE_HEADER)
        .or_else(|| incoming_axum_header(req, roze_context::ACCEPT_LANGUAGE_HEADER))
        .and_then(|value| roze_error::locale_from_accept_language(&value))
}

fn ensure_axum_header(req: &mut AxumRequest, key: &'static str) -> String {
    if let Some(value) = incoming_axum_header(req, key) {
        return value;
    }

    let generated = next_request_id();
    if let Ok(value) = axum::http::HeaderValue::from_str(&generated) {
        req.headers_mut()
            .insert(axum::http::HeaderName::from_static(key), value);
    }
    generated
}

fn ensure_axum_header_with_aliases(
    req: &mut AxumRequest,
    key: &'static str,
    aliases: &[&str],
) -> String {
    if let Some(value) = incoming_axum_header(req, key).or_else(|| {
        aliases
            .iter()
            .find_map(|alias| incoming_axum_header(req, alias))
    }) {
        insert_axum_request_header(req, key, &value);
        return value;
    }
    ensure_axum_header(req, key)
}

fn insert_axum_response_header(
    headers: &mut axum::http::HeaderMap,
    key: &'static str,
    value: &str,
) {
    if let Ok(value) = axum::http::HeaderValue::from_str(value) {
        headers.insert(axum::http::HeaderName::from_static(key), value);
    }
}

fn insert_axum_request_header(req: &mut AxumRequest, key: &'static str, value: &str) {
    let Ok(value) = HeaderValue::from_str(value) else {
        return;
    };
    req.headers_mut()
        .insert(HeaderName::from_static(key), value);
}

fn path_matches(pattern: &str, path: &str) -> bool {
    if let Some(prefix) = pattern.strip_suffix("/*") {
        path == prefix || path.starts_with(&format!("{prefix}/"))
    } else {
        path == pattern || path.ends_with(pattern)
    }
}

fn auth_context_from_principal(principal: &AuthPrincipal) -> AuthContext {
    AuthContext {
        subject: principal.subject.clone(),
        roles: principal.roles.clone(),
        tenant: principal.tenant.clone(),
    }
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

fn enforce_rate_limit(key: &str, config: &RateLimitConfig) -> std::result::Result<(), RozeError> {
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
    use axum::{body::Body, http::Request, routing::get, Extension, Json};
    use tower::ServiceExt;

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

    #[tokio::test]
    async fn timeout_layer_rejects_slow_requests() {
        let app = apply_timeout(
            axum::Router::new().route(
                "/slow",
                get(|| async {
                    tokio::time::sleep(Duration::from_millis(25)).await;
                    "ok"
                }),
            ),
            Duration::from_millis(1),
        );

        let response = app
            .oneshot(Request::builder().uri("/slow").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::REQUEST_TIMEOUT);
    }

    #[test]
    fn adaptive_shedding_rejects_after_unhealthy_window() {
        let runtime = Arc::new(AdaptiveShedding::new(SheddingConfig {
            concurrency: 10,
            window: Duration::from_millis(1000),
            min_samples: 1,
            max_avg_latency: Duration::from_millis(500),
            max_failure_ratio_per_mille: 0,
            cool_down: Duration::from_millis(1000),
        }));

        runtime.record(false, Duration::from_millis(10));

        assert!(matches!(
            runtime.try_begin(),
            Err(SheddingRejectReason::Overloaded)
        ));
    }

    #[test]
    fn resolves_builtin_and_custom_middleware() {
        let plan = resolve_middleware_plan(&[
            "auth".to_string(),
            "trace".to_string(),
            "cors".to_string(),
            "maxConns".to_string(),
            "gunzip".to_string(),
            "body-limit".to_string(),
            "breaker".to_string(),
            "audit".to_string(),
            "auth".to_string(),
        ]);

        assert_eq!(
            plan.builtins,
            vec![
                BuiltInMiddleware::Auth,
                BuiltInMiddleware::Trace,
                BuiltInMiddleware::Cors,
                BuiltInMiddleware::MaxConns,
                BuiltInMiddleware::Gunzip,
                BuiltInMiddleware::BodyLimit,
                BuiltInMiddleware::Breaker,
            ]
        );
        assert_eq!(plan.custom, vec!["audit"]);
    }

    #[test]
    fn route_policy_prefers_route_over_global() {
        let mut governance = GovernanceConfig {
            timeout_ms: Some(1000),
            rate_limit: Some(roze_config::RateLimitConfig {
                burst: 10,
                refill_ms: 20,
            }),
            ..Default::default()
        };
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

    #[test]
    fn hula_auth_policy_maps_paths_to_requirements() {
        let policy = default_hula_auth_policy();

        assert_eq!(policy.decision_for_path("/login"), AuthDecision::Public);
        assert_eq!(
            policy.decision_for_path("/message/send"),
            AuthDecision::User
        );
        assert_eq!(
            policy.decision_for_path("/admin/users/ban"),
            AuthDecision::Role("admin".to_string())
        );
    }

    #[test]
    fn auth_decision_enforces_subject_and_role() {
        let user = Context::background_with_request_id_and_trace_id("request-1", "trace-1")
            .with_auth(AuthContext {
                subject: "user-1".to_string(),
                roles: vec!["user".to_string()],
                tenant: None,
            });
        let admin = user.with_auth(AuthContext {
            subject: "admin-1".to_string(),
            roles: vec!["admin".to_string()],
            tenant: None,
        });

        assert!(enforce_auth_decision(&user, &AuthDecision::User).is_ok());
        assert_eq!(
            enforce_auth_decision(&user, &AuthDecision::Role("admin".to_string())),
            Err(RozeError::Forbidden)
        );
        assert!(enforce_auth_decision(&admin, &AuthDecision::Role("admin".to_string())).is_ok());
    }

    #[test]
    fn idempotency_key_reads_headers_and_query() {
        let request = Request::builder()
            .uri("/message/send?client_msg_id=msg-1")
            .header("x-idempotency-key", "idem-1")
            .body(Body::empty())
            .expect("request");

        assert_eq!(
            idempotency_key_from_headers(request.headers()).as_deref(),
            Some("idem-1")
        );
        assert_eq!(
            idempotency_key_from_query(request.uri().query()).as_deref(),
            Some("msg-1")
        );
    }

    #[tokio::test]
    async fn request_context_localizes_error_response_from_accept_language() {
        let app = apply_common(axum::Router::new().route(
            "/private",
            get(|| async { Err::<&'static str, RozeError>(RozeError::Unauthorized) }),
        ));

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/private")
                    .header(roze_context::ACCEPT_LANGUAGE_HEADER, "zh-CN,zh;q=0.9")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .expect("body");
        let value: serde_json::Value = serde_json::from_slice(&body).expect("json");

        assert_eq!(value["msg"], "未认证或登录已失效");
        assert!(value["request_id"].as_str().is_some());
        assert!(value["trace_id"].as_str().is_some());
    }

    #[tokio::test]
    async fn request_context_restores_hula_alias_headers() {
        let app = apply_common(axum::Router::new().route(
            "/message/send",
            get(|Extension(ctx): Extension<Context>| async move {
                Json(serde_json::json!({
                    "request_id": ctx.request_id(),
                    "trace_id": ctx.trace_id(),
                    "subject": ctx.subject(),
                    "tenant": ctx.tenant(),
                    "device_id": ctx.metadata_value(roze_context::DEVICE_ID_METADATA_KEY),
                    "scope": ctx.metadata_value(roze_context::SCOPE_METADATA_KEY),
                }))
            }),
        ));

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/message/send")
                    .header(roze_context::HULA_REQUEST_ID_HEADER, "request-hula")
                    .header(roze_context::HULA_TRACE_ID_HEADER, "trace-hula")
                    .header(roze_context::HULA_UID_HEADER, "user-hula")
                    .header(roze_context::HULA_TENANT_ID_HEADER, "tenant-hula")
                    .header(roze_context::HULA_DEVICE_ID_HEADER, "device-hula")
                    .header(roze_context::HULA_SCOPE_HEADER, "message:write")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .expect("body");
        let value: serde_json::Value = serde_json::from_slice(&body).expect("json");

        assert_eq!(value["request_id"], "request-hula");
        assert_eq!(value["trace_id"], "trace-hula");
        assert_eq!(value["subject"], "user-hula");
        assert_eq!(value["tenant"], "tenant-hula");
        assert_eq!(value["device_id"], "device-hula");
        assert_eq!(value["scope"], "message:write");
    }

    #[tokio::test]
    async fn request_context_stores_idempotency_key() {
        let app = apply_common(axum::Router::new().route(
            "/message/send",
            get(|Extension(ctx): Extension<Context>| async move {
                ctx.metadata_value(roze_context::IDEMPOTENCY_KEY_METADATA_KEY)
                    .unwrap_or_default()
            }),
        ));

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/message/send")
                    .header("x-idempotency-key", "idem-1")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .expect("body");

        assert_eq!(body.as_ref(), b"idem-1");
    }

    #[tokio::test]
    async fn cors_config_limits_allowed_origins() {
        let app = apply_common_with_config(
            axum::Router::new().route("/ping", get(|| async { "pong" })),
            CommonMiddlewareConfig {
                trace: false,
                stat: false,
                prometheus: false,
                cors_config: Some(CorsConfig {
                    allow_origins: vec!["https://example.com".to_string()],
                    allow_methods: vec!["GET".to_string()],
                    allow_headers: vec!["authorization".to_string()],
                    expose_headers: vec!["x-request-id".to_string()],
                    allow_credentials: true,
                    max_age: Some(Duration::from_secs(60)),
                }),
                ..Default::default()
            },
        );

        let allowed = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/ping")
                    .header("origin", "https://example.com")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(
            allowed
                .headers()
                .get("access-control-allow-origin")
                .and_then(|value| value.to_str().ok()),
            Some("https://example.com")
        );
        assert_eq!(
            allowed
                .headers()
                .get("access-control-allow-credentials")
                .and_then(|value| value.to_str().ok()),
            Some("true")
        );

        let denied = app
            .oneshot(
                Request::builder()
                    .uri("/ping")
                    .header("origin", "https://other.example")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert!(denied
            .headers()
            .get("access-control-allow-origin")
            .is_none());
    }

    #[tokio::test]
    async fn capacity_guard_rejects_when_shedding_limit_is_full() {
        let app = apply_common_with_config(
            axum::Router::new().route(
                "/slow",
                get(|| async {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    "ok"
                }),
            ),
            CommonMiddlewareConfig {
                trace: false,
                stat: false,
                prometheus: false,
                shedding: Some(SheddingConfig {
                    concurrency: 1,
                    window: Duration::from_millis(1000),
                    min_samples: 100,
                    max_avg_latency: Duration::from_millis(500),
                    max_failure_ratio_per_mille: 500,
                    cool_down: Duration::from_millis(1000),
                }),
                ..Default::default()
            },
        );

        let first = tokio::spawn(
            app.clone().oneshot(
                Request::builder()
                    .uri("/slow")
                    .body(Body::empty())
                    .expect("request"),
            ),
        );
        tokio::time::sleep(Duration::from_millis(1)).await;
        let second = app
            .oneshot(
                Request::builder()
                    .uri("/slow")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(second.status(), StatusCode::SERVICE_UNAVAILABLE);
        first.await.expect("first request").expect("first response");
    }
}
