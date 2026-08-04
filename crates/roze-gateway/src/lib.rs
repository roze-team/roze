use std::{
    collections::{BTreeMap, HashMap},
    convert::Infallible,
    future::Future,
    net::SocketAddr,
    pin::Pin,
    sync::{Arc, Mutex as StdMutex, OnceLock, Weak},
    task::{Context as TaskContext, Poll},
    time::{Duration, Instant},
};

use arc_swap::ArcSwap;
use base64::Engine as _;
use bytes::Bytes;
use futures_util::Stream;
use http::{header, HeaderMap, HeaderName, HeaderValue, Method, StatusCode};
use http_body_util::BodyExt;
use hyper_util::rt::TokioIo;
#[cfg(test)]
use roze_config::RouteGovernanceConfig;
use roze_config::{
    BreakerConfig, GatewayConfig, GatewayCorsConfig, GatewayFallbackResponse,
    GatewayHealthCheckConfig, GatewayOutlierConfig, GatewayRoute, GatewayService, GovernanceConfig,
    GovernanceFallbackConfig, RateLimitConfig, SheddingConfig,
};
use roze_context::Context;
use roze_http::rest::{self, HttpResponse, IncomingRequest};
use roze_jwt::{verify_token, JwtConfig};
use roze_resilience::{
    full_jitter_delay, BreakerDecision, BreakerPermit, BreakerRegistry, GovernanceBoundary,
    OperationKey, RetryBudgetRegistry, SheddingRegistry,
};
use roze_rpc::registry::{Registry, ServiceInstance};
use rustls_pki_types::pem::PemObject;
use sha1::{Digest as _, Sha1};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::TcpStream,
    sync::Mutex,
};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone)]
pub struct GatewayServiceRuntime {
    current: Arc<ArcSwap<GatewayRuntime>>,
    reload_lock: Arc<StdMutex<()>>,
    runtime_handle: Option<tokio::runtime::Handle>,
    #[cfg(test)]
    runtime: Arc<GatewayRuntime>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GatewayReloadOutcome {
    Applied,
    Skipped,
    Failed,
}

pub fn record_reload_outcome(outcome: GatewayReloadOutcome) {
    roze_metrics::record_gateway_config_reload(match outcome {
        GatewayReloadOutcome::Applied => "applied",
        GatewayReloadOutcome::Skipped => "skipped",
        GatewayReloadOutcome::Failed => "failed",
    });
}

struct GatewayRuntime {
    routes: Vec<CompiledRoute>,
    services: HashMap<String, GatewayService>,
    client: reqwest::Client,
    global_timeout: Option<Duration>,
    global_stream_idle_timeout: Option<Duration>,
    global_max_stream_connections: Option<u32>,
    global_fallback: Option<GatewayFallbackResponse>,
    global_middlewares: Vec<String>,
    jwt: Option<JwtConfig>,
    api_keys: Option<roze_auth::ApiKeyConfig>,
    request_body_limit_bytes: usize,
    rate_limiter: Arc<roze_rate_limit::RateLimiter>,
    rate_limiter_config: roze_rate_limit::RateLimiterConfig,
    trusted_proxies: roze_http::client_ip::TrustedProxyConfig,
    breakers: Arc<BreakerRegistry>,
    shedders: Arc<SheddingRegistry>,
    retry_budgets: Arc<RetryBudgetRegistry>,
    registry: Option<Arc<dyn Registry>>,
    registry_cursors: Arc<StdMutex<HashMap<String, u64>>>,
    outlier_states: Arc<Mutex<HashMap<String, OutlierState>>>,
    health_states: Arc<Mutex<HashMap<String, HealthState>>>,
    health_tasks: StdMutex<Vec<tokio::task::JoinHandle<()>>>,
    cors: Option<CorsPolicy>,
    stream_connection_states: Arc<StdMutex<HashMap<String, u32>>>,
    websocket_tls_config: Arc<OnceLock<Arc<rustls::ClientConfig>>>,
    websocket_tls_configs: HashMap<String, Arc<rustls::ClientConfig>>,
}

#[derive(Debug, Clone)]
struct CompiledRoute {
    path: String,
    service: String,
    methods: Vec<Method>,
    match_headers: BTreeMap<String, String>,
    match_cookies: BTreeMap<String, String>,
    traffic_percent: u32,
    mirror_service: Option<String>,
    mirror_percent: u32,
    timeout: Option<Duration>,
    retries: usize,
    retry_backoff: Duration,
    retry_max_backoff: Duration,
    retry_budget_percent: Option<u32>,
    rewrite: Option<String>,
    fallback: Option<GatewayFallbackResponse>,
    rate_limit: Option<RateLimitConfig>,
    breaker: Option<BreakerConfig>,
    shedding: Option<SheddingConfig>,
    middlewares: Vec<String>,
    instance_tags: BTreeMap<String, String>,
    stream_idle_timeout: Option<Duration>,
    max_stream_connections: Option<u32>,
}

#[derive(Debug, Clone)]
struct UpstreamTarget {
    base: String,
    instance_key: String,
    outlier: Option<GatewayOutlierConfig>,
}

#[derive(Debug, Default)]
struct OutlierState {
    failures: u32,
    ejected_until: Option<Instant>,
}

#[derive(Debug)]
struct HealthState {
    healthy: bool,
    failures: u32,
    successes: u32,
}

#[derive(Debug)]
struct CorsPolicy {
    allow_any_origin: bool,
    allow_origins: Vec<HeaderValue>,
    allow_methods: Vec<Method>,
    allow_any_header: bool,
    allow_headers: Vec<HeaderName>,
    max_age_seconds: Option<u64>,
}

struct StreamConnectionPermit {
    states: Arc<StdMutex<HashMap<String, u32>>>,
    key: String,
    service: String,
    route: String,
    protocol: &'static str,
    started: Instant,
}

struct SseBody {
    state: StdMutex<SseBodyState>,
}

struct SseBodyState {
    stream: Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>>,
    idle_timeout: Duration,
    idle_sleep: Option<Pin<Box<tokio::time::Sleep>>>,
    permit: Option<StreamConnectionPermit>,
    done: bool,
}

struct WebSocketTarget {
    addr: String,
    authority: String,
    path_and_query: String,
    server_name: String,
    secure: bool,
}

struct WebSocketHandshakeResponse {
    status: StatusCode,
    headers: Vec<(HeaderName, HeaderValue)>,
    remaining: Vec<u8>,
}

trait WebSocketIo: AsyncRead + AsyncWrite + Unpin + Send {}

impl<T> WebSocketIo for T where T: AsyncRead + AsyncWrite + Unpin + Send {}

impl Default for HealthState {
    fn default() -> Self {
        Self {
            healthy: true,
            failures: 0,
            successes: 0,
        }
    }
}

impl CorsPolicy {
    fn new(config: GatewayCorsConfig) -> Self {
        Self {
            allow_any_origin: config.allow_origins.iter().any(|origin| origin == "*"),
            allow_origins: config
                .allow_origins
                .into_iter()
                .filter(|origin| origin != "*")
                .filter_map(|origin| origin.parse().ok())
                .collect(),
            allow_methods: config
                .allow_methods
                .into_iter()
                .filter_map(|method| method.parse().ok())
                .collect(),
            allow_any_header: config.allow_headers.iter().any(|name| name == "*"),
            allow_headers: config
                .allow_headers
                .into_iter()
                .filter(|name| name != "*")
                .filter_map(|name| name.parse().ok())
                .collect(),
            max_age_seconds: config.max_age_seconds,
        }
    }

    fn allowed_origin(&self, headers: &HeaderMap) -> Option<HeaderValue> {
        let origin = headers.get(header::ORIGIN)?;
        if self.allow_any_origin {
            Some(HeaderValue::from_static("*"))
        } else if self.allow_origins.iter().any(|allowed| allowed == origin) {
            Some(origin.clone())
        } else {
            None
        }
    }

    fn preflight(&self, request: &IncomingRequest) -> Option<HttpResponse> {
        if request.method() != Method::OPTIONS {
            return None;
        }
        let requested_method = request
            .headers()
            .get(header::ACCESS_CONTROL_REQUEST_METHOD)?;
        let allowed_origin = match self.allowed_origin(request.headers()) {
            Some(origin) => origin,
            None => return Some(cors_rejected_response("CORS origin is not allowed")),
        };
        match requested_method
            .to_str()
            .ok()
            .and_then(|value| value.parse().ok())
        {
            Some(method) if self.allow_methods.contains(&method) => {}
            _ => return Some(cors_rejected_response("CORS method is not allowed")),
        }
        let requested_headers = match parse_requested_cors_headers(request.headers()) {
            Ok(headers) => headers,
            Err(()) => return Some(cors_rejected_response("invalid CORS request headers")),
        };
        if !self.allow_any_header
            && requested_headers
                .iter()
                .any(|requested| !self.allow_headers.contains(requested))
        {
            return Some(cors_rejected_response("CORS request header is not allowed"));
        }

        let mut response = http::Response::builder()
            .status(StatusCode::NO_CONTENT)
            .body(rest::full_body(Bytes::new()))
            .expect("valid CORS preflight response");
        response
            .headers_mut()
            .insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, allowed_origin);
        response.headers_mut().insert(
            header::ACCESS_CONTROL_ALLOW_METHODS,
            HeaderValue::from_str(
                &self
                    .allow_methods
                    .iter()
                    .map(Method::as_str)
                    .collect::<Vec<_>>()
                    .join(", "),
            )
            .expect("valid CORS allow methods"),
        );
        let allowed_headers = if self.allow_any_header {
            requested_headers
                .iter()
                .map(HeaderName::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        } else {
            self.allow_headers
                .iter()
                .map(HeaderName::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        };
        if let Ok(value) = HeaderValue::from_str(&allowed_headers) {
            if !allowed_headers.is_empty() {
                response
                    .headers_mut()
                    .insert(header::ACCESS_CONTROL_ALLOW_HEADERS, value);
            }
        }
        if let Some(max_age) = self.max_age_seconds {
            response.headers_mut().insert(
                header::ACCESS_CONTROL_MAX_AGE,
                HeaderValue::from_str(&max_age.to_string()).expect("valid CORS max age"),
            );
        }
        append_vary(
            response.headers_mut(),
            "Origin, Access-Control-Request-Method, Access-Control-Request-Headers",
        );
        Some(response)
    }

    fn apply_simple_response(&self, headers: &mut HeaderMap, origin: HeaderValue) {
        headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin);
        append_vary(headers, "Origin");
    }
}

impl Drop for StreamConnectionPermit {
    fn drop(&mut self) {
        let mut active = 0;
        if let Ok(mut states) = self.states.lock() {
            if let Some(current) = states.get_mut(&self.key) {
                *current = current.saturating_sub(1);
                active = *current;
                if *current == 0 {
                    states.remove(&self.key);
                }
            }
        }
        roze_metrics::record_gateway_stream_connection(
            self.service.clone(),
            self.route.clone(),
            self.protocol,
            "closed",
            active,
        );
        roze_metrics::record_gateway_stream_connection_duration(
            self.service.clone(),
            self.route.clone(),
            self.protocol,
            self.started.elapsed(),
        );
    }
}

impl SseBody {
    fn new(
        stream: impl Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static,
        idle_timeout: Option<Duration>,
        permit: Option<StreamConnectionPermit>,
    ) -> Self {
        let idle_timeout = idle_timeout.unwrap_or(Duration::ZERO);
        let idle_sleep =
            (!idle_timeout.is_zero()).then(|| Box::pin(tokio::time::sleep(idle_timeout)));
        Self {
            state: StdMutex::new(SseBodyState {
                stream: Box::pin(stream),
                idle_timeout,
                idle_sleep,
                permit,
                done: false,
            }),
        }
    }
}

impl hyper::body::Body for SseBody {
    type Data = Bytes;
    type Error = rest::BoxError;

    fn poll_frame(
        self: Pin<&mut Self>,
        context: &mut TaskContext<'_>,
    ) -> Poll<Option<Result<hyper::body::Frame<Self::Data>, Self::Error>>> {
        let mut state = match self.state.lock() {
            Ok(state) => state,
            Err(_) => {
                return Poll::Ready(Some(Err(rest::BoxError::new(std::io::Error::other(
                    "SSE body state lock is poisoned",
                )))))
            }
        };
        if state.done {
            return Poll::Ready(None);
        }
        match state.stream.as_mut().poll_next(context) {
            Poll::Ready(Some(Ok(bytes))) => {
                let idle_timeout = state.idle_timeout;
                if let Some(sleep) = state.idle_sleep.as_mut() {
                    sleep
                        .as_mut()
                        .reset(tokio::time::Instant::now() + idle_timeout);
                }
                return Poll::Ready(Some(Ok(hyper::body::Frame::data(bytes))));
            }
            Poll::Ready(Some(Err(error))) => {
                state.done = true;
                state.permit.take();
                return Poll::Ready(Some(Err(rest::BoxError::new(error))));
            }
            Poll::Ready(None) => {
                state.done = true;
                state.permit.take();
                return Poll::Ready(None);
            }
            Poll::Pending => {}
        }
        if let Some(sleep) = state.idle_sleep.as_mut() {
            if sleep.as_mut().poll(context).is_ready() {
                state.done = true;
                state.permit.take();
                return Poll::Ready(Some(Err(rest::BoxError::new(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "SSE stream idle timeout",
                )))));
            }
        }
        Poll::Pending
    }

    fn is_end_stream(&self) -> bool {
        self.state.lock().map(|state| state.done).unwrap_or(true)
    }
}

fn parse_requested_cors_headers(headers: &HeaderMap) -> Result<Vec<HeaderName>, ()> {
    let Some(value) = headers.get(header::ACCESS_CONTROL_REQUEST_HEADERS) else {
        return Ok(Vec::new());
    };
    let value = value.to_str().map_err(|_| ())?;
    value
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(|name| name.parse().map_err(|_| ()))
        .collect()
}

fn cors_rejected_response(message: &str) -> HttpResponse {
    let mut response = rest::text_response(StatusCode::FORBIDDEN, message);
    append_vary(
        response.headers_mut(),
        "Origin, Access-Control-Request-Method, Access-Control-Request-Headers",
    );
    response
}

fn append_vary(headers: &mut HeaderMap, value: &str) {
    let values = headers
        .get(header::VARY)
        .and_then(|current| current.to_str().ok())
        .into_iter()
        .flat_map(|current| current.split(','))
        .chain(value.split(','))
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .fold(Vec::<String>::new(), |mut values, item| {
            if !values.iter().any(|value| value.eq_ignore_ascii_case(item)) {
                values.push(item.to_string());
            }
            values
        });
    let merged = values.join(", ");
    if let Ok(value) = HeaderValue::from_str(&merged) {
        headers.insert(header::VARY, value);
    }
}

#[derive(Debug, Clone, Copy)]
enum AuthPolicy {
    Jwt,
    ApiKey,
    Any,
}

struct GatewaySheddingGuard<'a> {
    registry: &'a SheddingRegistry,
    key: String,
    config: roze_resilience::SheddingConfig,
    started: Instant,
    finished: bool,
}

impl GatewaySheddingGuard<'_> {
    fn finish(mut self, success: bool) {
        self.registry
            .record(&self.key, success, self.started.elapsed(), self.config);
        self.finished = true;
    }
}

impl Drop for GatewaySheddingGuard<'_> {
    fn drop(&mut self) {
        if !self.finished {
            self.registry.release(&self.key);
        }
    }
}

impl tower::Service<IncomingRequest> for GatewayServiceRuntime {
    type Response = HttpResponse;
    type Error = Infallible;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(
        &mut self,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        std::task::Poll::Ready(Ok(()))
    }

    fn call(&mut self, request: IncomingRequest) -> Self::Future {
        let runtime = self.current.load_full();
        Box::pin(async move { Ok(runtime.handle(request).await) })
    }
}

pub fn build_router(config: GatewayConfig, jwt: Option<JwtConfig>) -> GatewayServiceRuntime {
    build_router_with_registry(config, jwt, None)
}

pub fn build_router_with_registry(
    config: GatewayConfig,
    jwt: Option<JwtConfig>,
    registry: Option<Arc<dyn Registry>>,
) -> GatewayServiceRuntime {
    build_router_with_registry_and_governance(config, jwt, registry, None)
}

impl GatewayServiceRuntime {
    pub fn reload(
        &self,
        config: GatewayConfig,
        jwt: Option<JwtConfig>,
        api_keys: Option<roze_auth::ApiKeyConfig>,
        registry: Option<Arc<dyn Registry>>,
        governance: Option<GovernanceConfig>,
    ) -> anyhow::Result<()> {
        validate_gateway_config(&config)?;
        let _reload = self
            .reload_lock
            .lock()
            .map_err(|_| anyhow::anyhow!("gateway reload lock is poisoned"))?;
        let current = self.current.load_full();
        let _runtime_context = self.runtime_handle.as_ref().map(|handle| handle.enter());
        let next =
            build_gateway_runtime(config, jwt, api_keys, registry, governance, Some(&current))?;
        current.stop_health_checks();
        self.current.store(next);
        Ok(())
    }
}

pub fn build_router_with_registry_and_governance(
    config: GatewayConfig,
    jwt: Option<JwtConfig>,
    registry: Option<Arc<dyn Registry>>,
    governance: Option<GovernanceConfig>,
) -> GatewayServiceRuntime {
    build_router_with_registry_governance_and_auth(config, jwt, None, registry, governance)
}

pub fn build_router_with_registry_governance_and_auth(
    config: GatewayConfig,
    jwt: Option<JwtConfig>,
    api_keys: Option<roze_auth::ApiKeyConfig>,
    registry: Option<Arc<dyn Registry>>,
    governance: Option<GovernanceConfig>,
) -> GatewayServiceRuntime {
    try_build_router_with_registry_governance_and_auth(config, jwt, api_keys, registry, governance)
        .expect("invalid gateway runtime configuration")
}

pub fn try_build_router_with_registry_governance_and_auth(
    config: GatewayConfig,
    jwt: Option<JwtConfig>,
    api_keys: Option<roze_auth::ApiKeyConfig>,
    registry: Option<Arc<dyn Registry>>,
    governance: Option<GovernanceConfig>,
) -> anyhow::Result<GatewayServiceRuntime> {
    validate_gateway_config(&config)?;
    let runtime = build_gateway_runtime(config, jwt, api_keys, registry, governance, None)?;
    Ok(GatewayServiceRuntime {
        current: Arc::new(ArcSwap::from(runtime.clone())),
        reload_lock: Arc::new(StdMutex::new(())),
        runtime_handle: tokio::runtime::Handle::try_current().ok(),
        #[cfg(test)]
        runtime,
    })
}

fn build_gateway_runtime(
    config: GatewayConfig,
    jwt: Option<JwtConfig>,
    api_keys: Option<roze_auth::ApiKeyConfig>,
    registry: Option<Arc<dyn Registry>>,
    governance: Option<GovernanceConfig>,
    previous: Option<&GatewayRuntime>,
) -> anyhow::Result<Arc<GatewayRuntime>> {
    let websocket_tls_configs = build_service_websocket_tls_configs(&config.services)?;
    let cors = config.cors.map(CorsPolicy::new);
    let global_stream_idle_timeout = config.stream_idle_timeout_ms.map(Duration::from_millis);
    let global_max_stream_connections = config.max_stream_connections;
    let global_timeout = config.timeout_ms.map(Duration::from_millis).or_else(|| {
        governance
            .as_ref()
            .and_then(|governance| governance.timeout_ms)
            .map(Duration::from_millis)
    });
    let global_fallback = governance
        .as_ref()
        .and_then(governance_fallback)
        .or(config.fallback);
    let global_middlewares = normalize_middlewares(config.middlewares);
    let trusted_proxies =
        roze_http::client_ip::TrustedProxyConfig::new(&config.trusted_proxy_cidrs)?;
    let rate_limiter_config = governance
        .as_ref()
        .map(|value| value.rate_limiter.clone())
        .unwrap_or_default();
    let rate_limiter = match previous {
        Some(runtime) if runtime.rate_limiter_config == rate_limiter_config => {
            runtime.rate_limiter.clone()
        }
        _ => Arc::new(roze_rate_limit::RateLimiter::from_config(
            &rate_limiter_config,
        )?),
    };
    let services = config
        .services
        .into_iter()
        .map(|service| (service.name.clone(), service))
        .collect();
    let mut routes = compile_routes(config.routes, governance.as_ref());
    routes.sort_by_key(|route| std::cmp::Reverse(route.path.len()));

    let runtime = Arc::new(GatewayRuntime {
        routes,
        services,
        client: previous
            .map(|runtime| runtime.client.clone())
            .unwrap_or_default(),
        global_timeout,
        global_stream_idle_timeout,
        global_max_stream_connections,
        global_fallback,
        global_middlewares,
        jwt,
        api_keys,
        request_body_limit_bytes: config.request_body_limit_bytes.unwrap_or(2 * 1024 * 1024),
        rate_limiter,
        rate_limiter_config,
        trusted_proxies,
        breakers: previous
            .map(|runtime| runtime.breakers.clone())
            .unwrap_or_else(|| Arc::new(BreakerRegistry::new())),
        shedders: previous
            .map(|runtime| runtime.shedders.clone())
            .unwrap_or_else(|| Arc::new(SheddingRegistry::new())),
        retry_budgets: previous
            .map(|runtime| runtime.retry_budgets.clone())
            .unwrap_or_else(|| Arc::new(RetryBudgetRegistry::default())),
        registry,
        registry_cursors: previous
            .map(|runtime| runtime.registry_cursors.clone())
            .unwrap_or_else(|| Arc::new(StdMutex::new(HashMap::new()))),
        outlier_states: previous
            .map(|runtime| runtime.outlier_states.clone())
            .unwrap_or_else(|| Arc::new(Mutex::new(HashMap::new()))),
        health_states: previous
            .map(|runtime| runtime.health_states.clone())
            .unwrap_or_else(|| Arc::new(Mutex::new(HashMap::new()))),
        health_tasks: StdMutex::new(Vec::new()),
        cors,
        stream_connection_states: previous
            .map(|runtime| runtime.stream_connection_states.clone())
            .unwrap_or_else(|| Arc::new(StdMutex::new(HashMap::new()))),
        websocket_tls_config: previous
            .map(|runtime| runtime.websocket_tls_config.clone())
            .unwrap_or_else(|| Arc::new(OnceLock::new())),
        websocket_tls_configs,
    });
    runtime.spawn_health_checks();
    Ok(runtime)
}

impl Drop for GatewayRuntime {
    fn drop(&mut self) {
        self.stop_health_checks();
    }
}

impl GatewayRuntime {
    fn stop_health_checks(&self) {
        for task in self
            .health_tasks
            .lock()
            .expect("gateway health task lock")
            .drain(..)
        {
            task.abort();
        }
    }

    async fn handle(&self, request: IncomingRequest) -> HttpResponse {
        if let Some(cors) = self.cors.as_ref() {
            let cors_started = Instant::now();
            if let Some(response) = cors.preflight(&request) {
                roze_metrics::record_gateway_route(
                    "gateway",
                    "",
                    Method::OPTIONS.as_str(),
                    response.status().as_u16().to_string(),
                    if response.status().is_success() {
                        "cors_preflight"
                    } else {
                        "cors_rejected"
                    },
                    cors_started.elapsed(),
                );
                roze_metrics::record_http_request(
                    response.status().is_success(),
                    cors_started.elapsed(),
                );
                return response;
            }
            let allowed_origin = cors.allowed_origin(request.headers());
            let mut response = self.handle_inner(request).await;
            if let Some(origin) = allowed_origin {
                cors.apply_simple_response(response.headers_mut(), origin);
            }
            return response;
        }
        self.handle_inner(request).await
    }

    async fn handle_inner(&self, mut request: IncomingRequest) -> HttpResponse {
        let started = Instant::now();
        let method = request.method().clone();
        let path = request.uri().path().to_string();
        let Some(route) = self.select_route(&request) else {
            return self.finish_response(
                None,
                &method,
                "no_route",
                started,
                fallback_response(
                    self.global_fallback.as_ref(),
                    StatusCode::NOT_FOUND,
                    "gateway route not found",
                ),
            );
        };
        if !route.method_allowed(&method) {
            return self.finish_response(
                Some(route),
                &method,
                "method_not_allowed",
                started,
                fallback_response(
                    route.fallback.as_ref().or(self.global_fallback.as_ref()),
                    StatusCode::METHOD_NOT_ALLOWED,
                    "method not allowed",
                ),
            );
        }
        let Some(service) = self.services.get(&route.service) else {
            return self.finish_response(
                Some(route),
                &method,
                "service_not_found",
                started,
                fallback_response(
                    route.fallback.as_ref().or(self.global_fallback.as_ref()),
                    StatusCode::BAD_GATEWAY,
                    "gateway service not found",
                ),
            );
        };

        ensure_correlation_header(&mut request, roze_context::REQUEST_ID_HEADER);
        ensure_correlation_header(&mut request, roze_context::TRACE_ID_HEADER);

        // Identity headers are trusted only after this gateway authenticates
        // the request. Remove client-supplied values before building context
        // or forwarding the request downstream.
        clear_untrusted_auth_context_headers(request.headers_mut());
        if let Some(policy) = self.auth_policy(route) {
            let Some(principal) = validate_request_auth(
                request.headers(),
                policy,
                self.jwt.as_ref(),
                self.api_keys.as_ref(),
            ) else {
                return self.finish_response(
                    Some(route),
                    &method,
                    "unauthorized",
                    started,
                    fallback_response(
                        route.fallback.as_ref().or(self.global_fallback.as_ref()),
                        StatusCode::UNAUTHORIZED,
                        "unauthorized",
                    ),
                );
            };
            inject_auth_context_headers(request.headers_mut(), &principal);
        }

        let propagation = request
            .headers()
            .iter()
            .filter_map(|(name, value)| {
                Some((name.as_str().to_string(), value.to_str().ok()?.to_string()))
            })
            .collect::<BTreeMap<_, _>>();
        let context = gateway_context(
            Context::from_propagation_headers(&propagation),
            route
                .timeout
                .or_else(|| service.timeout_ms.map(Duration::from_millis))
                .or(self.global_timeout)
                .unwrap_or(DEFAULT_TIMEOUT),
        );
        let key = OperationKey::new(
            &route.service,
            GovernanceBoundary::Gateway,
            format!("{}:{}", method, route.path),
        )
        .to_string();
        let retry_key = key.clone();

        if let Some(config) = route.rate_limit.as_ref() {
            let client_ip = request
                .extensions()
                .get::<roze_http::extract::ConnectInfo<SocketAddr>>()
                .map(|peer| self.trusted_proxies.resolve(peer.0, request.headers()));
            let identity = roze_rate_limit::RateLimitIdentity::new(
                "gateway",
                "gateway",
                format!("{}:{}", method, route.path),
            )
            .with_client_ip(client_ip.map(|value| value.0))
            .with_subject(context.subject())
            .with_tenant(context.tenant())
            .with_headers(request.headers().iter().filter_map(|(name, value)| {
                Some((name.as_str(), value.to_str().ok()?.to_string()))
            }));
            let decision = self
                .rate_limiter
                .check(
                    &config.key,
                    &identity,
                    roze_rate_limit::RateLimit {
                        burst: config.burst,
                        refill: Duration::from_millis(config.refill_ms.max(1)),
                        tokens_per_refill: config.tokens_per_refill,
                    },
                )
                .await;
            let allowed = decision.as_ref().is_ok_and(|decision| decision.allowed);
            let metric = match &decision {
                Ok(decision) if decision.allowed && decision.degraded => "store_error_fail_open",
                Ok(decision) if decision.allowed => "allowed",
                Ok(_) => "rejected",
                Err(roze_rate_limit::RateLimitError::StoreUnavailable) => "store_error_fail_closed",
                Err(_) => "identity_rejected",
            };
            roze_metrics::record_resilience_decision("gateway", "gateway", "rate_limit", metric);
            if !allowed {
                let (status, retry_after) = match decision {
                    Ok(decision) => (StatusCode::TOO_MANY_REQUESTS, Some(decision.retry_after)),
                    Err(roze_rate_limit::RateLimitError::StoreUnavailable) => {
                        (StatusCode::SERVICE_UNAVAILABLE, None)
                    }
                    Err(_) => (StatusCode::TOO_MANY_REQUESTS, Some(Duration::from_secs(1))),
                };
                let mut response = if status == StatusCode::TOO_MANY_REQUESTS {
                    rest::text_response(status, "too many requests")
                } else {
                    fallback_response(
                        route.fallback.as_ref().or(self.global_fallback.as_ref()),
                        status,
                        "rate limit store unavailable",
                    )
                };
                if let Some(retry_after) = retry_after {
                    let retry_after_seconds =
                        retry_after.as_secs() + u64::from(retry_after.subsec_nanos() > 0);
                    if let Ok(value) = HeaderValue::try_from(retry_after_seconds.max(1).to_string())
                    {
                        response.headers_mut().insert(header::RETRY_AFTER, value);
                    }
                }
                return self.finish_response(
                    Some(route),
                    &method,
                    if status == StatusCode::TOO_MANY_REQUESTS {
                        "rate_limited"
                    } else {
                        "rate_limit_unavailable"
                    },
                    started,
                    response,
                );
            }
        }

        let breaker_permit = match route.breaker {
            Some(_) => match self.breakers.allow(key.clone()) {
                BreakerDecision::Allow(permit) => {
                    roze_metrics::record_resilience_decision(
                        "gateway",
                        "gateway",
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
                        "gateway", "gateway", "breaker", "open",
                    );
                    return self.finish_response(
                        Some(route),
                        &method,
                        "breaker_open",
                        started,
                        fallback_response(
                            route.fallback.as_ref().or(self.global_fallback.as_ref()),
                            StatusCode::SERVICE_UNAVAILABLE,
                            "service temporarily unavailable",
                        ),
                    );
                }
            },
            None => None,
        };

        let shedding_guard = match route.shedding {
            Some(config) => {
                let config = shedding_config(config);
                if !self.shedders.allow(key.clone(), config) {
                    cancel_breaker(&self.breakers, &key, breaker_permit, route.breaker);
                    roze_metrics::record_resilience_decision(
                        "gateway",
                        "gateway",
                        "load_shedding",
                        "shed",
                    );
                    return self.finish_response(
                        Some(route),
                        &method,
                        "load_shed",
                        started,
                        fallback_response(
                            route.fallback.as_ref().or(self.global_fallback.as_ref()),
                            StatusCode::TOO_MANY_REQUESTS,
                            "gateway load shed",
                        ),
                    );
                }
                roze_metrics::record_resilience_decision(
                    "gateway",
                    "gateway",
                    "load_shedding",
                    "allowed",
                );
                Some(GatewaySheddingGuard {
                    registry: &self.shedders,
                    key: key.clone(),
                    config,
                    started: Instant::now(),
                    finished: false,
                })
            }
            None => None,
        };

        if is_websocket_upgrade(request.headers()) {
            let result = self
                .proxy_websocket(&mut request, service, route, &path, &context)
                .await;
            let success = matches!(&result, Ok(response) if response.status() == StatusCode::SWITCHING_PROTOCOLS)
                || matches!(&result, Err(UpstreamError::StreamCapacity));
            finish_breaker(&self.breakers, &key, breaker_permit, route.breaker, success);
            if let Some(guard) = shedding_guard {
                guard.finish(success);
            }
            let (outcome, response) = match result {
                Ok(response) => ("websocket_upgraded", response),
                Err(UpstreamError::StreamCapacity) => (
                    "stream_capacity",
                    fallback_response(
                        route.fallback.as_ref().or(self.global_fallback.as_ref()),
                        StatusCode::TOO_MANY_REQUESTS,
                        "too many active gateway streams",
                    ),
                ),
                Err(UpstreamError::Timeout) => (
                    "websocket_timeout",
                    fallback_response(
                        route.fallback.as_ref().or(self.global_fallback.as_ref()),
                        StatusCode::GATEWAY_TIMEOUT,
                        "WebSocket upstream handshake timeout",
                    ),
                ),
                Err(error) => (
                    "websocket_failed",
                    fallback_response(
                        route.fallback.as_ref().or(self.global_fallback.as_ref()),
                        StatusCode::BAD_GATEWAY,
                        &error.to_string(),
                    ),
                ),
            };
            return self.finish_response(Some(route), &method, outcome, started, response);
        }

        let headers = request
            .headers()
            .iter()
            .filter(|(name, _)| !is_hop_by_hop_header(name.as_str()))
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect::<Vec<_>>();
        let query = request.uri().query().map(str::to_string);
        let body = match request.into_body().collect().await {
            Ok(collected) => {
                let body = collected.to_bytes();
                if body.len() > self.request_body_limit_bytes {
                    cancel_breaker(&self.breakers, &key, breaker_permit, route.breaker);
                    return self.finish_response(
                        Some(route),
                        &method,
                        "body_too_large",
                        started,
                        rest::text_response(
                            StatusCode::PAYLOAD_TOO_LARGE,
                            "request body too large",
                        ),
                    );
                }
                body
            }
            Err(error) => {
                cancel_breaker(&self.breakers, &key, breaker_permit, route.breaker);
                return self.finish_response(
                    Some(route),
                    &method,
                    "bad_request_body",
                    started,
                    rest::text_response(
                        StatusCode::BAD_REQUEST,
                        format!("failed to read request body: {error}"),
                    ),
                );
            }
        };

        self.dispatch_mirror(route, &method, &path, query.as_deref(), &headers, &body);

        let retryable_method = is_idempotent_method(&method);
        let max_retries = if retryable_method { route.retries } else { 0 };
        self.retry_budgets.record_call(&retry_key);
        let mut retries = 0usize;
        loop {
            let result = self
                .send_upstream(UpstreamRequest {
                    service,
                    route,
                    method: &method,
                    incoming_path: &path,
                    query: query.as_deref(),
                    headers: &headers,
                    body: body.clone(),
                    context: &context,
                })
                .await;
            let retry_reason = match &result {
                Ok(response) if retryable_gateway_status(response.status()) => {
                    Some(format!("status_{}", response.status().as_u16()))
                }
                Err(UpstreamError::Timeout) => Some("timeout".to_string()),
                Err(UpstreamError::Request(_)) => Some("upstream_error".to_string()),
                Err(UpstreamError::Unavailable(_)) => Some("unavailable".to_string()),
                _ => None,
            };
            let can_retry = retry_reason.is_some() && retries < max_retries;
            if can_retry {
                if !self
                    .retry_budgets
                    .allow_retry(&retry_key, route.retry_budget_percent)
                {
                    roze_metrics::record_resilience_decision(
                        "gateway",
                        "gateway",
                        "retry",
                        "budget_exhausted",
                    );
                } else {
                    let next_retry = retries + 1;
                    let delay =
                        full_jitter_delay(route.retry_backoff, route.retry_max_backoff, next_retry);
                    if retry_context_exhausted(&context, delay) {
                        roze_metrics::record_resilience_decision(
                            "gateway",
                            "gateway",
                            "retry",
                            "deadline_exhausted",
                        );
                    } else {
                        if !delay.is_zero() {
                            tokio::time::sleep(delay).await;
                        }
                        if !retry_context_exhausted(&context, Duration::ZERO) {
                            retries = next_retry;
                            roze_metrics::record_gateway_retry(
                                route.service.clone(),
                                route.path.clone(),
                                retry_reason.expect("retry reason checked"),
                            );
                            roze_metrics::record_resilience_decision(
                                "gateway", "gateway", "retry", "attempt",
                            );
                            continue;
                        }
                    }
                }
            }

            let success = matches!(&result, Ok(response) if !response.status().is_server_error())
                || matches!(&result, Err(UpstreamError::StreamCapacity));
            finish_breaker(&self.breakers, &key, breaker_permit, route.breaker, success);
            if let Some(guard) = shedding_guard {
                guard.finish(success);
            }
            let (outcome, response) = match result {
                Ok(response) if response.status().is_server_error() => (
                    "upstream_server_error",
                    route
                        .fallback
                        .as_ref()
                        .or(self.global_fallback.as_ref())
                        .map(|fallback| {
                            fallback_response(
                                Some(fallback),
                                StatusCode::BAD_GATEWAY,
                                "upstream server error",
                            )
                        })
                        .unwrap_or(response),
                ),
                Ok(response) => ("ok", response),
                Err(UpstreamError::Timeout) => (
                    "timeout",
                    fallback_response(
                        route.fallback.as_ref().or(self.global_fallback.as_ref()),
                        StatusCode::GATEWAY_TIMEOUT,
                        "upstream timeout",
                    ),
                ),
                Err(UpstreamError::Request(error)) => (
                    "upstream_failed",
                    fallback_response(
                        route.fallback.as_ref().or(self.global_fallback.as_ref()),
                        StatusCode::BAD_GATEWAY,
                        &error.to_string(),
                    ),
                ),
                Err(UpstreamError::Unavailable(error) | UpstreamError::Protocol(error)) => (
                    "upstream_unavailable",
                    fallback_response(
                        route.fallback.as_ref().or(self.global_fallback.as_ref()),
                        StatusCode::BAD_GATEWAY,
                        &error,
                    ),
                ),
                Err(UpstreamError::StreamCapacity) => (
                    "stream_capacity",
                    fallback_response(
                        route.fallback.as_ref().or(self.global_fallback.as_ref()),
                        StatusCode::TOO_MANY_REQUESTS,
                        "too many active gateway streams",
                    ),
                ),
            };
            return self.finish_response(Some(route), &method, outcome, started, response);
        }
    }

    async fn send_upstream(
        &self,
        request: UpstreamRequest<'_>,
    ) -> Result<HttpResponse, UpstreamError> {
        let UpstreamRequest {
            service,
            route,
            method,
            incoming_path,
            query,
            headers,
            body,
            context,
        } = request;
        let target = self.resolve_upstream(service, &route.instance_tags).await?;
        let path = rewrite_path(&route.path, route.rewrite.as_deref(), incoming_path);
        let mut upstream = join_url(&target.base, &path);
        if let Some(query) = query.filter(|query| !query.is_empty()) {
            upstream.push('?');
            upstream.push_str(query);
        }
        let timeout = context.remaining_timeout().unwrap_or(Duration::ZERO);
        if timeout.is_zero() {
            return Err(UpstreamError::Timeout);
        }
        let mut builder = self.client.request(method.clone(), upstream);
        for (name, value) in headers {
            builder = builder.header(name, value);
        }
        if !body.is_empty() {
            builder = builder.body(body);
        }
        let response = match tokio::time::timeout(timeout, builder.send()).await {
            Ok(Ok(response)) => response,
            Ok(Err(error)) => {
                self.record_outlier_failure(&target).await;
                roze_metrics::record_gateway_upstream(
                    route.service.clone(),
                    target.instance_key.clone(),
                    if error.is_timeout() {
                        "timeout"
                    } else {
                        "request_error"
                    },
                );
                return Err(if error.is_timeout() {
                    UpstreamError::Timeout
                } else {
                    UpstreamError::Request(error)
                });
            }
            Err(_) => {
                self.record_outlier_failure(&target).await;
                roze_metrics::record_gateway_upstream(
                    route.service.clone(),
                    target.instance_key.clone(),
                    "timeout",
                );
                return Err(UpstreamError::Timeout);
            }
        };
        let status =
            StatusCode::from_u16(response.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
        let response_headers = response
            .headers()
            .iter()
            .filter(|(name, _)| !is_hop_by_hop_header(name.as_str()))
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect::<Vec<_>>();
        if is_sse_response(status, response.headers()) {
            let permit = self.try_acquire_stream_connection(route, service, "sse")?;
            self.record_outlier_success(&target).await;
            roze_metrics::record_gateway_upstream(
                route.service.clone(),
                target.instance_key.clone(),
                "sse_opened",
            );
            let idle_timeout = route
                .stream_idle_timeout
                .or_else(|| service.stream_idle_timeout_ms.map(Duration::from_millis))
                .or(self.global_stream_idle_timeout);
            let mut out = http::Response::builder().status(status);
            for (name, value) in response_headers {
                out = out.header(name, value);
            }
            return Ok(out
                .body(SseBody::new(response.bytes_stream(), idle_timeout, permit).boxed())
                .expect("valid gateway SSE response"));
        }
        let body_timeout = context.remaining_timeout().unwrap_or(Duration::ZERO);
        if body_timeout.is_zero() {
            self.record_outlier_failure(&target).await;
            return Err(UpstreamError::Timeout);
        }
        let bytes = match tokio::time::timeout(body_timeout, response.bytes()).await {
            Ok(Ok(bytes)) => bytes,
            Ok(Err(error)) => {
                self.record_outlier_failure(&target).await;
                return Err(if error.is_timeout() {
                    UpstreamError::Timeout
                } else {
                    UpstreamError::Request(error)
                });
            }
            Err(_) => {
                self.record_outlier_failure(&target).await;
                return Err(UpstreamError::Timeout);
            }
        };
        if status.is_server_error() {
            self.record_outlier_failure(&target).await;
            roze_metrics::record_gateway_upstream(
                route.service.clone(),
                target.instance_key.clone(),
                format!("status_{}", status.as_u16()),
            );
        } else {
            self.record_outlier_success(&target).await;
            roze_metrics::record_gateway_upstream(
                route.service.clone(),
                target.instance_key.clone(),
                "ok",
            );
        }
        let mut out = http::Response::builder().status(status);
        for (name, value) in response_headers {
            out = out.header(name, value);
        }
        Ok(out
            .body(rest::full_body(bytes))
            .expect("valid gateway upstream response"))
    }

    async fn proxy_websocket(
        &self,
        request: &mut IncomingRequest,
        service: &GatewayService,
        route: &CompiledRoute,
        incoming_path: &str,
        context: &Context,
    ) -> Result<HttpResponse, UpstreamError> {
        let target = self.resolve_upstream(service, &route.instance_tags).await?;
        let permit = self.try_acquire_stream_connection(route, service, "websocket")?;
        let path = rewrite_path(&route.path, route.rewrite.as_deref(), incoming_path);
        let mut upstream_url = join_url(&target.base, &path);
        if let Some(query) = request.uri().query().filter(|query| !query.is_empty()) {
            upstream_url.push('?');
            upstream_url.push_str(query);
        }
        let mut websocket_target = websocket_target(&upstream_url)?;
        if let Some(server_name) = service
            .tls
            .as_ref()
            .and_then(|tls| tls.server_name.as_ref())
            .filter(|server_name| !server_name.trim().is_empty())
        {
            websocket_target.server_name = server_name.clone();
        }
        let expected_accept = expected_websocket_accept(request.headers())?;
        let timeout = context.remaining_timeout().unwrap_or(Duration::ZERO);
        if timeout.is_zero() {
            return Err(UpstreamError::Timeout);
        }
        let upstream =
            match tokio::time::timeout(timeout, TcpStream::connect(&websocket_target.addr)).await {
                Ok(Ok(stream)) => stream,
                Ok(Err(error)) => {
                    self.record_outlier_failure(&target).await;
                    return Err(UpstreamError::Protocol(format!(
                        "WebSocket upstream connect failed: {error}"
                    )));
                }
                Err(_) => {
                    self.record_outlier_failure(&target).await;
                    return Err(UpstreamError::Timeout);
                }
            };
        let mut upstream: Box<dyn WebSocketIo> = if websocket_target.secure {
            let tls_config = self
                .websocket_tls_configs
                .get(&service.name)
                .cloned()
                .unwrap_or_else(|| self.websocket_tls_config());
            let timeout = context.remaining_timeout().unwrap_or(Duration::ZERO);
            match tokio::time::timeout(
                timeout,
                connect_websocket_tls(upstream, &websocket_target, tls_config),
            )
            .await
            {
                Ok(Ok(stream)) => Box::new(stream),
                Ok(Err(error)) => {
                    self.record_outlier_failure(&target).await;
                    return Err(error);
                }
                Err(_) => {
                    self.record_outlier_failure(&target).await;
                    return Err(UpstreamError::Timeout);
                }
            }
        } else {
            Box::new(upstream)
        };
        let handshake_request = build_websocket_handshake_request(request, &websocket_target)?;
        let timeout = context.remaining_timeout().unwrap_or(Duration::ZERO);
        let handshake = match tokio::time::timeout(timeout, async {
            upstream
                .write_all(handshake_request.as_bytes())
                .await
                .map_err(|error| {
                    UpstreamError::Protocol(format!("WebSocket handshake write failed: {error}"))
                })?;
            read_websocket_handshake_response(upstream.as_mut()).await
        })
        .await
        {
            Ok(Ok(handshake)) => handshake,
            Ok(Err(error)) => {
                self.record_outlier_failure(&target).await;
                return Err(error);
            }
            Err(_) => {
                self.record_outlier_failure(&target).await;
                return Err(UpstreamError::Timeout);
            }
        };
        if handshake.status != StatusCode::SWITCHING_PROTOCOLS {
            self.record_outlier_failure(&target).await;
            return Err(UpstreamError::Protocol(format!(
                "WebSocket upstream handshake returned {}",
                handshake.status
            )));
        }
        validate_websocket_handshake(request.headers(), &handshake, &expected_accept)?;
        self.record_outlier_success(&target).await;
        roze_metrics::record_gateway_upstream(
            route.service.clone(),
            target.instance_key.clone(),
            "websocket_opened",
        );
        let on_upgrade = hyper::upgrade::on(request);
        let idle_timeout = route
            .stream_idle_timeout
            .or_else(|| service.stream_idle_timeout_ms.map(Duration::from_millis))
            .or(self.global_stream_idle_timeout);
        let service_name = route.service.clone();
        let instance_key = target.instance_key.clone();
        tokio::spawn(async move {
            let _permit = permit;
            let result = match on_upgrade.await {
                Ok(upgraded) => {
                    let mut client = TokioIo::new(upgraded);
                    if !handshake.remaining.is_empty() {
                        if let Err(error) = client.write_all(&handshake.remaining).await {
                            Err(error)
                        } else {
                            copy_websocket_tunnel(&mut client, upstream.as_mut(), idle_timeout)
                                .await
                        }
                    } else {
                        copy_websocket_tunnel(&mut client, upstream.as_mut(), idle_timeout).await
                    }
                }
                Err(error) => Err(std::io::Error::other(error)),
            };
            roze_metrics::record_gateway_upstream(
                service_name,
                instance_key,
                if result.is_ok() {
                    "websocket_closed"
                } else {
                    "websocket_error"
                },
            );
            if let Err(error) = result {
                tracing::warn!(
                    event = "gateway.websocket_tunnel_failed",
                    error = %error,
                    "gateway WebSocket tunnel failed"
                );
            }
        });

        let mut response = http::Response::builder()
            .status(StatusCode::SWITCHING_PROTOCOLS)
            .body(rest::empty_body())
            .expect("valid WebSocket upgrade response");
        for (name, value) in handshake.headers {
            if name != header::CONTENT_LENGTH && name != header::TRANSFER_ENCODING {
                response.headers_mut().insert(name, value);
            }
        }
        Ok(response)
    }

    fn websocket_tls_config(&self) -> Arc<rustls::ClientConfig> {
        self.websocket_tls_config
            .get_or_init(build_websocket_tls_config)
            .clone()
    }

    fn try_acquire_stream_connection(
        &self,
        route: &CompiledRoute,
        service: &GatewayService,
        protocol: &'static str,
    ) -> Result<Option<StreamConnectionPermit>, UpstreamError> {
        let maximum = route
            .max_stream_connections
            .or(service.max_stream_connections)
            .or(self.global_max_stream_connections);
        let key = OperationKey::new(
            &route.service,
            GovernanceBoundary::Gateway,
            format!("{protocol}:{}", route.path),
        )
        .to_string();
        let mut states = self.stream_connection_states.lock().map_err(|_| {
            UpstreamError::Unavailable("gateway stream connection state is poisoned".to_string())
        })?;
        let active = states.get(&key).copied().unwrap_or_default();
        if maximum.is_some_and(|maximum| active >= maximum) {
            roze_metrics::record_gateway_stream_connection(
                route.service.clone(),
                route.path.clone(),
                protocol,
                "rejected",
                active,
            );
            return Err(UpstreamError::StreamCapacity);
        }
        let active = active.saturating_add(1);
        states.insert(key.clone(), active);
        drop(states);
        roze_metrics::record_gateway_stream_connection(
            route.service.clone(),
            route.path.clone(),
            protocol,
            "opened",
            active,
        );
        Ok(Some(StreamConnectionPermit {
            states: self.stream_connection_states.clone(),
            key,
            service: route.service.clone(),
            route: route.path.clone(),
            protocol,
            started: Instant::now(),
        }))
    }

    async fn resolve_upstream(
        &self,
        service: &GatewayService,
        route_tags: &BTreeMap<String, String>,
    ) -> Result<UpstreamTarget, UpstreamError> {
        let registry_name = service
            .registry_name
            .as_deref()
            .filter(|name| !name.is_empty());
        if registry_name.is_some() || service.upstream.is_empty() {
            let name = registry_name.unwrap_or(&service.name);
            let registry = self.registry.as_ref().ok_or_else(|| {
                UpstreamError::Unavailable(format!(
                    "registry is required for gateway service '{}'",
                    service.name
                ))
            })?;
            let instances = registry.discover(name).await.map_err(|error| {
                UpstreamError::Unavailable(format!(
                    "failed to discover gateway service '{name}': {error}"
                ))
            })?;
            let mut tags = service.instance_tags.clone();
            tags.extend(route_tags.clone());
            let candidates = self
                .available_instances(name, instances, &tags, service.health_check.is_some())
                .await;
            let selection_key = format!("{name}:{tags:?}");
            let cursor = {
                let mut cursors = self
                    .registry_cursors
                    .lock()
                    .expect("gateway registry cursor lock");
                let cursor = cursors.entry(selection_key).or_default();
                let current = *cursor;
                *cursor = cursor.wrapping_add(1);
                current
            };
            let instance = pick_weighted_instance(&candidates, cursor).ok_or_else(|| {
                UpstreamError::Unavailable(format!(
                    "no healthy gateway instances for service '{name}' matching {tags:?}"
                ))
            })?;
            return Ok(UpstreamTarget {
                base: normalize_upstream_base(&instance.addr),
                instance_key: upstream_instance_key(name, &instance.addr),
                outlier: service.outlier,
            });
        }

        let target = UpstreamTarget {
            base: normalize_upstream_base(&service.upstream),
            instance_key: upstream_instance_key(&service.name, &service.upstream),
            outlier: service.outlier,
        };
        if !self
            .target_available(&target.instance_key, service.health_check.is_some())
            .await
        {
            return Err(UpstreamError::Unavailable(format!(
                "gateway upstream '{}' is unhealthy or ejected",
                target.instance_key
            )));
        }
        Ok(target)
    }

    async fn available_instances(
        &self,
        service_name: &str,
        instances: Vec<ServiceInstance>,
        required_tags: &BTreeMap<String, String>,
        health_check_enabled: bool,
    ) -> Vec<ServiceInstance> {
        let mut available = Vec::with_capacity(instances.len());
        for instance in instances {
            if !required_tags
                .iter()
                .all(|(key, value)| instance.metadata.get(key) == Some(value))
            {
                continue;
            }
            let key = upstream_instance_key(service_name, &instance.addr);
            if self.target_available(&key, health_check_enabled).await {
                available.push(instance);
            }
        }
        available
    }

    async fn target_available(&self, key: &str, health_check_enabled: bool) -> bool {
        let now = Instant::now();
        if self
            .outlier_states
            .lock()
            .await
            .get(key)
            .and_then(|state| state.ejected_until)
            .is_some_and(|until| until > now)
        {
            return false;
        }
        !health_check_enabled
            || self
                .health_states
                .lock()
                .await
                .get(key)
                .is_none_or(|state| state.healthy)
    }

    async fn record_outlier_success(&self, target: &UpstreamTarget) {
        if target.outlier.is_none() {
            return;
        }
        if let Some(state) = self
            .outlier_states
            .lock()
            .await
            .get_mut(&target.instance_key)
        {
            state.failures = 0;
            state.ejected_until = None;
        }
    }

    async fn record_outlier_failure(&self, target: &UpstreamTarget) {
        let Some(config) = target.outlier else {
            return;
        };
        let mut states = self.outlier_states.lock().await;
        let state = states.entry(target.instance_key.clone()).or_default();
        state.failures = state.failures.saturating_add(1);
        if state.failures >= config.failure_threshold.max(1) {
            state.failures = 0;
            state.ejected_until = Some(Instant::now() + Duration::from_millis(config.ejection_ms));
            roze_metrics::record_gateway_upstream(
                "gateway",
                target.instance_key.clone(),
                "ejected",
            );
            tracing::warn!(
                event = "gateway.upstream_ejected",
                upstream = %target.instance_key,
                ejection_ms = config.ejection_ms,
                "gateway upstream instance ejected"
            );
        }
    }

    fn spawn_health_checks(self: &Arc<Self>) {
        if tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        for service in self.services.values().cloned() {
            let Some(config) = service.health_check.clone() else {
                continue;
            };
            let weak = Arc::downgrade(self);
            let task = tokio::spawn(async move {
                loop {
                    let Some(runtime) = Weak::upgrade(&weak) else {
                        break;
                    };
                    runtime.check_service_health(&service, &config).await;
                    drop(runtime);
                    tokio::time::sleep(Duration::from_millis(config.interval_ms.max(1))).await;
                }
            });
            self.health_tasks
                .lock()
                .expect("gateway health task lock")
                .push(task);
        }
    }

    async fn check_service_health(
        &self,
        service: &GatewayService,
        config: &GatewayHealthCheckConfig,
    ) {
        let registry_name = service
            .registry_name
            .as_deref()
            .filter(|name| !name.is_empty());
        let targets = if registry_name.is_some() || service.upstream.is_empty() {
            let Some(registry) = self.registry.as_ref() else {
                return;
            };
            let name = registry_name.unwrap_or(&service.name);
            match registry.discover(name).await {
                Ok(instances) => instances
                    .into_iter()
                    .map(|instance| {
                        (
                            upstream_instance_key(name, &instance.addr),
                            normalize_upstream_base(&instance.addr),
                        )
                    })
                    .collect(),
                Err(error) => {
                    tracing::warn!(
                        event = "gateway.health_discovery_failed",
                        service = %service.name,
                        error = %error,
                        "gateway health check discovery failed"
                    );
                    return;
                }
            }
        } else {
            vec![(
                upstream_instance_key(&service.name, &service.upstream),
                normalize_upstream_base(&service.upstream),
            )]
        };

        for (key, base) in targets {
            let url = join_url(&base, &config.path);
            let healthy = matches!(
                tokio::time::timeout(
                    Duration::from_millis(config.timeout_ms.max(1)),
                    self.client.get(url).send()
                )
                .await,
                Ok(Ok(response)) if response.status().as_u16() == config.expected_status
            );
            self.record_health_result(&key, config, healthy).await;
        }
    }

    async fn record_health_result(
        &self,
        key: &str,
        config: &GatewayHealthCheckConfig,
        healthy: bool,
    ) {
        let mut states = self.health_states.lock().await;
        let state = states.entry(key.to_string()).or_default();
        if healthy {
            state.failures = 0;
            state.successes = state.successes.saturating_add(1);
            if !state.healthy && state.successes >= config.healthy_threshold.max(1) {
                state.healthy = true;
                roze_metrics::record_gateway_upstream("gateway", key, "healthy");
            }
        } else {
            state.successes = 0;
            state.failures = state.failures.saturating_add(1);
            if state.healthy && state.failures >= config.unhealthy_threshold.max(1) {
                state.healthy = false;
                roze_metrics::record_gateway_upstream("gateway", key, "unhealthy");
            }
        }
    }

    fn select_route(&self, request: &IncomingRequest) -> Option<&CompiledRoute> {
        let path = request.uri().path();
        let selection_key = gateway_selection_key(request.headers(), path);
        let bucket = stable_bucket(&selection_key);
        self.routes.iter().find(|route| {
            route.matches_path(path)
                && route.matches_request(request.headers())
                && bucket < route.traffic_percent
        })
    }

    fn dispatch_mirror(
        &self,
        route: &CompiledRoute,
        method: &Method,
        path: &str,
        query: Option<&str>,
        headers: &[(HeaderName, HeaderValue)],
        body: &Bytes,
    ) {
        let Some(mirror_name) = route.mirror_service.as_ref() else {
            return;
        };
        if stable_bucket(&format!("mirror:{}:{}", route.path, path)) >= route.mirror_percent {
            return;
        }
        let Some(service) = self.services.get(mirror_name) else {
            return;
        };
        if service.upstream.trim().is_empty() {
            return;
        }
        let mut upstream = join_url(&service.upstream, path);
        if let Some(query) = query.filter(|query| !query.is_empty()) {
            upstream.push('?');
            upstream.push_str(query);
        }
        let client = self.client.clone();
        let method = method.clone();
        let headers = headers.to_vec();
        let body = body.clone();
        let timeout = service
            .timeout_ms
            .map(Duration::from_millis)
            .or(self.global_timeout)
            .unwrap_or(DEFAULT_TIMEOUT);
        tokio::spawn(async move {
            let mut builder = client.request(method, upstream).timeout(timeout);
            for (name, value) in headers {
                builder = builder.header(name, value);
            }
            if !body.is_empty() {
                builder = builder.body(body);
            }
            let _ = builder.send().await;
        });
    }

    fn auth_policy(&self, route: &CompiledRoute) -> Option<AuthPolicy> {
        if has_middleware(&route.middlewares, "jwt")
            || has_middleware(&self.global_middlewares, "jwt")
        {
            Some(AuthPolicy::Jwt)
        } else if has_middleware(&route.middlewares, "api_key")
            || has_middleware(&route.middlewares, "apikey")
            || has_middleware(&self.global_middlewares, "api_key")
            || has_middleware(&self.global_middlewares, "apikey")
        {
            Some(AuthPolicy::ApiKey)
        } else if has_middleware(&route.middlewares, "auth")
            || has_middleware(&self.global_middlewares, "auth")
        {
            Some(AuthPolicy::Any)
        } else {
            None
        }
    }

    fn finish_response(
        &self,
        route: Option<&CompiledRoute>,
        method: &Method,
        outcome: &str,
        started: Instant,
        response: HttpResponse,
    ) -> HttpResponse {
        let status = response.status();
        let (service, route_path) = route
            .map(|route| (route.service.as_str(), route.path.as_str()))
            .unwrap_or(("gateway", ""));
        roze_metrics::record_gateway_route(
            service,
            route_path,
            method.as_str(),
            status.as_u16().to_string(),
            outcome,
            started.elapsed(),
        );
        roze_metrics::record_http_request(status.is_success(), started.elapsed());
        response
    }
}

#[derive(Debug)]
enum UpstreamError {
    Timeout,
    Request(reqwest::Error),
    Unavailable(String),
    StreamCapacity,
    Protocol(String),
}

impl std::fmt::Display for UpstreamError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Timeout => formatter.write_str("upstream timeout"),
            Self::Request(error) => error.fmt(formatter),
            Self::Unavailable(error) | Self::Protocol(error) => formatter.write_str(error),
            Self::StreamCapacity => formatter.write_str("too many active gateway streams"),
        }
    }
}

impl std::error::Error for UpstreamError {}

struct UpstreamRequest<'a> {
    service: &'a GatewayService,
    route: &'a CompiledRoute,
    method: &'a Method,
    incoming_path: &'a str,
    query: Option<&'a str>,
    headers: &'a [(HeaderName, HeaderValue)],
    body: Bytes,
    context: &'a Context,
}

pub fn validate_gateway_config(config: &GatewayConfig) -> anyhow::Result<()> {
    let mut services = std::collections::HashSet::new();
    for service in &config.services {
        anyhow::ensure!(
            !service.name.trim().is_empty(),
            "gateway service name is empty"
        );
        anyhow::ensure!(
            services.insert(service.name.as_str()),
            "duplicate gateway service '{}'",
            service.name
        );
        anyhow::ensure!(
            !service.upstream.trim().is_empty()
                || service
                    .registry_name
                    .as_deref()
                    .is_some_and(|name| !name.trim().is_empty()),
            "gateway service '{}' has neither upstream nor registry_name",
            service.name
        );
        if let Some(outlier) = service.outlier {
            anyhow::ensure!(
                outlier.failure_threshold > 0 && outlier.ejection_ms > 0,
                "gateway service '{}' has invalid outlier policy",
                service.name
            );
        }
        if let Some(health) = service.health_check.as_ref() {
            anyhow::ensure!(
                health.path.starts_with('/')
                    && health.interval_ms > 0
                    && health.timeout_ms > 0
                    && health.unhealthy_threshold > 0
                    && health.healthy_threshold > 0
                    && StatusCode::from_u16(health.expected_status).is_ok(),
                "gateway service '{}' has invalid health check",
                service.name
            );
        }
        if let Some(tls) = service.tls.as_ref() {
            anyhow::ensure!(
                tls.client_cert_file.is_some() == tls.client_key_file.is_some(),
                "gateway service '{}' must configure client_cert_file and client_key_file together",
                service.name
            );
            for path in &tls.ca_files {
                anyhow::ensure!(
                    !path.as_os_str().is_empty(),
                    "gateway service '{}' has an empty CA path",
                    service.name
                );
            }
            if let Some(server_name) = tls.server_name.as_ref() {
                rustls::pki_types::ServerName::try_from(server_name.clone()).map_err(|error| {
                    anyhow::anyhow!(
                        "gateway service '{}' has invalid TLS server_name '{}': {error}",
                        service.name,
                        server_name
                    )
                })?;
            }
        }
    }
    for route in &config.routes {
        anyhow::ensure!(
            route.path.starts_with('/'),
            "gateway route '{}' must start with '/'",
            route.path
        );
        anyhow::ensure!(
            services.contains(route.service.as_str()),
            "gateway route '{}' references unknown service '{}'",
            route.path,
            route.service
        );
        for method in &route.methods {
            method.parse::<Method>().map_err(|error| {
                anyhow::anyhow!(
                    "gateway route '{}' has invalid method '{}': {error}",
                    route.path,
                    method
                )
            })?;
        }
        anyhow::ensure!(
            route.traffic_percent <= 100 && route.mirror_percent <= 100,
            "gateway route '{}' traffic percentages must be in 0..=100",
            route.path
        );
        if let Some(mirror) = route.mirror_service.as_ref() {
            anyhow::ensure!(
                services.contains(mirror.as_str()),
                "gateway route '{}' references unknown mirror service '{}'",
                route.path,
                mirror
            );
        }
        if let Some(maximum) = route.max_stream_connections {
            anyhow::ensure!(
                maximum > 0,
                "gateway route '{}' has zero max_stream_connections",
                route.path
            );
        }
        if let Some(fallback) = route.fallback.as_ref() {
            StatusCode::from_u16(fallback.status).map_err(|error| {
                anyhow::anyhow!(
                    "gateway route '{}' has invalid fallback status: {error}",
                    route.path
                )
            })?;
        }
    }
    if let Some(cors) = config.cors.as_ref() {
        for origin in &cors.allow_origins {
            if origin != "*" {
                origin.parse::<HeaderValue>().map_err(|error| {
                    anyhow::anyhow!("gateway CORS origin '{origin}' is invalid: {error}")
                })?;
            }
        }
        for method in &cors.allow_methods {
            method.parse::<Method>().map_err(|error| {
                anyhow::anyhow!("gateway CORS method '{method}' is invalid: {error}")
            })?;
        }
        for name in &cors.allow_headers {
            if name != "*" {
                name.parse::<HeaderName>().map_err(|error| {
                    anyhow::anyhow!("gateway CORS header '{name}' is invalid: {error}")
                })?;
            }
        }
    }
    if let Some(fallback) = config.fallback.as_ref() {
        StatusCode::from_u16(fallback.status)
            .map_err(|error| anyhow::anyhow!("invalid gateway fallback status: {error}"))?;
    }
    Ok(())
}

fn compile_routes(
    routes: Vec<GatewayRoute>,
    governance: Option<&GovernanceConfig>,
) -> Vec<CompiledRoute> {
    routes
        .into_iter()
        .map(|route| {
            let path = normalize_path_prefix(&route.path);
            let rate_limit = route.rate_limit.clone().or_else(|| {
                governance.and_then(|governance| {
                    governance.resolve_rate_limit_config_for([
                        path.as_str(),
                        path.trim_start_matches('/'),
                        route.service.as_str(),
                    ])
                })
            });
            let policy = governance.map(|governance| {
                governance.resolve_policy_for([
                    path.as_str(),
                    path.trim_start_matches('/'),
                    route.service.as_str(),
                ])
            });
            let retry = policy.as_ref().and_then(|policy| policy.retry);
            CompiledRoute {
                path,
                service: route.service,
                methods: parse_methods(&route.methods),
                match_headers: route.match_headers,
                match_cookies: route.match_cookies,
                traffic_percent: route.traffic_percent,
                mirror_service: route.mirror_service,
                mirror_percent: route.mirror_percent,
                timeout: route
                    .timeout_ms
                    .map(Duration::from_millis)
                    .or_else(|| policy.as_ref().and_then(|policy| policy.timeout)),
                retries: route
                    .retries
                    .map(|retries| retries as usize)
                    .unwrap_or_else(|| {
                        retry
                            .map(|retry| retry.max_attempts.saturating_sub(1) as usize)
                            .unwrap_or_default()
                    }),
                retry_backoff: route
                    .retry_backoff_ms
                    .map(Duration::from_millis)
                    .or_else(|| retry.map(|retry| retry.backoff))
                    .unwrap_or_default(),
                retry_max_backoff: retry.map(|retry| retry.max_backoff).unwrap_or_else(|| {
                    route
                        .retry_backoff_ms
                        .map(Duration::from_millis)
                        .unwrap_or_default()
                }),
                retry_budget_percent: retry.and_then(|retry| retry.budget_percent),
                rewrite: route.rewrite,
                fallback: route.fallback.or_else(|| {
                    policy
                        .as_ref()
                        .and_then(|policy| policy.fallback.as_ref())
                        .map(|fallback| GatewayFallbackResponse {
                            status: fallback.status,
                            body: fallback.body.clone(),
                            headers: fallback.headers.clone(),
                        })
                }),
                rate_limit,
                breaker: route.breaker.or_else(|| {
                    policy
                        .as_ref()
                        .and_then(|policy| policy.breaker)
                        .map(|config| BreakerConfig {
                            failure_threshold: config.failure_threshold,
                            reset_timeout_ms: config
                                .reset_timeout
                                .as_millis()
                                .min(u128::from(u64::MAX))
                                as u64,
                        })
                }),
                shedding: route.shedding.or_else(|| {
                    policy
                        .as_ref()
                        .and_then(|policy| policy.shedding)
                        .map(|config| SheddingConfig {
                            concurrency: config.concurrency,
                            window_ms: config.window.as_millis().min(u128::from(u64::MAX)) as u64,
                            min_samples: config.min_samples,
                            max_avg_latency_ms: config
                                .max_avg_latency
                                .as_millis()
                                .min(u128::from(u64::MAX))
                                as u64,
                            max_failure_ratio_per_mille: config.max_failure_ratio_per_mille,
                            cool_down_ms: config.cool_down.as_millis().min(u128::from(u64::MAX))
                                as u64,
                        })
                }),
                middlewares: normalize_middlewares(route.middlewares),
                instance_tags: route.instance_tags,
                stream_idle_timeout: route.stream_idle_timeout_ms.map(Duration::from_millis),
                max_stream_connections: route.max_stream_connections,
            }
        })
        .collect()
}

fn governance_fallback(config: &GovernanceConfig) -> Option<GatewayFallbackResponse> {
    config.fallback.as_ref().and_then(convert_fallback)
}

fn convert_fallback(config: &GovernanceFallbackConfig) -> Option<GatewayFallbackResponse> {
    config.enabled.then(|| GatewayFallbackResponse {
        status: config.status,
        body: config.body.clone(),
        headers: config.headers.clone(),
    })
}

impl CompiledRoute {
    fn matches_path(&self, request_path: &str) -> bool {
        self.path == "/"
            || request_path == self.path
            || request_path.starts_with(&format!("{}/", self.path))
    }

    fn method_allowed(&self, method: &Method) -> bool {
        self.methods.is_empty() || self.methods.iter().any(|allowed| allowed == method)
    }

    fn matches_request(&self, headers: &HeaderMap) -> bool {
        let headers_match = self.match_headers.iter().all(|(name, expected)| {
            headers
                .get(name)
                .and_then(|value| value.to_str().ok())
                .is_some_and(|value| value == expected)
        });
        headers_match
            && self.match_cookies.iter().all(|(name, expected)| {
                cookie_value(headers, name).is_some_and(|value| value == expected)
            })
    }
}

fn cookie_value<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .find_map(|part| {
            let (key, value) = part.trim().split_once('=')?;
            (key == name).then_some(value)
        })
}

fn gateway_selection_key(headers: &HeaderMap, path: &str) -> String {
    for name in ["x-user-id", "x-tenant-id", roze_context::REQUEST_ID_HEADER] {
        if let Some(value) = headers.get(name).and_then(|value| value.to_str().ok()) {
            return value.to_string();
        }
    }
    path.to_string()
}

fn stable_bucket(value: &str) -> u32 {
    let hash = value.bytes().fold(0x811c9dc5u32, |hash, byte| {
        (hash ^ u32::from(byte)).wrapping_mul(0x01000193)
    });
    hash % 100
}

fn parse_methods(raw: &[String]) -> Vec<Method> {
    if raw
        .iter()
        .any(|method| matches!(method.trim(), "*" | "ALL" | "all"))
    {
        return Vec::new();
    }
    raw.iter()
        .filter_map(|method| method.trim().to_uppercase().parse().ok())
        .collect()
}

fn normalize_path_prefix(path: &str) -> String {
    let normalized = path.trim_end_matches('/');
    if normalized.is_empty() {
        "/".to_string()
    } else if normalized.starts_with('/') {
        normalized.to_string()
    } else {
        format!("/{normalized}")
    }
}

fn normalize_middlewares(raw: Vec<String>) -> Vec<String> {
    raw.into_iter()
        .map(|item| item.trim().to_lowercase())
        .filter(|item| !item.is_empty())
        .collect()
}

fn has_middleware(items: &[String], name: &str) -> bool {
    items.iter().any(|item| {
        item == name || item == &format!("builtin:{name}") || item == &format!("builtin::{name}")
    })
}

fn validate_request_auth(
    headers: &HeaderMap,
    policy: AuthPolicy,
    jwt: Option<&JwtConfig>,
    api_keys: Option<&roze_auth::ApiKeyConfig>,
) -> Option<roze_auth::AuthPrincipal> {
    if matches!(policy, AuthPolicy::Jwt | AuthPolicy::Any) {
        if let (Some(jwt), Some(token)) = (
            jwt,
            headers
                .get(header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok())
                .and_then(roze_jwt::extract_bearer_token),
        ) {
            if let Ok(claims) = verify_token(token, jwt) {
                return Some(claims.into());
            }
        }
    }
    if matches!(policy, AuthPolicy::ApiKey | AuthPolicy::Any) {
        if let Some(config) = api_keys {
            if let Some(value) = config
                .header
                .parse::<HeaderName>()
                .ok()
                .and_then(|name| headers.get(name))
                .and_then(|value| value.to_str().ok())
            {
                if let Some(principal) = roze_auth::verify_api_key(value, config) {
                    return Some(principal);
                }
            }
        }
    }
    None
}

fn clear_untrusted_auth_context_headers(headers: &mut HeaderMap) {
    for name in [
        roze_context::SUBJECT_HEADER,
        roze_context::TENANT_HEADER,
        roze_context::ROLES_HEADER,
        roze_context::PERMISSIONS_HEADER,
        roze_context::SCOPE_HEADER,
        roze_context::HULA_UID_HEADER,
        roze_context::HULA_TENANT_ID_HEADER,
        roze_context::HULA_ROLE_HEADER,
        roze_context::HULA_SCOPE_HEADER,
    ] {
        headers.remove(name);
    }
}

fn inject_auth_context_headers(headers: &mut HeaderMap, principal: &roze_auth::AuthPrincipal) {
    insert_header(headers, roze_context::SUBJECT_HEADER, &principal.subject);
    if let Some(tenant) = principal
        .tenant
        .as_deref()
        .filter(|tenant| !tenant.is_empty())
    {
        insert_header(headers, roze_context::TENANT_HEADER, tenant);
    }
    if !principal.roles.is_empty() {
        insert_header(
            headers,
            roze_context::ROLES_HEADER,
            &principal.roles.join(","),
        );
    }
    if !principal.permissions.is_empty() {
        insert_header(
            headers,
            roze_context::PERMISSIONS_HEADER,
            &principal.permissions.join(","),
        );
    }
    if !principal.scopes.is_empty() {
        insert_header(
            headers,
            roze_context::SCOPE_HEADER,
            &principal.scopes.join(","),
        );
    }
}

fn insert_header(headers: &mut HeaderMap, name: &'static str, value: &str) {
    if let Ok(value) = HeaderValue::from_str(value) {
        headers.insert(name, value);
    }
}

fn gateway_context(context: Context, timeout: Duration) -> Context {
    match (context.deadline(), context.remaining_timeout()) {
        (Some(_), None) => context,
        (Some(_), Some(remaining)) => context.with_timeout(remaining.min(timeout)),
        (None, _) => context.with_timeout(timeout),
    }
}

fn retry_context_exhausted(context: &Context, delay: Duration) -> bool {
    context.cancelled()
        || (context.deadline().is_some()
            && context
                .remaining_timeout()
                .is_none_or(|remaining| remaining <= delay))
}

fn is_idempotent_method(method: &Method) -> bool {
    method == Method::GET
        || method == Method::HEAD
        || method == Method::PUT
        || method == Method::DELETE
        || method == Method::OPTIONS
        || method == Method::TRACE
}

fn retryable_gateway_status(status: StatusCode) -> bool {
    matches!(
        status,
        StatusCode::INTERNAL_SERVER_ERROR
            | StatusCode::BAD_GATEWAY
            | StatusCode::SERVICE_UNAVAILABLE
            | StatusCode::GATEWAY_TIMEOUT
    )
}

fn shedding_config(config: SheddingConfig) -> roze_resilience::SheddingConfig {
    roze_resilience::SheddingConfig {
        concurrency: config.concurrency,
        window: Duration::from_millis(config.window_ms),
        min_samples: config.min_samples,
        max_avg_latency: Duration::from_millis(config.max_avg_latency_ms),
        max_failure_ratio_per_mille: config.max_failure_ratio_per_mille,
        cool_down: Duration::from_millis(config.cool_down_ms),
    }
}

fn finish_breaker(
    registry: &BreakerRegistry,
    key: &str,
    permit: Option<BreakerPermit>,
    config: Option<BreakerConfig>,
    success: bool,
) {
    let (Some(permit), Some(config)) = (permit, config) else {
        return;
    };
    roze_metrics::record_resilience_decision(
        "gateway",
        "gateway",
        "breaker",
        if success { "success" } else { "failure" },
    );
    if success {
        registry.record_success(key, permit);
    } else {
        registry.record_failure(
            key,
            permit,
            roze_resilience::BreakerConfig {
                failure_threshold: config.failure_threshold,
                reset_timeout: Duration::from_millis(config.reset_timeout_ms),
            },
        );
    }
}

fn cancel_breaker(
    registry: &BreakerRegistry,
    key: &str,
    permit: Option<BreakerPermit>,
    config: Option<BreakerConfig>,
) {
    let (Some(permit), Some(config)) = (permit, config) else {
        return;
    };
    registry.cancel(
        key,
        permit,
        roze_resilience::BreakerConfig {
            failure_threshold: config.failure_threshold,
            reset_timeout: Duration::from_millis(config.reset_timeout_ms),
        },
    );
}

fn ensure_correlation_header(request: &mut IncomingRequest, name: &'static str) {
    if request.headers().contains_key(name) {
        return;
    }
    if let Ok(value) = HeaderValue::from_str(&roze_trace::generate_trace_id()) {
        request.headers_mut().insert(name, value);
    }
}

fn fallback_response(
    fallback: Option<&GatewayFallbackResponse>,
    status: StatusCode,
    message: &str,
) -> HttpResponse {
    let Some(fallback) = fallback else {
        return rest::text_response(status, message);
    };
    let status = StatusCode::from_u16(fallback.status).unwrap_or(status);
    let mut response = match fallback.body.as_ref() {
        Some(body) => rest::json_response(status, body),
        None => rest::text_response(status, message),
    };
    for (name, value) in &fallback.headers {
        if let (Ok(name), Ok(value)) = (name.parse::<HeaderName>(), HeaderValue::from_str(value)) {
            response.headers_mut().insert(name, value);
        }
    }
    response
}

fn rewrite_path(route_path: &str, rewrite: Option<&str>, incoming_path: &str) -> String {
    let Some(rewrite) = rewrite else {
        return incoming_path.to_string();
    };
    let suffix = incoming_path.strip_prefix(route_path).unwrap_or_default();
    format!(
        "{}/{}",
        rewrite.trim_end_matches('/'),
        suffix.trim_start_matches('/')
    )
    .trim_end_matches('/')
    .to_string()
}

fn join_url(base: &str, path: &str) -> String {
    format!(
        "{}/{}",
        base.trim_end_matches('/'),
        path.trim_start_matches('/')
    )
}

fn is_sse_response(status: StatusCode, headers: &HeaderMap) -> bool {
    status.is_success()
        && headers
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(';').next())
            .is_some_and(|value| value.trim().eq_ignore_ascii_case("text/event-stream"))
}

fn is_websocket_upgrade(headers: &HeaderMap) -> bool {
    headers
        .get(header::UPGRADE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.eq_ignore_ascii_case("websocket"))
        && headers
            .get(header::CONNECTION)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| {
                value
                    .split(',')
                    .any(|part| part.trim().eq_ignore_ascii_case("upgrade"))
            })
}

fn websocket_target(upstream_url: &str) -> Result<WebSocketTarget, UpstreamError> {
    let url = reqwest::Url::parse(upstream_url).map_err(|error| {
        UpstreamError::Protocol(format!("invalid WebSocket upstream URL: {error}"))
    })?;
    if !matches!(url.scheme(), "http" | "ws" | "https" | "wss") {
        return Err(UpstreamError::Protocol(format!(
            "WebSocket upstream requires ws/http/wss/https, got {}",
            url.scheme()
        )));
    }
    let parsed_host = url
        .host_str()
        .ok_or_else(|| UpstreamError::Protocol("WebSocket upstream URL has no host".to_string()))?;
    let host = parsed_host
        .strip_prefix('[')
        .and_then(|host| host.strip_suffix(']'))
        .unwrap_or(parsed_host);
    let secure = matches!(url.scheme(), "https" | "wss");
    let port = url
        .port_or_known_default()
        .unwrap_or(if secure { 443 } else { 80 });
    let authority_host = if host.contains(':') {
        format!("[{host}]")
    } else {
        host.to_string()
    };
    let authority = if url.port().is_some() {
        format!("{authority_host}:{port}")
    } else {
        authority_host
    };
    let path = if url.path().is_empty() {
        "/"
    } else {
        url.path()
    };
    let path_and_query = match url.query() {
        Some(query) if !query.is_empty() => format!("{path}?{query}"),
        _ => path.to_string(),
    };
    Ok(WebSocketTarget {
        addr: if host.contains(':') {
            format!("[{host}]:{port}")
        } else {
            format!("{host}:{port}")
        },
        authority,
        path_and_query,
        server_name: host.to_string(),
        secure,
    })
}

fn default_websocket_root_store() -> rustls::RootCertStore {
    let mut roots =
        rustls::RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let loaded = rustls_native_certs::load_native_certs();
    for error in &loaded.errors {
        tracing::warn!(
            event = "gateway.websocket_native_certificate_rejected",
            error = %error,
            "native certificate could not be loaded"
        );
    }
    for certificate in loaded.certs {
        if let Err(error) = roots.add(certificate) {
            tracing::warn!(
                event = "gateway.websocket_root_certificate_rejected",
                error = %error,
                "native root certificate was rejected"
            );
        }
    }
    roots
}

fn websocket_tls_builder(
    roots: rustls::RootCertStore,
) -> rustls::ConfigBuilder<rustls::ClientConfig, rustls::client::WantsClientCert> {
    rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .expect("Ring supports Rustls safe protocol versions")
        .with_root_certificates(roots)
}

fn build_websocket_tls_config() -> Arc<rustls::ClientConfig> {
    Arc::new(websocket_tls_builder(default_websocket_root_store()).with_no_client_auth())
}

fn build_service_websocket_tls_configs(
    services: &[GatewayService],
) -> anyhow::Result<HashMap<String, Arc<rustls::ClientConfig>>> {
    let mut configs = HashMap::new();
    for service in services {
        let Some(tls) = service.tls.as_ref() else {
            continue;
        };
        if tls.ca_files.is_empty()
            && tls.client_cert_file.is_none()
            && tls.client_key_file.is_none()
        {
            continue;
        }
        let mut roots = default_websocket_root_store();
        for path in &tls.ca_files {
            for certificate in read_pem_certificates(path)? {
                roots.add(certificate).map_err(|error| {
                    anyhow::anyhow!(
                        "gateway service '{}' rejected CA '{}': {error}",
                        service.name,
                        path.display()
                    )
                })?;
            }
        }
        let config = match (&tls.client_cert_file, &tls.client_key_file) {
            (Some(certificate_path), Some(key_path)) => {
                let certificates = read_pem_certificates(certificate_path)?;
                anyhow::ensure!(
                    !certificates.is_empty(),
                    "gateway service '{}' client certificate file '{}' is empty",
                    service.name,
                    certificate_path.display()
                );
                let key =
                    rustls::pki_types::PrivateKeyDer::from_pem_file(key_path).map_err(|error| {
                        anyhow::anyhow!(
                            "gateway service '{}' cannot parse client key '{}': {error}",
                            service.name,
                            key_path.display()
                        )
                    })?;
                websocket_tls_builder(roots)
                    .with_client_auth_cert(certificates, key)
                    .map_err(|error| {
                        anyhow::anyhow!(
                            "gateway service '{}' client identity is invalid: {error}",
                            service.name
                        )
                    })?
            }
            (None, None) => websocket_tls_builder(roots).with_no_client_auth(),
            _ => unreachable!("gateway TLS identity pair was validated"),
        };
        configs.insert(service.name.clone(), Arc::new(config));
    }
    Ok(configs)
}

fn read_pem_certificates(
    path: &std::path::Path,
) -> anyhow::Result<Vec<rustls::pki_types::CertificateDer<'static>>> {
    rustls::pki_types::CertificateDer::pem_file_iter(path)
        .map_err(|error| {
            anyhow::anyhow!("cannot open TLS certificate '{}': {error}", path.display())
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| {
            anyhow::anyhow!("cannot parse TLS certificate '{}': {error}", path.display())
        })
}

async fn connect_websocket_tls(
    stream: TcpStream,
    target: &WebSocketTarget,
    config: Arc<rustls::ClientConfig>,
) -> Result<tokio_rustls::client::TlsStream<TcpStream>, UpstreamError> {
    let server_name =
        rustls::pki_types::ServerName::try_from(target.server_name.clone()).map_err(|error| {
            UpstreamError::Protocol(format!("invalid WebSocket TLS server name: {error}"))
        })?;
    tokio_rustls::TlsConnector::from(config)
        .connect(server_name, stream)
        .await
        .map_err(|error| {
            UpstreamError::Protocol(format!("WebSocket TLS handshake failed: {error}"))
        })
}

fn build_websocket_handshake_request(
    request: &IncomingRequest,
    target: &WebSocketTarget,
) -> Result<String, UpstreamError> {
    use std::fmt::Write as _;

    if request.method() != Method::GET
        || !is_websocket_upgrade(request.headers())
        || request.headers().get(header::SEC_WEBSOCKET_KEY).is_none()
        || request
            .headers()
            .get(header::SEC_WEBSOCKET_VERSION)
            .and_then(|value| value.to_str().ok())
            != Some("13")
    {
        return Err(UpstreamError::Protocol(
            "invalid WebSocket upgrade request".to_string(),
        ));
    }
    let mut out = String::new();
    write!(&mut out, "GET {} HTTP/1.1\r\n", target.path_and_query)
        .expect("writing to String cannot fail");
    write!(&mut out, "Host: {}\r\n", target.authority).expect("writing to String cannot fail");
    for (name, value) in request.headers() {
        if name == header::HOST || name == header::CONTENT_LENGTH {
            continue;
        }
        let value = value.to_str().map_err(|_| {
            UpstreamError::Protocol(format!("WebSocket header '{name}' is not valid text"))
        })?;
        writeln!(&mut out, "{}: {}\r", name.as_str(), value)
            .expect("writing to String cannot fail");
    }
    out.push_str("\r\n");
    Ok(out)
}

fn expected_websocket_accept(headers: &HeaderMap) -> Result<String, UpstreamError> {
    let key = headers
        .get(header::SEC_WEBSOCKET_KEY)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| UpstreamError::Protocol("missing WebSocket key".to_string()))?;
    let mut digest = Sha1::new();
    digest.update(key.as_bytes());
    digest.update(b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11");
    Ok(base64::engine::general_purpose::STANDARD.encode(digest.finalize()))
}

fn validate_websocket_handshake(
    request_headers: &HeaderMap,
    handshake: &WebSocketHandshakeResponse,
    expected_accept: &str,
) -> Result<(), UpstreamError> {
    let response_headers = handshake.headers.iter().cloned().collect::<HeaderMap>();
    if !is_websocket_upgrade(&response_headers)
        || response_headers
            .get(header::SEC_WEBSOCKET_ACCEPT)
            .and_then(|value| value.to_str().ok())
            != Some(expected_accept)
    {
        return Err(UpstreamError::Protocol(
            "invalid WebSocket upstream acceptance headers".to_string(),
        ));
    }
    if let Some(protocol) = response_headers
        .get(header::SEC_WEBSOCKET_PROTOCOL)
        .and_then(|value| value.to_str().ok())
    {
        let requested = request_headers
            .get(header::SEC_WEBSOCKET_PROTOCOL)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.split(',').any(|item| item.trim() == protocol));
        if !requested {
            return Err(UpstreamError::Protocol(
                "WebSocket upstream selected an unrequested protocol".to_string(),
            ));
        }
    }
    Ok(())
}

async fn read_websocket_handshake_response<R>(
    upstream: &mut R,
) -> Result<WebSocketHandshakeResponse, UpstreamError>
where
    R: AsyncRead + Unpin + ?Sized,
{
    const MAX_HANDSHAKE_BYTES: usize = 16 * 1024;
    let mut buffer = Vec::new();
    let mut chunk = [0_u8; 1024];
    let header_end = loop {
        if buffer.len() > MAX_HANDSHAKE_BYTES {
            return Err(UpstreamError::Protocol(format!(
                "WebSocket upstream handshake exceeded {MAX_HANDSHAKE_BYTES} bytes"
            )));
        }
        let read = upstream.read(&mut chunk).await.map_err(|error| {
            UpstreamError::Protocol(format!("WebSocket handshake read failed: {error}"))
        })?;
        if read == 0 {
            return Err(UpstreamError::Protocol(
                "WebSocket upstream closed during handshake".to_string(),
            ));
        }
        buffer.extend_from_slice(&chunk[..read]);
        if let Some(position) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
            break position;
        }
    };
    let remaining = buffer[header_end + 4..].to_vec();
    let header_text = std::str::from_utf8(&buffer[..header_end]).map_err(|error| {
        UpstreamError::Protocol(format!("WebSocket handshake is not UTF-8: {error}"))
    })?;
    let mut lines = header_text.split("\r\n");
    let status = lines
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|status| status.parse::<u16>().ok())
        .and_then(|status| StatusCode::from_u16(status).ok())
        .ok_or_else(|| {
            UpstreamError::Protocol("invalid WebSocket upstream status line".to_string())
        })?;
    let mut headers = Vec::new();
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            return Err(UpstreamError::Protocol(
                "invalid WebSocket upstream header".to_string(),
            ));
        };
        let name = HeaderName::from_bytes(name.trim().as_bytes()).map_err(|error| {
            UpstreamError::Protocol(format!("invalid WebSocket header name: {error}"))
        })?;
        let value = HeaderValue::from_str(value.trim()).map_err(|error| {
            UpstreamError::Protocol(format!("invalid WebSocket header value: {error}"))
        })?;
        headers.push((name, value));
    }
    Ok(WebSocketHandshakeResponse {
        status,
        headers,
        remaining,
    })
}

async fn copy_websocket_tunnel<A, B>(
    client: &mut A,
    upstream: &mut B,
    idle_timeout: Option<Duration>,
) -> std::io::Result<()>
where
    A: AsyncRead + AsyncWrite + Unpin,
    B: AsyncRead + AsyncWrite + Unpin + ?Sized,
{
    let Some(idle_timeout) = idle_timeout.filter(|timeout| !timeout.is_zero()) else {
        tokio::io::copy_bidirectional(client, upstream).await?;
        return Ok(());
    };
    let mut client_open = true;
    let mut upstream_open = true;
    let mut client_buffer = [0_u8; 16 * 1024];
    let mut upstream_buffer = [0_u8; 16 * 1024];
    while client_open || upstream_open {
        tokio::select! {
            result = client.read(&mut client_buffer), if client_open => {
                let read = result?;
                if read == 0 {
                    client_open = false;
                    timeout_io(idle_timeout, upstream.shutdown()).await?;
                } else {
                    timeout_io(idle_timeout, upstream.write_all(&client_buffer[..read])).await?;
                }
            }
            result = upstream.read(&mut upstream_buffer), if upstream_open => {
                let read = result?;
                if read == 0 {
                    upstream_open = false;
                    timeout_io(idle_timeout, client.shutdown()).await?;
                } else {
                    timeout_io(idle_timeout, client.write_all(&upstream_buffer[..read])).await?;
                }
            }
            _ = tokio::time::sleep(idle_timeout) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "WebSocket tunnel idle timeout",
                ));
            }
        }
    }
    Ok(())
}

async fn timeout_io<T>(
    timeout: Duration,
    future: impl Future<Output = std::io::Result<T>>,
) -> std::io::Result<T> {
    tokio::time::timeout(timeout, future).await.map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "WebSocket tunnel write timeout",
        )
    })?
}

fn normalize_upstream_base(upstream: &str) -> String {
    let upstream = upstream.trim().trim_end_matches('/');
    if upstream.starts_with("http://")
        || upstream.starts_with("https://")
        || upstream.starts_with("ws://")
        || upstream.starts_with("wss://")
    {
        upstream.to_string()
    } else {
        format!("http://{upstream}")
    }
}

fn upstream_instance_key(service: &str, upstream: &str) -> String {
    format!("{service}@{}", normalize_upstream_base(upstream))
}

fn pick_weighted_instance(instances: &[ServiceInstance], cursor: u64) -> Option<&ServiceInstance> {
    const MAX_EFFECTIVE_WEIGHT: u64 = 10_000;
    let total = instances
        .iter()
        .map(|instance| u64::from(instance.weight.max(1)).min(MAX_EFFECTIVE_WEIGHT))
        .sum::<u64>();
    if total == 0 {
        return None;
    }
    let mut point = cursor % total;
    for instance in instances {
        let weight = u64::from(instance.weight.max(1)).min(MAX_EFFECTIVE_WEIGHT);
        if point < weight {
            return Some(instance);
        }
        point -= weight;
    }
    instances.last()
}

fn is_hop_by_hop_header(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
            | "host"
    )
}

#[cfg(test)]
mod tests {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Mutex,
    };

    use http::Request;
    use roze_config::{RegistryConfig, RegistryKind, RetryConfig};
    use roze_rpc::registry::{
        ConsulRegistry, EtcdRegistry, MemoryRegistry, Registry, ServiceInstance,
    };
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;

    fn gateway_config(upstream: String, method: &str) -> GatewayConfig {
        GatewayConfig {
            listen: None,
            services: vec![GatewayService {
                name: "catalog".to_string(),
                upstream,
                registry_name: None,
                instance_tags: BTreeMap::new(),
                timeout_ms: None,
                stream_idle_timeout_ms: None,
                max_stream_connections: None,
                outlier: None,
                health_check: None,
                tls: None,
            }],
            routes: vec![GatewayRoute {
                path: "/catalog".to_string(),
                service: "catalog".to_string(),
                methods: vec![method.to_string()],
                weight: 100,
                match_headers: BTreeMap::new(),
                match_cookies: BTreeMap::new(),
                traffic_percent: 100,
                mirror_service: None,
                mirror_percent: 0,
                instance_tags: BTreeMap::new(),
                middlewares: Vec::new(),
                timeout_ms: Some(1_000),
                stream_idle_timeout_ms: None,
                max_stream_connections: None,
                retries: Some(1),
                retry_backoff_ms: Some(0),
                rewrite: Some("/items".to_string()),
                fallback: None,
                rate_limit: None,
                breaker: None,
                shedding: None,
            }],
            middlewares: Vec::new(),
            timeout_ms: None,
            stream_idle_timeout_ms: None,
            max_stream_connections: None,
            request_body_limit_bytes: Some(1024),
            fallback: None,
            cors: None,
            trusted_proxy_cidrs: Vec::new(),
        }
    }

    async fn scripted_upstream(
        statuses: Vec<u16>,
    ) -> (String, Arc<AtomicUsize>, Arc<Mutex<Vec<String>>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind upstream");
        let addr = listener.local_addr().expect("upstream addr");
        let hits = Arc::new(AtomicUsize::new(0));
        let hits_task = hits.clone();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let requests_task = requests.clone();
        tokio::spawn(async move {
            for status in statuses {
                let (mut stream, _) = listener.accept().await.expect("accept upstream request");
                let mut request = vec![0_u8; 4096];
                let read = stream
                    .read(&mut request)
                    .await
                    .expect("read upstream request");
                requests_task
                    .lock()
                    .expect("request capture lock")
                    .push(String::from_utf8_lossy(&request[..read]).to_string());
                hits_task.fetch_add(1, Ordering::SeqCst);
                let reason = if status == 200 {
                    "OK"
                } else {
                    "Service Unavailable"
                };
                let response = format!(
                    "HTTP/1.1 {status} {reason}\r\ncontent-length: 2\r\nconnection: close\r\n\r\nok"
                );
                stream
                    .write_all(response.as_bytes())
                    .await
                    .expect("write upstream response");
            }
        });
        (format!("http://{addr}"), hits, requests)
    }

    async fn reliable_upstream() -> (String, Arc<AtomicUsize>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind reliable upstream");
        let addr = listener.local_addr().expect("reliable upstream addr");
        let hits = Arc::new(AtomicUsize::new(0));
        let hits_task = hits.clone();
        tokio::spawn(async move {
            loop {
                let (mut stream, _) = listener.accept().await.expect("accept upstream request");
                let hits = hits_task.clone();
                tokio::spawn(async move {
                    let mut request = vec![0_u8; 4096];
                    let _ = stream
                        .read(&mut request)
                        .await
                        .expect("read reliable upstream request");
                    hits.fetch_add(1, Ordering::SeqCst);
                    stream
                        .write_all(
                            b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\nok",
                        )
                        .await
                        .expect("write reliable upstream response");
                });
            }
        });
        (format!("http://{addr}"), hits)
    }

    async fn held_sse_upstream() -> (String, tokio::sync::oneshot::Sender<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind SSE upstream");
        let addr = listener.local_addr().expect("SSE upstream addr");
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept SSE request");
            let mut request = vec![0_u8; 4096];
            let _ = stream.read(&mut request).await.expect("read SSE request");
            let first = b"data: first\n\n";
            let headers = "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncache-control: no-cache\r\ntransfer-encoding: chunked\r\nconnection: close\r\n\r\n";
            stream
                .write_all(headers.as_bytes())
                .await
                .expect("write SSE headers");
            stream
                .write_all(format!("{:x}\r\n", first.len()).as_bytes())
                .await
                .expect("write SSE chunk size");
            stream.write_all(first).await.expect("write SSE event");
            stream
                .write_all(b"\r\n")
                .await
                .expect("write SSE chunk delimiter");
            let _ = release_rx.await;
            stream
                .write_all(b"0\r\n\r\n")
                .await
                .expect("finish SSE response");
        });
        (format!("http://{addr}"), release_tx)
    }

    #[tokio::test]
    async fn retries_idempotent_request_and_preserves_rewrite_suffix() {
        let (upstream, hits, requests) = scripted_upstream(vec![503, 200]).await;
        let runtime = build_router(gateway_config(upstream, "GET"), None);
        let request = Request::builder()
            .method(Method::GET)
            .uri("/catalog/42?expand=price")
            .body(rest::full_body(Bytes::new()))
            .expect("request");

        let response = runtime.runtime.handle(request).await;

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(hits.load(Ordering::SeqCst), 2);
        assert!(requests
            .lock()
            .expect("request capture lock")
            .iter()
            .all(|request| request.starts_with("GET /items/42?expand=price HTTP/1.1")));
    }

    #[tokio::test]
    async fn registry_retry_ejects_failed_instance_and_selects_fresh_target() {
        let (bad, bad_hits, _) = scripted_upstream(vec![503]).await;
        let (good, good_hits, _) = scripted_upstream(vec![200]).await;
        let registry = Arc::new(MemoryRegistry::default());
        registry
            .register(ServiceInstance::new("catalog", bad))
            .await
            .expect("register bad upstream");
        registry
            .register(ServiceInstance::new("catalog", good))
            .await
            .expect("register good upstream");
        let mut config = gateway_config(String::new(), "GET");
        config.services[0].registry_name = Some("catalog".to_string());
        config.services[0].outlier = Some(GatewayOutlierConfig {
            failure_threshold: 1,
            ejection_ms: 60_000,
        });
        let runtime = build_router_with_registry(config, None, Some(registry));

        let response = runtime
            .runtime
            .handle(
                Request::builder()
                    .uri("/catalog")
                    .body(rest::full_body(Bytes::new()))
                    .expect("request"),
            )
            .await;

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(bad_hits.load(Ordering::SeqCst), 1);
        assert_eq!(good_hits.load(Ordering::SeqCst), 1);
    }

    async fn wait_for_registry_instance(
        registry: &dyn Registry,
        name: &str,
        expected_addr: Option<&str>,
    ) {
        for _ in 0..100 {
            let instances = registry.discover(name).await.expect("discover registry");
            let matches = match expected_addr {
                Some(addr) => instances.iter().any(|instance| instance.addr == addr),
                None => instances.is_empty(),
            };
            if matches {
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("registry state did not converge for {name}");
    }

    async fn assert_external_registry_gateway_recovery(
        registry: Arc<dyn Registry>,
        registry_label: &str,
    ) {
        let (upstream, hits, _) = scripted_upstream(vec![200, 200]).await;
        let addr = upstream
            .strip_prefix("http://")
            .expect("scripted upstream URL");
        let suffix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system time")
            .as_nanos();
        let name = format!(
            "roze-gateway-{registry_label}-{}-{suffix}",
            std::process::id()
        );
        let mut instance = ServiceInstance::new(&name, addr);
        instance
            .metadata
            .insert("version".to_string(), "integration".to_string());
        registry
            .register(instance.clone())
            .await
            .expect("register gateway upstream");
        wait_for_registry_instance(registry.as_ref(), &name, Some(addr)).await;

        let mut config = gateway_config(String::new(), "GET");
        config.services[0].registry_name = Some(name.clone());
        config.services[0]
            .instance_tags
            .insert("version".to_string(), "integration".to_string());
        let runtime = build_router_with_registry(config, None, Some(registry.clone()));
        let request = || {
            Request::builder()
                .uri("/catalog")
                .body(rest::full_body(Bytes::new()))
                .expect("gateway request")
        };

        assert_eq!(
            runtime.runtime.handle(request()).await.status(),
            StatusCode::OK
        );
        registry
            .deregister(&name, addr)
            .await
            .expect("deregister gateway upstream");
        wait_for_registry_instance(registry.as_ref(), &name, None).await;
        assert_eq!(
            runtime.runtime.handle(request()).await.status(),
            StatusCode::BAD_GATEWAY
        );

        registry
            .register(instance)
            .await
            .expect("reregister gateway upstream");
        wait_for_registry_instance(registry.as_ref(), &name, Some(addr)).await;
        assert_eq!(
            runtime.runtime.handle(request()).await.status(),
            StatusCode::OK
        );
        registry
            .deregister(&name, addr)
            .await
            .expect("cleanup gateway upstream");
        assert_eq!(hits.load(Ordering::SeqCst), 2);
    }

    async fn assert_external_registry_restart_recovery(
        registry: Arc<dyn Registry>,
        registry_label: &str,
    ) {
        let duration = std::env::var("ROZE_GATEWAY_REGISTRY_RECOVERY_SECONDS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(30);
        assert!(
            duration >= 10,
            "registry recovery duration must be at least 10s"
        );

        let (upstream, hits) = reliable_upstream().await;
        let addr = upstream
            .strip_prefix("http://")
            .expect("reliable upstream URL");
        let suffix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system time")
            .as_nanos();
        let name = format!(
            "roze-gateway-restart-{registry_label}-{}-{suffix}",
            std::process::id()
        );
        let mut instance = ServiceInstance::new(&name, addr);
        instance
            .metadata
            .insert("version".to_string(), "restart-recovery".to_string());
        registry
            .register(instance)
            .await
            .expect("register restart recovery upstream");
        wait_for_registry_instance(registry.as_ref(), &name, Some(addr)).await;

        let mut config = gateway_config(String::new(), "GET");
        config.services[0].registry_name = Some(name.clone());
        config.services[0]
            .instance_tags
            .insert("version".to_string(), "restart-recovery".to_string());
        let runtime = build_router_with_registry(config, None, Some(registry.clone()));
        let started = std::time::Instant::now();
        let deadline = started + Duration::from_secs(duration);
        let mut attempts = 0_u64;
        let mut successful_routes = 0_u64;
        let mut disconnect_observations = 0_u64;
        let mut recoveries = 0_u64;
        let mut disconnected_at: Option<std::time::Instant> = None;
        let mut recovery_latencies = Vec::new();
        let mut route_latencies = Vec::new();
        let mut ready_written = false;

        while std::time::Instant::now() < deadline {
            let request_started = std::time::Instant::now();
            let response = runtime
                .runtime
                .handle(
                    Request::builder()
                        .uri("/catalog")
                        .body(rest::full_body(Bytes::new()))
                        .expect("restart recovery request"),
                )
                .await;
            route_latencies.push(request_started.elapsed().as_micros() as u64);
            attempts += 1;
            match response.status() {
                StatusCode::OK => {
                    successful_routes += 1;
                    if let Some(disconnected) = disconnected_at.take() {
                        recoveries += 1;
                        recovery_latencies.push(disconnected.elapsed().as_micros() as u64);
                    }
                    if !ready_written {
                        if let Ok(path) = std::env::var("ROZE_GATEWAY_REGISTRY_READY_FILE") {
                            std::fs::write(path, format!("{registry_label}\n"))
                                .expect("write registry recovery ready file");
                        }
                        ready_written = true;
                    }
                }
                StatusCode::BAD_GATEWAY => {
                    disconnect_observations += 1;
                    disconnected_at.get_or_insert_with(std::time::Instant::now);
                }
                status => panic!("unexpected Gateway status during registry recovery: {status}"),
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        route_latencies.sort_unstable();
        recovery_latencies.sort_unstable();
        let percentile = |values: &[u64], percent: usize| -> u64 {
            if values.is_empty() {
                return 0;
            }
            let rank = (values.len() * percent).div_ceil(100).max(1);
            values[rank.saturating_sub(1).min(values.len() - 1)]
        };
        let elapsed_ms = started.elapsed().as_millis() as u64;
        let p99_route_us = percentile(&route_latencies, 99);
        let p99_recovery_us = percentile(&recovery_latencies, 99);
        println!(
            "roze_gateway_registry_recovery registry={registry_label} elapsed_ms={elapsed_ms} \
             attempts={attempts} successful_routes={successful_routes} \
             disconnect_observations={disconnect_observations} recoveries={recoveries} \
             p99_route_us={p99_route_us} p99_recovery_us={p99_recovery_us}"
        );

        assert!(
            ready_written,
            "Gateway never reached its registered upstream"
        );
        assert!(
            disconnect_observations > 0,
            "external coordinator did not produce a visible registry outage"
        );
        assert!(
            recoveries > 0,
            "Gateway did not recover after registry restart"
        );
        assert!(successful_routes > recoveries);
        assert_eq!(hits.load(Ordering::SeqCst) as u64, successful_routes);
        registry
            .deregister(&name, addr)
            .await
            .expect("cleanup restart recovery upstream");
    }

    fn external_registry_config(kind: RegistryKind, endpoint: String) -> RegistryConfig {
        RegistryConfig {
            kind,
            endpoints: vec![endpoint],
            prefix: "/roze/gateway-integration".to_string(),
            ttl_seconds: 10,
            renew_interval_secs: 2,
            user: None,
            pass: None,
            cert_file: None,
            cert_key_file: None,
            ca_cert_file: None,
            insecure_skip_verify: false,
        }
    }

    #[tokio::test]
    #[ignore = "requires ROZE_TEST_ETCD_ENDPOINT"]
    async fn gateway_routes_and_recovers_through_real_etcd_registry() {
        let endpoint =
            std::env::var("ROZE_TEST_ETCD_ENDPOINT").expect("ROZE_TEST_ETCD_ENDPOINT is required");
        let registry = Arc::new(EtcdRegistry::new(&external_registry_config(
            RegistryKind::Etcd,
            endpoint,
        )));
        assert_external_registry_gateway_recovery(registry, "etcd").await;
    }

    #[tokio::test]
    #[ignore = "requires ROZE_TEST_CONSUL_ENDPOINT"]
    async fn gateway_routes_and_recovers_through_real_consul_registry() {
        let endpoint = std::env::var("ROZE_TEST_CONSUL_ENDPOINT")
            .expect("ROZE_TEST_CONSUL_ENDPOINT is required");
        let registry = Arc::new(ConsulRegistry::new(&external_registry_config(
            RegistryKind::Consul,
            endpoint,
        )));
        assert_external_registry_gateway_recovery(registry, "consul").await;
    }

    #[tokio::test]
    #[ignore = "requires externally coordinated Etcd restart"]
    async fn gateway_automatically_reregisters_after_real_etcd_restart() {
        let endpoint =
            std::env::var("ROZE_TEST_ETCD_ENDPOINT").expect("ROZE_TEST_ETCD_ENDPOINT is required");
        let registry = Arc::new(EtcdRegistry::new(&external_registry_config(
            RegistryKind::Etcd,
            endpoint,
        )));
        assert_external_registry_restart_recovery(registry, "etcd").await;
    }

    #[tokio::test]
    #[ignore = "requires externally coordinated Consul restart"]
    async fn gateway_automatically_reregisters_after_real_consul_restart() {
        let endpoint = std::env::var("ROZE_TEST_CONSUL_ENDPOINT")
            .expect("ROZE_TEST_CONSUL_ENDPOINT is required");
        let registry = Arc::new(ConsulRegistry::new(&external_registry_config(
            RegistryKind::Consul,
            endpoint,
        )));
        assert_external_registry_restart_recovery(registry, "consul").await;
    }

    #[tokio::test]
    async fn hot_reload_atomically_replaces_runtime_and_rejects_invalid_snapshot() {
        let (old_upstream, old_hits, _) = scripted_upstream(vec![200, 200, 200]).await;
        let (new_upstream, new_hits, _) = scripted_upstream(vec![200]).await;
        let old_config = gateway_config(old_upstream, "GET");
        let runtime = build_router(old_config.clone(), None);
        let request = || {
            Request::builder()
                .uri("/catalog")
                .body(rest::full_body(Bytes::new()))
                .expect("request")
        };
        assert_eq!(
            runtime.current.load_full().handle(request()).await.status(),
            StatusCode::OK
        );

        let mut invalid = old_config.clone();
        invalid.routes[0].service = "missing".to_string();
        assert!(runtime.reload(invalid, None, None, None, None).is_err());
        assert_eq!(
            runtime.current.load_full().handle(request()).await.status(),
            StatusCode::OK
        );

        let mut invalid_tls = old_config.clone();
        invalid_tls.services[0].tls = Some(roze_config::GatewayUpstreamTlsConfig {
            ca_files: vec!["missing-private-ca.pem".into()],
            ..Default::default()
        });
        assert!(runtime.reload(invalid_tls, None, None, None, None).is_err());
        assert_eq!(
            runtime.current.load_full().handle(request()).await.status(),
            StatusCode::OK
        );

        let updated = gateway_config(new_upstream, "GET");
        runtime
            .reload(updated, None, None, None, None)
            .expect("valid gateway reload");
        assert_eq!(
            runtime.current.load_full().handle(request()).await.status(),
            StatusCode::OK
        );
        assert_eq!(old_hits.load(Ordering::SeqCst), 3);
        assert_eq!(new_hits.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn hot_reload_preserves_open_breaker_state() {
        let (failed_upstream, failed_hits, _) = scripted_upstream(vec![503]).await;
        let (healthy_upstream, healthy_hits, _) = scripted_upstream(vec![200]).await;
        let mut config = gateway_config(failed_upstream, "GET");
        config.routes[0].retries = Some(0);
        config.routes[0].breaker = Some(BreakerConfig {
            failure_threshold: 1,
            reset_timeout_ms: 60_000,
        });
        let runtime = build_router(config.clone(), None);
        let request = || {
            Request::builder()
                .uri("/catalog")
                .body(rest::full_body(Bytes::new()))
                .expect("request")
        };
        assert_eq!(
            runtime.current.load_full().handle(request()).await.status(),
            StatusCode::SERVICE_UNAVAILABLE
        );

        config.services[0].upstream = healthy_upstream;
        runtime
            .reload(config, None, None, None, None)
            .expect("reload with healthy upstream");
        assert_eq!(
            runtime.current.load_full().handle(request()).await.status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(failed_hits.load(Ordering::SeqCst), 1);
        assert_eq!(healthy_hits.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn registry_tags_are_strict_and_do_not_fallback_to_static_upstream() {
        let (upstream, hits, _) = scripted_upstream(vec![200]).await;
        let registry = Arc::new(MemoryRegistry::default());
        let mut instance = ServiceInstance::new("catalog", upstream.clone());
        instance
            .metadata
            .insert("version".to_string(), "stable".to_string());
        registry
            .register(instance)
            .await
            .expect("register upstream");
        let mut config = gateway_config(upstream, "GET");
        config.services[0].registry_name = Some("catalog".to_string());
        config.routes[0]
            .instance_tags
            .insert("version".to_string(), "canary".to_string());
        let runtime = build_router_with_registry(config, None, Some(registry));

        let response = runtime
            .runtime
            .handle(
                Request::builder()
                    .uri("/catalog")
                    .body(rest::full_body(Bytes::new()))
                    .expect("request"),
            )
            .await;

        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        assert_eq!(hits.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn cors_preflight_is_validated_without_calling_upstream() {
        let (upstream, hits, _) = scripted_upstream(vec![200]).await;
        let mut config = gateway_config(upstream, "GET");
        config.cors = Some(GatewayCorsConfig {
            allow_origins: vec!["https://console.example".to_string()],
            allow_methods: vec!["GET".to_string()],
            allow_headers: vec!["authorization".to_string(), "x-tenant".to_string()],
            max_age_seconds: Some(600),
        });
        let runtime = build_router(config, None);

        let allowed = runtime
            .runtime
            .handle(
                Request::builder()
                    .method(Method::OPTIONS)
                    .uri("/not-a-business-route")
                    .header(header::ORIGIN, "https://console.example")
                    .header(header::ACCESS_CONTROL_REQUEST_METHOD, "GET")
                    .header(
                        header::ACCESS_CONTROL_REQUEST_HEADERS,
                        "Authorization, X-Tenant",
                    )
                    .body(rest::full_body(Bytes::new()))
                    .expect("allowed preflight"),
            )
            .await;
        let denied = runtime
            .runtime
            .handle(
                Request::builder()
                    .method(Method::OPTIONS)
                    .uri("/catalog")
                    .header(header::ORIGIN, "https://untrusted.example")
                    .header(header::ACCESS_CONTROL_REQUEST_METHOD, "GET")
                    .body(rest::full_body(Bytes::new()))
                    .expect("denied preflight"),
            )
            .await;

        assert_eq!(allowed.status(), StatusCode::NO_CONTENT);
        assert_eq!(
            allowed.headers().get(header::ACCESS_CONTROL_ALLOW_ORIGIN),
            Some(&HeaderValue::from_static("https://console.example"))
        );
        assert_eq!(
            allowed.headers().get(header::ACCESS_CONTROL_MAX_AGE),
            Some(&HeaderValue::from_static("600"))
        );
        assert_eq!(denied.status(), StatusCode::FORBIDDEN);
        assert_eq!(hits.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn cors_headers_are_applied_to_allowed_simple_response() {
        let (upstream, hits, _) = scripted_upstream(vec![200]).await;
        let mut config = gateway_config(upstream, "GET");
        config.routes[0].retries = Some(0);
        config.cors = Some(GatewayCorsConfig {
            allow_origins: vec!["https://console.example".to_string()],
            allow_methods: vec!["GET".to_string()],
            allow_headers: Vec::new(),
            max_age_seconds: None,
        });
        let runtime = build_router(config, None);

        let response = runtime
            .runtime
            .handle(
                Request::builder()
                    .uri("/catalog")
                    .header(header::ORIGIN, "https://console.example")
                    .body(rest::full_body(Bytes::new()))
                    .expect("request"),
            )
            .await;

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::ACCESS_CONTROL_ALLOW_ORIGIN),
            Some(&HeaderValue::from_static("https://console.example"))
        );
        assert_eq!(
            response.headers().get(header::VARY),
            Some(&HeaderValue::from_static("Origin"))
        );
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn sse_response_streams_without_waiting_for_upstream_completion() {
        let (upstream, release) = held_sse_upstream().await;
        let mut config = gateway_config(upstream, "GET");
        config.routes[0].timeout_ms = Some(30);
        config.routes[0].stream_idle_timeout_ms = Some(500);
        config.routes[0].max_stream_connections = Some(1);
        config.routes[0].retries = Some(0);
        let runtime = build_router(config, None);

        let mut response = tokio::time::timeout(
            Duration::from_millis(100),
            runtime.runtime.handle(
                Request::builder()
                    .uri("/catalog")
                    .body(rest::full_body(Bytes::new()))
                    .expect("SSE request"),
            ),
        )
        .await
        .expect("SSE headers should not wait for body completion");

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE),
            Some(&HeaderValue::from_static("text/event-stream"))
        );
        let first = tokio::time::timeout(Duration::from_millis(100), response.body_mut().frame())
            .await
            .expect("first SSE event timeout")
            .expect("first SSE frame")
            .expect("first SSE frame result")
            .into_data()
            .expect("SSE data frame");
        assert_eq!(first, Bytes::from_static(b"data: first\n\n"));

        tokio::time::sleep(Duration::from_millis(50)).await;
        release.send(()).expect("release SSE upstream");
        response
            .body_mut()
            .collect()
            .await
            .expect("SSE remains alive beyond request header timeout");
        drop(response);

        let metrics = roze_metrics::http_metrics();
        assert!(metrics.contains("roze_gateway_stream_connection_events_total"));
        assert!(metrics.contains("protocol=\"sse\""));
        assert!(metrics.contains("outcome=\"opened\""));
        assert!(metrics.contains("outcome=\"closed\""));
    }

    #[tokio::test]
    async fn sse_idle_timeout_terminates_stalled_body() {
        let body = SseBody::new(
            futures_util::stream::pending::<Result<Bytes, reqwest::Error>>(),
            Some(Duration::from_millis(5)),
            None,
        )
        .boxed();

        let error = body
            .collect()
            .await
            .expect_err("stalled SSE should time out");

        assert!(error.to_string().contains("SSE stream idle timeout"));
    }

    #[test]
    fn stream_connection_limit_releases_capacity_with_body_lifecycle() {
        let mut config = gateway_config("http://127.0.0.1:1".to_string(), "GET");
        config.routes[0].max_stream_connections = Some(1);
        let runtime = build_router(config, None);
        let route = &runtime.runtime.routes[0];
        let service = runtime
            .runtime
            .services
            .get("catalog")
            .expect("catalog service");

        let first = runtime
            .runtime
            .try_acquire_stream_connection(route, service, "sse")
            .expect("first stream permit");
        assert!(matches!(
            runtime
                .runtime
                .try_acquire_stream_connection(route, service, "sse"),
            Err(UpstreamError::StreamCapacity)
        ));
        drop(first);
        assert!(runtime
            .runtime
            .try_acquire_stream_connection(route, service, "sse")
            .is_ok());
    }

    #[tokio::test]
    async fn active_health_check_removes_unhealthy_registry_instance() {
        let (bad, bad_hits, _) = scripted_upstream(vec![503]).await;
        let (good, good_hits, _) = scripted_upstream(vec![200, 200]).await;
        let registry = Arc::new(MemoryRegistry::default());
        registry
            .register(ServiceInstance::new("catalog", bad))
            .await
            .expect("register bad upstream");
        registry
            .register(ServiceInstance::new("catalog", good))
            .await
            .expect("register good upstream");
        let mut config = gateway_config(String::new(), "GET");
        config.services[0].registry_name = Some("catalog".to_string());
        config.services[0].health_check = Some(GatewayHealthCheckConfig {
            path: "/healthz".to_string(),
            interval_ms: 60_000,
            timeout_ms: 1_000,
            unhealthy_threshold: 1,
            healthy_threshold: 1,
            expected_status: 200,
        });
        config.routes[0].retries = Some(0);
        let runtime = build_router_with_registry(config, None, Some(registry));
        for _ in 0..100 {
            if bad_hits.load(Ordering::SeqCst) == 1 && good_hits.load(Ordering::SeqCst) == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }

        let response = runtime
            .runtime
            .handle(
                Request::builder()
                    .uri("/catalog")
                    .body(rest::full_body(Bytes::new()))
                    .expect("request"),
            )
            .await;

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(bad_hits.load(Ordering::SeqCst), 1);
        assert_eq!(good_hits.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn weighted_instance_selection_is_bounded_and_deterministic() {
        let mut low = ServiceInstance::new("catalog", "127.0.0.1:1");
        low.weight = 1;
        let mut high = ServiceInstance::new("catalog", "127.0.0.1:2");
        high.weight = 3;
        let instances = vec![low, high];
        let selected = (0..4)
            .map(|cursor| {
                pick_weighted_instance(&instances, cursor)
                    .expect("weighted instance")
                    .addr
                    .as_str()
            })
            .collect::<Vec<_>>();

        assert_eq!(
            selected,
            vec!["127.0.0.1:1", "127.0.0.1:2", "127.0.0.1:2", "127.0.0.1:2"]
        );
    }

    #[test]
    fn route_selection_supports_header_cookie_and_percentage_policies() {
        let mut config = gateway_config("http://127.0.0.1:1".to_string(), "GET");
        let mut canary = config.routes[0].clone();
        canary.match_headers = BTreeMap::from([("x-release".to_string(), "canary".to_string())]);
        canary.match_cookies = BTreeMap::from([("experiment".to_string(), "b".to_string())]);
        canary.instance_tags = BTreeMap::from([("version".to_string(), "canary".to_string())]);
        config.routes.insert(0, canary);
        let runtime = build_router(config, None);
        let request = Request::builder()
            .uri("/catalog")
            .header("x-release", "canary")
            .header(header::COOKIE, "experiment=b")
            .body(rest::full_body(Bytes::new()))
            .expect("request");
        let selected = runtime
            .runtime
            .select_route(&request)
            .expect("selected route");
        assert_eq!(
            selected.instance_tags.get("version").map(String::as_str),
            Some("canary")
        );
        assert!(stable_bucket("stable-user") < 100);
    }

    #[test]
    fn gateway_validation_rejects_unknown_mirror_and_invalid_percentages() {
        let mut config = gateway_config("http://127.0.0.1:1".to_string(), "GET");
        config.routes[0].mirror_service = Some("shadow".to_string());
        assert!(validate_gateway_config(&config).is_err());
        config.routes[0].mirror_service = None;
        config.routes[0].traffic_percent = 101;
        assert!(validate_gateway_config(&config).is_err());
    }

    #[tokio::test]
    async fn does_not_retry_non_idempotent_request() {
        let (upstream, hits, _) = scripted_upstream(vec![503]).await;
        let runtime = build_router(gateway_config(upstream, "POST"), None);
        let request = Request::builder()
            .method(Method::POST)
            .uri("/catalog")
            .body(rest::full_body(Bytes::from_static(b"{}")))
            .expect("request");

        let response = runtime.runtime.handle(request).await;

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn breaker_opens_after_final_upstream_failure() {
        let (upstream, hits, _) = scripted_upstream(vec![503]).await;
        let mut config = gateway_config(upstream, "GET");
        config.routes[0].retries = Some(0);
        config.routes[0].breaker = Some(BreakerConfig {
            failure_threshold: 1,
            reset_timeout_ms: 60_000,
        });
        let runtime = build_router(config, None);

        let first = runtime
            .runtime
            .handle(
                Request::builder()
                    .uri("/catalog")
                    .body(rest::full_body(Bytes::new()))
                    .expect("first request"),
            )
            .await;
        let second = runtime
            .runtime
            .handle(
                Request::builder()
                    .uri("/catalog")
                    .body(rest::full_body(Bytes::new()))
                    .expect("second request"),
            )
            .await;

        assert_eq!(first.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(second.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn rate_limit_rejects_before_upstream_call() {
        let (upstream, hits, _) = scripted_upstream(vec![200]).await;
        let mut config = gateway_config(upstream, "GET");
        config.routes[0].rate_limit = Some(RateLimitConfig {
            burst: 1,
            refill_ms: 60_000,
            tokens_per_refill: 1,
            key: Default::default(),
        });
        config.routes[0].fallback = Some(GatewayFallbackResponse {
            status: 598,
            body: Some(serde_json::json!({"message": "degraded"})),
            headers: BTreeMap::new(),
        });
        let runtime = build_router(config, None);

        let first = runtime
            .runtime
            .handle(
                Request::builder()
                    .uri("/catalog")
                    .body(rest::full_body(Bytes::new()))
                    .expect("first request"),
            )
            .await;
        let second = runtime
            .runtime
            .handle(
                Request::builder()
                    .uri("/catalog")
                    .body(rest::full_body(Bytes::new()))
                    .expect("second request"),
            )
            .await;

        assert_eq!(first.status(), StatusCode::OK);
        assert_eq!(second.status(), StatusCode::TOO_MANY_REQUESTS);
        let retry_after = second
            .headers()
            .get(header::RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok())
            .expect("Retry-After seconds");
        assert!((1..=60).contains(&retry_after));
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn hot_reload_preserves_rate_limit_state_when_store_config_is_unchanged() {
        let (upstream, hits, _) = scripted_upstream(vec![200]).await;
        let mut config = gateway_config(upstream, "GET");
        config.routes[0].rate_limit = Some(RateLimitConfig {
            burst: 1,
            refill_ms: 60_000,
            tokens_per_refill: 1,
            key: Default::default(),
        });
        let runtime = build_router(config.clone(), None);

        let first = runtime
            .runtime
            .handle(
                Request::builder()
                    .uri("/catalog")
                    .body(rest::full_body(Bytes::new()))
                    .expect("first request"),
            )
            .await;
        runtime
            .reload(config, None, None, None, None)
            .expect("reload unchanged limiter store");
        let second = runtime
            .current
            .load_full()
            .handle(
                Request::builder()
                    .uri("/catalog")
                    .body(rest::full_body(Bytes::new()))
                    .expect("second request"),
            )
            .await;

        assert_eq!(first.status(), StatusCode::OK);
        assert_eq!(second.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn timeout_uses_configured_fallback_response() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind stalled upstream");
        let addr = listener.local_addr().expect("stalled upstream addr");
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept stalled request");
            let mut request = vec![0_u8; 4096];
            let _ = stream
                .read(&mut request)
                .await
                .expect("read stalled request");
            tokio::time::sleep(Duration::from_millis(100)).await;
        });
        let mut config = gateway_config(format!("http://{addr}"), "GET");
        config.routes[0].timeout_ms = Some(5);
        config.routes[0].retries = Some(0);
        config.routes[0].fallback = Some(GatewayFallbackResponse {
            status: 598,
            body: Some(serde_json::json!({"code": 598, "message": "degraded"})),
            headers: BTreeMap::from([("x-roze-fallback".to_string(), "gateway".to_string())]),
        });
        let runtime = build_router(config, None);

        let response = runtime
            .runtime
            .handle(
                Request::builder()
                    .uri("/catalog")
                    .body(rest::full_body(Bytes::new()))
                    .expect("request"),
            )
            .await;

        assert_eq!(response.status().as_u16(), 598);
        assert_eq!(
            response
                .headers()
                .get("x-roze-fallback")
                .and_then(|value| value.to_str().ok()),
            Some("gateway")
        );
    }

    #[tokio::test]
    async fn shedding_holds_capacity_until_upstream_response_finishes() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind blocked upstream");
        let addr = listener.local_addr().expect("blocked upstream addr");
        let (accepted_tx, accepted_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept blocked request");
            let mut request = vec![0_u8; 4096];
            let _ = stream
                .read(&mut request)
                .await
                .expect("read blocked request");
            let _ = accepted_tx.send(());
            let _ = release_rx.await;
            stream
                .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\nok")
                .await
                .expect("write blocked response");
        });
        let mut config = gateway_config(format!("http://{addr}"), "GET");
        config.routes[0].retries = Some(0);
        config.routes[0].shedding = Some(SheddingConfig {
            concurrency: 1,
            ..Default::default()
        });
        let runtime = build_router(config, None);
        let first_runtime = runtime.runtime.clone();
        let first = tokio::spawn(async move {
            first_runtime
                .handle(
                    Request::builder()
                        .uri("/catalog")
                        .body(rest::full_body(Bytes::new()))
                        .expect("first request"),
                )
                .await
        });
        accepted_rx.await.expect("first request reached upstream");

        assert_eq!(
            runtime
                .runtime
                .shedders
                .snapshot("catalog:gateway:get:/catalog")
                .expect("shedding snapshot")
                .in_flight,
            1
        );
        let second = runtime
            .runtime
            .handle(
                Request::builder()
                    .uri("/catalog")
                    .body(rest::full_body(Bytes::new()))
                    .expect("second request"),
            )
            .await;

        assert_eq!(second.status(), StatusCode::TOO_MANY_REQUESTS);
        let _ = release_tx.send(());
        assert_eq!(
            first.await.expect("first gateway task").status(),
            StatusCode::OK
        );
    }

    #[test]
    fn governance_route_overrides_global_policy() {
        let mut governance = GovernanceConfig {
            retry: Some(RetryConfig {
                max_attempts: 2,
                backoff_ms: 100,
                max_backoff_ms: 500,
                budget_percent: Some(10),
            }),
            ..Default::default()
        };
        governance.routes.insert(
            "/catalog".to_string(),
            RouteGovernanceConfig {
                retry: Some(RetryConfig {
                    max_attempts: 4,
                    backoff_ms: 10,
                    max_backoff_ms: 40,
                    budget_percent: Some(20),
                }),
                ..Default::default()
            },
        );
        let config = gateway_config("http://127.0.0.1:1".to_string(), "GET");
        let mut route = config.routes[0].clone();
        route.retries = None;
        route.retry_backoff_ms = None;

        let compiled = compile_routes(vec![route], Some(&governance));

        assert_eq!(compiled[0].retries, 3);
        assert_eq!(compiled[0].retry_backoff, Duration::from_millis(10));
        assert_eq!(compiled[0].retry_max_backoff, Duration::from_millis(40));
        assert_eq!(compiled[0].retry_budget_percent, Some(20));
    }

    #[test]
    fn api_key_auth_injects_standard_context_headers() {
        let config = roze_auth::ApiKeyConfig {
            header: "x-api-key".to_string(),
            keys: vec![roze_auth::ApiKeyCredential {
                key: "secret".to_string(),
                subject: "worker-1".to_string(),
                roles: vec!["internal".to_string()],
                tenant: Some("tenant-1".to_string()),
                permissions: vec!["catalog:read".to_string()],
                scopes: vec!["catalog.read".to_string()],
            }],
        };
        let mut headers = HeaderMap::new();
        headers.insert("x-api-key", HeaderValue::from_static("secret"));

        let principal = validate_request_auth(&headers, AuthPolicy::ApiKey, None, Some(&config))
            .expect("valid API key");
        inject_auth_context_headers(&mut headers, &principal);

        assert_eq!(
            headers
                .get(roze_context::SUBJECT_HEADER)
                .and_then(|value| value.to_str().ok()),
            Some("worker-1")
        );
        assert_eq!(
            headers
                .get(roze_context::TENANT_HEADER)
                .and_then(|value| value.to_str().ok()),
            Some("tenant-1")
        );
        assert_eq!(
            headers
                .get(roze_context::ROLES_HEADER)
                .and_then(|value| value.to_str().ok()),
            Some("internal")
        );
        assert_eq!(
            headers
                .get(roze_context::PERMISSIONS_HEADER)
                .and_then(|value| value.to_str().ok()),
            Some("catalog:read")
        );
        assert_eq!(
            headers
                .get(roze_context::SCOPE_HEADER)
                .and_then(|value| value.to_str().ok()),
            Some("catalog.read")
        );
    }

    #[test]
    fn gateway_removes_untrusted_identity_headers_before_context_creation() {
        let mut headers = HeaderMap::new();
        for name in [
            roze_context::SUBJECT_HEADER,
            roze_context::TENANT_HEADER,
            roze_context::ROLES_HEADER,
            roze_context::PERMISSIONS_HEADER,
            roze_context::SCOPE_HEADER,
            roze_context::HULA_UID_HEADER,
            roze_context::HULA_TENANT_ID_HEADER,
            roze_context::HULA_ROLE_HEADER,
            roze_context::HULA_SCOPE_HEADER,
        ] {
            headers.insert(name, HeaderValue::from_static("spoofed"));
        }

        clear_untrusted_auth_context_headers(&mut headers);

        for name in [
            roze_context::SUBJECT_HEADER,
            roze_context::TENANT_HEADER,
            roze_context::ROLES_HEADER,
            roze_context::PERMISSIONS_HEADER,
            roze_context::SCOPE_HEADER,
            roze_context::HULA_UID_HEADER,
            roze_context::HULA_TENANT_ID_HEADER,
            roze_context::HULA_ROLE_HEADER,
            roze_context::HULA_SCOPE_HEADER,
        ] {
            assert!(headers.get(name).is_none(), "header {name} was not removed");
        }
    }

    #[test]
    fn jwt_auth_preserves_permissions_and_scopes_for_downstream_context() {
        let config = JwtConfig {
            jwt_keys: vec![roze_jwt::JwtKey {
                id: "active".into(),
                secret: "gateway-secret".into(),
            }],
            jwt_active_key_id: "active".into(),
            jwt_issuer: "https://issuer.example".into(),
            jwt_audience: "catalog-api".into(),
            jwt_expiration_secs: 60,
            jwt_clock_skew_secs: 0,
            revoked_token_ids: Vec::new(),
        };
        let token = roze_jwt::issue_token(
            &roze_jwt::Claims {
                sub: "worker-1".into(),
                roles: vec!["internal".into()],
                tenant: Some("tenant-1".into()),
                permissions: vec!["catalog:read".into()],
                scopes: vec!["catalog.read".into()],
                iss: String::new(),
                aud: String::new(),
                jti: "token-1".into(),
                iat: 0,
                exp: 0,
            },
            &config,
        )
        .expect("issue JWT");
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {token}")).expect("authorization header"),
        );

        let principal = validate_request_auth(&headers, AuthPolicy::Jwt, Some(&config), None)
            .expect("valid JWT");
        assert_eq!(principal.permissions, ["catalog:read"]);
        assert_eq!(principal.scopes, ["catalog.read"]);

        inject_auth_context_headers(&mut headers, &principal);
        let propagation = headers
            .iter()
            .filter_map(|(name, value)| {
                Some((name.as_str().to_string(), value.to_str().ok()?.to_string()))
            })
            .collect::<BTreeMap<_, _>>();
        let context = Context::from_propagation_headers(&propagation);
        assert!(context.has_permissions(["catalog:read"]));
        assert_eq!(
            context.metadata_value("scope").as_deref(),
            Some("catalog.read")
        );
    }

    #[test]
    fn websocket_handshake_is_rewritten_and_cryptographically_validated() {
        let request = Request::builder()
            .method(Method::GET)
            .uri("/socket?tenant=1")
            .header(header::CONNECTION, "keep-alive, Upgrade")
            .header(header::UPGRADE, "websocket")
            .header(header::SEC_WEBSOCKET_VERSION, "13")
            .header(header::SEC_WEBSOCKET_KEY, "dGhlIHNhbXBsZSBub25jZQ==")
            .header(header::AUTHORIZATION, "Bearer token")
            .body(rest::empty_body())
            .expect("WebSocket request");
        let target = WebSocketTarget {
            addr: "127.0.0.1:80".to_string(),
            authority: "upstream.internal".to_string(),
            path_and_query: "/events?tenant=1".to_string(),
            server_name: "upstream.internal".to_string(),
            secure: false,
        };
        let handshake_request =
            build_websocket_handshake_request(&request, &target).expect("handshake request");
        let accept = expected_websocket_accept(request.headers()).expect("WebSocket accept");
        let handshake = WebSocketHandshakeResponse {
            status: StatusCode::SWITCHING_PROTOCOLS,
            headers: vec![
                (header::CONNECTION, HeaderValue::from_static("Upgrade")),
                (header::UPGRADE, HeaderValue::from_static("websocket")),
                (
                    header::SEC_WEBSOCKET_ACCEPT,
                    HeaderValue::from_str(&accept).expect("accept header"),
                ),
            ],
            remaining: Vec::new(),
        };

        assert!(handshake_request.starts_with("GET /events?tenant=1 HTTP/1.1\r\n"));
        assert!(
            handshake_request.contains("host: upstream.internal\r\n")
                || handshake_request.contains("Host: upstream.internal\r\n")
        );
        assert!(handshake_request.contains("authorization: Bearer token\r\n"));
        assert_eq!(accept, "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
        validate_websocket_handshake(request.headers(), &handshake, &accept)
            .expect("valid upstream handshake");
    }

    #[tokio::test]
    async fn websocket_tunnel_copies_both_directions_and_enforces_idle_timeout() {
        let (mut gateway_client, mut client_peer) = tokio::io::duplex(1024);
        let (mut gateway_upstream, mut upstream_peer) = tokio::io::duplex(1024);
        let tunnel = tokio::spawn(async move {
            copy_websocket_tunnel(
                &mut gateway_client,
                &mut gateway_upstream,
                Some(Duration::from_millis(200)),
            )
            .await
        });

        client_peer
            .write_all(b"client-frame")
            .await
            .expect("client write");
        let mut client_frame = [0_u8; 12];
        upstream_peer
            .read_exact(&mut client_frame)
            .await
            .expect("upstream read");
        assert_eq!(&client_frame, b"client-frame");
        upstream_peer
            .write_all(b"server-frame")
            .await
            .expect("upstream write");
        let mut server_frame = [0_u8; 12];
        client_peer
            .read_exact(&mut server_frame)
            .await
            .expect("client read");
        assert_eq!(&server_frame, b"server-frame");
        tunnel.abort();

        let (mut stalled_client, _client_peer) = tokio::io::duplex(64);
        let (mut stalled_upstream, _upstream_peer) = tokio::io::duplex(64);
        let error = copy_websocket_tunnel(
            &mut stalled_client,
            &mut stalled_upstream,
            Some(Duration::from_millis(5)),
        )
        .await
        .expect_err("idle WebSocket tunnel should time out");
        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
    }

    #[tokio::test]
    async fn wss_target_uses_tls_sni_and_never_downgrades_to_plaintext() {
        let target = websocket_target("wss://localhost:9443/socket?tenant=1")
            .expect("secure WebSocket target");
        assert!(target.secure);
        assert_eq!(target.addr, "localhost:9443");
        assert_eq!(target.authority, "localhost:9443");
        assert_eq!(target.server_name, "localhost");
        assert_eq!(target.path_and_query, "/socket?tenant=1");
        let ipv6 = websocket_target("wss://[::1]:9443/socket").expect("IPv6 target");
        assert_eq!(ipv6.addr, "[::1]:9443");
        assert_eq!(ipv6.authority, "[::1]:9443");

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind plaintext upstream");
        let addr = listener.local_addr().expect("plaintext addr");
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept TLS attempt");
            stream
                .write_all(b"HTTP/1.1 101 Switching Protocols\r\n\r\n")
                .await
                .expect("write plaintext response");
        });
        let stream = TcpStream::connect(addr)
            .await
            .expect("connect plaintext upstream");
        let config = build_websocket_tls_config();
        let local_target = WebSocketTarget {
            addr: addr.to_string(),
            authority: "localhost".to_string(),
            path_and_query: "/socket".to_string(),
            server_name: "localhost".to_string(),
            secure: true,
        };

        let error = connect_websocket_tls(stream, &local_target, config)
            .await
            .expect_err("plaintext response must not be accepted as wss");
        assert!(error.to_string().contains("TLS handshake failed"));
    }

    #[tokio::test]
    async fn private_ca_and_client_certificate_complete_mutual_tls_handshake() {
        use rcgen::{BasicConstraints, CertificateParams, ExtendedKeyUsagePurpose, IsCa, KeyPair};

        let mut ca_params = CertificateParams::new(Vec::<String>::new()).expect("CA params");
        ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        let ca_key = KeyPair::generate().expect("CA key");
        let ca = ca_params.self_signed(&ca_key).expect("CA certificate");

        let mut server_params =
            CertificateParams::new(vec!["localhost".to_string()]).expect("server params");
        server_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        let server_key = KeyPair::generate().expect("server key");
        let server = server_params
            .signed_by(&server_key, &ca, &ca_key)
            .expect("server certificate");

        let mut client_params =
            CertificateParams::new(vec!["roze-client".to_string()]).expect("client params");
        client_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
        let client_key = KeyPair::generate().expect("client key");
        let client = client_params
            .signed_by(&client_key, &ca, &ca_key)
            .expect("client certificate");

        let directory = tempfile::tempdir().expect("TLS fixture directory");
        let ca_path = directory.path().join("ca.pem");
        let client_cert_path = directory.path().join("client.pem");
        let client_key_path = directory.path().join("client.key");
        std::fs::write(&ca_path, ca.pem()).expect("write CA");
        std::fs::write(&client_cert_path, client.pem()).expect("write client certificate");
        std::fs::write(&client_key_path, client_key.serialize_pem()).expect("write client key");

        let mut gateway = gateway_config("wss://localhost:1".to_string(), "GET");
        gateway.services[0].tls = Some(roze_config::GatewayUpstreamTlsConfig {
            ca_files: vec![ca_path],
            client_cert_file: Some(client_cert_path),
            client_key_file: Some(client_key_path),
            server_name: Some("localhost".to_string()),
        });
        let profiles =
            build_service_websocket_tls_configs(&gateway.services).expect("gateway mTLS profile");
        let client_config = profiles
            .get("catalog")
            .expect("catalog TLS profile")
            .clone();

        let mut client_roots = rustls::RootCertStore::empty();
        client_roots.add(ca.der().clone()).expect("client CA root");
        let verifier = rustls::server::WebPkiClientVerifier::builder_with_provider(
            Arc::new(client_roots),
            Arc::new(rustls::crypto::ring::default_provider()),
        )
        .build()
        .expect("client certificate verifier");
        let server_config = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .expect("server protocol versions")
        .with_client_cert_verifier(verifier)
        .with_single_cert(
            vec![server.der().clone()],
            rustls::pki_types::PrivatePkcs8KeyDer::from(server_key.serialize_der()).into(),
        )
        .expect("server TLS config");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mTLS server");
        let addr = listener.local_addr().expect("mTLS server addr");
        let server_task = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept mTLS client");
            let mut stream = tokio_rustls::TlsAcceptor::from(Arc::new(server_config))
                .accept(stream)
                .await
                .expect("accept verified mTLS client");
            let mut message = [0_u8; 4];
            stream
                .read_exact(&mut message)
                .await
                .expect("read mTLS message");
            assert_eq!(&message, b"ping");
            stream
                .write_all(b"pong")
                .await
                .expect("write mTLS response");
        });

        let tcp = TcpStream::connect(addr).await.expect("connect mTLS server");
        let target = WebSocketTarget {
            addr: addr.to_string(),
            authority: "localhost".to_string(),
            path_and_query: "/socket".to_string(),
            server_name: "localhost".to_string(),
            secure: true,
        };
        let mut tls = connect_websocket_tls(tcp, &target, client_config)
            .await
            .expect("mutual TLS handshake");
        tls.write_all(b"ping").await.expect("write through mTLS");
        let mut response = [0_u8; 4];
        tls.read_exact(&mut response)
            .await
            .expect("read through mTLS");
        assert_eq!(&response, b"pong");
        server_task.await.expect("mTLS server task");
    }
}
