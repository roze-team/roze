use std::{
    sync::OnceLock,
    time::{Duration, Instant},
};

use roze_context::Context;
use roze_error::RozeError;
use roze_resilience::{BreakerRegistry, RateLimitRegistry, SheddingRegistry};

static ROUTE_RATE_LIMITS: OnceLock<RateLimitRegistry> = OnceLock::new();
static ROUTE_BREAKERS: OnceLock<BreakerRegistry> = OnceLock::new();
static ROUTE_SHEDDERS: OnceLock<SheddingRegistry> = OnceLock::new();

#[derive(Debug, Clone)]
pub struct CommonMiddlewareConfig {
    pub request_context: bool,
    pub tracing: bool,
    pub auth: Option<AuthConfig>,
    pub cors_config: Option<CorsConfig>,
    pub timeout_ms: Option<u64>,
    pub body_limit_bytes: Option<usize>,
}

impl Default for CommonMiddlewareConfig {
    fn default() -> Self {
        Self {
            request_context: true,
            tracing: true,
            auth: None,
            cors_config: None,
            timeout_ms: None,
            body_limit_bytes: None,
        }
    }
}

impl From<&roze_config::HttpMiddlewaresConfig> for CommonMiddlewareConfig {
    fn from(config: &roze_config::HttpMiddlewaresConfig) -> Self {
        Self {
            request_context: true,
            tracing: true,
            auth: None,
            cors_config: config.cors_config.as_ref().map(CorsConfig::from),
            timeout_ms: config.timeout.then_some(30_000),
            body_limit_bytes: config.request_body_limit_bytes,
        }
    }
}

#[derive(Debug, Clone)]
pub struct AuthConfig {
    pub jwt_secret: String,
}

impl From<&roze_config::AuthConfig> for AuthConfig {
    fn from(config: &roze_config::AuthConfig) -> Self {
        Self {
            jwt_secret: config.jwt_secret.clone(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct CorsConfig {
    pub allow_origins: Vec<String>,
    pub allow_methods: Vec<String>,
    pub allow_headers: Vec<String>,
    pub max_age_seconds: Option<u64>,
}

impl From<&roze_config::HttpCorsConfig> for CorsConfig {
    fn from(config: &roze_config::HttpCorsConfig) -> Self {
        Self {
            allow_origins: config.allow_origins.clone(),
            allow_methods: config.allow_methods.clone(),
            allow_headers: config.allow_headers.clone(),
            max_age_seconds: config.max_age_seconds,
        }
    }
}

pub fn apply_common<S>(service: S) -> S {
    apply_common_with_config(service, CommonMiddlewareConfig::default())
}

pub fn apply_common_with_config<S>(service: S, _config: CommonMiddlewareConfig) -> S {
    service
}

pub fn apply_timeout<S>(service: S, _timeout_ms: u64) -> S {
    service
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

#[derive(Debug, Clone)]
pub struct RouteGuard {
    key: String,
    service: String,
    route: String,
    method: String,
    started: Instant,
    breaker: Option<RouteBreakerConfig>,
    shedding: Option<RouteSheddingConfig>,
}

#[derive(Debug, Clone)]
pub struct RoutePolicy {
    pub timeout: Option<Duration>,
    pub rate_limit: Option<RouteRateLimitConfig>,
    pub breaker: Option<RouteBreakerConfig>,
    pub shedding: Option<RouteSheddingConfig>,
    pub fallback: Option<roze_config::GovernanceFallbackConfig>,
}

#[derive(Debug, Clone, Copy)]
pub struct RouteRateLimitConfig {
    pub burst: u32,
    pub refill: Duration,
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
    let route_config = governance.routes.get(route);
    RoutePolicy {
        timeout: route_config
            .and_then(|route| route.timeout_ms)
            .or(governance.timeout_ms)
            .map(Duration::from_millis),
        rate_limit: route_config
            .and_then(|route| route.rate_limit)
            .or(governance.rate_limit)
            .map(|config| RouteRateLimitConfig {
                burst: config.burst,
                refill: Duration::from_millis(config.refill_ms),
            }),
        breaker: route_config
            .and_then(|route| route.breaker)
            .or(governance.breaker)
            .map(|config| RouteBreakerConfig {
                failure_threshold: config.failure_threshold,
                reset_timeout: Duration::from_millis(config.reset_timeout_ms),
            }),
        shedding: route_config
            .and_then(|route| route.shedding)
            .or(governance.shedding)
            .map(|config| RouteSheddingConfig {
                concurrency: config.concurrency,
                window: Duration::from_millis(config.window_ms),
                min_samples: config.min_samples,
                max_avg_latency: Duration::from_millis(config.max_avg_latency_ms),
                max_failure_ratio_per_mille: config.max_failure_ratio_per_mille,
                cool_down: Duration::from_millis(config.cool_down_ms),
            }),
        fallback: route_config
            .and_then(|route| route.fallback.clone())
            .or_else(|| governance.fallback.clone())
            .filter(|fallback| fallback.enabled),
    }
}

pub fn route_fallback(
    governance: Option<&roze_config::GovernanceConfig>,
    route: &str,
) -> Option<roze_config::GovernanceFallbackConfig> {
    route_policy(governance, route).fallback
}

pub fn apply_fallback(
    error: RozeError,
    fallback: Option<roze_config::GovernanceFallbackConfig>,
) -> RozeError {
    if error.is_client_error() {
        return error;
    }
    let Some(fallback) = fallback else {
        return error;
    };
    roze_metrics::record_resilience_decision("http", "fallback", "served");
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
    let key = format!("{service}:{method}:{route}");
    if let Some(config) = &policy.rate_limit {
        match enforce_route_rate_limit(&key, config) {
            Ok(()) => roze_metrics::record_resilience_decision("http", "rate_limit", "allowed"),
            Err(err) => {
                roze_metrics::record_resilience_decision("http", "rate_limit", "rejected");
                return Err(err);
            }
        }
    }
    if policy
        .breaker
        .as_ref()
        .is_some_and(|_| route_breaker_is_open(&key))
    {
        roze_metrics::record_resilience_decision("http", "breaker", "open");
        return Err(RozeError::Unavailable("circuit open".to_string()));
    }
    if policy.breaker.is_some() {
        roze_metrics::record_resilience_decision("http", "breaker", "allowed");
    }
    if let Some(config) = &policy.shedding {
        match enforce_route_shedding(&key, config) {
            Ok(()) => roze_metrics::record_resilience_decision("http", "load_shedding", "allowed"),
            Err(err) => {
                roze_metrics::record_resilience_decision("http", "load_shedding", "shed");
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
            shedding: policy.shedding,
        },
    ))
}

pub fn finish_route(guard: RouteGuard, success: bool, status: impl Into<String>) {
    let status = status.into();
    let elapsed = guard.started.elapsed();
    roze_metrics::record_http_request(success, elapsed);
    if let Some(config) = guard.breaker {
        let breaker_success = success || !status.starts_with('5');
        route_breaker_record(&guard.key, breaker_success, &config);
    }
    if let Some(config) = guard.shedding {
        let shedding_success = success || !status.starts_with('5');
        route_shedding_record(&guard.key, shedding_success, elapsed, &config);
    }
    roze_metrics::record_http_route(guard.service, guard.route, guard.method, status, elapsed);
}

fn enforce_route_rate_limit(key: &str, config: &RouteRateLimitConfig) -> Result<(), RozeError> {
    if ROUTE_RATE_LIMITS
        .get_or_init(RateLimitRegistry::new)
        .allow(key, route_rate_limit_config(*config))
    {
        Ok(())
    } else {
        Err(RozeError::RateLimited)
    }
}

fn route_rate_limit_config(config: RouteRateLimitConfig) -> roze_resilience::RateLimitConfig {
    roze_resilience::RateLimitConfig {
        burst: config.burst,
        refill: config.refill,
    }
}

fn route_breaker_is_open(key: &str) -> bool {
    ROUTE_BREAKERS
        .get_or_init(BreakerRegistry::new)
        .is_open(key)
}

fn route_breaker_record(key: &str, success: bool, config: &RouteBreakerConfig) {
    let registry = ROUTE_BREAKERS.get_or_init(BreakerRegistry::new);
    if success {
        registry.record_success(key);
        return;
    }
    registry.record_failure(
        key,
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

pub type RateLimitConfig = roze_resilience::RateLimitConfig;

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn route_policy_prefers_route_override() {
        let mut governance = roze_config::GovernanceConfig {
            timeout_ms: Some(1_000),
            rate_limit: Some(roze_config::RateLimitConfig {
                burst: 10,
                refill_ms: 100,
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
            apply_fallback(RozeError::BadRequest("bad".into()), Some(fallback.clone())),
            RozeError::BadRequest("bad".into())
        );
        assert!(matches!(
            apply_fallback(RozeError::Internal("boom".into()), Some(fallback)),
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

    #[test]
    fn begin_route_enforces_rate_limit() {
        let mut governance = roze_config::GovernanceConfig::default();
        let route = format!("limited_{}", std::process::id());
        governance.routes.insert(
            route.clone(),
            roze_config::RouteGovernanceConfig {
                rate_limit: Some(roze_config::RateLimitConfig {
                    burst: 1,
                    refill_ms: 60_000,
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
        assert!(matches!(second, Err(RozeError::RateLimited)));
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
}
