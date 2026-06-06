#![allow(dead_code, unused_imports)]

use crate::pb::roze_sample::{self as proto, roze_sample_server::RozeSample};
use crate::svc::ServiceContext;
use crate::types::*;

#[derive(Debug, Clone)]
pub struct RpcService {
    ctx: ServiceContext,
}

impl RpcService {
    pub fn new(ctx: ServiceContext) -> Self {
        Self { ctx }
    }
}

#[tonic::async_trait]
impl RozeSample for RpcService {
    async fn post_roze_sample_login(&self, request: tonic::Request<proto::LoginReq>) -> Result<tonic::Response<proto::LoginResp>, tonic::Status> {
        let request_ctx = roze_rpc::rpc::request_context(&request);
        let req = request.into_inner();
        let req = LoginReq { username: req.username, password: req.password };
        let resp = crate::logic::post_roze_sample_login(self.ctx.clone(), request_ctx, req)
            .await
            .map_err(|err| tonic::Status::internal(err.to_string()))?;
        Ok(tonic::Response::new(proto::LoginResp { token: resp.token, expires_at: resp.expires_at }))
    }
}
