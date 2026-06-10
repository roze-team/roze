use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Instant,
};

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
    sessions: Arc<Mutex<HashMap<String, WsSession>>>,
}

impl WsHub {
    pub fn register(&self, session: WsSession) {
        self.sessions
            .lock()
            .expect("ws hub lock poisoned")
            .insert(session.id.clone(), session);
    }

    pub fn disconnect(&self, id: &str) -> Option<WsSession> {
        self.sessions
            .lock()
            .expect("ws hub lock poisoned")
            .remove(id)
    }

    pub fn get(&self, id: &str) -> Option<WsSession> {
        self.sessions
            .lock()
            .expect("ws hub lock poisoned")
            .get(id)
            .cloned()
    }

    pub fn len(&self) -> usize {
        self.sessions.lock().expect("ws hub lock poisoned").len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registers_sessions() {
        let hub = WsHub::default();
        hub.register(WsSession::new("s1"));
        assert_eq!(hub.len(), 1);
        assert!(hub.get("s1").is_some());
    }
}
