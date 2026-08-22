//! Provider-neutral SMS delivery primitives.
//!
//! This crate delivers an already-renderable template request. OTP creation,
//! storage, verification, rate limiting, idempotency, and outbox orchestration
//! remain application responsibilities.

mod aliyun;
mod config;
mod mock;

pub use aliyun::AliyunSmsProvider;
pub use config::{AliyunSmsConfig, SmsRetryConfig};
pub use mock::{MockSmsProvider, MockSmsResponse};

use async_trait::async_trait;
use roze_error::RozeError;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::{fmt, time::Duration};

#[async_trait]
pub trait SmsProvider: Send + Sync + fmt::Debug {
    async fn send(&self, message: SmsMessage) -> Result<SmsSendResult, SmsError>;

    /// Readiness checks configuration/client construction only. It never sends.
    fn health_check(&self) -> Result<(), SmsError> {
        Ok(())
    }
}

#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub struct SmsMessage {
    pub phone_numbers: Vec<String>,
    pub sign_name: String,
    pub template_id: String,
    #[serde(default)]
    pub template_params: Map<String, Value>,
    #[serde(default)]
    pub out_id: Option<String>,
}

impl SmsMessage {
    pub fn new(phone_numbers: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            phone_numbers: phone_numbers.into_iter().map(Into::into).collect(),
            sign_name: String::new(),
            template_id: String::new(),
            template_params: Map::new(),
            out_id: None,
        }
    }
}

impl fmt::Debug for SmsMessage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SmsMessage")
            .field("phone_number_count", &self.phone_numbers.len())
            .field("sign_name", &"[REDACTED]")
            .field("template_id", &"[REDACTED]")
            .field("template_params", &"[REDACTED]")
            .field("out_id", &self.out_id.as_ref().map(|_| "[REDACTED]"))
            .finish()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SmsSendResult {
    pub provider: String,
    pub provider_code: String,
    pub provider_message: Option<String>,
    pub biz_id: Option<String>,
    pub request_id: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SmsErrorCategory {
    Configuration,
    Authentication,
    RateLimit,
    InvalidParameter,
    ProviderRejected,
    Network,
    Timeout,
    UnknownOutcome,
}

impl SmsErrorCategory {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Configuration => "configuration",
            Self::Authentication => "authentication",
            Self::RateLimit => "rate_limit",
            Self::InvalidParameter => "invalid_parameter",
            Self::ProviderRejected => "provider_rejected",
            Self::Network => "network",
            Self::Timeout => "timeout",
            Self::UnknownOutcome => "unknown_outcome",
        }
    }
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SmsError {
    pub category: SmsErrorCategory,
    pub provider: Option<Box<str>>,
    pub provider_code: Option<Box<str>>,
    pub provider_message: Option<Box<str>>,
    pub request_id: Option<Box<str>>,
    pub biz_id: Option<Box<str>>,
    pub retry_after: Option<Duration>,
}

impl SmsError {
    pub fn new(category: SmsErrorCategory) -> Self {
        Self {
            category,
            provider: None,
            provider_code: None,
            provider_message: None,
            request_id: None,
            biz_id: None,
            retry_after: None,
        }
    }

    pub fn into_roze_error(self) -> RozeError {
        match self.category {
            SmsErrorCategory::Authentication => RozeError::Unauthorized,
            SmsErrorCategory::RateLimit => {
                RozeError::rate_limited(self.retry_after.unwrap_or(Duration::from_secs(1)))
            }
            SmsErrorCategory::InvalidParameter => {
                RozeError::BadRequest("SMS request is invalid".to_string())
            }
            SmsErrorCategory::ProviderRejected => {
                RozeError::FailedPrecondition("SMS provider rejected the request".to_string())
            }
            SmsErrorCategory::Configuration => {
                RozeError::Internal("SMS provider configuration is invalid".to_string())
            }
            SmsErrorCategory::Network
            | SmsErrorCategory::Timeout
            | SmsErrorCategory::UnknownOutcome => {
                RozeError::Unavailable("SMS provider is unavailable".to_string())
            }
        }
    }
}

impl fmt::Debug for SmsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SmsError")
            .field("category", &self.category)
            .field("provider", &self.provider)
            .field("provider_code", &self.provider_code)
            .field("request_id", &self.request_id)
            .field("biz_id", &self.biz_id)
            .field("retry_after", &self.retry_after)
            .finish_non_exhaustive()
    }
}

impl fmt::Display for SmsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "SMS delivery failed ({})",
            self.category.as_str()
        )
    }
}

impl std::error::Error for SmsError {}

impl From<SmsError> for RozeError {
    fn from(error: SmsError) -> Self {
        error.into_roze_error()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_debug_and_display_do_not_expose_provider_messages() {
        let mut error = SmsError::new(SmsErrorCategory::ProviderRejected);
        error.provider_message = Some("phone=13900000000 code=123456".into());
        let rendered = format!("{error:?} {error}");
        assert!(!rendered.contains("13900000000"));
        assert!(!rendered.contains("123456"));
    }
}
