use std::collections::BTreeSet;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Permission(pub String);

impl Permission {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
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

#[derive(Debug, Clone, Default, PartialEq, Eq)]
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
}
