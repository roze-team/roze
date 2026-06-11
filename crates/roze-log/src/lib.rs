use std::sync::OnceLock;

use opentelemetry::trace::TracerProvider as _;
use opentelemetry::{global, KeyValue};
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::{
    propagation::TraceContextPropagator,
    trace::{Sampler, TracerProvider},
    Resource,
};
use roze_config::{ServiceConfig, TelemetryBatcher, TelemetryConfig};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

static TRACER_PROVIDER: OnceLock<TracerProvider> = OnceLock::new();

pub fn init_tracing() {
    init_tracing_with_filter("info");
}

pub fn init_tracing_with_filter(filter: impl AsRef<str>) {
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(filter.as_ref()));

    let _ = tracing_subscriber::registry()
        .with(env_filter)
        .with(tracing_subscriber::fmt::layer())
        .try_init();
}

pub fn init_tracing_with_config(config: &ServiceConfig) -> anyhow::Result<()> {
    init_tracing_with_config_and_filter(config, "info")
}

pub fn init_tracing_with_config_and_filter(
    config: &ServiceConfig,
    filter: impl AsRef<str>,
) -> anyhow::Result<()> {
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(filter.as_ref()));

    let subscriber = tracing_subscriber::registry()
        .with(env_filter)
        .with(tracing_subscriber::fmt::layer());

    let Some(telemetry) = config.telemetry.as_ref() else {
        let _ = subscriber.try_init();
        return Ok(());
    };

    let Some(endpoint) = telemetry
        .endpoint
        .as_deref()
        .filter(|value| !value.is_empty())
    else {
        let _ = subscriber.try_init();
        return Ok(());
    };

    global::set_text_map_propagator(TraceContextPropagator::new());

    let service_name = telemetry.name.as_deref().unwrap_or(config.name.as_str());
    let exporter = build_span_exporter(telemetry, endpoint)?;
    let provider = TracerProvider::builder()
        .with_batch_exporter(exporter, opentelemetry_sdk::runtime::Tokio)
        .with_resource(Resource::new(vec![KeyValue::new(
            "service.name",
            service_name.to_string(),
        )]))
        .with_sampler(Sampler::TraceIdRatioBased(clamp_sampler(telemetry.sampler)))
        .build();
    let tracer = provider.tracer(service_name.to_string());

    let _ = TRACER_PROVIDER.set(provider.clone());
    global::set_tracer_provider(provider);

    let _ = subscriber
        .with(tracing_opentelemetry::layer().with_tracer(tracer))
        .try_init();

    Ok(())
}

fn build_span_exporter(
    telemetry: &TelemetryConfig,
    endpoint: &str,
) -> Result<opentelemetry_otlp::SpanExporter, opentelemetry::trace::TraceError> {
    match telemetry.batcher {
        TelemetryBatcher::OtlpGrpc => opentelemetry_otlp::SpanExporter::builder()
            .with_tonic()
            .with_endpoint(endpoint)
            .build(),
        TelemetryBatcher::OtlpHttp => opentelemetry_otlp::SpanExporter::builder()
            .with_http()
            .with_endpoint(endpoint)
            .build(),
    }
}

fn clamp_sampler(sampler: f64) -> f64 {
    sampler.clamp(0.0, 1.0)
}
