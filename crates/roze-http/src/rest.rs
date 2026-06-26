use std::{future::Future, net::SocketAddr, time::Duration};

use axum::Router;
use roze_service::{RuntimeService, ServiceFuture};
use tokio::net::TcpListener;
use tracing::info;

#[derive(Debug, Clone)]
pub struct RestConfig {
    pub addr: SocketAddr,
    pub graceful_shutdown_timeout: Duration,
}

pub struct RestServer {
    config: RestConfig,
    endpoint: Router,
}

pub struct RestService {
    name: String,
    server: std::sync::Mutex<Option<RestServer>>,
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

    pub fn layer(&self, router: Router) -> Router {
        router
    }

    pub fn into_layer(self) -> RestRouterLayer {
        RestRouterLayer {
            config: self.config,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RestRouterLayer {
    config: RestConfig,
}

impl tower::Layer<Router> for RestRouterLayer {
    type Service = Router;

    fn layer(&self, inner: Router) -> Self::Service {
        RestLayerStack::new(self.config.clone()).layer(inner)
    }
}

impl RestService {
    pub fn new(name: impl Into<String>, server: RestServer) -> Self {
        Self {
            name: name.into(),
            server: std::sync::Mutex::new(Some(server)),
        }
    }
}

impl RuntimeService for RestService {
    fn name(&self) -> &str {
        &self.name
    }

    fn start(&self, shutdown: roze_shutdown::ShutdownListener) -> ServiceFuture<'_> {
        Box::pin(async move {
            let server = self
                .server
                .lock()
                .expect("rest service mutex")
                .take()
                .ok_or_else(|| anyhow::anyhow!("REST service {} already started", self.name))?;

            server
                .serve_with_shutdown(async move {
                    shutdown.wait().await;
                })
                .await
                .map_err(|error| anyhow::anyhow!("REST service {} failed: {error}", self.name))
        })
    }
}

impl RestServer {
    pub fn new(addr: SocketAddr, endpoint: Router) -> Self {
        Self {
            config: RestConfig {
                addr,
                graceful_shutdown_timeout: Duration::from_secs(10),
            },
            endpoint,
        }
    }

    pub fn with_config(config: RestConfig, endpoint: Router) -> Self {
        Self { config, endpoint }
    }

    pub fn config(&self) -> &RestConfig {
        &self.config
    }

    pub fn raw_router(&self) -> Router {
        self.endpoint.clone()
    }

    pub fn into_router(self) -> Router {
        RestLayerStack::new(self.config).layer(self.endpoint)
    }

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
        let service = self.into_router().into_make_service();
        axum::serve(listener, service)
            .with_graceful_shutdown(shutdown)
            .await
    }
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}

#[macro_export]
macro_rules! parse_json_request {
    ($result:expr) => {
        match $result {
            Ok(axum::Json(payload)) => payload,
            Err(err) => {
                return Err(roze_error::RozeError::BadRequest(err.to_string()));
            }
        }
    };
}

#[cfg(test)]
mod tests {
    use std::{
        net::SocketAddr,
        time::{Duration, Instant},
    };

    use axum::{routing::get, Router};
    use roze_service::ServiceGroup;
    use tower::{Layer, ServiceExt};

    use super::{RestConfig, RestLayerStack, RestServer, RestService};

    #[tokio::test]
    async fn rest_server_exposes_raw_and_layered_router() {
        let router = Router::new().route("/ready", get(|| async { "ok" }));
        let server = RestServer::new("127.0.0.1:0".parse().expect("addr"), router);

        let response = server
            .raw_router()
            .oneshot(
                axum::http::Request::builder()
                    .uri("/ready")
                    .body(axum::body::Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), axum::http::StatusCode::OK);

        let response = server
            .into_router()
            .oneshot(
                axum::http::Request::builder()
                    .uri("/ready")
                    .body(axum::body::Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), axum::http::StatusCode::OK);
    }

    #[test]
    fn rest_layer_stack_can_be_used_as_tower_layer() {
        let config = RestConfig {
            addr: SocketAddr::from(([127, 0, 0, 1], 0)),
            graceful_shutdown_timeout: Duration::from_secs(3),
        };
        let layer = RestLayerStack::new(config.clone()).into_layer();
        let router = Router::new();

        let _router = layer.layer(router);
        assert_eq!(config.graceful_shutdown_timeout, Duration::from_secs(3));
    }

    #[tokio::test]
    async fn rest_service_runs_inside_service_group() {
        let router = Router::new().route("/ready", get(|| async { "ok" }));
        let server = RestServer::new("127.0.0.1:0".parse().expect("addr"), router);
        let mut group = ServiceGroup::new();
        let handle = group.handle();

        group.add(RestService::new("rest", server));

        let started_at = Instant::now();
        let join = tokio::spawn(group.start_with_shutdown(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }));

        join.await
            .expect("service group should join")
            .expect("service group should stop cleanly");
        handle.shutdown();
        assert!(started_at.elapsed() < Duration::from_secs(1));
    }
}
