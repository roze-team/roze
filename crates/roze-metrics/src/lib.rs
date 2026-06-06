use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

static REQUEST_TOTAL: AtomicU64 = AtomicU64::new(0);
static REQUEST_FAILED: AtomicU64 = AtomicU64::new(0);
static REQUEST_ELAPSED_MS: AtomicU64 = AtomicU64::new(0);

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

#[derive(Debug, Default, Clone)]
pub struct MetricRegistry {
    inner: Arc<Mutex<BTreeMap<String, BTreeMap<MetricLabels, MetricValue>>>>,
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

    pub fn render(&self) -> String {
        let inner = self.inner.lock().expect("metric registry lock poisoned");
        let mut out = String::new();
        for (name, entries) in inner.iter() {
            for (labels, value) in entries {
                let labels_text = if labels.0.is_empty() {
                    String::new()
                } else {
                    let joined = labels
                        .0
                        .iter()
                        .map(|(key, value)| format!(r#"{key}="{value}""#))
                        .collect::<Vec<_>>()
                        .join(",");
                    format!("{{{joined}}}")
                };
                match value {
                    MetricValue::Counter(value) => {
                        out.push_str(&format!("{name}{labels_text} {value}\n"));
                    }
                    MetricValue::Gauge(value) => {
                        out.push_str(&format!("{name}{labels_text} {value}\n"));
                    }
                    MetricValue::Duration(value) => {
                        out.push_str(&format!("{name}{labels_text} {}\n", value.as_millis()));
                    }
                }
            }
        }
        out
    }

    fn update<F>(&self, name: String, labels: MetricLabels, f: F)
    where
        F: FnOnce(Option<MetricValue>) -> MetricValue,
    {
        let mut inner = self.inner.lock().expect("metric registry lock poisoned");
        let entry = inner.entry(name).or_default();
        let current = entry.remove(&labels);
        entry.insert(labels, f(current));
    }
}

pub fn record_http_request(success: bool, elapsed: Duration) {
    REQUEST_TOTAL.fetch_add(1, Ordering::Relaxed);
    if !success {
        REQUEST_FAILED.fetch_add(1, Ordering::Relaxed);
    }
    REQUEST_ELAPSED_MS.fetch_add(elapsed.as_millis() as u64, Ordering::Relaxed);
}

pub fn http_metrics() -> String {
    let total = REQUEST_TOTAL.load(Ordering::Relaxed);
    let failed = REQUEST_FAILED.load(Ordering::Relaxed);
    let elapsed_ms = REQUEST_ELAPSED_MS.load(Ordering::Relaxed);
    let avg_ms = if total == 0 { 0 } else { elapsed_ms / total };

    format!(
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
    )
}

pub fn service_registry() -> MetricRegistry {
    MetricRegistry::new()
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
    fn renders_registry_metrics() {
        let registry = MetricRegistry::new();
        registry.inc_counter("roze_jobs_total", MetricLabels::new().insert("job", "sync"), 2);
        registry.set_gauge("roze_queue_depth", MetricLabels::new(), 7.0);
        let rendered = registry.render();
        assert!(rendered.contains("roze_jobs_total"));
        assert!(rendered.contains("roze_queue_depth"));
    }
}
