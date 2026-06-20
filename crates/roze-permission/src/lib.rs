use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Permission(pub String);

impl Permission {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionSet {
    grants: BTreeSet<String>,
    denies: BTreeSet<String>,
}

impl PermissionSet {
    pub fn grant(&mut self, permission: impl Into<String>) {
        self.grants.insert(permission.into());
    }

    pub fn deny(&mut self, permission: impl Into<String>) {
        self.denies.insert(permission.into());
    }

    pub fn grants(&self) -> &BTreeSet<String> {
        &self.grants
    }

    pub fn denies(&self) -> &BTreeSet<String> {
        &self.denies
    }

    pub fn allows(&self, permission: impl AsRef<str>) -> bool {
        let permission = permission.as_ref();
        self.grants.contains(permission) && !self.denies.contains(permission)
    }

    pub fn contains_all<S>(&self, permissions: &[S]) -> bool
    where
        S: AsRef<str>,
    {
        permissions
            .iter()
            .all(|permission| self.allows(permission.as_ref()))
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionRule {
    pub require_all: Vec<String>,
    pub allow_any: Vec<String>,
    pub deny_any: Vec<String>,
}

impl PermissionRule {
    pub fn allows(&self, set: &PermissionSet) -> bool {
        if self
            .deny_any
            .iter()
            .any(|permission| set.grants.contains(permission) || set.denies.contains(permission))
        {
            return false;
        }

        if !self.require_all.is_empty()
            && !self
                .require_all
                .iter()
                .all(|permission| set.grants.contains(permission))
        {
            return false;
        }

        if self.allow_any.is_empty() {
            return true;
        }

        self.allow_any
            .iter()
            .any(|permission| set.grants.contains(permission))
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RolePolicy {
    pub roles: std::collections::BTreeMap<String, PermissionSet>,
}

impl RolePolicy {
    pub fn permissions_for_roles<I, S>(&self, roles: I) -> PermissionSet
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut set = PermissionSet::default();
        for role in roles {
            if let Some(role_set) = self.roles.get(role.as_ref()) {
                for grant in role_set.grants() {
                    set.grant(grant.clone());
                }
                for deny in role_set.denies() {
                    set.deny(deny.clone());
                }
            }
        }
        set
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Principal {
    pub subject: String,
    pub roles: Vec<String>,
    pub tenant: Option<String>,
    pub attributes: std::collections::BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttributeRule {
    pub equals: std::collections::BTreeMap<String, String>,
}

impl AttributeRule {
    pub fn allows(&self, principal: &Principal) -> bool {
        self.equals
            .iter()
            .all(|(key, expected)| principal.attributes.get(key) == Some(expected))
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccessRule {
    pub permissions: PermissionRule,
    pub attributes: AttributeRule,
    pub tenant_required: Option<String>,
}

impl AccessRule {
    pub fn allows(&self, principal: &Principal, role_policy: &RolePolicy) -> bool {
        if let Some(required) = &self.tenant_required {
            if principal.tenant.as_ref() != Some(required) {
                return false;
            }
        }
        if !self.attributes.allows(principal) {
            return false;
        }
        let permissions = role_policy.permissions_for_roles(&principal.roles);
        self.permissions.allows(&permissions)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evaluates_permissions() {
        let mut set = PermissionSet::default();
        set.grant("orders:read");
        set.grant("orders:write");
        let allow_rule = PermissionRule {
            require_all: vec!["orders:read".into()],
            allow_any: vec!["orders:write".into()],
            deny_any: vec![],
        };
        assert!(allow_rule.allows(&set));

        let deny_rule = PermissionRule {
            require_all: vec!["orders:read".into()],
            allow_any: vec!["orders:write".into()],
            deny_any: vec!["orders:write".into()],
        };
        assert!(!deny_rule.allows(&set));
    }

    #[test]
    fn evaluates_role_tenant_and_attributes() {
        let mut admin = PermissionSet::default();
        admin.grant("orders:read");
        let policy = RolePolicy {
            roles: [("admin".to_string(), admin)].into_iter().collect(),
        };
        let principal = Principal {
            subject: "u1".into(),
            roles: vec!["admin".into()],
            tenant: Some("t1".into()),
            attributes: [("region".to_string(), "ap".to_string())]
                .into_iter()
                .collect(),
        };
        let rule = AccessRule {
            permissions: PermissionRule {
                require_all: vec!["orders:read".into()],
                ..Default::default()
            },
            attributes: AttributeRule {
                equals: [("region".to_string(), "ap".to_string())]
                    .into_iter()
                    .collect(),
            },
            tenant_required: Some("t1".into()),
        };

        assert!(rule.allows(&principal, &policy));
    }
}
