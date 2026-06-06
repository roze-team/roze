use serde::{Deserialize, Serialize};

use roze_jwt::Claims;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuthPrincipal {
    pub subject: String,
    #[serde(default)]
    pub roles: Vec<String>,
    #[serde(default)]
    pub tenant: Option<String>,
}

impl From<Claims> for AuthPrincipal {
    fn from(value: Claims) -> Self {
        Self {
            subject: value.sub,
            roles: value.roles,
            tenant: value.tenant,
        }
    }
}

pub fn principal_from_claims(claims: &Claims) -> AuthPrincipal {
    AuthPrincipal {
        subject: claims.sub.clone(),
        roles: claims.roles.clone(),
        tenant: claims.tenant.clone(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use roze_jwt::Claims;

    #[test]
    fn converts_and_checks_roles() {
        let claims = Claims {
            sub: "user-1".into(),
            roles: vec!["admin".into(), "ops".into()],
            tenant: Some("acme".into()),
            iss: "roze".into(),
            iat: 1,
            exp: 2,
        };
        let principal = principal_from_claims(&claims);
        assert!(has_role(&principal, "admin"));
        assert!(has_any_role(&principal, &["support", "ops"]));
        assert!(belongs_to_tenant(&principal, "acme"));
        assert!(is_subject(&principal, "user-1"));
    }
}
