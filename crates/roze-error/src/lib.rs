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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodedErrorResponse {
    pub code: String,
    pub msg: String,
    pub data: Option<()>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct BusinessErrorCode {
    pub code: &'static str,
    pub status: u16,
}

impl BusinessErrorCode {
    pub const fn new(code: &'static str, status: u16) -> Self {
        Self { code, status }
    }
}

pub fn validate_business_error_catalog(catalog: &[BusinessErrorCode]) -> Result<(), String> {
    let mut codes = BTreeMap::new();
    for entry in catalog {
        if !is_valid_business_code(entry.code) {
            return Err(format!(
                "business error code {} must match DOMAIN-CATEGORY-NNN",
                entry.code
            ));
        }
        if !(400..=599).contains(&entry.status) {
            return Err(format!(
                "business error code {} has invalid HTTP status {}",
                entry.code, entry.status
            ));
        }
        if let Some(previous) = codes.insert(entry.code, entry.status) {
            if previous != entry.status {
                return Err(format!(
                    "business error code {} has conflicting HTTP statuses {} and {}",
                    entry.code, previous, entry.status
                ));
            }
        }
    }
    Ok(())
}

pub fn enforce_business_error_catalog(
    error: RozeError,
    catalog: &[BusinessErrorCode],
) -> RozeError {
    if catalog.is_empty() {
        return error;
    }
    let RozeError::Coded { status, code, .. } = &error else {
        return error;
    };
    if catalog
        .iter()
        .any(|entry| entry.code == code && entry.status == *status)
    {
        error
    } else {
        RozeError::Internal("unregistered business error".to_string())
    }
}

fn is_valid_business_code(code: &str) -> bool {
    let parts = code.split('-').collect::<Vec<_>>();
    parts.len() == 3
        && parts[0].len() >= 2
        && parts[1].len() >= 2
        && parts[2].len() == 3
        && parts[0..2].iter().all(|part| {
            part.bytes()
                .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
        })
        && parts[2].bytes().all(|byte| byte.is_ascii_digit())
}

#[derive(Debug, Clone, PartialEq, Eq, Error, Serialize, Deserialize)]
pub enum RozeError {
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("failed precondition: {0}")]
    FailedPrecondition(String),
    #[error("unauthorized")]
    Unauthorized,
    #[error("forbidden")]
    Forbidden,
    #[error("rate limited; retry after {retry_after_seconds}s")]
    RateLimited { retry_after_seconds: u64 },
    #[error("not found: {0}")]
    NotFound(String),
    #[error("service unavailable: {0}")]
    Unavailable(String),
    #[error("internal error: {0}")]
    Internal(String),
    #[error("coded error {code}: {message}")]
    Coded {
        status: u16,
        code: String,
        message: String,
        retry_after_seconds: Option<u64>,
    },
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
            RozeError::Conflict(_) => "conflict",
            RozeError::FailedPrecondition(_) => "failed_precondition",
            RozeError::Unauthorized => "unauthorized",
            RozeError::Forbidden => "forbidden",
            RozeError::RateLimited { .. } => "rate_limited",
            RozeError::NotFound(_) => "not_found",
            RozeError::Unavailable(_) => "unavailable",
            RozeError::Internal(_) => "internal",
            RozeError::Coded { .. } => "coded",
            RozeError::Fallback { .. } => "fallback",
        }
    }

    pub fn code(&self) -> i32 {
        match self {
            RozeError::BadRequest(_) => 400,
            RozeError::Conflict(_) => 409,
            RozeError::FailedPrecondition(_) => 412,
            RozeError::Unauthorized => 401,
            RozeError::Forbidden => 403,
            RozeError::RateLimited { .. } => 429,
            RozeError::NotFound(_) => 404,
            RozeError::Unavailable(_) => 503,
            RozeError::Internal(_) => 500,
            RozeError::Coded { status, .. } => i32::from(*status),
            RozeError::Fallback { status, .. } => i32::from(*status),
        }
    }

    pub fn message(&self) -> String {
        match self {
            RozeError::BadRequest(msg) => msg.clone(),
            RozeError::Conflict(msg) => msg.clone(),
            RozeError::FailedPrecondition(msg) => msg.clone(),
            RozeError::Unauthorized => "unauthorized".to_string(),
            RozeError::Forbidden => "forbidden".to_string(),
            RozeError::RateLimited { .. } => "rate limited".to_string(),
            RozeError::NotFound(msg) => msg.clone(),
            RozeError::Unavailable(msg) => msg.clone(),
            RozeError::Internal(msg) => msg.clone(),
            RozeError::Coded { message, .. } => message.clone(),
            RozeError::Fallback { body, .. } => fallback_message(body.as_ref()),
        }
    }

    pub fn message_i18n(&self, locale: impl AsRef<str>) -> String {
        match self {
            RozeError::BadRequest(msg) if !msg.is_empty() => msg.clone(),
            RozeError::Conflict(msg) if !msg.is_empty() => msg.clone(),
            RozeError::FailedPrecondition(msg) if !msg.is_empty() => msg.clone(),
            RozeError::NotFound(msg) if !msg.is_empty() => msg.clone(),
            RozeError::Unavailable(msg) if !msg.is_empty() => msg.clone(),
            RozeError::Internal(msg) if !msg.is_empty() => msg.clone(),
            RozeError::Coded { message, .. } => message.clone(),
            RozeError::Fallback { body, .. } => fallback_message(body.as_ref()),
            _ => localized_error_message(self.kind(), locale.as_ref()).to_string(),
        }
    }

    /// Returns a transport-safe message while retaining the full error in
    /// `Display` for server-side diagnostics.
    pub fn public_message_i18n(&self, locale: impl AsRef<str>) -> String {
        if matches!(self, RozeError::Internal(_)) {
            localized_error_message("internal", locale.as_ref()).to_string()
        } else {
            self.message_i18n(locale)
        }
    }

    pub fn is_client_error(&self) -> bool {
        matches!(
            self,
            RozeError::BadRequest(_)
                | RozeError::Conflict(_)
                | RozeError::FailedPrecondition(_)
                | RozeError::Unauthorized
                | RozeError::Forbidden
                | RozeError::RateLimited { .. }
                | RozeError::NotFound(_)
                | RozeError::Coded {
                    status: 400..=499,
                    ..
                }
        )
    }

    pub fn status_code(&self) -> StatusCode {
        match self {
            RozeError::BadRequest(_) => StatusCode::BAD_REQUEST,
            RozeError::Conflict(_) => StatusCode::CONFLICT,
            RozeError::FailedPrecondition(_) => StatusCode::PRECONDITION_FAILED,
            RozeError::Unauthorized => StatusCode::UNAUTHORIZED,
            RozeError::Forbidden => StatusCode::FORBIDDEN,
            RozeError::RateLimited { .. } => StatusCode::TOO_MANY_REQUESTS,
            RozeError::NotFound(_) => StatusCode::NOT_FOUND,
            RozeError::Unavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
            RozeError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
            RozeError::Coded { status, .. } => {
                StatusCode::from_u16(*status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR)
            }
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

    pub fn coded(status: StatusCode, code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Coded {
            status: status.as_u16(),
            code: code.into(),
            message: message.into(),
            retry_after_seconds: None,
        }
    }

    pub fn coded_rate_limited(
        code: impl Into<String>,
        message: impl Into<String>,
        retry_after: std::time::Duration,
    ) -> Self {
        let retry_after_seconds = retry_after.as_secs() + u64::from(retry_after.subsec_nanos() > 0);
        Self::Coded {
            status: StatusCode::TOO_MANY_REQUESTS.as_u16(),
            code: code.into(),
            message: message.into(),
            retry_after_seconds: Some(retry_after_seconds.max(1)),
        }
    }

    pub fn business_code(&self) -> Option<&str> {
        match self {
            Self::Coded { code, .. } => Some(code),
            _ => None,
        }
    }

    pub fn wire_code(&self) -> String {
        self.business_code()
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| self.code().to_string())
    }

    pub fn rate_limited(retry_after: std::time::Duration) -> Self {
        let retry_after_seconds = retry_after.as_secs() + u64::from(retry_after.subsec_nanos() > 0);
        Self::RateLimited {
            retry_after_seconds: retry_after_seconds.max(1),
        }
    }

    pub fn retry_after_seconds(&self) -> Option<u64> {
        match self {
            Self::RateLimited {
                retry_after_seconds,
            } => Some(*retry_after_seconds),
            Self::Coded {
                retry_after_seconds,
                ..
            } => *retry_after_seconds,
            _ => None,
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
            "conflict" => "资源冲突",
            "failed_precondition" => "前置条件不满足",
            "unauthorized" => "未认证或登录已失效",
            "forbidden" => "无权限访问",
            "not_found" => "资源不存在",
            "internal" => "服务器内部错误",
            _ => "服务器内部错误",
        },
        _ => match kind {
            "bad_request" => "bad request",
            "conflict" => "conflict",
            "failed_precondition" => "failed precondition",
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
            .map(|locale| self.public_message_i18n(locale))
            .unwrap_or_else(|| self.public_message_i18n("en-US"));
        let ids = current_request_ids();
        ErrorResponse {
            code: self.code(),
            msg: message,
            data: None,
            request_id: ids.as_ref().map(|ids| ids.request_id.clone()),
            trace_id: ids.as_ref().map(|ids| ids.trace_id.clone()),
        }
    }

    pub fn coded_response_body(&self) -> Option<CodedErrorResponse> {
        let code = self.business_code()?.to_string();
        let ids = current_request_ids();
        Some(CodedErrorResponse {
            code,
            msg: self.message(),
            data: None,
            request_id: ids.as_ref().map(|ids| ids.request_id.clone()),
            trace_id: ids.as_ref().map(|ids| ids.trace_id.clone()),
        })
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
        assert_eq!(RozeError::Conflict("x".into()).code(), 409);
        assert_eq!(RozeError::FailedPrecondition("x".into()).code(), 412);
        assert_eq!(RozeError::Unauthorized.code(), 401);
        assert_eq!(RozeError::Forbidden.code(), 403);
        assert_eq!(
            RozeError::rate_limited(std::time::Duration::from_secs(3)).code(),
            429
        );
        assert_eq!(RozeError::NotFound("x".into()).code(), 404);
        assert_eq!(RozeError::Unavailable("x".into()).code(), 503);
        assert_eq!(RozeError::Internal("x".into()).code(), 500);
        assert_eq!(
            RozeError::coded(
                StatusCode::UNPROCESSABLE_ENTITY,
                "RISK-REJECT-001",
                "rejected"
            )
            .code(),
            422
        );
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
    fn redacts_internal_details_from_transport_responses() {
        let error = RozeError::Internal("database password appeared in an error".to_string());
        assert_eq!(error.message(), "database password appeared in an error");
        assert_eq!(error.response_body().msg, "internal server error");
    }

    #[test]
    fn builds_string_coded_error_response_without_changing_legacy_codes() {
        let error = RozeError::coded(StatusCode::NOT_FOUND, "ORD-NFD-001", "order not found");
        assert_eq!(error.kind(), "coded");
        assert_eq!(error.status_code(), StatusCode::NOT_FOUND);
        assert_eq!(error.code(), 404);
        assert_eq!(error.wire_code(), "ORD-NFD-001");
        assert_eq!(
            error.coded_response_body().expect("coded body"),
            CodedErrorResponse {
                code: "ORD-NFD-001".to_string(),
                msg: "order not found".to_string(),
                data: None,
                request_id: None,
                trace_id: None,
            }
        );
        assert_eq!(RozeError::NotFound("missing".into()).wire_code(), "404");
    }

    #[test]
    fn coded_rate_limit_preserves_retry_after() {
        let error = RozeError::coded_rate_limited(
            "COM-LIMIT-001",
            "too many requests",
            std::time::Duration::from_millis(1_001),
        );
        assert_eq!(error.status_code(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(error.retry_after_seconds(), Some(2));
    }

    #[test]
    fn validates_and_enforces_bounded_business_error_catalogs() {
        const CATALOG: &[BusinessErrorCode] = &[
            BusinessErrorCode::new("ORD-NFD-001", 404),
            BusinessErrorCode::new("COM-DEP-002", 502),
        ];
        assert!(validate_business_error_catalog(CATALOG).is_ok());
        assert!(validate_business_error_catalog(&[BusinessErrorCode::new("bad", 404)]).is_err());

        let declared = RozeError::coded(StatusCode::NOT_FOUND, "ORD-NFD-001", "missing");
        assert_eq!(
            enforce_business_error_catalog(declared.clone(), CATALOG),
            declared
        );
        let unknown = RozeError::coded(StatusCode::NOT_FOUND, "ORD-NFD-999", "secret detail");
        assert_eq!(
            enforce_business_error_catalog(unknown, CATALOG),
            RozeError::Internal("unregistered business error".to_string())
        );
    }

    #[test]
    fn serializes_semantic_conflict_responses() {
        for (error, kind, code, status) in [
            (
                RozeError::Conflict("device already bound".into()),
                "conflict",
                409,
                StatusCode::CONFLICT,
            ),
            (
                RozeError::FailedPrecondition("stale version".into()),
                "failed_precondition",
                412,
                StatusCode::PRECONDITION_FAILED,
            ),
        ] {
            assert_eq!(error.kind(), kind);
            assert_eq!(error.status_code(), status);
            let body = error.response_body();
            assert_eq!(body.code, code);
            assert_eq!(body.msg, error.message());
        }
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
