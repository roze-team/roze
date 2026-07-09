use std::collections::BTreeMap;

use http::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::Value;
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
    #[error("rate limited")]
    RateLimited,
    #[error("not found: {0}")]
    NotFound(String),
    #[error("service unavailable: {0}")]
    Unavailable(String),
    #[error("internal error: {0}")]
    Internal(String),
    #[error("fallback response: {status}")]
    Fallback {
        status: u16,
        body: Option<Value>,
        headers: BTreeMap<String, String>,
    },
}

impl RozeError {
    pub fn kind(&self) -> &'static str {
        match self {
            RozeError::BadRequest(_) => "bad_request",
            RozeError::Unauthorized => "unauthorized",
            RozeError::Forbidden => "forbidden",
            RozeError::RateLimited => "rate_limited",
            RozeError::NotFound(_) => "not_found",
            RozeError::Unavailable(_) => "unavailable",
            RozeError::Internal(_) => "internal",
            RozeError::Fallback { .. } => "fallback",
        }
    }

    pub fn code(&self) -> i32 {
        match self {
            RozeError::BadRequest(_) => 400,
            RozeError::Unauthorized => 401,
            RozeError::Forbidden => 403,
            RozeError::RateLimited => 429,
            RozeError::NotFound(_) => 404,
            RozeError::Unavailable(_) => 503,
            RozeError::Internal(_) => 500,
            RozeError::Fallback { status, .. } => i32::from(*status),
        }
    }

    pub fn message(&self) -> String {
        match self {
            RozeError::BadRequest(msg) => msg.clone(),
            RozeError::Unauthorized => "unauthorized".to_string(),
            RozeError::Forbidden => "forbidden".to_string(),
            RozeError::RateLimited => "rate limited".to_string(),
            RozeError::NotFound(msg) => msg.clone(),
            RozeError::Unavailable(msg) => msg.clone(),
            RozeError::Internal(msg) => msg.clone(),
            RozeError::Fallback { body, .. } => fallback_message(body.as_ref()),
        }
    }

    pub fn message_i18n(&self, locale: impl AsRef<str>) -> String {
        match self {
            RozeError::BadRequest(msg) if !msg.is_empty() => msg.clone(),
            RozeError::NotFound(msg) if !msg.is_empty() => msg.clone(),
            RozeError::Unavailable(msg) if !msg.is_empty() => msg.clone(),
            RozeError::Internal(msg) if !msg.is_empty() => msg.clone(),
            RozeError::Fallback { body, .. } => fallback_message(body.as_ref()),
            _ => localized_error_message(self.kind(), locale.as_ref()).to_string(),
        }
    }

    pub fn is_client_error(&self) -> bool {
        matches!(
            self,
            RozeError::BadRequest(_)
                | RozeError::Unauthorized
                | RozeError::Forbidden
                | RozeError::RateLimited
                | RozeError::NotFound(_)
        )
    }

    pub fn status_code(&self) -> StatusCode {
        match self {
            RozeError::BadRequest(_) => StatusCode::BAD_REQUEST,
            RozeError::Unauthorized => StatusCode::UNAUTHORIZED,
            RozeError::Forbidden => StatusCode::FORBIDDEN,
            RozeError::RateLimited => StatusCode::TOO_MANY_REQUESTS,
            RozeError::NotFound(_) => StatusCode::NOT_FOUND,
            RozeError::Unavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
            RozeError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
            RozeError::Fallback { status, .. } => {
                StatusCode::from_u16(*status).unwrap_or(StatusCode::SERVICE_UNAVAILABLE)
            }
        }
    }

    pub fn fallback_response(
        status: u16,
        body: Option<Value>,
        headers: BTreeMap<String, String>,
    ) -> Self {
        RozeError::Fallback {
            status,
            body,
            headers,
        }
    }

    pub fn fallback_body(&self) -> Option<&Value> {
        match self {
            RozeError::Fallback { body, .. } => body.as_ref(),
            _ => None,
        }
    }

    pub fn fallback_headers(&self) -> Option<&BTreeMap<String, String>> {
        match self {
            RozeError::Fallback { headers, .. } => Some(headers),
            _ => None,
        }
    }
}

fn fallback_message(body: Option<&Value>) -> String {
    body.and_then(|body| body.get("message").and_then(Value::as_str))
        .or_else(|| body.and_then(|body| body.get("msg").and_then(Value::as_str)))
        .unwrap_or("fallback")
        .to_string()
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
            "rate_limited" => "rate limited",
            "not_found" => "not found",
            "unavailable" => "service unavailable",
            "fallback" => "fallback",
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
        assert_eq!(RozeError::RateLimited.code(), 429);
        assert_eq!(RozeError::NotFound("x".into()).code(), 404);
        assert_eq!(RozeError::Unavailable("x".into()).code(), 503);
        assert_eq!(RozeError::Internal("x".into()).code(), 500);
        assert_eq!(
            RozeError::fallback_response(598, None, BTreeMap::new()).code(),
            598
        );
    }

    #[test]
    fn builds_error_response_body() {
        let err = RozeError::Unauthorized;
        assert_eq!(err.status_code(), StatusCode::UNAUTHORIZED);
        assert_eq!(err.response_body().code, 401);
    }

    #[test]
    fn builds_fallback_error() {
        let mut headers = BTreeMap::new();
        headers.insert("x-roze-fallback".to_string(), "route".to_string());
        let err = RozeError::fallback_response(
            598,
            Some(serde_json::json!({"message": "degraded"})),
            headers,
        );

        assert_eq!(err.kind(), "fallback");
        assert_eq!(err.status_code(), StatusCode::from_u16(598).unwrap());
        assert_eq!(err.message(), "degraded");
        assert_eq!(
            err.fallback_headers()
                .and_then(|headers| headers.get("x-roze-fallback"))
                .map(String::as_str),
            Some("route")
        );
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
