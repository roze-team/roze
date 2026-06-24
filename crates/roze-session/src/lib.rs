use std::{collections::BTreeMap, sync::Arc};

use async_trait::async_trait;
use dashmap::DashMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    pub id: String,
    pub subject: String,
    pub user_id: Option<String>,
    pub roles: Vec<String>,
    pub metadata: BTreeMap<String, String>,
    pub issued_at: i64,
    pub expires_at: Option<i64>,
}

impl Session {
    pub fn new(id: impl Into<String>, subject: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            subject: subject.into(),
            user_id: None,
            roles: Vec::new(),
            metadata: BTreeMap::new(),
            issued_at: unix_seconds_now(),
            expires_at: None,
        }
    }

    pub fn with_user_id(mut self, user_id: impl Into<String>) -> Self {
        self.user_id = Some(user_id.into());
        self
    }

    pub fn add_role(mut self, role: impl Into<String>) -> Self {
        self.roles.push(role.into());
        self
    }

    pub fn insert_metadata(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.metadata.insert(key.into(), value.into());
        self
    }

    pub fn with_expiration(mut self, expires_at: i64) -> Self {
        self.expires_at = Some(expires_at);
        self
    }

    pub fn is_expired(&self) -> bool {
        self.expires_at
            .map(|expires_at| unix_seconds_now() >= expires_at)
            .unwrap_or(false)
    }
}

#[async_trait]
pub trait SessionStore: Send + Sync {
    async fn get(&self, session_id: &str) -> anyhow::Result<Option<Session>>;
    async fn upsert(&self, session: Session) -> anyhow::Result<()>;
    async fn delete(&self, session_id: &str) -> anyhow::Result<()>;
}

#[derive(Debug, Clone, Default)]
pub struct InMemorySessionStore {
    sessions: Arc<DashMap<String, Session>>,
}

impl InMemorySessionStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl SessionStore for InMemorySessionStore {
    async fn get(&self, session_id: &str) -> anyhow::Result<Option<Session>> {
        if let Some(session) = self.sessions.get(session_id) {
            if session.is_expired() {
                drop(session);
                self.sessions.remove(session_id);
                return Ok(None);
            }
            return Ok(Some(session.clone()));
        }
        Ok(None)
    }

    async fn upsert(&self, session: Session) -> anyhow::Result<()> {
        self.sessions.insert(session.id.clone(), session);
        Ok(())
    }

    async fn delete(&self, session_id: &str) -> anyhow::Result<()> {
        self.sessions.remove(session_id);
        Ok(())
    }
}

pub fn unix_seconds_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn stores_and_expires_sessions() {
        let store = InMemorySessionStore::new();
        let session = Session::new("sid-1", "subject-1").with_user_id("u1");
        store.upsert(session.clone()).await.expect("upsert");
        let loaded = store.get("sid-1").await.expect("get").expect("session");
        assert_eq!(loaded.user_id.as_deref(), Some("u1"));
        store
            .upsert(session.with_expiration(unix_seconds_now() - 1))
            .await
            .expect("upsert expired");
        assert!(store.get("sid-1").await.expect("get").is_none());
    }
}
