use std::collections::HashMap;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TraceContext {
    pub trace_id: String,
    pub span_id: Option<String>,
    pub sampled: bool,
}

impl TraceContext {
    pub fn new(trace_id: impl Into<String>) -> Self {
        Self {
            trace_id: trace_id.into(),
            span_id: None,
            sampled: true,
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
        headers
    }

    pub fn from_headers(headers: &HashMap<String, String>) -> Option<Self> {
        let trace_id = headers.get("x-trace-id")?.clone();
        let span_id = headers.get("x-span-id").cloned();
        let sampled = headers
            .get("x-trace-sampled")
            .map(|value| value == "true")
            .unwrap_or(true);
        Some(Self {
            trace_id,
            span_id,
            sampled,
        })
    }
}

pub fn trace_headers(trace_id: impl Into<String>) -> HashMap<String, String> {
    TraceContext::new(trace_id).inject_headers()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_headers() {
        let ctx = TraceContext::new("trace-1").with_span_id("span-1");
        let headers = ctx.inject_headers();
        let restored = TraceContext::from_headers(&headers).expect("restore");
        assert_eq!(restored.trace_id, "trace-1");
        assert_eq!(restored.span_id.as_deref(), Some("span-1"));
    }
}
