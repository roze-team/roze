use std::{
    collections::{BTreeMap, HashMap},
    convert::Infallible,
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex as StdMutex, Weak},
    time::{Duration, Instant},
};

use bytes::Bytes;
use http::{header, HeaderMap, HeaderName, HeaderValue, Method, StatusCode};
use http_body_util::BodyExt;
use roze_config::{
    BreakerConfig, GatewayConfig, GatewayFallbackResponse, GatewayHealthCheckConfig,
    GatewayOutlierConfig, GatewayRoute, GatewayService, GovernanceConfig, GovernanceFallbackConfig,
    RateLimitConfig, RouteGovernanceConfig, SheddingConfig,
};
use roze_context::Context;
use roze_http::rest::{self, HttpResponse, IncomingRequest};
use roze_jwt::{verify_token, JwtConfig};
use roze_resilience::{
    full_jitter_delay, BreakerDecision, BreakerPermit, BreakerRegistry, RateLimitRegistry,
    RetryBudgetRegistry, SheddingRegistry,
};
use roze_rpc::registry::{Registry, ServiceInstance};
use tokio::sync::Mutex;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone)]
pub struct GatewayServiceRuntime {
    runtime: Arc<GatewayRuntime>,
}

struct GatewayRuntime {
    routes: Vec<CompiledRoute>,
    services: HashMap<String, GatewayService>,
    client: reqwest::Client,
    global_timeout: Option<Duration>,
    global_fallback: Option<GatewayFallbackResponse>,
    global_middlewares: Vec<String>,
    jwt: Option<JwtConfig>,
    api_keys: Option<roze_auth::ApiKeyConfig>,
    request_body_limit_bytes: usize,
    rate_limits: RateLimitRegistry,
    breakers: BreakerRegistry,
    shedders: SheddingRegistry,
    retry_budgets: RetryBudgetRegistry,
    registry: Option<Arc<dyn Registry>>,
    registry_cursors: StdMutex<HashMap<String, u64>>,
    outlier_states: Mutex<HashMap<String, OutlierState>>,
    health_states: Mutex<HashMap<String, HealthState>>,
    health_tasks: StdMutex<Vec<tokio::task::JoinHandle<()>>>,
}

#[derive(Debug, Clone)]
struct CompiledRoute {
    path: String,
    service: String,
    methods: Vec<Method>,
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

impl Default for HealthState {
    fn default() -> Self {
        Self {
            healthy: true,
            failures: 0,
            successes: 0,
        }
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
        let runtime = self.runtime.clone();
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
        client: reqwest::Client::new(),
        global_timeout,
        global_fallback,
        global_middlewares,
        jwt,
        api_keys,
        request_body_limit_bytes: config.request_body_limit_bytes.unwrap_or(2 * 1024 * 1024),
        rate_limits: RateLimitRegistry::new(),
        breakers: BreakerRegistry::new(),
        shedders: SheddingRegistry::new(),
        retry_budgets: RetryBudgetRegistry::default(),
        registry,
        registry_cursors: StdMutex::new(HashMap::new()),
        outlier_states: Mutex::new(HashMap::new()),
        health_states: Mutex::new(HashMap::new()),
        health_tasks: StdMutex::new(Vec::new()),
    });
    runtime.spawn_health_checks();
    GatewayServiceRuntime { runtime }
}

impl Drop for GatewayRuntime {
    fn drop(&mut self) {
        for task in self
            .health_tasks
            .lock()
            .expect("gateway health task lock")
            .drain(..)
        {
            task.abort();
        }
    }
}

impl GatewayRuntime {
    async fn handle(&self, mut request: IncomingRequest) -> HttpResponse {
        let started = Instant::now();
        let method = request.method().clone();
        let path = request.uri().path().to_string();
        let Some(route) = self.select_route(&path) else {
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
        let key = format!("{}:{}", route.service, route.path);
        let retry_key = format!("{}:{}:{}", route.service, method, route.path);

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

        if let Some(config) = route.rate_limit {
            let allowed = self.rate_limits.allow(
                key.clone(),
                roze_resilience::RateLimitConfig {
                    burst: config.burst,
                    refill: Duration::from_millis(config.refill_ms.max(1)),
                },
            );
            roze_metrics::record_resilience_decision(
                "gateway",
                "gateway",
                "rate_limit",
                if allowed { "allowed" } else { "rejected" },
            );
            if !allowed {
                return self.finish_response(
                    Some(route),
                    &method,
                    "rate_limited",
                    started,
                    fallback_response(
                        route.fallback.as_ref().or(self.global_fallback.as_ref()),
                        StatusCode::TOO_MANY_REQUESTS,
                        "too many requests",
                    ),
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

            let success = matches!(&result, Ok(response) if !response.status().is_server_error());
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
                Err(UpstreamError::Unavailable(error)) => (
                    "upstream_unavailable",
                    fallback_response(
                        route.fallback.as_ref().or(self.global_fallback.as_ref()),
                        StatusCode::BAD_GATEWAY,
                        &error,
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
        let mut builder = self
            .client
            .request(method.clone(), upstream)
            .timeout(timeout);
        for (name, value) in headers {
            builder = builder.header(name, value);
        }
        if !body.is_empty() {
            builder = builder.body(body);
        }
        let response = match builder.send().await {
            Ok(response) => response,
            Err(error) => {
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
        };
        let status =
            StatusCode::from_u16(response.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
        let response_headers = response
            .headers()
            .iter()
            .filter(|(name, _)| !is_hop_by_hop_header(name.as_str()))
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect::<Vec<_>>();
        let bytes = match response.bytes().await {
            Ok(bytes) => bytes,
            Err(error) => {
                self.record_outlier_failure(&target).await;
                return Err(if error.is_timeout() {
                    UpstreamError::Timeout
                } else {
                    UpstreamError::Request(error)
                });
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

    fn select_route(&self, path: &str) -> Option<&CompiledRoute> {
        self.routes.iter().find(|route| route.matches_path(path))
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
}

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

fn compile_routes(
    routes: Vec<GatewayRoute>,
    governance: Option<&GovernanceConfig>,
) -> Vec<CompiledRoute> {
    routes
        .into_iter()
        .map(|route| {
            let path = normalize_path_prefix(&route.path);
            let route_governance = gateway_route_governance(governance, &path, &route.service);
            let retry = route_governance
                .and_then(|route| route.retry)
                .or_else(|| governance.and_then(|governance| governance.retry));
            CompiledRoute {
                path,
                service: route.service,
                methods: parse_methods(&route.methods),
                timeout: route
                    .timeout_ms
                    .or_else(|| route_governance.and_then(|route| route.timeout_ms))
                    .map(Duration::from_millis),
                retries: route
                    .retries
                    .map(|retries| retries as usize)
                    .unwrap_or_else(|| {
                        retry
                            .map(|retry| retry.max_attempts.saturating_sub(1) as usize)
                            .unwrap_or_default()
                    }),
                retry_backoff: Duration::from_millis(
                    route
                        .retry_backoff_ms
                        .or_else(|| retry.map(|retry| retry.backoff_ms))
                        .unwrap_or_default(),
                ),
                retry_max_backoff: Duration::from_millis(
                    retry
                        .map(|retry| retry.max_backoff_ms)
                        .unwrap_or_else(|| route.retry_backoff_ms.unwrap_or_default()),
                ),
                retry_budget_percent: retry.and_then(|retry| retry.budget_percent),
                rewrite: route.rewrite,
                fallback: route
                    .fallback
                    .or_else(|| route_governance.and_then(route_governance_fallback))
                    .or_else(|| governance.and_then(governance_fallback)),
                rate_limit: route
                    .rate_limit
                    .or_else(|| route_governance.and_then(|route| route.rate_limit))
                    .or_else(|| governance.and_then(|governance| governance.rate_limit)),
                breaker: route
                    .breaker
                    .or_else(|| route_governance.and_then(|route| route.breaker))
                    .or_else(|| governance.and_then(|governance| governance.breaker)),
                shedding: route
                    .shedding
                    .or_else(|| route_governance.and_then(|route| route.shedding))
                    .or_else(|| governance.and_then(|governance| governance.shedding)),
                middlewares: normalize_middlewares(route.middlewares),
                instance_tags: route.instance_tags,
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

fn governance_fallback(config: &GovernanceConfig) -> Option<GatewayFallbackResponse> {
    config.fallback.as_ref().and_then(convert_fallback)
}

fn route_governance_fallback(config: &RouteGovernanceConfig) -> Option<GatewayFallbackResponse> {
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
                return Some(roze_auth::principal(
                    claims.sub,
                    claims.roles,
                    claims.tenant,
                ));
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

fn normalize_upstream_base(upstream: &str) -> String {
    let upstream = upstream.trim().trim_end_matches('/');
    if upstream.starts_with("http://") || upstream.starts_with("https://") {
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
    use roze_config::RetryConfig;
    use roze_rpc::registry::MemoryRegistry;
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
            }],
            routes: vec![GatewayRoute {
                path: "/catalog".to_string(),
                service: "catalog".to_string(),
                methods: vec![method.to_string()],
                weight: 100,
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
                .snapshot("catalog:/catalog")
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
    }
}
