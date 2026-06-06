use std::sync::{Arc, Mutex};
use std::{
    future::Future,
    time::{Duration, Instant},
};

use poem::{
    http::{
        header::{HeaderName, HeaderValue},
        Method,
    },
    middleware::{CatchPanic, Cors},
    Endpoint, EndpointExt, Middleware, Request, Result,
};
use tracing::Instrument;

use roze_auth::principal_from_claims;
use roze_context::Context;
use roze_error::RozeError;
use roze_metrics::record_http_request;
use roze_jwt::{extract_bearer_token, verify_token, JwtConfig};
use roze_trace::{generate_trace_id, request_span, TRACE_ID_HEADER};

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
        let mut req = req;
        let method = req.method().clone();
        let uri = req.uri().clone();
        let trace_id = ensure_trace_id_header(&mut req);
        let span = request_span(method.as_str(), uri.path(), &trace_id);

        async move {
            let start = std::time::Instant::now();
            let response = self.ep.call(req).await;
            record_http_request(response.is_ok(), start.elapsed());
            match &response {
                Ok(_) => {
                    tracing::info!(
                        elapsed_ms = start.elapsed().as_millis(),
                        "request completed"
                    );
                }
                Err(err) => {
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
                .ok_or_else(|| RozeError::Unauthorized)?;
            let token = extract_bearer_token(header_value).ok_or_else(|| RozeError::Unauthorized)?;
            let claims =
                verify_token(token, &config).map_err(|err| RozeError::Internal(err.to_string()))?;
            req.extensions_mut().insert(principal_from_claims(&claims));
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
        let trace_id = incoming_trace_id(&req).unwrap_or_else(|| request_id.clone());
        req.extensions_mut()
            .insert(Context::background_with_trace_id(trace_id.clone()));
        req.extensions_mut().insert(RequestContext {
            request_id: request_id.clone(),
            trace_id,
        });
        let ep = &self.ep;

        async move { ep.call(req).await }
    }
}

pub fn trace() -> TraceMiddleware {
    TraceMiddleware
}

pub fn auth(config: JwtConfig) -> AuthMiddleware {
    AuthMiddleware::new(config)
}

pub fn rate_limit(config: RateLimitConfig) -> RateLimitMiddleware {
    RateLimitMiddleware::new(config)
}

pub fn breaker(config: BreakerConfig) -> BreakerMiddleware {
    BreakerMiddleware::new(config)
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
    pub trace_id: String,
}

#[derive(Debug, Clone)]
pub struct RateLimitMiddleware {
    config: RateLimitConfig,
    state: Arc<Mutex<RateLimitState>>,
}

#[derive(Debug)]
struct RateLimitState {
    tokens: f64,
    last_refill: Instant,
}

#[derive(Debug, Clone)]
pub struct RateLimitEndpoint<E> {
    ep: E,
    config: RateLimitConfig,
    state: Arc<Mutex<RateLimitState>>,
}

#[derive(Debug, Clone)]
pub struct BreakerMiddleware {
    config: BreakerConfig,
    state: Arc<Mutex<BreakerState>>,
}

#[derive(Debug)]
struct BreakerState {
    failures: u32,
    open_until: Option<Instant>,
}

#[derive(Debug, Clone)]
pub struct BreakerEndpoint<E> {
    ep: E,
    config: BreakerConfig,
    state: Arc<Mutex<BreakerState>>,
}

impl RateLimitMiddleware {
    pub fn new(config: RateLimitConfig) -> Self {
        Self {
            state: Arc::new(Mutex::new(RateLimitState {
                tokens: config.burst as f64,
                last_refill: Instant::now(),
            })),
            config,
        }
    }
}

impl<E> Middleware<E> for RateLimitMiddleware
where
    E: Endpoint,
{
    type Output = RateLimitEndpoint<E>;

    fn transform(&self, ep: E) -> Self::Output {
        RateLimitEndpoint {
            ep,
            config: self.config.clone(),
            state: Arc::clone(&self.state),
        }
    }
}

impl<E> Endpoint for RateLimitEndpoint<E>
where
    E: Endpoint,
{
    type Output = E::Output;

    fn call(&self, req: Request) -> impl Future<Output = Result<Self::Output>> + Send {
        let allowed = {
            let mut state = self.state.lock().expect("rate limit lock poisoned");
            refill_tokens(&mut state, &self.config);
            if state.tokens >= 1.0 {
                state.tokens -= 1.0;
                true
            } else {
                false
            }
        };

        async move {
            if !allowed {
                return Err(RozeError::Internal("rate limited".to_string()).into());
            }
            self.ep.call(req).await
        }
    }
}

impl BreakerMiddleware {
    pub fn new(config: BreakerConfig) -> Self {
        Self {
            state: Arc::new(Mutex::new(BreakerState {
                failures: 0,
                open_until: None,
            })),
            config,
        }
    }
}

impl<E> Middleware<E> for BreakerMiddleware
where
    E: Endpoint,
{
    type Output = BreakerEndpoint<E>;

    fn transform(&self, ep: E) -> Self::Output {
        BreakerEndpoint {
            ep,
            config: self.config.clone(),
            state: Arc::clone(&self.state),
        }
    }
}

impl<E> Endpoint for BreakerEndpoint<E>
where
    E: Endpoint,
{
    type Output = E::Output;

    fn call(&self, req: Request) -> impl Future<Output = Result<Self::Output>> + Send {
        let open = {
            let mut state = self.state.lock().expect("breaker lock poisoned");
            breaker_is_open(&mut state)
        };

        async move {
            if open {
                return Err(RozeError::Internal("circuit open".to_string()).into());
            }

            let response = self.ep.call(req).await;
            let mut state = self.state.lock().expect("breaker lock poisoned");
            match &response {
                Ok(_) => breaker_record_success(&mut state),
                Err(_) => breaker_record_failure(&mut state, &self.config),
            }
            response
        }
    }
}

fn next_request_id() -> String {
    generate_trace_id()
}

fn incoming_trace_id(req: &Request) -> Option<String> {
    req.headers()
        .get(TRACE_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn ensure_trace_id_header(req: &mut Request) -> String {
    if let Some(trace_id) = incoming_trace_id(req) {
        return trace_id;
    }

    let trace_id = generate_trace_id();
    let value = HeaderValue::from_str(&trace_id)
        .unwrap_or_else(|_| HeaderValue::from_static("trace-invalid"));
    req.headers_mut()
        .insert(HeaderName::from_static(TRACE_ID_HEADER), value);
    trace_id
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
