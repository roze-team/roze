use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fmt,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::RwLock;

const MAX_CACHED_OIDC_PROVIDERS: usize = 32;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuthPrincipal {
    pub subject: String,
    #[serde(default)]
    pub roles: Vec<String>,
    #[serde(default)]
    pub tenant: Option<String>,
    #[serde(default)]
    pub permissions: Vec<String>,
    #[serde(default)]
    pub scopes: Vec<String>,
    #[serde(default)]
    pub token_id: Option<String>,
    #[serde(default)]
    pub issuer: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AuthError {
    #[error("authorization token is malformed")]
    Malformed,
    #[error("authorization token has expired")]
    Expired,
    #[error("authorization token issuer is not trusted")]
    WrongIssuer,
    #[error("authorization token audience is not accepted")]
    WrongAudience,
    #[error("authorization token signing key is not trusted")]
    UnknownKey,
    #[error("authorization token has been revoked")]
    Revoked,
    #[error("authorization token signature is invalid")]
    InvalidSignature,
    #[error("identity provider is unavailable")]
    ProviderUnavailable,
}

impl AuthError {
    pub const fn code(self) -> &'static str {
        match self {
            Self::Malformed => "auth.token_malformed",
            Self::Expired => "auth.token_expired",
            Self::WrongIssuer => "auth.wrong_issuer",
            Self::WrongAudience => "auth.wrong_audience",
            Self::UnknownKey => "auth.unknown_signing_key",
            Self::Revoked => "auth.token_revoked",
            Self::InvalidSignature => "auth.invalid_signature",
            Self::ProviderUnavailable => "auth.provider_unavailable",
        }
    }
}

#[async_trait::async_trait]
pub trait BearerTokenVerifier: Send + Sync + std::fmt::Debug {
    async fn verify(&self, token: &str) -> Result<AuthPrincipal, AuthError>;
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OidcDiscoveryDocument {
    pub issuer: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub jwks_uri: String,
}

impl OidcDiscoveryDocument {
    pub fn validate(&self, expected_issuer: &str) -> Result<(), &'static str> {
        if self.issuer != expected_issuer {
            return Err("OIDC issuer does not match configured issuer");
        }
        for endpoint in [
            &self.authorization_endpoint,
            &self.token_endpoint,
            &self.jwks_uri,
        ] {
            if !endpoint.starts_with("https://") {
                return Err("OIDC endpoints must use HTTPS");
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct OidcProviderConfig {
    pub issuer: String,
    pub discovery_url: String,
    pub request_timeout: Duration,
    pub cache_ttl: Duration,
    pub stale_ttl: Duration,
}

#[derive(Debug, Clone)]
struct CachedDiscovery {
    document: OidcDiscoveryDocument,
    fetched_at: Instant,
}

#[derive(Debug, Clone)]
pub struct OidcDiscoveryCache {
    client: reqwest::Client,
    cached: Arc<RwLock<HashMap<String, CachedDiscovery>>>,
}

impl OidcDiscoveryCache {
    pub fn new(request_timeout: Duration) -> anyhow::Result<Self> {
        Ok(Self {
            client: reqwest::Client::builder()
                .timeout(request_timeout)
                .build()?,
            cached: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    pub async fn discover(
        &self,
        config: &OidcProviderConfig,
    ) -> anyhow::Result<OidcDiscoveryDocument> {
        anyhow::ensure!(
            config.discovery_url.starts_with("https://"),
            "OIDC discovery URL must use HTTPS"
        );
        anyhow::ensure!(
            config.cache_ttl <= config.stale_ttl,
            "OIDC stale TTL must not be shorter than cache TTL"
        );
        let cache_key = format!("{}\n{}", config.issuer, config.discovery_url);
        if let Some(cached) = self.cached.read().await.get(&cache_key) {
            if cached.fetched_at.elapsed() <= config.cache_ttl {
                return Ok(cached.document.clone());
            }
        }

        let fetched = async {
            let document = self
                .client
                .get(&config.discovery_url)
                .timeout(config.request_timeout)
                .send()
                .await?
                .error_for_status()?
                .json::<OidcDiscoveryDocument>()
                .await?;
            document
                .validate(&config.issuer)
                .map_err(anyhow::Error::msg)?;
            Ok::<_, anyhow::Error>(document)
        }
        .await;

        match fetched {
            Ok(document) => {
                let mut cached = self.cached.write().await;
                if cached.len() >= MAX_CACHED_OIDC_PROVIDERS && !cached.contains_key(&cache_key) {
                    if let Some(oldest) = cached
                        .iter()
                        .min_by_key(|(_, entry)| entry.fetched_at)
                        .map(|(key, _)| key.clone())
                    {
                        cached.remove(&oldest);
                    }
                }
                cached.insert(
                    cache_key.clone(),
                    CachedDiscovery {
                        document: document.clone(),
                        fetched_at: Instant::now(),
                    },
                );
                Ok(document)
            }
            Err(error) => {
                if let Some(cached) = self.cached.read().await.get(&cache_key) {
                    if cached.fetched_at.elapsed() <= config.stale_ttl {
                        return Ok(cached.document.clone());
                    }
                }
                Err(error)
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OAuth2Policy {
    pub client_id: String,
    pub redirect_uris: Vec<String>,
    pub scopes: Vec<String>,
    #[serde(default)]
    pub require_pkce: bool,
}

impl OAuth2Policy {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.client_id.trim().is_empty() {
            return Err("OAuth2 client ID must not be empty");
        }
        if self.redirect_uris.is_empty()
            || self
                .redirect_uris
                .iter()
                .any(|uri| !uri.starts_with("https://"))
        {
            return Err("OAuth2 redirect URIs must use HTTPS");
        }
        if self.scopes.is_empty() || self.scopes.iter().any(|scope| scope.trim().is_empty()) {
            return Err("OAuth2 scopes must not be empty");
        }
        if !self.require_pkce {
            return Err("OAuth2 authorization-code flows must require PKCE");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MtlsIdentity {
    pub subject: String,
    pub issuer: String,
    pub serial_number: String,
    #[serde(default)]
    pub tenant: Option<String>,
    #[serde(default)]
    pub roles: Vec<String>,
    #[serde(default)]
    pub permissions: Vec<String>,
    #[serde(default)]
    pub scopes: Vec<String>,
}

impl From<MtlsIdentity> for AuthPrincipal {
    fn from(identity: MtlsIdentity) -> Self {
        Self {
            subject: identity.subject,
            roles: identity.roles,
            tenant: identity.tenant,
            permissions: identity.permissions,
            scopes: identity.scopes,
            token_id: Some(identity.serial_number),
            issuer: Some(identity.issuer),
        }
    }
}

pub fn principal(
    subject: impl Into<String>,
    roles: impl Into<Vec<String>>,
    tenant: Option<String>,
) -> AuthPrincipal {
    AuthPrincipal {
        subject: subject.into(),
        roles: roles.into(),
        tenant,
        permissions: Vec::new(),
        scopes: Vec::new(),
        token_id: None,
        issuer: None,
    }
}

pub fn has_role(principal: &AuthPrincipal, role: &str) -> bool {
    principal.roles.iter().any(|item| item == role)
}

pub fn has_any_role(principal: &AuthPrincipal, roles: &[&str]) -> bool {
    roles.iter().any(|role| has_role(principal, role))
}

pub fn belongs_to_tenant(principal: &AuthPrincipal, tenant: &str) -> bool {
    principal.tenant.as_deref() == Some(tenant)
}

pub fn is_subject(principal: &AuthPrincipal, subject: &str) -> bool {
    principal.subject == subject
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ApiKeyConfig {
    #[serde(default = "default_api_key_header")]
    pub header: String,
    #[serde(default)]
    pub keys: Vec<ApiKeyCredential>,
}

impl Default for ApiKeyConfig {
    fn default() -> Self {
        Self {
            header: default_api_key_header(),
            keys: Vec::new(),
        }
    }
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ApiKeyCredential {
    pub key: String,
    pub subject: String,
    #[serde(default)]
    pub roles: Vec<String>,
    #[serde(default)]
    pub tenant: Option<String>,
    #[serde(default)]
    pub permissions: Vec<String>,
    #[serde(default)]
    pub scopes: Vec<String>,
}

impl fmt::Debug for ApiKeyCredential {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ApiKeyCredential")
            .field("key", &"[REDACTED]")
            .field("subject", &self.subject)
            .field("roles", &self.roles)
            .field("tenant", &self.tenant)
            .field("permissions", &self.permissions)
            .field("scopes", &self.scopes)
            .finish()
    }
}

pub fn verify_api_key(value: &str, config: &ApiKeyConfig) -> Option<AuthPrincipal> {
    config
        .keys
        .iter()
        .find(|credential| credential.key == value)
        .map(|credential| AuthPrincipal {
            subject: credential.subject.clone(),
            roles: credential.roles.clone(),
            tenant: credential.tenant.clone(),
            permissions: credential.permissions.clone(),
            scopes: credential.scopes.clone(),
            token_id: None,
            issuer: None,
        })
}

pub fn default_api_key_header() -> String {
    "x-api-key".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_and_checks_roles() {
        let principal = principal(
            "user-1",
            vec!["admin".to_string(), "ops".to_string()],
            Some("acme".to_string()),
        );
        assert!(has_role(&principal, "admin"));
        assert!(has_any_role(&principal, &["support", "ops"]));
        assert!(belongs_to_tenant(&principal, "acme"));
        assert!(is_subject(&principal, "user-1"));
    }

    #[test]
    fn verifies_api_key_credentials() {
        let config = ApiKeyConfig {
            header: "x-api-key".to_string(),
            keys: vec![ApiKeyCredential {
                key: "secret".to_string(),
                subject: "app-1".to_string(),
                roles: vec!["internal".to_string()],
                tenant: Some("acme".to_string()),
                permissions: vec!["orders:read".to_string()],
                scopes: vec!["orders".to_string()],
            }],
        };

        let principal = verify_api_key("secret", &config).expect("principal");

        assert_eq!(principal.subject, "app-1");
        assert!(has_role(&principal, "internal"));
        assert!(belongs_to_tenant(&principal, "acme"));
        assert_eq!(principal.permissions, ["orders:read"]);
        assert!(verify_api_key("bad", &config).is_none());
    }

    #[test]
    fn debug_redacts_api_key() {
        let credential = ApiKeyCredential {
            key: "super-secret-api-key".into(),
            subject: "user-1".into(),
            roles: vec![],
            tenant: None,
            permissions: vec![],
            scopes: vec![],
        };

        let rendered = format!("{credential:?}");
        assert!(!rendered.contains("super-secret-api-key"));
        assert!(rendered.contains("[REDACTED]"));
    }

    #[test]
    fn oidc_discovery_requires_exact_issuer_and_https() {
        let document = OidcDiscoveryDocument {
            issuer: "https://identity.example".into(),
            authorization_endpoint: "https://identity.example/authorize".into(),
            token_endpoint: "https://identity.example/token".into(),
            jwks_uri: "https://identity.example/jwks".into(),
        };
        assert!(document.validate("https://identity.example").is_ok());
        assert!(document.validate("https://other.example").is_err());
    }

    #[test]
    fn oauth2_policy_requires_https_pkce_and_scopes() {
        let policy = OAuth2Policy {
            client_id: "web".into(),
            redirect_uris: vec!["https://app.example/callback".into()],
            scopes: vec!["openid".into(), "profile".into()],
            require_pkce: true,
        };
        assert!(policy.validate().is_ok());

        let mut unsafe_policy = policy;
        unsafe_policy.require_pkce = false;
        assert_eq!(
            unsafe_policy.validate(),
            Err("OAuth2 authorization-code flows must require PKCE")
        );
    }

    #[test]
    fn authentication_errors_expose_stable_codes() {
        assert_eq!(AuthError::Expired.code(), "auth.token_expired");
        assert_eq!(
            AuthError::ProviderUnavailable.code(),
            "auth.provider_unavailable"
        );
    }
}
