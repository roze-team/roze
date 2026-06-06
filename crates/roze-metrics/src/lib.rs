use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

static REQUEST_TOTAL: AtomicU64 = AtomicU64::new(0);
static REQUEST_FAILED: AtomicU64 = AtomicU64::new(0);
static REQUEST_ELAPSED_MS: AtomicU64 = AtomicU64::new(0);

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
}
