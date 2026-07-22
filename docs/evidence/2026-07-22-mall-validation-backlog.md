# Mall Validation Backlog - 2026-07-22

This note records issues and follow-up optimization requests verified while
upgrading a mall system with Roze-generated REST, RPC, SQL model, OpenAPI, and
TypeScript SDK boundaries.

## Verified Fixes

- PostgreSQL `BIGINT`, `BIGSERIAL`, and `INT8` now generate signed `i64`
  fields instead of PostgreSQL-unsupported `u64` SQLx/SeaORM mappings.
- PostgreSQL `TIMESTAMPTZ` now generates SeaORM `DateTimeUtc` fields, and
  generated manifests include the required `chrono` serde support.
- REST, five RPC services, SQL models, OpenAPI, and the TypeScript SDK were
  regenerated with the updated `rozectl --update` flow and passed workspace
  compilation in the mall system.

## Upstream Optimization Backlog

### 1. Etcd TLS and Authentication Are Not Passed Into Registry

Severity: high.

`RpcClientEtcdConfig` contains `user`, `pass`, `cert_file`,
`cert_key_file`, `ca_cert_file`, and `insecure_skip_verify`, but these fields
are not carried into `RegistryConfig`. `EtcdRegistry` also builds a fixed
`reqwest::Client::new()`, so services cannot connect directly to etcd
deployments that require TLS and authentication.

Impact: production deployments are limited to unauthenticated HTTP etcd unless
they add a local proxy that terminates TLS and injects authentication. The mall
system currently compensates with a `127.0.0.1:2379` proxy.

Requested improvements:

- Move authentication, CA, client certificate, and verification policy into
  `RegistryConfig`.
- Build the etcd HTTP client from registry configuration.
- Attach etcd auth tokens to every request.
- Refresh tokens automatically when they expire or the etcd auth revision
  changes by calling `/v3/auth/authenticate` again.
- Share the same authenticated client across register, discover, watch,
  keepalive, and re-register paths.
- Add a three-node TLS plus authenticated etcd integration test covering watch,
  leader failover, and token refresh.

### 2. Generated Dependency Readiness Uses Static Healthy Markers

Severity: high.

Generated `ServiceContext::new` currently marks dependencies healthy after
initialization succeeds. If PostgreSQL, MySQL, Redis, NATS, etcd, or an RPC
dependency later becomes unavailable, `/readyz` can continue reporting healthy
state because it is no longer probing the dependency.

Requested improvements:

- Generate dynamic readiness dependencies with
  `HealthRegistry::register_dependency`.
- Probe databases with `SELECT 1`.
- Probe Redis with `PING`.
- Probe NATS through connection state or server info.
- Probe etcd through an authenticated health endpoint.
- Probe RPC dependencies through gRPC health where available, or at least by
  validating service discovery and connection establishment.
- Generate standard HTTP `/healthz`, `/readyz`, and `/startupz` for RPC
  services and share the same `HealthRegistry` with gRPC health.
- Keep readiness probes timeout-bound, concurrent, labeled, and summarized;
  liveness and readiness must remain separate states.

### 3. Persistent IdempotencyStore and OutboxStore Adapters Are Missing

Severity: medium.

Roze provides `InMemoryIdempotencyStore` and `InMemoryOutbox`, but the default
framework does not yet include a Redis-backed idempotency adapter or SQL-backed
outbox adapter. After a service restart, the default adapters cannot preserve
idempotency records or pending events.

Mall-system compensation:

- The management API uses Redis Lua scripts for atomic begin, complete, fail,
  request-fingerprint conflict detection, execution leases, and response
  replay.
- A shared runtime SQL outbox supports PostgreSQL and MySQL, `FOR UPDATE SKIP
  LOCKED`, lease recovery, exponential backoff, and dead-lettering after ten
  failures.

Requested improvements:

- Add an official Redis idempotency store.
- Add an official SQL outbox store under the transaction/runtime stack.
- Include schema migrations, maximum retry policy, dead-letter query and
  replay APIs, and monitoring metrics.

## Regression Baseline For Roze Upgrades

After upgrading Roze or `rozectl`, the mall system should run at least:

- `make generate` twice to verify deterministic generation and preservation of
  application-owned files.
- `make artifacts`.
- `cargo check --workspace`.
- `cargo test --workspace`.
- Frontend `npm run check`.
- PostgreSQL and MySQL migrations, real CRUD, outbox, and idempotency E2E
  tests.
- Etcd watch, instance removal, lease expiry, re-registration, dual-instance
  load balancing, and leader failover fault injection.
