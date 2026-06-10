use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum SessionEventKind {
    Joined,
    Left,
    Message,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionEvent {
    pub session_id: String,
    pub kind: SessionEventKind,
    #[serde(default)]
    pub payload: serde_json::Value,
}

impl SessionEvent {
    pub fn joined(session_id: impl Into<String>) -> Self {
        Self {
            session_id: session_id.into(),
            kind: SessionEventKind::Joined,
            payload: serde_json::Value::Null,
        }
    }

    pub fn left(session_id: impl Into<String>) -> Self {
        Self {
            session_id: session_id.into(),
            kind: SessionEventKind::Left,
            payload: serde_json::Value::Null,
        }
    }

    pub fn message(session_id: impl Into<String>, payload: serde_json::Value) -> Self {
        Self {
            session_id: session_id.into(),
            kind: SessionEventKind::Message,
            payload,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SessionHub {
    rooms: Arc<Mutex<HashMap<String, broadcast::Sender<SessionEvent>>>>,
    capacity: usize,
}

impl Default for SessionHub {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionHub {
    pub fn new() -> Self {
        Self {
            rooms: Arc::new(Mutex::new(HashMap::new())),
            capacity: 256,
        }
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            rooms: Arc::new(Mutex::new(HashMap::new())),
            capacity: capacity.max(1),
        }
    }

    fn room_for(&self, room: &str) -> broadcast::Sender<SessionEvent> {
        let mut rooms = self.rooms.lock().expect("session hub lock poisoned");
        rooms
            .entry(room.to_string())
            .or_insert_with(|| broadcast::channel(self.capacity).0)
            .clone()
    }

    pub fn subscribe(&self, room: &str) -> broadcast::Receiver<SessionEvent> {
        self.room_for(room).subscribe()
    }

    pub fn publish(&self, room: &str, event: SessionEvent) -> anyhow::Result<usize> {
        Ok(self.room_for(room).send(event)?)
    }

    pub fn join(&self, room: &str, session_id: impl Into<String>) -> anyhow::Result<usize> {
        self.publish(room, SessionEvent::joined(session_id))
    }

    pub fn leave(&self, room: &str, session_id: impl Into<String>) -> anyhow::Result<usize> {
        self.publish(room, SessionEvent::left(session_id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_events() {
        let event = SessionEvent::message("s1", serde_json::json!({"hello":"world"}));
        assert_eq!(event.session_id, "s1");
    }
}
