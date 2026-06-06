use std::{
    collections::HashMap,
    future::Future,
    hash::Hash,
    sync::Arc,
    time::{Duration, Instant},
};

use tokio::sync::RwLock;

#[derive(Debug, Clone)]
struct CacheEntry<V> {
    value: V,
    expires_at: Option<Instant>,
}

impl<V> CacheEntry<V> {
    fn new(value: V, ttl: Option<Duration>) -> Self {
        Self {
            value,
            expires_at: ttl.map(|ttl| Instant::now() + ttl),
        }
    }

    fn is_expired(&self) -> bool {
        self.expires_at
            .map(|expires_at| Instant::now() >= expires_at)
            .unwrap_or(false)
    }
}

#[derive(Debug, Clone)]
pub struct LocalCache<K, V> {
    entries: Arc<RwLock<HashMap<K, CacheEntry<V>>>>,
    default_ttl: Option<Duration>,
}

impl<K, V> LocalCache<K, V>
where
    K: Eq + Hash + Clone,
    V: Clone,
{
    pub fn new() -> Self {
        Self {
            entries: Arc::new(RwLock::new(HashMap::new())),
            default_ttl: None,
        }
    }

    pub fn with_ttl(ttl: Duration) -> Self {
        Self {
            entries: Arc::new(RwLock::new(HashMap::new())),
            default_ttl: Some(ttl),
        }
    }

    pub async fn insert(&self, key: K, value: V) -> Option<V> {
        self.insert_with_ttl(key, value, self.default_ttl).await
    }

    pub async fn insert_with_ttl(
        &self,
        key: K,
        value: V,
        ttl: Option<Duration>,
    ) -> Option<V> {
        let mut entries = self.entries.write().await;
        entries.insert(key, CacheEntry::new(value, ttl)).map(|entry| entry.value)
    }

    pub async fn get(&self, key: &K) -> Option<V> {
        let mut entries = self.entries.write().await;
        let should_remove = entries.get(key).map(|entry| entry.is_expired()).unwrap_or(false);
        if should_remove {
            entries.remove(key);
            return None;
        }
        entries.get(key).map(|entry| entry.value.clone())
    }

    pub async fn contains_key(&self, key: &K) -> bool {
        self.get(key).await.is_some()
    }

    pub async fn remove(&self, key: &K) -> Option<V> {
        self.entries.write().await.remove(key).map(|entry| entry.value)
    }

    pub async fn clear(&self) {
        self.entries.write().await.clear();
    }

    pub async fn len(&self) -> usize {
        self.prune_expired().await;
        self.entries.read().await.len()
    }

    pub async fn get_or_insert_with<F, Fut>(&self, key: K, factory: F) -> V
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = V>,
    {
        if let Some(value) = self.get(&key).await {
            return value;
        }

        let value = factory().await;
        let _ = self.insert(key, value.clone()).await;
        value
    }

    async fn prune_expired(&self) {
        let mut entries = self.entries.write().await;
        entries.retain(|_, entry| !entry.is_expired());
    }
}

impl<K, V> Default for LocalCache<K, V>
where
    K: Eq + Hash + Clone,
    V: Clone,
{
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn respects_ttl() {
        let cache = LocalCache::with_ttl(Duration::from_millis(15));
        cache.insert("key", "value").await;
        assert_eq!(cache.get(&"key").await.as_deref(), Some("value"));
        tokio::time::sleep(Duration::from_millis(25)).await;
        assert_eq!(cache.get(&"key").await, None);
    }
}
