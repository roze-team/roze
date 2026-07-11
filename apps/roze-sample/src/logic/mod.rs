#![allow(dead_code)]

use roze_error::RozeError;

use crate::svc::ServiceContext;
use crate::types::*;

pub async fn post_roze_sample_login(
    ctx: ServiceContext,
    request_ctx: roze_context::Context,
    req: LoginReq,
) -> Result<LoginResp, RozeError> {
    let _ = ctx;
    let _ = request_ctx;
    let _ = req;
    Ok(LoginResp::default_response())
}

impl LoginResp {
    fn default_response() -> Self {
        Self {
            token: String::new(),
            expires_at: Default::default(),
        }
    }
}
