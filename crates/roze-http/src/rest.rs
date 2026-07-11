use std::{
    convert::Infallible,
    error::Error,
    future::{ready, Future, Ready},
    net::SocketAddr,
    task::{Context, Poll},
    time::Duration,
};

use bytes::Bytes;
use http::{header, HeaderName, HeaderValue, Request, Response, StatusCode};
use http_body_util::{combinators::BoxBody, BodyExt, Full};
use hyper::server::conn::http1;
use hyper_util::rt::TokioIo;
use roze_service::{RuntimeService, ServiceFuture};
use serde::Serialize;
use tokio::{net::TcpListener, task::JoinSet};
use tower::{Service, ServiceExt};
use tracing::info;

#[derive(Debug)]
pub struct BoxError(Box<dyn Error + Send + Sync + 'static>);

impl BoxError {
    pub fn new(error: impl Error + Send + Sync + 'static) -> Self {
        Self(Box::new(error))
    }
}

impl AsRef<dyn Error + Send + Sync + 'static> for BoxError {
    fn as_ref(&self) -> &(dyn Error + Send + Sync + 'static) {
        self.0.as_ref()
    }
}

impl std::fmt::Display for BoxError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

impl Error for BoxError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.0.source()
    }
}

impl From<Infallible> for BoxError {
    fn from(never: Infallible) -> Self {
        match never {}
    }
}

pub type Body = BoxBody<Bytes, BoxError>;
pub type IncomingRequest = Request<Body>;
pub type HttpResponse = Response<Body>;

#[derive(Debug, Clone)]
pub struct RestConfig {
    pub addr: SocketAddr,
    pub graceful_shutdown_timeout: Duration,
}

pub struct RestServer<S> {
    config: RestConfig,
    make_service: S,
}

#[allow(dead_code)]
pub struct RestService<S> {
    name: String,
    server: std::sync::Mutex<Option<RestServer<S>>>,
}

#[derive(Debug, Clone)]
pub struct RestLayerStack {
    config: RestConfig,
}

impl RestLayerStack {
    pub fn new(config: RestConfig) -> Self {
        Self { config }
    }

    pub fn config(&self) -> &RestConfig {
        &self.config
    }

    pub fn layer<S>(&self, service: S) -> S {
        service
    }

    pub fn into_layer(self) -> RestServiceLayer {
        RestServiceLayer {
            config: self.config,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RestServiceLayer {
    config: RestConfig,
}

impl<S> tower::Layer<S> for RestServiceLayer {
    type Service = S;

    fn layer(&self, inner: S) -> Self::Service {
        RestLayerStack::new(self.config.clone()).layer(inner)
    }
}

impl<S> RestService<S> {
    pub fn new(name: impl Into<String>, server: RestServer<S>) -> Self {
        Self {
            name: name.into(),
            server: std::sync::Mutex::new(Some(server)),
        }
    }
}

impl RuntimeService for RestService<SharedService<crate::Router>> {
    fn name(&self) -> &str {
        &self.name
    }

    fn start(&self, shutdown: roze_shutdown::ShutdownListener) -> ServiceFuture<'_> {
        let server = match self.server.lock() {
            Ok(mut guard) => guard.take(),
            Err(_) => {
                return Box::pin(async {
                    Err(anyhow::anyhow!("REST service state lock is poisoned"))
                })
            }
        };
        Box::pin(async move {
            let server = server.ok_or_else(|| anyhow::anyhow!("REST service already started"))?;
            serve_router_service(server, shutdown).await?;
            Ok(())
        })
    }
}

async fn serve_router_service(
    server: RestServer<SharedService<crate::Router>>,
    shutdown: roze_shutdown::ShutdownListener,
) -> std::io::Result<()> {
    let addr = server.config.addr;
    info!(addr = %addr, "REST server listening");

    let listener = TcpListener::bind(addr).await?;
    let router = server.make_service.service;
    let mut connections = JoinSet::new();
    loop {
        tokio::select! {
            _ = wait_for_shutdown_flag(shutdown.clone()) => break,
            joined = connections.join_next(), if !connections.is_empty() => {
                if let Some(Err(error)) = joined {
                    tracing::debug!(error = %error, "HTTP connection task failed");
                }
            }
            accepted = listener.accept() => {
                let (stream, _peer_addr) = accepted?;
                let io = TokioIo::new(stream);
                let service = TowerToHyperService::new(router.clone());
                connections.spawn(async move {
                    if let Err(error) = http1::Builder::new()
                        .serve_connection(io, service)
                        .with_upgrades()
                        .await
                    {
                        tracing::debug!(error = %error, "HTTP connection closed with error");
                    }
                });
            }
        }
    }
    drain_connections(&mut connections, server.config.graceful_shutdown_timeout).await;
    Ok(())
}

async fn wait_for_shutdown_flag(shutdown: roze_shutdown::ShutdownListener) {
    while !shutdown.is_triggered() {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

impl<S> RestServer<S> {
    pub fn from_make_service(addr: SocketAddr, make_service: S) -> Self {
        Self {
            config: RestConfig {
                addr,
                graceful_shutdown_timeout: Duration::from_secs(10),
            },
            make_service,
        }
    }

    pub fn with_make_service_config(config: RestConfig, make_service: S) -> Self {
        Self {
            config,
            make_service,
        }
    }

    pub fn config(&self) -> &RestConfig {
        &self.config
    }
}

impl<S> RestServer<SharedService<S>> {
    pub fn new(addr: SocketAddr, service: S) -> Self {
        Self::from_make_service(addr, SharedService::new(service))
    }

    pub fn with_config(config: RestConfig, service: S) -> Self {
        Self::with_make_service_config(config, SharedService::new(service))
    }
}

#[derive(Clone)]
pub struct SharedService<S> {
    service: S,
}

impl<S> SharedService<S> {
    pub fn new(service: S) -> Self {
        Self { service }
    }
}

impl<S, Target> Service<Target> for SharedService<S>
where
    S: Clone,
{
    type Response = S;
    type Error = Infallible;
    type Future = Ready<Result<S, Infallible>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, _target: Target) -> Self::Future {
        ready(Ok(self.service.clone()))
    }
}

impl<M, S> RestServer<M>
where
    M: Service<SocketAddr, Response = S, Error = Infallible> + Clone + Send + 'static,
    M::Future: Send + 'static,
    S: Service<IncomingRequest, Response = HttpResponse, Error = Infallible>
        + Clone
        + Send
        + 'static,
    S::Future: Send + 'static,
{
    pub async fn serve(self) -> std::io::Result<()> {
        self.serve_with_shutdown(shutdown_signal()).await
    }

    pub async fn serve_with_shutdown<F>(self, shutdown: F) -> std::io::Result<()>
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let addr = self.config.addr;
        info!(addr = %addr, "REST server listening");

        let listener = TcpListener::bind(addr).await?;
        let mut make_service = self.make_service.clone();
        let mut connections = JoinSet::new();
        let mut shutdown = std::pin::pin!(shutdown);
        loop {
            tokio::select! {
                _ = &mut shutdown => break,
                joined = connections.join_next(), if !connections.is_empty() => {
                    if let Some(Err(error)) = joined {
                        tracing::debug!(error = %error, "HTTP connection task failed");
                    }
                }
                accepted = listener.accept() => {
                    let (stream, peer_addr) = accepted?;
                    let io = TokioIo::new(stream);
                    let service = make_service
                        .ready()
                        .await
                        .expect("infallible make service")
                        .call(peer_addr)
                        .await
                        .expect("infallible make service");
                    let service = TowerToHyperService::new(service);
                    connections.spawn(async move {
                        if let Err(error) = http1::Builder::new()
                            .serve_connection(io, service)
                            .with_upgrades()
                            .await
                        {
                            tracing::debug!(error = %error, "HTTP connection closed with error");
                        }
                    });
                }
            }
        }
        drain_connections(&mut connections, self.config.graceful_shutdown_timeout).await;
        Ok(())
    }
}

#[derive(Clone)]
struct TowerToHyperService<S> {
    inner: S,
}

impl<S> TowerToHyperService<S> {
    fn new(inner: S) -> Self {
        Self { inner }
    }
}

impl<S> hyper::service::Service<Request<hyper::body::Incoming>> for TowerToHyperService<S>
where
    S: Service<IncomingRequest, Response = HttpResponse, Error = Infallible>
        + Clone
        + Send
        + 'static,
    S::Future: Send + 'static,
{
    type Response = HttpResponse;
    type Error = Infallible;
    type Future = S::Future;

    fn call(&self, request: Request<hyper::body::Incoming>) -> Self::Future {
        let mut service = self.inner.clone();
        let request: IncomingRequest = request.map(|body| body.map_err(BoxError::new).boxed());
        service.call(request)
    }
}

async fn drain_connections(connections: &mut JoinSet<()>, timeout: Duration) {
    if tokio::time::timeout(timeout, async {
        while connections.join_next().await.is_some() {}
    })
    .await
    .is_err()
    {
        connections.abort_all();
        while connections.join_next().await.is_some() {}
    }
}

pub fn empty_response(status: StatusCode) -> HttpResponse {
    Response::builder()
        .status(status)
        .body(empty_body())
        .expect("empty response")
}

pub fn text_response(status: StatusCode, text: impl Into<String>) -> HttpResponse {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(full_body(text.into()))
        .expect("text response")
}

pub fn json_response<T: Serialize>(status: StatusCode, value: &T) -> HttpResponse {
    match serde_json::to_vec(value) {
        Ok(bytes) => Response::builder()
            .status(status)
            .header(header::CONTENT_TYPE, "application/json")
            .body(full_body(bytes))
            .expect("json response"),
        Err(error) => text_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to serialize response: {error}"),
        ),
    }
}

pub fn error_response(error: &roze_error::RozeError) -> HttpResponse {
    if let Some(body) = error.fallback_body() {
        let mut response = json_response(error.status_code(), body);
        apply_fallback_headers(&mut response, error);
        return response;
    }
    if error.fallback_headers().is_some() {
        let mut response = json_response(error.status_code(), &error.response_body());
        apply_fallback_headers(&mut response, error);
        return response;
    }
    json_response(error.status_code(), &error.response_body())
}

fn apply_fallback_headers(response: &mut HttpResponse, error: &roze_error::RozeError) {
    let Some(headers) = error.fallback_headers() else {
        return;
    };
    for (key, value) in headers {
        let Ok(key) = HeaderName::try_from(key.as_str()) else {
            continue;
        };
        let Ok(value) = HeaderValue::try_from(value.as_str()) else {
            continue;
        };
        response.headers_mut().insert(key, value);
    }
}

pub fn api_response<T: Serialize>(value: &roze_result::ApiResponse<T>) -> HttpResponse {
    json_response(StatusCode::OK, value)
}

pub fn full_body(data: impl Into<Bytes>) -> Body {
    Full::new(data.into()).map_err(Into::into).boxed()
}

pub fn empty_body() -> Body {
    Full::new(Bytes::new()).map_err(Into::into).boxed()
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}

#[cfg(test)]
mod tests {
    use std::{convert::Infallible, net::SocketAddr, time::Duration};

    use http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::{service_fn, Layer};

    use super::{
        error_response, text_response, RestConfig, RestLayerStack, RestServer, SharedService,
    };

    #[test]
    fn rest_layer_stack_can_be_used_as_tower_layer() {
        let config = RestConfig {
            addr: SocketAddr::from(([127, 0, 0, 1], 0)),
            graceful_shutdown_timeout: Duration::from_secs(3),
        };
        let layer = RestLayerStack::new(config.clone()).into_layer();
        let service = service_fn(|_request: Request<super::Body>| async {
            Ok::<_, Infallible>(text_response(StatusCode::OK, "ok"))
        });

        let _service = layer.layer(service);
        assert_eq!(config.graceful_shutdown_timeout, Duration::from_secs(3));
    }

    #[test]
    fn rest_server_accepts_make_service() {
        let config = RestConfig {
            addr: SocketAddr::from(([127, 0, 0, 1], 0)),
            graceful_shutdown_timeout: Duration::from_secs(5),
        };
        let service = service_fn(|_request: Request<super::Body>| async {
            Ok::<_, Infallible>(text_response(StatusCode::OK, "ok"))
        });
        let server =
            RestServer::with_make_service_config(config.clone(), SharedService::new(service));

        assert_eq!(server.config().addr, config.addr);
    }

    #[tokio::test]
    async fn fallback_error_response_uses_configured_body_and_headers() {
        let mut headers = std::collections::BTreeMap::new();
        headers.insert("x-roze-fallback".to_string(), "route".to_string());
        let error = roze_error::RozeError::fallback_response(
            598,
            Some(serde_json::json!({"code": 503, "message": "degraded"})),
            headers,
        );

        let response = error_response(&error);

        assert_eq!(response.status(), StatusCode::from_u16(598).unwrap());
        assert_eq!(
            response.headers().get("x-roze-fallback"),
            Some(&http::HeaderValue::from_static("route"))
        );
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], br#"{"code":503,"message":"degraded"}"#);
    }
}
