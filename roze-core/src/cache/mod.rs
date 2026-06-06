use std::time::Duration;

use redis::AsyncCommands;
use serde::{de::DeserializeOwned, Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheConfig {
    pub url: String,
    #[serde(default = "default_namespace")]
    pub namespace: String,
    #[serde(default = "default_ttl_secs")]
    pub default_ttl_secs: u64,
}

#[derive(Debug, Clone)]
pub struct RedisCache {
    client: redis::Client,
    config: CacheConfig,
}

impl RedisCache {
    pub async fn connect(config: &CacheConfig) -> anyhow::Result<Self> {
        let client = redis::Client::open(config.url.as_str())?;
        Ok(Self {
            client,
            config: config.clone(),
        })
    }

    pub async fn get_json<T>(&self, key: &str) -> anyhow::Result<Option<T>>
    where
        T: DeserializeOwned,
    {
        let mut conn = self.client.get_multiplexed_async_connection().await?;
        let value: Option<String> = conn.get(self.key(key)).await?;
        Ok(match value {
            Some(value) => Some(serde_json::from_str(&value)?),
            None => None,
        })
    }

    pub async fn set_json<T>(
        &self,
        key: &str,
        value: &T,
        ttl: Option<Duration>,
    ) -> anyhow::Result<()>
    where
        T: Serialize,
    {
        let mut conn = self.client.get_multiplexed_async_connection().await?;
        let ttl = ttl.unwrap_or_else(|| Duration::from_secs(self.config.default_ttl_secs));
        let payload = serde_json::to_string(value)?;
        let _: () = conn
            .set_ex(self.key(key), payload, ttl.as_secs() as u64)
            .await?;
        Ok(())
    }

    pub async fn del(&self, key: &str) -> anyhow::Result<()> {
        let mut conn = self.client.get_multiplexed_async_connection().await?;
        let _: () = conn.del(self.key(key)).await?;
        Ok(())
    }

    pub fn key(&self, key: &str) -> String {
        format!("{}:{}", self.config.namespace, key)
    }
}

fn default_namespace() -> String {
    "roze".to_string()
}

fn default_ttl_secs() -> u64 {
    300
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn namespaces_keys() {
        let config = CacheConfig {
            url: "redis://127.0.0.1/".to_string(),
            namespace: "roze".to_string(),
            default_ttl_secs: 300,
        };
        let cache = RedisCache {
            client: redis::Client::open(config.url.as_str()).expect("client"),
            config,
        };

        assert_eq!(cache.key("user:1"), "roze:user:1");
    }
}
