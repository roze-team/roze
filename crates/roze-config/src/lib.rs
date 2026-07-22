use std::{
    collections::BTreeMap,
    fmt,
    net::IpAddr,
    net::SocketAddr,
    path::{Path, PathBuf},
    time::Duration,
};

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

pub use roze_resilience::GovernancePolicy;

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
    pub rpc_clients: BTreeMap<String, RpcClientConfig>,
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

impl ServiceConfig {
    pub fn rpc_client_config(&self, name: &str) -> Option<RpcClientConfig> {
        self.rpc_client_config_ref(name).cloned()
    }

    pub fn rpc_client_config_ref(&self, name: &str) -> Option<&RpcClientConfig> {
        self.rpc_clients.get(name).or(self.rpc_client.as_ref())
    }
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
    pub stream_idle_timeout_ms: Option<u64>,
    #[serde(default)]
    pub max_stream_connections: Option<u32>,
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
    pub stream_idle_timeout_ms: Option<u64>,
    #[serde(default)]
    pub max_stream_connections: Option<u32>,
    #[serde(default)]
    pub outlier: Option<GatewayOutlierConfig>,
    #[serde(default)]
    pub health_check: Option<GatewayHealthCheckConfig>,
    #[serde(default)]
    pub tls: Option<GatewayUpstreamTlsConfig>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GatewayUpstreamTlsConfig {
    #[serde(default)]
    pub ca_files: Vec<PathBuf>,
    #[serde(default)]
    pub client_cert_file: Option<PathBuf>,
    #[serde(default)]
    pub client_key_file: Option<PathBuf>,
    #[serde(default)]
    pub server_name: Option<String>,
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
    pub match_headers: BTreeMap<String, String>,
    #[serde(default)]
    pub match_cookies: BTreeMap<String, String>,
    #[serde(default = "default_gateway_route_weight")]
    pub traffic_percent: u32,
    #[serde(default)]
    pub mirror_service: Option<String>,
    #[serde(default)]
    pub mirror_percent: u32,
    #[serde(default)]
    pub instance_tags: BTreeMap<String, String>,
    #[serde(default)]
    pub middlewares: Vec<String>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    #[serde(default)]
    pub stream_idle_timeout_ms: Option<u64>,
    #[serde(default)]
    pub max_stream_connections: Option<u32>,
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
    #[serde(default)]
    pub shedding: Option<SheddingConfig>,
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
    #[serde(default = "default_http_auth_public_routes")]
    pub auth_public_routes: Vec<String>,
    #[serde(default)]
    pub trust_forwarded_identity_headers: bool,
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
            auth_public_routes: default_http_auth_public_routes(),
            trust_forwarded_identity_headers: false,
        }
    }
}

fn default_http_auth_public_routes() -> Vec<String> {
    ["/healthz", "/readyz", "/startupz", "/metrics"]
        .into_iter()
        .map(ToOwned::to_owned)
        .collect()
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
    #[serde(default)]
    pub advertise_addr: Option<SocketAddr>,
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
    pub balancer: RpcClientBalancerKind,
    #[serde(default)]
    pub middlewares: RpcClientMiddlewaresConfig,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RpcClientBalancerKind {
    FirstAvailable,
    RoundRobin,
    WeightedRoundRobin,
    #[default]
    PowerOfTwoChoices,
    HealthAware,
}

#[derive(Clone, Default, Serialize, Deserialize)]
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

impl fmt::Debug for RpcClientEtcdConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RpcClientEtcdConfig")
            .field("hosts", &self.hosts)
            .field("key", &self.key)
            .field("id", &self.id)
            .field("user", &self.user)
            .field("pass", &self.pass.as_ref().map(|_| "[REDACTED]"))
            .field("cert_file", &self.cert_file)
            .field("cert_key_file", &self.cert_key_file)
            .field("ca_cert_file", &self.ca_cert_file)
            .field("insecure_skip_verify", &self.insecure_skip_verify)
            .finish()
    }
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

#[derive(Clone, Serialize, Deserialize)]
pub struct RegistryConfig {
    pub kind: RegistryKind,
    #[serde(default)]
    pub endpoints: Vec<String>,
    #[serde(default = "default_registry_prefix")]
    pub prefix: String,
    #[serde(default = "default_registry_ttl_secs")]
    pub ttl_seconds: u64,
    #[serde(default = "default_registry_renew_interval_secs")]
    pub renew_interval_secs: u64,
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

impl fmt::Debug for RegistryConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RegistryConfig")
            .field("kind", &self.kind)
            .field("endpoints", &self.endpoints)
            .field("prefix", &self.prefix)
            .field("ttl_seconds", &self.ttl_seconds)
            .field("renew_interval_secs", &self.renew_interval_secs)
            .field("user", &self.user)
            .field("pass", &self.pass.as_ref().map(|_| "[REDACTED]"))
            .field("cert_file", &self.cert_file)
            .field("cert_key_file", &self.cert_key_file)
            .field("ca_cert_file", &self.ca_cert_file)
            .field("insecure_skip_verify", &self.insecure_skip_verify)
            .finish()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthConfig {
    pub jwt_keys: Vec<JwtKeyConfig>,
    pub jwt_active_key_id: String,
    #[serde(default = "default_jwt_issuer")]
    pub jwt_issuer: String,
    pub jwt_audience: String,
    #[serde(default = "default_jwt_expiration_secs")]
    pub jwt_expiration_secs: u64,
    #[serde(default = "default_jwt_clock_skew_secs")]
    pub jwt_clock_skew_secs: u64,
    #[serde(default)]
    pub revoked_token_ids: Vec<String>,
    #[serde(default)]
    pub api_keys: Option<roze_auth::ApiKeyConfig>,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct JwtKeyConfig {
    pub id: String,
    pub secret: String,
}

impl fmt::Debug for JwtKeyConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("JwtKeyConfig")
            .field("id", &self.id)
            .field("secret", &"[REDACTED]")
            .finish()
    }
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
    #[serde(
        default = "default_kafka_client_id",
        deserialize_with = "deserialize_kafka_client_id"
    )]
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
    #[serde(default = "default_kafka_message_timeout_ms")]
    pub message_timeout_ms: u64,
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

impl GovernanceConfig {
    pub fn resolve_policy(&self, key: &str) -> GovernancePolicy {
        self.resolve_policy_for([key])
    }

    pub fn resolve_policy_for<'a>(
        &self,
        keys: impl IntoIterator<Item = &'a str>,
    ) -> GovernancePolicy {
        let scoped = keys.into_iter().find_map(|key| self.routes.get(key));
        let timeout_ms = scoped
            .and_then(|policy| policy.timeout_ms)
            .or(self.timeout_ms);
        let retry = scoped.and_then(|policy| policy.retry).or(self.retry);
        let rate_limit = scoped
            .and_then(|policy| policy.rate_limit)
            .or(self.rate_limit);
        let breaker = scoped.and_then(|policy| policy.breaker).or(self.breaker);
        let shedding = scoped.and_then(|policy| policy.shedding).or(self.shedding);
        GovernancePolicy {
            timeout: timeout_ms.map(Duration::from_millis),
            retry: retry.map(|config| roze_resilience::RetryPolicy {
                max_attempts: config.max_attempts,
                backoff: Duration::from_millis(config.backoff_ms),
                max_backoff: Duration::from_millis(config.max_backoff_ms),
                budget_percent: config.budget_percent,
            }),
            rate_limit: rate_limit.map(|config| roze_resilience::RateLimitConfig {
                burst: config.burst,
                refill: Duration::from_millis(config.refill_ms),
            }),
            breaker: breaker.map(|config| roze_resilience::BreakerConfig {
                failure_threshold: config.failure_threshold,
                reset_timeout: Duration::from_millis(config.reset_timeout_ms),
            }),
            shedding: shedding.map(|config| roze_resilience::SheddingConfig {
                concurrency: config.concurrency,
                window: Duration::from_millis(config.window_ms),
                min_samples: config.min_samples,
                max_avg_latency: Duration::from_millis(config.max_avg_latency_ms),
                max_failure_ratio_per_mille: config.max_failure_ratio_per_mille,
                cool_down: Duration::from_millis(config.cool_down_ms),
            }),
            fallback: scoped
                .and_then(|policy| policy.fallback.clone())
                .or_else(|| self.fallback.clone())
                .filter(|fallback| fallback.enabled)
                .map(|fallback| roze_resilience::GovernanceFallback {
                    status: fallback.status,
                    body: fallback.body,
                    headers: fallback.headers,
                }),
        }
    }
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

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct GovernanceFallbackConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_gateway_fallback_status")]
    pub status: u16,
    #[serde(default)]
    pub body: Option<Value>,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
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

fn default_jwt_clock_skew_secs() -> u64 {
    30
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

fn default_registry_prefix() -> String {
    "/roze/services".to_string()
}

pub fn http_proxy_environment_diagnostic(endpoint: &str) -> Option<String> {
    let proxy = active_proxy_env()?;
    let host = endpoint_host(endpoint)?;
    if no_proxy_matches(&host) || !looks_internal_host(&host) {
        return None;
    }

    Some(format!(
        "control-plane HTTP request to internal endpoint `{endpoint}` may be routed through {proxy}; add `{host}` to NO_PROXY or clear HTTP_PROXY/HTTPS_PROXY/ALL_PROXY for registry/config-center clients"
    ))
}

fn active_proxy_env() -> Option<&'static str> {
    const PROXY_ENV_KEYS: [&str; 6] = [
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
    ];

    PROXY_ENV_KEYS
        .into_iter()
        .find(|key| std::env::var_os(key).is_some_and(|value| !value.is_empty()))
}

fn endpoint_host(endpoint: &str) -> Option<String> {
    let normalized = if endpoint.starts_with("http://") || endpoint.starts_with("https://") {
        endpoint.to_string()
    } else {
        format!("http://{endpoint}")
    };
    reqwest::Url::parse(&normalized)
        .ok()
        .and_then(|url| url.host_str().map(ToString::to_string))
}

fn no_proxy_matches(host: &str) -> bool {
    let Some(no_proxy) = std::env::var_os("NO_PROXY").or_else(|| std::env::var_os("no_proxy"))
    else {
        return false;
    };
    let host = host.trim_matches('[').trim_matches(']');
    no_proxy
        .to_string_lossy()
        .split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .any(|entry| entry == "*" || entry == host || host.ends_with(entry.trim_start_matches('.')))
}

fn looks_internal_host(host: &str) -> bool {
    let host = host.trim_matches('[').trim_matches(']');
    if matches!(host, "localhost" | "local") || host.ends_with(".local") {
        return true;
    }
    match host.parse::<IpAddr>() {
        Ok(IpAddr::V4(ip)) => {
            ip.is_private() || ip.is_loopback() || ip.is_link_local() || ip.octets()[0] == 169
        }
        Ok(IpAddr::V6(ip)) => {
            ip.is_loopback() || ip.is_unique_local() || ip.is_unicast_link_local()
        }
        Err(_) => false,
    }
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

fn deserialize_kafka_client_id<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(Option::<String>::deserialize(deserializer)?.unwrap_or_else(default_kafka_client_id))
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

fn default_kafka_message_timeout_ms() -> u64 {
    30_000
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
    let path = path.as_ref();
    let dependency_defaults = path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("config/roze-dependencies.yaml");
    let mut builder = config::Config::builder();
    if dependency_defaults.is_file() {
        builder = builder.add_source(config::File::from(dependency_defaults));
    }
    builder
        .add_source(config::File::from(path))
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
    fn service_config_overrides_generated_dependency_defaults() {
        let root = std::env::temp_dir().join(format!(
            "roze-config-dependencies-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        fs::create_dir_all(root.join("config")).expect("create config directory");
        fs::write(
            root.join("config/roze-dependencies.yaml"),
            "name: defaults\nrpc_clients:\n  order:\n    endpoints: [127.0.0.1:4002]\n    timeout_ms: 1000\n",
        )
        .expect("write dependency defaults");
        fs::write(
            root.join("config.yaml"),
            "name: payment\nrpc_clients:\n  order:\n    timeout_ms: 2500\n",
        )
        .expect("write service config");

        let value: serde_json::Value = load(root.join("config.yaml")).expect("load merged config");
        assert_eq!(value["name"], "payment");
        assert_eq!(value["rpc_clients"]["order"]["timeout_ms"], 2500);
        assert_eq!(
            value["rpc_clients"]["order"]["endpoints"][0],
            "127.0.0.1:4002"
        );
        fs::remove_dir_all(root).expect("remove fixture");
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
status = 598
body = { code = 598, message = "governed" }
headers = { "x-fallback" = "governance" }

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
        let fallback = governance.fallback.expect("fallback");
        assert!(fallback.enabled);
        assert_eq!(fallback.status, 598);
        assert_eq!(fallback.body.expect("fallback body")["message"], "governed");
        assert_eq!(
            fallback.headers.get("x-fallback").map(String::as_str),
            Some("governance")
        );
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
    fn resolves_governance_policy_with_scoped_precedence() {
        let mut governance = GovernanceConfig {
            timeout_ms: Some(1_000),
            retry: Some(RetryConfig {
                max_attempts: 3,
                ..RetryConfig::default()
            }),
            rate_limit: Some(RateLimitConfig {
                burst: 100,
                refill_ms: 1_000,
            }),
            fallback: Some(GovernanceFallbackConfig {
                enabled: true,
                status: 503,
                ..GovernanceFallbackConfig::default()
            }),
            ..GovernanceConfig::default()
        };
        governance.routes.insert(
            "users".to_string(),
            RouteGovernanceConfig {
                timeout_ms: Some(250),
                retry: Some(RetryConfig {
                    max_attempts: 2,
                    ..RetryConfig::default()
                }),
                ..RouteGovernanceConfig::default()
            },
        );

        let policy = governance.resolve_policy_for(["/users", "users", "user-service"]);
        assert_eq!(policy.timeout, Some(Duration::from_millis(250)));
        assert_eq!(policy.retry.expect("retry").max_attempts, 2);
        assert_eq!(policy.rate_limit.expect("rate limit").burst, 100);
        assert_eq!(policy.fallback.expect("fallback").status, 503);

        governance
            .routes
            .get_mut("users")
            .expect("users policy")
            .fallback = Some(GovernanceFallbackConfig {
            enabled: false,
            ..GovernanceFallbackConfig::default()
        });
        assert!(governance.resolve_policy("users").fallback.is_none());

        governance.fallback.as_mut().expect("fallback").enabled = false;
        assert!(governance.resolve_policy("missing").fallback.is_none());
    }

    #[test]
    fn loads_auth_api_key_config() {
        let source = r#"
name = "demo"

[auth]
jwt_active_key_id = "2026-07"
jwt_issuer = "issuer"
jwt_audience = "demo-api"

[[auth.jwt_keys]]
id = "2026-07"
secret = "secret"

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
    fn debug_redacts_jwt_secret() {
        let key = JwtKeyConfig {
            id: "active".into(),
            secret: "super-secret-jwt-key".into(),
        };

        let rendered = format!("{key:?}");
        assert!(!rendered.contains("super-secret-jwt-key"));
        assert!(rendered.contains("[REDACTED]"));
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
    fn kafka_client_id_null_uses_default() {
        let source = r#"
name: demo
kafka:
  brokers: ["127.0.0.1:9092"]
  client_id: null
governance: {}
"#;
        let config: ServiceConfig = config::Config::builder()
            .add_source(config::File::from_str(source, config::FileFormat::Yaml))
            .build()
            .expect("build")
            .try_deserialize()
            .expect("deserialize");

        assert_eq!(
            config.kafka.expect("kafka").client_id,
            default_kafka_client_id()
        );
    }

    #[test]
    fn kafka_message_timeout_defaults_for_disconnect_recovery() {
        let source = r#"
name: demo
kafka:
  brokers: ["127.0.0.1:9092"]
  client_id: worker
governance: {}
"#;
        let config: ServiceConfig = config::Config::builder()
            .add_source(config::File::from_str(source, config::FileFormat::Yaml))
            .build()
            .expect("build")
            .try_deserialize()
            .expect("deserialize");

        assert_eq!(config.kafka.expect("kafka").message_timeout_ms, 30_000);
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
balancer = "health_aware"

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
        assert_eq!(client.balancer, RpcClientBalancerKind::HealthAware);
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

    #[test]
    fn loads_named_rpc_client_configs() {
        let source = r#"
name: demo
rpc_client:
  endpoints: ["127.0.0.1:4000"]
rpc_clients:
  order:
    etcd:
      hosts: ["http://127.0.0.1:2379"]
      key: shop-order-rpc
    timeout_ms: 1500
    balancer: weighted_round_robin
  payment:
    endpoints: ["127.0.0.1:4005"]
governance: {}
"#;
        let config: ServiceConfig = config::Config::builder()
            .add_source(config::File::from_str(source, config::FileFormat::Yaml))
            .build()
            .expect("build")
            .try_deserialize()
            .expect("deserialize");

        let order = config.rpc_client_config("order").expect("order client");
        assert_eq!(order.timeout_ms, 1500);
        assert_eq!(order.balancer, RpcClientBalancerKind::WeightedRoundRobin);
        let order_etcd = order.etcd.expect("order etcd");
        assert_eq!(order_etcd.key, "shop-order-rpc");
        assert_eq!(order_etcd.hosts, vec!["http://127.0.0.1:2379"]);

        let payment = config
            .rpc_client_config_ref("payment")
            .expect("payment client");
        assert_eq!(payment.endpoints, vec!["127.0.0.1:4005"]);

        let fallback = config
            .rpc_client_config("catalog")
            .expect("fallback client");
        assert_eq!(fallback.endpoints, vec!["127.0.0.1:4000"]);

        let source = r#"
name: demo
governance: {}
"#;
        let config: ServiceConfig = config::Config::builder()
            .add_source(config::File::from_str(source, config::FileFormat::Yaml))
            .build()
            .expect("build")
            .try_deserialize()
            .expect("deserialize");
        assert!(config.rpc_client_config("missing").is_none());
    }

    #[test]
    fn loads_gateway_upstream_mutual_tls_config() {
        let source = r#"
name: edge
gateway:
  services:
    - name: user
      upstream: "wss://user.internal/ws"
      tls:
        ca_files:
          - certs/internal-ca.pem
        client_cert_file: certs/gateway-client.pem
        client_key_file: certs/gateway-client.key
        server_name: user.internal
governance: {}
"#;
        let config: ServiceConfig = config::Config::builder()
            .add_source(config::File::from_str(source, config::FileFormat::Yaml))
            .build()
            .expect("build")
            .try_deserialize()
            .expect("deserialize");

        let tls = config
            .gateway
            .expect("gateway")
            .services
            .into_iter()
            .next()
            .expect("service")
            .tls
            .expect("tls");
        assert_eq!(tls.ca_files, vec![PathBuf::from("certs/internal-ca.pem")]);
        assert_eq!(
            tls.client_cert_file,
            Some(PathBuf::from("certs/gateway-client.pem"))
        );
        assert_eq!(
            tls.client_key_file,
            Some(PathBuf::from("certs/gateway-client.key"))
        );
        assert_eq!(tls.server_name.as_deref(), Some("user.internal"));
    }

    #[test]
    fn loads_rpc_advertise_addr() {
        let source = r#"
name = "demo"

[rpc]
addr = "0.0.0.0:4000"
advertise_addr = "192.168.1.10:4000"

[governance]
"#;
        let config: ServiceConfig = config::Config::builder()
            .add_source(config::File::from_str(source, config::FileFormat::Toml))
            .build()
            .expect("build")
            .try_deserialize()
            .expect("deserialize");

        let rpc = config.rpc.expect("rpc");
        assert_eq!(rpc.addr.to_string(), "0.0.0.0:4000");
        assert_eq!(
            rpc.advertise_addr.expect("advertise addr").to_string(),
            "192.168.1.10:4000"
        );
    }

    #[test]
    fn registry_prefix_defaults_and_can_be_overridden() {
        let source = r#"
name = "demo"

[registry]
kind = "etcd"
endpoints = ["127.0.0.1:2379"]

[governance]
"#;
        let config: ServiceConfig = config::Config::builder()
            .add_source(config::File::from_str(source, config::FileFormat::Toml))
            .build()
            .expect("build")
            .try_deserialize()
            .expect("deserialize");

        assert_eq!(
            config.registry.as_ref().expect("registry").prefix,
            "/roze/services"
        );

        let source = r#"
name = "demo"

[registry]
kind = "etcd"
endpoints = ["127.0.0.1:2379"]
prefix = "/shop/services"

[governance]
"#;
        let config: ServiceConfig = config::Config::builder()
            .add_source(config::File::from_str(source, config::FileFormat::Toml))
            .build()
            .expect("build")
            .try_deserialize()
            .expect("deserialize");

        assert_eq!(
            config.registry.as_ref().expect("registry").prefix,
            "/shop/services"
        );
    }

    #[test]
    fn registry_loads_etcd_tls_and_authentication() {
        let source = r#"
name = "demo"

[registry]
kind = "etcd"
endpoints = ["https://etcd.internal:2379"]
user = "roze"
pass = "secret"
cert_file = "certs/client.pem"
cert_key_file = "certs/client.key"
ca_cert_file = "certs/ca.pem"
insecure_skip_verify = true

[governance]
"#;
        let config: ServiceConfig = config::Config::builder()
            .add_source(config::File::from_str(source, config::FileFormat::Toml))
            .build()
            .expect("build")
            .try_deserialize()
            .expect("deserialize");

        let registry = config.registry.expect("registry");
        assert_eq!(registry.user.as_deref(), Some("roze"));
        assert_eq!(registry.pass.as_deref(), Some("secret"));
        assert_eq!(registry.cert_file.as_deref(), Some("certs/client.pem"));
        assert_eq!(registry.cert_key_file.as_deref(), Some("certs/client.key"));
        assert_eq!(registry.ca_cert_file.as_deref(), Some("certs/ca.pem"));
        assert!(registry.insecure_skip_verify);
        let debug = format!("{registry:?}");
        assert!(!debug.contains("secret"));
        assert!(debug.contains("[REDACTED]"));
    }

    #[test]
    fn proxy_diagnostic_warns_for_internal_endpoint_without_no_proxy() {
        static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _guard = ENV_LOCK.lock().expect("env lock");
        let old_http_proxy = std::env::var_os("HTTP_PROXY");
        let old_no_proxy = std::env::var_os("NO_PROXY");

        std::env::set_var("HTTP_PROXY", "http://127.0.0.1:9");
        std::env::set_var("NO_PROXY", "localhost,127.0.0.1,::1");
        let hint =
            http_proxy_environment_diagnostic("http://192.168.1.166:2379").expect("proxy hint");
        assert!(hint.contains("192.168.1.166"));
        assert!(hint.contains("NO_PROXY"));

        std::env::set_var("NO_PROXY", "localhost,127.0.0.1,::1,192.168.1.166");
        assert!(http_proxy_environment_diagnostic("http://192.168.1.166:2379").is_none());

        match old_http_proxy {
            Some(value) => std::env::set_var("HTTP_PROXY", value),
            None => std::env::remove_var("HTTP_PROXY"),
        }
        match old_no_proxy {
            Some(value) => std::env::set_var("NO_PROXY", value),
            None => std::env::remove_var("NO_PROXY"),
        }
    }
}
