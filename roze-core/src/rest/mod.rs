use std::{net::SocketAddr, time::Duration};

use poem::{
    error::ResponseError, http::StatusCode, listener::TcpListener, web::Json, Endpoint,
    IntoResponse, Response, Server,
};
use serde::Serialize;
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

#[derive(Debug, Clone, Serialize)]
pub struct ApiResponse<T>
where
    T: Serialize,
{
    pub code: i32,
    pub msg: String,
    pub data: Option<T>,
}

impl<T> ApiResponse<T>
where
    T: Serialize,
{
    pub fn ok(data: T) -> Self {
        Self {
            code: 0,
            msg: "OK".to_string(),
            data: Some(data),
        }
    }

    pub fn error(code: i32, msg: impl Into<String>) -> Self {
        Self {
            code,
            msg: msg.into(),
            data: None,
        }
    }
}

impl<T> IntoResponse for ApiResponse<T>
where
    T: Serialize + Send,
{
    fn into_response(self) -> Response {
        Json(self).into_response()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("unauthorized")]
    Unauthorized,
    #[error("not found: {0}")]
    NotFound(String),
    #[error("internal error: {0}")]
    Internal(String),
}

impl ResponseError for AppError {
    fn status(&self) -> StatusCode {
        match self {
            AppError::BadRequest(_) => StatusCode::BAD_REQUEST,
            AppError::Unauthorized => StatusCode::UNAUTHORIZED,
            AppError::NotFound(_) => StatusCode::NOT_FOUND,
            AppError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    fn as_response(&self) -> Response {
        let (code, msg) = match self {
            AppError::BadRequest(msg) => (400, msg.clone()),
            AppError::Unauthorized => (401, "unauthorized".to_string()),
            AppError::NotFound(msg) => (404, msg.clone()),
            AppError::Internal(msg) => (500, msg.clone()),
        };

        Json(ApiResponse::<()>::error(code, msg)).into_response()
    }
}

impl From<tonic::Status> for AppError {
    fn from(status: tonic::Status) -> Self {
        Self::Internal(status.to_string())
    }
}

#[macro_export]
macro_rules! parse_json_request {
    ($result:expr) => {
        match $result {
            Ok(poem::web::Json(payload)) => payload,
            Err(err) => {
                return Err($crate::rest::AppError::BadRequest(err.to_string()));
            }
        }
    };
}
