use roze_metrics::{MetricLabels, MetricRegistry};

#[derive(Debug, Clone, Default)]
pub struct PrometheusExporter {
    registry: MetricRegistry,
}

impl PrometheusExporter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn registry(&self) -> MetricRegistry {
        self.registry.clone()
    }

    pub fn inc_counter(&self, name: impl Into<String>, labels: MetricLabels, by: u64) {
        self.registry.inc_counter(name, labels, by);
    }

    pub fn set_gauge(&self, name: impl Into<String>, labels: MetricLabels, value: f64) {
        self.registry.set_gauge(name, labels, value);
    }

    pub fn observe_duration(
        &self,
        name: impl Into<String>,
        labels: MetricLabels,
        value: std::time::Duration,
    ) {
        self.registry.observe_duration(name, labels, value);
    }

    pub fn render(&self) -> String {
        self.registry.render()
    }
}

pub fn render_http_metrics() -> String {
    roze_metrics::http_metrics()
}

pub fn render_registry_metrics(registry: &MetricRegistry) -> String {
    registry.render()
}

pub fn render_service_metrics(service: impl AsRef<str>, uptime_seconds: u64) -> String {
    format!(
        concat!(
            "# HELP roze_service_uptime_seconds Service uptime in seconds\n",
            "# TYPE roze_service_uptime_seconds gauge\n",
            "roze_service_uptime_seconds{{service=\"{}\"}} {}\n"
        ),
        service.as_ref(),
        uptime_seconds
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn renders_metrics() {
        let text = render_service_metrics("demo", 7);
        assert!(text.contains("roze_service_uptime_seconds"));
    }

    #[test]
    fn exports_registry_metrics() {
        let exporter = PrometheusExporter::new();
        exporter.inc_counter(
            "roze_jobs_total",
            MetricLabels::new().insert("job", "sync"),
            1,
        );
        exporter.set_gauge("roze_queue_depth", MetricLabels::new(), 2.0);
        exporter.observe_duration(
            "roze_job_duration",
            MetricLabels::new(),
            Duration::from_millis(5),
        );
        let rendered = exporter.render();
        assert!(rendered.contains("roze_jobs_total"));
        assert!(rendered.contains("roze_queue_depth"));
        assert!(rendered.contains("roze_job_duration"));
    }
}
