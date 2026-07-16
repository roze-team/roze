use std::time::{Duration, SystemTime, UNIX_EPOCH};

use jsonwebtoken::{
    decode, decode_header, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Claims {
    pub sub: String,
    #[serde(default)]
    pub roles: Vec<String>,
    #[serde(default)]
    pub tenant: Option<String>,
    pub iss: String,
    pub aud: String,
    pub jti: String,
    pub iat: usize,
    pub exp: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JwtConfig {
    pub jwt_keys: Vec<JwtKey>,
    pub jwt_active_key_id: String,
    pub jwt_issuer: String,
    pub jwt_audience: String,
    pub jwt_expiration_secs: u64,
    pub jwt_clock_skew_secs: u64,
    pub revoked_token_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct JwtKey {
    pub id: String,
    pub secret: String,
}

impl From<&roze_config::AuthConfig> for JwtConfig {
    fn from(value: &roze_config::AuthConfig) -> Self {
        Self {
            jwt_keys: value
                .jwt_keys
                .iter()
                .map(|key| JwtKey {
                    id: key.id.clone(),
                    secret: key.secret.clone(),
                })
                .collect(),
            jwt_active_key_id: value.jwt_active_key_id.clone(),
            jwt_issuer: value.jwt_issuer.clone(),
            jwt_audience: value.jwt_audience.clone(),
            jwt_expiration_secs: value.jwt_expiration_secs,
            jwt_clock_skew_secs: value.jwt_clock_skew_secs,
            revoked_token_ids: value.revoked_token_ids.clone(),
        }
    }
}

pub fn issue_token(claims: &Claims, config: &JwtConfig) -> anyhow::Result<String> {
    let mut claims = claims.clone();
    let now = now_unix_secs()?;
    claims.iss = config.jwt_issuer.clone();
    claims.aud = config.jwt_audience.clone();
    claims.iat = now as usize;
    claims.exp = (now + config.jwt_expiration_secs) as usize;

    let key = config
        .jwt_keys
        .iter()
        .find(|key| key.id == config.jwt_active_key_id)
        .ok_or_else(|| {
            anyhow::anyhow!("active JWT key '{}' not found", config.jwt_active_key_id)
        })?;
    let mut header = Header::new(Algorithm::HS256);
    header.kid = Some(key.id.clone());
    Ok(encode(
        &header,
        &claims,
        &EncodingKey::from_secret(key.secret.as_bytes()),
    )?)
}

pub fn verify_token(token: &str, config: &JwtConfig) -> anyhow::Result<Claims> {
    let header = decode_header(token)?;
    let key_id = header
        .kid
        .ok_or_else(|| anyhow::anyhow!("JWT header is missing kid"))?;
    let key = config
        .jwt_keys
        .iter()
        .find(|key| key.id == key_id)
        .ok_or_else(|| anyhow::anyhow!("JWT signing key '{key_id}' is not trusted"))?;
    let mut validation = Validation::new(Algorithm::HS256);
    validation.validate_exp = true;
    validation.leeway = config.jwt_clock_skew_secs;
    validation.set_issuer(&[config.jwt_issuer.as_str()]);
    validation.set_audience(&[config.jwt_audience.as_str()]);
    let token_data = decode::<Claims>(
        token,
        &DecodingKey::from_secret(key.secret.as_bytes()),
        &validation,
    )?;
    anyhow::ensure!(
        !config.revoked_token_ids.contains(&token_data.claims.jti),
        "JWT has been revoked"
    );
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
            jwt_keys: vec![JwtKey {
                id: "2026-07".to_string(),
                secret: "secret".to_string(),
            }],
            jwt_active_key_id: "2026-07".to_string(),
            jwt_issuer: "roze".to_string(),
            jwt_audience: "roze-api".to_string(),
            jwt_expiration_secs: 60,
            jwt_clock_skew_secs: 5,
            revoked_token_ids: Vec::new(),
        };
        let claims = Claims {
            sub: "user-1".to_string(),
            roles: vec!["admin".to_string()],
            tenant: Some("acme".to_string()),
            iss: String::new(),
            aud: String::new(),
            jti: "token-1".to_string(),
            iat: 0,
            exp: 0,
        };

        let token = issue_token(&claims, &config).expect("token");
        let decoded = verify_token(&token, &config).expect("claims");

        assert_eq!(decoded.sub, "user-1");
        assert_eq!(decoded.roles, vec!["admin"]);
        assert_eq!(decoded.tenant.as_deref(), Some("acme"));
        assert_eq!(decoded.iss, "roze");
        assert_eq!(decoded.aud, "roze-api");
        assert!(decoded.exp > decoded.iat);
    }

    #[test]
    fn rotation_accepts_trusted_old_key_and_rejects_revoked_token() {
        let old = JwtConfig {
            jwt_keys: vec![JwtKey {
                id: "old".into(),
                secret: "old-secret".into(),
            }],
            jwt_active_key_id: "old".into(),
            jwt_issuer: "roze".into(),
            jwt_audience: "api".into(),
            jwt_expiration_secs: 60,
            jwt_clock_skew_secs: 0,
            revoked_token_ids: Vec::new(),
        };
        let claims = Claims {
            sub: "user-1".into(),
            roles: Vec::new(),
            tenant: None,
            iss: String::new(),
            aud: String::new(),
            jti: "revoked-1".into(),
            iat: 0,
            exp: 0,
        };
        let token = issue_token(&claims, &old).expect("old token");
        let mut rotated = old.clone();
        rotated.jwt_keys.push(JwtKey {
            id: "new".into(),
            secret: "new-secret".into(),
        });
        rotated.jwt_active_key_id = "new".into();
        assert!(verify_token(&token, &rotated).is_ok());
        rotated.revoked_token_ids.push("revoked-1".into());
        assert!(verify_token(&token, &rotated).is_err());
    }

    #[test]
    fn parses_bearer_token() {
        assert_eq!(extract_bearer_token("Bearer abc.def"), Some("abc.def"));
        assert_eq!(extract_bearer_token("bearer abc.def"), Some("abc.def"));
        assert_eq!(extract_bearer_token("Token abc.def"), None);
    }
}
