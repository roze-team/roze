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

JWT/API-key enforcement is active on the native path. WebSocket/SSE streaming,
CORS, and live config replacement remain separate work and must not be claimed
until their native integration tests pass.
