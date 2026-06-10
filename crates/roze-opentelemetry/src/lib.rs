use std::collections::HashMap;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TraceContext {
    pub trace_id: String,
    pub span_id: Option<String>,
    pub sampled: bool,
    pub baggage: HashMap<String, String>,
}

impl TraceContext {
    pub fn new(trace_id: impl Into<String>) -> Self {
        Self {
            trace_id: trace_id.into(),
            span_id: None,
            sampled: true,
            baggage: HashMap::new(),
        }
    }

    pub fn with_span_id(mut self, span_id: impl Into<String>) -> Self {
        self.span_id = Some(span_id.into());
        self
    }

    pub fn inject_headers(&self) -> HashMap<String, String> {
        let mut headers = HashMap::new();
        headers.insert("x-trace-id".to_string(), self.trace_id.clone());
        if let Some(span_id) = &self.span_id {
            headers.insert("x-span-id".to_string(), span_id.clone());
        }
        headers.insert("x-trace-sampled".to_string(), self.sampled.to_string());
        if !self.baggage.is_empty() {
            let baggage = self
                .baggage
                .iter()
                .map(|(key, value)| format!("{key}={value}"))
                .collect::<Vec<_>>()
                .join(",");
            headers.insert("baggage".to_string(), baggage);
        }
        headers.insert("traceparent".to_string(), self.traceparent());
        headers
    }

    pub fn from_headers(headers: &HashMap<String, String>) -> Option<Self> {
        let trace_id = headers.get("x-trace-id").cloned().or_else(|| {
            Self::from_traceparent(headers.get("traceparent")?)?
                .trace_id
                .into()
        })?;
        let span_id = headers.get("x-span-id").cloned();
        let sampled = headers
            .get("x-trace-sampled")
            .map(|value| value == "true")
            .unwrap_or(true);
        let baggage = headers
            .get("baggage")
            .map(|value| {
                value
                    .split(',')
                    .filter_map(|pair| pair.split_once('='))
                    .map(|(key, value)| (key.trim().to_string(), value.trim().to_string()))
                    .collect::<HashMap<_, _>>()
            })
            .unwrap_or_default();
        Some(Self {
            trace_id,
            span_id,
            sampled,
            baggage,
        })
    }

    pub fn with_baggage(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.baggage.insert(key.into(), value.into());
        self
    }

    pub fn traceparent(&self) -> String {
        let span_id = self
            .span_id
            .clone()
            .unwrap_or_else(|| "0000000000000000".to_string());
        format!(
            "00-{}-{}-{:02x}",
            pad_trace_id(&self.trace_id),
            pad_span_id(&span_id),
            u8::from(self.sampled)
        )
    }

    pub fn from_traceparent(value: &str) -> Option<Self> {
        let parts: Vec<_> = value.split('-').collect();
        if parts.len() != 4 {
            return None;
        }
        Some(Self {
            trace_id: trim_hex(parts[1]).to_string(),
            span_id: Some(trim_hex(parts[2]).to_string()),
            sampled: parts[3] != "00",
            baggage: HashMap::new(),
        })
    }
}

pub fn trace_headers(trace_id: impl Into<String>) -> HashMap<String, String> {
    TraceContext::new(trace_id).inject_headers()
}

fn pad_trace_id(trace_id: &str) -> String {
    let mut value = trim_hex(trace_id).to_string();
    while value.len() < 32 {
        value.insert(0, '0');
    }
    value.truncate(32);
    value
}

fn pad_span_id(span_id: &str) -> String {
    let mut value = trim_hex(span_id).to_string();
    while value.len() < 16 {
        value.insert(0, '0');
    }
    value.truncate(16);
    value
}

fn trim_hex(value: &str) -> &str {
    value.trim_start_matches('0')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_headers() {
        let ctx = TraceContext::new("trace-1")
            .with_span_id("span-1")
            .with_baggage("user", "42");
        let headers = ctx.inject_headers();
        let restored = TraceContext::from_headers(&headers).expect("restore");
        assert_eq!(restored.trace_id, "trace-1");
        assert_eq!(restored.span_id.as_deref(), Some("span-1"));
        assert_eq!(restored.baggage.get("user").map(String::as_str), Some("42"));
    }
}
