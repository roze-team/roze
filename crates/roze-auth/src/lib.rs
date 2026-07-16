use serde::{Deserialize, Serialize};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::RwLock;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuthPrincipal {
    pub subject: String,
    #[serde(default)]
    pub roles: Vec<String>,
    #[serde(default)]
    pub tenant: Option<String>,
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
    cached: Arc<RwLock<Option<CachedDiscovery>>>,
}

impl OidcDiscoveryCache {
    pub fn new(request_timeout: Duration) -> anyhow::Result<Self> {
        Ok(Self {
            client: reqwest::Client::builder()
                .timeout(request_timeout)
                .build()?,
            cached: Arc::new(RwLock::new(None)),
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
        if let Some(cached) = self.cached.read().await.as_ref() {
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
                *self.cached.write().await = Some(CachedDiscovery {
                    document: document.clone(),
                    fetched_at: Instant::now(),
                });
                Ok(document)
            }
            Err(error) => {
                if let Some(cached) = self.cached.read().await.as_ref() {
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MtlsIdentity {
    pub subject: String,
    pub issuer: String,
    pub serial_number: String,
    #[serde(default)]
    pub tenant: Option<String>,
    #[serde(default)]
    pub roles: Vec<String>,
}

impl From<MtlsIdentity> for AuthPrincipal {
    fn from(identity: MtlsIdentity) -> Self {
        Self {
            subject: identity.subject,
            roles: identity.roles,
            tenant: identity.tenant,
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ApiKeyCredential {
    pub key: String,
    pub subject: String,
    #[serde(default)]
    pub roles: Vec<String>,
    #[serde(default)]
    pub tenant: Option<String>,
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
            }],
        };

        let principal = verify_api_key("secret", &config).expect("principal");

        assert_eq!(principal.subject, "app-1");
        assert!(has_role(&principal, "internal"));
        assert!(belongs_to_tenant(&principal, "acme"));
        assert!(verify_api_key("bad", &config).is_none());
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
}
