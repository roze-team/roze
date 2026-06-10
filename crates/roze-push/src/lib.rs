use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PushMessage {
    pub topic: String,
    #[serde(default)]
    pub metadata: HashMap<String, String>,
    pub payload: serde_json::Value,
}

impl PushMessage {
    pub fn new(topic: impl Into<String>, payload: serde_json::Value) -> Self {
        Self {
            topic: topic.into(),
            metadata: HashMap::new(),
            payload,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PushBus {
    topics: Arc<Mutex<HashMap<String, broadcast::Sender<PushMessage>>>>,
    capacity: usize,
}

impl Default for PushBus {
    fn default() -> Self {
        Self::new()
    }
}

impl PushBus {
    pub fn new() -> Self {
        Self {
            topics: Arc::new(Mutex::new(HashMap::new())),
            capacity: 256,
        }
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            topics: Arc::new(Mutex::new(HashMap::new())),
            capacity: capacity.max(1),
        }
    }

    fn sender_for(&self, topic: &str) -> broadcast::Sender<PushMessage> {
        let mut topics = self.topics.lock().expect("push bus lock poisoned");
        topics
            .entry(topic.to_string())
            .or_insert_with(|| broadcast::channel(self.capacity).0)
            .clone()
    }

    pub fn subscribe(&self, topic: &str) -> broadcast::Receiver<PushMessage> {
        self.sender_for(topic).subscribe()
    }

    pub fn publish(&self, message: PushMessage) -> anyhow::Result<usize> {
        Ok(self.sender_for(&message.topic).send(message)?)
    }

    pub fn publish_json(
        &self,
        topic: impl Into<String>,
        payload: serde_json::Value,
    ) -> anyhow::Result<usize> {
        self.publish(PushMessage::new(topic, payload))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn makes_message() {
        let msg = PushMessage::new("alerts", serde_json::json!({"level":"info"}));
        assert_eq!(msg.topic, "alerts");
    }
}
