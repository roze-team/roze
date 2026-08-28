use super::super::*;

use roze_jwt::{issue_token, now_unix_secs, Claims};

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
        jwt_keys: vec![roze_jwt::JwtKey {
            id: "demo".to_string(),
            secret: format!("{}-demo-secret", ctx.config.name),
        }],
        jwt_active_key_id: "demo".to_string(),
        jwt_issuer: ctx.config.name.clone(),
        jwt_audience: ctx.config.name.clone(),
        jwt_expiration_secs: 24 * 60 * 60,
        jwt_clock_skew_secs: 30,
        revoked_token_ids: Vec::new(),
    });
    let claims = Claims {
        sub: req.username.clone(),
        roles: vec!["user".to_string()],
        permissions: Vec::new(),
        scopes: Vec::new(),
        tenant: None,
        iss: String::new(),
        aud: String::new(),
        jti: format!("login-{}", req.username),
        iat: 0,
        exp: 0,
    };
    let token = issue_token(&claims, &jwt).map_err(|err| RozeError::Internal(err.to_string()))?;
    let _expires_at = now_unix_secs().map_err(|err| RozeError::Internal(err.to_string()))?
        + jwt.jwt_expiration_secs;

    Ok(LoginResp { token })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn issues_token_for_non_empty_credentials() {
        let ctx = test_context().await;
        let response = login(
            ctx,
            roze_context::Context::background_with_trace_id("trace-login"),
            LoginReq {
                username: "demo".to_string(),
                password: "demo".to_string(),
            },
        )
        .await
        .expect("login");

        assert!(!response.token.is_empty());
    }

    async fn test_context() -> ServiceContext {
        let path = roze_config::service_config_path(env!("CARGO_MANIFEST_DIR"));
        let config = crate::config::load(path).expect("load test config");
        ServiceContext::new(config)
            .await
            .expect("build test context")
    }
}
