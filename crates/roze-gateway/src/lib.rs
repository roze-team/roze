use std::{
    collections::{BTreeMap, HashMap},
    hash::{Hash, Hasher},
    sync::Arc,
    time::{Duration, Instant},
};

use axum::{
    body::{to_bytes, Body},
    http::{header, HeaderName, HeaderValue, Method, Request, Response, StatusCode},
    routing::any,
    Router,
};
use reqwest::Response as ReqwestResponse;
use serde_json::Value;
use tokio::sync::Mutex;
use tower_http::cors::{Any, CorsLayer};
use tracing::warn;

use roze_config::{
    BreakerConfig, GatewayConfig, GatewayFallbackResponse, GatewayHealthCheckConfig,
    GatewayOutlierConfig, GatewayRoute, GatewayService, GovernanceConfig, RateLimitConfig,
    RouteGovernanceConfig,
};
use roze_jwt::{verify_token, Claims, JwtConfig};
use roze_rpc::registry::Registry;

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
    registry: Option<Arc<dyn Registry>>,
    registry_cursors: Arc<Mutex<HashMap<String, usize>>>,
    outlier_states: Arc<Mutex<HashMap<String, OutlierState>>>,
    health_states: Arc<Mutex<HashMap<String, HealthState>>>,
    rate_limit_states: Arc<Mutex<HashMap<String, TokenBucketState>>>,
    breaker_states: Arc<Mutex<HashMap<String, CircuitState>>>,
}

#[derive(Debug, Clone)]
struct ServiceEndpoint {
    name: String,
    upstream: String,
    registry_name: Option<String>,
    instance_tags: BTreeMap<String, String>,
    timeout_ms: Option<u64>,
    outlier: Option<GatewayOutlierConfig>,
    health_check: Option<GatewayHealthCheckConfig>,
}

#[derive(Debug, Clone)]
struct UpstreamTarget {
    base: String,
    instance_key: String,
    outlier: Option<GatewayOutlierConfig>,
}

#[derive(Clone)]
struct CompiledRoute {
    path: String,
    service: String,
    methods: Vec<Method>,
    weight: u32,
    instance_tags: BTreeMap<String, String>,
    timeout_ms: Option<u64>,
    retries: u32,
    retry_backoff_ms: u64,
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

#[derive(Debug, Clone, Copy, Default)]
struct OutlierState {
    failures: u32,
    ejected_until: Option<Instant>,
}

#[derive(Debug, Clone, Copy)]
struct HealthState {
    healthy: bool,
    failures: u32,
    successes: u32,
}

impl Default for HealthState {
    fn default() -> Self {
        Self {
            healthy: true,
            failures: 0,
            successes: 0,
        }
    }
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
            registry: None,
            registry_cursors: Arc::new(Mutex::new(HashMap::new())),
            outlier_states: Arc::new(Mutex::new(HashMap::new())),
            health_states: Arc::new(Mutex::new(HashMap::new())),
            rate_limit_states: Arc::new(Mutex::new(HashMap::new())),
            breaker_states: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

pub fn build_router(config: GatewayConfig, jwt: Option<JwtConfig>) -> Router {
    build_router_with_registry(config, jwt, None)
}

pub fn build_router_with_registry(
    config: GatewayConfig,
    jwt: Option<JwtConfig>,
    registry: Option<Arc<dyn Registry>>,
) -> Router {
    build_router_with_registry_and_governance(config, jwt, registry, None)
}

pub fn build_router_with_registry_and_governance(
    config: GatewayConfig,
    jwt: Option<JwtConfig>,
    registry: Option<Arc<dyn Registry>>,
    governance: Option<GovernanceConfig>,
) -> Router {
    let cors_config = config.cors;
    let mut runtime = GatewayRuntime {
        global_timeout_ms: config.timeout_ms,
        global_fallback: config.fallback,
        request_body_limit_bytes: config.request_body_limit_bytes,
        global_middlewares: normalize_middlewares(config.middlewares),
        jwt,
        registry,
        ..Default::default()
    };

    for service in config.services {
        runtime
            .services
            .insert(service.name.clone(), ServiceEndpoint::from(service));
    }

    let mut routes = compile_routes(config.routes, governance.as_ref());
    routes.sort_by_key(|route| std::cmp::Reverse(route.path.len()));
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
                weight: 100,
                instance_tags: BTreeMap::new(),
                timeout_ms: runtime.global_timeout_ms,
                retries: 0,
                retry_backoff_ms: 0,
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
    runtime.clone().spawn_health_checks();
    let app = Router::new().fallback(any(move |req: Request<Body>| {
        let runtime = runtime.clone();
        async move { runtime.handle_request(req).await }
    }));

    if let Some(cors) = cors_config {
        let cors = build_cors(
            cors.allow_origins,
            cors.allow_methods,
            cors.allow_headers,
            cors.max_age_seconds,
        );
        app.layer(cors)
    } else {
        app
    }
}

impl GatewayRuntime {
    async fn handle_request(self: Arc<Self>, req: Request<Body>) -> Response<Body> {
        let started = Instant::now();
        let request_path = req.uri().path().to_string();
        let request_method = req.method().clone();

        let mut req = req;
        let request_id = ensure_header(&mut req, roze_context::REQUEST_ID_HEADER);
        let trace_id = ensure_header(&mut req, roze_context::TRACE_ID_HEADER);

        let span = tracing::info_span!(
            "gateway.request",
            method = %request_method,
            path = %request_path,
            request_id = %request_id,
            trace_id = %trace_id,
        );
        let _enter = span.enter();

        let Some(route) = self.select_route(&request_path, &request_method, &request_id) else {
            warn!(
                path = %request_path,
                method = %request_method,
                event = "gateway.no_route",
                "no route matched"
            );
            self.record_gateway_response(
                None,
                &request_method,
                StatusCode::NOT_FOUND,
                "no_route",
                started,
            );
            return build_fallback(
                self.global_fallback.as_ref(),
                StatusCode::NOT_FOUND,
                "gateway route not found",
            );
        };

        self.inject_trace_headers(&mut req);

        if !route.method_allowed(&request_method) {
            warn!(
                path = %request_path,
                method = %request_method,
                route = %route.path,
                event = "gateway.method_not_allowed",
                "method blocked by route config"
            );
            self.record_gateway_response(
                Some(&route),
                &request_method,
                StatusCode::METHOD_NOT_ALLOWED,
                "method_not_allowed",
                started,
            );
            return build_fallback(
                route.fallback.as_ref().or(self.global_fallback.as_ref()),
                StatusCode::METHOD_NOT_ALLOWED,
                "method not allowed",
            );
        }

        // fixed middleware order: trace -> auth -> rate -> breaker -> timeout -> upstream
        if self.requires_auth(&route) {
            match validate_request_auth(&req, self.jwt.as_ref()) {
                Ok(claims) => inject_auth_context_headers(&mut req, &claims),
                Err(err) => {
                    warn!(
                        error = %err,
                        event = "gateway.auth_failed",
                        "auth validation failed"
                    );
                    self.record_gateway_response(
                        Some(&route),
                        &request_method,
                        StatusCode::UNAUTHORIZED,
                        "unauthorized",
                        started,
                    );
                    return build_fallback(
                        route.fallback.as_ref().or(self.global_fallback.as_ref()),
                        StatusCode::UNAUTHORIZED,
                        "unauthorized",
                    );
                }
            }
        }

        if !self.rate_allowed(&route).await {
            warn!(route = %route.path, event = "gateway.rate_limited", "route rate limited");
            self.record_gateway_response(
                Some(&route),
                &request_method,
                StatusCode::TOO_MANY_REQUESTS,
                "rate_limited",
                started,
            );
            return build_fallback(
                route.fallback.as_ref().or(self.global_fallback.as_ref()),
                StatusCode::TOO_MANY_REQUESTS,
                "too many requests",
            );
        }

        if self.is_breaker_open(&route).await {
            warn!(route = %route.path, event = "gateway.breaker_open", "breaker open");
            self.record_gateway_response(
                Some(&route),
                &request_method,
                StatusCode::SERVICE_UNAVAILABLE,
                "breaker_open",
                started,
            );
            return build_fallback(
                route.fallback.as_ref().or(self.global_fallback.as_ref()),
                StatusCode::SERVICE_UNAVAILABLE,
                "service temporarily unavailable",
            );
        }

        let upstream_method = request_method.clone();
        let upstream_path = request_path.clone();
        let upstream_query = req.uri().query().map(str::to_string);
        let upstream_headers = req
            .headers()
            .iter()
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect::<Vec<_>>();

        let body_limit = self
            .request_body_limit_bytes
            .unwrap_or(self.default_body_limit_bytes());
        let body = match to_bytes(req.into_body(), body_limit).await {
            Ok(body) => body.to_vec(),
            Err(err) => {
                warn!(
                    event = "gateway.request_body_invalid",
                    path = %request_path,
                    error = %err,
                    "read request body failed"
                );
                self.record_gateway_response(
                    Some(&route),
                    &request_method,
                    StatusCode::BAD_REQUEST,
                    "bad_request_body",
                    started,
                );
                return build_fallback(
                    route.fallback.as_ref().or(self.global_fallback.as_ref()),
                    StatusCode::BAD_REQUEST,
                    &err.to_string(),
                );
            }
        };

        let timeout_ms = route
            .timeout_ms
            .or(route.effective_service_timeout(&self.services))
            .or(self.global_timeout_ms)
            .unwrap_or(5_000);

        let max_attempts = route.retries.saturating_add(1);
        let mut attempt = 0;
        let mut last_error: Option<anyhow::Error> = None;
        let mut last_timeout = false;

        while attempt < max_attempts {
            attempt += 1;
            let result = tokio::time::timeout(
                Duration::from_millis(timeout_ms),
                self.proxy_to_upstream(
                    &route,
                    upstream_method.clone(),
                    upstream_path.clone(),
                    upstream_query.clone(),
                    upstream_headers.clone(),
                    body.clone(),
                ),
            )
            .await;

            match result {
                Ok(Ok(response)) => {
                    if response.status().is_server_error() && attempt < max_attempts {
                        warn!(
                            event = "gateway.upstream_status_retry",
                            route = %route.path,
                            attempt = attempt,
                            max_attempts = max_attempts,
                            status = response.status().as_u16(),
                            "upstream returned retryable status"
                        );
                        roze_metrics::record_gateway_retry(
                            route.service.clone(),
                            route.path.clone(),
                            format!("status_{}", response.status().as_u16()),
                        );
                    } else {
                        if response.status().is_server_error() {
                            self.record_breaker_failure(&route).await;
                        } else {
                            self.record_breaker_success(&route).await;
                        }
                        if attempt > 1 {
                            tracing::info!(
                                event = "gateway.upstream_retry_succeeded",
                                route = %route.path,
                                attempt = attempt,
                                max_attempts = max_attempts,
                                "proxy retry completed"
                            );
                        }
                        self.record_gateway_response(
                            Some(&route),
                            &request_method,
                            response.status(),
                            response_outcome(response.status()),
                            started,
                        );
                        return response;
                    }
                }
                Ok(Err(err)) => {
                    last_timeout = false;
                    warn!(
                        event = "gateway.upstream_failed",
                        route = %route.path,
                        attempt = attempt,
                        max_attempts = max_attempts,
                        error = %err,
                        "proxy to upstream failed"
                    );
                    if attempt < max_attempts {
                        roze_metrics::record_gateway_retry(
                            route.service.clone(),
                            route.path.clone(),
                            "upstream_error",
                        );
                    }
                    last_error = Some(err);
                }
                Err(_) => {
                    last_timeout = true;
                    warn!(
                        event = "gateway.upstream_timeout",
                        route = %route.path,
                        attempt = attempt,
                        max_attempts = max_attempts,
                        timeout_ms = timeout_ms,
                        "upstream request timeout"
                    );
                    if attempt < max_attempts {
                        roze_metrics::record_gateway_retry(
                            route.service.clone(),
                            route.path.clone(),
                            "timeout",
                        );
                    }
                }
            }

            if attempt < max_attempts && route.retry_backoff_ms > 0 {
                tokio::time::sleep(Duration::from_millis(route.retry_backoff_ms)).await;
            }
        }

        self.record_breaker_failure(&route).await;
        if last_timeout {
            self.record_gateway_response(
                Some(&route),
                &request_method,
                StatusCode::GATEWAY_TIMEOUT,
                "timeout",
                started,
            );
            build_fallback(
                route.fallback.as_ref().or(self.global_fallback.as_ref()),
                StatusCode::GATEWAY_TIMEOUT,
                "upstream timeout",
            )
        } else {
            let message = last_error
                .as_ref()
                .map(|err| err.to_string())
                .unwrap_or_else(|| "upstream failed".to_string());
            self.record_gateway_response(
                Some(&route),
                &request_method,
                StatusCode::BAD_GATEWAY,
                "upstream_failed",
                started,
            );
            build_fallback(
                route.fallback.as_ref().or(self.global_fallback.as_ref()),
                StatusCode::BAD_GATEWAY,
                &message,
            )
        }
    }

    fn default_body_limit_bytes(&self) -> usize {
        2 * 1024 * 1024
    }

    fn record_gateway_response(
        &self,
        route: Option<&CompiledRoute>,
        method: &Method,
        status: StatusCode,
        outcome: &str,
        started: Instant,
    ) {
        let (service, route_path) = route
            .map(|route| (route.service.clone(), route.path.clone()))
            .unwrap_or_else(|| (String::new(), String::new()));
        roze_metrics::record_gateway_route(
            service,
            route_path,
            method.as_str().to_string(),
            status.as_u16().to_string(),
            outcome.to_string(),
            started.elapsed(),
        );
    }

    fn requires_auth(&self, route: &CompiledRoute) -> bool {
        has_middleware(&route.middlewares, "auth")
            || has_middleware(&route.middlewares, "jwt")
            || has_middleware(&self.global_middlewares, "auth")
            || has_middleware(&self.global_middlewares, "jwt")
    }

    fn inject_trace_headers(&self, req: &mut Request<Body>) {
        let trace_id = ensure_header(req, roze_context::TRACE_ID_HEADER);
        let request_id = ensure_header(req, roze_context::REQUEST_ID_HEADER);
        tracing::debug!(event = "gateway.trace_headers", trace_id = %trace_id, request_id = %request_id);
        let _ = req;
    }

    async fn proxy_to_upstream(
        self: &Arc<Self>,
        route: &CompiledRoute,
        method: Method,
        incoming_path: String,
        incoming_query: Option<String>,
        headers: Vec<(HeaderName, HeaderValue)>,
        body: Vec<u8>,
    ) -> anyhow::Result<Response<Body>> {
        let service = self
            .services
            .get(&route.service)
            .ok_or_else(|| anyhow::anyhow!("service '{}' is not registered", route.service))?;
        let target = self.resolve_upstream(service, &route.instance_tags).await?;

        let method = parse_reqwest_method(&method)?;
        let rewritten_path = rewrite_path(&route.path, route.rewrite.as_deref(), &incoming_path);
        let upstream_url = build_upstream_url(
            &target.base,
            &rewritten_path,
            incoming_query.as_deref().filter(|query| !query.is_empty()),
        );

        let mut upstream_req = self.client.request(method, upstream_url);
        for (name, value) in headers {
            if !is_hop_by_hop_header(name.as_str()) {
                upstream_req = upstream_req.header(name, value);
            }
        }

        if !body.is_empty() {
            upstream_req = upstream_req.body(body);
        }
        let upstream_response = match upstream_req.send().await {
            Ok(response) => response,
            Err(err) => {
                self.record_outlier_failure(&target).await;
                roze_metrics::record_gateway_upstream(
                    route.service.clone(),
                    target.instance_key.clone(),
                    "request_error",
                );
                return Err(err.into());
            }
        };
        let response = build_upstream_response(upstream_response).await?;
        if response.status().is_server_error() {
            self.record_outlier_failure(&target).await;
            roze_metrics::record_gateway_upstream(
                route.service.clone(),
                target.instance_key.clone(),
                format!("status_{}", response.status().as_u16()),
            );
        } else {
            self.record_outlier_success(&target).await;
            roze_metrics::record_gateway_upstream(
                route.service.clone(),
                target.instance_key.clone(),
                "ok",
            );
        }
        Ok(response)
    }

    async fn resolve_upstream(
        &self,
        service: &ServiceEndpoint,
        route_instance_tags: &BTreeMap<String, String>,
    ) -> anyhow::Result<UpstreamTarget> {
        let registry_name = service
            .registry_name
            .as_deref()
            .filter(|name| !name.is_empty());
        let should_discover = registry_name.is_some() || service.upstream.is_empty();

        if should_discover {
            let registry = self
                .registry
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("registry is not configured"))?;
            let name = registry_name.unwrap_or(&service.name);
            let instances = registry.discover(name).await?;
            let available = self
                .available_instances(
                    name,
                    instances,
                    &effective_instance_tags(&service.instance_tags, route_instance_tags),
                    service.outlier,
                    service.health_check.as_ref(),
                )
                .await;
            if !available.is_empty() {
                let weighted = roze_rpc::registry::weighted_instances(&available);
                let candidates = if weighted.is_empty() {
                    &available
                } else {
                    &weighted
                };
                let idx = {
                    let mut cursors = self.registry_cursors.lock().await;
                    let cursor = cursors.entry(name.to_string()).or_default();
                    let idx = *cursor % candidates.len();
                    *cursor = cursor.wrapping_add(1);
                    idx
                };
                let instance = &candidates[idx];
                return Ok(UpstreamTarget {
                    base: normalize_upstream_base(&instance.addr),
                    instance_key: upstream_instance_key(name, &instance.addr),
                    outlier: service.outlier,
                });
            }
        }

        if service.upstream.is_empty() {
            anyhow::bail!("service upstream is empty")
        }

        Ok(UpstreamTarget {
            base: normalize_upstream_base(&service.upstream),
            instance_key: upstream_instance_key(&service.name, &service.upstream),
            outlier: service.outlier,
        })
    }

    async fn available_instances(
        &self,
        service_name: &str,
        instances: Vec<roze_rpc::registry::ServiceInstance>,
        required_tags: &BTreeMap<String, String>,
        outlier: Option<GatewayOutlierConfig>,
        health_check: Option<&GatewayHealthCheckConfig>,
    ) -> Vec<roze_rpc::registry::ServiceInstance> {
        if required_tags.is_empty() && outlier.is_none() && health_check.is_none() {
            return instances;
        }
        let now = Instant::now();
        let outlier_states = self.outlier_states.lock().await;
        let health_states = self.health_states.lock().await;
        let mut available = instances
            .iter()
            .filter(|instance| {
                if !instance_matches_tags(instance, required_tags) {
                    return false;
                }
                let key = upstream_instance_key(service_name, &instance.addr);
                let ejected = outlier_states
                    .get(&key)
                    .and_then(|state| state.ejected_until)
                    .is_some_and(|until| until > now);
                let unhealthy = health_check.is_some()
                    && health_states.get(&key).is_some_and(|state| !state.healthy);
                !ejected && !unhealthy
            })
            .cloned()
            .collect::<Vec<_>>();
        if available.is_empty() && required_tags.is_empty() {
            available = instances;
        }
        available
    }

    fn spawn_health_checks(self: Arc<Self>) {
        for service in self.services.values().cloned() {
            let Some(health_check) = service.health_check.clone() else {
                continue;
            };
            let runtime = self.clone();
            tokio::spawn(async move {
                loop {
                    runtime.check_service_health(&service, &health_check).await;
                    tokio::time::sleep(Duration::from_millis(health_check.interval_ms.max(1)))
                        .await;
                }
            });
        }
    }

    async fn check_service_health(
        &self,
        service: &ServiceEndpoint,
        health_check: &GatewayHealthCheckConfig,
    ) {
        let registry_name = service
            .registry_name
            .as_deref()
            .filter(|name| !name.is_empty());
        let mut targets = Vec::new();

        if let Some(name) = registry_name {
            if let Some(registry) = self.registry.as_ref() {
                match registry.discover(name).await {
                    Ok(instances) => {
                        targets.extend(instances.into_iter().map(|instance| {
                            (
                                upstream_instance_key(name, &instance.addr),
                                normalize_upstream_base(&instance.addr),
                            )
                        }));
                    }
                    Err(err) => {
                        warn!(
                            event = "gateway.health_check_discover_failed",
                            service = %service.name,
                            registry_name = %name,
                            error = %err,
                            "discover service instances for health check failed"
                        );
                    }
                }
            }
        } else if !service.upstream.is_empty() {
            targets.push((
                upstream_instance_key(&service.name, &service.upstream),
                normalize_upstream_base(&service.upstream),
            ));
        }

        for (key, base) in targets {
            let healthy = self.health_probe(&base, health_check).await;
            self.record_health_result(&key, health_check, healthy).await;
        }
    }

    async fn health_probe(&self, base: &str, health_check: &GatewayHealthCheckConfig) -> bool {
        let url = build_upstream_url(base, &health_check.path, None);
        let timeout = Duration::from_millis(health_check.timeout_ms.max(1));
        let result = tokio::time::timeout(timeout, self.client.get(url).send()).await;
        let Ok(Ok(response)) = result else {
            return false;
        };
        response.status().as_u16() == health_check.expected_status
    }

    async fn record_health_result(
        &self,
        instance_key: &str,
        health_check: &GatewayHealthCheckConfig,
        healthy: bool,
    ) {
        let mut states = self.health_states.lock().await;
        let state = states.entry(instance_key.to_string()).or_default();
        if healthy {
            state.failures = 0;
            state.successes = state.successes.saturating_add(1);
            if !state.healthy && state.successes >= health_check.healthy_threshold.max(1) {
                state.healthy = true;
                warn!(
                    event = "gateway.upstream_recovered",
                    upstream = %instance_key,
                    "upstream instance recovered"
                );
            }
        } else {
            state.successes = 0;
            state.failures = state.failures.saturating_add(1);
            if state.healthy && state.failures >= health_check.unhealthy_threshold.max(1) {
                state.healthy = false;
                warn!(
                    event = "gateway.upstream_unhealthy",
                    upstream = %instance_key,
                    "upstream instance marked unhealthy"
                );
            }
        }
    }

    async fn record_outlier_success(&self, target: &UpstreamTarget) {
        if target.outlier.is_none() {
            return;
        }
        let mut states = self.outlier_states.lock().await;
        if let Some(state) = states.get_mut(&target.instance_key) {
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
            warn!(
                event = "gateway.upstream_ejected",
                upstream = %target.instance_key,
                ejection_ms = config.ejection_ms,
                "upstream instance ejected"
            );
        }
    }

    fn select_route(&self, path: &str, method: &Method, seed: &str) -> Option<CompiledRoute> {
        let matches = self
            .routes
            .iter()
            .filter(|route| route.matches_path(path) && route.method_allowed(method))
            .collect::<Vec<_>>();
        let longest = matches.iter().map(|route| route.path.len()).max()?;
        let candidates = matches
            .into_iter()
            .filter(|route| route.path.len() == longest)
            .collect::<Vec<_>>();
        pick_weighted_route(&candidates, seed).cloned()
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
    services.first().map(|service| service.as_str())
}

fn pick_weighted_route<'a>(routes: &[&'a CompiledRoute], seed: &str) -> Option<&'a CompiledRoute> {
    if routes.is_empty() {
        return None;
    }
    if routes.len() == 1 {
        return Some(routes[0]);
    }

    let total = routes
        .iter()
        .map(|route| route.weight.max(1) as u64)
        .sum::<u64>();
    let mut point = stable_hash(seed) % total;
    for route in routes {
        let weight = route.weight.max(1) as u64;
        if point < weight {
            return Some(*route);
        }
        point -= weight;
    }

    routes.first().copied()
}

fn stable_hash(value: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

fn effective_instance_tags(
    service_tags: &BTreeMap<String, String>,
    route_tags: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    let mut tags = service_tags.clone();
    tags.extend(route_tags.clone());
    tags
}

fn instance_matches_tags(
    instance: &roze_rpc::registry::ServiceInstance,
    required_tags: &BTreeMap<String, String>,
) -> bool {
    required_tags.iter().all(|(key, expected)| {
        instance
            .metadata
            .get(key)
            .is_some_and(|actual| actual == expected)
    })
}

fn compile_routes(
    routes: Vec<GatewayRoute>,
    governance: Option<&GovernanceConfig>,
) -> Vec<CompiledRoute> {
    routes
        .into_iter()
        .map(|route| {
            let path = normalize_path_prefix(&route.path);
            let route_governance = gateway_route_governance(governance, &path, &route.service);
            let retry = effective_retry(governance, route_governance);
            CompiledRoute {
                path,
                service: route.service,
                methods: parse_methods(&route.methods),
                weight: route.weight,
                instance_tags: route.instance_tags,
                timeout_ms: route
                    .timeout_ms
                    .or_else(|| route_governance.and_then(|route| route.timeout_ms))
                    .or_else(|| governance.and_then(|governance| governance.timeout_ms)),
                retries: route
                    .retries
                    .unwrap_or_else(|| retry.map(retries_from_max_attempts).unwrap_or_default()),
                retry_backoff_ms: route
                    .retry_backoff_ms
                    .unwrap_or_else(|| retry.map(|retry| retry.backoff_ms).unwrap_or_default()),
                rewrite: route.rewrite,
                fallback: route.fallback,
                rate_limit: route
                    .rate_limit
                    .or_else(|| route_governance.and_then(|route| route.rate_limit))
                    .or_else(|| governance.and_then(|governance| governance.rate_limit)),
                breaker: route
                    .breaker
                    .or_else(|| route_governance.and_then(|route| route.breaker))
                    .or_else(|| governance.and_then(|governance| governance.breaker)),
                middlewares: normalize_middlewares(route.middlewares),
            }
        })
        .collect()
}

fn gateway_route_governance<'a>(
    governance: Option<&'a GovernanceConfig>,
    path: &str,
    service: &str,
) -> Option<&'a RouteGovernanceConfig> {
    let governance = governance?;
    governance
        .routes
        .get(path)
        .or_else(|| governance.routes.get(path.trim_start_matches('/')))
        .or_else(|| governance.routes.get(service))
}

fn effective_retry(
    governance: Option<&GovernanceConfig>,
    route_governance: Option<&RouteGovernanceConfig>,
) -> Option<roze_config::RetryConfig> {
    route_governance
        .and_then(|route| route.retry)
        .or_else(|| governance.and_then(|governance| governance.retry))
}

fn retries_from_max_attempts(retry: roze_config::RetryConfig) -> u32 {
    retry.max_attempts.saturating_sub(1)
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
        services
            .get(&self.service)
            .and_then(|service| service.timeout_ms)
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
        item == name || item == &format!("builtin:{name}") || item == &format!("builtin::{name}")
    })
}

fn validate_request_auth(req: &Request<Body>, jwt: Option<&JwtConfig>) -> anyhow::Result<Claims> {
    let jwt = jwt.ok_or_else(|| anyhow::anyhow!("jwt config missing"))?;

    let auth_header = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(roze_jwt::extract_bearer_token)
        .ok_or_else(|| anyhow::anyhow!("missing bearer token"))?;

    verify_token(auth_header, jwt).map_err(|err| anyhow::anyhow!(err.to_string()))
}

fn ensure_header(req: &mut Request<Body>, key: &str) -> String {
    if let Some(value) = req.headers().get(key).and_then(|value| value.to_str().ok()) {
        return value.to_string();
    }

    let generated = roze_trace::generate_trace_id();
    if let Ok(value) = HeaderValue::from_str(&generated) {
        if let Ok(name) = HeaderName::from_bytes(key.as_bytes()) {
            let _ = req.headers_mut().insert(name, value);
        }
    }
    generated
}

fn inject_auth_context_headers(req: &mut Request<Body>, claims: &Claims) {
    insert_header(req, roze_context::SUBJECT_HEADER, &claims.sub);
    if let Some(tenant) = claims.tenant.as_deref().filter(|tenant| !tenant.is_empty()) {
        insert_header(req, roze_context::TENANT_HEADER, tenant);
    }
    if !claims.roles.is_empty() {
        insert_header(req, roze_context::ROLES_HEADER, &claims.roles.join(","));
    }
}

fn insert_header(req: &mut Request<Body>, key: &'static str, value: &str) {
    let Ok(value) = HeaderValue::from_str(value) else {
        return;
    };
    let Ok(name) = HeaderName::from_bytes(key.as_bytes()) else {
        return;
    };
    req.headers_mut().insert(name, value);
}

fn rewrite_path(route_path: &str, rewrite: Option<&str>, request_path: &str) -> String {
    let rewrite = rewrite.map(normalize_path_prefix);

    if route_path == "/" {
        return rewrite
            .map(|to| if to == "/" { "/".to_string() } else { to })
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

fn response_outcome(status: StatusCode) -> &'static str {
    if status.is_success() {
        "ok"
    } else if status.is_client_error() {
        "client_error"
    } else if status.is_server_error() {
        "server_error"
    } else {
        "other"
    }
}

fn normalize_upstream_base(base: &str) -> String {
    let base = base.trim();
    if base.starts_with("http://") || base.starts_with("https://") {
        base.to_string()
    } else {
        format!("http://{base}")
    }
}

fn upstream_instance_key(service: &str, upstream: &str) -> String {
    format!("{service}:{}", normalize_upstream_base(upstream))
}

fn is_hop_by_hop_header(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailers"
            | "transfer-encoding"
            | "upgrade"
    )
}

async fn build_upstream_response(
    upstream_response: ReqwestResponse,
) -> anyhow::Result<Response<Body>> {
    let status = upstream_response.status();
    let headers = upstream_response.headers().clone();
    let body = upstream_response.bytes().await?;
    let mut response = Response::builder()
        .status(StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR))
        .body(Body::from(body))?;

    for (name, value) in &headers {
        response.headers_mut().insert(name.clone(), value.clone());
    }

    Ok(response)
}

fn build_cors(
    allow_origins: Vec<String>,
    allow_methods: Vec<String>,
    allow_headers: Vec<String>,
    max_age_seconds: Option<u64>,
) -> CorsLayer {
    let mut cors = CorsLayer::new();
    if !allow_origins.is_empty() {
        if allow_origins.iter().any(|origin| origin == "*") {
            cors = cors.allow_origin(Any);
        } else {
            let origins = allow_origins
                .into_iter()
                .filter_map(|origin| origin.parse::<HeaderValue>().ok())
                .collect::<Vec<_>>();
            if !origins.is_empty() {
                cors = cors.allow_origin(origins);
            }
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
        let headers = allow_headers
            .into_iter()
            .filter_map(|header| header.parse::<HeaderName>().ok())
            .collect::<Vec<_>>();
        if !headers.is_empty() {
            cors = cors.allow_headers(headers);
        }
    }
    if let Some(max_age) = max_age_seconds {
        cors = cors.max_age(Duration::from_secs(max_age));
    }
    cors
}

fn build_fallback(
    config: Option<&GatewayFallbackResponse>,
    status: StatusCode,
    message: &str,
) -> Response<Body> {
    let status = config
        .and_then(|cfg| StatusCode::from_u16(cfg.status).ok())
        .unwrap_or(status);
    let body = config
        .and_then(|cfg| cfg.body.clone())
        .unwrap_or_else(|| Value::String(message.to_string()));

    let mut response = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            serde_json::to_vec(&body).unwrap_or_else(|_| message.as_bytes().to_vec()),
        ))
        .expect("fallback response");

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
            name: value.name,
            upstream: value.upstream,
            registry_name: value.registry_name,
            instance_tags: value.instance_tags,
            timeout_ms: value.timeout_ms,
            outlier: value.outlier,
            health_check: value.health_check,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{extract::State, response::IntoResponse, routing::get, Router};
    use roze_rpc::registry::{MemoryRegistry, Registry, ServiceInstance};
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    use tower::ServiceExt;

    #[tokio::test]
    async fn retries_retryable_upstream_status() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let upstream = Router::new()
            .route(
                "/user",
                get(|State(attempts): State<Arc<AtomicUsize>>| async move {
                    let attempt = attempts.fetch_add(1, Ordering::SeqCst);
                    if attempt < 2 {
                        StatusCode::SERVICE_UNAVAILABLE.into_response()
                    } else {
                        "ok".into_response()
                    }
                }),
            )
            .with_state(attempts.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind upstream");
        let upstream_addr = listener.local_addr().expect("upstream addr");
        tokio::spawn(async move {
            let _ = axum::serve(listener, upstream).await;
        });

        let gateway = build_router(
            GatewayConfig {
                services: vec![GatewayService {
                    name: "user".to_string(),
                    upstream: format!("http://{upstream_addr}"),
                    ..empty_gateway_service()
                }],
                routes: vec![GatewayRoute {
                    path: "/user".to_string(),
                    service: "user".to_string(),
                    methods: vec!["GET".to_string()],
                    retries: Some(2),
                    retry_backoff_ms: Some(1),
                    ..empty_gateway_route()
                }],
                ..empty_gateway_config()
            },
            None,
        );

        let response = gateway
            .oneshot(
                Request::builder()
                    .uri("/user")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("gateway response");

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(attempts.load(Ordering::SeqCst), 3);

        let metrics = roze_metrics::http_metrics();
        assert!(metrics.contains("roze_gateway_route_requests_total"));
        assert!(metrics.contains("roze_gateway_route_retries_total"));
        assert!(metrics.contains(r#"service="user""#));
        assert!(metrics.contains(r#"route="/user""#));
        assert!(metrics.contains(r#"reason="status_503""#));
    }

    #[tokio::test]
    async fn discovers_registry_upstreams_round_robin() {
        let first_hits = Arc::new(AtomicUsize::new(0));
        let second_hits = Arc::new(AtomicUsize::new(0));
        let first_addr = spawn_text_upstream("first", first_hits.clone()).await;
        let second_addr = spawn_text_upstream("second", second_hits.clone()).await;

        let registry = Arc::new(MemoryRegistry::default());
        registry
            .register(ServiceInstance::new("user", first_addr.to_string()))
            .await
            .expect("register first");
        registry
            .register(ServiceInstance::new("user", second_addr.to_string()))
            .await
            .expect("register second");

        let gateway = build_router_with_registry(
            GatewayConfig {
                services: vec![GatewayService {
                    name: "user".to_string(),
                    registry_name: Some("user".to_string()),
                    ..empty_gateway_service()
                }],
                routes: vec![GatewayRoute {
                    path: "/user".to_string(),
                    service: "user".to_string(),
                    methods: vec!["GET".to_string()],
                    ..empty_gateway_route()
                }],
                ..empty_gateway_config()
            },
            None,
            Some(registry as Arc<dyn Registry>),
        );

        for _ in 0..2 {
            let response = gateway
                .clone()
                .oneshot(
                    Request::builder()
                        .uri("/user")
                        .body(Body::empty())
                        .expect("request"),
                )
                .await
                .expect("gateway response");
            assert_eq!(response.status(), StatusCode::OK);
        }

        assert_eq!(first_hits.load(Ordering::SeqCst), 1);
        assert_eq!(second_hits.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn ejects_registry_instance_after_retryable_status() {
        let bad_hits = Arc::new(AtomicUsize::new(0));
        let good_hits = Arc::new(AtomicUsize::new(0));
        let bad_addr =
            spawn_status_upstream(StatusCode::SERVICE_UNAVAILABLE, bad_hits.clone()).await;
        let good_addr = spawn_text_upstream("ok", good_hits.clone()).await;

        let registry = Arc::new(MemoryRegistry::default());
        registry
            .register(ServiceInstance::new("user", bad_addr.to_string()))
            .await
            .expect("register bad");
        registry
            .register(ServiceInstance::new("user", good_addr.to_string()))
            .await
            .expect("register good");

        let gateway = build_router_with_registry(
            GatewayConfig {
                services: vec![GatewayService {
                    name: "user".to_string(),
                    registry_name: Some("user".to_string()),
                    outlier: Some(GatewayOutlierConfig {
                        failure_threshold: 1,
                        ejection_ms: 60_000,
                    }),
                    ..empty_gateway_service()
                }],
                routes: vec![GatewayRoute {
                    path: "/user".to_string(),
                    service: "user".to_string(),
                    methods: vec!["GET".to_string()],
                    ..empty_gateway_route()
                }],
                ..empty_gateway_config()
            },
            None,
            Some(registry as Arc<dyn Registry>),
        );

        let first = gateway
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/user")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("first gateway response");
        assert_eq!(first.status(), StatusCode::SERVICE_UNAVAILABLE);

        for _ in 0..3 {
            let response = gateway
                .clone()
                .oneshot(
                    Request::builder()
                        .uri("/user")
                        .body(Body::empty())
                        .expect("request"),
                )
                .await
                .expect("gateway response");
            assert_eq!(response.status(), StatusCode::OK);
        }

        assert_eq!(bad_hits.load(Ordering::SeqCst), 1);
        assert_eq!(good_hits.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn active_health_check_skips_unhealthy_registry_instance() {
        let bad_hits = Arc::new(AtomicUsize::new(0));
        let good_hits = Arc::new(AtomicUsize::new(0));
        let bad_addr =
            spawn_health_upstream("bad", StatusCode::SERVICE_UNAVAILABLE, bad_hits.clone()).await;
        let good_addr = spawn_health_upstream("good", StatusCode::OK, good_hits.clone()).await;

        let registry = Arc::new(MemoryRegistry::default());
        registry
            .register(ServiceInstance::new("user", bad_addr.to_string()))
            .await
            .expect("register bad");
        registry
            .register(ServiceInstance::new("user", good_addr.to_string()))
            .await
            .expect("register good");

        let gateway = build_router_with_registry(
            GatewayConfig {
                services: vec![GatewayService {
                    name: "user".to_string(),
                    registry_name: Some("user".to_string()),
                    health_check: Some(GatewayHealthCheckConfig {
                        path: "/healthz".to_string(),
                        interval_ms: 10,
                        timeout_ms: 100,
                        unhealthy_threshold: 1,
                        healthy_threshold: 1,
                        expected_status: 200,
                    }),
                    ..empty_gateway_service()
                }],
                routes: vec![GatewayRoute {
                    path: "/user".to_string(),
                    service: "user".to_string(),
                    methods: vec!["GET".to_string()],
                    ..empty_gateway_route()
                }],
                ..empty_gateway_config()
            },
            None,
            Some(registry as Arc<dyn Registry>),
        );

        tokio::time::sleep(Duration::from_millis(80)).await;

        for _ in 0..3 {
            let response = gateway
                .clone()
                .oneshot(
                    Request::builder()
                        .uri("/user")
                        .body(Body::empty())
                        .expect("request"),
                )
                .await
                .expect("gateway response");
            assert_eq!(response.status(), StatusCode::OK);
        }

        assert_eq!(bad_hits.load(Ordering::SeqCst), 0);
        assert_eq!(good_hits.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn route_instance_tags_filter_registry_instances() {
        let blue_hits = Arc::new(AtomicUsize::new(0));
        let green_hits = Arc::new(AtomicUsize::new(0));
        let blue_addr = spawn_text_upstream("blue", blue_hits.clone()).await;
        let green_addr = spawn_text_upstream("green", green_hits.clone()).await;

        let registry = Arc::new(MemoryRegistry::default());
        let mut blue = ServiceInstance::new("user", blue_addr.to_string());
        blue.metadata
            .insert("version".to_string(), "blue".to_string());
        registry.register(blue).await.expect("register blue");
        let mut green = ServiceInstance::new("user", green_addr.to_string());
        green
            .metadata
            .insert("version".to_string(), "green".to_string());
        registry.register(green).await.expect("register green");

        let gateway = build_router_with_registry(
            GatewayConfig {
                services: vec![GatewayService {
                    name: "user".to_string(),
                    registry_name: Some("user".to_string()),
                    ..empty_gateway_service()
                }],
                routes: vec![GatewayRoute {
                    path: "/user".to_string(),
                    service: "user".to_string(),
                    methods: vec!["GET".to_string()],
                    instance_tags: BTreeMap::from([("version".to_string(), "green".to_string())]),
                    ..empty_gateway_route()
                }],
                ..empty_gateway_config()
            },
            None,
            Some(registry as Arc<dyn Registry>),
        );

        for _ in 0..3 {
            let response = gateway
                .clone()
                .oneshot(
                    Request::builder()
                        .uri("/user")
                        .body(Body::empty())
                        .expect("request"),
                )
                .await
                .expect("gateway response");
            assert_eq!(response.status(), StatusCode::OK);
        }

        assert_eq!(blue_hits.load(Ordering::SeqCst), 0);
        assert_eq!(green_hits.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn registry_instance_weight_controls_gateway_pick_order() {
        let first_hits = Arc::new(AtomicUsize::new(0));
        let second_hits = Arc::new(AtomicUsize::new(0));
        let first_addr = spawn_text_upstream("first", first_hits.clone()).await;
        let second_addr = spawn_text_upstream("second", second_hits.clone()).await;

        let registry = Arc::new(MemoryRegistry::default());
        let mut first = ServiceInstance::new("user", first_addr.to_string());
        first.weight = 2;
        registry.register(first).await.expect("register first");
        let mut second = ServiceInstance::new("user", second_addr.to_string());
        second.weight = 1;
        registry.register(second).await.expect("register second");

        let gateway = build_router_with_registry(
            GatewayConfig {
                services: vec![GatewayService {
                    name: "user".to_string(),
                    registry_name: Some("user".to_string()),
                    ..empty_gateway_service()
                }],
                routes: vec![GatewayRoute {
                    path: "/user".to_string(),
                    service: "user".to_string(),
                    methods: vec!["GET".to_string()],
                    ..empty_gateway_route()
                }],
                ..empty_gateway_config()
            },
            None,
            Some(registry as Arc<dyn Registry>),
        );

        for _ in 0..3 {
            let response = gateway
                .clone()
                .oneshot(
                    Request::builder()
                        .uri("/user")
                        .body(Body::empty())
                        .expect("request"),
                )
                .await
                .expect("gateway response");
            assert_eq!(response.status(), StatusCode::OK);
        }

        assert_eq!(first_hits.load(Ordering::SeqCst), 2);
        assert_eq!(second_hits.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn weighted_route_selection_uses_route_weights() {
        let low = CompiledRoute {
            path: "/user".to_string(),
            service: "v1".to_string(),
            methods: Vec::new(),
            weight: 1,
            instance_tags: BTreeMap::new(),
            timeout_ms: None,
            retries: 0,
            retry_backoff_ms: 0,
            rewrite: None,
            fallback: None,
            rate_limit: None,
            breaker: None,
            middlewares: Vec::new(),
        };
        let high = CompiledRoute {
            service: "v2".to_string(),
            weight: 9,
            ..low.clone()
        };
        let routes = vec![&low, &high];
        let mut low_count = 0;
        let mut high_count = 0;

        for idx in 0..100 {
            match pick_weighted_route(&routes, &format!("request-{idx}"))
                .expect("route")
                .service
                .as_str()
            {
                "v1" => low_count += 1,
                "v2" => high_count += 1,
                other => panic!("unexpected service {other}"),
            }
        }

        assert!(high_count > low_count);
    }

    #[test]
    fn routes_inherit_unified_governance_defaults() {
        let mut governance = GovernanceConfig {
            timeout_ms: Some(500),
            retry: Some(roze_config::RetryConfig {
                max_attempts: 3,
                backoff_ms: 25,
                max_backoff_ms: 250,
                budget_percent: None,
            }),
            rate_limit: Some(RateLimitConfig {
                burst: 10,
                refill_ms: 100,
            }),
            breaker: Some(BreakerConfig {
                failure_threshold: 4,
                reset_timeout_ms: 1_000,
            }),
            ..Default::default()
        };
        governance.routes.insert(
            "/user".to_string(),
            RouteGovernanceConfig {
                timeout_ms: Some(100),
                retry: Some(roze_config::RetryConfig {
                    max_attempts: 2,
                    backoff_ms: 5,
                    max_backoff_ms: 50,
                    budget_percent: None,
                }),
                ..Default::default()
            },
        );

        let routes = compile_routes(
            vec![GatewayRoute {
                path: "/user".to_string(),
                service: "user".to_string(),
                methods: vec!["GET".to_string()],
                ..empty_gateway_route()
            }],
            Some(&governance),
        );
        let route = routes.first().expect("compiled route");

        assert_eq!(route.timeout_ms, Some(100));
        assert_eq!(route.retries, 1);
        assert_eq!(route.retry_backoff_ms, 5);
        assert_eq!(route.rate_limit.expect("rate limit").burst, 10);
        assert_eq!(route.breaker.expect("breaker").failure_threshold, 4);
    }

    #[test]
    fn route_fields_override_unified_governance() {
        let governance = GovernanceConfig {
            timeout_ms: Some(500),
            retry: Some(roze_config::RetryConfig {
                max_attempts: 3,
                backoff_ms: 25,
                max_backoff_ms: 250,
                budget_percent: None,
            }),
            ..Default::default()
        };

        let routes = compile_routes(
            vec![GatewayRoute {
                path: "/user".to_string(),
                service: "user".to_string(),
                methods: vec!["GET".to_string()],
                timeout_ms: Some(50),
                retries: Some(4),
                retry_backoff_ms: Some(1),
                ..empty_gateway_route()
            }],
            Some(&governance),
        );
        let route = routes.first().expect("compiled route");

        assert_eq!(route.timeout_ms, Some(50));
        assert_eq!(route.retries, 4);
        assert_eq!(route.retry_backoff_ms, 1);
    }

    async fn spawn_text_upstream(
        text: &'static str,
        hits: Arc<AtomicUsize>,
    ) -> std::net::SocketAddr {
        let upstream = Router::new().route(
            "/user",
            get(move || {
                let hits = hits.clone();
                async move {
                    hits.fetch_add(1, Ordering::SeqCst);
                    text
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind upstream");
        let upstream_addr = listener.local_addr().expect("upstream addr");
        tokio::spawn(async move {
            let _ = axum::serve(listener, upstream).await;
        });
        upstream_addr
    }

    async fn spawn_status_upstream(
        status: StatusCode,
        hits: Arc<AtomicUsize>,
    ) -> std::net::SocketAddr {
        let upstream = Router::new().route(
            "/user",
            get(move || {
                let hits = hits.clone();
                async move {
                    hits.fetch_add(1, Ordering::SeqCst);
                    status
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind upstream");
        let upstream_addr = listener.local_addr().expect("upstream addr");
        tokio::spawn(async move {
            let _ = axum::serve(listener, upstream).await;
        });
        upstream_addr
    }

    async fn spawn_health_upstream(
        text: &'static str,
        health_status: StatusCode,
        hits: Arc<AtomicUsize>,
    ) -> std::net::SocketAddr {
        let upstream = Router::new()
            .route(
                "/user",
                get(move || {
                    let hits = hits.clone();
                    async move {
                        hits.fetch_add(1, Ordering::SeqCst);
                        text
                    }
                }),
            )
            .route("/healthz", get(move || async move { health_status }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind upstream");
        let upstream_addr = listener.local_addr().expect("upstream addr");
        tokio::spawn(async move {
            let _ = axum::serve(listener, upstream).await;
        });
        upstream_addr
    }

    fn empty_gateway_config() -> GatewayConfig {
        GatewayConfig {
            listen: None,
            services: Vec::new(),
            routes: Vec::new(),
            middlewares: Vec::new(),
            timeout_ms: None,
            request_body_limit_bytes: None,
            fallback: None,
            cors: None,
        }
    }

    fn empty_gateway_route() -> GatewayRoute {
        GatewayRoute {
            path: String::new(),
            service: String::new(),
            methods: Vec::new(),
            weight: 100,
            instance_tags: BTreeMap::new(),
            middlewares: Vec::new(),
            timeout_ms: None,
            retries: None,
            retry_backoff_ms: None,
            rewrite: None,
            fallback: None,
            rate_limit: None,
            breaker: None,
        }
    }

    fn empty_gateway_service() -> GatewayService {
        GatewayService {
            name: String::new(),
            upstream: String::new(),
            registry_name: None,
            instance_tags: BTreeMap::new(),
            timeout_ms: None,
            outlier: None,
            health_check: None,
        }
    }
}
