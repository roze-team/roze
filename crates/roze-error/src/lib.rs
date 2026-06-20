use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Error, Serialize, Deserialize)]
pub enum RozeError {
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("unauthorized")]
    Unauthorized,
    #[error("not found: {0}")]
    NotFound(String),
    #[error("internal error: {0}")]
    Internal(String),
}

impl RozeError {
    pub fn code(&self) -> i32 {
        match self {
            RozeError::BadRequest(_) => 400,
            RozeError::Unauthorized => 401,
            RozeError::NotFound(_) => 404,
            RozeError::Internal(_) => 500,
        }
    }

    pub fn message(&self) -> String {
        match self {
            RozeError::BadRequest(msg) => msg.clone(),
            RozeError::Unauthorized => "unauthorized".to_string(),
            RozeError::NotFound(msg) => msg.clone(),
            RozeError::Internal(msg) => msg.clone(),
        }
    }

    pub fn is_client_error(&self) -> bool {
        matches!(
            self,
            RozeError::BadRequest(_) | RozeError::Unauthorized | RozeError::NotFound(_)
        )
    }

    pub fn status_code(&self) -> StatusCode {
        match self {
            RozeError::BadRequest(_) => StatusCode::BAD_REQUEST,
            RozeError::Unauthorized => StatusCode::UNAUTHORIZED,
            RozeError::NotFound(_) => StatusCode::NOT_FOUND,
            RozeError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl IntoResponse for RozeError {
    fn into_response(self) -> Response {
        let status = self.status_code();
        let body = roze_result::ApiResponse::<()>::error(self.code(), self.message());
        (status, axum::Json(body)).into_response()
    }
}

impl From<roze_grpc::transport::Status> for RozeError {
    fn from(status: roze_grpc::transport::Status) -> Self {
        Self::Internal(status.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derives_status_codes() {
        assert_eq!(RozeError::BadRequest("x".into()).code(), 400);
        assert_eq!(RozeError::Unauthorized.code(), 401);
        assert_eq!(RozeError::NotFound("x".into()).code(), 404);
        assert_eq!(RozeError::Internal("x".into()).code(), 500);
    }

    #[test]
    fn converts_to_axum_error_response() {
        let resp = axum::response::IntoResponse::into_response(RozeError::Unauthorized);
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }
}
