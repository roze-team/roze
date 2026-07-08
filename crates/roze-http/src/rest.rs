use std::{convert::Infallible, error::Error, future::Future, net::SocketAddr, time::Duration};

use bytes::Bytes;
use http::{header, Request, Response, StatusCode};
use http_body_util::{combinators::BoxBody, BodyExt, Full};
use hyper::{server::conn::http1, service::service_fn};
use hyper_util::rt::TokioIo;
use serde::Serialize;
use tokio::net::TcpListener;
use tower::{Service, ServiceExt};
use tracing::info;

pub type BoxError = Box<dyn Error + Send + Sync + 'static>;
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
    service: S,
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

impl<S> RestServer<S> {
    pub fn new(addr: SocketAddr, service: S) -> Self {
        Self {
            config: RestConfig {
                addr,
                graceful_shutdown_timeout: Duration::from_secs(10),
            },
            service,
        }
    }

    pub fn with_config(config: RestConfig, service: S) -> Self {
        Self { config, service }
    }

    pub fn config(&self) -> &RestConfig {
        &self.config
    }
}

impl<S> RestServer<S>
where
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
        let service = self.service.boxed_clone();
        let mut shutdown = std::pin::pin!(shutdown);
        loop {
            tokio::select! {
                _ = &mut shutdown => break,
                accepted = listener.accept() => {
                    let (stream, _) = accepted?;
                    let io = TokioIo::new(stream);
                    let service = service.clone();
                    let service = service_fn(move |request: Request<hyper::body::Incoming>| {
                        let service = service.clone();
                        async move {
                            let request: IncomingRequest = request.map(|body| {
                                body.map_err(|error| -> BoxError { Box::new(error) }).boxed()
                            });
                            service.oneshot(request).await
                        }
                    });
                    if let Err(error) = http1::Builder::new().serve_connection(io, service).await {
                        tracing::debug!(error = %error, "HTTP connection closed with error");
                    }
                }
            }
        }
        Ok(())
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
    json_response(error.status_code(), &error.response_body())
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
    use tower::{service_fn, Layer};

    use super::{text_response, RestConfig, RestLayerStack};

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
}
