# Roze Upstream Issue Tracking - 2026-07-22

This file tracks only issues that are still reproducible in the current Roze
revision and require upstream work. Generator compatibility issues already
verified as fixed are not kept in the active backlog.

## Verification Environment

- Verification date: 2026-07-22
- Roze revision: `4e20455305fd295361ce0a22887527953cd54801`
- CLI: `rozectl 1.0.0`
- CLI path: `C:\Users\xFc\.cargo\bin\rozectl.exe`

## Reproducible Issues

### 1. Registry Does Not Use Etcd TLS And Authentication Configuration

Severity: high.

`RpcClientEtcdConfig` contains user, password, CA, client certificate, and
certificate verification options, but `RegistryConfig` keeps only endpoints,
prefix, TTL, and keepalive interval. `EtcdRegistry::new` still builds a fixed
`reqwest::Client::new()`, and requests do not include token acquisition or
refresh.

Expected native Roze support:

- TLS, CA, and mTLS client configuration.
- Etcd user authentication and automatic token refresh.
- A shared authenticated client for register, discover, watch, keepalive, and
  re-registration paths.
- Leader failover regression tests against a three-node TLS plus authenticated
  etcd cluster.

Current project state: deployment uses a loopback-only TLS verification and
authentication proxy to access etcd, so applications do not directly access an
unauthenticated etcd endpoint.

### 2. Generated Dependency Health Is Still Static

Severity: high.

Generated `ServiceContext::new` still calls
`register_static(healthy(...))` after Redis or NATS initialization succeeds,
then calls `mark_ready()`. When dependencies become unavailable at runtime, the
generated code itself does not keep `/readyz` updated.

Expected Roze behavior: generate timeout-bound dynamic dependency probes for
databases, Redis, NATS, etcd, and RPC dependencies, with liveness, readiness,
and startup represented as distinct states.

Current project state: `runtime-ops::readiness` continuously probes databases,
Redis, NATS, etcd, and all RPC dependencies concurrently, and each service's
`/readyz` returns the real-time result.

### 3. Official Persistent Idempotency And Outbox Adapters Are Missing

Severity: medium.

The generator still wires `InMemoryIdempotencyStore` and `InMemoryOutbox` by
default. Roze currently does not provide a production-ready Redis idempotency
store or SQL outbox implementation.

Expected Roze support:

- Redis atomic begin, complete, and fail operations; request fingerprint
  validation; execution leases; and response replay.
- PostgreSQL/MySQL outbox storage, claim leases, exponential backoff, maximum
  retries, dead-letter query, and dead-letter replay.
- Matching migrations, metrics, and integration tests.

Current project state: the management API uses Redis Lua scripts for persistent
idempotency. `runtime-ops::outbox::SqlOutboxStore` implements PostgreSQL/MySQL
persistence, failure retry, lease recovery, and dead-letter handling.

## Fixed Items Removed From This Tracking File

- PostgreSQL `BIGINT`, `BIGSERIAL`, and `INT8` generate `i64`.
- PostgreSQL `TIMESTAMPTZ` generates `DateTimeUtc` and includes the required
  dependencies.
- REST, RPC, SQL Model, OpenAPI, and TypeScript SDK generation can be refreshed
  with `rozectl 1.0.0`.

These items have been verified as fixed and are no longer maintained as active
upstream work. Complete generation and deployment results are carried by
project tests and release records.

## Verification Result

- `make generate` succeeded twice consecutively.
- `cargo check --workspace` passed.
- `cargo test --workspace -j 1` passed. Parallel Windows test builds failed
  earlier because of local memory pressure, so regression uses serial
  compilation on that machine.
- The project has been updated and locked to Roze revision
  `4e20455305fd295361ce0a22887527953cd54801`.

## Upgrade Regression Gate

After every Roze or `rozectl` upgrade:

1. Run `make generate` twice and confirm generation is deterministic and
   application-owned files are not overwritten.
2. Run `make artifacts`, `cargo check --workspace`, and
   `cargo test --workspace`.
3. Run frontend `npm run check`.
4. Verify PostgreSQL/MySQL migrations, real CRUD, Redis idempotency, and SQL
   outbox E2E.
5. Verify etcd watch, instance removal, lease expiry, re-registration,
   dual-instance load balancing, and leader failover.
6. Re-check the three issues above; remove an item from this file only after
   upstream implementation and integration tests are complete.
