use super::*;

pub async fn echo(
    ctx: ServiceContext,
    request_ctx: roze_context::Context,
    req: EchoRequest,
) -> Result<EchoResponse, RozeError> {
    let _ = ctx;
    let _ = request_ctx;
    Ok(EchoResponse {
        payload: req.payload,
    })
}
