use std::{net::SocketAddr, time::Duration};

use crate::{
    balance::Balancer,
    registry::Registry,
    shared::auth::{extract_bearer_token, verify_token, JwtConfig},
};
use tokio::time::sleep;
use tonic::transport::{Channel, Endpoint, Server};
use tonic::{metadata::MetadataValue, Request, Status};
use tracing::info;

#[derive(Debug, Clone)]
pub struct RpcConfig {
    pub addr: SocketAddr,
}

pub struct RpcServer {
    config: RpcConfig,
}

#[derive(Debug, Clone, Copy)]
pub struct RpcClientOptions {
    pub connect_timeout: Duration,
    pub request_timeout: Duration,
    pub max_retries: usize,
    pub retry_backoff: Duration,
}

impl Default for RpcClientOptions {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(5),
            request_timeout: Duration::from_secs(5),
            max_retries: 1,
            retry_backoff: Duration::from_millis(100),
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
        let subject = MetadataValue::try_from(claims.sub.as_str())
            .map_err(|_| Status::unauthenticated("invalid subject"))?;
        req.metadata_mut().insert("x-subject", subject);
        req.extensions_mut().insert(claims);
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
    let channel = Endpoint::from_shared(url)?
        .connect_timeout(options.connect_timeout)
        .timeout(options.request_timeout)
        .connect()
        .await?;
    Ok(channel)
}

pub async fn connect_via_registry<R, B>(
    service: &str,
    registry: &R,
    balancer: &B,
) -> anyhow::Result<Channel>
where
    R: Registry,
    B: Balancer,
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
    B: Balancer,
{
    let instances = registry.discover(service).await?;
    let instance = balancer
        .pick(&instances)
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
        tonic::Code::Unavailable | tonic::Code::DeadlineExceeded | tonic::Code::Unknown
    )
}

pub async fn retry_status<F, Fut, T>(mut call: F, options: RpcClientOptions) -> Result<T, Status>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, Status>>,
{
    let mut attempt = 0usize;
    loop {
        let response = call().await;
        match response {
            Ok(value) => return Ok(value),
            Err(status) if attempt < options.max_retries && should_retry_status(&status) => {
                attempt += 1;
                sleep(retry_delay(options.retry_backoff, attempt)).await;
            }
            Err(status) => return Err(status),
        }
    }
}

fn retry_delay(base: Duration, attempt: usize) -> Duration {
    let factor = attempt.max(1) as u32;
    base.saturating_mul(factor)
}

#[cfg(test)]
mod tests {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    use super::*;

    #[test]
    fn retry_status_targets_transient_errors() {
        assert!(should_retry_status(&Status::unavailable("down")));
        assert!(should_retry_status(&Status::deadline_exceeded("slow")));
        assert!(should_retry_status(&Status::new(
            tonic::Code::Unknown,
            "unknown"
        )));
        assert!(!should_retry_status(&Status::invalid_argument(
            "bad request"
        )));
    }

    #[tokio::test]
    async fn retry_status_retries_once() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let attempts_clone = attempts.clone();
        let result = retry_status(
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
            },
        )
        .await
        .expect("retry should succeed");

        assert_eq!(result, "ok");
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }
}
