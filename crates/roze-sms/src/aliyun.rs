use crate::{AliyunSmsConfig, SmsError, SmsErrorCategory, SmsMessage, SmsProvider, SmsSendResult};
use async_trait::async_trait;
use chrono::Utc;
use hmac::{Hmac, KeyInit, Mac};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, fmt, sync::Arc, time::Instant};
use uuid::Uuid;

const ACTION: &str = "SendSms";
const VERSION: &str = "2017-05-25";
const ALGORITHM: &str = "ACS3-HMAC-SHA256";
const EMPTY_BODY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

#[derive(Clone)]
pub struct AliyunSmsProvider {
    config: Arc<AliyunSmsConfig>,
    transport: Arc<dyn Transport>,
}

impl fmt::Debug for AliyunSmsProvider {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AliyunSmsProvider")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl AliyunSmsProvider {
    pub fn new(config: AliyunSmsConfig) -> Result<Self, SmsError> {
        config.validate()?;
        let client = reqwest::Client::builder()
            .connect_timeout(config.connect_timeout)
            .timeout(config.request_timeout)
            .https_only(true)
            .build()
            .map_err(|_| SmsError::new(SmsErrorCategory::Configuration))?;
        Ok(Self {
            config: Arc::new(config),
            transport: Arc::new(ReqwestTransport { client }),
        })
    }

    fn validate_message(&self, message: &mut SmsMessage) -> Result<(), SmsError> {
        if message.sign_name.trim().is_empty() {
            message.sign_name.clone_from(&self.config.sign_name);
        }
        if message.template_id.trim().is_empty() {
            message.template_id.clone_from(&self.config.template_id);
        }
        let invalid_phone = message.phone_numbers.is_empty()
            || message.phone_numbers.len() > 1000
            || message.phone_numbers.iter().any(|phone| {
                phone.is_empty()
                    || phone.len() > 32
                    || !phone
                        .bytes()
                        .enumerate()
                        .all(|(index, byte)| byte.is_ascii_digit() || (index == 0 && byte == b'+'))
            });
        if invalid_phone
            || message.sign_name.trim().is_empty()
            || message.template_id.trim().is_empty()
            || message.sign_name.len() > 128
            || message.template_id.len() > 128
        {
            return Err(SmsError::new(SmsErrorCategory::InvalidParameter));
        }
        serde_json::to_string(&message.template_params)
            .map_err(|_| SmsError::new(SmsErrorCategory::InvalidParameter))?;
        Ok(())
    }

    fn build_request(&self, message: &SmsMessage) -> Result<SignedRequest, SmsError> {
        let mut query = BTreeMap::new();
        query.insert("PhoneNumbers", message.phone_numbers.join(","));
        query.insert("SignName", message.sign_name.clone());
        query.insert("TemplateCode", message.template_id.clone());
        if !message.template_params.is_empty() {
            query.insert(
                "TemplateParam",
                serde_json::to_string(&message.template_params)
                    .map_err(|_| SmsError::new(SmsErrorCategory::InvalidParameter))?,
            );
        }
        if let Some(out_id) = message.out_id.as_deref().filter(|value| !value.is_empty()) {
            query.insert("OutId", out_id.to_string());
        }
        let canonical_query = canonical_query(&query);
        let date = Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
        let nonce = Uuid::now_v7().simple().to_string();
        let signed = sign(SigningInput {
            method: "POST",
            canonical_uri: "/",
            canonical_query: &canonical_query,
            host: &self.config.endpoint,
            action: ACTION,
            version: VERSION,
            date: &date,
            nonce: &nonce,
            access_key_id: &self.config.access_key_id,
            access_key_secret: &self.config.access_key_secret,
        });
        let mut headers = HeaderMap::new();
        for (name, value) in signed.headers {
            headers.insert(
                HeaderName::from_bytes(name.as_bytes())
                    .map_err(|_| SmsError::new(SmsErrorCategory::Configuration))?,
                HeaderValue::from_str(&value)
                    .map_err(|_| SmsError::new(SmsErrorCategory::Configuration))?,
            );
        }
        headers.insert(
            reqwest::header::AUTHORIZATION,
            HeaderValue::from_str(&signed.authorization)
                .map_err(|_| SmsError::new(SmsErrorCategory::Configuration))?,
        );
        Ok(SignedRequest {
            url: format!("https://{}/?{canonical_query}", self.config.endpoint),
            headers,
        })
    }

    fn can_retry(&self, attempt: u8, retryable: bool) -> bool {
        self.config.retry.enabled
            && retryable
            && attempt.saturating_add(1) < self.config.retry.max_attempts
    }

    async fn backoff(&self, attempt: u8) {
        let multiplier = 1_u64.checked_shl(u32::from(attempt)).unwrap_or(u64::MAX);
        let delay = self
            .config
            .retry
            .base_backoff_ms
            .saturating_mul(multiplier)
            .min(self.config.retry.max_backoff_ms);
        tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
    }
}

#[async_trait]
impl SmsProvider for AliyunSmsProvider {
    #[tracing::instrument(name = "roze.sms.send", skip_all, fields(provider = "aliyun"))]
    async fn send(&self, mut message: SmsMessage) -> Result<SmsSendResult, SmsError> {
        let started = Instant::now();
        if let Err(error) = self.validate_message(&mut message) {
            record(false, error.category, started);
            return Err(error);
        }

        let mut attempt = 0_u8;
        loop {
            let request = match self.build_request(&message) {
                Ok(request) => request,
                Err(error) => {
                    record(false, error.category, started);
                    return Err(error);
                }
            };
            match self.transport.send(request).await {
                Err(TransportError::Connect) if self.can_retry(attempt, true) => {
                    tracing::warn!(
                        event = "sms.retry",
                        provider = "aliyun",
                        reason = "connect",
                        attempt = attempt + 1,
                        "retrying SMS request before request transmission"
                    );
                    self.backoff(attempt).await;
                    attempt += 1;
                }
                Err(TransportError::Connect) => {
                    let error = provider_error(SmsErrorCategory::Network);
                    record(false, error.category, started);
                    return Err(error);
                }
                Err(TransportError::Timeout) => {
                    // A timeout cannot prove whether Aliyun accepted the request.
                    let error = provider_error(SmsErrorCategory::UnknownOutcome);
                    record(false, error.category, started);
                    return Err(error);
                }
                Err(TransportError::AfterSend) => {
                    let error = provider_error(SmsErrorCategory::UnknownOutcome);
                    record(false, error.category, started);
                    return Err(error);
                }
                Ok(response) => match classify_response(response) {
                    ResponseDecision::Success(result) => {
                        record(true, SmsErrorCategory::ProviderRejected, started);
                        return Ok(result);
                    }
                    ResponseDecision::Failure { error, retryable }
                        if self.can_retry(attempt, retryable) =>
                    {
                        tracing::warn!(
                            event = "sms.retry",
                            provider = "aliyun",
                            reason = error.category.as_str(),
                            attempt = attempt + 1,
                            "retrying explicitly rejected SMS request"
                        );
                        self.backoff(attempt).await;
                        attempt += 1;
                    }
                    ResponseDecision::Failure { error, .. } => {
                        record(false, error.category, started);
                        return Err(error);
                    }
                },
            }
        }
    }

    fn health_check(&self) -> Result<(), SmsError> {
        self.config.validate()
    }
}

fn record(success: bool, category: SmsErrorCategory, started: Instant) {
    roze_metrics::record_sms_send(
        "aliyun",
        if success { "success" } else { "failure" },
        if success { "none" } else { category.as_str() },
        started.elapsed(),
    );
}

fn provider_error(category: SmsErrorCategory) -> SmsError {
    let mut error = SmsError::new(category);
    error.provider = Some("aliyun".into());
    error
}

struct SignedRequest {
    url: String,
    headers: HeaderMap,
}

#[derive(Debug)]
struct HttpResponse {
    status: u16,
    body: Vec<u8>,
}

#[derive(Debug, Clone, Copy)]
enum TransportError {
    Connect,
    Timeout,
    AfterSend,
}

#[async_trait]
trait Transport: Send + Sync {
    async fn send(&self, request: SignedRequest) -> Result<HttpResponse, TransportError>;
}

struct ReqwestTransport {
    client: reqwest::Client,
}

#[async_trait]
impl Transport for ReqwestTransport {
    async fn send(&self, request: SignedRequest) -> Result<HttpResponse, TransportError> {
        let response = self
            .client
            .post(request.url)
            .headers(request.headers)
            .send()
            .await
            .map_err(|error| {
                if error.is_connect() {
                    TransportError::Connect
                } else if error.is_timeout() {
                    TransportError::Timeout
                } else {
                    TransportError::AfterSend
                }
            })?;
        let status = response.status().as_u16();
        let body = response
            .bytes()
            .await
            .map_err(|_| TransportError::AfterSend)?;
        Ok(HttpResponse {
            status,
            body: body.to_vec(),
        })
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct AliyunResponse {
    code: Option<String>,
    message: Option<String>,
    biz_id: Option<String>,
    request_id: Option<String>,
}

enum ResponseDecision {
    Success(SmsSendResult),
    Failure { error: SmsError, retryable: bool },
}

fn classify_response(response: HttpResponse) -> ResponseDecision {
    let parsed = serde_json::from_slice::<AliyunResponse>(&response.body).ok();
    let code = parsed
        .as_ref()
        .and_then(|value| value.code.clone())
        .unwrap_or_else(|| format!("HTTP_{}", response.status));
    let category = if response.status == 401 || response.status == 403 || is_auth_code(&code) {
        SmsErrorCategory::Authentication
    } else if response.status == 429 || is_rate_limit_code(&code) {
        SmsErrorCategory::RateLimit
    } else if response.status == 400 || is_parameter_code(&code) {
        SmsErrorCategory::InvalidParameter
    } else {
        SmsErrorCategory::ProviderRejected
    };

    if (200..300).contains(&response.status) && code.eq_ignore_ascii_case("OK") {
        let parsed = parsed.expect("successful response was parsed above");
        return ResponseDecision::Success(SmsSendResult {
            provider: "aliyun".to_string(),
            provider_code: code,
            provider_message: parsed.message,
            biz_id: parsed.biz_id,
            request_id: parsed.request_id,
        });
    }

    let mut error = provider_error(category);
    error.provider_code = Some(code.clone().into_boxed_str());
    if let Some(parsed) = parsed {
        error.provider_message = parsed.message.map(String::into_boxed_str);
        error.request_id = parsed.request_id.map(String::into_boxed_str);
        error.biz_id = parsed.biz_id.map(String::into_boxed_str);
    }
    let retryable = category == SmsErrorCategory::RateLimit
        || (category == SmsErrorCategory::ProviderRejected
            && ((500..600).contains(&response.status) || is_explicit_server_code(&code)));
    ResponseDecision::Failure { error, retryable }
}

fn is_auth_code(code: &str) -> bool {
    let code = code.to_ascii_lowercase();
    code.contains("signature")
        || code.contains("accesskey")
        || code.contains("unauthorized")
        || code.contains("permission")
        || code.contains("forbidden")
}

fn is_rate_limit_code(code: &str) -> bool {
    let code = code.to_ascii_lowercase();
    code.contains("throttl") || code.contains("business_limit_control")
}

fn is_parameter_code(code: &str) -> bool {
    let code = code.to_ascii_lowercase();
    code.contains("invalidparameter")
        || code.contains("missingparameter")
        || code.contains("mobile_number_illegal")
        || code.contains("template_missing_parameters")
        || code.contains("invalid_parameters")
        || code.contains("param_not_support")
}

fn is_explicit_server_code(code: &str) -> bool {
    matches!(
        code.to_ascii_lowercase().as_str(),
        "internalerror" | "serviceunavailable" | "isp.system_error" | "isp.service_unavailable"
    )
}

fn canonical_query(parameters: &BTreeMap<&str, String>) -> String {
    parameters
        .iter()
        .map(|(name, value)| format!("{}={}", rfc3986_encode(name), rfc3986_encode(value)))
        .collect::<Vec<_>>()
        .join("&")
}

fn rfc3986_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(char::from(*byte));
        } else {
            use std::fmt::Write as _;
            write!(&mut encoded, "%{byte:02X}").expect("writing to String cannot fail");
        }
    }
    encoded
}

struct SigningInput<'a> {
    method: &'a str,
    canonical_uri: &'a str,
    canonical_query: &'a str,
    host: &'a str,
    action: &'a str,
    version: &'a str,
    date: &'a str,
    nonce: &'a str,
    access_key_id: &'a str,
    access_key_secret: &'a str,
}

struct SigningOutput {
    authorization: String,
    headers: BTreeMap<&'static str, String>,
}

fn sign(input: SigningInput<'_>) -> SigningOutput {
    let headers = BTreeMap::from([
        ("host", input.host.to_string()),
        ("x-acs-action", input.action.to_string()),
        ("x-acs-content-sha256", EMPTY_BODY_SHA256.to_string()),
        ("x-acs-date", input.date.to_string()),
        ("x-acs-signature-nonce", input.nonce.to_string()),
        ("x-acs-version", input.version.to_string()),
    ]);
    let canonical_headers = headers
        .iter()
        .map(|(name, value)| format!("{name}:{}\n", value.trim()))
        .collect::<String>();
    let signed_headers = headers.keys().copied().collect::<Vec<_>>().join(";");
    let canonical_request = format!(
        "{}\n{}\n{}\n{}\n{}\n{}",
        input.method.to_ascii_uppercase(),
        input.canonical_uri,
        input.canonical_query,
        canonical_headers,
        signed_headers,
        EMPTY_BODY_SHA256
    );
    let canonical_hash = hex_lower(&Sha256::digest(canonical_request.as_bytes()));
    let string_to_sign = format!("{ALGORITHM}\n{canonical_hash}");
    let mut mac = Hmac::<Sha256>::new_from_slice(input.access_key_secret.as_bytes())
        .expect("HMAC accepts keys of any size");
    mac.update(string_to_sign.as_bytes());
    let signature = hex_lower(&mac.finalize().into_bytes());
    let authorization = format!(
        "{ALGORITHM} Credential={},SignedHeaders={signed_headers},Signature={signature}",
        input.access_key_id
    );
    SigningOutput {
        authorization,
        headers,
    }
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        write!(&mut out, "{byte:02x}").expect("writing to String cannot fail");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SmsRetryConfig;
    use std::{collections::VecDeque, sync::Mutex};

    fn config() -> AliyunSmsConfig {
        AliyunSmsConfig {
            access_key_id: "test-ak".to_string(),
            access_key_secret: "test-secret".to_string(),
            endpoint: "dysmsapi.aliyuncs.com".to_string(),
            region: "cn-hangzhou".to_string(),
            sign_name: "default-sign".to_string(),
            template_id: "SMS_default".to_string(),
            connect_timeout: std::time::Duration::from_secs(1),
            request_timeout: std::time::Duration::from_secs(3),
            retry: SmsRetryConfig::default(),
        }
    }

    #[test]
    fn official_acs3_signature_vector_matches() {
        let output = sign(SigningInput {
            method: "POST",
            canonical_uri: "/",
            canonical_query:
                "ImageId=win2019_1809_x64_dtc_zh-cn_40G_alibase_20230811.vhd&RegionId=cn-shanghai",
            host: "ecs.cn-shanghai.aliyuncs.com",
            action: "RunInstances",
            version: "2014-05-26",
            date: "2023-10-26T10:22:32Z",
            nonce: "3156853299f313e23d1673dc12e1703d",
            access_key_id: "YourAccessKeyId",
            access_key_secret: "YourAccessKeySecret",
        });
        assert!(output.authorization.ends_with(
            "Signature=06563a9e1b43f5dfe96b81484da74bceab24a1d853912eee15083a6f0f3283c0"
        ));
    }

    #[test]
    fn send_sms_query_uses_sorted_rfc3986_encoding() {
        let query = canonical_query(&BTreeMap::from([
            ("TemplateParam", r#"{"code":"12 34"}"#.to_string()),
            ("PhoneNumbers", "+8613900000000,13800000000".to_string()),
            ("SignName", "测试~签名".to_string()),
        ]));
        assert_eq!(query, "PhoneNumbers=%2B8613900000000%2C13800000000&SignName=%E6%B5%8B%E8%AF%95~%E7%AD%BE%E5%90%8D&TemplateParam=%7B%22code%22%3A%2212%2034%22%7D");
    }

    #[derive(Debug)]
    struct ScriptedTransport {
        responses: Mutex<VecDeque<Result<HttpResponse, TransportError>>>,
    }

    #[async_trait]
    impl Transport for ScriptedTransport {
        async fn send(&self, _request: SignedRequest) -> Result<HttpResponse, TransportError> {
            self.responses.lock().unwrap().pop_front().unwrap()
        }
    }

    fn provider_with(
        responses: impl IntoIterator<Item = Result<HttpResponse, TransportError>>,
    ) -> AliyunSmsProvider {
        AliyunSmsProvider {
            config: Arc::new(config()),
            transport: Arc::new(ScriptedTransport {
                responses: Mutex::new(responses.into_iter().collect()),
            }),
        }
    }

    fn message() -> SmsMessage {
        SmsMessage::new(["13900000000"])
    }

    fn response(status: u16, body: &str) -> Result<HttpResponse, TransportError> {
        Ok(HttpResponse {
            status,
            body: body.as_bytes().to_vec(),
        })
    }

    #[tokio::test]
    async fn parses_success_and_ids() {
        let provider = provider_with([response(
            200,
            r#"{"Code":"OK","Message":"OK","BizId":"biz-1","RequestId":"req-1"}"#,
        )]);
        let result = provider.send(message()).await.unwrap();
        assert_eq!(result.biz_id.as_deref(), Some("biz-1"));
        assert_eq!(result.request_id.as_deref(), Some("req-1"));
    }

    #[tokio::test]
    async fn maps_auth_rate_limit_and_template_errors() {
        for (body, expected) in [
            (
                r#"{"Code":"SignatureDoesNotMatch","RequestId":"r"}"#,
                SmsErrorCategory::Authentication,
            ),
            (
                r#"{"Code":"isv.BUSINESS_LIMIT_CONTROL","RequestId":"r"}"#,
                SmsErrorCategory::RateLimit,
            ),
            (
                r#"{"Code":"isv.TEMPLATE_MISSING_PARAMETERS","RequestId":"r"}"#,
                SmsErrorCategory::InvalidParameter,
            ),
        ] {
            let mut cfg = config();
            cfg.retry.enabled = false;
            let provider = AliyunSmsProvider {
                config: Arc::new(cfg),
                transport: Arc::new(ScriptedTransport {
                    responses: Mutex::new([response(200, body)].into_iter().collect()),
                }),
            };
            let error = provider.send(message()).await.unwrap_err();
            assert_eq!(error.category, expected);
            assert_eq!(error.request_id.as_deref(), Some("r"));
        }
    }

    #[tokio::test]
    async fn retries_connect_failure_but_never_unknown_timeout() {
        let provider = provider_with([
            Err(TransportError::Connect),
            response(200, r#"{"Code":"OK","BizId":"b","RequestId":"r"}"#),
        ]);
        assert!(provider.send(message()).await.is_ok());

        let provider = provider_with([
            Err(TransportError::Timeout),
            response(200, r#"{"Code":"OK"}"#),
        ]);
        let error = provider.send(message()).await.unwrap_err();
        assert_eq!(error.category, SmsErrorCategory::UnknownOutcome);
    }

    #[test]
    fn debug_output_redacts_all_sensitive_material() {
        let cfg = config();
        let debug = format!("{cfg:?}");
        assert!(!debug.contains("test-secret"));
        assert!(!debug.contains("test-ak"));
        assert!(!debug.contains("default-sign"));
        assert!(!debug.contains("SMS_default"));

        let mut message = message();
        message
            .template_params
            .insert("code".to_string(), serde_json::json!("123456"));
        let debug = format!("{message:?}");
        assert!(!debug.contains("13900000000"));
        assert!(!debug.contains("123456"));
    }
}
