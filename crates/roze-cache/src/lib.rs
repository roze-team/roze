use std::{
    collections::hash_map::DefaultHasher,
    collections::BTreeSet,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheFreshness {
    Fresh,
    Loaded,
    Stale,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheRead<T> {
    pub value: Option<T>,
    pub freshness: CacheFreshness,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CacheConsistencyPolicy {
    pub fresh_ttl: Duration,
    pub stale_ttl: Duration,
    pub negative_ttl: Duration,
    pub stale_on_error: bool,
}

impl Default for CacheConsistencyPolicy {
    fn default() -> Self {
        Self {
            fresh_ttl: Duration::from_secs(300),
            stale_ttl: Duration::from_secs(30),
            negative_ttl: Duration::from_secs(30),
            stale_on_error: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ConsistencyEnvelope<T> {
    value: Option<T>,
    fresh_until_millis: u64,
}

impl<T> ConsistencyEnvelope<T> {
    fn is_fresh(&self, now_millis: u64) -> bool {
        now_millis < self.fresh_until_millis
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InvalidationPlan {
    keys: BTreeSet<String>,
}

impl InvalidationPlan {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn key(mut self, key: impl Into<String>) -> Self {
        self.keys.insert(key.into());
        self
    }

    pub fn model(mut self, prefix: &str, field: &str, value: impl std::fmt::Display) -> Self {
        self.keys.insert(model_cache_key(prefix, field, value));
        self
    }

    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.keys.iter().map(String::as_str)
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }
}

pub fn model_cache_key(prefix: &str, field: &str, value: impl std::fmt::Display) -> String {
    format!(
        "model:v1:{}:{}:{}",
        escape_cache_segment(prefix),
        escape_cache_segment(field),
        escape_cache_segment(&value.to_string())
    )
}

fn escape_cache_segment(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.') {
            escaped.push(char::from(byte));
        } else {
            use std::fmt::Write;
            let _ = write!(escaped, "%{byte:02X}");
        }
    }
    escaped
}

impl RedisCache {
    pub async fn connect(config: &CacheConfig) -> anyhow::Result<Self> {
        let client = RedisClient::open_topology(config.url.as_str(), &config.cluster_urls)?
            .with_namespace(config.namespace.clone());
        Ok(Self {
            client,
            config: config.clone(),
            flights: SingleFlightGroup::new(),
        })
    }

    pub fn key(&self, key: &str) -> String {
        self.client.key(key)
    }

    pub async fn health_check(&self) -> anyhow::Result<()> {
        self.client.health_check().await
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

    pub async fn invalidate(&self, plan: &InvalidationPlan) -> anyhow::Result<usize> {
        let mut invalidated = 0;
        for key in plan.keys() {
            self.del(key).await?;
            invalidated += 1;
        }
        Ok(invalidated)
    }

    pub async fn get_or_load_consistent_option<T, F, Fut>(
        &self,
        key: &str,
        policy: CacheConsistencyPolicy,
        loader: F,
    ) -> anyhow::Result<CacheRead<T>>
    where
        T: Clone + Serialize + DeserializeOwned + Send + Sync + 'static,
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = anyhow::Result<Option<T>>>,
    {
        let now_millis = current_millis();
        let cached = self.client.get_json::<ConsistencyEnvelope<T>>(key).await?;
        if let Some(envelope) = cached.as_ref() {
            if envelope.is_fresh(now_millis) {
                return Ok(CacheRead {
                    value: envelope.value.clone(),
                    freshness: CacheFreshness::Fresh,
                });
            }
        }

        let cache_key = namespace_key(&self.config.namespace, key);
        let result = self
            .flights
            .do_call(cache_key, || async {
                match loader().await {
                    Ok(value) => {
                        let fresh_ttl = if value.is_some() {
                            policy.fresh_ttl
                        } else {
                            policy.negative_ttl
                        };
                        let envelope = ConsistencyEnvelope {
                            value: value.clone(),
                            fresh_until_millis: now_millis
                                .saturating_add(fresh_ttl.as_millis() as u64),
                        };
                        let hard_ttl = fresh_ttl.saturating_add(policy.stale_ttl);
                        self.client
                            .set_json(key, &envelope, hard_ttl)
                            .await
                            .map_err(|error| error.to_string())?;
                        Ok(CacheRead {
                            value,
                            freshness: CacheFreshness::Loaded,
                        })
                    }
                    Err(_) if policy.stale_on_error && cached.is_some() => Ok(CacheRead {
                        value: cached.expect("cached value checked").value,
                        freshness: CacheFreshness::Stale,
                    }),
                    Err(error) => Err(error.to_string()),
                }
            })
            .await;
        self.flights
            .reset(namespace_key(&self.config.namespace, key))
            .await;
        result.map_err(anyhow::Error::msg)
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
            .await;
        self.flights.reset(&cache_key).await;
        let result = result.map_err(anyhow::Error::msg)?;

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

fn current_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn namespaces_keys() {
        let config = CacheConfig {
            url: "redis://127.0.0.1/".to_string(),
            cluster_urls: Vec::new(),
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

    #[test]
    fn model_keys_are_versioned_and_escape_segments() {
        assert_eq!(
            model_cache_key("account", "email", "a:b@example.com"),
            "model:v1:account:email:a%3Ab%40example.com"
        );
        let plan = InvalidationPlan::new()
            .model("account", "id", 1)
            .model("account", "id", 1)
            .key("custom:v1:all");
        assert_eq!(plan.keys().count(), 2);
    }

    #[test]
    fn consistency_envelope_distinguishes_fresh_and_stale() {
        let envelope = ConsistencyEnvelope {
            value: Some(1),
            fresh_until_millis: 100,
        };
        assert!(envelope.is_fresh(99));
        assert!(!envelope.is_fresh(100));
    }
}
