use std::{collections::BTreeMap, net::SocketAddr, path::Path};

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub mod config_center;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceConfig {
    pub name: String,
    #[serde(default)]
    pub rest: Option<RestConfig>,
    #[serde(default)]
    pub rpc: Option<RpcConfig>,
    #[serde(default)]
    pub rpc_client: Option<RpcClientConfig>,
    #[serde(default)]
    pub registry: Option<RegistryConfig>,
    #[serde(default)]
    pub database: Option<DatabaseConfig>,
    #[serde(default)]
    pub mongo: Option<MongoConfig>,
    #[serde(default)]
    pub cache: Option<CacheConfig>,
    #[serde(default)]
    pub auth: Option<AuthConfig>,
    #[serde(default)]
    pub kafka: Option<KafkaConfig>,
    #[serde(default)]
    pub nats: Option<roze_nats::NatsConfig>,
    #[serde(default)]
    pub outbox: Option<OutboxConfig>,
    #[serde(default)]
    pub storage: Option<roze_storage::StorageConfig>,
    #[serde(default)]
    pub gateway: Option<GatewayConfig>,
    #[serde(default)]
    pub telemetry: Option<TelemetryConfig>,
    pub governance: GovernanceConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatewayConfig {
    #[serde(default)]
    pub listen: Option<SocketAddr>,
    #[serde(default)]
    pub services: Vec<GatewayService>,
    #[serde(default)]
    pub routes: Vec<GatewayRoute>,
    #[serde(default)]
    pub middlewares: Vec<String>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    #[serde(default)]
    pub request_body_limit_bytes: Option<usize>,
    #[serde(default)]
    pub fallback: Option<GatewayFallbackResponse>,
    #[serde(default)]
    pub cors: Option<GatewayCorsConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatewayService {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub upstream: String,
    #[serde(default)]
    pub registry_name: Option<String>,
    #[serde(default)]
    pub instance_tags: BTreeMap<String, String>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    #[serde(default)]
    pub outlier: Option<GatewayOutlierConfig>,
    #[serde(default)]
    pub health_check: Option<GatewayHealthCheckConfig>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct GatewayOutlierConfig {
    #[serde(default = "default_gateway_outlier_failure_threshold")]
    pub failure_threshold: u32,
    #[serde(default = "default_gateway_outlier_ejection_ms")]
    pub ejection_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatewayHealthCheckConfig {
    #[serde(default = "default_gateway_health_check_path")]
    pub path: String,
    #[serde(default = "default_gateway_health_check_interval_ms")]
    pub interval_ms: u64,
    #[serde(default = "default_gateway_health_check_timeout_ms")]
    pub timeout_ms: u64,
    #[serde(default = "default_gateway_health_check_unhealthy_threshold")]
    pub unhealthy_threshold: u32,
    #[serde(default = "default_gateway_health_check_healthy_threshold")]
    pub healthy_threshold: u32,
    #[serde(default = "default_gateway_health_check_expected_status")]
    pub expected_status: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatewayRoute {
    pub path: String,
    pub service: String,
    #[serde(default)]
    pub methods: Vec<String>,
    #[serde(default = "default_gateway_route_weight")]
    pub weight: u32,
    #[serde(default)]
    pub instance_tags: BTreeMap<String, String>,
    #[serde(default)]
    pub middlewares: Vec<String>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    #[serde(default)]
    pub retries: Option<u32>,
    #[serde(default)]
    pub retry_backoff_ms: Option<u64>,
    #[serde(default)]
    pub rewrite: Option<String>,
    #[serde(default)]
    pub fallback: Option<GatewayFallbackResponse>,
    #[serde(default)]
    pub rate_limit: Option<RateLimitConfig>,
    #[serde(default)]
    pub breaker: Option<BreakerConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatewayFallbackResponse {
    #[serde(default = "default_gateway_fallback_status")]
    pub status: u16,
    #[serde(default)]
    pub body: Option<Value>,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GatewayCorsConfig {
    #[serde(default)]
    pub allow_origins: Vec<String>,
    #[serde(default)]
    pub allow_methods: Vec<String>,
    #[serde(default)]
    pub allow_headers: Vec<String>,
    #[serde(default)]
    pub max_age_seconds: Option<u64>,
}

fn default_gateway_fallback_status() -> u16 {
    503
}

fn default_gateway_route_weight() -> u32 {
    100
}

fn default_gateway_outlier_failure_threshold() -> u32 {
    3
}

fn default_gateway_outlier_ejection_ms() -> u64 {
    30_000
}

fn default_gateway_health_check_path() -> String {
    "/healthz".to_string()
}

fn default_gateway_health_check_interval_ms() -> u64 {
    10_000
}

fn default_gateway_health_check_timeout_ms() -> u64 {
    1_000
}

fn default_gateway_health_check_unhealthy_threshold() -> u32 {
    3
}

fn default_gateway_health_check_healthy_threshold() -> u32 {
    1
}

fn default_gateway_health_check_expected_status() -> u16 {
    200
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RestConfig {
    pub addr: SocketAddr,
    pub register: bool,
    #[serde(default)]
    pub middlewares: HttpMiddlewaresConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HttpMiddlewaresConfig {
    #[serde(default = "default_true")]
    pub recover: bool,
    #[serde(default = "default_true")]
    pub trace: bool,
    #[serde(default = "default_true")]
    pub stat: bool,
    #[serde(default = "default_true")]
    pub prometheus: bool,
    #[serde(default = "default_true")]
    pub cors: bool,
    #[serde(default)]
    pub cors_config: Option<HttpCorsConfig>,
    #[serde(default)]
    pub timeout: bool,
    #[serde(default)]
    pub max_conns: Option<usize>,
    #[serde(default)]
    pub shedding: Option<SheddingConfig>,
    #[serde(default)]
    pub gunzip: bool,
    #[serde(default)]
    pub request_body_limit_bytes: Option<usize>,
}

impl Default for HttpMiddlewaresConfig {
    fn default() -> Self {
        Self {
            recover: true,
            trace: true,
            stat: true,
            prometheus: true,
            cors: true,
            cors_config: None,
            timeout: true,
            max_conns: None,
            shedding: None,
            gunzip: false,
            request_body_limit_bytes: None,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct HttpCorsConfig {
    #[serde(default)]
    pub allow_origins: Vec<String>,
    #[serde(default)]
    pub allow_methods: Vec<String>,
    #[serde(default)]
    pub allow_headers: Vec<String>,
    #[serde(default)]
    pub expose_headers: Vec<String>,
    #[serde(default)]
    pub allow_credentials: bool,
    #[serde(default)]
    pub max_age_seconds: Option<u64>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct SheddingConfig {
    #[serde(default = "default_shedding_concurrency")]
    pub concurrency: usize,
    #[serde(default = "default_shedding_window_ms")]
    pub window_ms: u64,
    #[serde(default = "default_shedding_min_samples")]
    pub min_samples: u64,
    #[serde(default = "default_shedding_max_avg_latency_ms")]
    pub max_avg_latency_ms: u64,
    #[serde(default = "default_shedding_max_failure_ratio_per_mille")]
    pub max_failure_ratio_per_mille: u32,
    #[serde(default = "default_shedding_cool_down_ms")]
    pub cool_down_ms: u64,
}

impl Default for SheddingConfig {
    fn default() -> Self {
        Self {
            concurrency: default_shedding_concurrency(),
            window_ms: default_shedding_window_ms(),
            min_samples: default_shedding_min_samples(),
            max_avg_latency_ms: default_shedding_max_avg_latency_ms(),
            max_failure_ratio_per_mille: default_shedding_max_failure_ratio_per_mille(),
            cool_down_ms: default_shedding_cool_down_ms(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcConfig {
    pub addr: SocketAddr,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutboxConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_outbox_batch_size")]
    pub batch_size: usize,
    #[serde(default = "default_outbox_interval_ms")]
    pub interval_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcClientConfig {
    #[serde(default)]
    pub etcd: Option<RpcClientEtcdConfig>,
    #[serde(default)]
    pub endpoints: Vec<String>,
    #[serde(default)]
    pub target: Option<String>,
    #[serde(default)]
    pub app: Option<String>,
    #[serde(default)]
    pub token: Option<String>,
    #[serde(default)]
    pub non_block: bool,
    #[serde(default = "default_rpc_client_timeout_ms")]
    pub timeout_ms: u64,
    #[serde(default = "default_rpc_client_keepalive_time_secs")]
    pub keepalive_time_secs: u64,
    #[serde(default)]
    pub middlewares: RpcClientMiddlewaresConfig,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RpcClientEtcdConfig {
    #[serde(default)]
    pub hosts: Vec<String>,
    pub key: String,
    #[serde(default)]
    pub id: Option<i64>,
    #[serde(default)]
    pub user: Option<String>,
    #[serde(default)]
    pub pass: Option<String>,
    #[serde(default)]
    pub cert_file: Option<String>,
    #[serde(default)]
    pub cert_key_file: Option<String>,
    #[serde(default)]
    pub ca_cert_file: Option<String>,
    #[serde(default)]
    pub insecure_skip_verify: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct RpcClientMiddlewaresConfig {
    #[serde(default = "default_true")]
    pub trace: bool,
    #[serde(default = "default_true")]
    pub recover: bool,
    #[serde(default = "default_true")]
    pub stat: bool,
    #[serde(default = "default_true")]
    pub prometheus: bool,
    #[serde(default = "default_true")]
    pub breaker: bool,
}

impl Default for RpcClientMiddlewaresConfig {
    fn default() -> Self {
        Self {
            trace: true,
            recover: true,
            stat: true,
            prometheus: true,
            breaker: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryConfig {
    pub kind: RegistryKind,
    #[serde(default)]
    pub endpoints: Vec<String>,
    #[serde(default = "default_registry_ttl_secs")]
    pub ttl_seconds: u64,
    #[serde(default = "default_registry_renew_interval_secs")]
    pub renew_interval_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthConfig {
    pub jwt_secret: String,
    #[serde(default = "default_jwt_issuer")]
    pub jwt_issuer: String,
    #[serde(default = "default_jwt_expiration_secs")]
    pub jwt_expiration_secs: u64,
    #[serde(default)]
    pub api_keys: Option<roze_auth::ApiKeyConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KafkaConfig {
    #[serde(default)]
    pub brokers: Vec<String>,
    #[serde(default, alias = "bootstrap")]
    pub bootstrap: Option<String>,
    #[serde(default)]
    pub bootstrap_servers: Option<Vec<String>>,
    #[serde(default)]
    pub topic_prefix: String,
    #[serde(default, alias = "group")]
    pub group_id: Option<String>,
    #[serde(default = "default_kafka_client_id")]
    pub client_id: String,
    #[serde(default = "default_kafka_acks")]
    pub acks: String,
    #[serde(default = "default_kafka_auto_offset_reset")]
    pub auto_offset_reset: String,
    #[serde(default = "default_kafka_enable_auto_commit", alias = "auto_commit")]
    pub enable_auto_commit: bool,
    #[serde(default)]
    pub enable_manual_ack: bool,
    #[serde(default = "default_kafka_linger_ms")]
    pub linger_ms: u64,
    #[serde(default = "default_kafka_batch_size")]
    pub batch_size: usize,
    #[serde(default = "default_kafka_session_timeout_ms")]
    pub session_timeout_ms: u64,
    #[serde(default = "default_kafka_heartbeat_interval_ms")]
    pub heartbeat_interval_ms: u64,
    #[serde(default = "default_kafka_max_poll_interval_ms")]
    pub max_poll_interval_ms: u64,
    #[serde(default = "default_kafka_flush_timeout_ms")]
    pub flush_timeout_ms: u64,
    #[serde(default = "default_kafka_max_retries")]
    pub max_retries: u32,
    #[serde(default = "default_kafka_retry_backoff_ms")]
    pub retry_backoff_ms: u64,
    #[serde(default)]
    pub retry_topic: Option<String>,
    #[serde(default)]
    pub dead_letter_topic: Option<String>,
    #[serde(default)]
    pub topic_regex: Option<String>,
    #[serde(default = "default_kafka_consumers")]
    pub consumer_workers: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TelemetryConfig {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub endpoint: Option<String>,
    #[serde(default = "default_telemetry_sampler")]
    pub sampler: f64,
    #[serde(default)]
    pub batcher: TelemetryBatcher,
    #[serde(default)]
    pub propagator: TelemetryPropagator,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TelemetryBatcher {
    #[default]
    #[serde(alias = "otlpgrpc")]
    OtlpGrpc,
    #[serde(alias = "otlphttp")]
    OtlpHttp,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TelemetryPropagator {
    #[default]
    #[serde(alias = "tracecontext", alias = "w3c")]
    TraceContext,
    Jaeger,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheConfig {
    pub url: String,
    #[serde(default = "default_cache_namespace")]
    pub namespace: String,
    #[serde(default = "default_cache_ttl_secs")]
    pub default_ttl_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseConfig {
    pub url: String,
    #[serde(default)]
    pub replicas: Vec<String>,
    #[serde(default)]
    pub policy: DatabaseReadPolicy,
    #[serde(default = "default_max_connections")]
    pub max_connections: u32,
    #[serde(default = "default_min_connections")]
    pub min_connections: u32,
    #[serde(default = "default_connect_timeout_secs")]
    pub connect_timeout_secs: u64,
    #[serde(default = "default_idle_timeout_secs")]
    pub idle_timeout_secs: u64,
    #[serde(default = "default_sqlx_logging")]
    pub sqlx_logging: bool,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum DatabaseReadPolicy {
    #[default]
    RoundRobin,
    Random,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MongoConfig {
    pub url: String,
    pub database: String,
    #[serde(default = "default_mongo_max_pool_size")]
    pub max_pool_size: u32,
    #[serde(default = "default_mongo_min_pool_size")]
    pub min_pool_size: u32,
    #[serde(default)]
    pub app_name: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GovernanceConfig {
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    #[serde(default)]
    pub retry: Option<RetryConfig>,
    #[serde(default)]
    pub rate_limit: Option<RateLimitConfig>,
    #[serde(default)]
    pub breaker: Option<BreakerConfig>,
    #[serde(default)]
    pub shedding: Option<SheddingConfig>,
    #[serde(default)]
    pub fallback: Option<GovernanceFallbackConfig>,
    #[serde(default)]
    pub routes: BTreeMap<String, RouteGovernanceConfig>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RouteGovernanceConfig {
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    #[serde(default)]
    pub retry: Option<RetryConfig>,
    #[serde(default)]
    pub rate_limit: Option<RateLimitConfig>,
    #[serde(default)]
    pub breaker: Option<BreakerConfig>,
    #[serde(default)]
    pub shedding: Option<SheddingConfig>,
    #[serde(default)]
    pub fallback: Option<GovernanceFallbackConfig>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct RetryConfig {
    #[serde(default = "default_retry_max_attempts")]
    pub max_attempts: u32,
    #[serde(default = "default_retry_backoff_ms")]
    pub backoff_ms: u64,
    #[serde(default = "default_retry_max_backoff_ms")]
    pub max_backoff_ms: u64,
    #[serde(default)]
    pub budget_percent: Option<u32>,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_attempts: default_retry_max_attempts(),
            backoff_ms: default_retry_backoff_ms(),
            max_backoff_ms: default_retry_max_backoff_ms(),
            budget_percent: None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct GovernanceFallbackConfig {
    #[serde(default)]
    pub enabled: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct RateLimitConfig {
    #[serde(default = "default_rate_limit_burst")]
    pub burst: u32,
    #[serde(default = "default_rate_limit_refill_ms")]
    pub refill_ms: u64,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            burst: default_rate_limit_burst(),
            refill_ms: default_rate_limit_refill_ms(),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct BreakerConfig {
    #[serde(default = "default_breaker_failure_threshold")]
    pub failure_threshold: u32,
    #[serde(default = "default_breaker_reset_timeout_ms")]
    pub reset_timeout_ms: u64,
}

impl Default for BreakerConfig {
    fn default() -> Self {
        Self {
            failure_threshold: default_breaker_failure_threshold(),
            reset_timeout_ms: default_breaker_reset_timeout_ms(),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RegistryKind {
    Memory,
    Etcd,
    Consul,
    Dns,
}

fn default_jwt_issuer() -> String {
    "roze".to_string()
}

fn default_jwt_expiration_secs() -> u64 {
    24 * 60 * 60
}

fn default_cache_namespace() -> String {
    "roze".to_string()
}

fn default_cache_ttl_secs() -> u64 {
    300
}

fn default_registry_ttl_secs() -> u64 {
    10
}

fn default_registry_renew_interval_secs() -> u64 {
    3
}

fn default_max_connections() -> u32 {
    100
}

fn default_min_connections() -> u32 {
    5
}

fn default_connect_timeout_secs() -> u64 {
    8
}

fn default_idle_timeout_secs() -> u64 {
    600
}

fn default_sqlx_logging() -> bool {
    true
}

fn default_retry_max_attempts() -> u32 {
    1
}

fn default_retry_backoff_ms() -> u64 {
    0
}

fn default_retry_max_backoff_ms() -> u64 {
    1_000
}

fn default_rpc_client_timeout_ms() -> u64 {
    2_000
}

fn default_rpc_client_keepalive_time_secs() -> u64 {
    20
}

fn default_outbox_batch_size() -> usize {
    100
}

fn default_outbox_interval_ms() -> u64 {
    1_000
}

fn default_true() -> bool {
    true
}

fn default_mongo_max_pool_size() -> u32 {
    100
}

fn default_mongo_min_pool_size() -> u32 {
    0
}

fn default_telemetry_sampler() -> f64 {
    1.0
}

fn default_rate_limit_burst() -> u32 {
    100
}

fn default_rate_limit_refill_ms() -> u64 {
    10
}

fn default_shedding_concurrency() -> usize {
    1000
}

fn default_shedding_window_ms() -> u64 {
    1000
}

fn default_shedding_min_samples() -> u64 {
    100
}

fn default_shedding_max_avg_latency_ms() -> u64 {
    500
}

fn default_shedding_max_failure_ratio_per_mille() -> u32 {
    500
}

fn default_shedding_cool_down_ms() -> u64 {
    1000
}

fn default_breaker_failure_threshold() -> u32 {
    5
}

fn default_breaker_reset_timeout_ms() -> u64 {
    30_000
}

fn default_kafka_client_id() -> String {
    "roze-kafka".to_string()
}

fn default_kafka_acks() -> String {
    "all".to_string()
}

fn default_kafka_auto_offset_reset() -> String {
    "earliest".to_string()
}

fn default_kafka_enable_auto_commit() -> bool {
    false
}

fn default_kafka_linger_ms() -> u64 {
    0
}

fn default_kafka_batch_size() -> usize {
    0
}

fn default_kafka_heartbeat_interval_ms() -> u64 {
    3_000
}

fn default_kafka_max_poll_interval_ms() -> u64 {
    300_000
}

fn default_kafka_flush_timeout_ms() -> u64 {
    5_000
}

fn default_kafka_max_retries() -> u32 {
    3
}

fn default_kafka_retry_backoff_ms() -> u64 {
    200
}

fn default_kafka_session_timeout_ms() -> u64 {
    10_000
}

fn default_kafka_consumers() -> u32 {
    1
}

pub fn load<T>(path: impl AsRef<Path>) -> Result<T, config::ConfigError>
where
    T: for<'de> Deserialize<'de>,
{
    config::Config::builder()
        .add_source(config::File::from(path.as_ref()))
        .add_source(config::Environment::with_prefix("ROZE").separator("__"))
        .build()?
        .try_deserialize()
}

pub use config_center::*;

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[derive(Debug, Deserialize, PartialEq)]
    struct DemoConfig {
        name: String,
    }

    #[test]
    fn loads_toml_config() {
        let path = std::env::temp_dir().join(format!(
            "roze-config-{}.toml",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        fs::write(&path, "name = \"roze\"\n").expect("write toml");

        let config: DemoConfig = load(&path).expect("load config");
        assert_eq!(
            config,
            DemoConfig {
                name: "roze".into()
            }
        );

        let _ = fs::remove_file(path);
    }

    #[test]
    fn loads_governance_defaults() {
        let source = r#"
name = "demo"

[rest]
addr = "127.0.0.1:3000"
register = false

[governance]
timeout_ms = 250

[governance.retry]
max_attempts = 3
backoff_ms = 25
max_backoff_ms = 250
budget_percent = 20

[governance.rate_limit]
burst = 10

[governance.shedding]
concurrency = 32

[governance.fallback]
enabled = true

[governance.routes.login.breaker]
failure_threshold = 2

[governance.routes.login.retry]
max_attempts = 2
backoff_ms = 5
"#;
        let config: ServiceConfig = config::Config::builder()
            .add_source(config::File::from_str(source, config::FileFormat::Toml))
            .build()
            .expect("build")
            .try_deserialize()
            .expect("deserialize");

        assert!(!config.rest.expect("rest").register);
        let governance = config.governance;
        assert_eq!(governance.timeout_ms, Some(250));
        let retry = governance.retry.expect("retry");
        assert_eq!(retry.max_attempts, 3);
        assert_eq!(retry.backoff_ms, 25);
        assert_eq!(retry.max_backoff_ms, 250);
        assert_eq!(retry.budget_percent, Some(20));
        assert_eq!(
            governance.rate_limit.expect("rate limit").refill_ms,
            default_rate_limit_refill_ms()
        );
        assert_eq!(governance.shedding.expect("shedding").concurrency, 32);
        assert!(governance.fallback.expect("fallback").enabled);
        assert_eq!(
            governance
                .routes
                .get("login")
                .and_then(|route| route.breaker)
                .expect("route breaker")
                .reset_timeout_ms,
            default_breaker_reset_timeout_ms()
        );
        assert_eq!(
            governance
                .routes
                .get("login")
                .and_then(|route| route.retry)
                .expect("route retry")
                .backoff_ms,
            5
        );
    }

    #[test]
    fn loads_auth_api_key_config() {
        let source = r#"
name = "demo"

[auth]
jwt_secret = "secret"
jwt_issuer = "issuer"

[auth.api_keys]
header = "x-service-key"

[[auth.api_keys.keys]]
key = "secret-key"
subject = "internal-worker"
roles = ["worker", "admin"]
tenant = "acme"

[governance]
"#;
        let config: ServiceConfig = config::Config::builder()
            .add_source(config::File::from_str(source, config::FileFormat::Toml))
            .build()
            .expect("build")
            .try_deserialize()
            .expect("deserialize");

        let api_keys = config.auth.expect("auth").api_keys.expect("api keys");
        assert_eq!(api_keys.header, "x-service-key");
        let credential = api_keys.keys.first().expect("credential");
        assert_eq!(credential.key, "secret-key");
        assert_eq!(credential.subject, "internal-worker");
        assert_eq!(credential.roles, vec!["worker", "admin"]);
        assert_eq!(credential.tenant.as_deref(), Some("acme"));
    }

    #[test]
    fn loads_mongo_defaults() {
        let source = r#"
name = "demo"

[mongo]
url = "mongodb://localhost:27017"
database = "demo"

[governance]
"#;
        let config: ServiceConfig = config::Config::builder()
            .add_source(config::File::from_str(source, config::FileFormat::Toml))
            .build()
            .expect("build")
            .try_deserialize()
            .expect("deserialize");

        let mongo = config.mongo.expect("mongo");
        assert_eq!(mongo.url, "mongodb://localhost:27017");
        assert_eq!(mongo.database, "demo");
        assert_eq!(mongo.max_pool_size, default_mongo_max_pool_size());
        assert_eq!(mongo.min_pool_size, default_mongo_min_pool_size());
        assert_eq!(mongo.app_name, None);
    }

    #[test]
    fn loads_cache_defaults() {
        let source = r#"
name = "demo"

[cache]
url = "redis://127.0.0.1/"

[governance]
"#;
        let config: ServiceConfig = config::Config::builder()
            .add_source(config::File::from_str(source, config::FileFormat::Toml))
            .build()
            .expect("build")
            .try_deserialize()
            .expect("deserialize");

        let cache = config.cache.expect("cache");
        assert_eq!(cache.url, "redis://127.0.0.1/");
        assert_eq!(cache.namespace, default_cache_namespace());
        assert_eq!(cache.default_ttl_secs, default_cache_ttl_secs());
    }

    #[test]
    fn loads_telemetry_defaults() {
        let source = r#"
name = "demo"

[telemetry]
endpoint = "http://127.0.0.1:4317"

[governance]
"#;
        let config: ServiceConfig = config::Config::builder()
            .add_source(config::File::from_str(source, config::FileFormat::Toml))
            .build()
            .expect("build")
            .try_deserialize()
            .expect("deserialize");

        let telemetry = config.telemetry.expect("telemetry");
        assert_eq!(telemetry.name, None);
        assert_eq!(telemetry.endpoint.as_deref(), Some("http://127.0.0.1:4317"));
        assert_eq!(telemetry.sampler, default_telemetry_sampler());
        assert_eq!(telemetry.batcher, TelemetryBatcher::OtlpGrpc);
        assert_eq!(telemetry.propagator, TelemetryPropagator::TraceContext);
    }

    #[test]
    fn loads_storage_config() {
        let source = r#"
name = "demo"

[storage]
provider = "aliyun_oss"
bucket = "images"
endpoint = "https://oss-cn-hangzhou.aliyuncs.com"
region = "cn-hangzhou"
public_base_url = "https://cdn.example.com"

[storage.validation]
max_size_bytes = 1024
allowed_mime_types = ["image/png"]
allowed_extensions = ["png"]

[governance]
"#;
        let config: ServiceConfig = config::Config::builder()
            .add_source(config::File::from_str(source, config::FileFormat::Toml))
            .build()
            .expect("build")
            .try_deserialize()
            .expect("deserialize");

        let storage = config.storage.expect("storage");
        assert_eq!(storage.bucket, "images");
        assert_eq!(storage.provider, roze_storage::StorageProvider::AliyunOss);
        assert_eq!(storage.validation.max_size_bytes, 1024);
    }

    #[test]
    fn loads_nats_and_outbox_config() {
        let source = r#"
name = "demo"

[nats]
servers = ["127.0.0.1:4222"]
client_name = "demo-api"
subject_prefix = "demo"

[nats.jetstream]
stream = "DEMO"
subjects = ["demo.*"]
durable = "demo-workers"

[outbox]
enabled = true
batch_size = 50
interval_ms = 500

[governance]
"#;
        let config: ServiceConfig = config::Config::builder()
            .add_source(config::File::from_str(source, config::FileFormat::Toml))
            .build()
            .expect("build")
            .try_deserialize()
            .expect("deserialize");

        let nats = config.nats.expect("nats");
        assert_eq!(nats.servers, vec!["127.0.0.1:4222"]);
        assert_eq!(nats.subject_name("orders"), "demo.orders");
        assert_eq!(nats.jetstream.stream, "DEMO");
        assert_eq!(nats.jetstream.durable, "demo-workers");

        let outbox = config.outbox.expect("outbox");
        assert!(outbox.enabled);
        assert_eq!(outbox.batch_size, 50);
        assert_eq!(outbox.interval_ms, 500);
    }

    #[test]
    fn loads_go_zero_style_telemetry_batcher() {
        let source = r#"
name = "demo"

[telemetry]
name = "demo-api"
endpoint = "http://127.0.0.1:4318"
sampler = 0.25
batcher = "otlphttp"
propagator = "jaeger"

[governance]
"#;
        let config: ServiceConfig = config::Config::builder()
            .add_source(config::File::from_str(source, config::FileFormat::Toml))
            .build()
            .expect("build")
            .try_deserialize()
            .expect("deserialize");

        let telemetry = config.telemetry.expect("telemetry");
        assert_eq!(telemetry.name.as_deref(), Some("demo-api"));
        assert_eq!(telemetry.sampler, 0.25);
        assert_eq!(telemetry.batcher, TelemetryBatcher::OtlpHttp);
        assert_eq!(telemetry.propagator, TelemetryPropagator::Jaeger);
    }

    #[test]
    fn loads_rpc_client_defaults() {
        let source = r#"
name = "demo"

[rpc_client]
endpoints = ["127.0.0.1:4000"]

[governance]
"#;
        let config: ServiceConfig = config::Config::builder()
            .add_source(config::File::from_str(source, config::FileFormat::Toml))
            .build()
            .expect("build")
            .try_deserialize()
            .expect("deserialize");

        let client = config.rpc_client.expect("rpc client");
        assert_eq!(client.endpoints, vec!["127.0.0.1:4000"]);
        assert_eq!(client.timeout_ms, default_rpc_client_timeout_ms());
        assert_eq!(
            client.keepalive_time_secs,
            default_rpc_client_keepalive_time_secs()
        );
        assert!(client.middlewares.trace);
        assert!(client.middlewares.breaker);
    }

    #[test]
    fn loads_rpc_client_etcd_config() {
        let source = r#"
name = "demo"

[rpc_client.etcd]
hosts = ["127.0.0.1:2379", "127.0.0.2:2379"]
key = "order.rpc"

[governance]
"#;
        let config: ServiceConfig = config::Config::builder()
            .add_source(config::File::from_str(source, config::FileFormat::Toml))
            .build()
            .expect("build")
            .try_deserialize()
            .expect("deserialize");

        let etcd = config.rpc_client.expect("rpc client").etcd.expect("etcd");
        assert_eq!(etcd.hosts, vec!["127.0.0.1:2379", "127.0.0.2:2379"]);
        assert_eq!(etcd.key, "order.rpc");
        assert_eq!(etcd.pass, None);
    }
}
