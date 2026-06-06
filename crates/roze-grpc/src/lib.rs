//! gRPC transport helpers built on tonic.

use std::{net::SocketAddr, time::Duration};

use roze_context::Context;
use roze_trace::{generate_trace_id, TRACE_ID_HEADER};
use tonic::transport::{Channel, Endpoint, Server};
use tonic::{metadata::MetadataValue, Request};

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
            let trace_id = metadata_trace_id(request).unwrap_or_else(generate_trace_id);
            Context::background_with_trace_id(trace_id)
        })
}

pub fn apply_context<T>(request: &mut Request<T>, context: &Context) {
    let trace_id = context.trace_id();
    if let Ok(value) = MetadataValue::try_from(trace_id.as_str()) {
        request.metadata_mut().insert(TRACE_ID_HEADER, value);
    }
    request.extensions_mut().insert(context.clone());
}

pub fn metadata_trace_id<T>(request: &Request<T>) -> Option<String> {
    request
        .metadata()
        .get(TRACE_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

pub fn with_trace_interceptor(
    context: Context,
) -> impl FnMut(Request<()>) -> Result<Request<()>, tonic::Status> + Clone {
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
        assert_eq!(normalize_endpoint("127.0.0.1:50051").expect("url"), "http://127.0.0.1:50051");
        assert_eq!(normalize_endpoint("http://127.0.0.1:50051").expect("url"), "http://127.0.0.1:50051");
    }

    #[test]
    fn context_round_trips_through_request() {
        let context = Context::background_with_trace_id("trace-1");
        let mut request = Request::new(());
        apply_context(&mut request, &context);
        assert_eq!(metadata_trace_id(&request).as_deref(), Some("trace-1"));
        let restored = request_context(&request);
        assert_eq!(restored.trace_id(), "trace-1");
    }
}
