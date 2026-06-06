use serde::{Deserialize, Serialize};
use poem::{error::ResponseError, http::StatusCode, IntoResponse, Response};
use poem::web::Json;
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
        matches!(self, RozeError::BadRequest(_) | RozeError::Unauthorized | RozeError::NotFound(_))
    }
}

impl ResponseError for RozeError {
    fn status(&self) -> StatusCode {
        match self {
            RozeError::BadRequest(_) => StatusCode::BAD_REQUEST,
            RozeError::Unauthorized => StatusCode::UNAUTHORIZED,
            RozeError::NotFound(_) => StatusCode::NOT_FOUND,
            RozeError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    fn as_response(&self) -> Response {
        let code = self.code();
        let msg = self.message();
        Json(roze_result::ApiResponse::<()>::error(code, msg)).into_response()
    }
}

impl From<tonic::Status> for RozeError {
    fn from(status: tonic::Status) -> Self {
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
}
