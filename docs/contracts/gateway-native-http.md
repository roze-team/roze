# Gateway Native HTTP Governance

The native HTTP gateway compiles route policy once when the runtime is built.
Explicit route fields take precedence over `governance.routes`, followed by
global governance and gateway defaults. Ordinary HTTP requests enforce method
constraints, request-size limits, rate limits, the shared closed/open/half-open
breaker, bounded adaptive shedding, timeout, fallback, and gateway metrics.

Retries are restricted to idempotent HTTP methods (`GET`, `HEAD`, `PUT`,
`DELETE`, `OPTIONS`, and `TRACE`). Network failures and status 500, 502, 503,
and 504 use the shared exponential full-jitter algorithm, `max_backoff_ms`, and
the service/route retry budget. The inbound Roze deadline caps the full logical
request; a retry is not scheduled when its backoff would consume the remaining
deadline. `attempt` metrics count only calls that actually reach the upstream.
The circuit breaker is settled once for the final logical result rather than
once per retry attempt.

Registry-backed services are discovered before every upstream attempt. Service
and route instance tags are merged, with route tags taking precedence, and tag
mismatches fail closed. Weighted selection uses a bounded cursor algorithm and
does not expand the candidate list by weight. Connection errors, timeouts, and
5xx responses feed passive outlier ejection; a retry therefore selects again
from the remaining healthy instances. Optional active probes apply independent
healthy and unhealthy thresholds to registry and static targets. Probe tasks
hold only a weak runtime reference and are aborted when the gateway is dropped.

Native CORS handling runs before route matching and the governance chain.
Preflight requests validate the configured origin, requested method, and
requested headers without consuming rate-limit, breaker, shedding, or upstream
capacity. Successful preflights return 204 with stable allow headers, optional
max age, and cache-safe `Vary` values. Ordinary responses emit allow-origin only
for configured origins. Preflight acceptance and rejection are included in
gateway and HTTP metrics.

SSE responses are forwarded as frames without buffering the complete upstream
body. The normal route timeout covers request transmission and response headers;
after an SSE response is established, `stream_idle_timeout_ms` independently
limits time between chunks. `max_stream_connections` follows route, service,
then gateway precedence. Connection permits live inside the response body, so
client disconnect, upstream completion, stream error, or idle timeout releases
capacity and records opened/rejected/closed events plus connection duration.

WebSocket requests pass through the same route matching, authentication, rate
limit, breaker, shedding, registry, tag, health, and outlier decisions before
the upgrade. The gateway rewrites the upstream path, validates the RFC 6455
version/key and cryptographic `Sec-WebSocket-Accept`, rejects unrequested
subprotocols, then runs a bidirectional byte tunnel. The route/service/gateway
stream connection limit and idle timeout also apply to WebSocket tunnels, with
the same lifecycle metrics as SSE. The native server explicitly enables Hyper
upgrades and integration smoke covers 101, masked client-to-upstream traffic,
upstream-to-client traffic, and concurrent-connection rejection. `wss` and
`https` WebSocket upstreams use Tokio-Rustls with an explicitly selected Ring
provider, strict SNI/certificate validation, bundled WebPKI public roots plus
valid system roots, and the remaining route handshake deadline. TLS state is
initialized once per gateway runtime and plaintext fallback is forbidden.

JWT/API-key enforcement is active on the native path. Private CA/client-certificate
mTLS configuration remains separate work and must not be claimed until its
native integration tests pass.

Config-center reloads build a complete immutable gateway runtime before an
atomic `ArcSwap` replacement. New requests use the new snapshot immediately;
in-flight HTTP, SSE, and WebSocket work retains its existing snapshot. Rate
limits, breaker state, retry budgets, outlier/health state, stream capacity,
registry cursors, the HTTP client, and TLS configuration survive replacement.
Changes outside `gateway`, `auth`, `governance`, or `registry` are skipped.
Parse errors, invalid gateway policy, registry construction failures, removal
of the gateway section, and listen-address changes retain the last valid
runtime. Applied, skipped, and failed outcomes have structured events and
`roze_gateway_config_reloads_total{outcome}` metrics.
