use std::{net::SocketAddr, time::Duration};

use axum::Router;
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

    pub async fn serve(self) -> std::io::Result<()> {
        info!(addr = %self.config.addr, "REST server listening");

        let listener = TcpListener::bind(self.config.addr).await?;
        let service = self.endpoint.into_make_service();
        axum::serve(listener, service)
            .with_graceful_shutdown(shutdown_signal())
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
