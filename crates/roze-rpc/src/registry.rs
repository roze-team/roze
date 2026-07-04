use std::{
    collections::HashMap,
    future::Future,
    net::ToSocketAddrs,
    pin::Pin,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use dashmap::DashMap;
use futures_util::StreamExt;
use serde::Deserialize;
use tokio::{sync::mpsc, time::interval};

use crate::balance::{self, Balancer};
use roze_config::{RegistryConfig, RegistryKind, RpcClientEtcdConfig, ServiceConfig};

pub type RegistryWatchFuture<'a> = Pin<
    Box<
        dyn Future<Output = anyhow::Result<mpsc::UnboundedReceiver<Vec<ServiceInstance>>>>
            + Send
            + 'a,
    >,
>;

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

    fn supports_watch(&self) -> bool {
        false
    }

    fn watch(&self, _name: &str) -> RegistryWatchFuture<'_> {
        Box::pin(async { anyhow::bail!("registry does not support watch") })
    }
}

#[derive(Debug, Default, Clone)]
pub struct MemoryRegistry {
    instances: Arc<DashMap<String, Vec<ServiceInstance>>>,
}

#[async_trait]
impl Registry for MemoryRegistry {
    async fn register(&self, instance: ServiceInstance) -> anyhow::Result<()> {
        let mut entry = self.instances.entry(instance.name.clone()).or_default();
        entry.retain(|existing| existing.addr != instance.addr);
        entry.push(instance);
        Ok(())
    }

    async fn deregister(&self, name: &str, addr: &str) -> anyhow::Result<()> {
        let should_remove = if let Some(mut service_instances) = self.instances.get_mut(name) {
            service_instances.retain(|instance| instance.addr != addr);
            service_instances.is_empty()
        } else {
            false
        };
        if should_remove {
            self.instances.remove(name);
        }
        Ok(())
    }

    async fn discover(&self, name: &str) -> anyhow::Result<Vec<ServiceInstance>> {
        Ok(self
            .instances
            .get(name)
            .map(|instances| instances.clone())
            .unwrap_or_default())
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
    prefix: String,
    ttl_seconds: u64,
    renew_interval_secs: u64,
    client: reqwest::Client,
    leases: Arc<Mutex<HashMap<String, tokio::task::JoinHandle<()>>>>,
}

impl EtcdRegistry {
    pub fn new(config: &RegistryConfig) -> Self {
        Self {
            endpoints: config.endpoints.clone(),
            prefix: normalize_etcd_registry_prefix(&config.prefix),
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
            .await
            .map_err(|err| with_proxy_diagnostic(err.into(), endpoint))?
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
        let key = etcd_instance_key(&self.prefix, &instance.name, &instance.addr);
        let endpoint = first_registry_endpoint(self.endpoints(), "http://127.0.0.1:2379");
        let lease_id = self.grant_lease(&endpoint).await?;
        let body = serde_json::json!({
            "key": STANDARD.encode(key.as_bytes()),
            "value": STANDARD.encode(payload),
            "lease": lease_id,
        });

        self.client
            .post(format!("{}/v3/kv/put", endpoint))
            .json(&body)
            .send()
            .await
            .map_err(|err| with_proxy_diagnostic(err.into(), &endpoint))?
            .error_for_status()?;

        let lease = lease_id.clone();
        let client = self.client.clone();
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
        let key = etcd_instance_key(&self.prefix, name, addr);
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
                .await
                .map_err(|err| with_proxy_diagnostic(err.into(), &endpoint))?
                .error_for_status()?;
        }

        Ok(())
    }

    async fn discover(&self, name: &str) -> anyhow::Result<Vec<ServiceInstance>> {
        let key_prefix = etcd_service_prefix(&self.prefix, name);
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
                Err(err) => {
                    tracing::warn!(
                        service = %name,
                        endpoint = %endpoint,
                        error = %with_proxy_diagnostic(err.into(), &endpoint),
                        "etcd registry discover request failed"
                    );
                    continue;
                }
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

    fn supports_watch(&self) -> bool {
        true
    }

    fn watch(&self, name: &str) -> RegistryWatchFuture<'_> {
        let registry = self.clone();
        let name = name.to_string();
        Box::pin(async move {
            let (tx, rx) = mpsc::unbounded_channel();
            tokio::spawn(async move {
                if let Ok(instances) = registry.discover(&name).await {
                    let _ = tx.send(instances);
                }
                loop {
                    let mut connected = false;
                    for endpoint in
                        registry_endpoints(registry.endpoints(), "http://127.0.0.1:2379")
                    {
                        match stream_etcd_registry_watch(&registry, &endpoint, &name, tx.clone())
                            .await
                        {
                            Ok(()) => {
                                connected = true;
                                break;
                            }
                            Err(err) => {
                                tracing::warn!(
                                    service = %name,
                                    endpoint = %endpoint,
                                    error = %err,
                                    "etcd registry watch failed"
                                );
                            }
                        }
                    }
                    if !connected {
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    }
                }
            });
            Ok(rx)
        })
    }
}

#[async_trait]
impl Registry for ConsulRegistry {
    async fn register(&self, instance: ServiceInstance) -> anyhow::Result<()> {
        let (address, port) = split_addr(&instance.addr)?;
        let service_id = consul_instance_id(&instance.name, &instance.addr);
        let endpoint = first_registry_endpoint(self.endpoints(), "http://127.0.0.1:8500");
        let weight = instance.weight.max(1);
        let mut metadata = instance.metadata;
        metadata.insert("weight".to_string(), weight.to_string());
        let body = serde_json::json!({
            "ID": service_id,
            "Name": instance.name,
            "Address": address,
            "Port": port,
            "Meta": metadata,
            "Tags": [format!("weight={weight}")],
            "Check": {
                "TTL": format!("{}s", self.ttl_seconds),
                "DeregisterCriticalServiceAfter": format!("{}s", self.ttl_seconds.saturating_mul(3)),
            },
        });

        self.client
            .put(format!("{}/v1/agent/service/register", endpoint))
            .json(&body)
            .send()
            .await
            .map_err(|err| with_proxy_diagnostic(err.into(), &endpoint))?
            .error_for_status()?;

        let client = self.client.clone();
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
            .await
            .map_err(|err| with_proxy_diagnostic(err.into(), &endpoint))?
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
                Err(err) => {
                    tracing::warn!(
                        service = %name,
                        endpoint = %endpoint,
                        error = %with_proxy_diagnostic(err.into(), &endpoint),
                        "consul registry discover request failed"
                    );
                    continue;
                }
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

fn registry_endpoints(configured: &[String], default: &str) -> Vec<String> {
    if configured.is_empty() {
        vec![default.to_string()]
    } else {
        configured
            .iter()
            .map(|endpoint| normalize_registry_endpoint(endpoint))
            .collect()
    }
}

fn first_registry_endpoint(configured: &[String], default: &str) -> String {
    configured
        .first()
        .map(|endpoint| normalize_registry_endpoint(endpoint))
        .unwrap_or_else(|| default.to_string())
}

fn normalize_registry_endpoint(endpoint: &str) -> String {
    let endpoint = endpoint.trim().trim_end_matches('/');
    if endpoint.starts_with("http://") || endpoint.starts_with("https://") {
        endpoint.to_string()
    } else {
        format!("http://{endpoint}")
    }
}

fn normalize_etcd_registry_prefix(prefix: &str) -> String {
    let prefix = prefix.trim().trim_matches('/');
    if prefix.is_empty() {
        "/roze/services".to_string()
    } else {
        format!("/{prefix}")
    }
}

fn etcd_service_prefix(prefix: &str, name: &str) -> String {
    format!("{}/{name}/", normalize_etcd_registry_prefix(prefix))
}

fn etcd_instance_key(prefix: &str, name: &str, addr: &str) -> String {
    format!("{}{}", etcd_service_prefix(prefix, name), addr)
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

async fn stream_etcd_registry_watch(
    registry: &EtcdRegistry,
    endpoint: &str,
    name: &str,
    tx: mpsc::UnboundedSender<Vec<ServiceInstance>>,
) -> anyhow::Result<()> {
    let key_prefix = etcd_service_prefix(&registry.prefix, name);
    let range_end = etcd_prefix_end(key_prefix.as_bytes());
    let body = serde_json::json!({
        "create_request": {
            "key": STANDARD.encode(key_prefix.as_bytes()),
            "range_end": STANDARD.encode(range_end),
        }
    });
    let response = registry
        .client
        .post(format!("{}/v3/watch", endpoint))
        .json(&body)
        .send()
        .await
        .map_err(|err| with_proxy_diagnostic(err.into(), endpoint))?
        .error_for_status()?;
    let mut stream = response.bytes_stream();
    let mut buffer = String::new();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        buffer.push_str(std::str::from_utf8(&chunk)?);
        if !etcd_registry_watch_has_event(&buffer) {
            continue;
        }

        let instances = registry.discover(name).await?;
        let _ = tx.send(instances);
        buffer.clear();
    }

    anyhow::bail!("etcd registry watch stream ended")
}

fn etcd_registry_watch_has_event(raw: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(raw)
        .ok()
        .and_then(|value| value.get("result").cloned())
        .and_then(|result| result.get("events").cloned())
        .and_then(|events| events.as_array().cloned())
        .is_some_and(|events| !events.is_empty())
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

fn with_proxy_diagnostic(err: anyhow::Error, endpoint: &str) -> anyhow::Error {
    if let Some(hint) = roze_config::http_proxy_environment_diagnostic(endpoint) {
        anyhow::anyhow!("{err}; {hint}")
    } else {
        err
    }
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
        prefix: "/roze/services".to_string(),
        ttl_seconds: 10,
        renew_interval_secs: 3,
    })
}

pub fn registry_config_from_rpc_client_etcd(config: &RpcClientEtcdConfig) -> RegistryConfig {
    RegistryConfig {
        kind: RegistryKind::Etcd,
        endpoints: config.hosts.clone(),
        prefix: "/roze/services".to_string(),
        ttl_seconds: 10,
        renew_interval_secs: 3,
    }
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
    watch_tasks: Arc<Mutex<HashMap<String, tokio::task::JoinHandle<()>>>>,
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
            watch_tasks: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn with_refresh_interval(mut self, refresh_interval: Duration) -> Self {
        self.refresh_interval = refresh_interval.max(Duration::from_millis(1));
        self
    }

    pub async fn discover(&self, name: &str) -> anyhow::Result<Vec<ServiceInstance>> {
        if let Some(instances) = self.cached_instances(name) {
            self.ensure_watch_task(name.to_string());
            self.ensure_refresh_task(name.to_string());
            return Ok(instances);
        }

        let instances = self.registry.discover(name).await?;
        self.store(name, instances.clone());
        self.ensure_watch_task(name.to_string());
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
        if let Some(handle) = self
            .watch_tasks
            .lock()
            .expect("registry watch lock poisoned")
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
        self.cache
            .lock()
            .expect("registry cache lock poisoned")
            .insert(
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
                        cache.lock().expect("registry cache lock poisoned").insert(
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

    fn ensure_watch_task(&self, name: String) {
        if !self.registry.supports_watch() {
            return;
        }
        let mut watch_tasks = self
            .watch_tasks
            .lock()
            .expect("registry watch lock poisoned");
        if watch_tasks.contains_key(&name) {
            return;
        }

        let registry = Arc::clone(&self.registry);
        let cache = Arc::clone(&self.cache);
        let name_for_task = name.clone();
        let handle = tokio::spawn(async move {
            match registry.watch(&name_for_task).await {
                Ok(mut rx) => {
                    while let Some(instances) = rx.recv().await {
                        cache.lock().expect("registry cache lock poisoned").insert(
                            name_for_task.clone(),
                            CachedEntry {
                                discovered_at: Instant::now(),
                                instances,
                            },
                        );
                    }
                }
                Err(err) => {
                    tracing::warn!(
                        service = %name_for_task,
                        error = %err,
                        "registry watch unavailable"
                    );
                }
            }
        });

        watch_tasks.insert(name, handle);
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

pub fn instance_score(instance: &ServiceInstance) -> i64 {
    if let Some(value) = instance.metadata.get("healthy") {
        if matches!(value.as_str(), "false" | "0" | "no") {
            return i64::MIN / 2;
        }
    }

    let weight = instance.weight.max(1) as i64;
    let load_penalty = instance
        .metadata
        .get("load")
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or_default();
    let latency_penalty = instance
        .metadata
        .get("latency_ms")
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or_default();
    let error_penalty = instance
        .metadata
        .get("errors")
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or_default();

    weight * 1000 - load_penalty - latency_penalty - error_penalty * 10
}

pub fn pick_with_strategy(
    strategy: balance::BalancerKind,
    instances: &[ServiceInstance],
) -> Option<ServiceInstance> {
    balance::pick(strategy, instances)
}

#[cfg(test)]
mod tests {
    use std::sync::RwLock;

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
    fn registry_endpoint_accepts_hosts_without_scheme() {
        assert_eq!(
            normalize_registry_endpoint("127.0.0.1:2379"),
            "http://127.0.0.1:2379"
        );
        assert_eq!(
            normalize_registry_endpoint("http://127.0.0.1:2379/"),
            "http://127.0.0.1:2379"
        );
        assert_eq!(
            normalize_registry_endpoint("https://etcd.example:2379"),
            "https://etcd.example:2379"
        );
    }

    #[test]
    fn etcd_registry_prefix_defaults_and_normalizes() {
        assert_eq!(normalize_etcd_registry_prefix(""), "/roze/services");
        assert_eq!(
            normalize_etcd_registry_prefix("/shop/services/"),
            "/shop/services"
        );
        assert_eq!(
            etcd_service_prefix("/shop/services", "catalog"),
            "/shop/services/catalog/"
        );
        assert_eq!(
            etcd_instance_key("/shop/services", "catalog", "127.0.0.1:4000"),
            "/shop/services/catalog/127.0.0.1:4000"
        );
    }

    #[test]
    fn rpc_client_etcd_config_builds_registry_config() {
        let config = RpcClientEtcdConfig {
            hosts: vec!["127.0.0.1:2379".to_string()],
            key: "order.rpc".to_string(),
            ..Default::default()
        };
        let registry = registry_config_from_rpc_client_etcd(&config);

        assert_eq!(registry.kind, RegistryKind::Etcd);
        assert_eq!(registry.endpoints, config.hosts);
        assert_eq!(registry.prefix, "/roze/services");
        assert_eq!(registry.ttl_seconds, 10);
        assert_eq!(registry.renew_interval_secs, 3);
    }

    #[test]
    fn instance_score_prefers_healthy_low_load_instances() {
        let mut good = ServiceInstance::new("user", "127.0.0.1:8080");
        good.metadata.insert("load".into(), "1".into());
        good.metadata.insert("latency_ms".into(), "5".into());
        let mut bad = ServiceInstance::new("user", "127.0.0.1:8081");
        bad.metadata.insert("healthy".into(), "false".into());
        bad.metadata.insert("load".into(), "100".into());
        assert!(instance_score(&good) > instance_score(&bad));
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
        let discoveries = registry
            .discoveries
            .load(std::sync::atomic::Ordering::SeqCst);
        assert!(
            discoveries >= 2,
            "expected background refresh to run, got {discoveries}"
        );

        resolver.invalidate("user");
    }

    #[tokio::test]
    async fn cached_resolver_updates_from_registry_watch() {
        #[derive(Debug, Clone)]
        struct WatchRegistry {
            current: Arc<RwLock<Vec<ServiceInstance>>>,
            tx: mpsc::UnboundedSender<Vec<ServiceInstance>>,
            rx: Arc<Mutex<Option<mpsc::UnboundedReceiver<Vec<ServiceInstance>>>>>,
        }

        impl WatchRegistry {
            fn new(initial: Vec<ServiceInstance>) -> Self {
                let (tx, rx) = mpsc::unbounded_channel();
                Self {
                    current: Arc::new(RwLock::new(initial)),
                    tx,
                    rx: Arc::new(Mutex::new(Some(rx))),
                }
            }

            fn send_snapshot(&self, snapshot: Vec<ServiceInstance>) {
                *self.current.write().expect("registry lock poisoned") = snapshot.clone();
                self.tx.send(snapshot).expect("send snapshot");
            }
        }

        #[async_trait]
        impl Registry for WatchRegistry {
            async fn register(&self, instance: ServiceInstance) -> anyhow::Result<()> {
                self.current
                    .write()
                    .expect("registry lock poisoned")
                    .push(instance);
                Ok(())
            }

            async fn deregister(&self, _name: &str, addr: &str) -> anyhow::Result<()> {
                self.current
                    .write()
                    .expect("registry lock poisoned")
                    .retain(|instance| instance.addr != addr);
                Ok(())
            }

            async fn discover(&self, _name: &str) -> anyhow::Result<Vec<ServiceInstance>> {
                Ok(self.current.read().expect("registry lock poisoned").clone())
            }

            fn supports_watch(&self) -> bool {
                true
            }

            fn watch(&self, _name: &str) -> RegistryWatchFuture<'_> {
                Box::pin(async {
                    self.rx
                        .lock()
                        .expect("registry lock poisoned")
                        .take()
                        .ok_or_else(|| anyhow::anyhow!("watch already taken"))
                })
            }
        }

        let registry = WatchRegistry::new(vec![ServiceInstance::new("user", "127.0.0.1:8080")]);
        let resolver = CachedRegistryResolver::new(
            registry.clone(),
            crate::balance::FirstAvailableBalancer,
            Duration::from_secs(60),
        );

        let initial = resolver.discover("user").await.expect("discover");
        assert_eq!(initial[0].addr, "127.0.0.1:8080");

        registry.send_snapshot(vec![ServiceInstance::new("user", "127.0.0.1:8081")]);
        tokio::time::sleep(Duration::from_millis(20)).await;

        let watched = resolver.discover("user").await.expect("discover");
        assert_eq!(watched[0].addr, "127.0.0.1:8081");
        resolver.invalidate("user");
    }

    fn external_registry_config(kind: RegistryKind, endpoint: String) -> RegistryConfig {
        RegistryConfig {
            kind,
            endpoints: vec![endpoint],
            prefix: "/roze/services".to_string(),
            ttl_seconds: 10,
            renew_interval_secs: 2,
        }
    }

    #[tokio::test]
    #[ignore = "requires ROZE_TEST_ETCD_ENDPOINT, for example http://127.0.0.1:12379"]
    async fn etcd_registry_registers_discovers_and_deregisters_against_real_service() {
        let endpoint =
            std::env::var("ROZE_TEST_ETCD_ENDPOINT").expect("ROZE_TEST_ETCD_ENDPOINT is required");
        let config = external_registry_config(RegistryKind::Etcd, endpoint);
        let registry = EtcdRegistry::new(&config);
        let name = format!("roze-test-etcd-{}", std::process::id());
        let addr = "127.0.0.1:18080";
        let mut instance = ServiceInstance::new(&name, addr);
        instance.weight = 3;
        instance
            .metadata
            .insert("version".to_string(), "integration".to_string());

        registry.register(instance).await.expect("register");
        let instances = registry.discover(&name).await.expect("discover");
        assert_eq!(instances.len(), 1);
        assert_eq!(instances[0].addr, addr);
        assert_eq!(instances[0].weight, 3);
        assert_eq!(
            instances[0].metadata.get("version").map(String::as_str),
            Some("integration")
        );

        registry.deregister(&name, addr).await.expect("deregister");
        let instances = registry
            .discover(&name)
            .await
            .expect("discover after deregister");
        assert!(instances.is_empty());
    }

    #[tokio::test]
    #[ignore = "requires ROZE_TEST_CONSUL_ENDPOINT, for example http://127.0.0.1:18500"]
    async fn consul_registry_registers_discovers_and_deregisters_against_real_service() {
        let endpoint = std::env::var("ROZE_TEST_CONSUL_ENDPOINT")
            .expect("ROZE_TEST_CONSUL_ENDPOINT is required");
        let config = external_registry_config(RegistryKind::Consul, endpoint);
        let registry = ConsulRegistry::new(&config);
        let name = format!("roze-test-consul-{}", std::process::id());
        let addr = "127.0.0.1:18081";
        let mut instance = ServiceInstance::new(&name, addr);
        instance.weight = 5;
        instance
            .metadata
            .insert("version".to_string(), "integration".to_string());

        registry.register(instance).await.expect("register");
        tokio::time::sleep(Duration::from_millis(200)).await;
        let instances = registry.discover(&name).await.expect("discover");
        assert_eq!(instances.len(), 1);
        assert_eq!(instances[0].addr, addr);
        assert_eq!(instances[0].weight, 5);
        assert_eq!(
            instances[0].metadata.get("version").map(String::as_str),
            Some("integration")
        );

        registry.deregister(&name, addr).await.expect("deregister");
        let instances = registry
            .discover(&name)
            .await
            .expect("discover after deregister");
        assert!(instances.is_empty());
    }
}
