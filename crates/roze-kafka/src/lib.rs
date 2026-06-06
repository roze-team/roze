use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct KafkaConfig {
    pub brokers: Vec<String>,
    #[serde(default)]
    pub client_id: Option<String>,
    #[serde(default)]
    pub group_id: Option<String>,
    #[serde(default)]
    pub topic_prefix: String,
}

impl KafkaConfig {
    pub fn topic_name(&self, topic: impl AsRef<str>) -> String {
        if self.topic_prefix.is_empty() {
            topic.as_ref().to_string()
        } else {
            format!("{}.{}", self.topic_prefix, topic.as_ref())
        }
    }

    pub fn brokers_csv(&self) -> String {
        self.brokers.join(",")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct KafkaRecord {
    pub topic: String,
    #[serde(default)]
    pub key: Option<String>,
    #[serde(default)]
    pub headers: std::collections::HashMap<String, String>,
    pub payload: serde_json::Value,
}

impl KafkaRecord {
    pub fn new(topic: impl Into<String>, payload: serde_json::Value) -> Self {
        Self {
            topic: topic.into(),
            key: None,
            headers: std::collections::HashMap::new(),
            payload,
        }
    }

    pub fn to_event(self) -> roze_eventbus::EventEnvelope {
        let mut event = roze_eventbus::EventEnvelope::new(self.topic, self.payload);
        event.key = self.key;
        event.headers = self.headers;
        event
    }

    pub fn from_event(event: roze_eventbus::EventEnvelope) -> Self {
        Self {
            topic: event.topic,
            key: event.key,
            headers: event.headers,
            payload: event.payload,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_brokers_and_topics() {
        let cfg = KafkaConfig {
            brokers: vec!["k1:9092".into(), "k2:9092".into()],
            client_id: Some("roze".into()),
            group_id: None,
            topic_prefix: "app".into(),
        };
        assert_eq!(cfg.brokers_csv(), "k1:9092,k2:9092");
        assert_eq!(cfg.topic_name("orders"), "app.orders");
    }
}
