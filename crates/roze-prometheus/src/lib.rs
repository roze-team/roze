pub fn render_http_metrics() -> String {
    roze_metrics::http_metrics()
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

    #[test]
    fn renders_metrics() {
        let text = render_service_metrics("demo", 7);
        assert!(text.contains("roze_service_uptime_seconds"));
    }
}
