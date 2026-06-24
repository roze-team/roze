use std::{collections::HashMap, sync::Arc, time::Instant};

use dashmap::DashMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WsFrame {
    Text(String),
    Binary(Vec<u8>),
    Close,
}

#[derive(Debug, Clone)]
pub struct WsSession {
    pub id: String,
    pub peer: Option<String>,
    pub headers: HashMap<String, String>,
    connected_at: Instant,
}

impl WsSession {
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            peer: None,
            headers: HashMap::new(),
            connected_at: Instant::now(),
        }
    }

    pub fn uptime(&self) -> std::time::Duration {
        self.connected_at.elapsed()
    }
}

#[derive(Debug, Clone, Default)]
pub struct WsHub {
    sessions: Arc<DashMap<String, WsSession>>,
}

impl WsHub {
    pub fn register(&self, session: WsSession) {
        self.sessions.insert(session.id.clone(), session);
    }

    pub fn disconnect(&self, id: &str) -> Option<WsSession> {
        self.sessions.remove(id).map(|(_, session)| session)
    }

    pub fn get(&self, id: &str) -> Option<WsSession> {
        self.sessions.get(id).map(|session| session.clone())
    }

    pub fn len(&self) -> usize {
        self.sessions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.sessions.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registers_sessions() {
        let hub = WsHub::default();
        assert!(hub.is_empty());
        hub.register(WsSession::new("s1"));
        assert_eq!(hub.len(), 1);
        assert!(!hub.is_empty());
        assert!(hub.get("s1").is_some());
    }
}
