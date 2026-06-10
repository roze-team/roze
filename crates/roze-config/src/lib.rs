use std::{collections::BTreeMap, net::SocketAddr, path::Path};

use serde::{Deserialize, Serialize};

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
    pub governance: GovernanceConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RestConfig {
    pub addr: SocketAddr,
    pub register: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcConfig {
    pub addr: SocketAddr,
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

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum DatabaseReadPolicy {
    RoundRobin,
    Random,
}

impl Default for DatabaseReadPolicy {
    fn default() -> Self {
        Self::RoundRobin
    }
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
    pub rate_limit: Option<RateLimitConfig>,
    #[serde(default)]
    pub breaker: Option<BreakerConfig>,
    #[serde(default)]
    pub routes: BTreeMap<String, RouteGovernanceConfig>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RouteGovernanceConfig {
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    #[serde(default)]
    pub rate_limit: Option<RateLimitConfig>,
    #[serde(default)]
    pub breaker: Option<BreakerConfig>,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
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

fn default_rpc_client_timeout_ms() -> u64 {
    2_000
}

fn default_rpc_client_keepalive_time_secs() -> u64 {
    20
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

fn default_rate_limit_burst() -> u32 {
    100
}

fn default_rate_limit_refill_ms() -> u64 {
    10
}

fn default_breaker_failure_threshold() -> u32 {
    5
}

fn default_breaker_reset_timeout_ms() -> u64 {
    30_000
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

[governance.rate_limit]
burst = 10

[governance.routes.login.breaker]
failure_threshold = 2
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
        assert_eq!(
            governance.rate_limit.expect("rate limit").refill_ms,
            default_rate_limit_refill_ms()
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
}
