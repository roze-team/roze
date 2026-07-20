use super::super::*;

pub async fn rpc_echo(
    ctx: ServiceContext,
    request_ctx: roze_context::Context,
    req: EchoRequest,
) -> Result<EchoResponse, RozeError> {
    let response = ctx
        .competitive()
        .echo(
            &request_ctx,
            competitive_roze_rpc::pb::competitive_v1::EchoRequest {
                payload: req.payload.into_bytes(),
            },
        )
        .await?;
    let payload = String::from_utf8(response.payload).map_err(|error| {
        RozeError::Internal(format!("RPC echo returned invalid UTF-8: {error}"))
    })?;
    Ok(EchoResponse { payload })
}
