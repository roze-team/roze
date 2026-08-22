# roze-sms

`roze-sms` provides a provider-neutral SMS delivery boundary and a native
Alibaba Cloud Dysmsapi `2017-05-25` `SendSms` provider. It uses HTTPS, the
current `ACS3-HMAC-SHA256` OpenAPI signature, a shared `reqwest::Client`,
bounded retries, bounded metrics labels, and redacted debug output.

The crate does **not** generate or verify OTPs. Applications must generate OTPs
with a CSPRNG, store only a digest in Redis with TTL/attempt limits/one-time
consumption, verify captcha proof when required, and rate limit by phone, IP,
device, and scene. Do not log phone numbers, OTPs, template parameters, or
credentials.

## Configuration

Use Roze typed application configuration and secret resolution. Credentials
must be injected through `${ENV_NAME}`, `env://ENV_NAME`, `file://...`, Vault,
or another `SecretProvider`; never commit resolved values.

```yaml
sms:
  provider: aliyun
  access_key_id: ${ALIYUN_SMS_ACCESS_KEY_ID}
  access_key_secret: ${ALIYUN_SMS_ACCESS_KEY_SECRET}
  endpoint: dysmsapi.aliyuncs.com
  region: cn-hangzhou
  sign_name: ${ALIYUN_SMS_SIGN_NAME}
  template_id: ${ALIYUN_SMS_TEMPLATE_ID}
  connect_timeout: 1s
  request_timeout: 3s
  retry:
    enabled: true
    max_attempts: 2
    base_backoff_ms: 50
    max_backoff_ms: 500
```

`AliyunSmsProvider::new` validates all required values and builds the HTTP
client, so configuration errors fail startup. `health_check` repeats only local
validation and never sends a real SMS.

## Minimal send

```rust,no_run
use roze_sms::{AliyunSmsConfig, AliyunSmsProvider, SmsMessage, SmsProvider};

# async fn example(config: AliyunSmsConfig) -> Result<(), roze_sms::SmsError> {
let provider = AliyunSmsProvider::new(config)?;
let mut message = SmsMessage::new(["+8613900000000"]);
// Empty sign/template fields intentionally use the validated configuration defaults.
message.template_params.insert("code".into(), serde_json::json!("123456"));
message.out_id = Some("registration-request-opaque-id".into());
let result = provider.send(message).await?;
// result.request_id and result.biz_id are safe for controlled diagnostics.
# Ok(())
# }
```

For application tests, use the deterministic FIFO `MockSmsProvider` and inspect
captured messages with `take_sent()`.

## Composition with Roze

- Rate limiting: use `roze-rate-limit` before `send`. Map the scene to the
  bounded operation, verified IP to `client_ip`, phone to `subject`, and device
  ID to an explicitly allowed header dimension. Roze hashes the complete
  identity before storage; never put these values in metrics or logs.
- Idempotency: claim the application's opaque business key with Roze Redis
  idempotency before calling `send`, commit after a definitive result, and do
  not release/retry an `UnknownOutcome` automatically.
- Async delivery: put only an application-approved delivery command in the
  Roze SQL outbox and relay it through `roze-job`/`roze-mq`. OTP templates often
  contain secrets, so assess encryption and expiry before persisting payloads.
- Metrics: `roze_sms_send_attempts_total` and
  `roze_sms_send_duration_ms_total` use only bounded `provider`, `outcome`, and
  `category` labels.

Retries are limited to connection establishment failures (no request bytes sent),
explicit throttling, and explicit HTTP/provider server failures. Timeouts and
response-body failures are classified as `UnknownOutcome` and are never retried
because the SMS may already have been accepted.

## Protocol references

- Alibaba Cloud Dysmsapi `SendSms` 2017-05-25 API documentation
- Alibaba Cloud OpenAPI V3 request structure and `ACS3-HMAC-SHA256` signature
- `github.com/alibabacloud-go/dysmsapi-20170525/v3` request/response behavior
