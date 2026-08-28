use super::super::*;

use crate::model::UserRepository;

pub async fn get_user(
    ctx: ServiceContext,
    request_ctx: roze_context::Context,
    req: GetUserReq,
) -> Result<UserResp, RozeError> {
    let _ = request_ctx;
    if req.id <= 0 {
        return Err(RozeError::BadRequest("id must be positive".into()));
    }

    let user = UserRepository::new(&ctx)
        .cached_find_by_id(req.id)
        .await
        .map_err(|err| RozeError::Internal(err.to_string()))?
        .ok_or_else(|| RozeError::NotFound(format!("user {} not found", req.id)))?;

    Ok(UserResp {
        id: user.id,
        username: user.username,
        created_at: user.created_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn rejects_non_positive_id_before_repository_access() {
        let path = roze_config::service_config_path(env!("CARGO_MANIFEST_DIR"));
        let config = crate::config::load(path).expect("load test config");
        let ctx = ServiceContext::new(config)
            .await
            .expect("build test context");

        let error = get_user(
            ctx,
            roze_context::Context::background_with_trace_id("trace-user"),
            GetUserReq { id: 0 },
        )
        .await
        .expect_err("non-positive id must fail");

        assert!(matches!(error, RozeError::BadRequest(_)));
    }
}
