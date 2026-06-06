use std::{net::SocketAddr, path::Path};

use serde::{Deserialize, Serialize};

use crate::db::DatabaseConfig;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceConfig {
    pub name: String,
    #[serde(default)]
    pub rest: Option<RestConfig>,
    #[serde(default)]
    pub rpc: Option<RpcConfig>,
    #[serde(default)]
    pub registry: Option<RegistryConfig>,
    #[serde(default)]
    pub database: Option<DatabaseConfig>,
    #[serde(default)]
    pub cache: Option<CacheConfig>,
    #[serde(default)]
    pub auth: Option<AuthConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RestConfig {
    pub addr: SocketAddr,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcConfig {
    pub addr: SocketAddr,
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
