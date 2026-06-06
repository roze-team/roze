use roze_error::RozeError;
use roze_jwt::{issue_token, now_unix_secs, Claims};

use crate::svc::ServiceContext;
use crate::types::*;

pub async fn login(ctx: ServiceContext, request_ctx: roze_context::Context, req: LoginReq) -> Result<LoginResp, RozeError> {
    let _ = request_ctx;
    if req.username.trim().is_empty() || req.password.trim().is_empty() {
        return Err(RozeError::BadRequest("username and password are required".into()));
    }

    let jwt = ctx.jwt_config_or_demo();
    let claims = Claims {
        sub: req.username.clone(),
        roles: vec!["user".to_string()],
        tenant: None,
        iss: String::new(),
        iat: 0,
        exp: 0,
    };
    let token = issue_token(&claims, &jwt)
        .map_err(|err| RozeError::Internal(err.to_string()))?;
    let expires_at = now_unix_secs()
        .map_err(|err| RozeError::Internal(err.to_string()))?
        + jwt.jwt_expiration_secs;

    Ok(LoginResp { token, expires_at })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{config::Config, svc::ServiceContext, types::LoginReq};

    #[tokio::test]
    async fn issues_token_for_non_empty_credentials() {
        let ctx = ServiceContext {
            config: Config {
                name: "user-api".to_string(),
                rest: None,
                rpc: None,
                registry: None,
                database: None,
                cache: None,
                auth: None,
            },
            db: None,
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
}
