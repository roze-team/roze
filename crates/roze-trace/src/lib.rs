use tracing::Span;

pub const TRACE_ID_HEADER: &str = "x-trace-id";

pub fn generate_trace_id() -> String {
    uuid::Uuid::now_v7().to_string()
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
        assert_eq!(
            uuid::Uuid::parse_str(&trace_id).unwrap().get_version_num(),
            7
        );

        let span = service_span("roze");
        record_trace_id(&span, "trace-123");
        let _guard = span.enter();
        let _ = request_span("GET", "/healthz", "trace-123");
    }
}
