use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

pub use roze_config::CacheConfig;
use roze_redis::{namespace_key, NamespacedRedisClient, RedisClient};
use roze_singleflight::SingleFlightGroup;
use serde::{de::DeserializeOwned, Deserialize, Serialize};

#[derive(Debug, Clone)]
pub struct RedisCache {
    client: NamespacedRedisClient,
    config: CacheConfig,
    flights: SingleFlightGroup,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum CachedEnvelope<T> {
    Value(T),
    Missing,
}

impl RedisCache {
    pub async fn connect(config: &CacheConfig) -> anyhow::Result<Self> {
        let client =
            RedisClient::open(config.url.as_str())?.with_namespace(config.namespace.clone());
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

    pub async fn set_json_jittered<T>(
        &self,
        key: &str,
        value: &T,
        ttl: Option<Duration>,
        jitter_ratio: f64,
    ) -> anyhow::Result<()>
    where
        T: Serialize,
    {
        let ttl = ttl.unwrap_or_else(|| Duration::from_secs(self.config.default_ttl_secs));
        let ttl = jittered_ttl(self.key(key).as_str(), ttl, jitter_ratio);
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
        self.get_or_set_json_option(key, ttl, None, || async { loader().await.map(Some) })
            .await?
            .ok_or_else(|| anyhow::anyhow!("loader returned no value for cache key `{key}`"))
    }

    pub async fn get_or_set_json_option<T, F, Fut>(
        &self,
        key: &str,
        ttl: Option<Duration>,
        negative_ttl: Option<Duration>,
        loader: F,
    ) -> anyhow::Result<Option<T>>
    where
        T: Clone + Serialize + DeserializeOwned + Send + Sync + 'static,
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = anyhow::Result<Option<T>>>,
    {
        if let Some(envelope) = self.get_envelope::<T>(key).await? {
            return Ok(match envelope {
                CachedEnvelope::Value(value) => Some(value),
                CachedEnvelope::Missing => None,
            });
        }

        let cache_key = namespace_key(&self.config.namespace, key);
        let result = self
            .flights
            .do_call(cache_key.clone(), || async {
                if let Some(envelope) = self
                    .get_envelope::<T>(key)
                    .await
                    .map_err(|err| err.to_string())?
                {
                    return Ok(match envelope {
                        CachedEnvelope::Value(value) => Some(value),
                        CachedEnvelope::Missing => None,
                    });
                }

                let loaded = loader().await.map_err(|err| err.to_string())?;
                match loaded {
                    Some(value) => {
                        let ttl = ttl
                            .unwrap_or_else(|| Duration::from_secs(self.config.default_ttl_secs));
                        let ttl = jittered_ttl(self.key(key).as_str(), ttl, 0.05);
                        self.set_envelope(key, &CachedEnvelope::Value(value.clone()), Some(ttl))
                            .await
                            .map_err(|err| err.to_string())?;
                        Ok(Some(value))
                    }
                    None => {
                        let negative_cache_ttl = negative_ttl.unwrap_or_else(|| {
                            Duration::from_secs(
                                self.config.default_ttl_secs.saturating_div(6).clamp(5, 60),
                            )
                        });
                        let ttl = default_negative_ttl(negative_cache_ttl);
                        self.set_envelope(key, &CachedEnvelope::<T>::Missing, Some(ttl))
                            .await
                            .map_err(|err| err.to_string())?;
                        Ok(None)
                    }
                }
            })
            .await
            .map_err(anyhow::Error::msg)?;

        Ok(result)
    }

    async fn set_envelope<T>(
        &self,
        key: &str,
        value: &CachedEnvelope<T>,
        ttl: Option<Duration>,
    ) -> anyhow::Result<()>
    where
        T: Serialize,
    {
        let ttl = ttl.unwrap_or_else(|| Duration::from_secs(self.config.default_ttl_secs));
        self.client.set_json(key, value, ttl).await
    }

    async fn get_envelope<T>(&self, key: &str) -> anyhow::Result<Option<CachedEnvelope<T>>>
    where
        T: DeserializeOwned,
    {
        self.client.get_json(key).await
    }
}

fn jittered_ttl(seed: &str, ttl: Duration, jitter_ratio: f64) -> Duration {
    if ttl.is_zero() || jitter_ratio <= 0.0 {
        return ttl;
    }

    let micros = ttl.as_micros();
    let spread = ((micros as f64) * jitter_ratio).round() as u128;
    if spread == 0 {
        return ttl;
    }

    let mut hasher = DefaultHasher::new();
    seed.hash(&mut hasher);
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .hash(&mut hasher);
    let hash = hasher.finish() as u128;
    let range = spread.saturating_mul(2).saturating_add(1);
    let delta = (hash % range) as i128 - spread as i128;
    let adjusted = (micros as i128 + delta).max(1) as u128;
    Duration::from_micros(adjusted.min(u64::MAX as u128) as u64)
}

fn default_negative_ttl(ttl: Duration) -> Duration {
    let ttl_secs = ttl.as_secs();
    let fallback = Duration::from_secs(ttl_secs.saturating_div(6).clamp(5, 60));
    if fallback.is_zero() {
        Duration::from_secs(5)
    } else {
        fallback
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
