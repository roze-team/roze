use std::{
    future::Future,
    hash::Hash,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use moka::future::Cache;

const DEFAULT_MAX_CAPACITY: u64 = 65_536;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalCacheStats {
    pub hits: u64,
    pub misses: u64,
    pub inserts: u64,
    pub removals: u64,
}

impl LocalCacheStats {
    pub fn requests(self) -> u64 {
        self.hits + self.misses
    }

    pub fn hit_ratio(self) -> f64 {
        let requests = self.requests();
        if requests == 0 {
            0.0
        } else {
            self.hits as f64 / requests as f64
        }
    }
}

#[derive(Debug, Default)]
struct LocalCacheCounters {
    hits: AtomicU64,
    misses: AtomicU64,
    inserts: AtomicU64,
    removals: AtomicU64,
}

impl LocalCacheCounters {
    fn snapshot(&self) -> LocalCacheStats {
        LocalCacheStats {
            hits: self.hits.load(Ordering::Relaxed),
            misses: self.misses.load(Ordering::Relaxed),
            inserts: self.inserts.load(Ordering::Relaxed),
            removals: self.removals.load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug, Clone)]
pub struct LocalCacheBuilder {
    max_capacity: u64,
    time_to_live: Option<Duration>,
    time_to_idle: Option<Duration>,
}

impl LocalCacheBuilder {
    pub fn new() -> Self {
        Self {
            max_capacity: DEFAULT_MAX_CAPACITY,
            time_to_live: None,
            time_to_idle: None,
        }
    }

    pub fn max_capacity(mut self, max_capacity: u64) -> Self {
        self.max_capacity = max_capacity.max(1);
        self
    }

    pub fn time_to_live(mut self, ttl: Duration) -> Self {
        self.time_to_live = Some(ttl);
        self
    }

    pub fn time_to_idle(mut self, ttl: Duration) -> Self {
        self.time_to_idle = Some(ttl);
        self
    }

    pub fn build<K, V>(self) -> LocalCache<K, V>
    where
        K: Eq + Hash + Clone + Send + Sync + 'static,
        V: Clone + Send + Sync + 'static,
    {
        let mut builder = Cache::builder().max_capacity(self.max_capacity);
        if let Some(ttl) = self.time_to_idle {
            builder = builder.time_to_idle(ttl);
        }
        LocalCache {
            entries: builder.build(),
            default_ttl: self.time_to_live,
            counters: Arc::new(LocalCacheCounters::default()),
        }
    }
}

impl Default for LocalCacheBuilder {
    fn default() -> Self {
        Self::new()
    }
}

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

#[derive(Clone)]
pub struct LocalCache<K, V> {
    entries: Cache<K, CacheEntry<V>>,
    default_ttl: Option<Duration>,
    counters: Arc<LocalCacheCounters>,
}

impl<K, V> LocalCache<K, V>
where
    K: Eq + Hash + Clone + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
{
    pub fn new() -> Self {
        Self::builder().build()
    }

    pub fn builder() -> LocalCacheBuilder {
        LocalCacheBuilder::new()
    }

    pub fn with_ttl(ttl: Duration) -> Self {
        Self::builder().time_to_live(ttl).build()
    }

    pub fn with_capacity(max_capacity: u64) -> Self {
        Self::builder().max_capacity(max_capacity).build()
    }

    pub async fn insert(&self, key: K, value: V) -> Option<V> {
        self.insert_with_ttl(key, value, self.default_ttl).await
    }

    pub async fn insert_with_ttl(&self, key: K, value: V, ttl: Option<Duration>) -> Option<V> {
        let previous = self
            .entries
            .get(&key)
            .await
            .filter(|entry| !entry.is_expired())
            .map(|entry| entry.value);
        self.entries.insert(key, CacheEntry::new(value, ttl)).await;
        self.counters.inserts.fetch_add(1, Ordering::Relaxed);
        previous
    }

    pub async fn get(&self, key: &K) -> Option<V> {
        if let Some(entry) = self.entries.get(key).await {
            if entry.is_expired() {
                self.entries.invalidate(key).await;
            } else {
                self.counters.hits.fetch_add(1, Ordering::Relaxed);
                return Some(entry.value);
            }
        }
        self.counters.misses.fetch_add(1, Ordering::Relaxed);
        None
    }

    pub async fn contains_key(&self, key: &K) -> bool {
        self.get(key).await.is_some()
    }

    pub async fn remove(&self, key: &K) -> Option<V> {
        let previous = self
            .entries
            .get(key)
            .await
            .filter(|entry| !entry.is_expired())
            .map(|entry| entry.value);
        self.entries.invalidate(key).await;
        if previous.is_some() {
            self.counters.removals.fetch_add(1, Ordering::Relaxed);
        }
        previous
    }

    pub async fn clear(&self) {
        self.entries.invalidate_all();
        self.entries.run_pending_tasks().await;
    }

    pub async fn len(&self) -> usize {
        self.prune_expired().await;
        self.entries.entry_count() as usize
    }

    pub async fn is_empty(&self) -> bool {
        self.len().await == 0
    }

    pub fn stats(&self) -> LocalCacheStats {
        self.counters.snapshot()
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
        let expired = self
            .entries
            .iter()
            .filter_map(|entry| {
                let (key, value) = entry;
                value.is_expired().then_some(key)
            })
            .collect::<Vec<_>>();
        for key in expired {
            self.entries.invalidate(key.as_ref()).await;
        }
        self.entries.run_pending_tasks().await;
    }
}

impl<K, V> Default for LocalCache<K, V>
where
    K: Eq + Hash + Clone + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
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
        assert!(!cache.is_empty().await);
        assert_eq!(cache.get(&"key").await, Some("value"));
        tokio::time::sleep(Duration::from_millis(25)).await;
        assert_eq!(cache.get(&"key").await, None);
        assert!(cache.is_empty().await);
    }

    #[tokio::test]
    async fn records_stats() {
        let cache = LocalCache::new();
        cache.insert("key", "value").await;
        assert_eq!(cache.get(&"key").await, Some("value"));
        assert_eq!(cache.get(&"missing").await, None);

        let stats = cache.stats();
        assert_eq!(stats.hits, 1);
        assert_eq!(stats.misses, 1);
        assert_eq!(stats.inserts, 1);
        assert_eq!(stats.hit_ratio(), 0.5);
    }

    #[tokio::test]
    async fn evicts_by_capacity() {
        let cache = LocalCache::with_capacity(1);
        cache.insert("a", "one").await;
        cache.insert("b", "two").await;
        cache.entries.run_pending_tasks().await;
        assert!(cache.len().await <= 1);
    }
}
