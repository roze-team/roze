# Roze Gateway SLO Template

This template defines a minimal production SLO for `apps/roze-gateway`.

## Availability

Target: `99.9%` successful gateway responses over 30 days.

Success signal:

```promql
1 - roze_gateway:route_error_ratio:rate1h
```

Default warning threshold:

```promql
roze_gateway:route_error_ratio:rate5m > 0.05
```

Default page threshold:

```promql
roze_gateway:route_error_ratio:rate1h > 0.01
```

## Stream Connectivity

Target: no unexpected SSE/WebSocket rejections for healthy routes.

Warning signal:

```promql
roze_gateway:stream_connection_rejects:increase5m > 0
```

Operational notes:

- Rejections are expected only when `max_stream_connections` is intentionally protecting the gateway.
- If rejections are unexpected, check route-level connection limits, active connection count, and upstream SSE/WebSocket client behavior.
- Long-lived streams should send heartbeat events so `stream_idle_timeout_ms` can distinguish healthy idle connections from stalled streams.

## Retry Budget

Retry spikes are early signals of upstream instability.

Warning signal:

```promql
sum by (service, route, reason) (roze_gateway:route_retries:rate5m) > 1
```

Operational notes:

- Compare retry spikes with `roze_gateway:upstream_events:rate5m`.
- If retries are caused by `status_5xx`, inspect upstream service health and outlier ejection state.
- If retries are caused by `timeout`, check route `timeout_ms`, upstream latency, and registry target health.

