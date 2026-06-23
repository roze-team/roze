# Roze Observability Pack

This directory contains minimal Prometheus and Grafana assets for Roze services.

## Files

- `prometheus.yml`: local scrape example for `apps/roze-gateway`.
- `prometheus-rules/roze-gateway-alerts.yml`: alert rules for gateway route errors, retries, and SSE/WebSocket stream connections.
- `prometheus-rules/roze-gateway-recording-rules.yml`: recording rules for gateway request, error, retry, upstream, and stream metrics.
- `grafana/roze-gateway-dashboard.json`: importable Grafana dashboard for gateway request, upstream, retry, and stream connection metrics.
- `slo/roze-gateway-slo.md`: minimal gateway SLO template.

## Local Prometheus Example

Mount this directory into Prometheus and start with:

```bash
prometheus --config.file=/etc/prometheus/prometheus.yml
```

For Docker, mount:

```text
deploy/observability/prometheus.yml -> /etc/prometheus/prometheus.yml
deploy/observability/prometheus-rules -> /etc/prometheus/rules
```

The gateway target defaults to `127.0.0.1:8081`. Change `scrape_configs[].static_configs[].targets` if your gateway listens elsewhere.
