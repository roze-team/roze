use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NatsConfig {
    pub servers: Vec<String>,
    #[serde(default)]
    pub client_name: Option<String>,
    #[serde(default)]
    pub subject_prefix: String,
}

impl NatsConfig {
    pub fn subject_name(&self, subject: impl AsRef<str>) -> String {
        if self.subject_prefix.is_empty() {
            subject.as_ref().to_string()
        } else {
            format!("{}.{}", self.subject_prefix, subject.as_ref())
        }
    }

    pub fn servers_csv(&self) -> String {
        self.servers.join(",")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NatsMessage {
    pub subject: String,
    #[serde(default)]
    pub reply_to: Option<String>,
    #[serde(default)]
    pub headers: std::collections::HashMap<String, String>,
    pub payload: serde_json::Value,
}

impl NatsMessage {
    pub fn new(subject: impl Into<String>, payload: serde_json::Value) -> Self {
        Self {
            subject: subject.into(),
            reply_to: None,
            headers: std::collections::HashMap::new(),
            payload,
        }
    }

    pub fn to_event(self) -> roze_eventbus::EventEnvelope {
        let mut event = roze_eventbus::EventEnvelope::new(self.subject, self.payload);
        event.headers = self.headers;
        event
    }

    pub fn from_event(event: roze_eventbus::EventEnvelope) -> Self {
        Self {
            subject: event.topic,
            reply_to: None,
            headers: event.headers,
            payload: event.payload,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_servers_and_subjects() {
        let cfg = NatsConfig {
            servers: vec!["n1:4222".into(), "n2:4222".into()],
            client_name: Some("roze".into()),
            subject_prefix: "app".into(),
        };
        assert_eq!(cfg.servers_csv(), "n1:4222,n2:4222");
        assert_eq!(cfg.subject_name("orders"), "app.orders");
    }
}
