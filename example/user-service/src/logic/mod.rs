use roze_core::rest::AppError;

use crate::svc::ServiceContext;
use crate::types::*;

pub async fn login(ctx: ServiceContext, req: LoginReq) -> Result<LoginResp, AppError> {
    let _ = ctx;
    let _ = req;
    Ok(LoginResp::default_response())
}

impl LoginResp {
    fn default_response() -> Self {
        Self {
            token: String::new(),
        }
    }
}
