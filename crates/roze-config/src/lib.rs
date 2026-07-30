use std::{
    any::Any,
    collections::BTreeMap,
    error::Error,
    fmt, fs,
    net::IpAddr,
    net::SocketAddr,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::Context as _;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use thiserror::Error as ThisError;

pub use roze_resilience::GovernancePolicy;

pub mod config_center;

#[derive(Clone, Serialize, Deserialize)]
pub struct ServiceConfig {
    pub name: String,
    #[serde(default)]
    pub profile: ServiceProfile,
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
    pub ai: Option<AiConfig>,
    #[serde(default)]
    pub kafka: Option<KafkaConfig>,
    #[serde(default)]
    pub nats: Option<roze_nats::NatsConfig>,
    #[serde(default)]
    pub outbox: Option<OutboxConfig>,
    #[serde(default)]
    pub idempotency: Option<IdempotencyConfig>,
    #[serde(default)]
    pub storage: Option<roze_storage::StorageConfig>,
    #[serde(default)]
    pub gateway: Option<GatewayConfig>,
    #[serde(default)]
    pub telemetry: Option<TelemetryConfig>,
    pub governance: GovernanceConfig,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ServiceProfile {
    #[default]
    Development,
    Test,
    Production,
}

impl ServiceProfile {
    pub fn is_production(self) -> bool {
        self == Self::Production
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Development => "development",
            Self::Test => "test",
            Self::Production => "production",
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AiConfig {
    #[serde(default = "default_ai_provider")]
    pub default_provider: String,
    #[serde(default = "default_ai_max_steps")]
    pub max_steps: usize,
    #[serde(default)]
    pub providers: BTreeMap<String, AiProviderConfig>,
}

impl AiConfig {
    pub fn default_provider_config(&self) -> Option<&AiProviderConfig> {
        self.providers.get(&self.default_provider)
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            !self.default_provider.trim().is_empty(),
            "ai.default_provider must not be empty"
        );
        anyhow::ensure!(
            (1..=64).contains(&self.max_steps),
            "ai.max_steps must be between 1 and 64"
        );
        anyhow::ensure!(
            !self.providers.is_empty(),
            "ai.providers must contain at least one provider"
        );
        anyhow::ensure!(
            self.providers.contains_key(&self.default_provider),
            "ai.default_provider `{}` is not declared in ai.providers",
            self.default_provider
        );
        for (name, provider) in &self.providers {
            anyhow::ensure!(
                !name.trim().is_empty(),
                "ai.providers contains an empty provider name"
            );
            provider
                .validate()
                .with_context(|| format!("invalid ai.providers.{name}"))?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AiProviderKind {
    #[default]
    OpenaiCompatible,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct AiProviderConfig {
    #[serde(default)]
    pub kind: AiProviderKind,
    pub base_url: String,
    #[serde(default)]
    pub api_key: Option<String>,
    pub model: String,
    #[serde(default = "default_ai_timeout_ms")]
    pub timeout_ms: u64,
}

impl AiProviderConfig {
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            !self.base_url.trim().is_empty(),
            "base_url must not be empty"
        );
        let url = reqwest::Url::parse(&self.base_url)
            .with_context(|| format!("base_url `{}` is not a valid URL", self.base_url))?;
        anyhow::ensure!(
            matches!(url.scheme(), "http" | "https"),
            "base_url must use http or https"
        );
        anyhow::ensure!(
            url.username().is_empty() && url.password().is_none(),
            "base_url must not contain credentials"
        );
        anyhow::ensure!(!self.model.trim().is_empty(), "model must not be empty");
        anyhow::ensure!(self.timeout_ms > 0, "timeout_ms must be greater than zero");
        Ok(())
    }
}

impl fmt::Debug for AiProviderConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AiProviderConfig")
            .field("kind", &self.kind)
            .field("base_url", &self.base_url)
            .field("api_key", &self.api_key.as_ref().map(|_| "[REDACTED]"))
            .field("model", &self.model)
            .field("timeout_ms", &self.timeout_ms)
            .finish()
    }
}

impl ServiceConfig {
    pub fn rpc_client_config(&self, name: &str) -> Option<RpcClientConfig> {
        self.rpc_client_config_ref(name).cloned()
    }

    pub fn rpc_client_config_ref(&self, name: &str) -> Option<&RpcClientConfig> {
        self.rpc_clients.get(name).or(self.rpc_client.as_ref())
    }

    pub fn resolved_rate_limiter_config(&self) -> roze_rate_limit::RateLimiterConfig {
        let mut config = self.governance.rate_limiter.clone();
        if config
            .redis_url
            .as_deref()
            .is_none_or(|url| url.trim().is_empty())
            && matches!(
                config.store,
                roze_rate_limit::RateLimitStoreKind::Auto
                    | roze_rate_limit::RateLimitStoreKind::Redis
            )
        {
            config.redis_url = self.cache.as_ref().map(|cache| cache.url.clone());
        }
        if config.namespace.is_none() {
            config.namespace = Some(self.profile.as_str().to_string());
        }
        config
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            !self.name.trim().is_empty(),
            "service name must not be empty"
        );
        self.governance.validate()?;
        if let Some(ai) = &self.ai {
            ai.validate()?;
        }
        if self.rpc.is_some() && self.rest.is_none() {
            let rpc_uses_client_ip = self
                .governance
                .rate_limit
                .iter()
                .chain(
                    self.governance
                        .routes
                        .values()
                        .filter_map(|policy| policy.rate_limit.as_ref()),
                )
                .any(|limit| {
                    limit
                        .key
                        .dimensions
                        .contains(&roze_rate_limit::RateLimitDimension::ClientIp)
                });
            anyhow::ensure!(
                !rpc_uses_client_ip,
                "RPC rate-limit policies cannot use the client_ip dimension; use route, subject, tenant, or trusted metadata"
            );
        }
        let rate_limiter = self.resolved_rate_limiter_config();
        rate_limiter.validate()?;
        if self.profile.is_production()
            && self.governance.uses_rate_limit()
            && rate_limiter.resolved_store_kind() == roze_rate_limit::RateLimitStoreKind::Memory
        {
            anyhow::bail!(
                "production services with rate limiting require Redis; configure governance.rate_limiter.redis_url or cache.url"
            );
        }
        Ok(())
    }
}

impl fmt::Debug for ServiceConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ServiceConfig")
            .field("name", &self.name)
            .field("profile", &self.profile)
            .field("rest", &self.rest.is_some())
            .field("rpc", &self.rpc.is_some())
            .field("rpc_client", &self.rpc_client.is_some())
            .field("rpc_clients", &self.rpc_clients.keys().collect::<Vec<_>>())
            .field("registry", &self.registry.is_some())
            .field("database", &self.database.is_some())
            .field("mongo", &self.mongo.is_some())
            .field("cache", &self.cache.is_some())
            .field("auth", &self.auth.is_some())
            .field("ai", &self.ai.is_some())
            .field("kafka", &self.kafka.is_some())
            .field("nats", &self.nats.is_some())
            .field("outbox", &self.outbox.is_some())
            .field("idempotency", &self.idempotency.is_some())
            .field("storage", &self.storage.is_some())
            .field("gateway", &self.gateway.is_some())
            .field("telemetry", &self.telemetry.is_some())
            .field("governance", &self.governance)
            .finish()
    }
}

fn default_ai_provider() -> String {
    "default".to_string()
}

fn default_ai_max_steps() -> usize {
    8
}

fn default_ai_timeout_ms() -> u64 {
    30_000
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
    #[serde(default)]
    pub trusted_proxy_cidrs: Vec<String>,
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
    /// Inject the accepted TCP peer as `ConnectInfo<SocketAddr>`.
    #[serde(default)]
    pub connect_info: bool,
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
    /// Networks allowed to supply `X-Forwarded-For`.
    ///
    /// An empty list means no forwarding header is trusted.
    #[serde(default)]
    pub trusted_proxy_cidrs: Vec<String>,
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
            trusted_proxy_cidrs: Vec::new(),
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
    #[serde(default)]
    pub store: OutboxStoreKind,
    #[serde(default = "default_outbox_table")]
    pub table: String,
    #[serde(default = "default_outbox_max_attempts")]
    pub max_attempts: u32,
    #[serde(default = "default_true")]
    pub migrate: bool,
    #[serde(default = "default_outbox_batch_size")]
    pub batch_size: usize,
    #[serde(default = "default_outbox_interval_ms")]
    pub interval_ms: u64,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OutboxStoreKind {
    #[default]
    Auto,
    Memory,
    Sql,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IdempotencyConfig {
    #[serde(default)]
    pub store: IdempotencyStoreKind,
    #[serde(default = "default_idempotency_key_prefix")]
    pub key_prefix: String,
    #[serde(default = "default_idempotency_record_ttl_millis")]
    pub record_ttl_millis: u64,
    #[serde(default)]
    pub unavailable_policy: IdempotencyUnavailablePolicy,
}

impl Default for IdempotencyConfig {
    fn default() -> Self {
        Self {
            store: IdempotencyStoreKind::Auto,
            key_prefix: default_idempotency_key_prefix(),
            record_ttl_millis: default_idempotency_record_ttl_millis(),
            unavailable_policy: IdempotencyUnavailablePolicy::FailFast,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IdempotencyStoreKind {
    #[default]
    Auto,
    Memory,
    Redis,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IdempotencyUnavailablePolicy {
    #[default]
    FailFast,
    FailClosed,
}

#[derive(Clone, Serialize, Deserialize)]
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

impl fmt::Debug for RpcClientConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RpcClientConfig")
            .field("etcd", &self.etcd)
            .field("endpoint_count", &self.endpoints.len())
            .field("target", &self.target)
            .field("app", &self.app)
            .field("token", &self.token.as_ref().map(|_| "[REDACTED]"))
            .field("non_block", &self.non_block)
            .field("timeout_ms", &self.timeout_ms)
            .field("keepalive_time_secs", &self.keepalive_time_secs)
            .field("balancer", &self.balancer)
            .field("middlewares", &self.middlewares)
            .finish()
    }
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
            .field("host_count", &self.hosts.len())
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
            .field("endpoint_count", &self.endpoints.len())
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

#[derive(Clone, Serialize, Deserialize)]
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

impl fmt::Debug for KafkaConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("KafkaConfig")
            .field("broker_count", &self.brokers.len())
            .field("bootstrap", &self.bootstrap.as_ref().map(|_| "[REDACTED]"))
            .field(
                "bootstrap_server_count",
                &self
                    .bootstrap_servers
                    .as_ref()
                    .map_or(0, std::vec::Vec::len),
            )
            .field("topic_prefix", &self.topic_prefix)
            .field("group_id", &self.group_id)
            .field("client_id", &self.client_id)
            .field("acks", &self.acks)
            .field("auto_offset_reset", &self.auto_offset_reset)
            .field("enable_auto_commit", &self.enable_auto_commit)
            .field("enable_manual_ack", &self.enable_manual_ack)
            .field("linger_ms", &self.linger_ms)
            .field("batch_size", &self.batch_size)
            .field("session_timeout_ms", &self.session_timeout_ms)
            .field("heartbeat_interval_ms", &self.heartbeat_interval_ms)
            .field("max_poll_interval_ms", &self.max_poll_interval_ms)
            .field("flush_timeout_ms", &self.flush_timeout_ms)
            .field("message_timeout_ms", &self.message_timeout_ms)
            .field("max_retries", &self.max_retries)
            .field("retry_backoff_ms", &self.retry_backoff_ms)
            .field("retry_topic", &self.retry_topic)
            .field("dead_letter_topic", &self.dead_letter_topic)
            .field("topic_regex", &self.topic_regex)
            .field("consumer_workers", &self.consumer_workers)
            .finish()
    }
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

#[derive(Clone, Serialize, Deserialize)]
pub struct CacheConfig {
    pub url: String,
    #[serde(default = "default_cache_namespace")]
    pub namespace: String,
    #[serde(default = "default_cache_ttl_secs")]
    pub default_ttl_secs: u64,
}

impl fmt::Debug for CacheConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CacheConfig")
            .field("url", &"[REDACTED]")
            .field("namespace", &self.namespace)
            .field("default_ttl_secs", &self.default_ttl_secs)
            .finish()
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct DatabaseConfig {
    #[serde(default)]
    pub mode: DatabaseMode,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub replicas: Vec<String>,
    #[serde(default)]
    pub topology: Option<DatabaseTopologyConfig>,
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

impl fmt::Debug for DatabaseConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DatabaseConfig")
            .field("mode", &self.mode)
            .field("url", &"[REDACTED]")
            .field("replica_count", &self.replicas.len())
            .field(
                "topology_name",
                &self
                    .topology
                    .as_ref()
                    .map(|topology| topology.name.as_str()),
            )
            .field(
                "topology_shard_count",
                &self
                    .topology
                    .as_ref()
                    .map_or(0, |topology| topology.shards.len()),
            )
            .field("policy", &self.policy)
            .field("max_connections", &self.max_connections)
            .field("min_connections", &self.min_connections)
            .field("connect_timeout_secs", &self.connect_timeout_secs)
            .field("idle_timeout_secs", &self.idle_timeout_secs)
            .field("sqlx_logging", &self.sqlx_logging)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum DatabaseMode {
    #[default]
    Direct,
    Proxy,
    Sharded,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DatabaseTopologyConfig {
    pub name: String,
    pub routing: DatabaseRouting,
    pub shards: Vec<DatabaseShardConfig>,
}

impl fmt::Debug for DatabaseTopologyConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DatabaseTopologyConfig")
            .field("name", &self.name)
            .field("routing", &self.routing)
            .field(
                "shards",
                &self
                    .shards
                    .iter()
                    .map(|shard| &shard.id)
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum DatabaseRouting {
    Fnv1a64JumpV1,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DatabaseShardConfig {
    pub id: String,
    pub primary: String,
    #[serde(default)]
    pub replicas: Vec<String>,
}

impl fmt::Debug for DatabaseShardConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DatabaseShardConfig")
            .field("id", &self.id)
            .field("primary", &"[REDACTED]")
            .field("replica_count", &self.replicas.len())
            .finish()
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum DatabaseReadPolicy {
    #[default]
    RoundRobin,
    Random,
}

#[derive(Clone, Serialize, Deserialize)]
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

impl fmt::Debug for MongoConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MongoConfig")
            .field("url", &"[REDACTED]")
            .field("database", &self.database)
            .field("max_pool_size", &self.max_pool_size)
            .field("min_pool_size", &self.min_pool_size)
            .field("app_name", &self.app_name)
            .finish()
    }
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
    pub rate_limiter: roze_rate_limit::RateLimiterConfig,
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
            .and_then(|policy| policy.rate_limit.as_ref())
            .or(self.rate_limit.as_ref());
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

    pub fn resolve_rate_limit_config(&self, key: &str) -> Option<RateLimitConfig> {
        self.resolve_rate_limit_config_for([key])
    }

    pub fn resolve_rate_limit_config_for<'a>(
        &self,
        keys: impl IntoIterator<Item = &'a str>,
    ) -> Option<RateLimitConfig> {
        keys.into_iter()
            .find_map(|key| self.routes.get(key))
            .and_then(|policy| policy.rate_limit.clone())
            .or_else(|| self.rate_limit.clone())
    }

    pub fn uses_rate_limit(&self) -> bool {
        self.rate_limit.is_some()
            || self
                .routes
                .values()
                .any(|policy| policy.rate_limit.is_some())
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        validate_governance_policy(
            "governance",
            self.timeout_ms,
            self.retry.as_ref(),
            self.rate_limit.as_ref(),
            self.breaker.as_ref(),
            self.shedding.as_ref(),
            self.fallback.as_ref(),
        )?;
        for (route, policy) in &self.routes {
            anyhow::ensure!(
                !route.trim().is_empty(),
                "governance route key must not be empty"
            );
            validate_governance_policy(
                &format!("governance.routes.{route}"),
                policy.timeout_ms,
                policy.retry.as_ref(),
                policy.rate_limit.as_ref(),
                policy.breaker.as_ref(),
                policy.shedding.as_ref(),
                policy.fallback.as_ref(),
            )?;
        }
        Ok(())
    }
}

fn validate_governance_policy(
    path: &str,
    timeout_ms: Option<u64>,
    retry: Option<&RetryConfig>,
    rate_limit: Option<&RateLimitConfig>,
    breaker: Option<&BreakerConfig>,
    shedding: Option<&SheddingConfig>,
    fallback: Option<&GovernanceFallbackConfig>,
) -> anyhow::Result<()> {
    if let Some(timeout_ms) = timeout_ms {
        anyhow::ensure!(timeout_ms > 0, "{path}.timeout_ms must be positive");
    }
    if let Some(retry) = retry {
        anyhow::ensure!(
            retry.max_attempts > 0,
            "{path}.retry.max_attempts must be positive"
        );
        anyhow::ensure!(
            retry.max_backoff_ms >= retry.backoff_ms,
            "{path}.retry.max_backoff_ms must be greater than or equal to backoff_ms"
        );
        if let Some(percent) = retry.budget_percent {
            anyhow::ensure!(
                percent <= 100,
                "{path}.retry.budget_percent must be in 0..=100"
            );
        }
    }
    if let Some(rate_limit) = rate_limit {
        rate_limit.validate(path)?;
    }
    if let Some(breaker) = breaker {
        anyhow::ensure!(
            breaker.failure_threshold > 0,
            "{path}.breaker.failure_threshold must be positive"
        );
        anyhow::ensure!(
            breaker.reset_timeout_ms > 0,
            "{path}.breaker.reset_timeout_ms must be positive"
        );
    }
    if let Some(shedding) = shedding {
        anyhow::ensure!(
            shedding.concurrency > 0,
            "{path}.shedding.concurrency must be positive"
        );
        anyhow::ensure!(
            shedding.window_ms > 0,
            "{path}.shedding.window_ms must be positive"
        );
        anyhow::ensure!(
            shedding.min_samples > 0,
            "{path}.shedding.min_samples must be positive"
        );
        anyhow::ensure!(
            shedding.max_avg_latency_ms > 0,
            "{path}.shedding.max_avg_latency_ms must be positive"
        );
        anyhow::ensure!(
            shedding.max_failure_ratio_per_mille <= 1_000,
            "{path}.shedding.max_failure_ratio_per_mille must be in 0..=1000"
        );
        anyhow::ensure!(
            shedding.cool_down_ms > 0,
            "{path}.shedding.cool_down_ms must be positive"
        );
    }
    if let Some(fallback) = fallback {
        anyhow::ensure!(
            (100..=599).contains(&fallback.status),
            "{path}.fallback.status must be a valid HTTP status"
        );
    }
    Ok(())
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RateLimitConfig {
    #[serde(default = "default_rate_limit_burst")]
    pub burst: u32,
    #[serde(default = "default_rate_limit_refill_ms")]
    pub refill_ms: u64,
    #[serde(default)]
    pub key: roze_rate_limit::RateLimitKeyPolicy,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            burst: default_rate_limit_burst(),
            refill_ms: default_rate_limit_refill_ms(),
            key: roze_rate_limit::RateLimitKeyPolicy::default(),
        }
    }
}

impl RateLimitConfig {
    fn validate(&self, path: &str) -> anyhow::Result<()> {
        anyhow::ensure!(self.burst > 0, "{path}.rate_limit.burst must be positive");
        anyhow::ensure!(
            self.refill_ms > 0,
            "{path}.rate_limit.refill_ms must be positive"
        );
        self.key
            .validate()
            .map_err(|error| anyhow::anyhow!("{path}.rate_limit.key is invalid: {error}"))
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

fn default_outbox_table() -> String {
    "roze_outbox".to_string()
}

fn default_outbox_max_attempts() -> u32 {
    16
}

fn default_idempotency_key_prefix() -> String {
    "roze:idempotency:v1".to_string()
}

fn default_idempotency_record_ttl_millis() -> u64 {
    86_400_000
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

/// Resolves secret references found in configuration strings.
///
/// Providers must return `Ok(None)` for unsupported references. Error messages
/// must identify the reference, never the resolved secret value.
pub trait SecretProvider: fmt::Debug + Send + Sync {
    fn resolve(
        &self,
        reference: &str,
        base_dir: &Path,
    ) -> Result<Option<String>, SecretProviderError>;
}

#[derive(Debug, ThisError)]
pub enum SecretProviderError {
    #[error("secret environment variable `{name}` is not set")]
    MissingEnvironment { name: String },
    #[error("secret file `{path}` could not be read")]
    FileRead {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("secret reference `{reference}` is invalid")]
    InvalidReference { reference: String },
}

#[derive(Debug, Clone, Copy, Default)]
pub struct EnvironmentAndFileSecretProvider;

impl SecretProvider for EnvironmentAndFileSecretProvider {
    fn resolve(
        &self,
        reference: &str,
        base_dir: &Path,
    ) -> Result<Option<String>, SecretProviderError> {
        let env_name = reference
            .strip_prefix("env://")
            .or_else(|| {
                reference
                    .strip_prefix("${")
                    .and_then(|value| value.strip_suffix('}'))
            })
            .filter(|name| !name.is_empty());
        if let Some(name) = env_name {
            return std::env::var(name).map(Some).map_err(|_| {
                SecretProviderError::MissingEnvironment {
                    name: name.to_string(),
                }
            });
        }
        if reference.starts_with("env://") || reference.starts_with("${") {
            return Err(SecretProviderError::InvalidReference {
                reference: reference.to_string(),
            });
        }

        let Some(path) = reference.strip_prefix("file://") else {
            return Ok(None);
        };
        if path.is_empty() {
            return Err(SecretProviderError::InvalidReference {
                reference: reference.to_string(),
            });
        }
        let path = PathBuf::from(path);
        let path = if path.is_absolute() {
            path
        } else {
            base_dir.join(path)
        };
        fs::read_to_string(&path)
            .map(|value| Some(value.trim_end_matches(['\r', '\n']).to_string()))
            .map_err(|source| SecretProviderError::FileRead { path, source })
    }
}

pub fn load<T>(path: impl AsRef<Path>) -> Result<T, config::ConfigError>
where
    T: for<'de> Deserialize<'de> + 'static,
{
    load_with_secret_provider(path, &EnvironmentAndFileSecretProvider)
}

/// Environment variable used by generated services to select an external
/// runtime configuration file.
pub const SERVICE_CONFIG_PATH_ENV: &str = "ROZE_CONFIG_PATH";

/// Resolves the service configuration path with deployment configuration
/// taking precedence over source-tree and working-directory defaults.
pub fn service_config_path(manifest_dir: impl AsRef<Path>) -> PathBuf {
    resolve_service_config_path(
        std::env::var_os(SERVICE_CONFIG_PATH_ENV),
        manifest_dir.as_ref(),
    )
}

fn resolve_service_config_path(
    configured_path: Option<std::ffi::OsString>,
    manifest_dir: &Path,
) -> PathBuf {
    if let Some(path) = configured_path {
        return PathBuf::from(path);
    }

    let manifest_config = manifest_dir.join("config.yaml");
    if manifest_config.exists() {
        manifest_config
    } else {
        PathBuf::from("config.yaml")
    }
}

pub fn load_service(path: impl AsRef<Path>) -> Result<ServiceConfig, config::ConfigError> {
    load_service_with_secret_provider(path, &EnvironmentAndFileSecretProvider)
}

pub fn load_service_with_secret_provider(
    path: impl AsRef<Path>,
    provider: &dyn SecretProvider,
) -> Result<ServiceConfig, config::ConfigError> {
    load_with_secret_provider(path, provider)
}

pub fn load_with_secret_provider<T>(
    path: impl AsRef<Path>,
    provider: &dyn SecretProvider,
) -> Result<T, config::ConfigError>
where
    T: for<'de> Deserialize<'de> + 'static,
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
    let config = builder
        .add_source(config::File::from(path))
        .add_source(config::Environment::with_prefix("ROZE").separator("__"))
        .build()?;
    let mut value = config.try_deserialize::<Value>()?;
    merge_jwt_key_environment_override(&mut value)?;
    let base_dir = path.parent().unwrap_or_else(|| Path::new("."));
    resolve_secret_references(&mut value, base_dir, provider)?;
    validate_structured_secrets(&value)?;
    deserialize_config_value(value)
}

pub(crate) fn deserialize_config_value<T>(value: Value) -> Result<T, config::ConfigError>
where
    T: for<'de> Deserialize<'de> + 'static,
{
    let strict = value
        .get("profile")
        .and_then(Value::as_str)
        .is_some_and(|profile| profile.eq_ignore_ascii_case("production"));
    let encoded = serde_json::to_vec(&value)
        .map_err(|error| config::ConfigError::Message(error.to_string()))?;
    let mut deserializer = serde_json::Deserializer::from_slice(&encoded);
    let mut unknown = std::collections::BTreeSet::new();
    let config = serde_ignored::deserialize(&mut deserializer, |path| {
        unknown.insert(path.to_string());
    })
    .map_err(|error| config::ConfigError::Message(error.to_string()))?;
    if !unknown.is_empty() {
        let fields = unknown.into_iter().collect::<Vec<_>>().join(", ");
        if strict {
            return Err(config::ConfigError::Message(format!(
                "production configuration contains unknown fields: {fields}"
            )));
        }
        tracing::warn!(unknown_fields = %fields, "configuration contains unknown fields");
    }
    if let Some(service) = (&config as &dyn Any).downcast_ref::<ServiceConfig>() {
        service
            .validate()
            .map_err(|error| config::ConfigError::Message(error.to_string()))?;
    }
    Ok(config)
}

fn resolve_secret_references(
    value: &mut Value,
    base_dir: &Path,
    provider: &dyn SecretProvider,
) -> Result<(), config::ConfigError> {
    match value {
        Value::String(reference) => {
            if let Some(secret) = provider
                .resolve(reference, base_dir)
                .map_err(secret_config_error)?
            {
                *reference = secret;
            }
        }
        Value::Array(values) => {
            for value in values {
                resolve_secret_references(value, base_dir, provider)?;
            }
        }
        Value::Object(values) => {
            for value in values.values_mut() {
                resolve_secret_references(value, base_dir, provider)?;
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
    Ok(())
}

fn secret_config_error(error: SecretProviderError) -> config::ConfigError {
    let mut message = error.to_string();
    if let Some(source) = error.source() {
        message.push_str(": ");
        message.push_str(&source.to_string());
    }
    config::ConfigError::Message(message)
}

fn merge_jwt_key_environment_override(value: &mut Value) -> Result<(), config::ConfigError> {
    let Ok(raw) = std::env::var("ROZE_AUTH_JWT_KEYS") else {
        return Ok(());
    };
    merge_jwt_key_overlay(value, &raw)
}

fn merge_jwt_key_overlay(value: &mut Value, raw: &str) -> Result<(), config::ConfigError> {
    let overlay: Vec<Value> = serde_json::from_str(raw).map_err(|_| {
        config::ConfigError::Message(
            "ROZE_AUTH_JWT_KEYS must be a JSON array of JWT key objects".to_string(),
        )
    })?;
    let Some(root) = value.as_object_mut() else {
        return Ok(());
    };
    let auth = root
        .entry("auth")
        .or_insert_with(|| Value::Object(Default::default()));
    let auth = auth.as_object_mut().ok_or_else(|| {
        config::ConfigError::Message("auth configuration must be an object".to_string())
    })?;
    let keys = auth
        .entry("jwt_keys")
        .or_insert_with(|| Value::Array(Vec::new()));
    let keys = keys.as_array_mut().ok_or_else(|| {
        config::ConfigError::Message("auth.jwt_keys must be an array".to_string())
    })?;
    for replacement in overlay {
        let id = replacement
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.trim().is_empty())
            .ok_or_else(|| {
                config::ConfigError::Message(
                    "ROZE_AUTH_JWT_KEYS entries require a non-empty id".to_string(),
                )
            })?;
        if let Some(existing) = keys.iter_mut().find(|key| {
            key.get("id")
                .and_then(Value::as_str)
                .is_some_and(|current| current == id)
        }) {
            *existing = replacement;
        } else {
            keys.push(replacement);
        }
    }
    Ok(())
}

fn validate_structured_secrets(value: &Value) -> Result<(), config::ConfigError> {
    let Some(auth) = value.get("auth") else {
        return Ok(());
    };
    let keys = auth
        .get("jwt_keys")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            config::ConfigError::Message("auth.jwt_keys must be an array".to_string())
        })?;
    let mut ids = std::collections::BTreeSet::new();
    for key in keys {
        let id = key
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.trim().is_empty())
            .ok_or_else(|| {
                config::ConfigError::Message("JWT key id must not be empty".to_string())
            })?;
        if !ids.insert(id) {
            return Err(config::ConfigError::Message(format!(
                "duplicate JWT key id `{id}`"
            )));
        }
        let secret_length = key
            .get("secret")
            .and_then(Value::as_str)
            .map(str::len)
            .unwrap_or_default();
        if secret_length < 32 {
            return Err(config::ConfigError::Message(format!(
                "JWT key `{id}` secret must contain at least 32 bytes"
            )));
        }
    }
    let active = auth
        .get("jwt_active_key_id")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !ids.contains(active) {
        return Err(config::ConfigError::Message(format!(
            "active JWT key `{active}` was not found"
        )));
    }
    Ok(())
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
    fn runtime_config_path_precedes_manifest_and_loads_external_service_config() {
        static CONFIG_PATH_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _guard = CONFIG_PATH_ENV_LOCK.lock().expect("config path env lock");
        let previous = std::env::var_os(SERVICE_CONFIG_PATH_ENV);
        let root = std::env::temp_dir().join(format!(
            "roze-external-config-path-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        let manifest_dir = root.join("source");
        let deployment_dir = root.join("deployment");
        fs::create_dir_all(&manifest_dir).expect("create manifest dir");
        fs::create_dir_all(&deployment_dir).expect("create deployment dir");
        fs::write(
            manifest_dir.join("config.yaml"),
            "name: source\ngovernance: {}\n",
        )
        .expect("write source config");
        let configured = deployment_dir.join("service.yaml");
        fs::write(&configured, "name: deployment\ngovernance: {}\n")
            .expect("write deployment config");

        std::env::set_var(SERVICE_CONFIG_PATH_ENV, &configured);
        let resolved = service_config_path(&manifest_dir);
        let service = load_service(&resolved).expect("load external service config");
        assert_eq!(resolved, configured);
        assert_eq!(service.name, "deployment");

        match previous {
            Some(value) => std::env::set_var(SERVICE_CONFIG_PATH_ENV, value),
            None => std::env::remove_var(SERVICE_CONFIG_PATH_ENV),
        }
        fs::remove_dir_all(root).expect("remove config root");
    }

    #[test]
    fn runtime_config_path_uses_manifest_then_working_directory() {
        let root = std::env::temp_dir().join(format!(
            "roze-config-path-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        fs::create_dir_all(&root).expect("create config root");
        assert_eq!(
            resolve_service_config_path(None, &root),
            PathBuf::from("config.yaml")
        );

        let manifest_config = root.join("config.yaml");
        fs::write(&manifest_config, "name: test\n").expect("write config");
        assert_eq!(resolve_service_config_path(None, &root), manifest_config);

        fs::remove_dir_all(root).expect("remove config root");
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

    #[derive(Debug)]
    struct TestSecretProvider;

    impl SecretProvider for TestSecretProvider {
        fn resolve(
            &self,
            reference: &str,
            _base_dir: &Path,
        ) -> Result<Option<String>, SecretProviderError> {
            Ok((reference == "test://jwt").then(|| "0123456789abcdef0123456789abcdef".to_string()))
        }
    }

    #[test]
    fn resolves_pluggable_secret_references_before_validation() {
        let path = std::env::temp_dir().join(format!(
            "roze-secret-provider-{}.yaml",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        fs::write(
            &path,
            r#"
name: demo
auth:
  jwt_keys:
    - id: active
      secret: test://jwt
  jwt_active_key_id: active
  jwt_audience: demo
governance: {}
"#,
        )
        .expect("write config");

        let config: ServiceConfig =
            load_with_secret_provider(&path, &TestSecretProvider).expect("load");
        let key = &config.auth.expect("auth").jwt_keys[0];
        assert_eq!(key.secret.len(), 32);
        assert!(!format!("{key:?}").contains(&key.secret));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn resolves_relative_secret_files_and_trims_line_endings() {
        let root = std::env::temp_dir().join(format!(
            "roze-secret-file-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        fs::create_dir_all(&root).expect("create root");
        fs::write(
            root.join("jwt.secret"),
            "0123456789abcdef0123456789abcdef\r\n",
        )
        .expect("write secret");
        fs::write(
            root.join("config.yaml"),
            r#"
name: demo
auth:
  jwt_keys:
    - id: active
      secret: file://jwt.secret
  jwt_active_key_id: active
  jwt_audience: demo
governance: {}
"#,
        )
        .expect("write config");

        let config: ServiceConfig = load(root.join("config.yaml")).expect("load");
        assert_eq!(
            config.auth.expect("auth").jwt_keys[0].secret,
            "0123456789abcdef0123456789abcdef"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn merges_jwt_rotation_keys_by_id_from_one_environment_value() {
        let mut value = serde_json::json!({
            "name": "demo",
            "auth": {
                "jwt_keys": [
                    {"id": "active", "secret": "0123456789abcdef0123456789abcdef"},
                    {"id": "old", "secret": "11111111111111111111111111111111"}
                ],
                "jwt_active_key_id": "active",
                "jwt_audience": "demo"
            },
            "governance": {}
        });
        merge_jwt_key_overlay(
            &mut value,
            r#"[{"id":"active","secret":"abcdef0123456789abcdef0123456789"},{"id":"next","secret":"fedcba9876543210fedcba9876543210"}]"#,
        )
        .expect("merge");
        validate_structured_secrets(&value).expect("validate");
        let config: ServiceConfig = serde_json::from_value(value).expect("deserialize");
        let keys = config.auth.expect("auth").jwt_keys;
        assert_eq!(keys.len(), 3);
        assert_eq!(
            keys.iter()
                .find(|key| key.id == "active")
                .expect("active")
                .secret,
            "abcdef0123456789abcdef0123456789"
        );
        assert!(keys.iter().any(|key| key.id == "old"));
        assert!(keys.iter().any(|key| key.id == "next"));
    }

    #[test]
    fn rejects_short_or_missing_active_jwt_keys_without_exposing_secret() {
        let short = serde_json::json!({
            "auth": {
                "jwt_keys": [{"id": "old", "secret": "too-short"}],
                "jwt_active_key_id": "active"
            }
        });
        let error = validate_structured_secrets(&short).expect_err("reject invalid keys");
        let message = error.to_string();
        assert!(!message.contains("too-short"));
        assert!(
            message.contains("at least 32 bytes") || message.contains("active JWT key"),
            "{message}"
        );
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
    fn loads_distributed_rate_limit_key_policy_without_exposing_redis_url() {
        let source = r#"
name: auth
governance:
  rate_limiter:
    store: redis
    redis_url: redis://user:secret@127.0.0.1:6379
    key_prefix: auth:rate-limit
    timeout_ms: 75
    unavailable_policy: fail-open
  rate_limit:
    burst: 5
    refill_ms: 1000
    key:
      dimensions: [route, client_ip, tenant]
      headers: [x-login-account]
      missing: reject
"#;
        let config: ServiceConfig = config::Config::builder()
            .add_source(config::File::from_str(source, config::FileFormat::Yaml))
            .build()
            .unwrap()
            .try_deserialize()
            .unwrap();
        assert_eq!(
            config.governance.rate_limiter.store,
            roze_rate_limit::RateLimitStoreKind::Redis
        );
        let limit = config.governance.rate_limit.expect("rate limit");
        assert_eq!(
            limit.key.dimensions,
            vec![
                roze_rate_limit::RateLimitDimension::Route,
                roze_rate_limit::RateLimitDimension::ClientIp,
                roze_rate_limit::RateLimitDimension::Tenant,
            ]
        );
        assert_eq!(limit.key.headers, vec!["x-login-account"]);
        let debug = format!("{:?}", config.governance.rate_limiter);
        assert!(!debug.contains("secret"));
        assert!(debug.contains("[REDACTED]"));
    }

    #[test]
    fn production_rate_limiter_auto_reuses_cache_and_scopes_profile() {
        let path = std::env::temp_dir().join(format!(
            "roze-config-rate-limit-auto-{}.yaml",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        fs::write(
            &path,
            r#"
name: auth
profile: production
cache:
  url: redis://user:cache-secret@127.0.0.1:6379
governance:
  rate_limiter:
    store: auto
    key_prefix: roze:rate-limit:v1
  rate_limit:
    burst: 10
    refill_ms: 100
"#,
        )
        .expect("write config");

        let config = load_service(&path).expect("load validated service config");
        let limiter = config.resolved_rate_limiter_config();
        assert_eq!(
            limiter.resolved_store_kind(),
            roze_rate_limit::RateLimitStoreKind::Redis
        );
        assert_eq!(limiter.namespace.as_deref(), Some("production"));
        assert_eq!(
            limiter.redis_url.as_deref(),
            Some("redis://user:cache-secret@127.0.0.1:6379")
        );
        let _ = fs::remove_file(path);
    }

    #[test]
    fn production_service_config_rejects_unknown_and_invalid_governance_fields() {
        let root = std::env::temp_dir().join(format!(
            "roze-config-strict-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        fs::create_dir_all(&root).expect("create root");
        let unknown = root.join("unknown.yaml");
        fs::write(
            &unknown,
            r#"
name: auth
profile: production
governance:
  refil_ms: 100
"#,
        )
        .expect("write unknown field config");
        let error = load_service(&unknown).expect_err("reject unknown production field");
        assert!(error.to_string().contains("governance.refil_ms"));

        let invalid = root.join("invalid.yaml");
        fs::write(
            &invalid,
            r#"
name: auth
profile: production
governance:
  timeout_ms: 0
"#,
        )
        .expect("write invalid governance config");
        let error = load_service(&invalid).expect_err("reject invalid governance");
        assert!(error
            .to_string()
            .contains("governance.timeout_ms must be positive"));
        fs::remove_dir_all(root).expect("remove root");
    }

    #[test]
    fn development_config_keeps_unknown_fields_non_fatal() {
        let path = std::env::temp_dir().join(format!(
            "roze-config-development-unknown-{}.yaml",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        fs::write(
            &path,
            "name: demo\nprofile: development\nunknown_extension: true\ngovernance: {}\n",
        )
        .expect("write development config");
        let config = load_service(&path).expect("development unknown field is warning-only");
        assert_eq!(config.name, "demo");
        let _ = fs::remove_file(path);
    }

    #[test]
    fn service_config_debug_redacts_connection_credentials() {
        let source = r#"
name: payments
rpc_client:
  endpoints: [https://user:rpc-secret@example.test]
  token: rpc-token-secret
cache:
  url: redis://user:cache-secret@example.test
database:
  url: postgres://user:database-secret@example.test/payments
mongo:
  url: mongodb://user:mongo-secret@example.test/payments
  database: payments
kafka:
  brokers: [sasl://user:kafka-secret@example.test]
nats:
  servers: [nats://user:nats-secret@example.test]
storage:
  provider: s3_compatible
  bucket: payments
  access_key: storage-access-secret
  secret_key: storage-secret
governance: {}
"#;
        let value = config::Config::builder()
            .add_source(config::File::from_str(source, config::FileFormat::Yaml))
            .build()
            .expect("build")
            .try_deserialize::<Value>()
            .expect("deserialize value");
        let config: ServiceConfig = deserialize_config_value(value).expect("deserialize config");
        let debug = format!("{config:?}");
        for secret in [
            "rpc-secret",
            "rpc-token-secret",
            "cache-secret",
            "database-secret",
            "mongo-secret",
            "kafka-secret",
            "nats-secret",
            "storage-access-secret",
            "storage-secret",
        ] {
            assert!(!debug.contains(secret), "debug output leaked {secret}");
        }
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
                key: Default::default(),
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
    fn database_sharding_topology_deserializes_explicitly() {
        let source = r#"
name: demo
database:
  mode: sharded
  topology:
    name: commerce
    routing: fnv1a64-jump-v1
    shards:
      - id: shard-00
        primary: postgres://db-00/commerce
        replicas:
          - postgres://db-00-replica/commerce
      - id: shard-01
        primary: postgres://db-01/commerce
governance: {}
"#;
        let config: ServiceConfig = config::Config::builder()
            .add_source(config::File::from_str(source, config::FileFormat::Yaml))
            .build()
            .expect("build")
            .try_deserialize()
            .expect("deserialize");
        let database = config.database.expect("database");
        assert_eq!(database.mode, DatabaseMode::Sharded);
        assert!(database.url.is_empty());
        let topology = database.topology.expect("topology");
        assert_eq!(topology.name, "commerce");
        assert_eq!(topology.routing, DatabaseRouting::Fnv1a64JumpV1);
        assert_eq!(topology.shards.len(), 2);
        assert_eq!(topology.shards[0].id, "shard-00");
    }

    #[test]
    fn ai_provider_config_validates_and_redacts_credentials() {
        let config: ServiceConfig = deserialize_config_value(serde_json::json!({
            "name": "demo",
            "ai": {
                "default_provider": "primary",
                "max_steps": 12,
                "providers": {
                    "primary": {
                        "kind": "openai_compatible",
                        "base_url": "https://api.example.com/v1",
                        "api_key": "secret-value",
                        "model": "example-model",
                        "timeout_ms": 5000
                    }
                }
            },
            "governance": {}
        }))
        .expect("valid service config");

        let ai = config.ai.expect("AI config");
        assert_eq!(ai.max_steps, 12);
        assert_eq!(
            ai.default_provider_config().expect("provider").model,
            "example-model"
        );
        let debug = format!("{:?}", ai.default_provider_config().unwrap());
        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains("secret-value"));
    }

    #[test]
    fn ai_provider_config_rejects_missing_defaults_and_url_credentials() {
        let missing = AiConfig {
            default_provider: "missing".to_string(),
            max_steps: 8,
            providers: BTreeMap::from([(
                "primary".to_string(),
                AiProviderConfig {
                    kind: AiProviderKind::OpenaiCompatible,
                    base_url: "https://api.example.com/v1".to_string(),
                    api_key: None,
                    model: "example-model".to_string(),
                    timeout_ms: 5_000,
                },
            )]),
        };
        assert!(missing
            .validate()
            .unwrap_err()
            .to_string()
            .contains("missing"));

        let credentials = AiProviderConfig {
            kind: AiProviderKind::OpenaiCompatible,
            base_url: "https://user:password@example.com/v1".to_string(),
            api_key: None,
            model: "example-model".to_string(),
            timeout_ms: 5_000,
        };
        assert!(credentials
            .validate()
            .unwrap_err()
            .to_string()
            .contains("must not contain credentials"));
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
