# Middleware Contract

Roze HTTP middleware is split into two layers:

- Service-wide middleware configured by `rest.middlewares` in `config.yaml`.
- Route-scoped governance and custom hooks declared in `.api` with `@server`,
  `@middleware`, or `jwt`.

Generated REST services call
`roze_middleware::apply_common_with_config(route::router(ctx), config)` from
`src/main.rs`. The generated `config.yaml` exposes the service-wide knobs under
`rest.middlewares`.

## Service-wide Middleware

```yaml
rest:
  addr: 127.0.0.1:3000
  register: false
  middlewares:
    recover: true
    trace: true
    stat: true
    prometheus: true
    cors: true
    # cors_config:
    #   allow_origins: ["*"]
    #   allow_methods: ["GET", "POST", "PUT", "PATCH", "DELETE"]
    #   allow_headers: ["authorization", "content-type", "x-request-id", "x-trace-id"]
    #   expose_headers: ["x-request-id", "x-trace-id"]
    #   allow_credentials: false
    #   max_age_seconds: 3600
    timeout: true
    # max_conns: 1000
    # shedding:
    #   concurrency: 1000
    #   window_ms: 1000
    #   min_samples: 100
    #   max_avg_latency_ms: 500
    #   max_failure_ratio_per_mille: 500
    #   cool_down_ms: 1000
    # gunzip: true
    # request_body_limit_bytes: 2097152
```

| Field | Default | Behavior |
| --- | --- | --- |
| `recover` | `true` | Converts panics into HTTP responses through Tower HTTP panic recovery. |
| `trace` | `true` | Creates request spans and logs request completion/failure. |
| `stat` | `true` | Enables request metrics recording through the trace middleware. |
| `prometheus` | `true` | Keeps metrics collection enabled for `/metrics` output. |
| `cors` | `true` | Applies CORS. Without `cors_config`, the policy is permissive. |
| `cors_config` | unset | Optional allow origins, methods, headers, exposed headers, credentials, and max age. |
| `timeout` | `true` | Enables framework-level timeout enforcement. Generated routes apply the service-wide `governance.timeout_ms` as a Tower HTTP timeout layer; generated handler adapters still enforce route-specific effective timeouts from `governance.routes`. |
| `max_conns` | unset | Hard concurrent request cap. Exceeded requests return `503`. |
| `shedding` | unset | Adaptive load shedding. Exceeded concurrency or unhealthy recent windows return `503`. |
| `gunzip` | `false` | Decompresses gzip request bodies before extraction. |
| `request_body_limit_bytes` | unset | Rejects oversized request bodies with `413`. |

`trace`, `stat`, and `prometheus` share the same request observation path today:
`trace` creates spans/logs, and the same layer records HTTP metrics. `/metrics`
is still served by generated route glue.

## CORS

`cors: true` keeps CORS enabled. If `cors_config` is omitted, Roze uses a
permissive policy for newly generated services. Set `cors_config` to restrict
origins and preflight behavior:

```yaml
rest:
  middlewares:
    cors: true
    cors_config:
      allow_origins: ["https://app.example.com"]
      allow_methods: ["GET", "POST", "PUT", "PATCH", "DELETE"]
      allow_headers: ["authorization", "content-type"]
      expose_headers: ["x-request-id", "x-trace-id"]
      allow_credentials: true
      max_age_seconds: 3600
```

Use `["*"]` for wildcard origins, methods, or headers. Do not combine wildcard
origins with credentialed browser requests in production; browsers reject that
combination by specification.

## Adaptive Shedding

`shedding` combines a hard concurrency limit with a rolling health window:

- `concurrency`: maximum concurrent requests allowed through the shedding guard.
- `window_ms`: statistics window duration.
- `min_samples`: minimum completed requests before latency/failure thresholds
  can trigger shedding.
- `max_avg_latency_ms`: average latency threshold for the active window.
- `max_failure_ratio_per_mille`: failure ratio threshold, in per-mille units.
  For example, `500` means 50%.
- `cool_down_ms`: how long to reject requests after an unhealthy window is
  detected.

The guard records completed request status and elapsed time. If a window has at
least `min_samples` requests and either average latency or failure ratio exceeds
the configured threshold, the service enters cool-down and returns `503 service
overloaded` until the cool-down expires.

## Route-scoped Middleware

`.api` middleware names are resolved into built-ins and custom hooks.

Built-in names are not generated as `src/middleware/<name>.rs` stubs:

| Built-in | Accepted aliases |
| --- | --- |
| Auth | `auth`, `jwt` |
| Trace | `trace`, `tracing` |
| Recover | `recover`, `recovery`, `panic_recover` |
| Stat | `stat`, `stats` |
| Prometheus | `prometheus`, `metrics`, `metric` |
| CORS | `cors` |
| Timeout | `timeout` |
| Rate limit | `rate_limit`, `ratelimit`, `rate` |
| Breaker | `breaker`, `circuit_breaker` |
| Max connections | `max_conns`, `max_connections`, `max_conn`, `max_connection` |
| Shedding | `shedding`, `load_shed`, `load_shedding` |
| Gunzip | `gunzip`, `gzip`, `request_gunzip` |
| Body limit | `body_limit`, `request_body_limit`, `max_bytes`, `max_body_bytes` |
| Idempotency | `idempotency`, `idempotency_key` |

Unknown names are treated as custom application middleware. The generator
creates `src/middleware/<name>.rs` once and preserves it during `--update`.

Example:

```go
@server (
  prefix: /api/v1
  middleware: auth, trace, audit
)
service user-api {
  @handler getUser
  get /users/:id (GetUserReq) returns (UserResp)
}
```

`auth` and `trace` are built-ins. `audit` is custom, so the generated handler
calls `crate::middleware::audit(&ctx, &request_ctx).await`.

## Governance Interaction

Route governance still lives under `governance`:

```yaml
governance:
  timeout_ms: 5000
  retry:
    max_attempts: 2
    backoff_ms: 50
    max_backoff_ms: 500
  rate_limit:
    burst: 100
    refill_ms: 10
  breaker:
    failure_threshold: 5
    reset_timeout_ms: 30000
  shedding:
    concurrency: 1000
    max_avg_latency_ms: 500
  fallback:
    enabled: true
    status: 503
    body:
      code: 503
      message: degraded
    headers:
      x-roze-fallback: governance
  routes: {}
```

`begin_route` applies route/global rate limit and breaker policy and attaches
the effective timeout to `roze_context::Context`. `shedding` is enforced before
logic execution, and `fallback` is resolved with route policy taking precedence
over global policy. Disabled fallback entries are ignored, so generated services
default to explicit fail-closed behavior until an operator enables a degradation
path with evidence. Generated REST adapters apply fallback only to non-client
errors and render the configured status, JSON body, and headers through
`RozeError::Fallback`; RPC adapters expose fallback status/body as gRPC
metadata while preserving an unavailable status for typed clients, and
`roze_rpc::rpc::error_from_status` restores that metadata into
`RozeError::Fallback` for application code that wants structured degradation
handling. `retry`, `shedding`, and `fallback` are part of the shared governance
schema so Gateway/RPC/MQ can use the same configuration model where applicable.

Timeout is intentionally a framework concern:

- Generated route glue applies the service-wide `governance.timeout_ms` through
  `roze_middleware::apply_timeout`, which uses Tower HTTP timeout middleware.
- Generated handler adapters enforce route-specific effective timeouts when
  `governance.routes` overrides the global value.
- Business logic should not create generic request timeout wrappers. Logic code
  should only handle domain-specific deadlines when the business operation
  itself requires them.

## Ownership

- Service-wide middleware config is owned by `config.yaml`; `--update`
  preserves existing `config.yaml`.
- Built-in route middleware is generator/framework-owned.
- Custom route middleware files are application-owned and preserved on
  `--update`.
