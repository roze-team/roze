use std::time::{Duration, SystemTime, UNIX_EPOCH};

use jsonwebtoken::{decode, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Claims {
    pub sub: String,
    #[serde(default)]
    pub roles: Vec<String>,
    #[serde(default)]
    pub tenant: Option<String>,
    pub iss: String,
    pub iat: usize,
    pub exp: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JwtConfig {
    pub jwt_secret: String,
    pub jwt_issuer: String,
    pub jwt_expiration_secs: u64,
}

impl From<&crate::config::AuthConfig> for JwtConfig {
    fn from(value: &crate::config::AuthConfig) -> Self {
        Self {
            jwt_secret: value.jwt_secret.clone(),
            jwt_issuer: value.jwt_issuer.clone(),
            jwt_expiration_secs: value.jwt_expiration_secs,
        }
    }
}

pub fn issue_token(claims: &Claims, config: &JwtConfig) -> anyhow::Result<String> {
    let mut claims = claims.clone();
    let now = now_unix_secs()?;
    claims.iss = config.jwt_issuer.clone();
    claims.iat = now as usize;
    claims.exp = (now + config.jwt_expiration_secs) as usize;

    Ok(encode(
        &Header::new(Algorithm::HS256),
        &claims,
        &EncodingKey::from_secret(config.jwt_secret.as_bytes()),
    )?)
}

pub fn verify_token(token: &str, config: &JwtConfig) -> anyhow::Result<Claims> {
    let mut validation = Validation::new(Algorithm::HS256);
    validation.validate_exp = true;
    validation.set_issuer(&[config.jwt_issuer.as_str()]);
    let token_data = decode::<Claims>(
        token,
        &DecodingKey::from_secret(config.jwt_secret.as_bytes()),
        &validation,
    )?;
    Ok(token_data.claims)
}

pub fn extract_bearer_token(header_value: &str) -> Option<&str> {
    header_value
        .strip_prefix("Bearer ")
        .or_else(|| header_value.strip_prefix("bearer "))
}

pub fn now_unix_secs() -> anyhow::Result<u64> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_round_trip() {
        let config = JwtConfig {
            jwt_secret: "secret".to_string(),
            jwt_issuer: "roze".to_string(),
            jwt_expiration_secs: 60,
        };
        let claims = Claims {
            sub: "user-1".to_string(),
            roles: vec!["admin".to_string()],
            tenant: Some("acme".to_string()),
            iss: String::new(),
            iat: 0,
            exp: 0,
        };

        let token = issue_token(&claims, &config).expect("token");
        let decoded = verify_token(&token, &config).expect("claims");

        assert_eq!(decoded.sub, "user-1");
        assert_eq!(decoded.roles, vec!["admin"]);
        assert_eq!(decoded.tenant.as_deref(), Some("acme"));
        assert_eq!(decoded.iss, "roze");
        assert!(decoded.exp > decoded.iat);
    }

    #[test]
    fn parses_bearer_token() {
        assert_eq!(extract_bearer_token("Bearer abc.def"), Some("abc.def"));
        assert_eq!(extract_bearer_token("bearer abc.def"), Some("abc.def"));
        assert_eq!(extract_bearer_token("Token abc.def"), None);
    }
}
