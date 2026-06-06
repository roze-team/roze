use std::sync::atomic::{AtomicU64, Ordering};
use std::{future::Future, time::Duration};

use poem::{
    http::Method,
    middleware::{CatchPanic, Cors},
    Endpoint, EndpointExt, Middleware, Request, Result,
};
use tracing::{info_span, Instrument};

use crate::{
    rest::AppError,
    shared::auth::{extract_bearer_token, verify_token, JwtConfig},
};

static REQUEST_TOTAL: AtomicU64 = AtomicU64::new(0);
static REQUEST_FAILED: AtomicU64 = AtomicU64::new(0);
static REQUEST_ELAPSED_MS: AtomicU64 = AtomicU64::new(0);

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
        let method = req.method().clone();
        let uri = req.uri().clone();
        let span = info_span!("http_request", %method, %uri);

        async move {
            let start = std::time::Instant::now();
            let response = self.ep.call(req).await;
            REQUEST_TOTAL.fetch_add(1, Ordering::Relaxed);
            REQUEST_ELAPSED_MS.fetch_add(start.elapsed().as_millis() as u64, Ordering::Relaxed);
            match &response {
                Ok(_) => {
                    tracing::info!(
                        elapsed_ms = start.elapsed().as_millis(),
                        "request completed"
                    );
                }
                Err(err) => {
                    REQUEST_FAILED.fetch_add(1, Ordering::Relaxed);
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
                .ok_or_else(|| AppError::Unauthorized)?;
            let token = extract_bearer_token(header_value).ok_or_else(|| AppError::Unauthorized)?;
            let claims =
                verify_token(token, &config).map_err(|err| AppError::Internal(err.to_string()))?;
            req.extensions_mut().insert(claims);
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
        req.extensions_mut().insert(RequestContext {
            request_id: request_id.clone(),
        });
        let ep = &self.ep;

        async move {
            let response = ep.call(req).await?;
            Ok(response)
        }
    }
}

pub fn trace() -> TraceMiddleware {
    TraceMiddleware
}

pub fn auth(config: JwtConfig) -> AuthMiddleware {
    AuthMiddleware::new(config)
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

pub fn metrics() -> String {
    let total = REQUEST_TOTAL.load(Ordering::Relaxed);
    let failed = REQUEST_FAILED.load(Ordering::Relaxed);
    let elapsed_ms = REQUEST_ELAPSED_MS.load(Ordering::Relaxed);
    let avg_ms = if total == 0 { 0 } else { elapsed_ms / total };

    format!(
        concat!(
            "# HELP roze_http_requests_total Total HTTP requests\n",
            "# TYPE roze_http_requests_total counter\n",
            "roze_http_requests_total {}\n",
            "# HELP roze_http_requests_failed_total Failed HTTP requests\n",
            "# TYPE roze_http_requests_failed_total counter\n",
            "roze_http_requests_failed_total {}\n",
            "# HELP roze_http_request_duration_ms_total Total HTTP request duration in milliseconds\n",
            "# TYPE roze_http_request_duration_ms_total counter\n",
            "roze_http_request_duration_ms_total {}\n",
            "# HELP roze_http_request_duration_ms_avg Average HTTP request duration in milliseconds\n",
            "# TYPE roze_http_request_duration_ms_avg gauge\n",
            "roze_http_request_duration_ms_avg {}\n"
        ),
        total, failed, elapsed_ms, avg_ms
    )
}

pub fn apply_auth<E>(endpoint: E, config: JwtConfig) -> impl Endpoint
where
    E: Endpoint,
{
    endpoint.with(AuthMiddleware::new(config))
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
}

fn next_request_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = crate::shared::auth::now_unix_secs().unwrap_or_default();
    format!("{nanos:x}-{seq:x}")
}
