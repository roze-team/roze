use http::StatusCode;
use serde::{Deserialize, Serialize};
use thiserror::Error;

tokio::task_local! {
    static REQUEST_LOCALE: String;
    static REQUEST_IDS: RequestIds;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestIds {
    pub request_id: String,
    pub trace_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorResponse {
    pub code: i32,
    pub msg: String,
    pub data: Option<()>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Error, Serialize, Deserialize)]
pub enum RozeError {
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("unauthorized")]
    Unauthorized,
    #[error("forbidden")]
    Forbidden,
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
            RozeError::Forbidden => "forbidden",
            RozeError::NotFound(_) => "not_found",
            RozeError::Internal(_) => "internal",
        }
    }

    pub fn code(&self) -> i32 {
        match self {
            RozeError::BadRequest(_) => 400,
            RozeError::Unauthorized => 401,
            RozeError::Forbidden => 403,
            RozeError::NotFound(_) => 404,
            RozeError::Internal(_) => 500,
        }
    }

    pub fn message(&self) -> String {
        match self {
            RozeError::BadRequest(msg) => msg.clone(),
            RozeError::Unauthorized => "unauthorized".to_string(),
            RozeError::Forbidden => "forbidden".to_string(),
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
            RozeError::BadRequest(_)
                | RozeError::Unauthorized
                | RozeError::Forbidden
                | RozeError::NotFound(_)
        )
    }

    pub fn status_code(&self) -> StatusCode {
        match self {
            RozeError::BadRequest(_) => StatusCode::BAD_REQUEST,
            RozeError::Unauthorized => StatusCode::UNAUTHORIZED,
            RozeError::Forbidden => StatusCode::FORBIDDEN,
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
            "forbidden" => "无权限访问",
            "not_found" => "资源不存在",
            "internal" => "服务器内部错误",
            _ => "服务器内部错误",
        },
        _ => match kind {
            "bad_request" => "bad request",
            "unauthorized" => "unauthorized",
            "forbidden" => "forbidden",
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

impl RozeError {
    pub fn response_body(&self) -> ErrorResponse {
        let message = current_locale()
            .map(|locale| self.message_i18n(locale))
            .unwrap_or_else(|| self.message());
        let ids = current_request_ids();
        ErrorResponse {
            code: self.code(),
            msg: message,
            data: None,
            request_id: ids.as_ref().map(|ids| ids.request_id.clone()),
            trace_id: ids.as_ref().map(|ids| ids.trace_id.clone()),
        }
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

pub async fn scope_error_context<F>(
    locale: Option<String>,
    request_id: Option<String>,
    trace_id: Option<String>,
    future: F,
) -> F::Output
where
    F: std::future::Future,
{
    match (locale, request_id, trace_id) {
        (Some(locale), Some(request_id), Some(trace_id)) => {
            REQUEST_LOCALE
                .scope(
                    locale,
                    REQUEST_IDS.scope(
                        RequestIds {
                            request_id,
                            trace_id,
                        },
                        future,
                    ),
                )
                .await
        }
        (Some(locale), _, _) => REQUEST_LOCALE.scope(locale, future).await,
        (None, Some(request_id), Some(trace_id)) => {
            REQUEST_IDS
                .scope(
                    RequestIds {
                        request_id,
                        trace_id,
                    },
                    future,
                )
                .await
        }
        (None, _, _) => future.await,
    }
}

pub fn current_locale() -> Option<String> {
    REQUEST_LOCALE.try_with(Clone::clone).ok()
}

pub fn current_request_ids() -> Option<RequestIds> {
    REQUEST_IDS.try_with(Clone::clone).ok()
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
        assert_eq!(RozeError::Forbidden.code(), 403);
        assert_eq!(RozeError::NotFound("x".into()).code(), 404);
        assert_eq!(RozeError::Internal("x".into()).code(), 500);
    }

    #[test]
    fn builds_error_response_body() {
        let err = RozeError::Unauthorized;
        assert_eq!(err.status_code(), StatusCode::UNAUTHORIZED);
        assert_eq!(err.response_body().code, 401);
    }

    #[tokio::test]
    async fn builds_localized_error_response_body() {
        let body = scope_locale(Some("zh-CN".to_string()), async {
            RozeError::Unauthorized.response_body()
        })
        .await;
        assert_eq!(body.code, 401);
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
