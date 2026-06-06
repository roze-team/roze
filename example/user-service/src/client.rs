#![allow(dead_code, unused_imports)]

use crate::pb::user_api::{self as proto, user_api_client::UserApiClient as ProtoClient};
use roze_core::balance::Balancer;
use roze_core::registry::Registry;
use tonic::transport::{Channel, Endpoint};

#[derive(Debug, Clone)]
pub struct RpcClient {
    inner: ProtoClient<Channel>,
}

impl RpcClient {
    pub fn new(channel: Channel) -> Self {
        Self {
            inner: ProtoClient::new(channel),
        }
    }

    pub fn inner_mut(&mut self) -> &mut ProtoClient<Channel> {
        &mut self.inner
    }

    pub async fn connect(addr: impl AsRef<str>) -> anyhow::Result<Self> {
        let url = roze_core::rpc::normalize_endpoint(addr.as_ref())?;
        let channel = Endpoint::from_shared(url)?.connect().await?;
        Ok(Self::new(channel))
    }

    pub async fn connect_via_registry<R, B>(
        service: &str,
        registry: &R,
        balancer: &B,
    ) -> anyhow::Result<Self>
    where
        R: Registry,
        B: Balancer,
    {
        let channel = roze_core::rpc::connect_via_registry(service, registry, balancer).await?;
        Ok(Self::new(channel))
    }
}

impl RpcClient {
    pub async fn login(&mut self, req: proto::LoginReq) -> Result<proto::LoginResp, tonic::Status> {
        let response = self.inner.login(tonic::Request::new(req)).await?;
        Ok(response.into_inner())
    }
}
