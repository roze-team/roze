# Distributed Rate Limiting

Roze uses one rate-limit contract across generated REST services, RPC services,
and `roze-gateway`. The policy selects an identity, while the store owns token
bucket state. This keeps application code independent from local or Redis
storage.

## Configuration

```yaml
governance:
  rate_limiter:
    store: auto
    # Explicit URL takes precedence over cache.url.
    # redis_url: env://REDIS_URL
    key_prefix: roze:rate-limit:v1
    # Defaults to development, test, or production from ServiceConfig.profile.
    # namespace: production-eu
    timeout_ms: 100
    unavailable_policy: fail-closed
  rate_limit:
    burst: 100
    refill_ms: 10
    key:
      dimensions: [route, client_ip, tenant]
      headers: []
      missing: reject
```

`governance.routes.<operation>.rate_limit` overrides the global limit and may
define its own key policy. Supported dimensions are:

- `route`: service, boundary (`rest`, `rpc`, or `gateway`), and operation.
- `client_ip`: the verified client address.
- `subject`: authenticated subject from `roze_context::Context`.
- `tenant`: authenticated tenant from `roze_context::Context`.
- `headers`: explicitly named request headers or RPC metadata.

Dimensions and headers form a composite key in declaration order. `missing:
reject` rejects requests whose selected identity is unavailable. `missing:
omit` is intended only for policies that deliberately group anonymous and
authenticated traffic. Roze hashes the complete key material before storage;
raw IP addresses, subjects, tenants, and header values are not used as Redis
keys or metric labels.

Header dimensions must not contain secrets. Prefer authenticated `subject` or
`tenant` claims over client-controlled headers.

## Client IP trust boundary

Generated REST services obtain `client_ip` from `roze_middleware::ClientIp`.
Enable `rest.connect_info` so the TCP peer address is available. Forwarded
headers are considered only when the peer matches `trusted_proxy_cidrs`.

The gateway follows the same rule through `gateway.trusted_proxy_cidrs`.
The default list is empty, so an untrusted client cannot select another quota
bucket by forging `X-Forwarded-For`.

RPC has no TCP client-IP dimension in its public method contract. Select
`subject`, `tenant`, route, or trusted metadata for RPC policies. Configuring an
unavailable dimension with `missing: reject` fails that request explicitly.

## Stores and failure behavior

`memory` is deterministic and suitable for tests or a single development
process. Generated production services and the gateway fail during startup
when rate limiting is enabled but the configured store is `memory`.

`auto` is the generated default. It selects an explicit
`governance.rate_limiter.redis_url`, then `cache.url`, and otherwise uses memory
only outside production. `redis` follows the same URL precedence but requires a
usable URL. This lets deployments share their configured Redis connection
location without duplicating a secret while preserving an explicit override for
isolated rate-limit infrastructure.

`redis` uses one Lua operation and Redis server time to refill and consume a
token atomically. Multiple instances therefore share one quota without relying
on application clock synchronization. Bucket state has a bounded TTL and
survives an application restart.

Redis keys use `key_prefix` plus a namespace. Generated services default the
namespace to the service profile, while the hashed identity already includes
the service, boundary, and operation. Deployments that share Redis across
several production environments should set an explicit environment namespace.

Every store operation is bounded by `timeout_ms`:

- `fail-closed` returns `503` / RPC `Unavailable` when the store cannot decide.
- `fail-open` allows the request and records a degraded decision.

Redis-backed generated services register the store in readiness checks.
Connection strings are redacted from configuration debug output.

## Startup validation

`roze_config::load_service` resolves secrets and validates the complete service
configuration before listeners start. It rejects zero limits or timeouts,
empty/duplicate key dimensions, invalid or duplicate header names, invalid
governance ranges, missing production Redis configuration, and empty
namespaces.

Unknown configuration fields are fatal in the production profile, including
their full object path in the error. Development and test profiles keep unknown
fields warning-only so application-owned experimental sections remain usable.
`ServiceConfig` debug output reports section presence and bounded identifiers,
not database, cache, broker, object-storage, RPC token, or connection secrets.

## Protocol behavior and observability

A depleted bucket returns:

- REST and Gateway: HTTP `429 Too Many Requests` with integer `Retry-After`
  seconds.
- RPC: gRPC `ResourceExhausted` with `retry-after` response metadata.

Decisions use bounded labels:

`roze_resilience_decisions_total{service,boundary,kind="rate_limit",decision}`

The decision label distinguishes allowed, rejected, identity rejection, and
store failure policy outcomes. Identity values and arbitrary header values are
never metric labels.

Gateway configuration-center reloads rebuild the limiter and atomically replace
the active runtime. Generated REST and RPC processes load their limiter during
startup; changing their store or key policy currently requires the normal
service configuration rollout.

## Verification

Unit and contract tests cover identity isolation, missing dimensions,
fail-open/fail-closed behavior, protocol error mapping, and response metadata.
The ignored Redis integration test uses `ROZE_TEST_REDIS_URL` and verifies two
independent limiter instances share an atomic quota and that state remains
effective after an application-side limiter restart:

```bash
cargo test -p roze-rate-limit two_instances_share_atomic_redis_quota_across_restart -- --ignored
```
