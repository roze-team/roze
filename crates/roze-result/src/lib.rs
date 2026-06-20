use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ApiResponse<T> {
    pub code: i32,
    pub msg: String,
    pub data: Option<T>,
}

impl<T> ApiResponse<T> {
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

pub type ApiResult<T> = Result<ApiResponse<T>, ()>;

impl<T> IntoResponse for ApiResponse<T>
where
    T: Serialize + Send,
{
    fn into_response(self) -> Response {
        axum::Json(self).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_ok_response() {
        let resp = ApiResponse::ok(123);
        assert_eq!(resp.code, 0);
        assert_eq!(resp.msg, "OK");
        assert_eq!(resp.data, Some(123));
    }

    #[test]
    fn converts_to_axum_response() {
        let resp = axum::response::IntoResponse::into_response(ApiResponse::ok(123));
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
    }
}
