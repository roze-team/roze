use std::{net::SocketAddr, time::Duration};

use poem::{listener::TcpListener, Endpoint, Server};
use tracing::info;

#[derive(Debug, Clone)]
pub struct RestConfig {
    pub addr: SocketAddr,
    pub graceful_shutdown_timeout: Duration,
}

pub struct RestServer<E> {
    config: RestConfig,
    endpoint: E,
}

impl<E> RestServer<E>
where
    E: Endpoint + 'static,
{
    pub fn new(addr: SocketAddr, endpoint: E) -> Self {
        Self {
            config: RestConfig {
                addr,
                graceful_shutdown_timeout: Duration::from_secs(10),
            },
            endpoint,
        }
    }

    pub fn with_config(config: RestConfig, endpoint: E) -> Self {
        Self { config, endpoint }
    }

    pub async fn serve(self) -> std::io::Result<()> {
        info!(addr = %self.config.addr, "REST server listening");

        Server::new(TcpListener::bind(self.config.addr))
            .run_with_graceful_shutdown(
                self.endpoint,
                shutdown_signal(),
                Some(self.config.graceful_shutdown_timeout),
            )
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
            Ok(poem::web::Json(payload)) => payload,
            Err(err) => {
                return Err(roze_error::RozeError::BadRequest(err.to_string()));
            }
        }
    };
}
