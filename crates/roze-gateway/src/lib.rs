use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};

use poem::{
    endpoint,
    http::{header, HeaderName, HeaderValue, Method, StatusCode},
    middleware::Cors,
    Endpoint, Request, Response, Result, Route,
};
use reqwest::Response as ReqwestResponse;
use serde_json::Value;
use tokio::sync::Mutex;
use tracing::warn;

use roze_config::{
    BreakerConfig, GatewayConfig, GatewayFallbackResponse, GatewayRoute, GatewayService, RateLimitConfig,
};
use roze_jwt::{verify_token, JwtConfig};
use roze_trace::TRACE_ID_HEADER;

pub const REQUEST_ID_HEADER: &str = "x-request-id";

#[derive(Clone)]
struct GatewayRuntime {
    routes: Vec<CompiledRoute>,
    services: HashMap<String, ServiceEndpoint>,
    global_timeout_ms: Option<u64>,
    global_fallback: Option<GatewayFallbackResponse>,
    request_body_limit_bytes: Option<usize>,
    global_middlewares: Vec<String>,
    client: reqwest::Client,
    jwt: Option<JwtConfig>,
    rate_limit_states: Arc<Mutex<HashMap<String, TokenBucketState>>>,
    breaker_states: Arc<Mutex<HashMap<String, CircuitState>>>,
}

#[derive(Debug, Clone)]
struct ServiceEndpoint {
    upstream: String,
    timeout_ms: Option<u64>,
}

#[derive(Clone)]
struct CompiledRoute {
    path: String,
    service: String,
    methods: Vec<Method>,
    timeout_ms: Option<u64>,
    rewrite: Option<String>,
    fallback: Option<GatewayFallbackResponse>,
    rate_limit: Option<RateLimitConfig>,
    breaker: Option<BreakerConfig>,
    middlewares: Vec<String>,
}

#[derive(Debug, Clone, Copy)]
struct TokenBucketState {
    tokens: f64,
    last_refill: Instant,
}

#[derive(Debug, Clone, Copy)]
struct CircuitState {
    failure_count: u32,
    open_until: Option<Instant>,
}

impl Default for GatewayRuntime {
    fn default() -> Self {
        Self {
            routes: Vec::new(),
            services: HashMap::new(),
            global_timeout_ms: None,
            global_fallback: None,
            request_body_limit_bytes: None,
            global_middlewares: Vec::new(),
            client: reqwest::Client::new(),
            jwt: None,
            rate_limit_states: Arc::new(Mutex::new(HashMap::new())),
            breaker_states: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

pub fn build_router(config: GatewayConfig, jwt: Option<JwtConfig>) -> Route {
    let mut runtime = GatewayRuntime {
        global_timeout_ms: config.timeout_ms,
        global_fallback: config.fallback,
        request_body_limit_bytes: config.request_body_limit_bytes,
        global_middlewares: normalize_middlewares(config.middlewares),
        jwt,
        ..Default::default()
    };

    for service in config.services {
        runtime
            .services
            .insert(service.name.clone(), ServiceEndpoint::from(service));
    }

    let mut routes = compile_routes(config.routes);
    routes.sort_by(|left, right| right.path.len().cmp(&left.path.len()));
    if routes.is_empty() && !runtime.services.is_empty() {
        if let Some(service) = first_service_name(runtime.services.keys().collect::<Vec<_>>()) {
            warn!(
                service = %service,
                "gateway has no routes, fallback route created for service"
            );
            routes.push(CompiledRoute {
                path: "/".to_string(),
                service: service.to_string(),
                methods: Vec::new(),
                timeout_ms: runtime.global_timeout_ms,
                rewrite: None,
                fallback: runtime.global_fallback.clone(),
                rate_limit: None,
                breaker: None,
                middlewares: Vec::new(),
            });
        }
    }
    runtime.routes = routes;

    let runtime = Arc::new(runtime);
    let handler = endpoint::make(move |req: Request| {
        let runtime = runtime.clone();
        async move { runtime.handle_request(req).await }
    });

    let mut app = Route::new().at("/*path", handler);

    if let Some(cors) = config.cors {
        let mut cors = build_cors(cors.allow_origins, cors.allow_methods, cors.allow_headers, cors.max_age_seconds);
        app = app.with(cors);
    }

    app
}

impl GatewayRuntime {
    async fn handle_request(self: Arc<Self>, req: Request) -> Result<Response> {
        let request_path = req.uri().path().to_string();
        let request_method = req.method().clone();

        let mut req = req;
        let request_id = ensure_header(&mut req, REQUEST_ID_HEADER);
        let trace_id = ensure_header(&mut req, TRACE_ID_HEADER);

        let span = tracing::info_span!(
            "gateway.request",
            method = %request_method,
            path = %request_path,
            request_id = %request_id,
            trace_id = %trace_id,
        );
        let _enter = span.enter();

        let Some(route) = self.select_route(&request_path, &request_method) else {
            warn!(
                path = %request_path,
                method = %request_method,
                event = "gateway.no_route",
                "no route matched"
            );
            return Ok(build_fallback(
                self.global_fallback.as_ref(),
                StatusCode::NOT_FOUND,
                "gateway route not found",
            ));
        };

        let _ = self.inject_trace_headers(&mut req);

        if !route.method_allowed(&request_method) {
            warn!(
                path = %request_path,
                method = %request_method,
                route = %route.path,
                event = "gateway.method_not_allowed",
                "method blocked by route config"
            );
            return Ok(build_fallback(
                route.fallback.as_ref().or(self.global_fallback.as_ref()),
                StatusCode::METHOD_NOT_ALLOWED,
                "method not allowed",
            ));
        }

        // fixed middleware order: trace -> auth -> rate -> breaker -> timeout -> upstream
        if self.requires_auth(&route) {
            if let Err(err) = validate_request_auth(&req, self.jwt.as_ref()) {
                warn!(
                    error = %err,
                    event = "gateway.auth_failed",
                    "auth validation failed"
                );
                return Ok(build_fallback(
                    route.fallback.as_ref().or(self.global_fallback.as_ref()),
                    StatusCode::UNAUTHORIZED,
                    "unauthorized",
                ));
            }
        }

        if !self.rate_allowed(&route).await {
            warn!(route = %route.path, event = "gateway.rate_limited", "route rate limited");
            return Ok(build_fallback(
                route.fallback.as_ref().or(self.global_fallback.as_ref()),
                StatusCode::TOO_MANY_REQUESTS,
                "too many requests",
            ));
        }

        if self.is_breaker_open(&route).await {
            warn!(route = %route.path, event = "gateway.breaker_open", "breaker open");
            return Ok(build_fallback(
                route.fallback.as_ref().or(self.global_fallback.as_ref()),
                StatusCode::SERVICE_UNAVAILABLE,
                "service temporarily unavailable",
            ));
        }

        let body = match req
            .into_body()
            .into_bytes_limit(
                self.request_body_limit_bytes
                    .unwrap_or(self.default_body_limit_bytes()),
            )
            .await
        {
            Ok(body) => body.to_vec(),
            Err(err) => {
                warn!(
                    event = "gateway.request_body_invalid",
                    path = %request_path,
                    error = %err,
                    "read request body failed"
                );
                return Ok(build_fallback(
                    route.fallback.as_ref().or(self.global_fallback.as_ref()),
                    StatusCode::BAD_REQUEST,
                    &err.to_string(),
                ));
            }
        };

        let timeout_ms = route
            .timeout_ms
            .or(route.effective_service_timeout(&self.services))
            .or(self.global_timeout_ms)
            .unwrap_or(5_000);

        let result = tokio::time::timeout(
            Duration::from_millis(timeout_ms),
            self.proxy_to_upstream(route, req, body),
        )
        .await;

        match result {
            Ok(Ok(response)) => {
                self.record_breaker_success(&route).await;
                Ok(response)
            }
            Ok(Err(err)) => {
                self.record_breaker_failure(&route).await;
                warn!(
                    event = "gateway.upstream_failed",
                    route = %route.path,
                    error = %err,
                    "proxy to upstream failed"
                );
                Ok(build_fallback(
                    route.fallback.as_ref().or(self.global_fallback.as_ref()),
                    StatusCode::BAD_GATEWAY,
                    &err.to_string(),
                ))
            }
            Err(_) => {
                self.record_breaker_failure(&route).await;
                warn!(
                    event = "gateway.upstream_timeout",
                    route = %route.path,
                    timeout_ms = timeout_ms,
                    "upstream request timeout"
                );
                Ok(build_fallback(
                    route.fallback.as_ref().or(self.global_fallback.as_ref()),
                    StatusCode::GATEWAY_TIMEOUT,
                    "upstream timeout",
                ))
            }
        }
    }

    fn default_body_limit_bytes(&self) -> usize {
        2 * 1024 * 1024
    }

    fn requires_auth(&self, route: &CompiledRoute) -> bool {
        has_middleware(&route.middlewares, "auth")
            || has_middleware(&route.middlewares, "jwt")
            || has_middleware(&self.global_middlewares, "auth")
            || has_middleware(&self.global_middlewares, "jwt")
    }

    fn inject_trace_headers(&self, req: &mut Request) {
        let trace_id = ensure_header(req, TRACE_ID_HEADER);
        let request_id = ensure_header(req, REQUEST_ID_HEADER);
        tracing::debug!(event = "gateway.trace_headers", trace_id = %trace_id, request_id = %request_id);
        let _ = req;
    }

    async fn proxy_to_upstream(
        self: &Arc<Self>,
        route: &CompiledRoute,
        mut req: Request,
        body: Vec<u8>,
    ) -> anyhow::Result<Response> {
        let service = self
            .services
            .get(&route.service)
            .ok_or_else(|| anyhow::anyhow!("service '{}' is not registered", route.service))?;

        let method = parse_reqwest_method(req.method())?;
        let incoming_path = req.uri().path();
        let incoming_query = req.uri().query().unwrap_or_default();
        let rewritten_path = rewrite_path(&route.path, route.rewrite.as_deref(), incoming_path);
        let upstream_url = build_upstream_url(
            &service.upstream,
            &rewritten_path,
            if incoming_query.is_empty() {
                None
            } else {
                Some(incoming_query)
            },
        );

        let mut upstream_req = self.client.request(method, upstream_url);
        for (name, value) in req.headers() {
            if !is_hop_by_hop_header(name.as_str()) {
                upstream_req = upstream_req.header(name, value);
            }
        }

        if !body.is_empty() {
            upstream_req = upstream_req.body(body);
        }
        let upstream_response = upstream_req.send().await?;
        build_upstream_response(upstream_response).await
    }

    fn select_route(&self, path: &str, method: &Method) -> Option<CompiledRoute> {
        self.routes
            .iter()
            .find(|route| route.matches_path(path) && route.method_allowed(method))
            .cloned()
    }

    async fn rate_allowed(&self, route: &CompiledRoute) -> bool {
        let Some(rate_limit) = route.rate_limit else {
            return true;
        };

        let mut states = self.rate_limit_states.lock().await;
        let state = states.entry(route.path.clone()).or_insert_with(|| {
            let now = Instant::now();
            TokenBucketState {
                tokens: rate_limit.burst as f64,
                last_refill: now,
            }
        });

        let now = Instant::now();
        let elapsed = now.duration_since(state.last_refill).as_secs_f64();
        let refill_interval = (rate_limit.refill_ms.max(1) as f64) / 1000.0;
        if elapsed >= refill_interval && rate_limit.burst > 0 {
            let refill = elapsed / refill_interval;
            state.tokens = (state.tokens + refill).min(rate_limit.burst as f64);
            state.last_refill = now;
        }

        if state.tokens >= 1.0 {
            state.tokens -= 1.0;
            return true;
        }

        false
    }

    async fn is_breaker_open(&self, route: &CompiledRoute) -> bool {
        let Some(cfg) = route.breaker else {
            return false;
        };

        let mut states = self.breaker_states.lock().await;
        let state = states
            .entry(route.path.clone())
            .or_insert_with(|| CircuitState {
                failure_count: 0,
                open_until: None,
            });

        match state.open_until {
            Some(open_until) if Instant::now() < open_until => true,
            Some(open_until) if Instant::now() >= open_until => {
                state.open_until = None;
                state.failure_count = 0;
                false
            }
            _ => {
                let _ = cfg;
                false
            }
        }
    }

    async fn record_breaker_success(&self, route: &CompiledRoute) {
        let Some(_) = route.breaker else {
            return;
        };

        let mut states = self.breaker_states.lock().await;
        if let Some(state) = states.get_mut(&route.path) {
            state.failure_count = 0;
            state.open_until = None;
        }
    }

    async fn record_breaker_failure(&self, route: &CompiledRoute) {
        let Some(cfg) = route.breaker else {
            return;
        };

        let mut states = self.breaker_states.lock().await;
        let state = states
            .entry(route.path.clone())
            .or_insert_with(|| CircuitState {
                failure_count: 0,
                open_until: None,
            });
        state.failure_count = state.failure_count.saturating_add(1);
        if state.failure_count >= cfg.failure_threshold.max(1) {
            state.failure_count = 0;
            state.open_until = Some(Instant::now() + Duration::from_millis(cfg.reset_timeout_ms));
        }
    }
}

fn first_service_name(mut services: Vec<&String>) -> Option<&str> {
    services.sort();
    services.first().map(String::as_str)
}

fn compile_routes(routes: Vec<GatewayRoute>) -> Vec<CompiledRoute> {
    routes
        .into_iter()
        .map(|route| CompiledRoute {
            path: normalize_path_prefix(&route.path),
            service: route.service,
            methods: parse_methods(&route.methods),
            timeout_ms: route.timeout_ms,
            rewrite: route.rewrite,
            fallback: route.fallback,
            rate_limit: route.rate_limit,
            breaker: route.breaker,
            middlewares: normalize_middlewares(route.middlewares),
        })
        .collect()
}

impl CompiledRoute {
    fn matches_path(&self, request_path: &str) -> bool {
        if self.path == "/" {
            return true;
        }
        request_path == self.path || request_path.starts_with(&format!("{}/", self.path))
    }

    fn method_allowed(&self, method: &Method) -> bool {
        if self.methods.is_empty() {
            true
        } else {
            self.methods.iter().any(|allowed| allowed == method)
        }
    }

    fn effective_service_timeout(
        &self,
        services: &HashMap<String, ServiceEndpoint>,
    ) -> Option<u64> {
        services.get(&self.service).and_then(|service| service.timeout_ms)
    }
}

fn parse_reqwest_method(method: &Method) -> anyhow::Result<reqwest::Method> {
    method
        .as_str()
        .parse::<reqwest::Method>()
        .map_err(anyhow::Error::from)
}

fn parse_methods(raw: &[String]) -> Vec<Method> {
    if raw.is_empty() {
        return Vec::new();
    }

    let mut methods: Vec<Method> = Vec::new();
    for method in raw {
        let normalized = method.trim().to_uppercase();
        if matches!(normalized.as_str(), "*" | "ALL") {
            return Vec::new();
        }
        if let Ok(value) = normalized.parse::<Method>() {
            methods.push(value);
        }
    }
    methods.sort_unstable_by_key(|method| method.as_str().to_string());
    methods.dedup();
    methods
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
        item == name
            || item == &format!("builtin:{name}")
            || item == &format!("builtin::{name}")
    })
}

fn validate_request_auth(req: &Request, jwt: Option<&JwtConfig>) -> anyhow::Result<()> {
    let jwt = jwt.ok_or_else(|| anyhow::anyhow!("jwt config missing"))?;

    let auth_header = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(roze_jwt::extract_bearer_token)
        .ok_or_else(|| anyhow::anyhow!("missing bearer token"))?;

    verify_token(auth_header, jwt).map_err(|err| anyhow::anyhow!(err.to_string()))?;
    Ok(())
}

fn ensure_header(req: &mut Request, key: &str) -> String {
    if let Some(value) = req.headers().get(key).and_then(|value| value.to_str().ok()) {
        return value.to_string();
    }

    let generated = roze_trace::generate_trace_id();
    if let Ok(value) = HeaderValue::from_str(&generated) {
        let _ = req.headers_mut().insert(HeaderName::from_static(key), value);
    }
    generated
}

fn rewrite_path(route_path: &str, rewrite: Option<&str>, request_path: &str) -> String {
    let rewrite = rewrite.map(normalize_path_prefix);

    if route_path == "/" {
        return rewrite
            .map(|to| {
                if to == "/" {
                    "/".to_string()
                } else {
                    to
                }
            })
            .unwrap_or_else(|| request_path.to_string());
    }

    let request_suffix = request_path
        .strip_prefix(route_path)
        .unwrap_or(request_path)
        .trim_start_matches('/');

    match rewrite {
        None => {
            if request_suffix.is_empty() {
                route_path.to_string()
            } else {
                format!("{}/{}", route_path, request_suffix)
            }
        }
        Some(rewrite_to) => {
            if request_suffix.is_empty() {
                rewrite_to.to_string()
            } else if rewrite_to == "/" {
                format!("/{request_suffix}")
            } else {
                format!("{rewrite_to}/{request_suffix}")
            }
        }
    }
}

fn build_upstream_url(base: &str, path: &str, query: Option<&str>) -> String {
    let path = normalize_path_prefix(path);
    let upstream_path = path.trim_start_matches('/');
    let trimmed_base = base.trim_end_matches('/');
    let mut url = if upstream_path.is_empty() || upstream_path == "/" {
        trimmed_base.to_string()
    } else {
        format!("{}/{}", trimmed_base, upstream_path)
    };
    if upstream_path.is_empty() || upstream_path == "/" {
        url.push('/');
    }

    if let Some(query) = query.filter(|query| !query.is_empty()) {
        url.push('?');
        url.push_str(query);
    }
    url
}

fn is_hop_by_hop_header(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "connection" | "keep-alive" | "proxy-authenticate" | "proxy-authorization"
            | "te" | "trailers" | "transfer-encoding" | "upgrade"
    )
}

async fn build_upstream_response(upstream_response: ReqwestResponse) -> anyhow::Result<Response> {
    let status = upstream_response.status();
    let body = upstream_response.bytes().await?;
    let mut poem_response = Response::builder()
        .status(
            StatusCode::from_u16(status.as_u16())
                .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR),
        )
        .body(body.to_vec())
        .unwrap_or_else(|_| Response::from("gateway upstream response build failed"));

    for (name, value) in upstream_response.headers() {
        poem_response.headers_mut().insert(name.clone(), value.clone());
    }

    Ok(poem_response)
}

fn build_cors(
    allow_origins: Vec<String>,
    allow_methods: Vec<String>,
    allow_headers: Vec<String>,
    max_age_seconds: Option<u64>,
) -> Cors {
    let mut cors = Cors::new();
    if !allow_origins.is_empty() {
        if allow_origins.len() == 1 {
            cors = cors.allow_origin(&allow_origins[0]);
        } else {
            cors = cors.allow_origin_regex(&allow_origins.join("|"));
        }
    }
    if !allow_methods.is_empty() {
        let methods = allow_methods
            .into_iter()
            .filter_map(|method| method.parse::<Method>().ok())
            .collect::<Vec<_>>();
        if !methods.is_empty() {
            cors = cors.allow_methods(methods);
        }
    }
    if !allow_headers.is_empty() {
        cors = cors.allow_headers(allow_headers);
    }
    if let Some(max_age) = max_age_seconds {
        cors = cors.max_age(Duration::from_secs(max_age));
    }
    cors
}

fn build_fallback(config: Option<&GatewayFallbackResponse>, status: StatusCode, message: &str) -> Response {
    let status = config
        .and_then(|cfg| StatusCode::from_u16(cfg.status).ok())
        .unwrap_or(status);
    let body = config
        .and_then(|cfg| cfg.body.clone())
        .unwrap_or_else(|| Value::String(message.to_string()));

    let mut response = Response::builder()
        .status(status)
        .body(serde_json::to_vec(&body).unwrap_or_else(|_| message.as_bytes().to_vec()))
        .unwrap_or_else(|_| Response::from(message.to_string()));
    response.set_content_type("application/json");

    if let Some(cfg) = config {
        for (name, value) in &cfg.headers {
            let parsed_name = match HeaderName::from_bytes(name.as_bytes()) {
                Ok(name) => name,
                Err(_) => continue,
            };
            let parsed_value = match HeaderValue::from_str(value) {
                Ok(value) => value,
                Err(_) => continue,
            };
            response.headers_mut().insert(parsed_name, parsed_value);
        }
    }

    response
}

impl From<GatewayService> for ServiceEndpoint {
    fn from(value: GatewayService) -> Self {
        Self {
            upstream: value.upstream,
            timeout_ms: value.timeout_ms,
        }
    }
}
