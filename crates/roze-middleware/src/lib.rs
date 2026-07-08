use std::time::Instant;

use roze_context::Context;
use roze_error::RozeError;

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
    service: String,
    route: String,
    method: String,
    started: Instant,
}

pub fn begin_route(
    service: String,
    route: impl Into<String>,
    method: impl Into<String>,
    request_ctx: Context,
    _governance: Option<&roze_config::GovernanceConfig>,
) -> Result<(Context, RouteGuard), RozeError> {
    let route = route.into();
    let method = method.into();
    Ok((
        request_ctx,
        RouteGuard {
            service,
            route,
            method,
            started: Instant::now(),
        },
    ))
}

pub fn finish_route(guard: RouteGuard, success: bool, status: impl Into<String>) {
    let status = status.into();
    let elapsed = guard.started.elapsed();
    roze_metrics::record_http_request(success, elapsed);
    roze_metrics::record_http_route(guard.service, guard.route, guard.method, status, elapsed);
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
}
