use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuthPrincipal {
    pub subject: String,
    #[serde(default)]
    pub roles: Vec<String>,
    #[serde(default)]
    pub tenant: Option<String>,
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
}
