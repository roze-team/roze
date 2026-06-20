use std::sync::OnceLock;

use opentelemetry::global;
use opentelemetry::trace::TracerProvider as _;
use roze_config::ServiceConfig;
use roze_opentelemetry::SdkTracerProvider;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

static TRACER_PROVIDER: OnceLock<SdkTracerProvider> = OnceLock::new();

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

    let Some(provider) = roze_opentelemetry::build_tracer_provider(config)? else {
        let _ = subscriber.try_init();
        return Ok(());
    };

    let telemetry = config
        .telemetry
        .as_ref()
        .expect("provider requires telemetry config");
    let service_name = roze_opentelemetry::service_name(config, telemetry);
    let tracer = provider.tracer(service_name.to_string());

    let _ = TRACER_PROVIDER.set(provider.clone());
    global::set_tracer_provider(provider);

    let _ = subscriber
        .with(tracing_opentelemetry::layer().with_tracer(tracer))
        .try_init();

    Ok(())
}
