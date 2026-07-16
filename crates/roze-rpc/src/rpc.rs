use std::{
    collections::BTreeMap,
    net::SocketAddr,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, OnceLock,
    },
    time::{Duration, Instant},
};

use crate::{
    balance::{build_balancer, Balancer, BalancerKind},
    registry::{
        registry_config_from_rpc_client_etcd, CachedRegistryResolver, EtcdRegistry, Registry,
        ServiceInstance,
    },
};
use roze_context::{AuthContext, Context};
use roze_error::RozeError;
use roze_grpc::transport::{
    Channel, Code, Endpoint, MetadataMap, MetadataValue, Request, Server, Status,
};
use roze_jwt::{extract_bearer_token, verify_token, JwtConfig};
use roze_metrics::{record_resilience_decision, record_rpc_method};
use roze_resilience::{
    full_jitter_delay, BreakerDecision, BreakerPermit, BreakerRegistry, GovernanceBoundary,
    OperationKey, RateLimitRegistry, RetryBudgetRegistry, SheddingRegistry,
};
use roze_trace::generate_trace_id;
use tokio::time::sleep;
use tracing::info;

static METHOD_RATE_LIMITS: OnceLock<RateLimitRegistry> = OnceLock::new();
static METHOD_BREAKERS: OnceLock<BreakerRegistry> = OnceLock::new();
static METHOD_SHEDDERS: OnceLock<SheddingRegistry> = OnceLock::new();
static CLIENT_RETRY_BUDGETS: OnceLock<RetryBudgetRegistry> = OnceLock::new();
static RPC_ENDPOINT_CURSOR: AtomicUsize = AtomicUsize::new(0);

pub const ERROR_CODE_METADATA: &str = "x-roze-error-code";
pub const ERROR_KIND_METADATA: &str = "x-roze-error-kind";
pub const FALLBACK_STATUS_METADATA: &str = "x-roze-fallback-status";
pub const FALLBACK_BODY_METADATA: &str = "x-roze-fallback-body";
pub const FALLBACK_HEADERS_METADATA: &str = "x-roze-fallback-headers";

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
        let registry = EtcdRegistry::new(&registry_config);
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
            match request
                .metadata()
                .get(roze_context::TIMEOUT_HEADER)
                .and_then(|value| value.to_str().ok())
                .and_then(|raw| raw.parse::<u64>().ok())
                .map(Duration::from_millis)
            {
                Some(timeout) => ctx.with_timeout(timeout),
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
        let timeout_ms = timeout.as_millis().to_string();
        insert_metadata(
            request.metadata_mut(),
            roze_context::TIMEOUT_HEADER,
            &timeout_ms,
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
    for (key, value) in context.metadata() {
        let header = format!("{}{}", roze_context::METADATA_HEADER_PREFIX, key);
        insert_metadata(request.metadata_mut(), &header, &value);
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
    Status::with_metadata(
        code,
        error.message_i18n(locale.as_deref().unwrap_or("en-US")),
        metadata,
    )
}

fn grpc_code_from_error(error: &RozeError) -> Code {
    match error {
        RozeError::BadRequest(_) => Code::InvalidArgument,
        RozeError::Unauthorized => Code::Unauthenticated,
        RozeError::Forbidden => Code::PermissionDenied,
        RozeError::RateLimited => Code::ResourceExhausted,
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
    if metadata_value(status.metadata(), ERROR_KIND_METADATA) == Some("fallback") {
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
    match status.code() {
        Code::InvalidArgument => RozeError::BadRequest(status.message().to_string()),
        Code::Unauthenticated => RozeError::Unauthorized,
        Code::PermissionDenied => RozeError::Forbidden,
        Code::ResourceExhausted => RozeError::RateLimited,
        Code::NotFound => RozeError::NotFound(status.message().to_string()),
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

#[derive(Debug, Clone, Copy)]
pub struct MethodRateLimitConfig {
    pub burst: u32,
    pub refill: Duration,
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
    MethodPolicy {
        timeout: policy.timeout,
        rate_limit: policy.rate_limit.map(|config| MethodRateLimitConfig {
            burst: config.burst,
            refill: config.refill,
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
    if let Some(config) = &policy.rate_limit {
        match enforce_method_rate_limit(&key, config) {
            Ok(()) => record_resilience_decision(service.as_str(), "rpc", "rate_limit", "allowed"),
            Err(status) => {
                record_resilience_decision(service.as_str(), "rpc", "rate_limit", "rejected");
                return Err(status);
            }
        }
    }
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
    tracing::debug!(
        protocol = "rpc",
        service,
        method,
        max_retries = policy.max_retries,
        backoff_ms = policy.backoff.as_millis(),
        max_backoff_ms = policy.max_backoff.as_millis(),
        budget_percent = policy.budget_percent,
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
fn enforce_method_rate_limit(key: &str, config: &MethodRateLimitConfig) -> Result<(), Status> {
    if METHOD_RATE_LIMITS
        .get_or_init(RateLimitRegistry::new)
        .allow(key, rate_limit_config(*config))
    {
        Ok(())
    } else {
        Err(Status::resource_exhausted("rate limited"))
    }
}

fn rate_limit_config(config: MethodRateLimitConfig) -> roze_resilience::RateLimitConfig {
    roze_resilience::RateLimitConfig {
        burst: config.burst,
        refill: config.refill,
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
                RozeError::RateLimited,
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
            RozeError::RateLimited
        );
        assert_eq!(
            error_from_status(&Status::unavailable("down")),
            RozeError::Unavailable("down".into())
        );
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
