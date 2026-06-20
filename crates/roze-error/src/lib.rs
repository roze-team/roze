use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

tokio::task_local! {
    static REQUEST_LOCALE: String;
}

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
    pub fn kind(&self) -> &'static str {
        match self {
            RozeError::BadRequest(_) => "bad_request",
            RozeError::Unauthorized => "unauthorized",
            RozeError::NotFound(_) => "not_found",
            RozeError::Internal(_) => "internal",
        }
    }

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

    pub fn message_i18n(&self, locale: impl AsRef<str>) -> String {
        match self {
            RozeError::BadRequest(msg) if !msg.is_empty() => msg.clone(),
            RozeError::NotFound(msg) if !msg.is_empty() => msg.clone(),
            RozeError::Internal(msg) if !msg.is_empty() => msg.clone(),
            _ => localized_error_message(self.kind(), locale.as_ref()).to_string(),
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

pub fn localized_error_message(kind: &str, locale: &str) -> &'static str {
    match normalize_locale(locale).as_deref() {
        Some("zh-CN") => match kind {
            "bad_request" => "请求参数错误",
            "unauthorized" => "未认证或登录已失效",
            "not_found" => "资源不存在",
            "internal" => "服务器内部错误",
            _ => "服务器内部错误",
        },
        _ => match kind {
            "bad_request" => "bad request",
            "unauthorized" => "unauthorized",
            "not_found" => "not found",
            "internal" => "internal server error",
            _ => "internal server error",
        },
    }
}

pub fn locale_from_accept_language(raw: &str) -> Option<String> {
    raw.split(',')
        .filter_map(|part| part.trim().split(';').next())
        .find_map(normalize_locale)
}

pub fn normalize_locale(raw: &str) -> Option<String> {
    let normalized = raw.trim().replace('_', "-");
    if normalized.is_empty() || normalized == "*" {
        return None;
    }
    let lower = normalized.to_ascii_lowercase();
    if lower == "zh" || lower.starts_with("zh-cn") || lower.starts_with("zh-hans") {
        return Some("zh-CN".to_string());
    }
    if lower == "en" || lower.starts_with("en-us") || lower.starts_with("en-") {
        return Some("en-US".to_string());
    }
    None
}

impl IntoResponse for RozeError {
    fn into_response(self) -> Response {
        let status = self.status_code();
        let message = current_locale()
            .map(|locale| self.message_i18n(locale))
            .unwrap_or_else(|| self.message());
        let body = roze_result::ApiResponse::<()>::error(self.code(), message);
        (status, axum::Json(body)).into_response()
    }
}

pub async fn scope_locale<F>(locale: Option<String>, future: F) -> F::Output
where
    F: std::future::Future,
{
    match locale {
        Some(locale) => REQUEST_LOCALE.scope(locale, future).await,
        None => future.await,
    }
}

pub fn current_locale() -> Option<String> {
    REQUEST_LOCALE.try_with(Clone::clone).ok()
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

    #[tokio::test]
    async fn converts_to_localized_axum_error_response() {
        let resp = scope_locale(Some("zh-CN".to_string()), async {
            axum::response::IntoResponse::into_response(RozeError::Unauthorized)
        })
        .await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn localizes_standard_error_messages() {
        assert_eq!(
            RozeError::Unauthorized.message_i18n("zh-CN"),
            "未认证或登录已失效"
        );
        assert_eq!(
            RozeError::Unauthorized.message_i18n("en-US"),
            "unauthorized"
        );
        assert_eq!(
            locale_from_accept_language("zh-CN,zh;q=0.9,en;q=0.8").as_deref(),
            Some("zh-CN")
        );
    }
}
