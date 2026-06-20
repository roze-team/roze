use std::collections::{BTreeMap, HashMap};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use axum::{
    extract::{Request as AxumRequest, State},
    middleware::Next,
    response::Response as AxumResponse,
    Router,
};
use tower::ServiceBuilder;
use tower_http::{catch_panic::CatchPanicLayer, cors::CorsLayer};
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
    router.layer(
        ServiceBuilder::new()
            .layer(CatchPanicLayer::new())
            .layer(CorsLayer::permissive())
            .layer(axum::middleware::from_fn(axum_trace))
            .layer(axum::middleware::from_fn(axum_request_context)),
    )
}

pub fn apply_auth<S>(router: Router<S>, config: JwtConfig) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    router.layer(axum::middleware::from_fn_with_state(config, axum_auth))
}

#[deprecated(note = "use apply_common; the middleware crate is Axum-only")]
pub fn apply_common_axum<S>(router: Router<S>) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    apply_common(router)
}

pub async fn axum_request_context(mut req: AxumRequest, next: Next) -> AxumResponse {
    let request_id = ensure_axum_header(&mut req, roze_context::REQUEST_ID_HEADER);
    let trace_id = ensure_axum_header(&mut req, roze_context::TRACE_ID_HEADER);
    let context = incoming_axum_timeout(&req)
        .map(|timeout| {
            Context::background_with_request_id_and_trace_id(request_id.clone(), trace_id.clone())
                .with_timeout(timeout)
        })
        .unwrap_or_else(|| {
            Context::background_with_request_id_and_trace_id(request_id.clone(), trace_id.clone())
        })
        .with_metadata_map(incoming_axum_metadata(&req));
    req.extensions_mut().insert(context.clone());

    let locale = context.locale();
    let mut response = roze_error::scope_locale(locale, next.run(req)).await;
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
    let trace_id = ensure_axum_header(&mut req, roze_context::TRACE_ID_HEADER);
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

fn incoming_axum_timeout(req: &AxumRequest) -> Option<Duration> {
    let raw = req
        .headers()
        .get(roze_context::TIMEOUT_HEADER)
        .and_then(|value| value.to_str().ok())?;
    let millis = raw.parse::<u64>().ok()?;
    Some(Duration::from_millis(millis))
}

fn incoming_axum_metadata(req: &AxumRequest) -> BTreeMap<String, String> {
    let mut metadata = req
        .headers()
        .iter()
        .filter_map(|(name, value)| {
            let key = name
                .as_str()
                .strip_prefix(roze_context::METADATA_HEADER_PREFIX)?;
            let value = value.to_str().ok()?;
            Some((key.to_string(), value.to_string()))
        })
        .collect::<BTreeMap<_, _>>();
    if let Some(locale) = incoming_axum_header(req, roze_context::LOCALE_HEADER)
        .or_else(|| incoming_axum_header(req, roze_context::ACCEPT_LANGUAGE_HEADER))
        .and_then(|value| roze_error::locale_from_accept_language(&value))
    {
        metadata.insert(roze_context::LOCALE_METADATA_KEY.to_string(), locale);
    }
    metadata
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

fn insert_axum_response_header(
    headers: &mut axum::http::HeaderMap,
    key: &'static str,
    value: &str,
) {
    if let Ok(value) = axum::http::HeaderValue::from_str(value) {
        headers.insert(axum::http::HeaderName::from_static(key), value);
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
    use axum::{body::Body, http::Request, routing::get};
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
    }
}
