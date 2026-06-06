#![allow(dead_code, unused_imports)]

use crate::pb::roze_sample::{self as proto, roze_sample_client::RozeSampleClient as ProtoClient};
use roze_rpc::balance::Balancer;
use roze_rpc::registry::{CachedRegistryResolver, Registry};
use tonic::transport::{Channel, Endpoint};

#[derive(Debug, Clone)]
pub struct RpcClient {
    inner: ProtoClient<Channel>,
    options: roze_rpc::rpc::RpcClientOptions,
}

impl RpcClient {
    pub fn new(channel: Channel) -> Self {
        Self {
            inner: ProtoClient::new(channel),
            options: roze_rpc::rpc::RpcClientOptions::default(),
        }
    }

    pub fn with_options(channel: Channel, options: roze_rpc::rpc::RpcClientOptions) -> Self {
        Self {
            inner: ProtoClient::new(channel),
            options,
        }
    }

    pub fn inner_mut(&mut self) -> &mut ProtoClient<Channel> {
        &mut self.inner
    }

    pub async fn connect(addr: impl AsRef<str>) -> anyhow::Result<Self> {
        let url = roze_rpc::rpc::normalize_endpoint(addr.as_ref())?;
        let options = roze_rpc::rpc::RpcClientOptions::default();
        let channel = Endpoint::from_shared(url)?.connect_timeout(options.connect_timeout).timeout(options.request_timeout).connect().await?;
        Ok(Self::with_options(channel, options))
    }

    pub async fn connect_via_registry<R, B>(service: &str, registry: &R, balancer: &B) -> anyhow::Result<Self>
    where
        R: Registry,
        B: Balancer,
    {
        let channel = roze_rpc::rpc::connect_via_registry_with_options(service, registry, balancer, roze_rpc::rpc::RpcClientOptions::default()).await?;
        Ok(Self::with_options(channel, roze_rpc::rpc::RpcClientOptions::default()))
    }

    pub async fn connect_via_cached_registry<R, B>(service: &str, resolver: &CachedRegistryResolver<R, B>) -> anyhow::Result<Self>
    where
        R: Registry,
        B: Balancer,
    {
        let channel = roze_rpc::rpc::connect_via_cached_registry_with_options(service, resolver, roze_rpc::rpc::RpcClientOptions::default()).await?;
        Ok(Self::with_options(channel, roze_rpc::rpc::RpcClientOptions::default()))
    }
}

impl RpcClient {
    pub async fn post_roze_sample_login(&mut self, context: &roze_context::Context, req: proto::LoginReq) -> Result<proto::LoginResp, tonic::Status> {
        let options = self.options;
        let request_template = req.clone();
        let context = context.clone();
        let inner = self.inner.clone();
        let response = roze_rpc::rpc::retry_status(
            || {
                let mut request = tonic::Request::new(request_template.clone());
                let context = context.clone();
                let mut inner = inner.clone();
                async move {
                    if let Some(timeout) = context.remaining_timeout() {
                        request.set_timeout(timeout);
                    } else {
                        request.set_timeout(options.request_timeout);
                    }
                    roze_rpc::rpc::apply_request_context(&mut request, &context);
                    inner.post_roze_sample_login(request).await
                }
            },
            options,
        ).await?;
        Ok(response.into_inner())
    }
}
