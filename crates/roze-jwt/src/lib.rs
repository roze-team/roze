use std::{
    collections::HashMap,
    fmt,
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use jsonwebtoken::{
    decode, decode_header, encode, errors::ErrorKind, jwk::JwkSet, Algorithm, DecodingKey,
    EncodingKey, Header, Validation,
};
use roze_auth::{
    AuthError, AuthPrincipal, BearerTokenVerifier, OidcDiscoveryCache, OidcProviderConfig,
};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

const MAX_CACHED_JWKS_DOCUMENTS: usize = 32;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Claims {
    pub sub: String,
    #[serde(default)]
    pub roles: Vec<String>,
    #[serde(default)]
    pub tenant: Option<String>,
    #[serde(default)]
    pub permissions: Vec<String>,
    #[serde(default)]
    pub scopes: Vec<String>,
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

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct JwtKey {
    pub id: String,
    pub secret: String,
}

impl fmt::Debug for JwtKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("JwtKey")
            .field("id", &self.id)
            .field("secret", &"[REDACTED]")
            .finish()
    }
}

impl From<Claims> for AuthPrincipal {
    fn from(claims: Claims) -> Self {
        Self {
            subject: claims.sub,
            roles: claims.roles,
            tenant: claims.tenant,
            permissions: claims.permissions,
            scopes: claims.scopes,
            token_id: Some(claims.jti),
            issuer: Some(claims.iss),
        }
    }
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
    verify_token_typed(token, config).map_err(anyhow::Error::new)
}

pub fn verify_token_typed(token: &str, config: &JwtConfig) -> Result<Claims, AuthError> {
    let header = decode_header(token).map_err(map_jwt_error)?;
    let key_id = header.kid.ok_or(AuthError::UnknownKey)?;
    let key = config
        .jwt_keys
        .iter()
        .find(|key| key.id == key_id)
        .ok_or(AuthError::UnknownKey)?;
    let mut validation = Validation::new(Algorithm::HS256);
    validation.validate_exp = true;
    validation.leeway = config.jwt_clock_skew_secs;
    validation.set_issuer(&[config.jwt_issuer.as_str()]);
    validation.set_audience(&[config.jwt_audience.as_str()]);
    let token_data = decode::<Claims>(
        token,
        &DecodingKey::from_secret(key.secret.as_bytes()),
        &validation,
    )
    .map_err(map_jwt_error)?;
    if config.revoked_token_ids.contains(&token_data.claims.jti) {
        return Err(AuthError::Revoked);
    }
    Ok(token_data.claims)
}

fn map_jwt_error(error: jsonwebtoken::errors::Error) -> AuthError {
    match error.kind() {
        ErrorKind::ExpiredSignature => AuthError::Expired,
        ErrorKind::InvalidIssuer => AuthError::WrongIssuer,
        ErrorKind::InvalidAudience => AuthError::WrongAudience,
        ErrorKind::InvalidSignature => AuthError::InvalidSignature,
        _ => AuthError::Malformed,
    }
}

#[derive(Debug, Clone)]
pub struct LocalJwtVerifier {
    config: JwtConfig,
}

impl LocalJwtVerifier {
    pub fn new(config: JwtConfig) -> Self {
        Self { config }
    }
}

#[async_trait::async_trait]
impl BearerTokenVerifier for LocalJwtVerifier {
    async fn verify(&self, token: &str) -> Result<AuthPrincipal, AuthError> {
        verify_token_typed(token, &self.config).map(Into::into)
    }
}

#[derive(Debug, Clone)]
pub struct OidcJwtVerifierConfig {
    pub provider: OidcProviderConfig,
    pub audience: String,
    pub clock_skew: Duration,
    pub jwks_cache_ttl: Duration,
    pub jwks_stale_ttl: Duration,
    pub allowed_algorithms: Vec<Algorithm>,
    pub revoked_token_ids: Vec<String>,
}

#[derive(Debug, Clone)]
struct CachedJwks {
    set: JwkSet,
    fetched_at: Instant,
}

#[derive(Debug, Clone)]
pub struct OidcJwtVerifier {
    config: OidcJwtVerifierConfig,
    discovery: OidcDiscoveryCache,
    client: reqwest::Client,
    jwks: Arc<RwLock<HashMap<String, CachedJwks>>>,
}

impl OidcJwtVerifier {
    pub fn new(config: OidcJwtVerifierConfig) -> anyhow::Result<Self> {
        anyhow::ensure!(
            !config.audience.trim().is_empty(),
            "OIDC audience must not be empty"
        );
        anyhow::ensure!(
            config.jwks_cache_ttl <= config.jwks_stale_ttl,
            "OIDC JWKS stale TTL must not be shorter than cache TTL"
        );
        anyhow::ensure!(
            !config.allowed_algorithms.is_empty()
                && !config
                    .allowed_algorithms
                    .iter()
                    .any(|algorithm| algorithm == &Algorithm::HS256),
            "OIDC must use an explicit asymmetric signing algorithm"
        );
        Ok(Self {
            discovery: OidcDiscoveryCache::new(config.provider.request_timeout)?,
            client: reqwest::Client::builder()
                .timeout(config.provider.request_timeout)
                .build()?,
            config,
            jwks: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    async fn load_jwks(&self, uri: &str, force_refresh: bool) -> Result<JwkSet, AuthError> {
        if !force_refresh {
            if let Some(cached) = self.jwks.read().await.get(uri) {
                if cached.fetched_at.elapsed() <= self.config.jwks_cache_ttl {
                    return Ok(cached.set.clone());
                }
            }
        }

        let fetched = self
            .client
            .get(uri)
            .send()
            .await
            .map_err(|_| AuthError::ProviderUnavailable)
            .and_then(|response| {
                response
                    .error_for_status()
                    .map_err(|_| AuthError::ProviderUnavailable)
            });
        let fetched = match fetched {
            Ok(response) => response
                .json::<JwkSet>()
                .await
                .map_err(|_| AuthError::ProviderUnavailable),
            Err(error) => Err(error),
        };
        match fetched {
            Ok(set) => {
                let mut cache = self.jwks.write().await;
                if cache.len() >= MAX_CACHED_JWKS_DOCUMENTS && !cache.contains_key(uri) {
                    if let Some(oldest) = cache
                        .iter()
                        .min_by_key(|(_, entry)| entry.fetched_at)
                        .map(|(key, _)| key.clone())
                    {
                        cache.remove(&oldest);
                    }
                }
                cache.insert(
                    uri.to_string(),
                    CachedJwks {
                        set: set.clone(),
                        fetched_at: Instant::now(),
                    },
                );
                Ok(set)
            }
            Err(error) => {
                if let Some(cached) = self.jwks.read().await.get(uri) {
                    if cached.fetched_at.elapsed() <= self.config.jwks_stale_ttl {
                        return Ok(cached.set.clone());
                    }
                }
                Err(error)
            }
        }
    }

    async fn decoding_key(&self, uri: &str, key_id: &str) -> Result<DecodingKey, AuthError> {
        let current = self.load_jwks(uri, false).await?;
        if let Some(key) = current.find(key_id) {
            return DecodingKey::from_jwk(key).map_err(|_| AuthError::UnknownKey);
        }
        let refreshed = self.load_jwks(uri, true).await?;
        let key = refreshed.find(key_id).ok_or(AuthError::UnknownKey)?;
        DecodingKey::from_jwk(key).map_err(|_| AuthError::UnknownKey)
    }
}

#[async_trait::async_trait]
impl BearerTokenVerifier for OidcJwtVerifier {
    async fn verify(&self, token: &str) -> Result<AuthPrincipal, AuthError> {
        let header = decode_header(token).map_err(map_jwt_error)?;
        let key_id = header.kid.as_deref().ok_or(AuthError::UnknownKey)?;
        if !self.config.allowed_algorithms.contains(&header.alg) {
            return Err(AuthError::InvalidSignature);
        }
        let document = self
            .discovery
            .discover(&self.config.provider)
            .await
            .map_err(|_| AuthError::ProviderUnavailable)?;
        let key = self.decoding_key(&document.jwks_uri, key_id).await?;
        let mut validation = Validation::new(header.alg);
        validation.validate_exp = true;
        validation.leeway = self.config.clock_skew.as_secs();
        validation.set_issuer(&[self.config.provider.issuer.as_str()]);
        validation.set_audience(&[self.config.audience.as_str()]);
        let claims = decode::<Claims>(token, &key, &validation)
            .map_err(map_jwt_error)?
            .claims;
        if self.config.revoked_token_ids.contains(&claims.jti) {
            return Err(AuthError::Revoked);
        }
        Ok(claims.into())
    }
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
            permissions: vec!["users:read".to_string()],
            scopes: vec!["profile".to_string()],
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
        assert_eq!(decoded.permissions, vec!["users:read"]);
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
            permissions: Vec::new(),
            scopes: Vec::new(),
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
        assert_eq!(
            verify_token_typed(&token, &rotated),
            Err(AuthError::Revoked)
        );
    }

    #[tokio::test]
    async fn local_verifier_returns_the_unified_principal() {
        let config = JwtConfig {
            jwt_keys: vec![JwtKey {
                id: "active".into(),
                secret: "secret".into(),
            }],
            jwt_active_key_id: "active".into(),
            jwt_issuer: "roze".into(),
            jwt_audience: "api".into(),
            jwt_expiration_secs: 60,
            jwt_clock_skew_secs: 0,
            revoked_token_ids: Vec::new(),
        };
        let token = issue_token(
            &Claims {
                sub: "user-1".into(),
                roles: vec!["admin".into()],
                tenant: Some("tenant-1".into()),
                permissions: vec!["orders:read".into()],
                scopes: vec!["orders".into()],
                iss: String::new(),
                aud: String::new(),
                jti: "token-1".into(),
                iat: 0,
                exp: 0,
            },
            &config,
        )
        .expect("token");
        let principal = LocalJwtVerifier::new(config)
            .verify(&token)
            .await
            .expect("principal");
        assert_eq!(principal.subject, "user-1");
        assert_eq!(principal.permissions, ["orders:read"]);
        assert_eq!(principal.scopes, ["orders"]);
        assert_eq!(principal.token_id.as_deref(), Some("token-1"));
    }

    #[test]
    fn parses_bearer_token() {
        assert_eq!(extract_bearer_token("Bearer abc.def"), Some("abc.def"));
        assert_eq!(extract_bearer_token("bearer abc.def"), Some("abc.def"));
        assert_eq!(extract_bearer_token("Token abc.def"), None);
    }

    #[test]
    fn debug_redacts_jwt_secret() {
        let key = JwtKey {
            id: "active".into(),
            secret: "super-secret-jwt-key".into(),
        };

        let rendered = format!("{key:?}");
        assert!(!rendered.contains("super-secret-jwt-key"));
        assert!(rendered.contains("[REDACTED]"));
    }
}
