use roze_sms::{AliyunSmsConfig, AliyunSmsProvider, SmsMessage, SmsProvider, SmsRetryConfig};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = AliyunSmsConfig {
        access_key_id: std::env::var("ALIYUN_SMS_ACCESS_KEY_ID")?,
        access_key_secret: std::env::var("ALIYUN_SMS_ACCESS_KEY_SECRET")?,
        endpoint: "dysmsapi.aliyuncs.com".to_string(),
        region: "cn-hangzhou".to_string(),
        sign_name: std::env::var("ALIYUN_SMS_SIGN_NAME")?,
        template_id: std::env::var("ALIYUN_SMS_TEMPLATE_ID")?,
        connect_timeout: std::time::Duration::from_secs(1),
        request_timeout: std::time::Duration::from_secs(3),
        retry: SmsRetryConfig::default(),
    };
    let provider = AliyunSmsProvider::new(config)?;
    let phone = std::env::var("ALIYUN_SMS_TEST_PHONE")?;
    let mut message = SmsMessage::new([phone]);
    message
        .template_params
        .insert("code".to_string(), serde_json::json!("123456"));
    let result = provider.send(message).await?;
    println!(
        "SMS accepted: request_id={:?}, biz_id={:?}",
        result.request_id, result.biz_id
    );
    Ok(())
}
