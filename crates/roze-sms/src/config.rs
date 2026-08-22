use crate::{SmsError, SmsErrorCategory};
use serde::{Deserialize, Serialize};
use std::{fmt, time::Duration};

fn default_endpoint() -> String {
    "dysmsapi.aliyuncs.com".to_string()
}
fn default_region() -> String {
    "cn-hangzhou".to_string()
}
fn default_connect_timeout() -> Duration {
    Duration::from_secs(1)
}
fn default_request_timeout() -> Duration {
    Duration::from_secs(3)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct SmsRetryConfig {
    pub enabled: bool,
    /// Total attempts, including the initial request.
    pub max_attempts: u8,
    pub base_backoff_ms: u64,
    pub max_backoff_ms: u64,
}

impl Default for SmsRetryConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_attempts: 2,
            base_backoff_ms: 50,
            max_backoff_ms: 500,
        }
    }
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AliyunSmsConfig {
    pub access_key_id: String,
    pub access_key_secret: String,
    #[serde(default = "default_endpoint")]
    pub endpoint: String,
    #[serde(default = "default_region")]
    pub region: String,
    pub sign_name: String,
    pub template_id: String,
    #[serde(default = "default_connect_timeout", with = "duration_serde")]
    pub connect_timeout: Duration,
    #[serde(default = "default_request_timeout", with = "duration_serde")]
    pub request_timeout: Duration,
    #[serde(default)]
    pub retry: SmsRetryConfig,
}

impl AliyunSmsConfig {
    pub fn validate(&self) -> Result<(), SmsError> {
        let required = [
            &self.access_key_id,
            &self.access_key_secret,
            &self.endpoint,
            &self.region,
            &self.sign_name,
            &self.template_id,
        ];
        if required.iter().any(|value| value.trim().is_empty())
            || self.connect_timeout.is_zero()
            || self.request_timeout.is_zero()
            || self.retry.max_attempts == 0
            || self.retry.base_backoff_ms > self.retry.max_backoff_ms
        {
            return Err(SmsError::new(SmsErrorCategory::Configuration));
        }
        if self.endpoint.contains('/') || self.endpoint.contains(':') {
            return Err(SmsError::new(SmsErrorCategory::Configuration));
        }
        Ok(())
    }
}

impl fmt::Debug for AliyunSmsConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AliyunSmsConfig")
            .field("access_key_id", &"[REDACTED]")
            .field("access_key_secret", &"[REDACTED]")
            .field("endpoint", &self.endpoint)
            .field("region", &self.region)
            .field("sign_name", &"[REDACTED]")
            .field("template_id", &"[REDACTED]")
            .field("connect_timeout", &self.connect_timeout)
            .field("request_timeout", &self.request_timeout)
            .field("retry", &self.retry)
            .finish()
    }
}

mod duration_serde {
    use serde::{de::Error as _, Deserialize, Deserializer, Serializer};
    use std::time::Duration;

    pub fn serialize<S>(duration: &Duration, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        if duration.subsec_millis() == 0 {
            serializer.serialize_str(&format!("{}s", duration.as_secs()))
        } else {
            serializer.serialize_str(&format!("{}ms", duration.as_millis()))
        }
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Duration, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        let (number, unit) = if let Some(value) = raw.strip_suffix("ms") {
            (value, "ms")
        } else if let Some(value) = raw.strip_suffix('s') {
            (value, "s")
        } else if let Some(value) = raw.strip_suffix('m') {
            (value, "m")
        } else {
            return Err(D::Error::custom("duration must end in ms, s, or m"));
        };
        let value = number
            .parse::<u64>()
            .map_err(|_| D::Error::custom("duration value must be an unsigned integer"))?;
        Ok(match unit {
            "ms" => Duration::from_millis(value),
            "s" => Duration::from_secs(value),
            _ => Duration::from_secs(value.saturating_mul(60)),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_required_configuration_fails_without_echoing_secrets() {
        let config: AliyunSmsConfig = serde_json::from_value(serde_json::json!({
            "access_key_id": "",
            "access_key_secret": "super-secret",
            "sign_name": "sign",
            "template_id": "SMS_1",
            "connect_timeout": "1s",
            "request_timeout": "3s"
        }))
        .unwrap();
        let error = config.validate().unwrap_err();
        assert_eq!(error.category, SmsErrorCategory::Configuration);
        assert!(!format!("{config:?} {error:?}").contains("super-secret"));
    }
}
