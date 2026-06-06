use std::{
    collections::HashMap,
    net::ToSocketAddrs,
    sync::{Arc, Mutex, RwLock},
    time::{Duration, Instant},
};

use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde::Deserialize;
use tokio::time::interval;

use crate::balance::{self, Balancer};
use roze_config::{RegistryConfig, RegistryKind, ServiceConfig};

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, serde::Serialize)]
pub struct ServiceInstance {
    pub name: String,
    pub addr: String,
    pub weight: u32,
    pub metadata: HashMap<String, String>,
}

impl ServiceInstance {
    pub fn new(name: impl Into<String>, addr: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            addr: addr.into(),
            weight: 1,
            metadata: HashMap::new(),
        }
    }
}

#[async_trait]
pub trait Registry: Send + Sync + 'static {
    async fn register(&self, instance: ServiceInstance) -> anyhow::Result<()>;
    async fn deregister(&self, name: &str, addr: &str) -> anyhow::Result<()>;
    async fn discover(&self, name: &str) -> anyhow::Result<Vec<ServiceInstance>>;
}

#[derive(Debug, Default, Clone)]
pub struct MemoryRegistry {
    instances: Arc<RwLock<HashMap<String, Vec<ServiceInstance>>>>,
}

#[async_trait]
impl Registry for MemoryRegistry {
    async fn register(&self, instance: ServiceInstance) -> anyhow::Result<()> {
        let mut instances = self.instances.write().expect("registry lock poisoned");
        let entry = instances.entry(instance.name.clone()).or_default();
        entry.retain(|existing| existing.addr != instance.addr);
        entry.push(instance);
        Ok(())
    }

    async fn deregister(&self, name: &str, addr: &str) -> anyhow::Result<()> {
        let mut instances = self.instances.write().expect("registry lock poisoned");
        if let Some(service_instances) = instances.get_mut(name) {
            service_instances.retain(|instance| instance.addr != addr);
        }
        Ok(())
    }

    async fn discover(&self, name: &str) -> anyhow::Result<Vec<ServiceInstance>> {
        let instances = self.instances.read().expect("registry lock poisoned");
        Ok(instances.get(name).cloned().unwrap_or_default())
    }
}

#[derive(Debug, Clone)]
pub struct DnsRegistry {
    endpoints: Vec<String>,
}

impl DnsRegistry {
    pub fn new(endpoints: Vec<String>) -> Self {
        Self { endpoints }
    }
}

#[async_trait]
impl Registry for DnsRegistry {
    async fn register(&self, _instance: ServiceInstance) -> anyhow::Result<()> {
        anyhow::bail!("dns registry does not support register");
    }

    async fn deregister(&self, _name: &str, _addr: &str) -> anyhow::Result<()> {
        anyhow::bail!("dns registry does not support deregister");
    }

    async fn discover(&self, name: &str) -> anyhow::Result<Vec<ServiceInstance>> {
        let mut instances = Vec::new();
        for endpoint in &self.endpoints {
            for addr in endpoint.to_socket_addrs()? {
                instances.push(ServiceInstance::new(name, addr.to_string()));
            }
        }
        Ok(instances)
    }
}

#[derive(Debug, Clone)]
pub struct EtcdRegistry {
    endpoints: Vec<String>,
    ttl_seconds: u64,
    renew_interval_secs: u64,
    client: reqwest::Client,
    leases: Arc<Mutex<HashMap<String, tokio::task::JoinHandle<()>>>>,
}

impl EtcdRegistry {
    pub fn new(config: &RegistryConfig) -> Self {
        Self {
            endpoints: config.endpoints.clone(),
            ttl_seconds: config.ttl_seconds.max(1),
            renew_interval_secs: config.renew_interval_secs.max(1),
            client: reqwest::Client::new(),
            leases: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn endpoints(&self) -> &[String] {
        if self.endpoints.is_empty() {
            &[]
        } else {
            &self.endpoints
        }
    }

    async fn grant_lease(&self, endpoint: &str) -> anyhow::Result<String> {
        let body = serde_json::json!({
            "TTL": self.ttl_seconds,
        });
        let resp = self
            .client
            .post(format!("{}/v3/lease/grant", endpoint))
            .json(&body)
            .send()
            .await?
            .error_for_status()?;
        let value: serde_json::Value = resp.json().await?;
        extract_json_field(&value, "ID")
            .ok_or_else(|| anyhow::anyhow!("missing etcd lease id in response"))
    }
}

#[derive(Debug, Clone)]
pub struct ConsulRegistry {
    endpoints: Vec<String>,
    ttl_seconds: u64,
    renew_interval_secs: u64,
    client: reqwest::Client,
    leases: Arc<Mutex<HashMap<String, tokio::task::JoinHandle<()>>>>,
}

impl ConsulRegistry {
    pub fn new(config: &RegistryConfig) -> Self {
        Self {
            endpoints: config.endpoints.clone(),
            ttl_seconds: config.ttl_seconds.max(1),
            renew_interval_secs: config.renew_interval_secs.max(1),
            client: reqwest::Client::new(),
            leases: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn endpoints(&self) -> &[String] {
        if self.endpoints.is_empty() {
            &[]
        } else {
            &self.endpoints
        }
    }

    fn check_id(&self, service_id: &str) -> String {
        consul_check_id(service_id)
    }
}

#[async_trait]
impl Registry for EtcdRegistry {
    async fn register(&self, instance: ServiceInstance) -> anyhow::Result<()> {
        let payload = serde_json::to_vec(&instance)?;
        let key = etcd_instance_key(&instance.name, &instance.addr);
        let endpoint = first_registry_endpoint(self.endpoints(), "http://127.0.0.1:2379");
        let lease_id = self.grant_lease(endpoint).await?;
        let body = serde_json::json!({
            "key": STANDARD.encode(key.as_bytes()),
            "value": STANDARD.encode(payload),
            "lease": lease_id,
        });

        self.client
            .post(format!("{}/v3/kv/put", endpoint))
            .json(&body)
            .send()
            .await?
            .error_for_status()?;

        let lease = lease_id.clone();
        let client = self.client.clone();
        let endpoint = endpoint.to_string();
        let renew_interval = self.renew_interval_secs;
        let renew_handle = tokio::spawn(async move {
            let mut ticker = interval(Duration::from_secs(renew_interval));
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                ticker.tick().await;
                let body = serde_json::json!({ "ID": lease });
                let result = client
                    .post(format!("{}/v3/lease/keepalive", endpoint))
                    .json(&body)
                    .send()
                    .await;
                let Ok(resp) = result else {
                    break;
                };
                if resp.error_for_status().is_err() {
                    break;
                }
            }
        });

        let mut leases = self.leases.lock().expect("lease lock poisoned");
        if let Some(previous) = leases.insert(key, renew_handle) {
            previous.abort();
        }

        Ok(())
    }

    async fn deregister(&self, name: &str, addr: &str) -> anyhow::Result<()> {
        let key = etcd_instance_key(name, addr);
        if let Some(handle) = self
            .leases
            .lock()
            .expect("lease lock poisoned")
            .remove(&key)
        {
            handle.abort();
        }
        let body = serde_json::json!({
            "key": STANDARD.encode(key.as_bytes()),
        });

        for endpoint in registry_endpoints(self.endpoints(), "http://127.0.0.1:2379") {
            self.client
                .post(format!("{}/v3/kv/deleterange", endpoint))
                .json(&body)
                .send()
                .await?
                .error_for_status()?;
        }

        Ok(())
    }

    async fn discover(&self, name: &str) -> anyhow::Result<Vec<ServiceInstance>> {
        let key_prefix = etcd_service_prefix(name);
        let range_end = etcd_prefix_end(key_prefix.as_bytes());
        let body = serde_json::json!({
            "key": STANDARD.encode(key_prefix.as_bytes()),
            "range_end": STANDARD.encode(range_end),
        });

        for endpoint in registry_endpoints(self.endpoints(), "http://127.0.0.1:2379") {
            let resp = self
                .client
                .post(format!("{}/v3/kv/range", endpoint))
                .json(&body)
                .send()
                .await;

            let resp = match resp {
                Ok(resp) => resp.error_for_status()?,
                Err(_) => continue,
            };

            let payload: EtcdRangeResponse = resp.json().await?;
            let mut instances = Vec::new();
            if let Some(kvs) = payload.kvs {
                for kv in kvs {
                    if let Some(value) = kv.value {
                        let decoded = STANDARD.decode(value)?;
                        let mut instance: ServiceInstance = serde_json::from_slice(&decoded)?;
                        instance.name = name.to_string();
                        instances.push(instance);
                    }
                }
            }
            return Ok(instances);
        }

        Ok(Vec::new())
    }
}

#[async_trait]
impl Registry for ConsulRegistry {
    async fn register(&self, instance: ServiceInstance) -> anyhow::Result<()> {
        let (address, port) = split_addr(&instance.addr)?;
        let service_id = consul_instance_id(&instance.name, &instance.addr);
        let endpoint = first_registry_endpoint(self.endpoints(), "http://127.0.0.1:8500");
        let body = serde_json::json!({
            "ID": service_id,
            "Name": instance.name,
            "Address": address,
            "Port": port,
            "Meta": instance.metadata,
            "Tags": [format!("weight={}", instance.weight.max(1))],
            "Check": {
                "TTL": format!("{}s", self.ttl_seconds),
                "DeregisterCriticalServiceAfter": format!("{}s", self.ttl_seconds.saturating_mul(3)),
            },
        });

        self.client
            .put(format!("{}/v1/agent/service/register", endpoint))
            .json(&body)
            .send()
            .await?
            .error_for_status()?;

        let client = self.client.clone();
        let endpoint = endpoint.to_string();
        let renew_interval = self.renew_interval_secs;
        let ttl_seconds = self.ttl_seconds;
        let check_id = self.check_id(&service_id);
        let renew_handle = tokio::spawn(async move {
            let mut ticker = interval(Duration::from_secs(renew_interval));
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                ticker.tick().await;
                let result = client
                    .put(format!("{}/v1/agent/check/pass/{}", endpoint, check_id))
                    .query(&[("note", format!("ttl={}s", ttl_seconds))])
                    .send()
                    .await;
                let Ok(resp) = result else {
                    break;
                };
                if resp.error_for_status().is_err() {
                    break;
                }
            }
        });

        let mut leases = self.leases.lock().expect("lease lock poisoned");
        if let Some(previous) = leases.insert(service_id, renew_handle) {
            previous.abort();
        }

        Ok(())
    }

    async fn deregister(&self, name: &str, addr: &str) -> anyhow::Result<()> {
        let id = consul_instance_id(name, addr);
        if let Some(handle) = self.leases.lock().expect("lease lock poisoned").remove(&id) {
            handle.abort();
        }
        let endpoint = first_registry_endpoint(self.endpoints(), "http://127.0.0.1:8500");
        self.client
            .put(format!("{}/v1/agent/service/deregister/{}", endpoint, id))
            .send()
            .await?
            .error_for_status()?;

        Ok(())
    }

    async fn discover(&self, name: &str) -> anyhow::Result<Vec<ServiceInstance>> {
        for endpoint in registry_endpoints(self.endpoints(), "http://127.0.0.1:8500") {
            let resp = self
                .client
                .get(format!(
                    "{}/v1/health/service/{}?passing=true",
                    endpoint, name
                ))
                .send()
                .await;

            let resp = match resp {
                Ok(resp) => resp.error_for_status()?,
                Err(_) => continue,
            };

            let services: Vec<ConsulHealthService> = resp.json().await?;
            let mut instances = Vec::new();
            for service in services {
                if let Some(instance) = consul_to_instance(name, service) {
                    instances.push(instance);
                }
            }
            return Ok(instances);
        }

        Ok(Vec::new())
    }
}

#[derive(Debug, Deserialize)]
struct EtcdRangeResponse {
    #[serde(default)]
    kvs: Option<Vec<EtcdKv>>,
}

#[derive(Debug, Deserialize)]
struct EtcdKv {
    #[serde(default)]
    value: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ConsulHealthService {
    #[serde(rename = "Service")]
    service: ConsulServiceRecord,
}

#[derive(Debug, Deserialize)]
struct ConsulServiceRecord {
    #[serde(rename = "Address")]
    address: String,
    #[serde(rename = "Port")]
    port: u16,
    #[serde(rename = "Meta")]
    meta: Option<HashMap<String, String>>,
}

fn registry_endpoints<'a>(configured: &'a [String], default: &'a str) -> Vec<&'a str> {
    if configured.is_empty() {
        vec![default]
    } else {
        configured.iter().map(String::as_str).collect()
    }
}

fn first_registry_endpoint<'a>(configured: &'a [String], default: &'a str) -> &'a str {
    configured.first().map(String::as_str).unwrap_or(default)
}

fn etcd_service_prefix(name: &str) -> String {
    format!("/roze/services/{name}/")
}

fn etcd_instance_key(name: &str, addr: &str) -> String {
    format!("{}{}", etcd_service_prefix(name), addr)
}

fn etcd_prefix_end(prefix: &[u8]) -> Vec<u8> {
    let mut end = prefix.to_vec();
    for idx in (0..end.len()).rev() {
        if end[idx] != 0xff {
            end[idx] += 1;
            end.truncate(idx + 1);
            return end;
        }
    }
    Vec::new()
}

fn consul_instance_id(name: &str, addr: &str) -> String {
    format!("{}-{}", name, addr.replace(':', "-"))
}

fn consul_check_id(service_id: &str) -> String {
    format!("service:{service_id}")
}

fn split_addr(addr: &str) -> anyhow::Result<(String, u16)> {
    let socket: std::net::SocketAddr = addr.parse()?;
    Ok((socket.ip().to_string(), socket.port()))
}

fn extract_json_field(value: &serde_json::Value, field: &str) -> Option<String> {
    value.get(field).and_then(|value| match value {
        serde_json::Value::String(value) => Some(value.clone()),
        serde_json::Value::Number(value) => Some(value.to_string()),
        _ => None,
    })
}

fn consul_to_instance(name: &str, service: ConsulHealthService) -> Option<ServiceInstance> {
    let mut addr = service.service.address;
    if addr.is_empty() {
        return None;
    }
    if service.service.port != 0 {
        addr = format!("{}:{}", addr, service.service.port);
    }

    let mut instance = ServiceInstance::new(name, addr);
    if let Some(meta) = service.service.meta {
        if let Some(weight) = meta.get("weight") {
            if let Ok(parsed) = weight.parse::<u32>() {
                instance.weight = parsed.max(1);
            }
        }
        instance.metadata = meta;
    }

    Some(instance)
}

pub fn build_registry(config: &RegistryConfig) -> anyhow::Result<Arc<dyn Registry>> {
    match config.kind {
        RegistryKind::Memory => Ok(Arc::new(MemoryRegistry::default())),
        RegistryKind::Dns => Ok(Arc::new(DnsRegistry::new(config.endpoints.clone()))),
        RegistryKind::Etcd => Ok(Arc::new(EtcdRegistry::new(config))),
        RegistryKind::Consul => Ok(Arc::new(ConsulRegistry::new(config))),
    }
}

pub fn build_service_registry(config: &ServiceConfig) -> anyhow::Result<Option<Arc<dyn Registry>>> {
    match config.registry.as_ref() {
        Some(registry) => build_registry(registry).map(Some),
        None => Ok(None),
    }
}

pub fn registry_from_kind(kind: RegistryKind) -> anyhow::Result<Arc<dyn Registry>> {
    build_registry(&RegistryConfig {
        kind,
        endpoints: Vec::new(),
        ttl_seconds: 10,
        renew_interval_secs: 3,
    })
}

pub struct RegistryResolver<R, B> {
    registry: R,
    balancer: B,
}

impl<R, B> RegistryResolver<R, B>
where
    R: Registry,
    B: Balancer,
{
    pub fn new(registry: R, balancer: B) -> Self {
        Self { registry, balancer }
    }

    pub async fn pick(&self, name: &str) -> anyhow::Result<Option<ServiceInstance>> {
        let instances = self.registry.discover(name).await?;
        Ok(self.balancer.pick(&instances))
    }

    pub async fn discover(&self, name: &str) -> anyhow::Result<Vec<ServiceInstance>> {
        self.registry.discover(name).await
    }
}

#[derive(Debug, Clone)]
pub struct CachedRegistryResolver<R, B> {
    registry: Arc<R>,
    balancer: Arc<B>,
    cache_ttl: Duration,
    refresh_interval: Duration,
    cache: Arc<Mutex<HashMap<String, CachedEntry>>>,
    refresh_tasks: Arc<Mutex<HashMap<String, tokio::task::JoinHandle<()>>>>,
}

#[derive(Debug, Clone)]
struct CachedEntry {
    discovered_at: Instant,
    instances: Vec<ServiceInstance>,
}

impl<R, B> CachedRegistryResolver<R, B>
where
    R: Registry,
    B: Balancer,
{
    pub fn new(registry: R, balancer: B, cache_ttl: Duration) -> Self {
        let refresh_interval = if cache_ttl.is_zero() {
            Duration::from_secs(1)
        } else {
            cache_ttl / 2
        }
        .max(Duration::from_millis(1));

        Self {
            registry: Arc::new(registry),
            balancer: Arc::new(balancer),
            cache_ttl,
            refresh_interval,
            cache: Arc::new(Mutex::new(HashMap::new())),
            refresh_tasks: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn with_refresh_interval(mut self, refresh_interval: Duration) -> Self {
        self.refresh_interval = refresh_interval.max(Duration::from_millis(1));
        self
    }

    pub async fn discover(&self, name: &str) -> anyhow::Result<Vec<ServiceInstance>> {
        if let Some(instances) = self.cached_instances(name) {
            self.ensure_refresh_task(name.to_string());
            return Ok(instances);
        }

        let instances = self.registry.discover(name).await?;
        self.store(name, instances.clone());
        self.ensure_refresh_task(name.to_string());
        Ok(instances)
    }

    pub async fn pick(&self, name: &str) -> anyhow::Result<Option<ServiceInstance>> {
        let instances = self.discover(name).await?;
        Ok(self.balancer.pick(&instances))
    }

    pub fn invalidate(&self, name: &str) {
        self.cache
            .lock()
            .expect("registry cache lock poisoned")
            .remove(name);
        if let Some(handle) = self
            .refresh_tasks
            .lock()
            .expect("registry refresh lock poisoned")
            .remove(name)
        {
            handle.abort();
        }
    }

    fn cached_instances(&self, name: &str) -> Option<Vec<ServiceInstance>> {
        let cache = self.cache.lock().expect("registry cache lock poisoned");
        let entry = cache.get(name)?;
        if entry.discovered_at.elapsed() <= self.cache_ttl {
            Some(entry.instances.clone())
        } else {
            None
        }
    }

    fn store(&self, name: &str, instances: Vec<ServiceInstance>) {
        self.cache.lock().expect("registry cache lock poisoned").insert(
            name.to_string(),
            CachedEntry {
                discovered_at: Instant::now(),
                instances,
            },
        );
    }

    fn ensure_refresh_task(&self, name: String) {
        let mut refresh_tasks = self
            .refresh_tasks
            .lock()
            .expect("registry refresh lock poisoned");
        if refresh_tasks.contains_key(&name) {
            return;
        }

        let registry = Arc::clone(&self.registry);
        let cache = Arc::clone(&self.cache);
        let refresh_interval = self.refresh_interval;
        let name_for_task = name.clone();
        let handle = tokio::spawn(async move {
            let mut ticker = interval(refresh_interval);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                ticker.tick().await;
                match registry.discover(&name_for_task).await {
                    Ok(instances) => {
                        cache
                            .lock()
                            .expect("registry cache lock poisoned")
                            .insert(
                                name_for_task.clone(),
                                CachedEntry {
                                    discovered_at: Instant::now(),
                                    instances,
                                },
                            );
                    }
                    Err(err) => {
                        tracing::warn!(service = %name_for_task, error = %err, "registry refresh failed");
                    }
                }
            }
        });

        refresh_tasks.insert(name, handle);
    }
}

pub fn weighted_instances(instances: &[ServiceInstance]) -> Vec<ServiceInstance> {
    let mut out = Vec::new();
    for instance in instances {
        let weight = instance.weight.max(1);
        for _ in 0..weight {
            out.push(instance.clone());
        }
    }
    out
}

pub fn pick_with_strategy(
    strategy: balance::BalancerKind,
    instances: &[ServiceInstance],
) -> Option<ServiceInstance> {
    balance::pick(strategy, instances)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn memory_registry_registers_and_discovers() {
        let registry = MemoryRegistry::default();
        registry
            .register(ServiceInstance::new("user", "127.0.0.1:8080"))
            .await
            .expect("register");
        registry
            .register(ServiceInstance::new("user", "127.0.0.1:8081"))
            .await
            .expect("register");

        let instances = registry.discover("user").await.expect("discover");
        assert_eq!(instances.len(), 2);

        registry
            .deregister("user", "127.0.0.1:8080")
            .await
            .expect("deregister");
        let instances = registry.discover("user").await.expect("discover");
        assert_eq!(instances.len(), 1);
        assert_eq!(instances[0].addr, "127.0.0.1:8081");
    }

    #[test]
    fn weighted_instances_expand_by_weight() {
        let mut a = ServiceInstance::new("user", "127.0.0.1:8080");
        a.weight = 2;
        let mut b = ServiceInstance::new("user", "127.0.0.1:8081");
        b.weight = 1;

        let weighted = weighted_instances(&[a.clone(), b.clone()]);
        assert_eq!(weighted.len(), 3);
        assert_eq!(weighted[0].addr, a.addr);
        assert_eq!(weighted[1].addr, a.addr);
        assert_eq!(weighted[2].addr, b.addr);
    }

    #[test]
    fn pick_with_strategy_supports_round_robin() {
        use crate::balance::{Balancer, RoundRobinBalancer};

        let mut a = ServiceInstance::new("user", "127.0.0.1:8080");
        a.weight = 1;
        let mut b = ServiceInstance::new("user", "127.0.0.1:8081");
        b.weight = 1;
        let balancer = RoundRobinBalancer::default();

        let first = balancer.pick(&[a.clone(), b.clone()]).expect("pick");
        let second = balancer.pick(&[a.clone(), b.clone()]).expect("pick");

        assert_ne!(first.addr, second.addr);
    }

    #[test]
    fn registry_from_kind_builds_memory_registry() {
        let registry = registry_from_kind(RegistryKind::Memory).expect("registry");
        let _ = registry;
    }

    #[test]
    fn service_resolver_picks_instance() {
        let registry = MemoryRegistry::default();
        let balancer = crate::balance::FirstAvailableBalancer;
        let resolver = RegistryResolver::new(registry, balancer);
        let _ = resolver;
    }

    #[tokio::test]
    async fn cached_resolver_reuses_discovery_until_expired() {
        let registry = MemoryRegistry::default();
        registry
            .register(ServiceInstance::new("user", "127.0.0.1:8080"))
            .await
            .expect("register");
        let resolver = CachedRegistryResolver::new(
            registry,
            crate::balance::FirstAvailableBalancer,
            Duration::from_secs(60),
        );

        let first = resolver.discover("user").await.expect("discover");
        assert_eq!(first.len(), 1);
        let second = resolver.discover("user").await.expect("discover");
        assert_eq!(second.len(), 1);
        assert_eq!(first[0].addr, second[0].addr);
    }

    #[tokio::test]
    async fn cached_resolver_refreshes_in_background() {
        #[derive(Debug, Clone, Default)]
        struct CountingRegistry {
            instances: Arc<RwLock<HashMap<String, Vec<ServiceInstance>>>>,
            discoveries: Arc<std::sync::atomic::AtomicUsize>,
        }

        #[async_trait]
        impl Registry for CountingRegistry {
            async fn register(&self, instance: ServiceInstance) -> anyhow::Result<()> {
                let mut instances = self.instances.write().expect("registry lock poisoned");
                instances
                    .entry(instance.name.clone())
                    .or_default()
                    .push(instance);
                Ok(())
            }

            async fn deregister(&self, name: &str, addr: &str) -> anyhow::Result<()> {
                let mut instances = self.instances.write().expect("registry lock poisoned");
                if let Some(service_instances) = instances.get_mut(name) {
                    service_instances.retain(|instance| instance.addr != addr);
                }
                Ok(())
            }

            async fn discover(&self, name: &str) -> anyhow::Result<Vec<ServiceInstance>> {
                self.discoveries
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let instances = self.instances.read().expect("registry lock poisoned");
                Ok(instances.get(name).cloned().unwrap_or_default())
            }
        }

        let registry = CountingRegistry::default();
        registry
            .register(ServiceInstance::new("user", "127.0.0.1:8080"))
            .await
            .expect("register");
        let resolver = CachedRegistryResolver::new(
            registry.clone(),
            crate::balance::FirstAvailableBalancer,
            Duration::from_millis(20),
        )
        .with_refresh_interval(Duration::from_millis(5));

        let initial = resolver.discover("user").await.expect("discover");
        assert_eq!(initial.len(), 1);

        tokio::time::sleep(Duration::from_millis(30)).await;
        let discoveries = registry.discoveries.load(std::sync::atomic::Ordering::SeqCst);
        assert!(discoveries >= 2, "expected background refresh to run, got {discoveries}");

        resolver.invalidate("user");
    }
}
