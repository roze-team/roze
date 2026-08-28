use super::super::*;

pub async fn post_user_login(
    ctx: ServiceContext,
    request_ctx: roze_context::Context,
    req: LoginReq,
) -> Result<LoginResp, RozeError> {
    let _ = ctx;
    let _ = request_ctx;
    let _ = req;
    Ok(LoginResp::default())
}
