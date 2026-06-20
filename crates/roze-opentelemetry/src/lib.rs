pub use opentelemetry_sdk::trace::SdkTracerProvider;

use std::collections::HashMap;

use opentelemetry::{
    global,
    propagation::{text_map_propagator::FieldIter, Extractor, Injector, TextMapPropagator},
    trace::{SpanContext, TraceContextExt, TraceFlags, TraceId, TraceState},
    Context, SpanId,
};
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::{propagation::TraceContextPropagator, trace::Sampler, Resource};
use roze_config::{ServiceConfig, TelemetryBatcher, TelemetryConfig, TelemetryPropagator};

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
        headers.insert(
            roze_context::TRACE_ID_HEADER.to_string(),
            self.trace_id.clone(),
        );
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
        let trace_id = headers
            .get(roze_context::TRACE_ID_HEADER)
            .cloned()
            .or_else(|| Some(Self::from_traceparent(headers.get("traceparent")?)?.trace_id))?;
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

pub fn build_tracer_provider(config: &ServiceConfig) -> anyhow::Result<Option<SdkTracerProvider>> {
    let Some(telemetry) = config.telemetry.as_ref() else {
        return Ok(None);
    };

    let Some(endpoint) = telemetry
        .endpoint
        .as_deref()
        .filter(|value| !value.is_empty())
    else {
        return Ok(None);
    };

    set_global_propagator(telemetry.propagator);

    let service_name = service_name(config, telemetry);
    let exporter = build_span_exporter(telemetry, endpoint)?;
    let provider = SdkTracerProvider::builder()
        .with_batch_exporter(exporter)
        .with_resource(
            Resource::builder()
                .with_service_name(service_name.to_string())
                .build(),
        )
        .with_sampler(Sampler::TraceIdRatioBased(clamp_sampler(telemetry.sampler)))
        .build();

    Ok(Some(provider))
}

pub fn set_global_propagator(propagator: TelemetryPropagator) {
    match propagator {
        TelemetryPropagator::TraceContext => {
            global::set_text_map_propagator(TraceContextPropagator::new());
        }
        TelemetryPropagator::Jaeger => {
            global::set_text_map_propagator(JaegerPropagator::new());
        }
    }
}

pub fn service_name<'a>(config: &'a ServiceConfig, telemetry: &'a TelemetryConfig) -> &'a str {
    telemetry.name.as_deref().unwrap_or(config.name.as_str())
}

fn build_span_exporter(
    telemetry: &TelemetryConfig,
    endpoint: &str,
) -> anyhow::Result<opentelemetry_otlp::SpanExporter> {
    let exporter = match telemetry.batcher {
        TelemetryBatcher::OtlpGrpc => opentelemetry_otlp::SpanExporter::builder()
            .with_tonic()
            .with_endpoint(endpoint)
            .build()?,
        TelemetryBatcher::OtlpHttp => opentelemetry_otlp::SpanExporter::builder()
            .with_http()
            .with_endpoint(endpoint)
            .build()?,
    };
    Ok(exporter)
}

fn clamp_sampler(sampler: f64) -> f64 {
    sampler.clamp(0.0, 1.0)
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

#[derive(Debug, Clone)]
pub struct JaegerPropagator {
    fields: Vec<String>,
}

impl JaegerPropagator {
    pub fn new() -> Self {
        Self {
            fields: vec!["uber-trace-id".to_string()],
        }
    }
}

impl Default for JaegerPropagator {
    fn default() -> Self {
        Self::new()
    }
}

impl TextMapPropagator for JaegerPropagator {
    fn inject_context(&self, cx: &Context, injector: &mut dyn Injector) {
        let span = cx.span();
        let span_context = span.span_context();
        if !span_context.is_valid() {
            return;
        }

        let flags = if span_context.trace_flags().is_sampled() {
            "1"
        } else {
            "0"
        };
        injector.set(
            "uber-trace-id",
            format!(
                "{:032x}:{:016x}:0:{flags}",
                span_context.trace_id(),
                span_context.span_id(),
            ),
        );
    }

    fn extract_with_context(&self, cx: &Context, extractor: &dyn Extractor) -> Context {
        let Some(value) = extractor.get("uber-trace-id") else {
            return cx.clone();
        };

        let Some(span_context) = parse_jaeger_trace_id(value) else {
            return cx.clone();
        };

        cx.with_remote_span_context(span_context)
    }

    fn fields(&self) -> FieldIter<'_> {
        FieldIter::new(&self.fields)
    }
}

fn parse_jaeger_trace_id(value: &str) -> Option<SpanContext> {
    let mut parts = value.split(':');
    let trace_id = TraceId::from_hex(parts.next()?).ok()?;
    let span_id = SpanId::from_hex(parts.next()?).ok()?;
    let _parent_span_id = parts.next()?;
    let flags = u8::from_str_radix(parts.next()?, 16).ok()?;
    if parts.next().is_some() {
        return None;
    }

    let trace_flags = if flags & 1 == 1 {
        TraceFlags::SAMPLED
    } else {
        TraceFlags::NOT_SAMPLED
    };
    let span_context =
        SpanContext::new(trace_id, span_id, trace_flags, true, TraceState::default());
    span_context.is_valid().then_some(span_context)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn parses_jaeger_trace_header() {
        let mut headers = HashMap::new();
        headers.insert(
            "uber-trace-id".to_string(),
            "5f467fe7bf42676c05e20ba4a90e448e:4c721bf33e3caf8f:0:1".to_string(),
        );

        let cx = JaegerPropagator::new().extract(&headers);
        let span_context = cx.span().span_context().clone();

        assert!(span_context.is_valid());
        assert!(span_context.is_remote());
        assert!(span_context.is_sampled());
        assert_eq!(
            span_context.trace_id().to_string(),
            "5f467fe7bf42676c05e20ba4a90e448e"
        );
        assert_eq!(span_context.span_id().to_string(), "4c721bf33e3caf8f");
    }

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
