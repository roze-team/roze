use std::{
    collections::BTreeMap,
    fmt,
    net::SocketAddr,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex, OnceLock,
    },
    time::{Duration, Instant},
};

use dashmap::DashMap;

use crate::{
    balance::{
        build_balancer, AttemptLease, AttemptOutcome, Balancer, BalancerKind, EwmaP2cBalancer,
    },
    registry::{
        registry_config_from_rpc_client_etcd, CachedRegistryResolver, EtcdRegistry, Registry,
        ServiceInstance,
    },
};
use roze_context::{AuthContext, Context};
use roze_error::RozeError;
use roze_grpc::transport::{
    Channel, Code, Endpoint, MetadataMap, MetadataValue, Request, Response, Server, Status,
};
use roze_jwt::{extract_bearer_token, verify_token, JwtConfig};
use roze_metrics::{record_resilience_decision, record_rpc_client_attempt, record_rpc_method};
use roze_resilience::{
    full_jitter_delay, BreakerDecision, BreakerPermit, BreakerRegistry, GovernanceBoundary,
    OperationKey, RetryBudgetRegistry, SheddingRegistry,
};
use roze_trace::generate_trace_id;
use tokio::time::sleep;
use tracing::info;

static METHOD_BREAKERS: OnceLock<BreakerRegistry> = OnceLock::new();
static METHOD_SHEDDERS: OnceLock<SheddingRegistry> = OnceLock::new();
static CLIENT_RETRY_BUDGETS: OnceLock<RetryBudgetRegistry> = OnceLock::new();
static RPC_ENDPOINT_CURSOR: AtomicUsize = AtomicUsize::new(0);

pub const ERROR_CODE_METADATA: &str = "x-roze-error-code";
pub const ERROR_KIND_METADATA: &str = "x-roze-error-kind";
pub const FALLBACK_STATUS_METADATA: &str = "x-roze-fallback-status";
pub const FALLBACK_BODY_METADATA: &str = "x-roze-fallback-body";
pub const FALLBACK_HEADERS_METADATA: &str = "x-roze-fallback-headers";
pub const RETRY_AFTER_METADATA: &str = "retry-after";

#[derive(Debug, Clone)]
pub struct RpcConfig {
    pub addr: SocketAddr,
}

pub struct RpcServer {
    config: RpcConfig,
}

pub struct ServiceRegistrationGuard {
    registry: Arc<dyn Registry>,
    service_name: String,
    addr: String,
    active: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct RpcClientOptions {
    pub connect_timeout: Duration,
    pub request_timeout: Duration,
    pub keepalive_time: Duration,
    pub max_retries: usize,
    pub retry_backoff: Duration,
    pub retry_max_backoff: Duration,
    pub non_block: bool,
    pub trace: bool,
    pub stat: bool,
    pub prometheus: bool,
    pub breaker: bool,
}

impl Default for RpcClientOptions {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(5),
            request_timeout: Duration::from_millis(2_000),
            keepalive_time: Duration::from_secs(20),
            max_retries: 1,
            retry_backoff: Duration::from_millis(100),
            retry_max_backoff: Duration::from_millis(500),
            non_block: false,
            trace: true,
            stat: true,
            prometheus: true,
            breaker: true,
        }
    }
}

impl RpcClientOptions {
    pub fn from_config(config: &roze_config::RpcClientConfig) -> Self {
        Self {
            request_timeout: Duration::from_millis(config.timeout_ms),
            keepalive_time: Duration::from_secs(config.keepalive_time_secs),
            non_block: config.non_block,
            trace: config.middlewares.trace,
            stat: config.middlewares.stat,
            prometheus: config.middlewares.prometheus,
            breaker: config.middlewares.breaker,
            ..Self::default()
        }
    }
}

#[derive(Clone)]
pub struct DynamicRpcChannels {
    source: DynamicDiscoverySource,
    options: RpcClientOptions,
    balancer: EwmaP2cBalancer,
    channels: Arc<DashMap<String, Channel>>,
    cache: Arc<Mutex<Option<DynamicDiscoveryCache>>>,
    cache_ttl: Duration,
    watch_started: Arc<AtomicBool>,
}

#[derive(Clone)]
enum DynamicDiscoverySource {
    Static(Arc<Vec<ServiceInstance>>),
    Registry(Arc<dyn Registry>),
}

#[derive(Debug, Clone)]
struct DynamicDiscoveryCache {
    discovered_at: Instant,
    instances: Vec<ServiceInstance>,
}

pub struct DynamicRpcAttempt {
    channel: Channel,
    lease: AttemptLease,
}

impl fmt::Debug for DynamicRpcChannels {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DynamicRpcChannels")
            .field(
                "source",
                &match &self.source {
                    DynamicDiscoverySource::Static(_) => "static",
                    DynamicDiscoverySource::Registry(_) => "registry",
                },
            )
            .field("channel_count", &self.channels.len())
            .field("cache_ttl", &self.cache_ttl)
            .finish()
    }
}

impl DynamicRpcAttempt {
    pub fn into_parts(self) -> (Channel, AttemptLease) {
        (self.channel, self.lease)
    }
}

impl DynamicRpcChannels {
    pub fn from_config(config: &roze_config::RpcClientConfig) -> anyhow::Result<Option<Self>> {
        validate_rpc_client_config_mode(config)?;
        if config.balancer != roze_config::RpcClientBalancerKind::PowerOfTwoChoices {
            return Ok(None);
        }
        let source = if rpc_client_has_static_endpoints(config) {
            let instances = rpc_client_static_instances(config)?;
            if instances.len() < 2 {
                return Ok(None);
            }
            DynamicDiscoverySource::Static(Arc::new(instances))
        } else if let Some(etcd) = config.etcd.as_ref() {
            let registry_config = registry_config_from_rpc_client_etcd(etcd);
            DynamicDiscoverySource::Registry(Arc::new(EtcdRegistry::try_new(&registry_config)?))
        } else {
            return Ok(None);
        };
        Ok(Some(Self {
            source,
            options: RpcClientOptions::from_config(config),
            balancer: EwmaP2cBalancer::default(),
            channels: Arc::new(DashMap::new()),
            cache: Arc::new(Mutex::new(None)),
            cache_ttl: Duration::from_secs(5),
            watch_started: Arc::new(AtomicBool::new(false)),
        }))
    }

    pub fn with_cache_ttl(mut self, cache_ttl: Duration) -> Self {
        self.cache_ttl = cache_ttl;
        self
    }

    pub async fn attempt(&self, service: &str) -> Result<DynamicRpcAttempt, Status> {
        let instances = self.instances(service).await?;
        let lease = self
            .balancer
            .pick_tracked(&instances)
            .ok_or_else(|| Status::unavailable("no available RPC instances"))?;
        let addr = lease.instance().addr.clone();
        if let Some(channel) = self.channels.get(&addr).map(|entry| entry.clone()) {
            return Ok(DynamicRpcAttempt { channel, lease });
        }
        match connect_channel_with_options(&addr, self.options).await {
            Ok(channel) => {
                self.channels.insert(addr, channel.clone());
                Ok(DynamicRpcAttempt { channel, lease })
            }
            Err(_) => {
                lease.finish(AttemptOutcome::Failure);
                Err(Status::unavailable("RPC endpoint connection failed"))
            }
        }
    }

    /// Establishes the seed channel used to construct a generated tonic client.
    ///
    /// Generated clients replace this channel on every real call attempt. Trying
    /// every discovered address here prevents one dead endpoint from making the
    /// otherwise healthy dynamic client impossible to construct.
    pub async fn initial_channel(&self, service: &str) -> anyhow::Result<Channel> {
        let instances = self
            .instances(service)
            .await
            .map_err(|status| anyhow::anyhow!(status.message().to_owned()))?;
        let mut last_error = None;
        for instance in instances {
            if let Some(channel) = self.channels.get(&instance.addr).map(|entry| entry.clone()) {
                return Ok(channel);
            }
            match connect_channel_with_options(&instance.addr, self.options).await {
                Ok(channel) => {
                    self.channels.insert(instance.addr.clone(), channel.clone());
                    return Ok(channel);
                }
                Err(error) => last_error = Some(error),
            }
        }
        Err(last_error.unwrap_or_else(|| anyhow::anyhow!("no available RPC instances")))
    }

    pub fn balancer(&self) -> &EwmaP2cBalancer {
        &self.balancer
    }

    pub fn channel_count(&self) -> usize {
        self.channels.len()
    }

    async fn instances(&self, service: &str) -> Result<Vec<ServiceInstance>, Status> {
        let instances = match &self.source {
            DynamicDiscoverySource::Static(instances) => instances.as_ref().clone(),
            DynamicDiscoverySource::Registry(registry) => {
                self.ensure_registry_watch(service, registry).await;
                if let Some(instances) = self.cached_instances(false) {
                    instances
                } else {
                    match registry.discover(service).await {
                        Ok(instances) if !instances.is_empty() => {
                            *self.cache.lock().expect("RPC discovery cache poisoned") =
                                Some(DynamicDiscoveryCache {
                                    discovered_at: Instant::now(),
                                    instances: instances.clone(),
                                });
                            instances
                        }
                        Ok(_) => self.cached_instances(true).ok_or_else(|| {
                            Status::unavailable("RPC registry returned no instances")
                        })?,
                        Err(_) => self
                            .cached_instances(true)
                            .ok_or_else(|| Status::unavailable("RPC registry discovery failed"))?,
                    }
                }
            }
        };
        let active = instances
            .iter()
            .map(|instance| instance.addr.as_str())
            .collect::<std::collections::HashSet<_>>();
        self.channels
            .retain(|addr, _| active.contains(addr.as_str()));
        Ok(instances)
    }

    fn cached_instances(&self, allow_stale: bool) -> Option<Vec<ServiceInstance>> {
        let cache = self.cache.lock().expect("RPC discovery cache poisoned");
        let entry = cache.as_ref()?;
        if allow_stale || entry.discovered_at.elapsed() <= self.cache_ttl {
            Some(entry.instances.clone())
        } else {
            None
        }
    }

    async fn ensure_registry_watch(&self, service: &str, registry: &Arc<dyn Registry>) {
        if !registry.supports_watch()
            || self
                .watch_started
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
        {
            return;
        }
        let mut receiver = match registry.watch(service).await {
            Ok(receiver) => receiver,
            Err(error) => {
                self.watch_started.store(false, Ordering::Release);
                tracing::warn!(
                    service,
                    error = %error,
                    "RPC registry watch could not start; TTL discovery remains active"
                );
                return;
            }
        };
        let cache = Arc::clone(&self.cache);
        let watch_started = Arc::clone(&self.watch_started);
        let service = service.to_string();
        tokio::spawn(async move {
            while let Some(instances) = receiver.recv().await {
                *cache.lock().expect("RPC discovery cache poisoned") =
                    Some(DynamicDiscoveryCache {
                        discovered_at: Instant::now(),
                        instances,
                    });
            }
            watch_started.store(false, Ordering::Release);
            tracing::warn!(
                service,
                "RPC registry watch ended; TTL discovery will restart it"
            );
        });
    }
}

pub fn finish_attempt_status<T>(lease: AttemptLease, result: &Result<T, Status>) {
    finish_attempt_status_for("", "", lease, result);
}

pub fn finish_attempt_status_for<T>(
    service: &str,
    method: &str,
    lease: AttemptLease,
    result: &Result<T, Status>,
) {
    let outcome = match result {
        Ok(_) => AttemptOutcome::Success,
        Err(status) if status.code() == Code::DeadlineExceeded => AttemptOutcome::Timeout,
        Err(status) if status.code() == Code::Cancelled => AttemptOutcome::Cancelled,
        Err(_) => AttemptOutcome::Failure,
    };
    if !service.is_empty() && !method.is_empty() {
        let outcome_label = match outcome {
            AttemptOutcome::Success => "success",
            AttemptOutcome::Failure => "failure",
            AttemptOutcome::Timeout => "timeout",
            AttemptOutcome::Cancelled => "cancelled",
        };
        record_rpc_client_attempt(service, method, outcome_label);
    }
    lease.finish(outcome);
}

impl RpcServer {
    pub fn new(addr: SocketAddr) -> Self {
        Self {
            config: RpcConfig { addr },
        }
    }

    pub fn builder(&self) -> Server {
        info!(addr = %self.config.addr, "building RPC server");
        Server::builder()
    }
}

impl ServiceRegistrationGuard {
    pub async fn start(
        registry: Arc<dyn Registry>,
        service_name: impl Into<String>,
        addr: SocketAddr,
    ) -> anyhow::Result<Self> {
        Self::start_with_advertise_addr(registry, service_name, addr, addr).await
    }

    pub async fn start_with_advertise_addr(
        registry: Arc<dyn Registry>,
        service_name: impl Into<String>,
        _bind_addr: SocketAddr,
        advertise_addr: SocketAddr,
    ) -> anyhow::Result<Self> {
        let service_name = service_name.into();
        let addr = advertise_addr.to_string();
        registry
            .register(ServiceInstance::new(service_name.clone(), addr.clone()))
            .await?;
        Ok(Self {
            registry,
            service_name,
            addr,
            active: true,
        })
    }

    pub async fn shutdown(&mut self) -> anyhow::Result<()> {
        if self.active {
            self.registry
                .deregister(&self.service_name, &self.addr)
                .await?;
            self.active = false;
        }
        Ok(())
    }
}

impl Drop for ServiceRegistrationGuard {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        self.active = false;
        let registry = Arc::clone(&self.registry);
        let service_name = self.service_name.clone();
        let addr = self.addr.clone();
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                let _ = registry.deregister(&service_name, &addr).await;
            });
        }
    }
}

#[allow(clippy::result_large_err)]
pub fn auth_interceptor(
    config: JwtConfig,
) -> impl FnMut(Request<()>) -> Result<Request<()>, Status> + Clone {
    move |mut req: Request<()>| {
        let header_value = req
            .metadata()
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            .ok_or_else(|| Status::unauthenticated("missing authorization header"))?;
        let token = extract_bearer_token(header_value)
            .ok_or_else(|| Status::unauthenticated("missing bearer token"))?;
        let claims =
            verify_token(token, &config).map_err(|err| Status::unauthenticated(err.to_string()))?;
        insert_metadata(
            req.metadata_mut(),
            roze_context::SUBJECT_HEADER,
            &claims.sub,
        );
        if let Some(tenant) = claims.tenant.as_deref() {
            insert_metadata(req.metadata_mut(), roze_context::TENANT_HEADER, tenant);
        }
        if !claims.roles.is_empty() {
            insert_metadata(
                req.metadata_mut(),
                roze_context::ROLES_HEADER,
                &claims.roles.join(","),
            );
        }
        let context = request_context(&req).with_auth(AuthContext {
            subject: claims.sub.clone(),
            roles: claims.roles.clone(),
            tenant: claims.tenant.clone(),
        });
        req.extensions_mut().insert(roze_auth::principal(
            claims.sub.clone(),
            claims.roles.clone(),
            claims.tenant.clone(),
        ));
        apply_request_context(&mut req, &context);
        Ok(req)
    }
}

pub async fn connect_channel(addr: impl AsRef<str>) -> anyhow::Result<Channel> {
    connect_channel_with_options(addr, RpcClientOptions::default()).await
}

pub async fn connect_channel_with_options(
    addr: impl AsRef<str>,
    options: RpcClientOptions,
) -> anyhow::Result<Channel> {
    let url = normalize_endpoint(addr.as_ref())?;
    tracing::debug!(
        protocol = "rpc",
        endpoint = %safe_endpoint_label(&url),
        connect_timeout_ms = options.connect_timeout.as_millis(),
        request_timeout_ms = options.request_timeout.as_millis(),
        "RPC channel connection starting"
    );
    let channel = Endpoint::from_shared(url)?
        .connect_timeout(options.connect_timeout)
        .timeout(options.request_timeout)
        .http2_keep_alive_interval(options.keepalive_time)
        .connect()
        .await?;
    tracing::debug!(protocol = "rpc", "RPC channel connected");
    Ok(channel)
}

fn safe_endpoint_label(endpoint: &str) -> String {
    let endpoint = endpoint
        .split(['?', '#'])
        .next()
        .unwrap_or(endpoint)
        .trim_end_matches('/');
    if let Some((scheme, rest)) = endpoint.split_once("://") {
        let authority = rest.split('/').next().unwrap_or_default();
        let authority = authority
            .rsplit_once('@')
            .map_or(authority, |(_, host)| host);
        format!("{scheme}://{authority}")
    } else {
        let authority = endpoint.split('/').next().unwrap_or_default();
        authority
            .rsplit_once('@')
            .map_or(authority, |(_, host)| host)
            .to_string()
    }
}

pub async fn connect_channel_from_config(
    config: &roze_config::RpcClientConfig,
) -> anyhow::Result<Channel> {
    validate_rpc_client_config_mode(config)?;
    let options = RpcClientOptions::from_config(config);
    if let Some(target) = config.target.as_deref().filter(|target| !target.is_empty()) {
        info!(
            mode = "target",
            "rpc client config selected direct target mode"
        );
        return connect_channel_with_options(target, options).await;
    }

    if rpc_client_has_static_endpoints(config) {
        let target = rpc_client_pick_static_endpoint(config, &RPC_ENDPOINT_CURSOR)?;
        info!(
            mode = "endpoints",
            balancer = ?config.balancer,
            "rpc client config selected static endpoint mode"
        );
        return connect_channel_with_options(target, options).await;
    }

    if let Some(etcd) = config.etcd.as_ref() {
        let registry_config = registry_config_from_rpc_client_etcd(etcd);
        let registry = EtcdRegistry::try_new(&registry_config)?;
        info!(
            mode = "etcd",
            service = %etcd.key,
            "rpc client config selected etcd discovery mode"
        );
        let balancer = build_balancer(rpc_client_balancer_kind(config));
        return connect_via_registry_with_options(&etcd.key, &registry, balancer.as_ref(), options)
            .await;
    }

    let target = rpc_client_target(config)?;
    connect_channel_with_options(target, options).await
}

pub fn rpc_client_balancer_kind(config: &roze_config::RpcClientConfig) -> BalancerKind {
    match config.balancer {
        roze_config::RpcClientBalancerKind::FirstAvailable => BalancerKind::FirstAvailable,
        roze_config::RpcClientBalancerKind::RoundRobin => BalancerKind::RoundRobin,
        roze_config::RpcClientBalancerKind::WeightedRoundRobin => BalancerKind::WeightedRoundRobin,
        roze_config::RpcClientBalancerKind::PowerOfTwoChoices => BalancerKind::PowerOfTwoChoices,
        roze_config::RpcClientBalancerKind::HealthAware => BalancerKind::HealthAware,
    }
}

pub fn validate_rpc_client_config_mode(
    config: &roze_config::RpcClientConfig,
) -> anyhow::Result<()> {
    let modes = [
        rpc_client_has_direct_target(config),
        rpc_client_has_static_endpoints(config),
        config.etcd.is_some(),
    ]
    .into_iter()
    .filter(|enabled| *enabled)
    .count();
    if modes > 1 {
        anyhow::bail!(
            "rpc client config must select exactly one connection mode: target, endpoints, or etcd"
        );
    }
    Ok(())
}

pub fn rpc_client_has_direct_target(config: &roze_config::RpcClientConfig) -> bool {
    config
        .target
        .as_deref()
        .is_some_and(|target| !target.is_empty())
}

pub fn rpc_client_has_static_endpoints(config: &roze_config::RpcClientConfig) -> bool {
    config.endpoints.iter().any(|endpoint| !endpoint.is_empty())
}

pub fn rpc_client_round_robin_endpoint<'a>(
    config: &'a roze_config::RpcClientConfig,
    cursor: &AtomicUsize,
) -> anyhow::Result<&'a str> {
    let endpoints = config
        .endpoints
        .iter()
        .map(String::as_str)
        .filter(|endpoint| !endpoint.is_empty())
        .collect::<Vec<_>>();
    if endpoints.is_empty() {
        anyhow::bail!("rpc client config must set at least one endpoint")
    }
    let idx = cursor.fetch_add(1, Ordering::Relaxed) % endpoints.len();
    Ok(endpoints[idx])
}

pub fn rpc_client_pick_static_endpoint(
    config: &roze_config::RpcClientConfig,
    cursor: &AtomicUsize,
) -> anyhow::Result<String> {
    if config.balancer == roze_config::RpcClientBalancerKind::RoundRobin {
        let picked = rpc_client_round_robin_endpoint(config, cursor)?.to_string();
        tracing::debug!(
            protocol = "rpc",
            balancer = ?config.balancer,
            candidate_count = config.endpoints.iter().filter(|endpoint| !endpoint.is_empty()).count(),
            selected_endpoint = %safe_endpoint_label(&picked),
            "RPC static endpoint selected"
        );
        return Ok(picked);
    }
    let instances = rpc_client_static_instances(config)?;
    let picked = build_balancer(rpc_client_balancer_kind(config))
        .pick(&instances)
        .ok_or_else(|| anyhow::anyhow!("rpc client config must set at least one endpoint"))?;
    tracing::debug!(
        protocol = "rpc",
        balancer = ?config.balancer,
        candidate_count = instances.len(),
        selected_endpoint = %safe_endpoint_label(&picked.addr),
        "RPC static endpoint selected"
    );
    Ok(picked.addr)
}

pub fn rpc_client_static_instances(
    config: &roze_config::RpcClientConfig,
) -> anyhow::Result<Vec<ServiceInstance>> {
    let instances = config
        .endpoints
        .iter()
        .map(String::as_str)
        .filter(|endpoint| !endpoint.is_empty())
        .map(|endpoint| ServiceInstance::new("static", endpoint))
        .collect::<Vec<_>>();
    if instances.is_empty() {
        anyhow::bail!("rpc client config must set at least one endpoint")
    }
    Ok(instances)
}

pub fn rpc_client_target(config: &roze_config::RpcClientConfig) -> anyhow::Result<&str> {
    if let Some(target) = config.target.as_deref().filter(|target| !target.is_empty()) {
        return Ok(target);
    }
    if let Some(endpoint) = config
        .endpoints
        .iter()
        .map(String::as_str)
        .find(|endpoint| !endpoint.is_empty())
    {
        return Ok(endpoint);
    }
    if config.etcd.is_some() {
        anyhow::bail!(
            "rpc client etcd discovery config requires generated client registry resolver connection"
        );
    }
    anyhow::bail!("rpc client config must set target or at least one endpoint")
}

pub async fn connect_via_registry<R, B>(
    service: &str,
    registry: &R,
    balancer: &B,
) -> anyhow::Result<Channel>
where
    R: Registry,
    B: Balancer + ?Sized,
{
    connect_via_registry_with_options(service, registry, balancer, RpcClientOptions::default())
        .await
}

pub async fn connect_via_registry_with_options<R, B>(
    service: &str,
    registry: &R,
    balancer: &B,
    options: RpcClientOptions,
) -> anyhow::Result<Channel>
where
    R: Registry,
    B: Balancer + ?Sized,
{
    let instances = registry.discover(service).await?;
    let instance = balancer
        .pick(&instances)
        .ok_or_else(|| anyhow::anyhow!("no available instances for service `{service}`"))?;
    connect_channel_with_options(instance.addr, options).await
}

pub async fn connect_via_cached_registry_with_options<R, B>(
    service: &str,
    resolver: &CachedRegistryResolver<R, B>,
    options: RpcClientOptions,
) -> anyhow::Result<Channel>
where
    R: Registry,
    B: Balancer,
{
    let instance = resolver
        .pick(service)
        .await?
        .ok_or_else(|| anyhow::anyhow!("no available instances for service `{service}`"))?;
    connect_channel_with_options(instance.addr, options).await
}

pub fn normalize_endpoint(addr: &str) -> anyhow::Result<String> {
    if addr.starts_with("http://") || addr.starts_with("https://") {
        Ok(addr.to_string())
    } else {
        Ok(format!("http://{addr}"))
    }
}

pub fn should_retry_status(status: &Status) -> bool {
    matches!(
        status.code(),
        Code::Unavailable | Code::DeadlineExceeded | Code::Unknown
    )
}

pub fn trace_id_from_metadata(request: &Request<()>) -> Option<String> {
    request
        .metadata()
        .get(roze_context::TRACE_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

pub fn request_context<T>(request: &Request<T>) -> Context {
    request
        .extensions()
        .get::<Context>()
        .cloned()
        .unwrap_or_else(|| {
            let request_id = request
                .metadata()
                .get(roze_context::REQUEST_ID_HEADER)
                .and_then(|value| value.to_str().ok())
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
                .unwrap_or_else(generate_trace_id);
            let trace_id = request
                .metadata()
                .get(roze_context::TRACE_ID_HEADER)
                .and_then(|value| value.to_str().ok())
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
                .unwrap_or_else(generate_trace_id);
            let mut ctx = Context::background_with_request_id_and_trace_id(request_id, trace_id)
                .with_metadata_map(context_metadata_from_tonic(request.metadata()));
            if let Some(auth) = context_auth_from_tonic(request.metadata()) {
                ctx = ctx.with_auth(auth);
            }
            let ctx = match request
                .metadata()
                .get(roze_context::TIMEOUT_HEADER)
                .and_then(|value| value.to_str().ok())
                .and_then(|raw| raw.parse::<u64>().ok())
                .map(Duration::from_millis)
            {
                Some(timeout) => ctx.with_timeout(timeout),
                None => ctx,
            };
            match request
                .metadata()
                .get(roze_context::RETRY_BUDGET_HEADER)
                .and_then(|value| value.to_str().ok())
                .and_then(|raw| raw.parse::<usize>().ok())
            {
                Some(remaining) => ctx.with_retry_budget(remaining),
                None => ctx,
            }
        })
}

pub fn apply_request_context<T>(request: &mut Request<T>, context: &Context) {
    insert_metadata(
        request.metadata_mut(),
        roze_context::REQUEST_ID_HEADER,
        &context.request_id(),
    );
    insert_metadata(
        request.metadata_mut(),
        roze_context::TRACE_ID_HEADER,
        &context.trace_id(),
    );
    if let Some(timeout) = context.remaining_timeout() {
        // Preserve a still-live deadline when sub-millisecond precision is
        // lost at the metadata boundary. Encoding `0` would make the
        // receiver treat the request as already expired.
        let timeout_ms = timeout_metadata_millis(timeout);
        insert_metadata(
            request.metadata_mut(),
            roze_context::TIMEOUT_HEADER,
            &timeout_ms,
        );
    }
    if let Some(remaining) = context.retry_budget_remaining() {
        insert_metadata(
            request.metadata_mut(),
            roze_context::RETRY_BUDGET_HEADER,
            &remaining.to_string(),
        );
    }
    if let Some(auth) = context.auth() {
        insert_metadata(
            request.metadata_mut(),
            roze_context::SUBJECT_HEADER,
            &auth.subject,
        );
        if let Some(tenant) = auth.tenant {
            insert_metadata(request.metadata_mut(), roze_context::TENANT_HEADER, &tenant);
        }
        if !auth.roles.is_empty() {
            insert_metadata(
                request.metadata_mut(),
                roze_context::ROLES_HEADER,
                &auth.roles.join(","),
            );
        }
    }
    if let Some(idempotency_key) = context.idempotency_key() {
        insert_metadata(
            request.metadata_mut(),
            roze_context::IDEMPOTENCY_KEY_HEADER,
            &idempotency_key,
        );
    }
    for (key, value) in context.metadata() {
        let header = format!("{}{}", roze_context::METADATA_HEADER_PREFIX, key);
        insert_metadata(request.metadata_mut(), &header, &value);
    }
}

fn timeout_metadata_millis(timeout: Duration) -> String {
    timeout.as_millis().max(1).to_string()
}

pub fn response_with_context<T>(payload: T, context: &Context) -> Response<T> {
    let mut response = Response::new(payload);
    if let Some(remaining) = context.retry_budget_remaining() {
        insert_metadata(
            response.metadata_mut(),
            roze_context::RETRY_BUDGET_HEADER,
            &remaining.to_string(),
        );
    }
    response
}

pub struct RetryBudgetDelegation {
    context: Context,
    allocated: usize,
}

impl RetryBudgetDelegation {
    pub fn context(&self) -> &Context {
        &self.context
    }

    pub fn allocated(&self) -> usize {
        self.allocated
    }
}

pub fn delegate_retry_budget(context: &Context) -> RetryBudgetDelegation {
    let available = context.retry_budget_remaining().unwrap_or(0);
    let allocated = context.take_retry_budget_up_to(available / 2);
    RetryBudgetDelegation {
        context: context.with_retry_budget(allocated),
        allocated,
    }
}

pub fn reconcile_delegated_retry_budget<T>(
    context: &Context,
    delegation: RetryBudgetDelegation,
    result: &Result<Response<T>, Status>,
) {
    let metadata = match result {
        Ok(response) => response.metadata(),
        Err(status) => status.metadata(),
    };
    let returned = metadata_value(metadata, roze_context::RETRY_BUDGET_HEADER)
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0)
        .min(delegation.allocated);
    context.restore_retry_budget(returned);
}

pub fn reconcile_retry_budget<T>(context: &Context, result: &Result<Response<T>, Status>) {
    let metadata = match result {
        Ok(response) => response.metadata(),
        Err(status) => status.metadata(),
    };
    if let Some(remaining) = metadata_value(metadata, roze_context::RETRY_BUDGET_HEADER)
        .and_then(|value| value.parse::<usize>().ok())
    {
        context.limit_retry_budget(remaining);
    }
}

pub fn status_from_error(error: RozeError, context: &Context) -> Status {
    let code = grpc_code_from_error(&error);
    let locale = context.locale();
    let mut metadata = MetadataMap::new();
    insert_metadata(
        &mut metadata,
        ERROR_CODE_METADATA,
        &error.code().to_string(),
    );
    insert_metadata(&mut metadata, ERROR_KIND_METADATA, error.kind());
    insert_metadata(
        &mut metadata,
        roze_context::REQUEST_ID_HEADER,
        &context.request_id(),
    );
    insert_metadata(
        &mut metadata,
        roze_context::TRACE_ID_HEADER,
        &context.trace_id(),
    );
    if let Some(locale) = locale.as_deref() {
        insert_metadata(&mut metadata, roze_context::LOCALE_HEADER, locale);
    }
    if let Some(remaining) = context.retry_budget_remaining() {
        insert_metadata(
            &mut metadata,
            roze_context::RETRY_BUDGET_HEADER,
            &remaining.to_string(),
        );
    }
    if let RozeError::Fallback {
        status,
        body,
        headers,
    } = &error
    {
        insert_metadata(&mut metadata, FALLBACK_STATUS_METADATA, &status.to_string());
        if let Some(body) = body {
            insert_metadata(&mut metadata, FALLBACK_BODY_METADATA, &body.to_string());
        }
        if !headers.is_empty() {
            if let Ok(headers) = serde_json::to_string(headers) {
                insert_metadata(&mut metadata, FALLBACK_HEADERS_METADATA, &headers);
            }
        }
    }
    if let Some(retry_after) = error.retry_after_seconds() {
        insert_metadata(
            &mut metadata,
            RETRY_AFTER_METADATA,
            &retry_after.to_string(),
        );
    }
    Status::with_metadata(
        code,
        error.message_i18n(locale.as_deref().unwrap_or("en-US")),
        metadata,
    )
}

fn grpc_code_from_error(error: &RozeError) -> Code {
    match error {
        RozeError::BadRequest(_) => Code::InvalidArgument,
        RozeError::Conflict(_) => Code::AlreadyExists,
        RozeError::FailedPrecondition(_) => Code::FailedPrecondition,
        RozeError::Unauthorized => Code::Unauthenticated,
        RozeError::Forbidden => Code::PermissionDenied,
        RozeError::RateLimited { .. } => Code::ResourceExhausted,
        RozeError::NotFound(_) => Code::NotFound,
        RozeError::Unavailable(_) => Code::Unavailable,
        RozeError::Internal(_) => Code::Internal,
        RozeError::Fallback { status, .. } => match *status {
            400 | 422 => Code::InvalidArgument,
            401 => Code::Unauthenticated,
            403 => Code::PermissionDenied,
            404 => Code::NotFound,
            409 => Code::AlreadyExists,
            429 => Code::ResourceExhausted,
            500 => Code::Internal,
            _ => Code::Unavailable,
        },
    }
}

pub fn invalid_argument_status(message: impl Into<String>, context: &Context) -> Status {
    let error = RozeError::BadRequest(message.into());
    status_from_error(error, context)
}

pub fn enforce_permissions<S>(context: &Context, required: &[S]) -> Result<(), Status>
where
    S: AsRef<str>,
{
    if required.is_empty() {
        return Ok(());
    }
    if context.has_permissions(required.iter().map(AsRef::as_ref)) {
        Ok(())
    } else {
        Err(status_from_error(RozeError::Forbidden, context))
    }
}

pub fn error_from_status(status: &Status) -> RozeError {
    let error_kind = metadata_value(status.metadata(), ERROR_KIND_METADATA);
    if error_kind == Some("fallback") {
        let fallback_status = metadata_value(status.metadata(), FALLBACK_STATUS_METADATA)
            .and_then(|value| value.parse::<u16>().ok())
            .unwrap_or(503);
        let body = metadata_value(status.metadata(), FALLBACK_BODY_METADATA)
            .and_then(|value| serde_json::from_str(value).ok());
        let headers = metadata_value(status.metadata(), FALLBACK_HEADERS_METADATA)
            .and_then(|value| serde_json::from_str::<BTreeMap<String, String>>(value).ok())
            .unwrap_or_default();
        return RozeError::fallback_response(fallback_status, body, headers);
    }
    if error_kind == Some("conflict") {
        return RozeError::Conflict(status.message().to_string());
    }
    if error_kind == Some("failed_precondition") {
        return RozeError::FailedPrecondition(status.message().to_string());
    }
    match status.code() {
        Code::InvalidArgument => RozeError::BadRequest(status.message().to_string()),
        Code::Unauthenticated => RozeError::Unauthorized,
        Code::PermissionDenied => RozeError::Forbidden,
        Code::ResourceExhausted => RozeError::RateLimited {
            retry_after_seconds: metadata_value(status.metadata(), RETRY_AFTER_METADATA)
                .and_then(|value| value.parse().ok())
                .unwrap_or(1),
        },
        Code::NotFound => RozeError::NotFound(status.message().to_string()),
        Code::AlreadyExists | Code::Aborted => RozeError::Conflict(status.message().to_string()),
        Code::FailedPrecondition => RozeError::FailedPrecondition(status.message().to_string()),
        Code::Unavailable => RozeError::Unavailable(status.message().to_string()),
        _ => RozeError::Internal(status.message().to_string()),
    }
}

pub fn apply_fallback(
    service: impl Into<String>,
    error: RozeError,
    fallback: Option<roze_config::GovernanceFallbackConfig>,
) -> RozeError {
    if error.is_client_error() {
        return error;
    }
    let Some(fallback) = fallback else {
        return error;
    };
    record_resilience_decision(service, "rpc", "fallback", "served");
    RozeError::fallback_response(fallback.status, fallback.body, fallback.headers)
}

fn insert_metadata(metadata: &mut MetadataMap, key: &str, value: &str) {
    let Ok(key) = key.parse::<roze_grpc::transport::MetadataKey<roze_grpc::transport::Ascii>>()
    else {
        return;
    };
    let Ok(value) = MetadataValue::try_from(value) else {
        return;
    };
    metadata.insert(key, value);
}

fn metadata_value<'a>(metadata: &'a MetadataMap, key: &str) -> Option<&'a str> {
    metadata.get(key).and_then(|value| value.to_str().ok())
}

fn context_auth_from_tonic(metadata: &MetadataMap) -> Option<AuthContext> {
    let subject = metadata
        .get(roze_context::SUBJECT_HEADER)
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())?
        .to_string();
    let tenant = metadata
        .get(roze_context::TENANT_HEADER)
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    let roles = metadata
        .get(roze_context::ROLES_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(parse_roles)
        .unwrap_or_default();
    Some(AuthContext {
        subject,
        roles,
        tenant,
    })
}

fn context_metadata_from_tonic(
    metadata: &MetadataMap,
) -> std::collections::BTreeMap<String, String> {
    metadata
        .iter()
        .filter_map(|entry| {
            let roze_grpc::transport::KeyAndValueRef::Ascii(key, value) = entry else {
                return None;
            };
            let key = key
                .as_str()
                .strip_prefix(roze_context::METADATA_HEADER_PREFIX)?;
            Some((key.to_string(), value.to_str().ok()?.to_string()))
        })
        .collect()
}

fn parse_roles(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|role| !role.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

pub fn apply_client_auth<T>(
    request: &mut Request<T>,
    options: &RpcClientOptions,
    config: Option<&roze_config::RpcClientConfig>,
) {
    if !options.trace {
        request.metadata_mut().remove(roze_context::TRACE_ID_HEADER);
    }
    let Some(config) = config else {
        return;
    };
    if let Some(app) = config.app.as_deref().filter(|app| !app.is_empty()) {
        if let Ok(value) = MetadataValue::try_from(app) {
            request.metadata_mut().insert("x-app", value);
        }
    }
    if let Some(token) = config.token.as_deref().filter(|token| !token.is_empty()) {
        let authorization = format!("Bearer {token}");
        if let Ok(value) = MetadataValue::try_from(authorization.as_str()) {
            request.metadata_mut().insert("authorization", value);
        }
    }
}

pub fn client_request<T>(
    payload: T,
    context: &Context,
    options: RpcClientOptions,
    config: Option<&roze_config::RpcClientConfig>,
) -> Request<T> {
    let mut request = Request::new(payload);
    if let Some(timeout) = context.remaining_timeout() {
        request.set_timeout(timeout);
    } else {
        request.set_timeout(options.request_timeout);
    }
    apply_request_context(&mut request, context);
    apply_client_auth(&mut request, &options, config);
    request
}

#[derive(Debug, Clone)]
pub struct MethodPolicy {
    pub timeout: Option<Duration>,
    pub rate_limit: Option<MethodRateLimitConfig>,
    pub breaker: Option<MethodBreakerConfig>,
    pub shedding: Option<MethodSheddingConfig>,
    pub fallback: Option<roze_config::GovernanceFallbackConfig>,
}

#[derive(Debug, Clone)]
pub struct MethodRateLimitConfig {
    pub burst: u32,
    pub refill: Duration,
    pub tokens_per_refill: u32,
    pub key: roze_rate_limit::RateLimitKeyPolicy,
}

#[derive(Debug, Clone, Copy)]
pub struct MethodBreakerConfig {
    pub failure_threshold: u32,
    pub reset_timeout: Duration,
}

#[derive(Debug, Clone, Copy)]
pub struct MethodSheddingConfig {
    pub concurrency: usize,
    pub window: Duration,
    pub min_samples: u64,
    pub max_avg_latency: Duration,
    pub max_failure_ratio_per_mille: u32,
    pub cool_down: Duration,
}

#[derive(Debug, Clone, Copy)]
pub struct MethodRetryPolicy {
    pub max_retries: usize,
    pub backoff: Duration,
    pub max_backoff: Duration,
    pub budget_percent: Option<u32>,
}

#[derive(Debug)]
pub struct MethodGuard {
    key: String,
    service: String,
    method: String,
    request_id: String,
    trace_id: String,
    started_at: Instant,
    breaker: Option<MethodBreakerConfig>,
    breaker_permit: Option<BreakerPermit>,
    shedding: Option<MethodSheddingConfig>,
    finished: bool,
}

impl Drop for MethodGuard {
    fn drop(&mut self) {
        if self.finished {
            return;
        }

        tracing::warn!(
            protocol = "rpc",
            service = %self.service,
            method = %self.method,
            elapsed_ms = self.started_at.elapsed().as_millis(),
            request_id = %self.request_id,
            trace_id = %self.trace_id,
            "RPC method cancelled"
        );

        let elapsed = self.started_at.elapsed();
        if let (Some(config), Some(permit)) = (self.breaker, self.breaker_permit) {
            method_breaker_cancel(&self.key, permit, &config);
            if permit == BreakerPermit::Probe {
                record_resilience_decision(
                    self.service.as_str(),
                    "rpc",
                    "breaker",
                    "probe_cancelled",
                );
            }
        }
        if self.shedding.is_some() {
            method_shedding_release(&self.key);
            record_resilience_decision(self.service.as_str(), "rpc", "load_shedding", "cancelled");
        }
        record_rpc_method(
            self.service.as_str(),
            self.method.as_str(),
            "cancelled",
            elapsed,
        );
    }
}

pub fn method_policy(
    governance: Option<&roze_config::GovernanceConfig>,
    method: &str,
) -> MethodPolicy {
    let Some(governance) = governance else {
        return MethodPolicy {
            timeout: None,
            rate_limit: None,
            breaker: None,
            shedding: None,
            fallback: None,
        };
    };
    let policy = governance.resolve_policy(method);
    let rate_limit = governance.resolve_rate_limit_config(method);
    MethodPolicy {
        timeout: policy.timeout,
        rate_limit: rate_limit.map(|config| MethodRateLimitConfig {
            burst: config.burst,
            refill: Duration::from_millis(config.refill_ms),
            tokens_per_refill: config.tokens_per_refill,
            key: config.key,
        }),
        breaker: policy.breaker.map(|config| MethodBreakerConfig {
            failure_threshold: config.failure_threshold,
            reset_timeout: config.reset_timeout,
        }),
        shedding: policy.shedding.map(|config| MethodSheddingConfig {
            concurrency: config.concurrency,
            window: config.window,
            min_samples: config.min_samples,
            max_avg_latency: config.max_avg_latency,
            max_failure_ratio_per_mille: config.max_failure_ratio_per_mille,
            cool_down: config.cool_down,
        }),
        fallback: policy
            .fallback
            .map(|fallback| roze_config::GovernanceFallbackConfig {
                enabled: true,
                status: fallback.status,
                body: fallback.body,
                headers: fallback.headers,
            }),
    }
}

pub fn method_fallback(
    governance: Option<&roze_config::GovernanceConfig>,
    method: &str,
) -> Option<roze_config::GovernanceFallbackConfig> {
    method_policy(governance, method).fallback
}

pub fn retry_policy_for_method(
    options: RpcClientOptions,
    governance: Option<&roze_config::GovernanceConfig>,
    method: &str,
) -> MethodRetryPolicy {
    let Some(governance) = governance else {
        return MethodRetryPolicy::from_options(options);
    };
    let retry = governance.resolve_policy(method).retry;
    match retry {
        Some(retry) => MethodRetryPolicy {
            max_retries: retry.max_attempts.saturating_sub(1) as usize,
            backoff: retry.backoff,
            max_backoff: retry.max_backoff,
            budget_percent: retry.budget_percent,
        },
        None => MethodRetryPolicy::from_options(options),
    }
}

impl MethodRetryPolicy {
    pub fn from_options(options: RpcClientOptions) -> Self {
        Self {
            max_retries: options.max_retries,
            backoff: options.retry_backoff,
            max_backoff: options.retry_max_backoff,
            budget_percent: None,
        }
    }
}

#[allow(clippy::result_large_err)]
pub fn begin_method(
    service: impl Into<String>,
    method: impl Into<String>,
    request_ctx: Context,
    governance: Option<&roze_config::GovernanceConfig>,
) -> Result<(Context, MethodGuard), Status> {
    let service = service.into();
    let method = method.into();
    let policy = method_policy(governance, &method);
    tracing::debug!(
        protocol = "rpc",
        service = %service,
        method = %method,
        timeout_ms = policy.timeout.map(|value| value.as_millis()),
        rate_limit = policy.rate_limit.is_some(),
        breaker = policy.breaker.is_some(),
        shedding = policy.shedding.is_some(),
        fallback = policy.fallback.as_ref().is_some_and(|value| value.enabled),
        "RPC governance policy resolved"
    );
    let key = OperationKey::new(&service, GovernanceBoundary::Rpc, &method).to_string();
    let breaker_permit = match policy.breaker {
        Some(_) => match method_breaker_allow(&key) {
            BreakerDecision::Allow(permit) => {
                record_resilience_decision(
                    service.as_str(),
                    "rpc",
                    "breaker",
                    if permit == BreakerPermit::Probe {
                        "half_open_probe"
                    } else {
                        "allowed"
                    },
                );
                Some(permit)
            }
            BreakerDecision::Reject => {
                record_resilience_decision(service.as_str(), "rpc", "breaker", "open");
                return Err(Status::unavailable("circuit open"));
            }
        },
        None => None,
    };
    if let Some(config) = &policy.shedding {
        match enforce_method_shedding(&key, config) {
            Ok(()) => {
                record_resilience_decision(service.as_str(), "rpc", "load_shedding", "allowed")
            }
            Err(status) => {
                record_resilience_decision(service.as_str(), "rpc", "load_shedding", "shed");
                return Err(status);
            }
        }
    }
    let request_ctx = match policy.timeout {
        Some(timeout) => request_ctx.with_timeout(timeout),
        None => request_ctx,
    };
    let request_id = request_ctx.request_id();
    let trace_id = request_ctx.trace_id();
    tracing::info!(
        protocol = "rpc",
        service = %service,
        method = %method,
        request_id = %request_id,
        trace_id = %trace_id,
        "RPC method started"
    );
    Ok((
        request_ctx,
        MethodGuard {
            key,
            service,
            method,
            request_id,
            trace_id,
            started_at: Instant::now(),
            breaker: policy.breaker,
            breaker_permit,
            shedding: policy.shedding,
            finished: false,
        },
    ))
}

pub fn finish_method(mut guard: MethodGuard, code: impl Into<String>) {
    let code = code.into();
    let success = code == "ok";
    let elapsed = guard.started_at.elapsed();
    if let (Some(config), Some(permit)) = (guard.breaker, guard.breaker_permit) {
        record_resilience_decision(
            guard.service.as_str(),
            "rpc",
            "breaker",
            if success { "success" } else { "failure" },
        );
        method_breaker_record(&guard.key, permit, success, &config);
    }
    if let Some(config) = guard.shedding {
        method_shedding_record(&guard.key, success, elapsed, &config);
    }
    guard.finished = true;
    tracing::info!(
        protocol = "rpc",
        service = %guard.service,
        method = %guard.method,
        code = %code,
        success,
        elapsed_ms = elapsed.as_millis(),
        request_id = %guard.request_id,
        trace_id = %guard.trace_id,
        "RPC method completed"
    );
    record_rpc_method(guard.service.as_str(), guard.method.as_str(), code, elapsed);
}

pub async fn retry_status<F, Fut, T>(
    service: &str,
    context: &Context,
    mut call: F,
    options: RpcClientOptions,
) -> Result<T, Status>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, Status>>,
{
    retry_status_with_policy(
        service,
        "default",
        context,
        &mut call,
        MethodRetryPolicy::from_options(options),
    )
    .await
}

pub async fn retry_status_for_method<F, Fut, T>(
    service: &str,
    context: &Context,
    mut call: F,
    options: RpcClientOptions,
    governance: Option<&roze_config::GovernanceConfig>,
    method: &str,
) -> Result<T, Status>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, Status>>,
{
    retry_status_with_policy(
        service,
        method,
        context,
        &mut call,
        retry_policy_for_method(options, governance, method),
    )
    .await
}

async fn retry_status_with_policy<F, Fut, T>(
    service: &str,
    method: &str,
    context: &Context,
    call: &mut F,
    policy: MethodRetryPolicy,
) -> Result<T, Status>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, Status>>,
{
    let budget_key = OperationKey::new(service, GovernanceBoundary::Rpc, method).to_string();
    context.ensure_retry_budget(policy.max_retries);
    tracing::debug!(
        protocol = "rpc",
        service,
        method,
        max_retries = policy.max_retries,
        backoff_ms = policy.backoff.as_millis(),
        max_backoff_ms = policy.max_backoff.as_millis(),
        budget_percent = policy.budget_percent,
        request_retry_budget_remaining = context.retry_budget_remaining(),
        remaining_deadline_ms = context.remaining_timeout().map(|value| value.as_millis()),
        "RPC retry policy resolved"
    );
    CLIENT_RETRY_BUDGETS
        .get_or_init(RetryBudgetRegistry::default)
        .record_call(&budget_key);
    let mut attempt = 0usize;
    loop {
        tracing::debug!(
            protocol = "rpc",
            service,
            method,
            attempt = attempt + 1,
            "RPC attempt starting"
        );
        let response = call().await;
        match response {
            Ok(value) => {
                tracing::debug!(
                    protocol = "rpc",
                    service,
                    method,
                    attempt = attempt + 1,
                    outcome = "success",
                    "RPC attempt completed"
                );
                return Ok(value);
            }
            Err(status) if attempt < policy.max_retries && should_retry_status(&status) => {
                tracing::debug!(protocol = "rpc", service, method, attempt = attempt + 1, code = ?status.code(), outcome = "retryable", "RPC attempt completed");
                if !CLIENT_RETRY_BUDGETS
                    .get_or_init(RetryBudgetRegistry::default)
                    .allow_retry(&budget_key, policy.budget_percent)
                {
                    tracing::debug!(
                        protocol = "rpc",
                        service,
                        method,
                        decision = "budget_exhausted",
                        "RPC retry stopped"
                    );
                    record_resilience_decision(service, "rpc", "retry", "budget_exhausted");
                    return Err(status);
                }
                let next_attempt = attempt + 1;
                let delay = full_jitter_delay(policy.backoff, policy.max_backoff, next_attempt);
                tracing::debug!(
                    protocol = "rpc",
                    service,
                    method,
                    next_attempt = next_attempt + 1,
                    delay_ms = delay.as_millis(),
                    remaining_deadline_ms =
                        context.remaining_timeout().map(|value| value.as_millis()),
                    "RPC retry scheduled"
                );
                if let Some(status) = retry_context_status(context, delay) {
                    tracing::debug!(protocol = "rpc", service, method, decision = ?status.code(), "RPC retry stopped by context");
                    record_resilience_decision(
                        service,
                        "rpc",
                        "retry",
                        if status.code() == Code::Cancelled {
                            "cancelled"
                        } else {
                            "deadline_exhausted"
                        },
                    );
                    return Err(status);
                }
                if !delay.is_zero() {
                    sleep(delay).await;
                }
                if let Some(status) = retry_context_status(context, Duration::ZERO) {
                    tracing::debug!(protocol = "rpc", service, method, decision = ?status.code(), "RPC retry stopped after backoff");
                    record_resilience_decision(
                        service,
                        "rpc",
                        "retry",
                        if status.code() == Code::Cancelled {
                            "cancelled"
                        } else {
                            "deadline_exhausted"
                        },
                    );
                    return Err(status);
                }
                if !context.try_consume_retry_budget() {
                    tracing::debug!(
                        protocol = "rpc",
                        service,
                        method,
                        decision = "request_budget_exhausted",
                        "RPC retry stopped by propagated request budget"
                    );
                    record_resilience_decision(service, "rpc", "retry", "request_budget_exhausted");
                    return Err(status);
                }
                attempt = next_attempt;
                record_resilience_decision(service, "rpc", "retry", "attempt");
            }
            Err(status) => {
                tracing::debug!(protocol = "rpc", service, method, attempt = attempt + 1, code = ?status.code(), outcome = "failed", "RPC attempt completed without retry");
                return Err(status);
            }
        }
    }
}

fn retry_context_status(context: &Context, delay: Duration) -> Option<Status> {
    if context.cancelled() {
        return Some(Status::cancelled("RPC retry cancelled"));
    }
    if context.deadline().is_some()
        && context
            .remaining_timeout()
            .is_none_or(|remaining| remaining <= delay)
    {
        return Some(Status::deadline_exceeded(
            "RPC retry backoff exceeds remaining deadline",
        ));
    }
    None
}

#[allow(clippy::result_large_err)]
pub async fn enforce_method_rate_limit<T>(
    limiter: &roze_rate_limit::RateLimiter,
    service: &str,
    method: &str,
    request: &Request<T>,
    request_ctx: &Context,
    governance: Option<&roze_config::GovernanceConfig>,
) -> Result<(), Status> {
    let Some(config) = method_policy(governance, method).rate_limit else {
        return Ok(());
    };
    let identity = roze_rate_limit::RateLimitIdentity::new(service, "rpc", method)
        .with_subject(request_ctx.subject())
        .with_tenant(request_ctx.tenant())
        .with_headers(request.metadata().iter().filter_map(|entry| match entry {
            roze_grpc::transport::KeyAndValueRef::Ascii(name, value) => {
                Some((name.as_str(), value.to_str().ok()?.to_string()))
            }
            roze_grpc::transport::KeyAndValueRef::Binary(_, _) => None,
        }));
    match limiter
        .check(
            &config.key,
            &identity,
            roze_rate_limit::RateLimit {
                burst: config.burst,
                refill: config.refill,
                tokens_per_refill: config.tokens_per_refill,
            },
        )
        .await
    {
        Ok(decision) if decision.allowed => {
            record_resilience_decision(
                service,
                "rpc",
                "rate_limit",
                if decision.degraded {
                    "store_error_fail_open"
                } else {
                    "allowed"
                },
            );
            Ok(())
        }
        Ok(decision) => {
            record_resilience_decision(service, "rpc", "rate_limit", "rejected");
            Err(status_from_error(
                RozeError::rate_limited(decision.retry_after),
                request_ctx,
            ))
        }
        Err(roze_rate_limit::RateLimitError::StoreUnavailable) => {
            record_resilience_decision(service, "rpc", "rate_limit", "store_error_fail_closed");
            Err(status_from_error(
                RozeError::Unavailable("rate limit store unavailable".to_string()),
                request_ctx,
            ))
        }
        Err(_) => {
            record_resilience_decision(service, "rpc", "rate_limit", "identity_rejected");
            Err(status_from_error(
                RozeError::rate_limited(Duration::from_secs(1)),
                request_ctx,
            ))
        }
    }
}

fn method_breaker_allow(key: &str) -> BreakerDecision {
    METHOD_BREAKERS.get_or_init(BreakerRegistry::new).allow(key)
}

fn method_breaker_record(
    key: &str,
    permit: BreakerPermit,
    success: bool,
    config: &MethodBreakerConfig,
) {
    let registry = METHOD_BREAKERS.get_or_init(BreakerRegistry::new);
    if success {
        registry.record_success(key, permit);
        return;
    }
    registry.record_failure(
        key,
        permit,
        roze_resilience::BreakerConfig {
            failure_threshold: config.failure_threshold,
            reset_timeout: config.reset_timeout,
        },
    );
}

fn method_breaker_cancel(key: &str, permit: BreakerPermit, config: &MethodBreakerConfig) {
    METHOD_BREAKERS.get_or_init(BreakerRegistry::new).cancel(
        key,
        permit,
        roze_resilience::BreakerConfig {
            failure_threshold: config.failure_threshold,
            reset_timeout: config.reset_timeout,
        },
    );
}

#[allow(clippy::result_large_err)]
fn enforce_method_shedding(key: &str, config: &MethodSheddingConfig) -> Result<(), Status> {
    if METHOD_SHEDDERS
        .get_or_init(SheddingRegistry::new)
        .allow(key, method_shedding_config(*config))
    {
        Ok(())
    } else {
        Err(Status::unavailable("load shed"))
    }
}

fn method_shedding_record(
    key: &str,
    success: bool,
    elapsed: Duration,
    config: &MethodSheddingConfig,
) {
    METHOD_SHEDDERS.get_or_init(SheddingRegistry::new).record(
        key,
        success,
        elapsed,
        method_shedding_config(*config),
    );
}

fn method_shedding_release(key: &str) {
    METHOD_SHEDDERS
        .get_or_init(SheddingRegistry::new)
        .release(key);
}

fn method_shedding_config(config: MethodSheddingConfig) -> roze_resilience::SheddingConfig {
    roze_resilience::SheddingConfig {
        concurrency: config.concurrency,
        window: config.window,
        min_samples: config.min_samples,
        max_avg_latency: config.max_avg_latency,
        max_failure_ratio_per_mille: config.max_failure_ratio_per_mille,
        cool_down: config.cool_down,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    use super::*;

    #[test]
    fn debug_endpoint_label_redacts_credentials_and_url_details() {
        assert_eq!(
            safe_endpoint_label("https://user:secret@example.com:443/private?token=secret#part"),
            "https://example.com:443"
        );
        assert_eq!(
            safe_endpoint_label("user:secret@example.com:50051/private"),
            "example.com:50051"
        );
    }

    #[test]
    fn retry_status_targets_transient_errors() {
        assert!(should_retry_status(&Status::unavailable("down")));
        assert!(should_retry_status(&Status::deadline_exceeded("slow")));
        assert!(should_retry_status(&Status::new(Code::Unknown, "unknown")));
        assert!(!should_retry_status(&Status::invalid_argument(
            "bad request"
        )));
    }

    #[test]
    fn status_from_error_maps_all_roze_error_variants() {
        let context = Context::background_with_request_id_and_trace_id("request-1", "trace-1");
        let cases = [
            (
                RozeError::BadRequest("bad".into()),
                Code::InvalidArgument,
                400,
                "bad_request",
            ),
            (
                RozeError::Conflict("already exists".into()),
                Code::AlreadyExists,
                409,
                "conflict",
            ),
            (
                RozeError::FailedPrecondition("stale version".into()),
                Code::FailedPrecondition,
                412,
                "failed_precondition",
            ),
            (
                RozeError::Unauthorized,
                Code::Unauthenticated,
                401,
                "unauthorized",
            ),
            (
                RozeError::Forbidden,
                Code::PermissionDenied,
                403,
                "forbidden",
            ),
            (
                RozeError::RateLimited {
                    retry_after_seconds: 2,
                },
                Code::ResourceExhausted,
                429,
                "rate_limited",
            ),
            (
                RozeError::NotFound("missing".into()),
                Code::NotFound,
                404,
                "not_found",
            ),
            (
                RozeError::Unavailable("down".into()),
                Code::Unavailable,
                503,
                "unavailable",
            ),
            (
                RozeError::Internal("boom".into()),
                Code::Internal,
                500,
                "internal",
            ),
            (
                RozeError::fallback_response(
                    598,
                    Some(serde_json::json!({"message": "degraded"})),
                    Default::default(),
                ),
                Code::Unavailable,
                598,
                "fallback",
            ),
        ];

        for (error, expected_code, expected_error_code, expected_kind) in cases {
            let status = status_from_error(error, &context);
            let expected_error_code = expected_error_code.to_string();

            assert_eq!(status.code(), expected_code);
            assert_eq!(
                status
                    .metadata()
                    .get(ERROR_CODE_METADATA)
                    .and_then(|value| value.to_str().ok()),
                Some(expected_error_code.as_str())
            );
            assert_eq!(
                status
                    .metadata()
                    .get(ERROR_KIND_METADATA)
                    .and_then(|value| value.to_str().ok()),
                Some(expected_kind)
            );
        }
    }

    #[test]
    fn rate_limited_status_exports_retry_after_metadata() {
        let context = Context::background_with_request_id_and_trace_id("request-1", "trace-1");
        let status = status_from_error(
            RozeError::RateLimited {
                retry_after_seconds: 3,
            },
            &context,
        );

        assert_eq!(status.code(), Code::ResourceExhausted);
        assert_eq!(
            status
                .metadata()
                .get(RETRY_AFTER_METADATA)
                .and_then(|value| value.to_str().ok()),
            Some("3")
        );
    }

    #[test]
    fn status_from_fallback_error_exports_metadata() {
        let context = Context::background_with_request_id_and_trace_id("request-1", "trace-1");
        let mut headers = BTreeMap::new();
        headers.insert("x-roze-fallback".to_string(), "method".to_string());
        let status = status_from_error(
            RozeError::fallback_response(
                598,
                Some(serde_json::json!({"message": "degraded"})),
                headers,
            ),
            &context,
        );

        assert_eq!(status.code(), Code::Unavailable);
        assert_eq!(
            status
                .metadata()
                .get(FALLBACK_STATUS_METADATA)
                .and_then(|value| value.to_str().ok()),
            Some("598")
        );
        assert_eq!(
            status
                .metadata()
                .get(FALLBACK_BODY_METADATA)
                .and_then(|value| value.to_str().ok()),
            Some(r#"{"message":"degraded"}"#)
        );
        assert_eq!(
            status
                .metadata()
                .get(FALLBACK_HEADERS_METADATA)
                .and_then(|value| value.to_str().ok()),
            Some(r#"{"x-roze-fallback":"method"}"#)
        );
    }

    #[test]
    fn conflict_fallback_maps_to_already_exists() {
        let context = Context::background_with_request_id_and_trace_id("request-1", "trace-1");
        let mut headers = BTreeMap::new();
        headers.insert(
            "x-roze-error-code".to_string(),
            "IDEMPOTENCY_KEY_REUSED".to_string(),
        );
        let status = status_from_error(
            RozeError::fallback_response(
                409,
                Some(serde_json::json!({
                    "code": "IDEMPOTENCY_KEY_REUSED",
                    "message": "idempotency key was reused"
                })),
                headers,
            ),
            &context,
        );

        assert_eq!(status.code(), Code::AlreadyExists);
        assert_eq!(
            status
                .metadata()
                .get(FALLBACK_STATUS_METADATA)
                .and_then(|value| value.to_str().ok()),
            Some("409")
        );
    }

    #[test]
    fn error_from_status_restores_roze_error_variants_with_grpc_codes() {
        assert_eq!(
            error_from_status(&Status::resource_exhausted("slow down")),
            RozeError::RateLimited {
                retry_after_seconds: 1
            }
        );
        assert_eq!(
            error_from_status(&Status::unavailable("down")),
            RozeError::Unavailable("down".into())
        );
        assert_eq!(
            error_from_status(&Status::already_exists("already bound")),
            RozeError::Conflict("already bound".into())
        );
        assert_eq!(
            error_from_status(&Status::failed_precondition("stale version")),
            RozeError::FailedPrecondition("stale version".into())
        );
    }

    #[test]
    fn semantic_errors_round_trip_through_grpc_metadata() {
        let context = Context::background_with_request_id_and_trace_id("request-1", "trace-1");
        for error in [
            RozeError::Conflict("already bound".into()),
            RozeError::FailedPrecondition("stale version".into()),
        ] {
            let status = status_from_error(error.clone(), &context);
            assert_eq!(error_from_status(&status), error);
        }
    }

    #[test]
    fn error_from_status_restores_fallback_metadata() {
        let context = Context::background_with_request_id_and_trace_id("request-1", "trace-1");
        let mut headers = BTreeMap::new();
        headers.insert("x-roze-fallback".to_string(), "method".to_string());
        let status = status_from_error(
            RozeError::fallback_response(
                598,
                Some(serde_json::json!({"message": "degraded"})),
                headers,
            ),
            &context,
        );

        let error = error_from_status(&status);

        assert!(matches!(error, RozeError::Fallback { status: 598, .. }));
        assert_eq!(
            error.fallback_body(),
            Some(&serde_json::json!({"message": "degraded"}))
        );
        assert_eq!(
            error
                .fallback_headers()
                .and_then(|headers| headers.get("x-roze-fallback"))
                .map(String::as_str),
            Some("method")
        );
    }

    #[tokio::test]
    async fn retry_status_retries_once() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let attempts_clone = attempts.clone();
        let context = Context::background();
        let result = retry_status(
            "catalog",
            &context,
            move || {
                let attempts = attempts_clone.clone();
                async move {
                    let current = attempts.fetch_add(1, Ordering::SeqCst);
                    if current == 0 {
                        Err(Status::unavailable("temporary"))
                    } else {
                        Ok("ok")
                    }
                }
            },
            RpcClientOptions {
                connect_timeout: Duration::from_secs(1),
                request_timeout: Duration::from_secs(1),
                max_retries: 1,
                retry_backoff: Duration::from_millis(0),
                ..RpcClientOptions::default()
            },
        )
        .await
        .expect("retry should succeed");

        assert_eq!(result, "ok");
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn retry_policy_prefers_method_override() {
        let mut governance = roze_config::GovernanceConfig {
            retry: Some(roze_config::RetryConfig {
                max_attempts: 4,
                backoff_ms: 100,
                max_backoff_ms: 1_000,
                budget_percent: Some(20),
            }),
            ..Default::default()
        };
        governance.routes.insert(
            "GetUser".to_string(),
            roze_config::RouteGovernanceConfig {
                retry: Some(roze_config::RetryConfig {
                    max_attempts: 2,
                    backoff_ms: 10,
                    max_backoff_ms: 50,
                    budget_percent: Some(10),
                }),
                ..Default::default()
            },
        );

        let policy =
            retry_policy_for_method(RpcClientOptions::default(), Some(&governance), "GetUser");

        assert_eq!(policy.max_retries, 1);
        assert_eq!(policy.backoff, Duration::from_millis(10));
        assert_eq!(policy.max_backoff, Duration::from_millis(50));
        assert_eq!(policy.budget_percent, Some(10));
    }

    #[tokio::test]
    async fn retry_status_for_method_honors_retry_budget() {
        let method = format!("RetryBudget{}", std::process::id());
        let mut governance = roze_config::GovernanceConfig::default();
        governance.routes.insert(
            method.clone(),
            roze_config::RouteGovernanceConfig {
                retry: Some(roze_config::RetryConfig {
                    max_attempts: 3,
                    backoff_ms: 0,
                    max_backoff_ms: 0,
                    budget_percent: Some(1),
                }),
                ..Default::default()
            },
        );
        let attempts = Arc::new(AtomicUsize::new(0));
        let attempts_clone = attempts.clone();
        let context = Context::background();

        let err = retry_status_for_method(
            "catalog",
            &context,
            move || {
                let attempts = attempts_clone.clone();
                async move {
                    attempts.fetch_add(1, Ordering::SeqCst);
                    Err::<(), _>(Status::unavailable("temporary"))
                }
            },
            RpcClientOptions::default(),
            Some(&governance),
            &method,
        )
        .await
        .expect_err("retry budget should stop retries");

        assert_eq!(err.code(), Code::Unavailable);
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn propagated_request_budget_caps_real_retry_attempts() {
        let method = format!("RequestRetryBudget{}", std::process::id());
        let mut governance = roze_config::GovernanceConfig::default();
        governance.routes.insert(
            method.clone(),
            roze_config::RouteGovernanceConfig {
                retry: Some(roze_config::RetryConfig {
                    max_attempts: 4,
                    backoff_ms: 0,
                    max_backoff_ms: 0,
                    budget_percent: None,
                }),
                ..Default::default()
            },
        );
        let attempts = Arc::new(AtomicUsize::new(0));
        let attempts_clone = Arc::clone(&attempts);
        let context = Context::background().with_retry_budget(1);

        let error = retry_status_for_method(
            "catalog",
            &context,
            move || {
                let attempts = Arc::clone(&attempts_clone);
                async move {
                    attempts.fetch_add(1, Ordering::SeqCst);
                    Err::<(), _>(Status::unavailable("temporary"))
                }
            },
            RpcClientOptions::default(),
            Some(&governance),
            &method,
        )
        .await
        .expect_err("request budget should stop the second retry");

        assert_eq!(error.code(), Code::Unavailable);
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
        assert_eq!(context.retry_budget_remaining(), Some(0));
    }

    #[tokio::test]
    async fn retry_status_stops_when_backoff_exceeds_deadline() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let attempts_clone = attempts.clone();
        let context = Context::background().with_timeout(Duration::from_nanos(1));

        let err = retry_status(
            "catalog",
            &context,
            move || {
                let attempts = attempts_clone.clone();
                async move {
                    attempts.fetch_add(1, Ordering::SeqCst);
                    Err::<(), _>(Status::unavailable("temporary"))
                }
            },
            RpcClientOptions {
                max_retries: 2,
                retry_backoff: Duration::from_secs(1),
                retry_max_backoff: Duration::from_secs(1),
                ..RpcClientOptions::default()
            },
        )
        .await
        .expect_err("expired deadline must stop retries");

        assert_eq!(err.code(), Code::DeadlineExceeded);
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn retry_status_stops_when_context_is_cancelled() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let attempts_clone = attempts.clone();
        let context = Context::background();
        context.cancel();

        let err = retry_status(
            "catalog",
            &context,
            move || {
                let attempts = attempts_clone.clone();
                async move {
                    attempts.fetch_add(1, Ordering::SeqCst);
                    Err::<(), _>(Status::unavailable("temporary"))
                }
            },
            RpcClientOptions {
                max_retries: 2,
                retry_backoff: Duration::ZERO,
                ..RpcClientOptions::default()
            },
        )
        .await
        .expect_err("cancelled context must stop retries");

        assert_eq!(err.code(), Code::Cancelled);
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn request_context_restores_standard_metadata() {
        let mut request = Request::new(());
        insert_metadata(
            request.metadata_mut(),
            roze_context::REQUEST_ID_HEADER,
            "request-abc",
        );
        insert_metadata(
            request.metadata_mut(),
            roze_context::TRACE_ID_HEADER,
            "trace-abc",
        );
        insert_metadata(
            request.metadata_mut(),
            roze_context::SUBJECT_HEADER,
            "user-1",
        );
        insert_metadata(
            request.metadata_mut(),
            roze_context::TENANT_HEADER,
            "tenant-1",
        );
        insert_metadata(
            request.metadata_mut(),
            roze_context::ROLES_HEADER,
            "admin,ops",
        );
        insert_metadata(request.metadata_mut(), "x-roze-meta-locale", "zh-CN");

        let context = request_context(&request);
        assert_eq!(context.request_id(), "request-abc");
        assert_eq!(context.trace_id(), "trace-abc");
        assert_eq!(context.subject().as_deref(), Some("user-1"));
        assert_eq!(context.tenant().as_deref(), Some("tenant-1"));
        assert_eq!(context.roles(), vec!["admin", "ops"]);
        assert_eq!(context.metadata_value("locale").as_deref(), Some("zh-CN"));
    }

    #[test]
    fn apply_request_context_sets_standard_metadata() {
        let mut request = Request::new(());
        let context = Context::background_with_request_id_and_trace_id("request-xyz", "trace-xyz")
            .with_auth(AuthContext {
                subject: "user-1".to_string(),
                roles: vec!["admin".to_string()],
                tenant: Some("tenant-1".to_string()),
            })
            .with_metadata("locale", "zh-CN");

        apply_request_context(&mut request, &context);

        let request_id = request
            .metadata()
            .get(roze_context::REQUEST_ID_HEADER)
            .and_then(|value| value.to_str().ok())
            .expect("request id metadata");
        let trace_id = request
            .metadata()
            .get(roze_context::TRACE_ID_HEADER)
            .and_then(|value| value.to_str().ok())
            .expect("trace id metadata");
        assert_eq!(request_id, "request-xyz");
        assert_eq!(trace_id, "trace-xyz");
        assert_eq!(
            request
                .metadata()
                .get(roze_context::SUBJECT_HEADER)
                .and_then(|value| value.to_str().ok()),
            Some("user-1")
        );
        assert_eq!(
            request
                .metadata()
                .get("x-roze-meta-locale")
                .and_then(|value| value.to_str().ok()),
            Some("zh-CN")
        );
    }

    #[test]
    fn timeout_metadata_preserves_live_submillisecond_deadline() {
        assert_eq!(timeout_metadata_millis(Duration::from_nanos(1)), "1");
        assert_eq!(timeout_metadata_millis(Duration::from_micros(999)), "1");
        assert_eq!(timeout_metadata_millis(Duration::from_millis(7)), "7");
    }

    #[test]
    fn client_request_round_trips_idempotency_key_across_rpc_boundary() {
        let context = Context::background_with_request_id_and_trace_id("request-1", "trace-1")
            .with_idempotency_key("order-42")
            .with_locale("zh-CN")
            .with_retry_budget(3);

        let request = client_request((), &context, RpcClientOptions::default(), None);
        assert_eq!(
            request
                .metadata()
                .get(roze_context::IDEMPOTENCY_KEY_HEADER)
                .and_then(|value| value.to_str().ok()),
            Some("order-42")
        );

        let restored = request_context(&request);
        assert_eq!(restored.idempotency_key().as_deref(), Some("order-42"));
        assert_eq!(restored.locale().as_deref(), Some("zh-CN"));
        assert_eq!(restored.retry_budget_remaining(), Some(3));
    }

    #[test]
    fn response_metadata_tightens_parent_retry_budget_on_success_and_error() {
        let parent = Context::background().with_retry_budget(4);
        let downstream = Context::background().with_retry_budget(2);
        assert!(downstream.try_consume_retry_budget());

        let success = Ok::<_, Status>(response_with_context((), &downstream));
        reconcile_retry_budget(&parent, &success);
        assert_eq!(parent.retry_budget_remaining(), Some(1));

        let error = Err::<Response<()>, _>(status_from_error(
            RozeError::Unavailable("temporary".to_string()),
            &Context::background().with_retry_budget(0),
        ));
        reconcile_retry_budget(&parent, &error);
        assert_eq!(parent.retry_budget_remaining(), Some(0));
    }

    #[test]
    fn delegated_retry_budget_is_conserved_across_concurrent_fanout() {
        let parent = Context::background().with_retry_budget(8);
        let first = delegate_retry_budget(&parent);
        let second = delegate_retry_budget(&parent);
        let third = delegate_retry_budget(&parent);

        assert_eq!(first.allocated(), 4);
        assert_eq!(second.allocated(), 2);
        assert_eq!(third.allocated(), 1);
        assert_eq!(parent.retry_budget_remaining(), Some(1));

        let first_result = Ok::<_, Status>(response_with_context((), first.context()));
        reconcile_delegated_retry_budget(&parent, first, &first_result);
        let second_result = Err::<Response<()>, _>(status_from_error(
            RozeError::Unavailable("temporary".to_string()),
            second.context(),
        ));
        reconcile_delegated_retry_budget(&parent, second, &second_result);
        let missing_response = Err::<Response<()>, _>(Status::unavailable("connection lost"));
        reconcile_delegated_retry_budget(&parent, third, &missing_response);

        assert_eq!(
            parent.retry_budget_remaining(),
            Some(7),
            "only explicitly returned credits may re-enter the parent pool"
        );
    }

    #[test]
    fn downstream_cannot_return_more_retry_budget_than_was_delegated() {
        let parent = Context::background().with_retry_budget(4);
        let delegation = delegate_retry_budget(&parent);
        let forged = Ok::<_, Status>(response_with_context(
            (),
            &Context::background().with_retry_budget(usize::MAX),
        ));

        reconcile_delegated_retry_budget(&parent, delegation, &forged);

        assert_eq!(parent.retry_budget_remaining(), Some(4));
    }

    #[test]
    fn rpc_client_options_follow_config_defaults() {
        let config = roze_config::RpcClientConfig {
            etcd: None,
            endpoints: vec!["127.0.0.1:4000".to_string()],
            target: None,
            app: None,
            token: None,
            non_block: true,
            timeout_ms: 2_500,
            keepalive_time_secs: 30,
            balancer: Default::default(),
            middlewares: roze_config::RpcClientMiddlewaresConfig {
                trace: false,
                ..Default::default()
            },
        };

        let options = RpcClientOptions::from_config(&config);
        assert_eq!(options.request_timeout, Duration::from_millis(2_500));
        assert_eq!(options.keepalive_time, Duration::from_secs(30));
        assert!(options.non_block);
        assert!(!options.trace);
        assert_eq!(
            rpc_client_balancer_kind(&config),
            BalancerKind::PowerOfTwoChoices
        );
        assert_eq!(
            rpc_client_target(&config).expect("target"),
            "127.0.0.1:4000"
        );
    }

    #[test]
    fn dynamic_rpc_channels_enable_per_attempt_default_p2c() {
        let mut config = roze_config::RpcClientConfig {
            etcd: None,
            endpoints: vec!["127.0.0.1:4000".to_string(), "127.0.0.1:4001".to_string()],
            target: None,
            app: None,
            token: None,
            non_block: false,
            timeout_ms: 2_000,
            keepalive_time_secs: 20,
            balancer: roze_config::RpcClientBalancerKind::PowerOfTwoChoices,
            middlewares: Default::default(),
        };
        let dynamic = DynamicRpcChannels::from_config(&config)
            .expect("dynamic config")
            .expect("default P2C should be per-attempt");
        assert_eq!(dynamic.channel_count(), 0);

        config.balancer = roze_config::RpcClientBalancerKind::RoundRobin;
        assert!(DynamicRpcChannels::from_config(&config)
            .expect("round-robin config")
            .is_none());
    }

    #[tokio::test]
    async fn dynamic_rpc_channels_apply_watch_remove_and_readd() {
        #[derive(Clone)]
        struct WatchRegistry {
            initial: Vec<ServiceInstance>,
            receiver:
                Arc<Mutex<Option<tokio::sync::mpsc::UnboundedReceiver<Vec<ServiceInstance>>>>>,
        }

        #[async_trait::async_trait]
        impl Registry for WatchRegistry {
            async fn register(&self, _instance: ServiceInstance) -> anyhow::Result<()> {
                Ok(())
            }

            async fn deregister(&self, _name: &str, _addr: &str) -> anyhow::Result<()> {
                Ok(())
            }

            async fn discover(&self, _name: &str) -> anyhow::Result<Vec<ServiceInstance>> {
                Ok(self.initial.clone())
            }

            fn supports_watch(&self) -> bool {
                true
            }

            fn watch(&self, _name: &str) -> crate::registry::RegistryWatchFuture<'_> {
                Box::pin(async move {
                    self.receiver
                        .lock()
                        .expect("watch receiver poisoned")
                        .take()
                        .ok_or_else(|| anyhow::anyhow!("watch already started"))
                })
            }
        }

        let first = ServiceInstance::new("catalog", "127.0.0.1:4000");
        let second = ServiceInstance::new("catalog", "127.0.0.1:4001");
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        let registry = WatchRegistry {
            initial: vec![first.clone()],
            receiver: Arc::new(Mutex::new(Some(receiver))),
        };
        let dynamic = DynamicRpcChannels {
            source: DynamicDiscoverySource::Registry(Arc::new(registry)),
            options: RpcClientOptions::default(),
            balancer: EwmaP2cBalancer::default(),
            channels: Arc::new(DashMap::new()),
            cache: Arc::new(Mutex::new(None)),
            cache_ttl: Duration::from_secs(60),
            watch_started: Arc::new(AtomicBool::new(false)),
        };

        assert_eq!(
            dynamic.instances("catalog").await.expect("discover"),
            vec![first.clone()]
        );
        sender.send(vec![second.clone()]).expect("watch update");
        tokio::task::yield_now().await;
        assert_eq!(
            dynamic.instances("catalog").await.expect("watch replace"),
            vec![second.clone()]
        );

        sender.send(Vec::new()).expect("watch remove");
        tokio::task::yield_now().await;
        assert!(dynamic
            .instances("catalog")
            .await
            .expect("watch remove")
            .is_empty());

        sender.send(vec![first.clone()]).expect("watch re-add");
        tokio::task::yield_now().await;
        assert_eq!(
            dynamic.instances("catalog").await.expect("watch re-add"),
            vec![first]
        );
    }

    #[test]
    fn finish_attempt_status_settles_timeout_once() {
        let balancer = EwmaP2cBalancer::default();
        let instance = ServiceInstance::new("catalog", "127.0.0.1:4000");
        let lease = balancer
            .pick_tracked(std::slice::from_ref(&instance))
            .expect("lease");
        let result = Err::<(), _>(Status::deadline_exceeded("slow"));
        finish_attempt_status(lease, &result);
        let snapshot = balancer.snapshot(&instance).expect("snapshot");
        assert_eq!(snapshot.inflight, 0);
        assert_eq!(snapshot.success_per_mille, 0);
    }

    #[tokio::test]
    async fn aborting_inflight_attempt_releases_lease() {
        let balancer = EwmaP2cBalancer::default();
        let instance = ServiceInstance::new("catalog", "127.0.0.1:4000");
        let task_balancer = balancer.clone();
        let task_instance = instance.clone();
        let (acquired_tx, acquired_rx) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            let _lease = task_balancer
                .pick_tracked(std::slice::from_ref(&task_instance))
                .expect("lease");
            let _ = acquired_tx.send(());
            std::future::pending::<()>().await;
        });

        acquired_rx.await.expect("attempt acquired");
        assert_eq!(
            balancer
                .snapshot(&instance)
                .expect("active snapshot")
                .inflight,
            1
        );

        task.abort();
        let error = task.await.expect_err("task should be cancelled");
        assert!(error.is_cancelled());
        assert_eq!(
            balancer
                .snapshot(&instance)
                .expect("released snapshot")
                .inflight,
            0
        );
    }

    #[tokio::test]
    async fn panicking_inflight_attempt_releases_lease() {
        let balancer = EwmaP2cBalancer::default();
        let instance = ServiceInstance::new("catalog", "127.0.0.1:4000");
        let task_balancer = balancer.clone();
        let task_instance = instance.clone();
        let task = tokio::spawn(async move {
            let _lease = task_balancer
                .pick_tracked(std::slice::from_ref(&task_instance))
                .expect("lease");
            panic!("simulated RPC attempt panic");
        });

        let error = task.await.expect_err("task should panic");

        assert!(error.is_panic());
        assert_eq!(
            balancer
                .snapshot(&instance)
                .expect("released snapshot")
                .inflight,
            0
        );
    }

    #[test]
    fn rpc_client_balancer_kind_follows_config() {
        let config = roze_config::RpcClientConfig {
            etcd: None,
            endpoints: vec!["127.0.0.1:4000".to_string()],
            target: None,
            app: None,
            token: None,
            non_block: false,
            timeout_ms: 2_000,
            keepalive_time_secs: 20,
            balancer: roze_config::RpcClientBalancerKind::HealthAware,
            middlewares: Default::default(),
        };

        assert_eq!(rpc_client_balancer_kind(&config), BalancerKind::HealthAware);
    }

    #[test]
    fn rpc_client_static_endpoints_round_robin() {
        let config = roze_config::RpcClientConfig {
            etcd: None,
            endpoints: vec!["127.0.0.1:4000".to_string(), "127.0.0.1:4001".to_string()],
            target: None,
            app: None,
            token: None,
            non_block: false,
            timeout_ms: 2_000,
            keepalive_time_secs: 20,
            balancer: roze_config::RpcClientBalancerKind::RoundRobin,
            middlewares: Default::default(),
        };
        let cursor = AtomicUsize::new(0);

        assert!(rpc_client_has_static_endpoints(&config));
        assert_eq!(
            rpc_client_pick_static_endpoint(&config, &cursor).expect("endpoint"),
            "127.0.0.1:4000"
        );
        assert_eq!(
            rpc_client_pick_static_endpoint(&config, &cursor).expect("endpoint"),
            "127.0.0.1:4001"
        );
        assert_eq!(
            rpc_client_pick_static_endpoint(&config, &cursor).expect("endpoint"),
            "127.0.0.1:4000"
        );
    }

    #[test]
    fn rpc_client_static_endpoints_use_configured_balancer() {
        let config = roze_config::RpcClientConfig {
            etcd: None,
            endpoints: vec!["127.0.0.1:4000".to_string(), "127.0.0.1:4001".to_string()],
            target: None,
            app: None,
            token: None,
            non_block: false,
            timeout_ms: 2_000,
            keepalive_time_secs: 20,
            balancer: roze_config::RpcClientBalancerKind::FirstAvailable,
            middlewares: Default::default(),
        };

        assert_eq!(
            rpc_client_pick_static_endpoint(&config, &AtomicUsize::new(0)).expect("endpoint"),
            "127.0.0.1:4000"
        );
        assert_eq!(
            rpc_client_static_instances(&config)
                .expect("instances")
                .len(),
            2
        );
    }

    #[test]
    fn rpc_client_etcd_config_is_not_a_direct_target() {
        let config = roze_config::RpcClientConfig {
            etcd: Some(roze_config::RpcClientEtcdConfig {
                hosts: vec!["127.0.0.1:2379".to_string()],
                key: "order.rpc".to_string(),
                ..Default::default()
            }),
            endpoints: Vec::new(),
            target: None,
            app: None,
            token: None,
            non_block: false,
            timeout_ms: 2_000,
            keepalive_time_secs: 20,
            balancer: Default::default(),
            middlewares: Default::default(),
        };

        assert!(!rpc_client_has_direct_target(&config));
        assert!(rpc_client_target(&config).is_err());
    }

    #[test]
    fn rpc_client_config_rejects_mixed_connection_modes() {
        let config = roze_config::RpcClientConfig {
            etcd: Some(roze_config::RpcClientEtcdConfig {
                hosts: vec!["127.0.0.1:2379".to_string()],
                key: "order.rpc".to_string(),
                ..Default::default()
            }),
            endpoints: vec!["127.0.0.1:4000".to_string()],
            target: None,
            app: None,
            token: None,
            non_block: false,
            timeout_ms: 2_000,
            keepalive_time_secs: 20,
            balancer: Default::default(),
            middlewares: Default::default(),
        };

        let err = validate_rpc_client_config_mode(&config).expect_err("mixed mode");
        assert!(err.to_string().contains("exactly one connection mode"));
    }

    #[test]
    fn apply_client_auth_sets_app_and_token_metadata() {
        let config = roze_config::RpcClientConfig {
            etcd: None,
            endpoints: Vec::new(),
            target: Some("dns:///user.rpc".to_string()),
            app: Some("admin".to_string()),
            token: Some("secret".to_string()),
            non_block: false,
            timeout_ms: 2_000,
            keepalive_time_secs: 20,
            balancer: Default::default(),
            middlewares: Default::default(),
        };
        let options = RpcClientOptions::from_config(&config);
        let mut request = Request::new(());

        apply_client_auth(&mut request, &options, Some(&config));

        assert_eq!(
            request
                .metadata()
                .get("x-app")
                .and_then(|value| value.to_str().ok()),
            Some("admin")
        );
        assert_eq!(
            request
                .metadata()
                .get("authorization")
                .and_then(|value| value.to_str().ok()),
            Some("Bearer secret")
        );
    }

    #[test]
    fn method_policy_prefers_method_override() {
        let mut governance = roze_config::GovernanceConfig {
            timeout_ms: Some(1000),
            fallback: Some(roze_config::GovernanceFallbackConfig {
                enabled: true,
                status: 503,
                body: Some(serde_json::json!({"message": "global"})),
                headers: Default::default(),
            }),
            ..Default::default()
        };
        governance.routes.insert(
            "GetUser".into(),
            roze_config::RouteGovernanceConfig {
                timeout_ms: Some(50),
                rate_limit: None,
                breaker: None,
                shedding: Some(roze_config::SheddingConfig {
                    concurrency: 3,
                    window_ms: 1_000,
                    min_samples: 10,
                    max_avg_latency_ms: 500,
                    max_failure_ratio_per_mille: 500,
                    cool_down_ms: 1_000,
                }),
                fallback: Some(roze_config::GovernanceFallbackConfig {
                    enabled: true,
                    status: 598,
                    body: Some(serde_json::json!({"message": "method"})),
                    headers: Default::default(),
                }),
                ..Default::default()
            },
        );

        let policy = method_policy(Some(&governance), "GetUser");

        assert_eq!(policy.timeout, Some(Duration::from_millis(50)));
        assert_eq!(policy.shedding.expect("shedding").concurrency, 3);
        let fallback = policy.fallback.expect("fallback");
        assert_eq!(fallback.status, 598);
        assert_eq!(
            fallback.body.expect("fallback body")["message"],
            serde_json::json!("method")
        );
    }

    #[test]
    fn method_fallback_ignores_disabled_policy() {
        let governance = roze_config::GovernanceConfig {
            fallback: Some(roze_config::GovernanceFallbackConfig {
                enabled: false,
                status: 503,
                body: Some(serde_json::json!({"message": "off"})),
                headers: Default::default(),
            }),
            ..Default::default()
        };

        assert!(method_fallback(Some(&governance), "GetUser").is_none());
    }

    #[test]
    fn permission_enforcement_returns_permission_denied_status() {
        let context = Context::background().with_permissions(["users:read"]);

        assert!(enforce_permissions(&context, &["users:read"]).is_ok());
        let status = enforce_permissions(&context, &["users:write"])
            .expect_err("missing permission should be rejected");
        assert_eq!(status.code(), Code::PermissionDenied);
    }

    #[test]
    fn apply_fallback_only_for_server_errors() {
        let fallback = roze_config::GovernanceFallbackConfig {
            enabled: true,
            status: 598,
            body: Some(serde_json::json!({"message": "degraded"})),
            headers: Default::default(),
        };

        assert_eq!(
            apply_fallback(
                "catalog",
                RozeError::BadRequest("bad".into()),
                Some(fallback.clone())
            ),
            RozeError::BadRequest("bad".into())
        );
        assert!(matches!(
            apply_fallback(
                "catalog",
                RozeError::Internal("boom".into()),
                Some(fallback)
            ),
            RozeError::Fallback { status: 598, .. }
        ));
    }

    #[test]
    fn begin_method_sheds_when_concurrency_is_full() {
        let mut governance = roze_config::GovernanceConfig::default();
        let method = format!("Shed{}", std::process::id());
        governance.routes.insert(
            method.clone(),
            roze_config::RouteGovernanceConfig {
                shedding: Some(roze_config::SheddingConfig {
                    concurrency: 1,
                    window_ms: 1_000,
                    min_samples: 10,
                    max_avg_latency_ms: 1_000,
                    max_failure_ratio_per_mille: 500,
                    cool_down_ms: 60_000,
                }),
                ..Default::default()
            },
        );

        let first = begin_method(
            "svc",
            method.clone(),
            Context::background(),
            Some(&governance),
        );
        assert!(first.is_ok());
        let second = begin_method("svc", method, Context::background(), Some(&governance));
        assert!(matches!(
            second,
            Err(status) if status.code() == Code::Unavailable && status.message() == "load shed"
        ));
    }

    #[test]
    fn method_breaker_serializes_half_open_probe_and_recovers_after_cancel() {
        let mut governance = roze_config::GovernanceConfig::default();
        let method = format!("HalfOpen{}", std::process::id());
        governance.routes.insert(
            method.clone(),
            roze_config::RouteGovernanceConfig {
                breaker: Some(roze_config::BreakerConfig {
                    failure_threshold: 1,
                    reset_timeout_ms: 1,
                }),
                ..Default::default()
            },
        );

        let (_ctx, failing) = begin_method(
            "svc",
            method.clone(),
            Context::background(),
            Some(&governance),
        )
        .expect("closed breaker should allow request");
        finish_method(failing, "internal");
        std::thread::sleep(Duration::from_millis(2));

        let (_ctx, cancelled_probe) = begin_method(
            "svc",
            method.clone(),
            Context::background(),
            Some(&governance),
        )
        .expect("expired breaker should allow one probe");
        drop(cancelled_probe);
        let protected = begin_method(
            "svc",
            method.clone(),
            Context::background(),
            Some(&governance),
        );
        assert!(
            matches!(protected, Err(status) if status.code() == Code::Unavailable && status.message() == "circuit open")
        );

        std::thread::sleep(Duration::from_millis(2));
        let (_ctx, successful_probe) = begin_method(
            "svc",
            method.clone(),
            Context::background(),
            Some(&governance),
        )
        .expect("cancelled probe should become retryable after reset timeout");
        let concurrent = begin_method(
            "svc",
            method.clone(),
            Context::background(),
            Some(&governance),
        );
        assert!(
            matches!(concurrent, Err(status) if status.code() == Code::Unavailable && status.message() == "circuit open")
        );
        finish_method(successful_probe, "ok");

        let recovered = begin_method("svc", method, Context::background(), Some(&governance));
        assert!(recovered.is_ok());
    }

    #[test]
    fn dropping_method_guard_releases_shedding_without_opening_breaker() {
        let mut governance = roze_config::GovernanceConfig::default();
        let method = format!("Cancelled{}", std::process::id());
        governance.routes.insert(
            method.clone(),
            roze_config::RouteGovernanceConfig {
                breaker: Some(roze_config::BreakerConfig {
                    failure_threshold: 1,
                    reset_timeout_ms: 60_000,
                }),
                shedding: Some(roze_config::SheddingConfig {
                    concurrency: 1,
                    window_ms: 1_000,
                    min_samples: 1,
                    max_avg_latency_ms: 1_000,
                    max_failure_ratio_per_mille: 0,
                    cool_down_ms: 60_000,
                }),
                ..Default::default()
            },
        );

        let (_ctx, guard) = begin_method(
            "svc",
            method.clone(),
            Context::background(),
            Some(&governance),
        )
        .expect("first call should acquire shedding capacity");
        drop(guard);

        let next = begin_method("svc", method, Context::background(), Some(&governance));
        assert!(next.is_ok());
    }

    #[tokio::test]
    async fn registration_guard_registers_and_shutdown_deregisters() {
        let registry = Arc::new(crate::registry::MemoryRegistry::default());
        let mut guard = ServiceRegistrationGuard::start(
            registry.clone(),
            "svc",
            "127.0.0.1:9000".parse().unwrap(),
        )
        .await
        .expect("start");

        assert_eq!(registry.discover("svc").await.expect("discover").len(), 1);
        guard.shutdown().await.expect("shutdown");
        assert!(registry.discover("svc").await.expect("discover").is_empty());
    }

    #[tokio::test]
    async fn registration_guard_uses_advertise_addr() {
        let registry = Arc::new(crate::registry::MemoryRegistry::default());
        let mut guard = ServiceRegistrationGuard::start_with_advertise_addr(
            registry.clone(),
            "svc",
            "0.0.0.0:9000".parse().unwrap(),
            "192.168.1.10:9000".parse().unwrap(),
        )
        .await
        .expect("start");

        let instances = registry.discover("svc").await.expect("discover");
        assert_eq!(instances[0].addr, "192.168.1.10:9000");
        guard.shutdown().await.expect("shutdown");
        assert!(registry.discover("svc").await.expect("discover").is_empty());
    }
}
