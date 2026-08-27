# Logging contract

Roze uses `tracing` for one structured logging contract across REST, RPC,
Stream, WebSocket, Maud HTML, AI, jobs, and lifecycle boundaries. Local log
output is configured by `ServiceConfig.logging`; distributed span export stays
under `ServiceConfig.telemetry`.

## Configuration

When `logging` is omitted, Roze preserves the development default: enabled
text output to stdout at `info` level. A production-oriented configuration is:

```yaml
logging:
  enabled: true
  level: info
  # Explicit config wins over RUST_LOG; omit to allow RUST_LOG.
  env_filter: "info,hyper=warn,tower_http=warn"
  format: json
  stdout: true
  ansi: false
  target: true
  caller: false
  thread_ids: false
  span_events: none
  utc_time: true
  time_format: "%Y-%m-%dT%H:%M:%S%.3f%:z"
  non_blocking_buffer: 8192
  lossy: true
  file:
    directory: logs
    file_name_pattern: "service.{date}.log"
    rotation: daily
    date_format: "%Y-%m-%d"
    max_file_size_bytes: 104857600
    compression: gzip
    compression_level: 6
    retention_days: 7
    maintenance_interval_secs: 3600
```

Filter precedence is `logging.env_filter`, then `RUST_LOG`, then
`logging.level`. File output uses a bounded asynchronous writer. With
`lossy: true`, a full queue drops lines instead of blocking service work;
`TracingGuard::dropped_lines` exposes the writer count. Roze also publishes
`roze_log_lines_dropped_total{sink="file"}` and emits the bounded
`log.lines.dropped` event when maintenance observes new drops. Keep the returned
guard alive until service shutdown so pending lines and OpenTelemetry spans flush.

Hourly and daily rotation require exactly one `{date}` token in
`file_name_pattern`; `rotation: never` forbids it. `date_format` uses Chrono
strftime syntax and follows `logging.utc_time`. A non-zero
`max_file_size_bytes` adds numeric segments before the extension, for example
`service.2026-08-28.1.log`. Set it to `0` to disable size rotation.

`compression: gzip` compresses each inactive segment independently using
`compression_level` from `0` through `9`; `compression: none` disables it.
This is per-file gzip compression, not a multi-file tar/zip archive. Rotated
files are compressed and expired only when the date segment matches the
configured pattern. The active file and unrelated files are never maintained.
Startup fails when the configured sink cannot be prepared or the subscriber
cannot be installed.

The former `file_name` and `compress_rotated` fields are removed rather than
aliased. `LogFileConfig` rejects unknown fields in every profile, so deployments
must migrate atomically to `file_name_pattern` and `compression`.

## Event schema

Every operational log uses a stable dotted `event` name. JSON output always
adds timestamp, level and target; boundary events additionally use the relevant
bounded fields:

- `service` and `protocol`
- `operation`, route, method, or topic
- `request_id` and `trace_id` for request/message-scoped work
- `elapsed_ms`, outcome, status, numeric code, and `error_kind` where applicable
- `response_kind: html` and `content_type: text/html; charset=utf-8` for Maud

Use constants from `roze_log::events` for shared framework event names. Normal
milestones are `INFO`, client rejection/cancellation is `WARN`, and failures are
`ERROR`. Do not emit a completion event for a failed operation; use the matching
failure event.

## Sensitive data

Never log request/message bodies, rendered HTML, authorization or cookie
values, credentials, tokens, SQL arguments, dependency response bodies, or
fallback payloads. Prefer omitting a sensitive field. If a value must be
represented, wrap it with `roze_log::Sensitive`, which formats only as
`[REDACTED]` for both `Display` and `Debug`.

Configuration redaction protects `Debug` formatting only. It does not make
`%config.secret`, serialization, or arbitrary error strings safe.

## Generated code

Generated binaries bind the returned `TracingGuard` for the complete service
lifetime. REST/RPC logic emits started and exactly one completed/failed event
with elapsed time and request correlation. Maud rendering emits
`html.render.completed` with render time and content type, never the HTML body.
Stream consumers retain topic/partition/offset/attempt and trace context but do
not log message payloads.
