use std::{
    ffi::OsStr,
    fmt,
    fs::{self, File, OpenOptions},
    io::{BufReader, BufWriter},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc, Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::{Duration, SystemTime},
};

use anyhow::Context as _;
use chrono::{Local, Utc};
use opentelemetry::global;
use opentelemetry::trace::TracerProvider as _;
use roze_config::{
    LogCompression, LogFileConfig, LogFormat, LogRotation, LogSpanEvents, LoggingConfig,
    ServiceConfig,
};
use roze_opentelemetry::SdkTracerProvider;
use tracing_subscriber::{
    fmt::{
        format::{FmtSpan, Writer},
        time::FormatTime,
    },
    layer::SubscriberExt,
    util::SubscriberInitExt,
    EnvFilter, Layer, Registry,
};

type BoxLayer = Box<dyn Layer<Registry> + Send + Sync>;

/// Stable event names emitted by Roze framework and generated boundaries.
pub mod events {
    pub const SERVICE_CONFIG_LOADED: &str = "service.config.loaded";
    pub const SERVICE_STARTING: &str = "service.starting";
    pub const SERVICE_STOPPED: &str = "service.stopped";
    pub const SERVICE_FAILED: &str = "service.failed";
    pub const SERVICE_HEALTH_DRAINING: &str = "service.health.draining";
    pub const SERVICE_CONTEXT_INITIALIZED: &str = "service.context.initialized";
    pub const SERVICE_LIFECYCLE_HOOK_STARTED: &str = "service.lifecycle.hook_started";
    pub const SERVICE_LIFECYCLE_HOOK_COMPLETED: &str = "service.lifecycle.hook_completed";
    pub const SERVICE_LIFECYCLE_HOOK_FAILED: &str = "service.lifecycle.hook_failed";
    pub const SERVICE_LIFECYCLE_HOOKS_TIMED_OUT: &str = "service.lifecycle.hooks_timed_out";
    pub const SERVICE_TASK_STARTING: &str = "service.task.starting";
    pub const SERVICE_TASK_COMPLETED: &str = "service.task.completed";
    pub const SERVICE_TASK_FAILED: &str = "service.task.failed";
    pub const SERVICE_TASK_JOIN_FAILED: &str = "service.task.join_failed";
    pub const SERVICE_REGISTRY_REGISTERED: &str = "service.registry.registered";
    pub const SERVICE_REGISTRY_UNREGISTERED: &str = "service.registry.unregistered";
    pub const HTTP_REQUEST_STARTED: &str = "http.request.started";
    pub const HTTP_REQUEST_COMPLETED: &str = "http.request.completed";
    pub const HTTP_REQUEST_FAILED: &str = "http.request.failed";
    pub const HTTP_REQUEST_REJECTED: &str = "http.request.rejected";
    pub const REST_ROUTE_STARTED: &str = "rest.route.started";
    pub const REST_ROUTE_COMPLETED: &str = "rest.route.completed";
    pub const REST_ROUTE_CANCELLED: &str = "rest.route.cancelled";
    pub const RPC_SERVER_LISTENING: &str = "rpc.server.listening";
    pub const RPC_SERVER_SHUTDOWN_REQUESTED: &str = "rpc.server.shutdown_requested";
    pub const APPLICATION_LOGIC_STARTED: &str = "application.logic.started";
    pub const APPLICATION_LOGIC_COMPLETED: &str = "application.logic.completed";
    pub const APPLICATION_LOGIC_FAILED: &str = "application.logic.failed";
    pub const HTML_RENDER_COMPLETED: &str = "html.render.completed";
    pub const STREAM_MESSAGE_RECEIVED: &str = "stream.message.received";
    pub const STREAM_MESSAGE_COMPLETED: &str = "stream.message.completed";
    pub const STREAM_MESSAGE_NACKED: &str = "stream.message.nacked";
    pub const GATEWAY_CONFIG_RELOAD_FAILED: &str = "gateway.config.reload.failed";
    pub const GATEWAY_CONFIG_RELOAD_SKIPPED: &str = "gateway.config.hot_reloaded.skipped";
    pub const GATEWAY_CONFIG_RELOADED: &str = "gateway.config.hot_reloaded";
    pub const KAFKA_MESSAGE_RETRY_TOPIC_MISSING: &str = "kafka.message.retry_topic_missing";
    pub const KAFKA_MESSAGE_DEAD_LETTER_MISSING: &str = "kafka.message.dead_letter_missing";
    pub const KAFKA_MESSAGE_RECOVERY_DROPPED: &str = "kafka.message.recovery_dropped";
    pub const LOG_MAINTENANCE_FAILED: &str = "log.maintenance.failed";
    pub const LOG_LINES_DROPPED: &str = "log.lines.dropped";
}

/// A display/debug wrapper that never reveals its inner value.
/// Prefer omitting sensitive fields entirely.
pub struct Sensitive<T>(pub T);

impl<T> fmt::Debug for Sensitive<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}

impl<T> fmt::Display for Sensitive<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}

#[derive(Clone)]
struct ConfigTimer {
    utc: bool,
    format: String,
}

impl FormatTime for ConfigTimer {
    fn format_time(&self, writer: &mut Writer<'_>) -> fmt::Result {
        if self.utc {
            write!(writer, "{}", Utc::now().format(&self.format))
        } else {
            write!(writer, "{}", Local::now().format(&self.format))
        }
    }
}

struct MaintenanceWorker {
    stop: mpsc::Sender<()>,
    handle: Option<JoinHandle<()>>,
}

impl MaintenanceWorker {
    fn spawn(
        config: LogFileConfig,
        active_file: ActiveFile,
        error_counters: Vec<tracing_appender::non_blocking::ErrorCounter>,
        reported_dropped_lines: Arc<AtomicU64>,
    ) -> anyhow::Result<Option<Self>> {
        run_log_maintenance(&config, &active_file)?;
        if config.maintenance_interval_secs == 0 {
            return Ok(None);
        }
        let (stop, receiver) = mpsc::channel();
        let interval = Duration::from_secs(config.maintenance_interval_secs);
        let handle = thread::Builder::new()
            .name("roze-log-maintenance".to_string())
            .spawn(move || loop {
                match receiver.recv_timeout(interval) {
                    Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        report_dropped_lines(&error_counters, &reported_dropped_lines);
                        if let Err(error) = run_log_maintenance(&config, &active_file) {
                            tracing::warn!(
                                event = events::LOG_MAINTENANCE_FAILED,
                                error = %error,
                                "log maintenance failed"
                            );
                        }
                    }
                }
            })
            .context("failed to spawn log maintenance worker")?;
        Ok(Some(Self {
            stop,
            handle: Some(handle),
        }))
    }

    fn shutdown(&mut self) {
        let _ = self.stop.send(());
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

type ActiveFile = Arc<Mutex<PathBuf>>;

#[derive(Clone)]
struct ConfigurableRollingWriter {
    config: LogFileConfig,
    utc_time: bool,
    state: Arc<Mutex<RollingState>>,
    active_file: ActiveFile,
}

struct RollingState {
    bucket: String,
    sequence: u32,
    file: File,
    bytes: u64,
}

impl ConfigurableRollingWriter {
    fn new(config: LogFileConfig, utc_time: bool) -> anyhow::Result<Self> {
        let bucket = current_bucket(config.rotation, utc_time);
        let base_name = current_file_name(&config, utc_time);
        let (sequence, path, file, bytes) = open_writable_file(&config, &base_name)?;
        let active_file = Arc::new(Mutex::new(path.clone()));
        Ok(Self {
            config,
            utc_time,
            state: Arc::new(Mutex::new(RollingState {
                bucket,
                sequence,
                file,
                bytes,
            })),
            active_file,
        })
    }

    fn active_file(&self) -> ActiveFile {
        Arc::clone(&self.active_file)
    }

    fn prepare(&self, incoming: usize) -> std::io::Result<std::sync::MutexGuard<'_, RollingState>> {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        let bucket = current_bucket(self.config.rotation, self.utc_time);
        if state.bucket != bucket {
            let base_name = current_file_name(&self.config, self.utc_time);
            let (sequence, path, file, bytes) =
                open_writable_file(&self.config, &base_name).map_err(std::io::Error::other)?;
            state = self.replace_state(state, bucket, sequence, path, file, bytes);
        } else if self.config.max_file_size_bytes > 0
            && state.bytes > 0
            && state.bytes.saturating_add(incoming as u64) > self.config.max_file_size_bytes
        {
            let base_name = current_file_name(&self.config, self.utc_time);
            let (sequence, path, file, bytes) =
                open_writable_file_from(&self.config, &base_name, state.sequence.saturating_add(1))
                    .map_err(std::io::Error::other)?;
            state = self.replace_state(state, bucket, sequence, path, file, bytes);
        }
        Ok(state)
    }

    fn replace_state<'a>(
        &self,
        mut state: std::sync::MutexGuard<'a, RollingState>,
        bucket: String,
        sequence: u32,
        path: PathBuf,
        file: File,
        bytes: u64,
    ) -> std::sync::MutexGuard<'a, RollingState> {
        *self
            .active_file
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = path.clone();
        *state = RollingState {
            bucket,
            sequence,
            file,
            bytes,
        };
        state
    }
}

impl std::io::Write for ConfigurableRollingWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let mut state = self.prepare(bytes.len())?;
        let written = state.file.write(bytes)?;
        state.bytes = state.bytes.saturating_add(written as u64);
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .file
            .flush()
    }
}

fn current_bucket(rotation: LogRotation, utc_time: bool) -> String {
    let format = match rotation {
        LogRotation::Hourly => "%Y-%m-%d-%H",
        LogRotation::Daily => "%Y-%m-%d",
        LogRotation::Never => return "never".to_string(),
    };
    if utc_time {
        Utc::now().format(format).to_string()
    } else {
        Local::now().format(format).to_string()
    }
}

fn current_file_name(config: &LogFileConfig, utc_time: bool) -> String {
    if !config.file_name_pattern.contains("{date}") {
        return config.file_name_pattern.clone();
    }
    let date = if utc_time {
        Utc::now().format(&config.date_format).to_string()
    } else {
        Local::now().format(&config.date_format).to_string()
    };
    config.file_name_pattern.replace("{date}", &date)
}

fn open_writable_file(
    config: &LogFileConfig,
    base_name: &str,
) -> anyhow::Result<(u32, PathBuf, File, u64)> {
    open_writable_file_from(config, base_name, 0)
}

fn open_writable_file_from(
    config: &LogFileConfig,
    base_name: &str,
    mut sequence: u32,
) -> anyhow::Result<(u32, PathBuf, File, u64)> {
    loop {
        let name = if sequence == 0 {
            base_name.to_string()
        } else {
            sequenced_file_name(base_name, sequence)
        };
        let path = config.directory.join(name);
        let (file, bytes) = open_append(&path)?;
        if config.max_file_size_bytes == 0 || bytes < config.max_file_size_bytes {
            return Ok((sequence, path, file, bytes));
        }
        sequence = sequence
            .checked_add(1)
            .context("log file sequence exhausted")?;
    }
}

fn open_append(path: &Path) -> anyhow::Result<(File, u64)> {
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("failed to open log file {}", path.display()))?;
    let bytes = file.metadata().map(|metadata| metadata.len()).unwrap_or(0);
    Ok((file, bytes))
}

fn sequenced_file_name(base_name: &str, sequence: u32) -> String {
    let path = Path::new(base_name);
    match (
        path.file_stem().and_then(OsStr::to_str),
        path.extension().and_then(OsStr::to_str),
    ) {
        (Some(stem), Some(extension)) => format!("{stem}.{sequence}.{extension}"),
        _ => format!("{base_name}.{sequence}"),
    }
}

fn is_sequenced_name(name: &str, base_name: &str) -> bool {
    let path = Path::new(base_name);
    match (
        path.file_stem().and_then(OsStr::to_str),
        path.extension().and_then(OsStr::to_str),
    ) {
        (Some(stem), Some(extension)) => name
            .strip_prefix(&format!("{stem}."))
            .and_then(|value| value.strip_suffix(&format!(".{extension}")))
            .is_some_and(|sequence| {
                !sequence.is_empty() && sequence.bytes().all(|byte| byte.is_ascii_digit())
            }),
        _ => name
            .strip_prefix(&format!("{base_name}."))
            .is_some_and(|sequence| {
                !sequence.is_empty() && sequence.bytes().all(|byte| byte.is_ascii_digit())
            }),
    }
}

/// Owns asynchronous writer and telemetry resources for a service lifetime.
#[must_use = "dropping the tracing guard immediately disables asynchronous log flushing"]
pub struct TracingGuard {
    maintenance: Option<MaintenanceWorker>,
    tracer_provider: Option<SdkTracerProvider>,
    writer_guards: Vec<tracing_appender::non_blocking::WorkerGuard>,
    error_counters: Vec<tracing_appender::non_blocking::ErrorCounter>,
    reported_dropped_lines: Arc<AtomicU64>,
}

impl TracingGuard {
    /// Number of lines dropped by lossy asynchronous file writers.
    pub fn dropped_lines(&self) -> u64 {
        report_dropped_lines(&self.error_counters, &self.reported_dropped_lines)
    }
}

impl Drop for TracingGuard {
    fn drop(&mut self) {
        if let Some(mut maintenance) = self.maintenance.take() {
            maintenance.shutdown();
        }
        report_dropped_lines(&self.error_counters, &self.reported_dropped_lines);
        if let Some(provider) = self.tracer_provider.take() {
            if let Err(error) = provider.shutdown() {
                eprintln!("failed to shut down OpenTelemetry provider: {error}");
            }
        }
        // Flush asynchronous writers before returning from `drop` so callers can
        // safely inspect or rotate log files immediately after dropping the guard.
        self.writer_guards.clear();
    }
}

pub fn init_tracing() {
    init_tracing_with_filter("info");
}

pub fn init_tracing_with_filter(filter: impl AsRef<str>) {
    let env_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(filter.as_ref()));

    if let Err(error) = tracing_subscriber::registry()
        .with(env_filter)
        .with(tracing_subscriber::fmt::layer())
        .try_init()
    {
        eprintln!("failed to initialize tracing subscriber: {error}");
    }
}

pub fn init_tracing_with_config(config: &ServiceConfig) -> anyhow::Result<TracingGuard> {
    init_tracing_with_config_and_filter(config, &config.logging.level)
}

pub fn init_tracing_with_config_and_filter(
    config: &ServiceConfig,
    fallback_filter: impl AsRef<str>,
) -> anyhow::Result<TracingGuard> {
    let (tracer_provider, otel_layer) =
        if let Some(provider) = roze_opentelemetry::build_tracer_provider(config)? {
            let telemetry = config
                .telemetry
                .as_ref()
                .expect("provider requires telemetry config");
            let service_name = roze_opentelemetry::service_name(config, telemetry);
            let tracer = provider.tracer(service_name.to_string());
            let layer = tracing_opentelemetry::layer().with_tracer(tracer).boxed();
            (Some(provider), Some(layer))
        } else {
            (None, None)
        };
    init_logging_pipeline(
        &config.logging,
        fallback_filter.as_ref(),
        tracer_provider,
        otel_layer,
    )
}

/// Initializes local logging for processes that do not use `ServiceConfig`,
/// such as generated stream workers.
pub fn init_tracing_with_logging(logging: &LoggingConfig) -> anyhow::Result<TracingGuard> {
    init_logging_pipeline(logging, &logging.level, None, None)
}

fn init_logging_pipeline(
    logging: &LoggingConfig,
    fallback_filter: &str,
    tracer_provider: Option<SdkTracerProvider>,
    otel_layer: Option<BoxLayer>,
) -> anyhow::Result<TracingGuard> {
    logging.validate()?;
    let filter = resolve_filter(logging, fallback_filter)?;
    let timer = ConfigTimer {
        utc: logging.utc_time,
        format: logging.time_format.clone(),
    };
    let span_events = span_events(logging.span_events);
    let mut layers: Vec<BoxLayer> = Vec::new();
    let mut writer_guards = Vec::new();
    let mut error_counters = Vec::new();
    let reported_dropped_lines = Arc::new(AtomicU64::new(0));
    let mut maintenance = None;

    if logging.enabled && logging.stdout {
        layers.push(
            format_layer(
                logging,
                timer.clone(),
                span_events.clone(),
                true,
                std::io::stdout,
            )
            .with_filter(filter.clone())
            .boxed(),
        );
    }

    if logging.enabled {
        if let Some(file) = &logging.file {
            fs::create_dir_all(&file.directory).with_context(|| {
                format!(
                    "failed to create log directory {}",
                    file.directory.display()
                )
            })?;
            let appender = ConfigurableRollingWriter::new(file.clone(), logging.utc_time)
                .with_context(|| {
                    format!(
                        "failed to open rolling log file in {}",
                        file.directory.display()
                    )
                })?;
            let active_file = appender.active_file();
            let (writer, guard) = tracing_appender::non_blocking::NonBlockingBuilder::default()
                .buffered_lines_limit(logging.non_blocking_buffer)
                .lossy(logging.lossy)
                .finish(appender);
            error_counters.push(writer.error_counter());
            maintenance = MaintenanceWorker::spawn(
                file.clone(),
                active_file,
                error_counters.clone(),
                Arc::clone(&reported_dropped_lines),
            )?;
            writer_guards.push(guard);
            layers.push(
                format_layer(logging, timer.clone(), span_events, false, writer)
                    .with_filter(filter.clone())
                    .boxed(),
            );
        }
    }

    if let Some(layer) = otel_layer {
        layers.push(layer.with_filter(filter).boxed());
    }
    if let Some(provider) = tracer_provider.as_ref() {
        global::set_tracer_provider(provider.clone());
    }

    tracing_subscriber::registry()
        .with(layers)
        .try_init()
        .map_err(|error| anyhow::anyhow!("failed to initialize tracing subscriber: {error}"))?;

    Ok(TracingGuard {
        maintenance,
        tracer_provider,
        writer_guards,
        error_counters,
        reported_dropped_lines,
    })
}

fn report_dropped_lines(
    counters: &[tracing_appender::non_blocking::ErrorCounter],
    reported: &AtomicU64,
) -> u64 {
    let total = counters
        .iter()
        .map(|counter| counter.dropped_lines() as u64)
        .sum::<u64>();
    let previous = reported.swap(total, Ordering::Relaxed);
    if total > previous {
        let dropped = total - previous;
        roze_metrics::record_log_dropped_lines("file", dropped);
        tracing::warn!(
            event = events::LOG_LINES_DROPPED,
            sink = "file",
            dropped_lines = dropped,
            dropped_lines_total = total,
            "asynchronous log lines dropped"
        );
    }
    total
}

fn resolve_filter(config: &LoggingConfig, fallback: &str) -> anyhow::Result<EnvFilter> {
    if let Some(filter) = config.env_filter.as_deref() {
        return EnvFilter::try_new(filter)
            .with_context(|| format!("invalid logging.env_filter `{filter}`"));
    }
    if let Ok(filter) = EnvFilter::try_from_default_env() {
        return Ok(filter);
    }
    EnvFilter::try_new(fallback).with_context(|| format!("invalid logging filter `{fallback}`"))
}

fn format_layer<W>(
    config: &LoggingConfig,
    timer: ConfigTimer,
    span_events: FmtSpan,
    allow_ansi: bool,
    writer: W,
) -> BoxLayer
where
    W: for<'writer> tracing_subscriber::fmt::MakeWriter<'writer> + Send + Sync + 'static,
{
    match config.format {
        LogFormat::Json => tracing_subscriber::fmt::layer()
            .json()
            .with_writer(writer)
            .with_ansi(false)
            .with_target(config.target)
            .with_timer(timer)
            .with_span_events(span_events)
            .with_thread_ids(config.thread_ids)
            .with_file(config.caller)
            .with_line_number(config.caller)
            .boxed(),
        LogFormat::Text => tracing_subscriber::fmt::layer()
            .with_writer(writer)
            .with_ansi(allow_ansi && config.ansi)
            .with_target(config.target)
            .with_timer(timer)
            .with_span_events(span_events)
            .with_thread_ids(config.thread_ids)
            .with_file(config.caller)
            .with_line_number(config.caller)
            .boxed(),
    }
}

fn span_events(value: LogSpanEvents) -> FmtSpan {
    match value {
        LogSpanEvents::None => FmtSpan::NONE,
        LogSpanEvents::New => FmtSpan::NEW,
        LogSpanEvents::Enter => FmtSpan::ENTER,
        LogSpanEvents::Exit => FmtSpan::EXIT,
        LogSpanEvents::Close => FmtSpan::CLOSE,
        LogSpanEvents::Active => FmtSpan::ACTIVE,
        LogSpanEvents::Full => FmtSpan::FULL,
    }
}

fn run_log_maintenance(config: &LogFileConfig, active_file: &ActiveFile) -> anyhow::Result<()> {
    let now = SystemTime::now();
    let retention = Duration::from_secs(config.retention_days.saturating_mul(86_400));
    let active_file = active_file
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone();
    for entry in fs::read_dir(&config.directory).with_context(|| {
        format!(
            "failed to read log directory {}",
            config.directory.display()
        )
    })? {
        let entry = entry.context("failed to read log directory entry")?;
        let path = entry.path();
        if !path.is_file() || !is_owned_log_file(&path, config) {
            continue;
        }
        if path == active_file {
            continue;
        }

        if config.retention_days > 0 {
            let modified = entry
                .metadata()
                .and_then(|metadata| metadata.modified())
                .unwrap_or(now);
            if now.duration_since(modified).unwrap_or_default() >= retention {
                fs::remove_file(&path)
                    .with_context(|| format!("failed to remove expired log {}", path.display()))?;
                continue;
            }
        }

        if config.compression == LogCompression::Gzip
            && path.extension().and_then(OsStr::to_str) != Some("gz")
        {
            compress_file(&path, config.compression_level)?;
        }
    }
    Ok(())
}

fn is_owned_log_file(path: &Path, config: &LogFileConfig) -> bool {
    path.file_name()
        .and_then(OsStr::to_str)
        .is_some_and(|name| {
            if name.ends_with(".tmp") {
                return false;
            }
            let name = name.strip_suffix(".gz").unwrap_or(name);
            if let Some((prefix, suffix)) = config.file_name_pattern.split_once("{date}") {
                let Some(middle) = name
                    .strip_prefix(prefix)
                    .and_then(|value| value.strip_suffix(suffix))
                else {
                    return false;
                };
                valid_date_segment(middle, &config.date_format)
            } else {
                name == config.file_name_pattern
                    || is_sequenced_name(name, &config.file_name_pattern)
            }
        })
}

fn valid_date_segment(value: &str, date_format: &str) -> bool {
    if parses_date_segment(value, date_format) {
        return true;
    }
    value.rsplit_once('.').is_some_and(|(date, sequence)| {
        !sequence.is_empty()
            && sequence.bytes().all(|byte| byte.is_ascii_digit())
            && parses_date_segment(date, date_format)
    })
}

fn parses_date_segment(value: &str, date_format: &str) -> bool {
    let mut parsed = chrono::format::Parsed::new();
    chrono::format::parse(
        &mut parsed,
        value,
        chrono::format::StrftimeItems::new(date_format),
    )
    .is_ok()
}

fn compress_file(path: &Path, level: u32) -> anyhow::Result<()> {
    let name = path
        .file_name()
        .and_then(OsStr::to_str)
        .context("rotated log has an invalid file name")?;
    let destination = path.with_file_name(format!("{name}.gz"));
    if destination.exists() {
        return Ok(());
    }
    let temporary = path.with_file_name(format!("{name}.gz.tmp"));
    if temporary.exists() {
        fs::remove_file(&temporary).with_context(|| {
            format!(
                "failed to remove incomplete compressed log {}",
                temporary.display()
            )
        })?;
    }
    let input = File::open(path)
        .with_context(|| format!("failed to open rotated log {}", path.display()))?;
    let output = File::create(&temporary)
        .with_context(|| format!("failed to create compressed log {}", temporary.display()))?;
    let mut reader = BufReader::new(input);
    let writer = BufWriter::new(output);
    let mut encoder = flate2::write::GzEncoder::new(writer, flate2::Compression::new(level));
    std::io::copy(&mut reader, &mut encoder)
        .with_context(|| format!("failed to compress rotated log {}", path.display()))?;
    encoder
        .finish()
        .with_context(|| format!("failed to finalize compressed log {}", temporary.display()))?;
    fs::rename(&temporary, &destination)
        .with_context(|| format!("failed to publish compressed log {}", destination.display()))?;
    fs::remove_file(path)
        .with_context(|| format!("failed to remove rotated log {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read as _;

    #[test]
    fn sensitive_never_formats_inner_value() {
        let value = Sensitive("secret-value");
        assert_eq!(format!("{value}"), "[REDACTED]");
        assert_eq!(format!("{value:?}"), "[REDACTED]");
    }

    #[test]
    fn maintenance_only_owns_exact_log_prefix() {
        let config = LogFileConfig {
            file_name_pattern: "roze.{date}.log".to_string(),
            date_format: "%Y-%m-%d".to_string(),
            ..LogFileConfig::default()
        };
        assert!(is_owned_log_file(Path::new("roze.2026-01-01.log"), &config));
        assert!(is_owned_log_file(
            Path::new("roze.2026-01-01.2.log.gz"),
            &config
        ));
        assert!(!is_owned_log_file(
            Path::new("roze.not-a-date.log"),
            &config
        ));
        assert!(!is_owned_log_file(
            Path::new("roze.2026-01-01.log.gz.tmp"),
            &config
        ));
        assert!(!is_owned_log_file(Path::new("other.log"), &config));
    }

    #[test]
    fn maintenance_compresses_rotated_logs_and_preserves_unrelated_files() {
        let directory = tempfile::tempdir().expect("temp directory");
        let rotated = directory.path().join("roze.2000-01-01.log");
        let active = directory.path().join("roze.2099-01-01.log");
        let unrelated = directory.path().join("other.2000-01-01.log");
        fs::write(&rotated, b"structured log\n").expect("rotated log");
        fs::write(&active, b"active\n").expect("active log");
        fs::write(&unrelated, b"unrelated\n").expect("unrelated log");
        let config = LogFileConfig {
            directory: directory.path().to_path_buf(),
            file_name_pattern: "roze.{date}.log".to_string(),
            compression: LogCompression::Gzip,
            retention_days: 0,
            maintenance_interval_secs: 0,
            ..LogFileConfig::default()
        };

        let active_file = Arc::new(Mutex::new(active.clone()));
        run_log_maintenance(&config, &active_file).expect("maintenance");

        assert!(!rotated.exists());
        assert!(active.exists());
        assert!(!directory.path().join("roze.2099-01-01.log.gz").exists());
        assert!(unrelated.exists());
        let compressed = directory.path().join("roze.2000-01-01.log.gz");
        let mut decoder =
            flate2::read::GzDecoder::new(File::open(compressed).expect("compressed log"));
        let mut body = String::new();
        decoder.read_to_string(&mut body).expect("decode gzip");
        assert_eq!(body, "structured log\n");
    }

    #[test]
    fn configured_json_file_is_structured_and_flushed_on_drop() {
        let directory = tempfile::tempdir().expect("temp directory");
        let logging = LoggingConfig {
            format: LogFormat::Json,
            env_filter: Some("info".to_string()),
            stdout: false,
            ansi: false,
            lossy: false,
            file: Some(LogFileConfig {
                directory: directory.path().to_path_buf(),
                file_name_pattern: "service.log".to_string(),
                rotation: LogRotation::Never,
                maintenance_interval_secs: 0,
                ..LogFileConfig::default()
            }),
            ..LoggingConfig::default()
        };

        let guard = init_tracing_with_logging(&logging).expect("initialize logging");
        tracing::info!(
            event = events::SERVICE_STARTING,
            service = "test-service",
            protocol = "test",
            "service starting"
        );
        drop(guard);

        let body =
            fs::read_to_string(directory.path().join("service.log")).expect("read flushed log");
        let record: serde_json::Value =
            serde_json::from_str(body.trim()).expect("structured JSON line");
        assert_eq!(record["level"], "INFO");
        assert_eq!(record["fields"]["event"], events::SERVICE_STARTING);
        assert_eq!(record["fields"]["service"], "test-service");
        assert_eq!(record["fields"]["protocol"], "test");
        assert!(record["timestamp"].is_string());
    }

    #[test]
    fn size_rotation_preserves_extension_and_existing_segments() {
        let directory = tempfile::tempdir().expect("temp directory");
        let config = LogFileConfig {
            directory: directory.path().to_path_buf(),
            file_name_pattern: "service.log".to_string(),
            rotation: LogRotation::Never,
            max_file_size_bytes: 5,
            ..LogFileConfig::default()
        };
        let mut writer = ConfigurableRollingWriter::new(config, true).expect("rolling writer");
        std::io::Write::write_all(&mut writer, b"first").expect("first segment");
        std::io::Write::write_all(&mut writer, b"second").expect("second segment");
        std::io::Write::flush(&mut writer).expect("flush");

        assert_eq!(
            fs::read(directory.path().join("service.log")).unwrap(),
            b"first"
        );
        assert_eq!(
            fs::read(directory.path().join("service.1.log")).unwrap(),
            b"second"
        );
    }
}
