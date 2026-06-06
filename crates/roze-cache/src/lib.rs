use std::time::Duration;

pub use roze_config::CacheConfig;
use roze_redis::{namespace_key, NamespacedRedisClient, RedisClient};
use roze_singleflight::SingleFlightGroup;
use serde::{de::DeserializeOwned, Serialize};

#[derive(Debug, Clone)]
pub struct RedisCache {
    client: NamespacedRedisClient,
    config: CacheConfig,
    flights: SingleFlightGroup,
}

impl RedisCache {
    pub async fn connect(config: &CacheConfig) -> anyhow::Result<Self> {
        let client = RedisClient::open(config.url.as_str())?.with_namespace(config.namespace.clone());
        Ok(Self {
            client,
            config: config.clone(),
            flights: SingleFlightGroup::new(),
        })
    }

    pub fn key(&self, key: &str) -> String {
        self.client.key(key)
    }

    pub async fn get_json<T>(&self, key: &str) -> anyhow::Result<Option<T>>
    where
        T: DeserializeOwned,
    {
        self.client.get_json(key).await
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
        let ttl = ttl.unwrap_or_else(|| Duration::from_secs(self.config.default_ttl_secs));
        self.client.set_json(key, value, ttl).await
    }

    pub async fn del(&self, key: &str) -> anyhow::Result<()> {
        self.client.del(key).await
    }

    pub async fn get_or_set_json<T, F, Fut>(
        &self,
        key: &str,
        ttl: Option<Duration>,
        loader: F,
    ) -> anyhow::Result<T>
    where
        T: Clone + Serialize + DeserializeOwned + Send + Sync + 'static,
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = anyhow::Result<T>>,
    {
        if let Some(value) = self.get_json(key).await? {
            return Ok(value);
        }

        let cache_key = namespace_key(&self.config.namespace, key);
        let result = self
            .flights
            .do_call(cache_key.clone(), || async {
                if let Some(value) = self.get_json(key).await.map_err(|err| err.to_string())? {
                    return Ok(value);
                }

                let value = loader().await.map_err(|err| err.to_string())?;
                self.set_json(key, &value, ttl).await.map_err(|err| err.to_string())?;
                Ok(value)
            })
            .await
            .map_err(anyhow::Error::msg)?;

        Ok(result)
    }
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
            client: RedisClient::open(config.url.as_str())
                .expect("client")
                .with_namespace(config.namespace.clone()),
            config,
            flights: SingleFlightGroup::new(),
        };

        assert_eq!(cache.key("user:1"), "roze:user:1");
    }
}
