use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use tracing::Span;

pub const TRACE_ID_HEADER: &str = "x-trace-id";

pub fn generate_trace_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{nanos:x}-{seq:x}")
}

pub fn service_span(service: impl AsRef<str>) -> Span {
    tracing::info_span!("service", name = service.as_ref())
}

pub fn request_span(
    method: impl AsRef<str>,
    path: impl AsRef<str>,
    trace_id: impl AsRef<str>,
) -> Span {
    tracing::info_span!(
        "request",
        method = method.as_ref(),
        path = path.as_ref(),
        trace_id = trace_id.as_ref()
    )
}

pub fn record_trace_id(span: &Span, trace_id: impl AsRef<str>) {
    span.record("trace_id", tracing::field::display(trace_id.as_ref()));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_trace_helpers() {
        let trace_id = generate_trace_id();
        assert!(!trace_id.is_empty());

        let span = service_span("roze");
        record_trace_id(&span, "trace-123");
        let _guard = span.enter();
        let _ = request_span("GET", "/healthz", "trace-123");
    }
}
