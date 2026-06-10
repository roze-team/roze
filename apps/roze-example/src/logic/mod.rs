use roze_error::RozeError;
use roze_jwt::{issue_token, now_unix_secs, Claims};

use crate::model::UserRepository;
use crate::svc::ServiceContext;
use crate::types::*;

pub async fn login(
    ctx: ServiceContext,
    request_ctx: roze_context::Context,
    req: LoginReq,
) -> Result<LoginResp, RozeError> {
    let _ = request_ctx;
    if req.username.trim().is_empty() || req.password.trim().is_empty() {
        return Err(RozeError::BadRequest(
            "username and password are required".into(),
        ));
    }

    let jwt = ctx.jwt_config().unwrap_or_else(|| roze_jwt::JwtConfig {
        jwt_secret: format!("{}-demo-secret", ctx.config.name),
        jwt_issuer: ctx.config.name.clone(),
        jwt_expiration_secs: 24 * 60 * 60,
    });
    let claims = Claims {
        sub: req.username.clone(),
        roles: vec!["user".to_string()],
        tenant: None,
        iss: String::new(),
        iat: 0,
        exp: 0,
    };
    let token = issue_token(&claims, &jwt).map_err(|err| RozeError::Internal(err.to_string()))?;
    let expires_at = now_unix_secs().map_err(|err| RozeError::Internal(err.to_string()))?
        + jwt.jwt_expiration_secs;

    let _ = expires_at;
    Ok(LoginResp { token })
}

pub async fn get_user(
    ctx: ServiceContext,
    request_ctx: roze_context::Context,
    req: GetUserReq,
) -> Result<UserResp, RozeError> {
    let _ = request_ctx;
    if req.id <= 0 {
        return Err(RozeError::BadRequest("id must be positive".into()));
    }

    let repo = UserRepository::new(&ctx);
    let user = repo
        .cached_find_by_id(req.id)
        .await
        .map_err(|err| RozeError::Internal(err.to_string()))?
        .ok_or_else(|| RozeError::NotFound(format!("user {} not found", req.id)))?;

    Ok(UserResp {
        id: user.id as i64,
        username: user.username,
        created_at: user.created_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::Config,
        svc::ServiceContext,
        types::{GetUserReq, LoginReq},
    };

    #[tokio::test]
    async fn issues_token_for_non_empty_credentials() {
        let ctx = ServiceContext {
            config: Config {
                name: "user-api".to_string(),
                rest: None,
                rpc: None,
                rpc_client: None,
                registry: None,
                database: None,
                mongo: None,
                cache: None,
                auth: None,
                governance: Default::default(),
            },
            db: None,
            mongo: None,
            cache: None,
        };

        let resp = login(
            ctx,
            roze_context::Context::background_with_trace_id("trace-1"),
            LoginReq {
                username: "demo".to_string(),
                password: "demo".to_string(),
            },
        )
        .await
        .expect("login");

        assert!(!resp.token.is_empty());
    }

    #[tokio::test]
    async fn validates_user_id_and_hits_repository_path() {
        let ctx = ServiceContext {
            config: Config {
                name: "user-api".to_string(),
                rest: None,
                rpc: None,
                rpc_client: None,
                registry: None,
                database: None,
                mongo: None,
                cache: None,
                auth: None,
                governance: Default::default(),
            },
            db: None,
            mongo: None,
            cache: None,
        };

        let err = get_user(
            ctx,
            roze_context::Context::background_with_trace_id("trace-2"),
            GetUserReq { id: 1 },
        )
        .await
        .expect_err("repository lookup should fail without db");

        assert!(matches!(err, RozeError::Internal(_)));
    }
}
