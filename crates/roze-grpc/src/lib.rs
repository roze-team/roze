//! gRPC transport helpers built on tonic.
//!
//! This crate intentionally centralizes the transport-facing API so the rest of the
//! workspace depends on `roze_grpc::transport` instead of importing `tonic`
//! directly. That keeps the transport boundary easy to swap in the future.

use std::{collections::BTreeMap, net::SocketAddr, time::Duration};

use roze_context::{AuthContext, Context};
use roze_trace::generate_trace_id;

pub mod transport {
    pub use tonic::metadata::{Ascii, KeyAndValueRef, MetadataKey, MetadataMap, MetadataValue};
    pub use tonic::server::NamedService;
    pub use tonic::transport::{Channel, Endpoint, Server};
    pub use tonic::{Code, Request, Response, Status};
}

pub mod build {
    use std::{error::Error, path::Path};

    pub fn compile<P>(proto_files: &[P], includes: &[P]) -> Result<(), Box<dyn Error>>
    where
        P: AsRef<Path>,
    {
        tonic_prost_build::configure()
            .build_server(true)
            .build_client(true)
            .compile_protos(proto_files, includes)?;
        Ok(())
    }
}

mod router {
    use std::{
        collections::BTreeMap,
        convert::Infallible,
        fmt,
        future::Future,
        pin::Pin,
        task::{Context, Poll},
    };

    use http::{Request, Response};
    use tonic::{body::Body, server::NamedService, Status};
    use tower::{util::BoxCloneService, Service, ServiceExt};

    type Route = BoxCloneService<Request<Body>, Response<Body>, Infallible>;
    type RouteFuture =
        Pin<Box<dyn Future<Output = Result<Response<Body>, Infallible>> + Send + 'static>>;

    /// A Roze-owned gRPC service router.
    ///
    /// It dispatches standard gRPC paths (`/{service}/{method}`) without enabling
    /// Tonic's optional third-party router feature.
    #[derive(Clone, Default)]
    pub struct GrpcRouter {
        routes: BTreeMap<&'static str, Route>,
    }

    impl fmt::Debug for GrpcRouter {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter
                .debug_struct("GrpcRouter")
                .field("services", &self.routes.keys().collect::<Vec<_>>())
                .finish()
        }
    }

    impl GrpcRouter {
        pub fn new<S>(service: S) -> Self
        where
            S: Service<Request<Body>, Response = Response<Body>, Error = Infallible>
                + NamedService
                + Clone
                + Send
                + Sync
                + 'static,
            S::Future: Send + 'static,
        {
            Self::default().add_service(service)
        }

        #[must_use]
        pub fn add_service<S>(mut self, service: S) -> Self
        where
            S: Service<Request<Body>, Response = Response<Body>, Error = Infallible>
                + NamedService
                + Clone
                + Send
                + Sync
                + 'static,
            S::Future: Send + 'static,
        {
            assert!(
                self.routes
                    .insert(S::NAME, BoxCloneService::new(service))
                    .is_none(),
                "gRPC service `{}` is already registered",
                S::NAME
            );
            self
        }
    }

    impl Service<Request<Body>> for GrpcRouter {
        type Response = Response<Body>;
        type Error = Infallible;
        type Future = RouteFuture;

        fn poll_ready(&mut self, _context: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }

        fn call(&mut self, request: Request<Body>) -> Self::Future {
            let route = service_name(request.uri().path())
                .and_then(|service| self.routes.get(service))
                .cloned();
            Box::pin(async move {
                match route {
                    Some(route) => route.oneshot(request).await,
                    None => Ok(unimplemented_response()),
                }
            })
        }
    }

    fn service_name(path: &str) -> Option<&str> {
        let path = path.strip_prefix('/')?;
        let (service, method) = path.split_once('/')?;
        (!service.is_empty() && !method.is_empty()).then_some(service)
    }

    fn unimplemented_response() -> Response<Body> {
        let (parts, ()) = Status::unimplemented("").into_http::<()>().into_parts();
        Response::from_parts(parts, Body::empty())
    }

    #[cfg(test)]
    mod tests {
        use std::{
            future::{ready, Ready},
            task::{Context, Poll},
        };

        use http::{header::HeaderValue, Request, Response};
        use tonic::{body::Body, server::NamedService};
        use tower::{Service, ServiceExt};

        use super::GrpcRouter;

        #[derive(Clone)]
        struct DemoService;

        impl NamedService for DemoService {
            const NAME: &'static str = "roze.demo.v1.Demo";
        }

        impl Service<Request<Body>> for DemoService {
            type Response = Response<Body>;
            type Error = std::convert::Infallible;
            type Future = Ready<Result<Self::Response, Self::Error>>;

            fn poll_ready(&mut self, _context: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
                Poll::Ready(Ok(()))
            }

            fn call(&mut self, _request: Request<Body>) -> Self::Future {
                let mut response = Response::new(Body::empty());
                response
                    .headers_mut()
                    .insert("x-roze-service", HeaderValue::from_static("demo"));
                ready(Ok(response))
            }
        }

        #[tokio::test]
        async fn routes_by_grpc_service_name_and_rejects_unknown_services() {
            let router = GrpcRouter::new(DemoService);
            let response = router
                .clone()
                .oneshot(
                    Request::builder()
                        .uri("/roze.demo.v1.Demo/Get")
                        .body(Body::empty())
                        .expect("request"),
                )
                .await
                .expect("route response");
            assert_eq!(response.headers()["x-roze-service"], "demo");

            let response = router
                .oneshot(
                    Request::builder()
                        .uri("/roze.missing.v1.Missing/Get")
                        .body(Body::empty())
                        .expect("request"),
                )
                .await
                .expect("fallback response");
            assert_eq!(response.headers()["grpc-status"], "12");
        }
    }
}

pub use router::GrpcRouter;

#[macro_export]
macro_rules! include_proto {
    ($package:literal) => {
        tonic::include_proto!($package);
    };
}

use self::transport::{Channel, Endpoint, MetadataValue, Request, Server};

#[derive(Debug, Clone, Copy)]
pub struct GrpcClientOptions {
    pub connect_timeout: Duration,
    pub request_timeout: Duration,
}

impl Default for GrpcClientOptions {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(5),
            request_timeout: Duration::from_secs(5),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct GrpcServerConfig {
    pub addr: SocketAddr,
}

pub struct GrpcServer {
    config: GrpcServerConfig,
}

impl GrpcServer {
    pub fn new(addr: SocketAddr) -> Self {
        Self {
            config: GrpcServerConfig { addr },
        }
    }

    pub fn builder(&self) -> Server {
        Server::builder()
    }

    pub fn addr(&self) -> SocketAddr {
        self.config.addr
    }

    pub fn config(&self) -> GrpcServerConfig {
        self.config
    }
}

pub fn normalize_endpoint(addr: &str) -> anyhow::Result<String> {
    let addr = addr.trim();
    if addr.is_empty() {
        anyhow::bail!("gRPC endpoint address is empty");
    }
    if addr.starts_with("http://") || addr.starts_with("https://") {
        Ok(addr.to_string())
    } else {
        Ok(format!("http://{addr}"))
    }
}

pub async fn connect(addr: impl AsRef<str>) -> anyhow::Result<Channel> {
    let url = normalize_endpoint(addr.as_ref())?;
    Ok(Endpoint::from_shared(url)?.connect().await?)
}

pub async fn connect_with_options(
    addr: impl AsRef<str>,
    options: GrpcClientOptions,
) -> anyhow::Result<Channel> {
    let url = normalize_endpoint(addr.as_ref())?;
    Ok(Endpoint::from_shared(url)?
        .connect_timeout(options.connect_timeout)
        .timeout(options.request_timeout)
        .connect()
        .await?)
}

pub fn server_builder() -> Server {
    Server::builder()
}

pub fn request_context<T>(request: &Request<T>) -> Context {
    request
        .extensions()
        .get::<Context>()
        .cloned()
        .unwrap_or_else(|| {
            let request_id = metadata_value(request, roze_context::REQUEST_ID_HEADER)
                .unwrap_or_else(generate_trace_id);
            let trace_id = metadata_trace_id(request).unwrap_or_else(generate_trace_id);
            let mut ctx = Context::background_with_request_id_and_trace_id(request_id, trace_id)
                .with_metadata_map(metadata_context_values(request.metadata()));
            if let Some(auth) = metadata_auth(request.metadata()) {
                ctx = ctx.with_auth(auth);
            }
            match metadata_timeout(request) {
                Some(timeout) => ctx.with_timeout(timeout),
                None => ctx,
            }
        })
}

pub fn apply_context<T>(request: &mut Request<T>, context: &Context) {
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
    request.extensions_mut().insert(context.clone());
}

pub fn metadata_trace_id<T>(request: &Request<T>) -> Option<String> {
    request
        .metadata()
        .get(roze_context::TRACE_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

pub fn metadata_timeout<T>(request: &Request<T>) -> Option<Duration> {
    let raw = request
        .metadata()
        .get(roze_context::TIMEOUT_HEADER)
        .and_then(|value| value.to_str().ok())?;
    let millis = raw.parse::<u64>().ok()?;
    Some(Duration::from_millis(millis))
}

fn metadata_value<T>(request: &Request<T>, key: &str) -> Option<String> {
    request
        .metadata()
        .get(key)
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn insert_metadata(metadata: &mut transport::MetadataMap, key: &str, value: &str) {
    let Ok(key) = key.parse::<transport::MetadataKey<transport::Ascii>>() else {
        return;
    };
    let Ok(value) = MetadataValue::try_from(value) else {
        return;
    };
    metadata.insert(key, value);
}

fn metadata_auth(metadata: &transport::MetadataMap) -> Option<AuthContext> {
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

fn metadata_context_values(metadata: &transport::MetadataMap) -> BTreeMap<String, String> {
    metadata
        .iter()
        .filter_map(|entry| {
            let transport::KeyAndValueRef::Ascii(key, value) = entry else {
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

#[allow(clippy::result_large_err)]
pub fn with_context_interceptor(
    context: Context,
) -> impl FnMut(Request<()>) -> Result<Request<()>, transport::Status> + Clone {
    move |mut request: Request<()>| {
        apply_context(&mut request, &context);
        Ok(request)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_addresses() {
        assert_eq!(
            normalize_endpoint(" 127.0.0.1:50051 ").expect("url"),
            "http://127.0.0.1:50051"
        );
        assert_eq!(
            normalize_endpoint("http://127.0.0.1:50051").expect("url"),
            "http://127.0.0.1:50051"
        );
        assert!(normalize_endpoint("  ").is_err());
    }

    #[test]
    fn context_round_trips_through_request() {
        let context = Context::background_with_request_id_and_trace_id("request-1", "trace-1")
            .with_auth(AuthContext {
                subject: "user-1".to_string(),
                roles: vec!["admin".to_string(), "ops".to_string()],
                tenant: Some("tenant-1".to_string()),
            })
            .with_metadata("locale", "zh-CN");
        let mut request = Request::new(());
        apply_context(&mut request, &context);
        assert_eq!(metadata_trace_id(&request).as_deref(), Some("trace-1"));
        let restored = request_context(&request);
        assert_eq!(restored.request_id(), "request-1");
        assert_eq!(restored.trace_id(), "trace-1");
        assert_eq!(restored.subject().as_deref(), Some("user-1"));
        assert_eq!(restored.tenant().as_deref(), Some("tenant-1"));
        assert_eq!(restored.roles(), vec!["admin", "ops"]);
        assert_eq!(restored.metadata_value("locale").as_deref(), Some("zh-CN"));
    }
}
