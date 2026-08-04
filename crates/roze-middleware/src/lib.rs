use std::{
    collections::BTreeMap,
    net::SocketAddr,
    sync::OnceLock,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use async_trait::async_trait;
use futures_util::TryStreamExt as _;
use http_body_util::BodyExt as _;
use roze_auth::BearerTokenVerifier as _;
use roze_context::{AuthContext, Context};
use roze_error::RozeError;
use roze_redis::redis;
use roze_resilience::{
    BreakerDecision, BreakerPermit, BreakerRegistry, GovernanceBoundary, OperationKey,
    SheddingRegistry,
};
use serde::Serialize;
use tokio::io::AsyncReadExt as _;
use tokio_util::io::StreamReader;

static ROUTE_BREAKERS: OnceLock<BreakerRegistry> = OnceLock::new();
static ROUTE_SHEDDERS: OnceLock<SheddingRegistry> = OnceLock::new();

#[derive(Debug, Clone)]
pub struct CommonMiddlewareConfig {
    pub request_context: bool,
    pub tracing: bool,
    pub auth: Option<AuthConfig>,
    pub cors: bool,
    pub cors_config: Option<CorsConfig>,
    pub timeout_ms: Option<u64>,
    pub gunzip: bool,
    pub body_limit_bytes: Option<usize>,
    pub trust_forwarded_identity_headers: bool,
    pub trusted_proxies: Option<roze_http::client_ip::TrustedProxyConfig>,
}

impl Default for CommonMiddlewareConfig {
    fn default() -> Self {
        Self {
            request_context: true,
            tracing: true,
            auth: None,
            cors: true,
            cors_config: None,
            timeout_ms: None,
            gunzip: false,
            body_limit_bytes: None,
            trust_forwarded_identity_headers: false,
            trusted_proxies: None,
        }
    }
}

impl From<&roze_config::HttpMiddlewaresConfig> for CommonMiddlewareConfig {
    fn from(config: &roze_config::HttpMiddlewaresConfig) -> Self {
        Self {
            request_context: true,
            tracing: true,
            auth: None,
            cors: config.cors,
            cors_config: config.cors_config.as_ref().map(CorsConfig::from),
            timeout_ms: config.timeout.then_some(30_000),
            gunzip: config.gunzip,
            body_limit_bytes: config.request_body_limit_bytes,
            trust_forwarded_identity_headers: config.trust_forwarded_identity_headers,
            trusted_proxies: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct AuthConfig {
    pub jwt: roze_jwt::JwtConfig,
    pub public_routes: Vec<String>,
}

impl From<&roze_config::AuthConfig> for AuthConfig {
    fn from(config: &roze_config::AuthConfig) -> Self {
        Self {
            jwt: roze_jwt::JwtConfig::from(config),
            public_routes: Vec::new(),
        }
    }
}

impl CommonMiddlewareConfig {
    pub fn from_service(
        middlewares: &roze_config::HttpMiddlewaresConfig,
        auth: Option<&roze_config::AuthConfig>,
    ) -> Self {
        Self::try_from_service(middlewares, auth).unwrap_or_else(|error| {
            tracing::error!(error = %error, "invalid trusted proxy configuration; forwarding headers will not be trusted");
            let mut config = Self::from(middlewares);
            config.auth = auth.map(|auth| AuthConfig {
                jwt: roze_jwt::JwtConfig::from(auth),
                public_routes: middlewares.auth_public_routes.clone(),
            });
            config
        })
    }

    pub fn try_from_service(
        middlewares: &roze_config::HttpMiddlewaresConfig,
        auth: Option<&roze_config::AuthConfig>,
    ) -> Result<Self, roze_http::client_ip::TrustedProxyConfigError> {
        let mut config = Self::from(middlewares);
        config.auth = auth.map(|auth| AuthConfig {
            jwt: roze_jwt::JwtConfig::from(auth),
            public_routes: middlewares.auth_public_routes.clone(),
        });
        let trusted_proxies =
            roze_http::client_ip::TrustedProxyConfig::new(&middlewares.trusted_proxy_cidrs)?;
        config.trusted_proxies = Some(trusted_proxies);
        Ok(config)
    }
}

#[derive(Debug, Clone, Default)]
pub struct CorsConfig {
    pub allow_origins: Vec<String>,
    pub allow_methods: Vec<String>,
    pub allow_headers: Vec<String>,
    pub expose_headers: Vec<String>,
    pub allow_credentials: bool,
    pub max_age_seconds: Option<u64>,
}

impl From<&roze_config::HttpCorsConfig> for CorsConfig {
    fn from(config: &roze_config::HttpCorsConfig) -> Self {
        Self {
            allow_origins: config.allow_origins.clone(),
            allow_methods: config.allow_methods.clone(),
            allow_headers: config.allow_headers.clone(),
            expose_headers: config.expose_headers.clone(),
            allow_credentials: config.allow_credentials,
            max_age_seconds: config.max_age_seconds,
        }
    }
}

pub fn apply_common(service: roze_http::Router) -> roze_http::Router {
    apply_common_with_config(service, CommonMiddlewareConfig::default())
}

pub fn apply_common_with_config(
    mut service: roze_http::Router,
    config: CommonMiddlewareConfig,
) -> roze_http::Router {
    if config.tracing {
        service = service.layer(roze_http::middleware::from_fn(trace_http_request));
    }
    if let Some(auth) = config.auth {
        service = service.layer(roze_http::middleware::from_fn_with_state(
            auth,
            authenticate_request,
        ));
    }
    if config.request_context {
        service = service.layer(roze_http::middleware::from_fn(inject_request_context));
    }
    if let Some(trusted_proxies) = config.trusted_proxies {
        service = service.layer(roze_http::middleware::from_fn_with_state(
            trusted_proxies,
            inject_client_ip,
        ));
    }
    if !config.trust_forwarded_identity_headers {
        service = service.layer(roze_http::middleware::from_fn(
            strip_untrusted_identity_headers,
        ));
    }
    if let Some(limit) = config.body_limit_bytes {
        service = service.layer(roze_http::middleware::from_fn_with_state(
            limit,
            enforce_request_body_limit,
        ));
        service = service.layer(roze_http::extract::DefaultBodyLimit::max(limit));
    }
    if config.gunzip {
        service = service.layer(roze_http::middleware::from_fn_with_state(
            GzipBodyPolicy {
                limit: config
                    .body_limit_bytes
                    .unwrap_or(roze_http::extract::DEFAULT_BODY_LIMIT),
                configured_limit: config.body_limit_bytes.is_some(),
            },
            decompress_gzip_request,
        ));
    }
    if config.cors {
        service = service.layer(cors_layer(config.cors_config.as_ref()));
    }
    service
}

async fn inject_client_ip(
    roze_http::extract::State(config): roze_http::extract::State<
        roze_http::client_ip::TrustedProxyConfig,
    >,
    mut request: roze_http::IncomingRequest,
    next: roze_http::middleware::Next,
) -> roze_http::HttpResponse {
    if let Some(peer) = request
        .extensions()
        .get::<roze_http::extract::ConnectInfo<SocketAddr>>()
        .map(|peer| peer.0)
    {
        let client_ip = config.resolve(peer, request.headers());
        request.extensions_mut().insert(client_ip);
    }
    next.run(request).await
}

async fn authenticate_request(
    roze_http::extract::State(config): roze_http::extract::State<AuthConfig>,
    mut request: roze_http::IncomingRequest,
    next: roze_http::middleware::Next,
) -> roze_http::HttpResponse {
    if route_is_public(
        request.method().as_str(),
        request.uri().path(),
        &config.public_routes,
    ) {
        return next.run(request).await;
    }

    let Some(token) = bearer_token(request.headers()) else {
        return unauthorized_response();
    };
    let verifier = roze_jwt::LocalJwtVerifier::new(config.jwt);
    let Ok(principal) = verifier.verify(token).await else {
        return unauthorized_response();
    };
    let context = request
        .extensions()
        .get::<Context>()
        .cloned()
        .unwrap_or_default()
        .with_auth(AuthContext {
            subject: principal.subject,
            roles: principal.roles,
            tenant: principal.tenant,
        })
        .with_permissions(principal.permissions)
        .with_metadata(roze_context::SCOPE_METADATA_KEY, principal.scopes.join(","));
    request.extensions_mut().insert(context);
    next.run(request).await
}

fn bearer_token(headers: &roze_http::http::HeaderMap) -> Option<&str> {
    let value = headers
        .get(roze_http::http::header::AUTHORIZATION)?
        .to_str()
        .ok()?;
    let (scheme, token) = value.split_once(' ')?;
    (scheme.eq_ignore_ascii_case("bearer") && !token.trim().is_empty()).then(|| token.trim())
}

fn route_is_public(method: &str, path: &str, allowlist: &[String]) -> bool {
    allowlist.iter().any(|entry| {
        let entry = entry.trim();
        if entry.is_empty() {
            return false;
        }
        let (allowed_method, pattern) = entry
            .split_once(char::is_whitespace)
            .map(|(method, path)| (Some(method.trim()), path.trim()))
            .unwrap_or((None, entry));
        if allowed_method.is_some_and(|allowed| !allowed.eq_ignore_ascii_case(method)) {
            return false;
        }
        pattern
            .strip_suffix('*')
            .map_or(path == pattern, |prefix| path.starts_with(prefix))
    })
}

fn unauthorized_response() -> roze_http::HttpResponse {
    let mut response = roze_http::IntoResponse::into_response(RozeError::Unauthorized);
    response.headers_mut().insert(
        roze_http::http::header::WWW_AUTHENTICATE,
        roze_http::http::HeaderValue::from_static("Bearer"),
    );
    response
}

async fn trace_http_request(
    request: roze_http::IncomingRequest,
    next: roze_http::middleware::Next,
) -> roze_http::HttpResponse {
    let method = request.method().clone();
    let path = request.uri().path().to_string();
    let (request_id, trace_id) = request
        .extensions()
        .get::<Context>()
        .map(|context| (context.request_id(), context.trace_id()))
        .unwrap_or_else(|| (String::new(), String::new()));
    let started = Instant::now();
    tracing::info!(
        protocol = "http",
        method = %method,
        path = %path,
        request_id = %request_id,
        trace_id = %trace_id,
        "HTTP request started"
    );

    let response = next.run(request).await;
    tracing::info!(
        protocol = "http",
        method = %method,
        path = %path,
        status = response.status().as_u16(),
        elapsed_ms = started.elapsed().as_millis(),
        request_id = %request_id,
        trace_id = %trace_id,
        "HTTP request completed"
    );
    response
}

fn cors_layer(config: Option<&CorsConfig>) -> tower_http::cors::CorsLayer {
    use roze_http::http::{HeaderName, HeaderValue, Method};
    use tower_http::cors::{AllowOrigin, Any, CorsLayer};

    let Some(config) = config else {
        return CorsLayer::permissive();
    };
    let mut layer = CorsLayer::new();
    let origins = config
        .allow_origins
        .iter()
        .filter_map(|value| value.parse::<HeaderValue>().ok())
        .collect::<Vec<_>>();
    if config.allow_origins.is_empty() || config.allow_origins.iter().any(|value| value == "*") {
        layer = if config.allow_credentials {
            layer.allow_origin(AllowOrigin::mirror_request())
        } else {
            layer.allow_origin(Any)
        };
    } else if !origins.is_empty() {
        layer = layer.allow_origin(origins);
    }
    let methods = config
        .allow_methods
        .iter()
        .filter_map(|value| value.parse::<Method>().ok())
        .collect::<Vec<_>>();
    layer = if methods.is_empty() {
        layer.allow_methods(Any)
    } else {
        layer.allow_methods(methods)
    };
    let headers = config
        .allow_headers
        .iter()
        .filter_map(|value| value.parse::<HeaderName>().ok())
        .collect::<Vec<_>>();
    layer = if headers.is_empty() {
        layer.allow_headers(Any)
    } else {
        layer.allow_headers(headers)
    };
    let exposed = config
        .expose_headers
        .iter()
        .filter_map(|value| value.parse::<HeaderName>().ok())
        .collect::<Vec<_>>();
    if !exposed.is_empty() {
        layer = layer.expose_headers(exposed);
    }
    if config.allow_credentials {
        layer = layer.allow_credentials(true);
    }
    if let Some(seconds) = config.max_age_seconds {
        layer = layer.max_age(Duration::from_secs(seconds));
    }
    layer
}

async fn inject_request_context(
    mut request: roze_http::IncomingRequest,
    next: roze_http::middleware::Next,
) -> roze_http::HttpResponse {
    let headers = request
        .headers()
        .iter()
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|value| (name.as_str().to_string(), value.to_string()))
        })
        .collect::<BTreeMap<_, _>>();
    let context = Context::from_propagation_headers(&headers);
    let propagation_headers = context.propagation_headers();
    request.extensions_mut().insert(context);

    let mut response = next.run(request).await;
    for name in [
        roze_context::REQUEST_ID_HEADER,
        roze_context::TRACE_ID_HEADER,
    ] {
        let Some(value) = propagation_headers.get(name) else {
            continue;
        };
        let (Ok(name), Ok(value)) = (
            name.parse::<roze_http::http::HeaderName>(),
            value.parse::<roze_http::http::HeaderValue>(),
        ) else {
            continue;
        };
        response.headers_mut().insert(name, value);
    }
    response
}

async fn strip_untrusted_identity_headers(
    mut request: roze_http::IncomingRequest,
    next: roze_http::middleware::Next,
) -> roze_http::HttpResponse {
    strip_identity_headers(request.headers_mut());
    next.run(request).await
}

fn strip_identity_headers(headers: &mut roze_http::http::HeaderMap) {
    for name in [
        roze_context::SUBJECT_HEADER,
        roze_context::TENANT_HEADER,
        roze_context::ROLES_HEADER,
        roze_context::PERMISSIONS_HEADER,
        roze_context::SCOPE_HEADER,
        roze_context::HULA_TENANT_ID_HEADER,
        roze_context::HULA_UID_HEADER,
        roze_context::HULA_ROLE_HEADER,
        roze_context::HULA_SCOPE_HEADER,
    ] {
        headers.remove(name);
    }
}

async fn enforce_request_body_limit(
    roze_http::extract::State(limit): roze_http::extract::State<usize>,
    request: roze_http::IncomingRequest,
    next: roze_http::middleware::Next,
) -> roze_http::HttpResponse {
    let (parts, body) = request.into_parts();
    match roze_http::body::to_bytes(body, limit).await {
        Ok(bytes) => {
            let request = roze_http::http::Request::from_parts(parts, roze_http::body::full(bytes));
            next.run(request).await
        }
        Err(roze_http::body::BodyError::LengthLimitExceeded { .. }) => {
            roze_http::IntoResponse::into_response((
                roze_http::http::StatusCode::PAYLOAD_TOO_LARGE,
                "request body too large",
            ))
        }
        Err(error) => roze_http::IntoResponse::into_response((
            roze_http::http::StatusCode::BAD_REQUEST,
            format!("failed to read request body: {error}"),
        )),
    }
}

#[derive(Debug, Clone, Copy)]
struct GzipBodyPolicy {
    limit: usize,
    configured_limit: bool,
}

async fn decompress_gzip_request(
    roze_http::extract::State(policy): roze_http::extract::State<GzipBodyPolicy>,
    request: roze_http::IncomingRequest,
    next: roze_http::middleware::Next,
) -> roze_http::HttpResponse {
    if request
        .headers()
        .get(roze_http::http::header::CONTENT_ENCODING)
        .is_none_or(|encoding| encoding.as_bytes() != b"gzip")
    {
        return next.run(request).await;
    }

    let (mut parts, body) = request.into_parts();
    match decompress_gzip_limited(body, policy.limit).await {
        Ok(bytes) => {
            parts
                .headers
                .remove(roze_http::http::header::CONTENT_ENCODING);
            parts
                .headers
                .remove(roze_http::http::header::CONTENT_LENGTH);
            next.run(roze_http::http::Request::from_parts(
                parts,
                roze_http::body::full(bytes),
            ))
            .await
        }
        Err(roze_http::body::BodyError::LengthLimitExceeded { .. }) => {
            let status = if policy.configured_limit {
                roze_http::http::StatusCode::PAYLOAD_TOO_LARGE
            } else {
                roze_http::http::StatusCode::BAD_REQUEST
            };
            roze_http::IntoResponse::into_response((status, "request body too large"))
        }
        Err(error) => roze_http::IntoResponse::into_response((
            roze_http::http::StatusCode::BAD_REQUEST,
            format!("failed to decompress request body: {error}"),
        )),
    }
}

async fn decompress_gzip_limited(
    body: roze_http::body::Body,
    limit: usize,
) -> Result<roze_http::body::Bytes, roze_http::body::BodyError> {
    let stream = body.into_data_stream().map_err(std::io::Error::other);
    let reader = StreamReader::new(stream);
    let decoder = async_compression::tokio::bufread::GzipDecoder::new(reader);
    let read_limit = u64::try_from(limit).unwrap_or(u64::MAX).saturating_add(1);
    let mut limited = decoder.take(read_limit);
    let mut decoded = Vec::with_capacity(limit.min(64 * 1024));
    limited
        .read_to_end(&mut decoded)
        .await
        .map_err(|error| roze_http::body::BodyError::Body(roze_http::BoxError::new(error)))?;
    if decoded.len() > limit {
        return Err(roze_http::body::BodyError::LengthLimitExceeded {
            limit,
            actual: decoded.len(),
        });
    }
    Ok(roze_http::body::Bytes::from(decoded))
}

pub fn apply_timeout(service: roze_http::Router, timeout_ms: u64) -> roze_http::Router {
    let timeout = Duration::from_millis(timeout_ms.max(1));
    service.layer(roze_http::middleware::from_fn_with_state(
        timeout,
        enforce_http_timeout,
    ))
}

async fn enforce_http_timeout(
    roze_http::extract::State(timeout): roze_http::extract::State<Duration>,
    request: roze_http::IncomingRequest,
    next: roze_http::middleware::Next,
) -> roze_http::HttpResponse {
    match tokio::time::timeout(timeout, next.run(request)).await {
        Ok(response) => response,
        Err(_) => roze_http::IntoResponse::into_response((
            roze_http::http::StatusCode::GATEWAY_TIMEOUT,
            "request timeout",
        )),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BuiltInMiddleware {
    Auth,
    Trace,
    Metrics,
    RateLimit,
    Breaker,
    Shedding,
    Idempotency,
}

impl BuiltInMiddleware {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "auth" | "jwt" => Some(Self::Auth),
            "trace" | "tracing" => Some(Self::Trace),
            "metrics" | "metric" => Some(Self::Metrics),
            "rate_limit" | "ratelimit" | "rate" => Some(Self::RateLimit),
            "breaker" | "circuit_breaker" => Some(Self::Breaker),
            "shedding" | "load_shedding" => Some(Self::Shedding),
            "idempotency" | "idempotency_key" => Some(Self::Idempotency),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MiddlewarePlan {
    pub builtins: Vec<BuiltInMiddleware>,
    pub custom: Vec<String>,
}

pub fn resolve_middleware_plan(middlewares: &[String]) -> MiddlewarePlan {
    let mut plan = MiddlewarePlan::default();
    for middleware in middlewares {
        if let Some(built_in) = BuiltInMiddleware::parse(middleware) {
            if !plan.builtins.contains(&built_in) {
                plan.builtins.push(built_in);
            }
        } else {
            plan.custom.push(middleware.clone());
        }
    }
    plan
}

#[derive(Debug)]
pub struct RouteGuard {
    key: String,
    service: String,
    route: String,
    method: String,
    started: Instant,
    breaker: Option<RouteBreakerConfig>,
    breaker_permit: Option<BreakerPermit>,
    shedding: Option<RouteSheddingConfig>,
    finished: bool,
}

impl Drop for RouteGuard {
    fn drop(&mut self) {
        if self.finished {
            return;
        }

        let elapsed = self.started.elapsed();
        if let (Some(config), Some(permit)) = (self.breaker, self.breaker_permit) {
            route_breaker_cancel(&self.key, permit, &config);
            if permit == BreakerPermit::Probe {
                roze_metrics::record_resilience_decision(
                    self.service.as_str(),
                    "rest",
                    "breaker",
                    "probe_cancelled",
                );
            }
        }
        if self.shedding.is_some() {
            route_shedding_release(&self.key);
            roze_metrics::record_resilience_decision(
                self.service.as_str(),
                "rest",
                "load_shedding",
                "cancelled",
            );
        }
        roze_metrics::record_http_request(false, elapsed);
        roze_metrics::record_http_route(
            self.service.as_str(),
            self.route.as_str(),
            self.method.as_str(),
            "cancelled",
            elapsed,
        );
    }
}

#[derive(Debug, Clone)]
pub struct RoutePolicy {
    pub timeout: Option<Duration>,
    pub rate_limit: Option<RouteRateLimitConfig>,
    pub breaker: Option<RouteBreakerConfig>,
    pub shedding: Option<RouteSheddingConfig>,
    pub fallback: Option<roze_config::GovernanceFallbackConfig>,
}

#[derive(Debug, Clone)]
pub struct RouteRateLimitConfig {
    pub burst: u32,
    pub refill: Duration,
    pub tokens_per_refill: u32,
    pub key: roze_rate_limit::RateLimitKeyPolicy,
}

#[derive(Debug, Clone, Copy)]
pub struct RouteBreakerConfig {
    pub failure_threshold: u32,
    pub reset_timeout: Duration,
}

#[derive(Debug, Clone, Copy)]
pub struct RouteSheddingConfig {
    pub concurrency: usize,
    pub window: Duration,
    pub min_samples: u64,
    pub max_avg_latency: Duration,
    pub max_failure_ratio_per_mille: u32,
    pub cool_down: Duration,
}

pub fn route_policy(
    governance: Option<&roze_config::GovernanceConfig>,
    route: &str,
) -> RoutePolicy {
    let Some(governance) = governance else {
        return RoutePolicy {
            timeout: None,
            rate_limit: None,
            breaker: None,
            shedding: None,
            fallback: None,
        };
    };
    let policy = governance.resolve_policy(route);
    let rate_limit = governance.resolve_rate_limit_config(route);
    RoutePolicy {
        timeout: policy.timeout,
        rate_limit: rate_limit.map(|config| RouteRateLimitConfig {
            burst: config.burst,
            refill: Duration::from_millis(config.refill_ms),
            tokens_per_refill: config.tokens_per_refill,
            key: config.key,
        }),
        breaker: policy.breaker.map(|config| RouteBreakerConfig {
            failure_threshold: config.failure_threshold,
            reset_timeout: config.reset_timeout,
        }),
        shedding: policy.shedding.map(|config| RouteSheddingConfig {
            concurrency: config.concurrency,
            window: config.window,
            min_samples: config.min_samples,
            max_avg_latency: config.max_avg_latency,
            max_failure_ratio_per_mille: config.max_failure_ratio_per_mille,
            cool_down: config.cool_down,
        }),
        fallback: policy
            .fallback
            .map(|fallback| roze_config::GovernanceFallbackConfig {
                enabled: true,
                status: fallback.status,
                body: fallback.body,
                headers: fallback.headers,
            }),
    }
}

pub fn route_fallback(
    governance: Option<&roze_config::GovernanceConfig>,
    route: &str,
) -> Option<roze_config::GovernanceFallbackConfig> {
    route_policy(governance, route).fallback
}

pub fn apply_fallback(
    service: impl Into<String>,
    error: RozeError,
    fallback: Option<roze_config::GovernanceFallbackConfig>,
) -> RozeError {
    if error.is_client_error() {
        return error;
    }
    let Some(fallback) = fallback else {
        return error;
    };
    roze_metrics::record_resilience_decision(service, "rest", "fallback", "served");
    RozeError::fallback_response(fallback.status, fallback.body, fallback.headers)
}

pub fn enforce_permissions<S>(request_ctx: &Context, required: &[S]) -> Result<(), RozeError>
where
    S: AsRef<str>,
{
    if required.is_empty() {
        return Ok(());
    }
    if request_ctx.has_permissions(required.iter().map(AsRef::as_ref)) {
        Ok(())
    } else {
        Err(RozeError::Forbidden)
    }
}

pub fn begin_route(
    service: String,
    route: impl Into<String>,
    method: impl Into<String>,
    request_ctx: Context,
    governance: Option<&roze_config::GovernanceConfig>,
) -> Result<(Context, RouteGuard), RozeError> {
    let route = route.into();
    let method = method.into();
    let policy = route_policy(governance, &route);
    let key = OperationKey::new(
        &service,
        GovernanceBoundary::Rest,
        format!("{method}:{route}"),
    )
    .to_string();
    let breaker_permit = match policy.breaker {
        Some(_) => match route_breaker_allow(&key) {
            BreakerDecision::Allow(permit) => {
                roze_metrics::record_resilience_decision(
                    service.as_str(),
                    "rest",
                    "breaker",
                    if permit == BreakerPermit::Probe {
                        "half_open_probe"
                    } else {
                        "allowed"
                    },
                );
                Some(permit)
            }
            BreakerDecision::Reject => {
                roze_metrics::record_resilience_decision(
                    service.as_str(),
                    "rest",
                    "breaker",
                    "open",
                );
                return Err(RozeError::Unavailable("circuit open".to_string()));
            }
        },
        None => None,
    };
    if let Some(config) = &policy.shedding {
        match enforce_route_shedding(&key, config) {
            Ok(()) => roze_metrics::record_resilience_decision(
                service.as_str(),
                "rest",
                "load_shedding",
                "allowed",
            ),
            Err(err) => {
                roze_metrics::record_resilience_decision(
                    service.as_str(),
                    "rest",
                    "load_shedding",
                    "shed",
                );
                return Err(err);
            }
        }
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
            started: Instant::now(),
            breaker: policy.breaker,
            breaker_permit,
            shedding: policy.shedding,
            finished: false,
        },
    ))
}

pub fn finish_route(mut guard: RouteGuard, success: bool, status: impl Into<String>) {
    let status = status.into();
    let elapsed = guard.started.elapsed();
    if let (Some(config), Some(permit)) = (guard.breaker, guard.breaker_permit) {
        let breaker_success = success || !status.starts_with('5');
        route_breaker_record(&guard.key, permit, breaker_success, &config);
    }
    if let Some(config) = guard.shedding {
        let shedding_success = success || !status.starts_with('5');
        route_shedding_record(&guard.key, shedding_success, elapsed, &config);
    }
    guard.finished = true;
    roze_metrics::record_http_request(success, elapsed);
    roze_metrics::record_http_route(
        guard.service.as_str(),
        guard.route.as_str(),
        guard.method.as_str(),
        status,
        elapsed,
    );
}

#[allow(clippy::too_many_arguments)]
pub async fn enforce_route_rate_limit(
    limiter: &roze_rate_limit::RateLimiter,
    service: &str,
    route: &str,
    method: &str,
    request_ctx: &Context,
    client_ip: Option<roze_http::client_ip::ClientIp>,
    headers: &roze_http::http::HeaderMap,
    governance: Option<&roze_config::GovernanceConfig>,
) -> Result<(), RozeError> {
    let Some(config) = route_policy(governance, route).rate_limit else {
        return Ok(());
    };
    let identity =
        roze_rate_limit::RateLimitIdentity::new(service, "rest", format!("{method}:{route}"))
            .with_client_ip(client_ip.map(|value| value.0))
            .with_subject(request_ctx.subject())
            .with_tenant(request_ctx.tenant())
            .with_headers(headers.iter().filter_map(|(name, value)| {
                Some((name.as_str(), value.to_str().ok()?.to_string()))
            }));
    let decision = limiter
        .check(
            &config.key,
            &identity,
            roze_rate_limit::RateLimit {
                burst: config.burst,
                refill: config.refill,
                tokens_per_refill: config.tokens_per_refill,
            },
        )
        .await;
    match decision {
        Ok(decision) if decision.allowed => {
            roze_metrics::record_resilience_decision(
                service,
                "rest",
                "rate_limit",
                if decision.degraded {
                    "store_error_fail_open"
                } else {
                    "allowed"
                },
            );
            Ok(())
        }
        Ok(decision) => {
            roze_metrics::record_resilience_decision(service, "rest", "rate_limit", "rejected");
            Err(RozeError::rate_limited(decision.retry_after))
        }
        Err(roze_rate_limit::RateLimitError::StoreUnavailable) => {
            roze_metrics::record_resilience_decision(
                service,
                "rest",
                "rate_limit",
                "store_error_fail_closed",
            );
            Err(RozeError::Unavailable(
                "rate limit store unavailable".to_string(),
            ))
        }
        Err(_) => {
            roze_metrics::record_resilience_decision(
                service,
                "rest",
                "rate_limit",
                "identity_rejected",
            );
            Err(RozeError::rate_limited(Duration::from_secs(1)))
        }
    }
}

fn route_breaker_allow(key: &str) -> BreakerDecision {
    ROUTE_BREAKERS.get_or_init(BreakerRegistry::new).allow(key)
}

fn route_breaker_record(
    key: &str,
    permit: BreakerPermit,
    success: bool,
    config: &RouteBreakerConfig,
) {
    let registry = ROUTE_BREAKERS.get_or_init(BreakerRegistry::new);
    if success {
        registry.record_success(key, permit);
        return;
    }
    registry.record_failure(
        key,
        permit,
        roze_resilience::BreakerConfig {
            failure_threshold: config.failure_threshold,
            reset_timeout: config.reset_timeout,
        },
    );
}

fn route_breaker_cancel(key: &str, permit: BreakerPermit, config: &RouteBreakerConfig) {
    ROUTE_BREAKERS.get_or_init(BreakerRegistry::new).cancel(
        key,
        permit,
        roze_resilience::BreakerConfig {
            failure_threshold: config.failure_threshold,
            reset_timeout: config.reset_timeout,
        },
    );
}

fn enforce_route_shedding(key: &str, config: &RouteSheddingConfig) -> Result<(), RozeError> {
    if ROUTE_SHEDDERS
        .get_or_init(SheddingRegistry::new)
        .allow(key, route_shedding_config(*config))
    {
        Ok(())
    } else {
        Err(RozeError::Unavailable("load shed".to_string()))
    }
}

fn route_shedding_record(
    key: &str,
    success: bool,
    elapsed: Duration,
    config: &RouteSheddingConfig,
) {
    ROUTE_SHEDDERS.get_or_init(SheddingRegistry::new).record(
        key,
        success,
        elapsed,
        route_shedding_config(*config),
    );
}

fn route_shedding_release(key: &str) {
    ROUTE_SHEDDERS
        .get_or_init(SheddingRegistry::new)
        .release(key);
}

fn route_shedding_config(config: RouteSheddingConfig) -> roze_resilience::SheddingConfig {
    roze_resilience::SheddingConfig {
        concurrency: config.concurrency,
        window: config.window,
        min_samples: config.min_samples,
        max_avg_latency: config.max_avg_latency,
        max_failure_ratio_per_mille: config.max_failure_ratio_per_mille,
        cool_down: config.cool_down,
    }
}

pub fn idempotency_key_from_headers<'a>(
    headers: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> Option<String> {
    headers
        .into_iter()
        .find(|(key, _)| key.eq_ignore_ascii_case("idempotency-key"))
        .map(|(_, value)| value.to_string())
}

#[derive(Debug, Clone, PartialEq)]
pub enum IdempotencyDecision {
    Execute,
    Replay(serde_json::Value),
    InFlight,
    Conflict,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IdempotencyPolicy {
    pub lease_millis: u64,
}

impl Default for IdempotencyPolicy {
    fn default() -> Self {
        Self {
            lease_millis: 30_000,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
struct IdempotencyRecord {
    fingerprint: String,
    response: Option<serde_json::Value>,
    lease_until_millis: Option<u64>,
}

#[async_trait]
pub trait IdempotencyStore: std::fmt::Debug + Send + Sync + 'static {
    async fn begin(
        &self,
        scope: &str,
        key: &str,
        fingerprint: &str,
        now_millis: u64,
        policy: IdempotencyPolicy,
    ) -> anyhow::Result<IdempotencyDecision>;
    async fn complete(
        &self,
        scope: &str,
        key: &str,
        fingerprint: &str,
        response: serde_json::Value,
    ) -> anyhow::Result<()>;
    async fn fail(&self, scope: &str, key: &str, fingerprint: &str) -> anyhow::Result<()>;
}

#[derive(Debug, Clone, Default)]
pub struct InMemoryIdempotencyStore {
    states: Arc<Mutex<BTreeMap<String, IdempotencyRecord>>>,
}

#[async_trait]
impl IdempotencyStore for InMemoryIdempotencyStore {
    async fn begin(
        &self,
        scope: &str,
        key: &str,
        fingerprint: &str,
        now_millis: u64,
        policy: IdempotencyPolicy,
    ) -> anyhow::Result<IdempotencyDecision> {
        let key = format!("{scope}:{key}");
        let mut states = self.states.lock().expect("idempotency lock poisoned");
        let decision = match states.get_mut(&key) {
            Some(record) if record.fingerprint != fingerprint => IdempotencyDecision::Conflict,
            Some(record) if record.response.is_some() => {
                IdempotencyDecision::Replay(record.response.clone().expect("response checked"))
            }
            Some(record)
                if record
                    .lease_until_millis
                    .is_some_and(|lease| lease > now_millis) =>
            {
                IdempotencyDecision::InFlight
            }
            Some(record) => {
                record.lease_until_millis = Some(now_millis.saturating_add(policy.lease_millis));
                IdempotencyDecision::Execute
            }
            None => {
                states.insert(
                    key,
                    IdempotencyRecord {
                        fingerprint: fingerprint.to_string(),
                        response: None,
                        lease_until_millis: Some(now_millis.saturating_add(policy.lease_millis)),
                    },
                );
                IdempotencyDecision::Execute
            }
        };
        Ok(decision)
    }

    async fn complete(
        &self,
        scope: &str,
        key: &str,
        fingerprint: &str,
        response: serde_json::Value,
    ) -> anyhow::Result<()> {
        let mut states = self.states.lock().expect("idempotency lock poisoned");
        let record = states
            .get_mut(&format!("{scope}:{key}"))
            .ok_or_else(|| anyhow::anyhow!("idempotency request was not started"))?;
        if record.fingerprint != fingerprint {
            anyhow::bail!("idempotency request fingerprint changed");
        }
        record.response = Some(response);
        record.lease_until_millis = None;
        Ok(())
    }

    async fn fail(&self, scope: &str, key: &str, fingerprint: &str) -> anyhow::Result<()> {
        let storage_key = format!("{scope}:{key}");
        let mut states = self.states.lock().expect("idempotency lock poisoned");
        if states
            .get(&storage_key)
            .is_some_and(|record| record.fingerprint == fingerprint && record.response.is_none())
        {
            states.remove(&storage_key);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RedisIdempotencyConfig {
    pub url: String,
    pub cluster_urls: Vec<String>,
    pub key_prefix: String,
    pub record_ttl_millis: u64,
}

impl RedisIdempotencyConfig {
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            cluster_urls: Vec::new(),
            key_prefix: "roze:idempotency:v1".to_string(),
            record_ttl_millis: 86_400_000,
        }
    }
}

#[derive(Clone)]
pub struct RedisIdempotencyStore {
    client: roze_redis::RedisClient,
    key_prefix: String,
    record_ttl_millis: u64,
}

impl std::fmt::Debug for RedisIdempotencyStore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RedisIdempotencyStore")
            .field("key_prefix", &self.key_prefix)
            .field("record_ttl_millis", &self.record_ttl_millis)
            .finish_non_exhaustive()
    }
}

impl RedisIdempotencyStore {
    pub fn connect(config: RedisIdempotencyConfig) -> anyhow::Result<Self> {
        anyhow::ensure!(
            !config.key_prefix.trim().is_empty(),
            "Redis idempotency key prefix must not be empty"
        );
        anyhow::ensure!(
            config.record_ttl_millis > 0,
            "Redis idempotency record TTL must be positive"
        );
        Ok(Self {
            client: roze_redis::RedisClient::open_topology(&config.url, &config.cluster_urls)?,
            key_prefix: config.key_prefix.trim_end_matches(':').to_string(),
            record_ttl_millis: config.record_ttl_millis,
        })
    }

    fn storage_key(&self, scope: &str, key: &str) -> String {
        format!("{}:{}:{}:{key}", self.key_prefix, scope.len(), scope)
    }

    async fn connection(&self) -> anyhow::Result<roze_redis::RedisConnection> {
        self.client.connection().await
    }

    pub async fn health_check(&self) -> anyhow::Result<()> {
        let mut connection = self.connection().await?;
        let response: String = redis::cmd("PING").query_async(&mut connection).await?;
        anyhow::ensure!(
            response.eq_ignore_ascii_case("PONG"),
            "Redis idempotency health check returned an unexpected response"
        );
        Ok(())
    }
}

const REDIS_IDEMPOTENCY_BEGIN: &str = r#"
local fingerprint = redis.call('HGET', KEYS[1], 'fingerprint')
if fingerprint then
  if fingerprint ~= ARGV[1] then return {'conflict', ''} end
  local response = redis.call('HGET', KEYS[1], 'response')
  if response then return {'replay', response} end
  local lease = tonumber(redis.call('HGET', KEYS[1], 'lease_until') or '0')
  if lease > tonumber(ARGV[2]) then return {'in_flight', ''} end
else
  redis.call('HSET', KEYS[1], 'fingerprint', ARGV[1])
end
redis.call('HSET', KEYS[1], 'lease_until', ARGV[3])
redis.call('PEXPIRE', KEYS[1], ARGV[4])
return {'execute', ''}
"#;

const REDIS_IDEMPOTENCY_COMPLETE: &str = r#"
local fingerprint = redis.call('HGET', KEYS[1], 'fingerprint')
if not fingerprint then return 0 end
if fingerprint ~= ARGV[1] then return -1 end
redis.call('HSET', KEYS[1], 'response', ARGV[2])
redis.call('HDEL', KEYS[1], 'lease_until')
redis.call('PEXPIRE', KEYS[1], ARGV[3])
return 1
"#;

const REDIS_IDEMPOTENCY_FAIL: &str = r#"
local fingerprint = redis.call('HGET', KEYS[1], 'fingerprint')
if fingerprint == ARGV[1] and redis.call('HEXISTS', KEYS[1], 'response') == 0 then
  return redis.call('DEL', KEYS[1])
end
return 0
"#;

#[async_trait]
impl IdempotencyStore for RedisIdempotencyStore {
    async fn begin(
        &self,
        scope: &str,
        key: &str,
        fingerprint: &str,
        now_millis: u64,
        policy: IdempotencyPolicy,
    ) -> anyhow::Result<IdempotencyDecision> {
        let mut connection = self.connection().await?;
        let lease_until = now_millis.saturating_add(policy.lease_millis);
        let (decision, response): (String, String) = redis::Script::new(REDIS_IDEMPOTENCY_BEGIN)
            .key(self.storage_key(scope, key))
            .arg(fingerprint)
            .arg(now_millis)
            .arg(lease_until)
            .arg(self.record_ttl_millis)
            .invoke_async(&mut connection)
            .await?;
        match decision.as_str() {
            "execute" => Ok(IdempotencyDecision::Execute),
            "in_flight" => Ok(IdempotencyDecision::InFlight),
            "conflict" => Ok(IdempotencyDecision::Conflict),
            "replay" => Ok(IdempotencyDecision::Replay(serde_json::from_str(
                &response,
            )?)),
            other => anyhow::bail!("unknown Redis idempotency decision: {other}"),
        }
    }

    async fn complete(
        &self,
        scope: &str,
        key: &str,
        fingerprint: &str,
        response: serde_json::Value,
    ) -> anyhow::Result<()> {
        let mut connection = self.connection().await?;
        let result: i64 = redis::Script::new(REDIS_IDEMPOTENCY_COMPLETE)
            .key(self.storage_key(scope, key))
            .arg(fingerprint)
            .arg(serde_json::to_string(&response)?)
            .arg(self.record_ttl_millis)
            .invoke_async(&mut connection)
            .await?;
        match result {
            1 => Ok(()),
            0 => anyhow::bail!("idempotency request was not started"),
            -1 => anyhow::bail!("idempotency request fingerprint changed"),
            other => anyhow::bail!("unknown Redis idempotency completion result: {other}"),
        }
    }

    async fn fail(&self, scope: &str, key: &str, fingerprint: &str) -> anyhow::Result<()> {
        let mut connection = self.connection().await?;
        let _: i64 = redis::Script::new(REDIS_IDEMPOTENCY_FAIL)
            .key(self.storage_key(scope, key))
            .arg(fingerprint)
            .invoke_async(&mut connection)
            .await?;
        Ok(())
    }
}

pub const IDEMPOTENCY_MISSING_KEY: &str = "IDEMPOTENCY_MISSING_KEY";
pub const IDEMPOTENCY_IN_FLIGHT: &str = "IDEMPOTENCY_IN_FLIGHT";
pub const IDEMPOTENCY_KEY_REUSED: &str = "IDEMPOTENCY_KEY_REUSED";
pub const IDEMPOTENCY_STORAGE_UNAVAILABLE: &str = "IDEMPOTENCY_STORAGE_UNAVAILABLE";
pub const IDEMPOTENCY_REPLAY_INVALID: &str = "IDEMPOTENCY_REPLAY_INVALID";

pub fn idempotency_fingerprint(value: &impl Serialize) -> Result<String, RozeError> {
    serde_json::to_string(value)
        .map_err(|error| RozeError::Internal(format!("idempotency fingerprint failed: {error}")))
}

pub fn idempotency_now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

pub fn idempotency_error(status: u16, code: &str, message: &str) -> RozeError {
    let mut headers = BTreeMap::new();
    headers.insert("x-roze-error-code".to_string(), code.to_string());
    RozeError::fallback_response(
        status,
        Some(serde_json::json!({"code": code, "message": message})),
        headers,
    )
}

pub async fn begin_idempotency(
    store: &dyn IdempotencyStore,
    scope: &str,
    key: &str,
    fingerprint: &str,
    now_millis: u64,
) -> Result<IdempotencyDecision, RozeError> {
    store
        .begin(
            scope,
            key,
            fingerprint,
            now_millis,
            IdempotencyPolicy::default(),
        )
        .await
        .map_err(|error| {
            idempotency_error(
                503,
                IDEMPOTENCY_STORAGE_UNAVAILABLE,
                &format!("idempotency storage unavailable: {error}"),
            )
        })
}

pub async fn complete_idempotency(
    store: &dyn IdempotencyStore,
    scope: &str,
    key: &str,
    fingerprint: &str,
    response: serde_json::Value,
) -> Result<(), RozeError> {
    store
        .complete(scope, key, fingerprint, response)
        .await
        .map_err(|error| {
            idempotency_error(
                503,
                IDEMPOTENCY_STORAGE_UNAVAILABLE,
                &format!("idempotency storage unavailable: {error}"),
            )
        })
}

pub async fn fail_idempotency(
    store: &dyn IdempotencyStore,
    scope: &str,
    key: &str,
    fingerprint: &str,
) {
    if let Err(error) = store.fail(scope, key, fingerprint).await {
        tracing::warn!(scope, key, error = %error, "failed to release idempotency request");
    }
}

pub type RateLimitConfig = roze_resilience::RateLimitConfig;

#[cfg(test)]
mod tests {
    use super::*;

    fn test_auth_config(expiration_secs: u64) -> AuthConfig {
        AuthConfig {
            jwt: roze_jwt::JwtConfig {
                jwt_keys: vec![roze_jwt::JwtKey {
                    id: "test-key".into(),
                    secret: "test-secret-at-least-32-bytes-long".into(),
                }],
                jwt_active_key_id: "test-key".into(),
                jwt_issuer: "https://issuer.example".into(),
                jwt_audience: "roze-tests".into(),
                jwt_expiration_secs: expiration_secs,
                jwt_clock_skew_secs: 0,
                revoked_token_ids: Vec::new(),
            },
            public_routes: vec!["GET /public".into()],
        }
    }

    fn issue_test_token(config: &AuthConfig) -> String {
        roze_jwt::issue_token(
            &roze_jwt::Claims {
                sub: "verified-user".into(),
                roles: vec!["admin".into()],
                tenant: Some("tenant-1".into()),
                permissions: vec!["orders:read".into()],
                scopes: vec!["orders".into()],
                iss: String::new(),
                aud: String::new(),
                jti: "token-1".into(),
                iat: 0,
                exp: 0,
            },
            &config.jwt,
        )
        .expect("issue token")
    }

    fn authenticated_context_app(config: AuthConfig) -> roze_http::Router {
        use roze_http::{extract::Extension, routing::get, Router};

        apply_common_with_config(
            Router::new()
                .route(
                    "/protected",
                    get(|Extension(ctx): Extension<Context>| async move {
                        format!(
                            "{}|{}|{}|{}",
                            ctx.subject().unwrap_or_default(),
                            ctx.tenant().unwrap_or_default(),
                            ctx.roles().join(","),
                            ctx.permissions().join(",")
                        )
                    }),
                )
                .route("/public", get(|| async { "public" })),
            CommonMiddlewareConfig {
                auth: Some(config),
                cors: false,
                ..Default::default()
            },
        )
    }

    #[tokio::test]
    async fn common_middleware_injects_request_context() {
        use roze_http::{extract::Extension, routing::get, Router};
        use tower::ServiceExt as _;

        let app = apply_common(Router::new().route(
            "/context",
            get(|Extension(ctx): Extension<Context>| async move { ctx.request_id().to_string() }),
        ));
        let request = roze_http::http::Request::builder()
            .uri("/context")
            .header(roze_context::REQUEST_ID_HEADER, "request-123")
            .header(roze_context::TRACE_ID_HEADER, "trace-456")
            .body(roze_http::body::empty())
            .expect("request");

        let response = app.oneshot(request).await.expect("infallible router");
        assert_eq!(response.status(), roze_http::http::StatusCode::OK);
        assert_eq!(
            response.headers()[roze_context::REQUEST_ID_HEADER],
            "request-123"
        );
        assert_eq!(
            response.headers()[roze_context::TRACE_ID_HEADER],
            "trace-456"
        );
        let body = roze_http::body::to_bytes(response.into_body(), 128)
            .await
            .expect("response body");
        assert_eq!(&body[..], b"request-123");
    }

    #[tokio::test]
    async fn common_auth_rejects_missing_and_invalid_tokens() {
        use tower::ServiceExt as _;

        let app = authenticated_context_app(test_auth_config(3_600));
        for authorization in [None, Some("Bearer invalid")] {
            let mut request = roze_http::http::Request::builder().uri("/protected");
            if let Some(value) = authorization {
                request = request.header(roze_http::http::header::AUTHORIZATION, value);
            }
            let response = app
                .clone()
                .oneshot(
                    request
                        .body(roze_http::body::empty())
                        .expect("protected request"),
                )
                .await
                .expect("infallible router");
            assert_eq!(response.status(), roze_http::http::StatusCode::UNAUTHORIZED);
            assert_eq!(
                response.headers()[roze_http::http::header::WWW_AUTHENTICATE],
                "Bearer"
            );
        }
    }

    #[tokio::test]
    async fn common_auth_populates_context_from_verified_claims() {
        use tower::ServiceExt as _;

        let config = test_auth_config(3_600);
        let token = issue_test_token(&config);
        let app = authenticated_context_app(config);
        let request = roze_http::http::Request::builder()
            .uri("/protected")
            .header(
                roze_http::http::header::AUTHORIZATION,
                format!("Bearer {token}"),
            )
            .header(roze_context::SUBJECT_HEADER, "forged-user")
            .body(roze_http::body::empty())
            .expect("protected request");
        let response = app.oneshot(request).await.expect("infallible router");
        assert_eq!(response.status(), roze_http::http::StatusCode::OK);
        let body = roze_http::body::to_bytes(response.into_body(), 256)
            .await
            .expect("response body");
        assert_eq!(&body[..], b"verified-user|tenant-1|admin|orders:read");
    }

    #[tokio::test]
    async fn common_auth_rejects_expired_tokens_and_allows_public_routes() {
        use tower::ServiceExt as _;

        let config = test_auth_config(0);
        let token = issue_test_token(&config);
        tokio::time::sleep(Duration::from_millis(1_100)).await;
        let app = authenticated_context_app(config);
        let expired = roze_http::http::Request::builder()
            .uri("/protected")
            .header(
                roze_http::http::header::AUTHORIZATION,
                format!("Bearer {token}"),
            )
            .body(roze_http::body::empty())
            .expect("protected request");
        let response = app
            .clone()
            .oneshot(expired)
            .await
            .expect("infallible router");
        assert_eq!(response.status(), roze_http::http::StatusCode::UNAUTHORIZED);

        let public = roze_http::http::Request::builder()
            .uri("/public")
            .body(roze_http::body::empty())
            .expect("public request");
        let response = app.oneshot(public).await.expect("infallible router");
        assert_eq!(response.status(), roze_http::http::StatusCode::OK);
    }

    #[tokio::test]
    async fn request_context_strips_identity_headers_unless_proxy_is_trusted() {
        use roze_http::{extract::Extension, routing::get, Router};
        use tower::ServiceExt as _;

        let router = || {
            Router::new().route(
                "/identity",
                get(|Extension(ctx): Extension<Context>| async move {
                    ctx.subject().unwrap_or_default()
                }),
            )
        };
        let request = || {
            roze_http::http::Request::builder()
                .uri("/identity")
                .header(roze_context::SUBJECT_HEADER, "forged-user")
                .body(roze_http::body::empty())
                .expect("identity request")
        };

        let response = apply_common(router())
            .oneshot(request())
            .await
            .expect("infallible router");
        let body = roze_http::body::to_bytes(response.into_body(), 128)
            .await
            .expect("response body");
        assert!(body.is_empty());

        let trusted = apply_common_with_config(
            router(),
            CommonMiddlewareConfig {
                trust_forwarded_identity_headers: true,
                ..Default::default()
            },
        );
        let response = trusted.oneshot(request()).await.expect("infallible router");
        let body = roze_http::body::to_bytes(response.into_body(), 128)
            .await
            .expect("response body");
        assert_eq!(&body[..], b"forged-user");
    }

    #[tokio::test]
    async fn trusted_proxy_middleware_injects_resolved_client_ip() {
        use roze_http::{
            client_ip::{ClientIp, TrustedProxyConfig},
            extract::ConnectInfo,
            routing::get,
            Router,
        };
        use tower::ServiceExt as _;

        let router = Router::new().route(
            "/client-ip",
            get(|client: ClientIp| async move { client.to_string() }),
        );
        let app = apply_common_with_config(
            router,
            CommonMiddlewareConfig {
                trusted_proxies: Some(
                    TrustedProxyConfig::new(["10.0.0.0/8"]).expect("trusted proxies"),
                ),
                ..Default::default()
            },
        );
        let mut request = roze_http::http::Request::builder()
            .uri("/client-ip")
            .header("x-forwarded-for", "198.51.100.8, 10.1.0.4")
            .body(roze_http::body::empty())
            .expect("request");
        request.extensions_mut().insert(ConnectInfo(
            "10.2.0.5:443".parse::<SocketAddr>().expect("peer"),
        ));

        let response = app.oneshot(request).await.expect("infallible router");
        let body = roze_http::body::to_bytes(response.into_body(), 128)
            .await
            .expect("response body");
        assert_eq!(&body[..], b"198.51.100.8");
    }

    #[tokio::test]
    async fn timeout_middleware_returns_gateway_timeout() {
        use roze_http::{routing::get, Router};
        use tower::ServiceExt as _;

        let app = apply_timeout(
            Router::new().route(
                "/slow",
                get(|| async {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    "late"
                }),
            ),
            5,
        );
        let request = roze_http::http::Request::builder()
            .uri("/slow")
            .body(roze_http::body::empty())
            .expect("request");

        let response = app.oneshot(request).await.expect("infallible router");
        assert_eq!(
            response.status(),
            roze_http::http::StatusCode::GATEWAY_TIMEOUT
        );
        let body = roze_http::body::to_bytes(response.into_body(), 128)
            .await
            .expect("response body");
        assert_eq!(&body[..], b"request timeout");
    }

    #[tokio::test]
    async fn common_middleware_rejects_oversized_request_body() {
        use roze_http::{routing::post, Router};
        use tower::ServiceExt as _;

        let app = apply_common_with_config(
            Router::new().route("/upload", post(|body: String| async move { body })),
            CommonMiddlewareConfig {
                body_limit_bytes: Some(4),
                ..Default::default()
            },
        );
        let request = roze_http::http::Request::builder()
            .method(roze_http::http::Method::POST)
            .uri("/upload")
            .body(roze_http::body::full("12345"))
            .expect("request");

        let response = app
            .clone()
            .oneshot(request)
            .await
            .expect("infallible router");
        assert_eq!(
            response.status(),
            roze_http::http::StatusCode::PAYLOAD_TOO_LARGE
        );
        let body = roze_http::body::to_bytes(response.into_body(), 128)
            .await
            .expect("response body");
        assert_eq!(&body[..], b"request body too large");

        let request = roze_http::http::Request::builder()
            .method(roze_http::http::Method::POST)
            .uri("/upload")
            .body(roze_http::body::full("1234"))
            .expect("request");
        let response = app.oneshot(request).await.expect("infallible router");
        assert_eq!(response.status(), roze_http::http::StatusCode::OK);
        let body = roze_http::body::to_bytes(response.into_body(), 128)
            .await
            .expect("response body");
        assert_eq!(&body[..], b"1234");
    }

    #[derive(serde::Deserialize)]
    struct LargeJsonPayload {
        value: String,
    }

    #[derive(serde::Deserialize)]
    struct LargeFormPayload {
        value: String,
    }

    struct CustomBody(roze_http::body::Bytes);

    impl roze_http::extract::FromRequest for CustomBody {
        type Rejection = RozeError;

        fn from_request(
            request: roze_http::IncomingRequest,
        ) -> roze_http::extract::ExtractFuture<'static, Self, Self::Rejection> {
            Box::pin(async move {
                Ok(Self(
                    <roze_http::body::Bytes as roze_http::extract::FromRequest>::from_request(
                        request,
                    )
                    .await?,
                ))
            })
        }
    }

    #[tokio::test]
    async fn configured_body_limit_raises_json_extractor_limit() {
        use roze_http::{routing::post, Json, Router};
        use tower::ServiceExt as _;

        let value = "a".repeat(roze_http::extract::DEFAULT_BODY_LIMIT + 1);
        let body = serde_json::to_vec(&serde_json::json!({ "value": value })).unwrap();
        let router = || {
            Router::new().route(
                "/json",
                post(|Json(payload): Json<LargeJsonPayload>| async move {
                    payload.value.len().to_string()
                }),
            )
        };
        let request = || {
            roze_http::http::Request::builder()
                .method(roze_http::http::Method::POST)
                .uri("/json")
                .header(roze_http::http::header::CONTENT_TYPE, "application/json")
                .body(roze_http::body::full(body.clone()))
                .unwrap()
        };

        let native_default = apply_common(router())
            .oneshot(request())
            .await
            .expect("native default response");
        assert_eq!(
            native_default.status(),
            roze_http::http::StatusCode::BAD_REQUEST
        );

        let configured = apply_common_with_config(
            router(),
            CommonMiddlewareConfig {
                body_limit_bytes: Some(32 * 1024 * 1024),
                ..Default::default()
            },
        )
        .oneshot(request())
        .await
        .expect("configured response");
        assert_eq!(configured.status(), roze_http::http::StatusCode::OK);

        let malformed_oversized = roze_http::http::Request::builder()
            .method(roze_http::http::Method::POST)
            .uri("/json")
            .header(roze_http::http::header::CONTENT_TYPE, "application/json")
            .body(roze_http::body::full("not-json-and-too-large"))
            .unwrap();
        let rejected = apply_common_with_config(
            router(),
            CommonMiddlewareConfig {
                body_limit_bytes: Some(8),
                ..Default::default()
            },
        )
        .oneshot(malformed_oversized)
        .await
        .expect("oversized JSON response");
        assert_eq!(
            rejected.status(),
            roze_http::http::StatusCode::PAYLOAD_TOO_LARGE
        );
    }

    #[tokio::test]
    async fn configured_body_limit_covers_form_and_custom_extractors_without_content_length() {
        use roze_http::{extract::Form, routing::post, Router};
        use tower::ServiceExt as _;

        let value = "a".repeat(roze_http::extract::DEFAULT_BODY_LIMIT + 1);
        let form_body = format!("value={value}");
        let app = apply_common_with_config(
            Router::new()
                .route(
                    "/form",
                    post(|Form(payload): Form<LargeFormPayload>| async move {
                        payload.value.len().to_string()
                    }),
                )
                .route(
                    "/custom",
                    post(|body: CustomBody| async move {
                        format!("{}:{}", body.0.len(), body.0.as_ptr() as usize)
                    }),
                ),
            CommonMiddlewareConfig {
                body_limit_bytes: Some(32 * 1024 * 1024),
                ..Default::default()
            },
        );

        let form_request = roze_http::http::Request::builder()
            .method(roze_http::http::Method::POST)
            .uri("/form")
            .header(
                roze_http::http::header::CONTENT_TYPE,
                "application/x-www-form-urlencoded",
            )
            .body(roze_http::body::full(form_body.clone()))
            .unwrap();
        assert!(!form_request
            .headers()
            .contains_key(roze_http::http::header::CONTENT_LENGTH));
        let form_response = app
            .clone()
            .oneshot(form_request)
            .await
            .expect("form response");
        assert_eq!(form_response.status(), roze_http::http::StatusCode::OK);

        let custom_payload = roze_http::body::Bytes::from(value);
        let original_storage = custom_payload.clone();
        let custom_request = roze_http::http::Request::builder()
            .method(roze_http::http::Method::POST)
            .uri("/custom")
            .body(roze_http::body::full(custom_payload))
            .unwrap();
        let custom_response = app.oneshot(custom_request).await.expect("custom response");
        assert_eq!(custom_response.status(), roze_http::http::StatusCode::OK);
        let custom_response_body = roze_http::body::to_bytes(custom_response.into_body(), 64)
            .await
            .expect("custom response body");
        let (length, pointer) = std::str::from_utf8(&custom_response_body)
            .unwrap()
            .split_once(':')
            .unwrap();
        assert_eq!(length.parse::<usize>().unwrap(), original_storage.len());
        assert_eq!(
            pointer.parse::<usize>().unwrap(),
            original_storage.as_ptr() as usize
        );
    }

    #[tokio::test]
    async fn configured_body_limit_rejects_decompressed_body_before_extraction() {
        use roze_http::{routing::post, Router};
        use tower::ServiceExt as _;

        const GZIP_12345: &[u8] = &[
            31, 139, 8, 0, 0, 0, 0, 0, 4, 0, 51, 52, 50, 54, 49, 5, 0, 28, 58, 245, 203, 5, 0, 0, 0,
        ];
        let router = || Router::new().route("/gzip", post(|body: String| async move { body }));
        let request = || {
            roze_http::http::Request::builder()
                .method(roze_http::http::Method::POST)
                .uri("/gzip")
                .header(roze_http::http::header::CONTENT_ENCODING, "gzip")
                .body(roze_http::body::full(GZIP_12345))
                .unwrap()
        };

        let rejected = apply_common_with_config(
            router(),
            CommonMiddlewareConfig {
                gunzip: true,
                body_limit_bytes: Some(4),
                ..Default::default()
            },
        )
        .oneshot(request())
        .await
        .expect("rejected response");
        assert_eq!(
            rejected.status(),
            roze_http::http::StatusCode::PAYLOAD_TOO_LARGE
        );

        let accepted = apply_common_with_config(
            router(),
            CommonMiddlewareConfig {
                gunzip: true,
                body_limit_bytes: Some(5),
                ..Default::default()
            },
        )
        .oneshot(request())
        .await
        .expect("accepted response");
        assert_eq!(accepted.status(), roze_http::http::StatusCode::OK);
    }

    #[tokio::test]
    async fn common_middleware_applies_configured_cors_policy() {
        use roze_http::{routing::get, Router};
        use tower::ServiceExt as _;

        let app = apply_common_with_config(
            Router::new().route("/data", get(|| async { "ok" })),
            CommonMiddlewareConfig {
                cors: true,
                cors_config: Some(CorsConfig {
                    allow_origins: vec!["https://app.example.com".into()],
                    allow_methods: vec!["GET".into()],
                    allow_headers: vec!["authorization".into()],
                    expose_headers: vec!["x-request-id".into()],
                    allow_credentials: true,
                    max_age_seconds: Some(600),
                }),
                ..Default::default()
            },
        );
        let preflight = roze_http::http::Request::builder()
            .method(roze_http::http::Method::OPTIONS)
            .uri("/data")
            .header("origin", "https://app.example.com")
            .header("access-control-request-method", "GET")
            .header("access-control-request-headers", "authorization")
            .body(roze_http::body::empty())
            .expect("preflight");
        let response = app
            .clone()
            .oneshot(preflight)
            .await
            .expect("infallible router");
        assert_eq!(response.status(), roze_http::http::StatusCode::OK);
        assert_eq!(
            response.headers()["access-control-allow-origin"],
            "https://app.example.com"
        );
        assert_eq!(response.headers()["access-control-allow-methods"], "GET");
        assert_eq!(
            response.headers()["access-control-allow-headers"],
            "authorization"
        );
        assert_eq!(response.headers()["access-control-max-age"], "600");
        assert_eq!(
            response.headers()["access-control-allow-credentials"],
            "true"
        );

        let request = roze_http::http::Request::builder()
            .uri("/data")
            .header("origin", "https://app.example.com")
            .body(roze_http::body::empty())
            .expect("request");
        let response = app
            .clone()
            .oneshot(request)
            .await
            .expect("infallible router");
        assert_eq!(
            response.headers()["access-control-expose-headers"],
            "x-request-id"
        );

        let request = roze_http::http::Request::builder()
            .uri("/data")
            .header("origin", "https://blocked.example.com")
            .body(roze_http::body::empty())
            .expect("request");
        let response = app.oneshot(request).await.expect("infallible router");
        assert!(!response
            .headers()
            .contains_key("access-control-allow-origin"));
    }

    #[tokio::test]
    async fn common_middleware_can_disable_cors() {
        use roze_http::{routing::get, Router};
        use tower::ServiceExt as _;

        let app = apply_common_with_config(
            Router::new().route("/data", get(|| async { "ok" })),
            CommonMiddlewareConfig {
                cors: false,
                ..Default::default()
            },
        );
        let request = roze_http::http::Request::builder()
            .uri("/data")
            .header("origin", "https://app.example.com")
            .body(roze_http::body::empty())
            .expect("request");
        let response = app.oneshot(request).await.expect("infallible router");
        assert!(!response
            .headers()
            .contains_key("access-control-allow-origin"));
    }

    #[test]
    fn parses_builtin_middleware_names() {
        assert_eq!(
            BuiltInMiddleware::parse("auth"),
            Some(BuiltInMiddleware::Auth)
        );
        assert_eq!(
            BuiltInMiddleware::parse("rate_limit"),
            Some(BuiltInMiddleware::RateLimit)
        );
        assert_eq!(BuiltInMiddleware::parse("custom"), None);
    }

    #[test]
    fn resolves_middleware_plan() {
        let plan = resolve_middleware_plan(&["auth".into(), "audit".into()]);
        assert_eq!(plan.builtins, vec![BuiltInMiddleware::Auth]);
        assert_eq!(plan.custom, vec!["audit"]);
    }

    #[tokio::test]
    async fn idempotency_store_replays_completed_requests_and_releases_failures() {
        let store = InMemoryIdempotencyStore::default();
        assert_eq!(
            store
                .begin(
                    "create-order",
                    "key-1",
                    "fingerprint-1",
                    100,
                    IdempotencyPolicy::default(),
                )
                .await
                .unwrap(),
            IdempotencyDecision::Execute
        );
        assert_eq!(
            store
                .begin(
                    "create-order",
                    "key-1",
                    "fingerprint-1",
                    101,
                    IdempotencyPolicy::default(),
                )
                .await
                .unwrap(),
            IdempotencyDecision::InFlight
        );
        assert_eq!(
            store
                .begin(
                    "create-order",
                    "key-1",
                    "fingerprint-2",
                    101,
                    IdempotencyPolicy::default(),
                )
                .await
                .unwrap(),
            IdempotencyDecision::Conflict
        );
        store
            .complete(
                "create-order",
                "key-1",
                "fingerprint-1",
                serde_json::json!({"id": 1}),
            )
            .await
            .unwrap();
        assert_eq!(
            store
                .begin(
                    "create-order",
                    "key-1",
                    "fingerprint-1",
                    102,
                    IdempotencyPolicy::default(),
                )
                .await
                .unwrap(),
            IdempotencyDecision::Replay(serde_json::json!({"id": 1}))
        );

        assert_eq!(
            store
                .begin(
                    "create-order",
                    "key-2",
                    "fingerprint-2",
                    100,
                    IdempotencyPolicy::default(),
                )
                .await
                .unwrap(),
            IdempotencyDecision::Execute
        );
        store
            .fail("create-order", "key-2", "fingerprint-2")
            .await
            .unwrap();
        assert_eq!(
            store
                .begin(
                    "create-order",
                    "key-2",
                    "fingerprint-2",
                    101,
                    IdempotencyPolicy::default(),
                )
                .await
                .unwrap(),
            IdempotencyDecision::Execute
        );
    }

    #[tokio::test]
    async fn idempotency_store_recovers_expired_processing_lease() {
        let store = InMemoryIdempotencyStore::default();
        let policy = IdempotencyPolicy { lease_millis: 10 };
        assert_eq!(
            store
                .begin("orders", "key", "body", 100, policy)
                .await
                .unwrap(),
            IdempotencyDecision::Execute
        );
        assert_eq!(
            store
                .begin("orders", "key", "body", 109, policy)
                .await
                .unwrap(),
            IdempotencyDecision::InFlight
        );
        assert_eq!(
            store
                .begin("orders", "key", "body", 110, policy)
                .await
                .unwrap(),
            IdempotencyDecision::Execute
        );
    }

    #[test]
    fn redis_idempotency_configuration_is_validated_and_debug_is_redacted() {
        let mut config = RedisIdempotencyConfig::new("redis://user:secret@127.0.0.1/");
        config.key_prefix = "test:idempotency".into();
        let store = RedisIdempotencyStore::connect(config).expect("store");
        assert_eq!(store.storage_key("ab", "c:d"), "test:idempotency:2:ab:c:d");
        let debug = format!("{store:?}");
        assert!(!debug.contains("secret"));
        assert!(debug.contains("test:idempotency"));

        let mut invalid = RedisIdempotencyConfig::new("redis://127.0.0.1/");
        invalid.record_ttl_millis = 0;
        assert!(RedisIdempotencyStore::connect(invalid).is_err());
    }

    #[tokio::test]
    #[ignore = "requires ROZE_TEST_REDIS_URL"]
    async fn redis_idempotency_store_runs_atomic_state_machine() {
        let url = std::env::var("ROZE_TEST_REDIS_URL").expect("ROZE_TEST_REDIS_URL is required");
        let mut config = RedisIdempotencyConfig::new(url);
        config.key_prefix = format!("roze:test:idempotency:{}", std::process::id());
        config.record_ttl_millis = 60_000;
        let store = RedisIdempotencyStore::connect(config).expect("store");
        let scope = "create-order";
        let key = format!("key-{}", idempotency_now_millis());
        let policy = IdempotencyPolicy { lease_millis: 10 };

        assert_eq!(
            store
                .begin(scope, &key, "body-1", 100, policy)
                .await
                .unwrap(),
            IdempotencyDecision::Execute
        );
        assert_eq!(
            store
                .begin(scope, &key, "body-1", 109, policy)
                .await
                .unwrap(),
            IdempotencyDecision::InFlight
        );
        assert_eq!(
            store
                .begin(scope, &key, "body-2", 110, policy)
                .await
                .unwrap(),
            IdempotencyDecision::Conflict
        );
        assert_eq!(
            store
                .begin(scope, &key, "body-1", 110, policy)
                .await
                .unwrap(),
            IdempotencyDecision::Execute
        );
        store
            .complete(scope, &key, "body-1", serde_json::json!({"id": 1}))
            .await
            .unwrap();
        assert_eq!(
            store
                .begin(scope, &key, "body-1", 111, policy)
                .await
                .unwrap(),
            IdempotencyDecision::Replay(serde_json::json!({"id": 1}))
        );

        let failed_key = format!("{key}-failed");
        store
            .begin(scope, &failed_key, "body", 100, policy)
            .await
            .unwrap();
        store.fail(scope, &failed_key, "body").await.unwrap();
        assert_eq!(
            store
                .begin(scope, &failed_key, "body", 101, policy)
                .await
                .unwrap(),
            IdempotencyDecision::Execute
        );
        store.fail(scope, &failed_key, "body").await.unwrap();
    }

    #[test]
    fn route_policy_prefers_route_override() {
        let mut governance = roze_config::GovernanceConfig {
            timeout_ms: Some(1_000),
            rate_limit: Some(roze_config::RateLimitConfig {
                burst: 10,
                refill_ms: 100,
                tokens_per_refill: 1,
                key: Default::default(),
            }),
            breaker: Some(roze_config::BreakerConfig {
                failure_threshold: 10,
                reset_timeout_ms: 1_000,
            }),
            fallback: Some(roze_config::GovernanceFallbackConfig {
                enabled: true,
                status: 503,
                body: Some(serde_json::json!({"message": "global"})),
                headers: Default::default(),
            }),
            ..Default::default()
        };
        governance.routes.insert(
            "get_user".to_string(),
            roze_config::RouteGovernanceConfig {
                timeout_ms: Some(50),
                rate_limit: Some(roze_config::RateLimitConfig {
                    burst: 1,
                    refill_ms: 1_000,
                    tokens_per_refill: 1,
                    key: Default::default(),
                }),
                breaker: Some(roze_config::BreakerConfig {
                    failure_threshold: 1,
                    reset_timeout_ms: 30_000,
                }),
                fallback: Some(roze_config::GovernanceFallbackConfig {
                    enabled: true,
                    status: 598,
                    body: Some(serde_json::json!({"message": "route"})),
                    headers: Default::default(),
                }),
                ..Default::default()
            },
        );

        let policy = route_policy(Some(&governance), "get_user");
        assert_eq!(policy.timeout, Some(Duration::from_millis(50)));
        assert_eq!(policy.rate_limit.expect("rate limit").burst, 1);
        assert_eq!(policy.breaker.expect("breaker").failure_threshold, 1);
        let fallback = policy.fallback.expect("fallback");
        assert_eq!(fallback.status, 598);
        assert_eq!(
            fallback.body.expect("fallback body")["message"],
            serde_json::json!("route")
        );
    }

    #[test]
    fn route_fallback_ignores_disabled_policy() {
        let governance = roze_config::GovernanceConfig {
            fallback: Some(roze_config::GovernanceFallbackConfig {
                enabled: false,
                status: 503,
                body: Some(serde_json::json!({"message": "off"})),
                headers: Default::default(),
            }),
            ..Default::default()
        };

        assert!(route_fallback(Some(&governance), "get_user").is_none());
    }

    #[test]
    fn apply_fallback_only_for_server_errors() {
        let fallback = roze_config::GovernanceFallbackConfig {
            enabled: true,
            status: 598,
            body: Some(serde_json::json!({"message": "degraded"})),
            headers: Default::default(),
        };

        assert_eq!(
            apply_fallback(
                "catalog",
                RozeError::BadRequest("bad".into()),
                Some(fallback.clone())
            ),
            RozeError::BadRequest("bad".into())
        );
        assert!(matches!(
            apply_fallback(
                "catalog",
                RozeError::Internal("boom".into()),
                Some(fallback)
            ),
            RozeError::Fallback { status: 598, .. }
        ));
    }

    #[test]
    fn permission_enforcement_requires_all_declared_permissions() {
        let context = Context::background().with_permissions(["users:read", "users:write"]);

        assert!(enforce_permissions(&context, &["users:read"]).is_ok());
        assert!(enforce_permissions(&context, &["users:read", "users:write"]).is_ok());
        assert!(matches!(
            enforce_permissions(&context, &["users:delete"]),
            Err(RozeError::Forbidden)
        ));
    }

    #[tokio::test]
    async fn route_rate_limit_uses_configured_store() {
        let mut governance = roze_config::GovernanceConfig::default();
        let route = format!("limited_{}", std::process::id());
        governance.routes.insert(
            route.clone(),
            roze_config::RouteGovernanceConfig {
                rate_limit: Some(roze_config::RateLimitConfig {
                    burst: 1,
                    refill_ms: 60_000,
                    tokens_per_refill: 1,
                    key: Default::default(),
                }),
                ..Default::default()
            },
        );

        let limiter =
            roze_rate_limit::RateLimiter::from_config(&Default::default()).expect("limiter");
        let headers = roze_http::http::HeaderMap::new();
        let context = Context::background();
        assert!(enforce_route_rate_limit(
            &limiter,
            "svc",
            &route,
            "GET",
            &context,
            None,
            &headers,
            Some(&governance),
        )
        .await
        .is_ok());
        let second = enforce_route_rate_limit(
            &limiter,
            "svc",
            &route,
            "GET",
            &context,
            None,
            &headers,
            Some(&governance),
        )
        .await;
        assert!(matches!(
            second,
            Err(RozeError::RateLimited {
                retry_after_seconds: _
            })
        ));
    }

    #[tokio::test]
    async fn route_rate_limit_isolates_verified_client_addresses() {
        let governance = roze_config::GovernanceConfig {
            rate_limit: Some(roze_config::RateLimitConfig {
                burst: 1,
                refill_ms: 60_000,
                tokens_per_refill: 1,
                key: roze_rate_limit::RateLimitKeyPolicy {
                    dimensions: vec![
                        roze_rate_limit::RateLimitDimension::Route,
                        roze_rate_limit::RateLimitDimension::ClientIp,
                    ],
                    ..Default::default()
                },
            }),
            ..Default::default()
        };
        let limiter =
            roze_rate_limit::RateLimiter::from_config(&Default::default()).expect("limiter");
        let headers = roze_http::http::HeaderMap::new();
        let context = Context::background();

        for address in ["203.0.113.10", "203.0.113.11"] {
            assert!(
                enforce_route_rate_limit(
                    &limiter,
                    "svc",
                    "login",
                    "POST",
                    &context,
                    Some(roze_http::client_ip::ClientIp(address.parse().unwrap())),
                    &headers,
                    Some(&governance),
                )
                .await
                .is_ok(),
                "{address} should receive an independent bucket"
            );
        }
    }

    #[test]
    fn finish_route_opens_breaker_after_server_failure() {
        let mut governance = roze_config::GovernanceConfig::default();
        let route = format!("breaker_{}", std::process::id());
        governance.routes.insert(
            route.clone(),
            roze_config::RouteGovernanceConfig {
                breaker: Some(roze_config::BreakerConfig {
                    failure_threshold: 1,
                    reset_timeout_ms: 60_000,
                }),
                ..Default::default()
            },
        );

        let (_ctx, guard) = begin_route(
            "svc".to_string(),
            route.clone(),
            "GET",
            Context::background(),
            Some(&governance),
        )
        .expect("first request should pass before breaker opens");
        finish_route(guard, false, "500");

        let next = begin_route(
            "svc".to_string(),
            route,
            "GET",
            Context::background(),
            Some(&governance),
        );
        assert!(matches!(next, Err(RozeError::Unavailable(message)) if message == "circuit open"));
    }

    #[test]
    fn route_breaker_serializes_half_open_probe_and_recovers_after_cancel() {
        let mut governance = roze_config::GovernanceConfig::default();
        let route = format!("half_open_{}", std::process::id());
        governance.routes.insert(
            route.clone(),
            roze_config::RouteGovernanceConfig {
                breaker: Some(roze_config::BreakerConfig {
                    failure_threshold: 1,
                    reset_timeout_ms: 1,
                }),
                ..Default::default()
            },
        );

        let (_ctx, failing) = begin_route(
            "svc".to_string(),
            route.clone(),
            "GET",
            Context::background(),
            Some(&governance),
        )
        .expect("closed breaker should allow request");
        finish_route(failing, false, "500");
        std::thread::sleep(Duration::from_millis(2));

        let (_ctx, cancelled_probe) = begin_route(
            "svc".to_string(),
            route.clone(),
            "GET",
            Context::background(),
            Some(&governance),
        )
        .expect("expired breaker should allow one probe");
        drop(cancelled_probe);
        let protected = begin_route(
            "svc".to_string(),
            route.clone(),
            "GET",
            Context::background(),
            Some(&governance),
        );
        assert!(
            matches!(protected, Err(RozeError::Unavailable(message)) if message == "circuit open")
        );

        std::thread::sleep(Duration::from_millis(2));
        let (_ctx, successful_probe) = begin_route(
            "svc".to_string(),
            route.clone(),
            "GET",
            Context::background(),
            Some(&governance),
        )
        .expect("cancelled probe should become retryable after reset timeout");
        let concurrent = begin_route(
            "svc".to_string(),
            route.clone(),
            "GET",
            Context::background(),
            Some(&governance),
        );
        assert!(
            matches!(concurrent, Err(RozeError::Unavailable(message)) if message == "circuit open")
        );
        finish_route(successful_probe, true, "200");

        let recovered = begin_route(
            "svc".to_string(),
            route,
            "GET",
            Context::background(),
            Some(&governance),
        );
        assert!(recovered.is_ok());
    }

    #[test]
    fn begin_route_sheds_when_concurrency_is_full() {
        let mut governance = roze_config::GovernanceConfig::default();
        let route = format!("shed_{}", std::process::id());
        governance.routes.insert(
            route.clone(),
            roze_config::RouteGovernanceConfig {
                shedding: Some(roze_config::SheddingConfig {
                    concurrency: 1,
                    window_ms: 1_000,
                    min_samples: 10,
                    max_avg_latency_ms: 1_000,
                    max_failure_ratio_per_mille: 500,
                    cool_down_ms: 60_000,
                }),
                ..Default::default()
            },
        );

        let first = begin_route(
            "svc".to_string(),
            route.clone(),
            "GET",
            Context::background(),
            Some(&governance),
        );
        assert!(first.is_ok());
        let second = begin_route(
            "svc".to_string(),
            route,
            "GET",
            Context::background(),
            Some(&governance),
        );
        assert!(matches!(second, Err(RozeError::Unavailable(message)) if message == "load shed"));
    }

    #[test]
    fn dropping_route_guard_releases_shedding_without_opening_breaker() {
        let mut governance = roze_config::GovernanceConfig::default();
        let route = format!("cancelled_{}", std::process::id());
        governance.routes.insert(
            route.clone(),
            roze_config::RouteGovernanceConfig {
                breaker: Some(roze_config::BreakerConfig {
                    failure_threshold: 1,
                    reset_timeout_ms: 60_000,
                }),
                shedding: Some(roze_config::SheddingConfig {
                    concurrency: 1,
                    window_ms: 1_000,
                    min_samples: 1,
                    max_avg_latency_ms: 1_000,
                    max_failure_ratio_per_mille: 0,
                    cool_down_ms: 60_000,
                }),
                ..Default::default()
            },
        );

        let (_ctx, guard) = begin_route(
            "svc".to_string(),
            route.clone(),
            "GET",
            Context::background(),
            Some(&governance),
        )
        .expect("first request should acquire shedding capacity");
        drop(guard);

        let next = begin_route(
            "svc".to_string(),
            route,
            "GET",
            Context::background(),
            Some(&governance),
        );
        assert!(next.is_ok());
    }
}
