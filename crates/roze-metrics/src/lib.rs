use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, OnceLock,
    },
    time::Duration,
};

use dashmap::{mapref::entry::Entry, DashMap};

static REQUEST_TOTAL: AtomicU64 = AtomicU64::new(0);
static REQUEST_FAILED: AtomicU64 = AtomicU64::new(0);
static REQUEST_ELAPSED_MS: AtomicU64 = AtomicU64::new(0);
static ROUTE_METRICS: OnceLock<MetricRegistry> = OnceLock::new();
static RPC_METRICS: OnceLock<MetricRegistry> = OnceLock::new();
static GATEWAY_METRICS: OnceLock<MetricRegistry> = OnceLock::new();
static QUEUE_METRICS: OnceLock<MetricRegistry> = OnceLock::new();
static RESILIENCE_METRICS: OnceLock<MetricRegistry> = OnceLock::new();
static REPORT_METRICS: OnceLock<MetricRegistry> = OnceLock::new();
const LATENCY_BUCKETS: usize = 65;

/// Fixed-memory, power-of-two latency histogram for long-running evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LatencyHistogram {
    buckets: [u64; LATENCY_BUCKETS],
    count: u64,
}

impl Default for LatencyHistogram {
    fn default() -> Self {
        Self {
            buckets: [0; LATENCY_BUCKETS],
            count: 0,
        }
    }
}

impl LatencyHistogram {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn observe(&mut self, latency: Duration) {
        let micros = latency.as_micros().min(u128::from(u64::MAX)) as u64;
        let bucket = if micros == 0 {
            0
        } else {
            (u64::BITS - micros.leading_zeros()) as usize
        };
        self.buckets[bucket] = self.buckets[bucket].saturating_add(1);
        self.count = self.count.saturating_add(1);
    }

    pub fn count(&self) -> u64 {
        self.count
    }

    /// Returns the inclusive upper bound of the bucket containing the percentile.
    pub fn percentile_upper_bound_micros(&self, percentile: u8) -> Option<u64> {
        if self.count == 0 || !(1..=100).contains(&percentile) {
            return None;
        }
        let target = (u128::from(self.count) * u128::from(percentile)).div_ceil(100);
        let mut cumulative = 0_u128;
        for (index, count) in self.buckets.iter().enumerate() {
            cumulative += u128::from(*count);
            if cumulative >= target {
                return Some(if index == 0 {
                    0
                } else if index >= u64::BITS as usize {
                    u64::MAX
                } else {
                    (1_u64 << index) - 1
                });
            }
        }
        Some(u64::MAX)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MetricLabels(BTreeMap<String, String>);

impl MetricLabels {
    pub fn new() -> Self {
        Self(BTreeMap::new())
    }

    pub fn insert(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.0.insert(key.into(), value.into());
        self
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum MetricValue {
    Counter(u64),
    Gauge(f64),
    Duration(Duration),
}

#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct MetricKey {
    name: String,
    labels: MetricLabels,
}

#[derive(Debug, Default, Clone)]
pub struct MetricRegistry {
    inner: Arc<DashMap<MetricKey, MetricValue>>,
}

impl MetricRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn inc_counter(&self, name: impl Into<String>, labels: MetricLabels, by: u64) {
        self.update(name.into(), labels, |value| match value {
            Some(MetricValue::Counter(current)) => MetricValue::Counter(current + by),
            _ => MetricValue::Counter(by),
        });
    }

    pub fn set_gauge(&self, name: impl Into<String>, labels: MetricLabels, value: f64) {
        self.update(name.into(), labels, |_| MetricValue::Gauge(value));
    }

    pub fn observe_duration(&self, name: impl Into<String>, labels: MetricLabels, value: Duration) {
        self.update(name.into(), labels, |_| MetricValue::Duration(value));
    }

    pub fn series_count(&self) -> usize {
        self.inner.len()
    }

    pub fn render(&self) -> String {
        let mut entries = self
            .inner
            .iter()
            .map(|entry| (entry.key().clone(), entry.value().clone()))
            .collect::<Vec<_>>();
        entries.sort_by(|(left, _), (right, _)| left.cmp(right));

        let mut out = String::new();
        for (key, value) in entries {
            let labels_text = if key.labels.0.is_empty() {
                String::new()
            } else {
                let joined = key
                    .labels
                    .0
                    .iter()
                    .map(|(key, value)| {
                        format!(
                            r#"{}="{}""#,
                            normalize_label_key(key),
                            escape_label_value(value)
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(",");
                format!("{{{joined}}}")
            };
            match value {
                MetricValue::Counter(value) => {
                    out.push_str(&format!("{}{} {}\n", key.name, labels_text, value));
                }
                MetricValue::Gauge(value) => {
                    out.push_str(&format!("{}{} {}\n", key.name, labels_text, value));
                }
                MetricValue::Duration(value) => {
                    out.push_str(&format!(
                        "{}{} {}\n",
                        key.name,
                        labels_text,
                        value.as_millis()
                    ));
                }
            }
        }
        out
    }

    fn update<F>(&self, name: String, labels: MetricLabels, f: F)
    where
        F: FnOnce(Option<MetricValue>) -> MetricValue,
    {
        let key = MetricKey { name, labels };
        match self.inner.entry(key) {
            Entry::Occupied(mut entry) => {
                let current = entry.get().clone();
                entry.insert(f(Some(current)));
            }
            Entry::Vacant(entry) => {
                entry.insert(f(None));
            }
        }
    }
}

pub fn record_http_request(success: bool, elapsed: Duration) {
    REQUEST_TOTAL.fetch_add(1, Ordering::Relaxed);
    if !success {
        REQUEST_FAILED.fetch_add(1, Ordering::Relaxed);
    }
    REQUEST_ELAPSED_MS.fetch_add(elapsed.as_millis() as u64, Ordering::Relaxed);
}

pub fn record_http_route(
    service: impl Into<String>,
    route: impl Into<String>,
    method: impl Into<String>,
    status: impl Into<String>,
    elapsed: Duration,
) {
    let labels = MetricLabels::new()
        .insert("service", service.into())
        .insert("route", route.into())
        .insert("method", method.into())
        .insert("status", status.into());
    let registry = route_metrics_registry();
    registry.inc_counter("roze_http_route_requests_total", labels.clone(), 1);
    registry.inc_counter(
        "roze_http_route_request_duration_ms_total",
        labels,
        elapsed.as_millis() as u64,
    );
}

pub fn record_rpc_method(
    service: impl Into<String>,
    method: impl Into<String>,
    code: impl Into<String>,
    elapsed: Duration,
) {
    let labels = MetricLabels::new()
        .insert("service", service.into())
        .insert("method", method.into())
        .insert("code", code.into());
    let registry = rpc_metrics_registry();
    registry.inc_counter("roze_rpc_method_requests_total", labels.clone(), 1);
    registry.inc_counter(
        "roze_rpc_method_request_duration_ms_total",
        labels,
        elapsed.as_millis() as u64,
    );
}

pub fn record_rpc_client_attempt(
    service: impl Into<String>,
    method: impl Into<String>,
    outcome: impl Into<String>,
) {
    let outcome = normalize_rpc_client_attempt_outcome(outcome.into());
    let labels = MetricLabels::new()
        .insert("service", service.into())
        .insert("method", method.into())
        .insert("outcome", outcome);
    rpc_metrics_registry().inc_counter("roze_rpc_client_attempts_total", labels, 1);
}

fn normalize_rpc_client_attempt_outcome(outcome: String) -> String {
    match outcome.as_str() {
        "success" | "failure" | "timeout" | "cancelled" => outcome,
        _ => "other".to_string(),
    }
}

pub fn record_gateway_route(
    service: impl Into<String>,
    route: impl Into<String>,
    method: impl Into<String>,
    status: impl Into<String>,
    outcome: impl Into<String>,
    elapsed: Duration,
) {
    let labels = MetricLabels::new()
        .insert("service", service.into())
        .insert("route", route.into())
        .insert("method", method.into())
        .insert("status", status.into())
        .insert("outcome", outcome.into());
    let registry = gateway_metrics_registry();
    registry.inc_counter("roze_gateway_route_requests_total", labels.clone(), 1);
    registry.inc_counter(
        "roze_gateway_route_request_duration_ms_total",
        labels,
        elapsed.as_millis() as u64,
    );
}

pub fn record_gateway_retry(
    service: impl Into<String>,
    route: impl Into<String>,
    reason: impl Into<String>,
) {
    let labels = MetricLabels::new()
        .insert("service", service.into())
        .insert("route", route.into())
        .insert("reason", reason.into());
    gateway_metrics_registry().inc_counter("roze_gateway_route_retries_total", labels, 1);
}

pub fn record_gateway_upstream(
    service: impl Into<String>,
    upstream: impl Into<String>,
    outcome: impl Into<String>,
) {
    let labels = MetricLabels::new()
        .insert("service", service.into())
        .insert("upstream", upstream.into())
        .insert("outcome", outcome.into());
    gateway_metrics_registry().inc_counter("roze_gateway_upstream_events_total", labels, 1);
}

pub fn record_gateway_stream_connection(
    service: impl Into<String>,
    route: impl Into<String>,
    protocol: impl Into<String>,
    outcome: impl Into<String>,
    active: u32,
) {
    let service = service.into();
    let route = route.into();
    let protocol = protocol.into();
    let labels = MetricLabels::new()
        .insert("service", service.clone())
        .insert("route", route.clone())
        .insert("protocol", protocol.clone())
        .insert("outcome", outcome.into());
    let active_labels = MetricLabels::new()
        .insert("service", service)
        .insert("route", route)
        .insert("protocol", protocol);
    let registry = gateway_metrics_registry();
    registry.inc_counter("roze_gateway_stream_connection_events_total", labels, 1);
    registry.set_gauge(
        "roze_gateway_stream_connections_active",
        active_labels,
        active as f64,
    );
}

pub fn record_gateway_stream_connection_duration(
    service: impl Into<String>,
    route: impl Into<String>,
    protocol: impl Into<String>,
    duration: Duration,
) {
    let labels = MetricLabels::new()
        .insert("service", service.into())
        .insert("route", route.into())
        .insert("protocol", protocol.into());
    gateway_metrics_registry().inc_counter(
        "roze_gateway_stream_connection_duration_ms_total",
        labels,
        duration.as_millis() as u64,
    );
}

pub fn record_gateway_config_reload(outcome: impl Into<String>) {
    let labels = MetricLabels::new().insert("outcome", outcome.into());
    gateway_metrics_registry().inc_counter("roze_gateway_config_reloads_total", labels, 1);
}

pub fn record_queue_event(
    system: impl Into<String>,
    topic: impl Into<String>,
    group: impl Into<String>,
    outcome: impl Into<String>,
) {
    let labels = MetricLabels::new()
        .insert("system", system.into())
        .insert("topic", topic.into())
        .insert("group", group.into())
        .insert("outcome", outcome.into());
    queue_metrics_registry().inc_counter("roze_queue_events_total", labels, 1);
}

pub fn record_queue_offset(
    system: impl Into<String>,
    topic: impl Into<String>,
    group: impl Into<String>,
    partition: i32,
    offset: i64,
) {
    let labels = MetricLabels::new()
        .insert("system", system.into())
        .insert("topic", topic.into())
        .insert("group", group.into())
        .insert("partition", partition.to_string());
    queue_metrics_registry().set_gauge("roze_queue_last_offset", labels, offset as f64);
}

pub fn record_resilience_decision(
    service: impl Into<String>,
    boundary: impl Into<String>,
    kind: impl Into<String>,
    decision: impl Into<String>,
) {
    let labels = MetricLabels::new()
        .insert("service", service.into())
        .insert("boundary", boundary.into())
        .insert("kind", kind.into())
        .insert("decision", decision.into());
    resilience_metrics_registry().inc_counter("roze_resilience_decisions_total", labels, 1);
}

pub fn record_report_export(
    format: impl Into<String>,
    outcome: impl Into<String>,
    bytes: u64,
    elapsed: Duration,
) {
    let labels = MetricLabels::new()
        .insert("format", format.into())
        .insert("outcome", outcome.into());
    let registry = report_metrics_registry();
    registry.inc_counter("roze_report_export_events_total", labels.clone(), 1);
    registry.inc_counter("roze_report_export_bytes_total", labels.clone(), bytes);
    registry.inc_counter(
        "roze_report_export_duration_ms_total",
        labels,
        elapsed.as_millis() as u64,
    );
}

pub fn record_chart_query(
    outcome: impl Into<String>,
    scanned_rows: u64,
    result_rows: u64,
    elapsed: Duration,
) {
    let labels = MetricLabels::new().insert("outcome", outcome.into());
    let registry = report_metrics_registry();
    registry.inc_counter("roze_chart_query_events_total", labels.clone(), 1);
    registry.inc_counter(
        "roze_chart_query_scanned_rows_total",
        labels.clone(),
        scanned_rows,
    );
    registry.inc_counter(
        "roze_chart_query_result_rows_total",
        labels.clone(),
        result_rows,
    );
    registry.inc_counter(
        "roze_chart_query_duration_ms_total",
        labels,
        elapsed.as_millis() as u64,
    );
}

pub fn http_metrics() -> String {
    let total = REQUEST_TOTAL.load(Ordering::Relaxed);
    let failed = REQUEST_FAILED.load(Ordering::Relaxed);
    let elapsed_ms = REQUEST_ELAPSED_MS.load(Ordering::Relaxed);
    let avg_ms = elapsed_ms.checked_div(total).unwrap_or(0);

    let mut out = format!(
        concat!(
            "# HELP roze_http_requests_total Total HTTP requests\n",
            "# TYPE roze_http_requests_total counter\n",
            "roze_http_requests_total {}\n",
            "# HELP roze_http_requests_failed_total Failed HTTP requests\n",
            "# TYPE roze_http_requests_failed_total counter\n",
            "roze_http_requests_failed_total {}\n",
            "# HELP roze_http_request_duration_ms_total Total HTTP request duration in milliseconds\n",
            "# TYPE roze_http_request_duration_ms_total counter\n",
            "roze_http_request_duration_ms_total {}\n",
            "# HELP roze_http_request_duration_ms_avg Average HTTP request duration in milliseconds\n",
            "# TYPE roze_http_request_duration_ms_avg gauge\n",
            "roze_http_request_duration_ms_avg {}\n"
        ),
        total, failed, elapsed_ms, avg_ms
    );
    out.push_str(&route_metrics_registry().render());
    out.push_str(&rpc_metrics_registry().render());
    out.push_str(&gateway_metrics_registry().render());
    out.push_str(&queue_metrics_registry().render());
    out.push_str(&resilience_metrics_registry().render());
    out.push_str(&report_metrics_registry().render());
    out
}

pub fn service_registry() -> MetricRegistry {
    MetricRegistry::new()
}

pub fn route_metrics_registry() -> &'static MetricRegistry {
    ROUTE_METRICS.get_or_init(MetricRegistry::new)
}

pub fn rpc_metrics_registry() -> &'static MetricRegistry {
    RPC_METRICS.get_or_init(MetricRegistry::new)
}

pub fn gateway_metrics_registry() -> &'static MetricRegistry {
    GATEWAY_METRICS.get_or_init(MetricRegistry::new)
}

pub fn queue_metrics_registry() -> &'static MetricRegistry {
    QUEUE_METRICS.get_or_init(MetricRegistry::new)
}

pub fn resilience_metrics_registry() -> &'static MetricRegistry {
    RESILIENCE_METRICS.get_or_init(MetricRegistry::new)
}

pub fn report_metrics_registry() -> &'static MetricRegistry {
    REPORT_METRICS.get_or_init(MetricRegistry::new)
}

fn escape_label_value(value: &str) -> String {
    value
        .replace('\\', r"\\")
        .replace('\n', r"\n")
        .replace('"', r#"\""#)
}

fn normalize_label_key(key: &str) -> String {
    let mut normalized = String::with_capacity(key.len().max(1));
    for ch in key.chars() {
        let valid = ch == '_' || ch.is_ascii_alphanumeric();
        normalized.push(if valid { ch } else { '_' });
    }
    if normalized.is_empty() {
        return "_".to_string();
    }
    if normalized
        .as_bytes()
        .first()
        .is_some_and(|first| first.is_ascii_digit())
    {
        normalized.insert(0, '_');
    }
    normalized
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_metrics() {
        record_http_request(true, Duration::from_millis(12));
        record_http_request(false, Duration::from_millis(3));
        let metrics = http_metrics();
        assert!(metrics.contains("roze_http_requests_total"));
        assert!(metrics.contains("roze_http_requests_failed_total"));
    }

    #[test]
    fn latency_histogram_tracks_percentiles_with_fixed_buckets() {
        let mut histogram = LatencyHistogram::new();
        for micros in [1, 2, 3, 4, 5, 8, 13, 21, 34, 55] {
            histogram.observe(Duration::from_micros(micros));
        }

        assert_eq!(histogram.count(), 10);
        assert_eq!(histogram.percentile_upper_bound_micros(50), Some(7));
        assert_eq!(histogram.percentile_upper_bound_micros(95), Some(63));
        assert_eq!(histogram.percentile_upper_bound_micros(99), Some(63));
        assert_eq!(histogram.percentile_upper_bound_micros(0), None);
    }

    #[test]
    fn empty_latency_histogram_has_no_percentile() {
        assert_eq!(
            LatencyHistogram::new().percentile_upper_bound_micros(50),
            None
        );
    }

    #[test]
    fn renders_registry_metrics() {
        let registry = MetricRegistry::new();
        registry.inc_counter(
            "roze_jobs_total",
            MetricLabels::new().insert("job", "sync"),
            2,
        );
        registry.set_gauge("roze_queue_depth", MetricLabels::new(), 7.0);
        let rendered = registry.render();
        assert!(rendered.contains("roze_jobs_total"));
        assert!(rendered.contains("roze_queue_depth"));
    }

    #[test]
    fn escapes_label_values() {
        let registry = MetricRegistry::new();
        registry.inc_counter(
            "roze_events_total",
            MetricLabels::new().insert("path", "C:\\tmp\n\"quoted\""),
            1,
        );

        let rendered = registry.render();
        assert!(rendered.contains(r#"path="C:\\tmp\n\"quoted\"""#));
    }

    #[test]
    fn normalizes_label_keys() {
        let registry = MetricRegistry::new();
        registry.inc_counter(
            "roze_events_total",
            MetricLabels::new()
                .insert("http.status-code", "200")
                .insert("1route", "/healthz")
                .insert("", "empty"),
            1,
        );

        let rendered = registry.render();
        assert!(rendered.contains(r#"http_status_code="200""#));
        assert!(rendered.contains(r#"_1route="/healthz""#));
        assert!(rendered.contains(r#"_="empty""#));
    }

    #[test]
    fn renders_route_and_rpc_metrics_with_labels() {
        record_http_route("svc", "/users/:id", "GET", "200", Duration::from_millis(7));
        record_rpc_method("svc", "GetUser", "ok", Duration::from_millis(11));
        record_rpc_client_attempt("svc", "GetUser", "timeout");

        let metrics = http_metrics();
        assert!(metrics.contains("roze_http_route_requests_total"));
        assert!(metrics.contains(r#"service="svc""#));
        assert!(metrics.contains(r#"route="/users/:id""#));
        assert!(metrics.contains("roze_rpc_method_requests_total"));
        assert!(metrics.contains("roze_rpc_client_attempts_total"));
        assert!(metrics.contains(r#"method="GetUser""#));
        assert!(metrics.contains(r#"outcome="timeout""#));
    }

    #[test]
    fn rpc_client_attempt_normalizes_adversarial_outcomes() {
        let unique_error = format!("tenant-17 /orders/{} secret failure", std::process::id());
        record_rpc_client_attempt("svc", "GetUser", unique_error.clone());

        let metrics = http_metrics();
        assert!(metrics.contains(r#"outcome="other""#));
        assert!(!metrics.contains(&unique_error));
    }

    #[test]
    fn adversarial_rpc_attempt_values_create_one_bounded_series() {
        let registry = MetricRegistry::new();
        for index in 0..1_000 {
            let outcome = normalize_rpc_client_attempt_outcome(format!(
                "tenant-{index} /orders/{index} secret error {index}"
            ));
            let labels = MetricLabels::new()
                .insert("service", "checkout")
                .insert("method", "CreateOrder")
                .insert("outcome", outcome);
            registry.inc_counter("roze_rpc_client_attempts_total", labels, 1);
        }

        assert_eq!(registry.series_count(), 1);
        let rendered = registry.render();
        assert!(rendered.contains(r#"outcome="other""#));
        assert!(!rendered.contains("tenant-999"));
    }

    #[test]
    fn renders_gateway_metrics_with_labels() {
        record_gateway_route(
            "gateway",
            "/api",
            "GET",
            "200",
            "ok",
            Duration::from_millis(9),
        );
        record_gateway_retry("gateway", "/api", "status_503");
        record_gateway_upstream("gateway", "http://127.0.0.1:8080", "ejected");
        record_gateway_config_reload("applied");

        let metrics = http_metrics();
        assert!(metrics.contains("roze_gateway_route_requests_total"));
        assert!(metrics.contains("roze_gateway_route_retries_total"));
        assert!(metrics.contains("roze_gateway_upstream_events_total"));
        assert!(metrics.contains("roze_gateway_config_reloads_total"));
        assert!(metrics.contains(r#"outcome="ok""#));
    }

    #[test]
    fn renders_queue_metrics_with_labels() {
        record_queue_event("kafka", "orders", "workers", "acked");
        record_queue_offset("kafka", "orders", "workers", 0, 42);

        let metrics = http_metrics();

        assert!(metrics.contains("roze_queue_events_total"));
        assert!(metrics.contains("roze_queue_last_offset"));
        assert!(metrics.contains(r#"system="kafka""#));
        assert!(metrics.contains(r#"topic="orders""#));
        assert!(metrics.contains(r#"group="workers""#));
        assert!(metrics.contains(r#"partition="0""#));
        assert!(metrics.contains(r#"outcome="acked""#));
    }

    #[test]
    fn renders_resilience_decision_metrics() {
        record_resilience_decision("catalog", "rest", "breaker", "open");

        let metrics = http_metrics();

        assert!(metrics.contains("roze_resilience_decisions_total"));
        assert!(metrics.contains(r#"service="catalog""#));
        assert!(metrics.contains(r#"boundary="rest""#));
        assert!(metrics.contains(r#"kind="breaker""#));
        assert!(metrics.contains(r#"decision="open""#));
    }
}
