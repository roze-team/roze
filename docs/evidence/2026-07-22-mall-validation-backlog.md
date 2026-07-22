# Roze Upstream Issue Tracking - 2026-07-22

This file tracks only issues that are still reproducible in the current Roze
revision and require upstream work. Generator compatibility issues already
verified as fixed are not kept in the active backlog.

## Verification Environment

- Verification date: 2026-07-22
- Roze base revision: `e38933ff39704a093a3daa554ce409d6b7b6629c`
- CLI: `rozectl 1.0.0`
- CLI path: `C:\Users\xFc\.cargo\bin\rozectl.exe`

## Resolved In The Current Worktree

### 1. Registry Uses Etcd TLS And Authentication Configuration

`RegistryConfig` now carries user/password, CA, client certificate/key, and
certificate-verification settings. `EtcdRegistry` builds one shared TLS client;
register, discover, watch, keepalive, deregistration, and re-registration all
use its cached authentication token. A 401/403 clears the token, authenticates
again, and retries the original request once. Deterministic tests cover token
refresh and authenticated endpoint failover. The real three-node TLS cluster
gate remains an external integration/evidence run, not a missing runtime path.

### 2. Generated Dependency Health Is Dynamic

Generated service contexts register timeout-bound probes for SQL databases,
MongoDB, Redis, NATS, the registry, and managed RPC clients. RPC probes follow
dynamic discovery instead of pinning the initial channel. `/healthz` remains
process liveness, `/readyz` runs dependency checks concurrently, and
`/startupz` represents startup/draining phase. Generated REST and RPC compile
smokes pass with the new surface.

## Remaining Reproducible Issues

### 1. Official SQL Outbox Adapters Are Missing

Severity: medium. Roze now provides `RedisIdempotencyStore`, using Lua for
atomic begin/complete/fail, fingerprint validation, execution leases, response
replay, and bounded retention. The generator intentionally retains in-memory
defaults so deployments must explicitly inject persistent infrastructure.

Roze still lacks official PostgreSQL/MySQL Outbox stores with transactional
enqueue, claim leases, maximum retry/dead-letter transitions, dead-letter
query/replay, migrations, metrics, and real integration tests. The project-side
`runtime-ops::outbox::SqlOutboxStore` remains necessary until that complete
adapter surface is upstream.

## Fixed Items Removed From This Tracking File

- PostgreSQL `BIGINT`, `BIGSERIAL`, and `INT8` generate `i64`.
- PostgreSQL `TIMESTAMPTZ` generates `DateTimeUtc` and includes the required
  dependencies.
- REST, RPC, SQL Model, OpenAPI, and TypeScript SDK generation can be refreshed
  with `rozectl 1.0.0`.

These items have been verified as fixed and are no longer maintained as active
upstream work. Complete generation and deployment results are carried by
project tests and release records.

## Base Revision Verification Result

- `make generate` succeeded twice consecutively.
- `cargo check --workspace` passed.
- `cargo test --workspace -j 1` passed. Parallel Windows test builds failed
  earlier because of local memory pressure, so regression uses serial
  compilation on that machine.
- The current changes are based on Roze revision
  `e38933ff39704a093a3daa554ce409d6b7b6629c`.

## Current Worktree Validation

- Registry TLS/auth configuration, token refresh, authenticated endpoint
  failover, dynamic health crates, and Redis idempotency unit tests passed.
- The complete non-database `rozectl` suite passed: 237 tests passed and 10
  external/compile tests remained ignored.
- Generated REST and RPC projects both passed their ignored compile-and-clippy
  smoke tests.
- Targeted `cargo clippy --all-targets -- -D warnings` passed for every changed
  runtime crate and the gateway.
- A native Windows `cargo check --workspace` remains environment-blocked in
  `rdkafka-sys`: its vendored build invokes Unix `cp` and then tries to execute
  `configure` as a Win32 binary. This is unrelated to the changed crates; the
  targeted checks above pass.

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
6. Re-check the remaining issues above; remove an item from this file only after
   upstream implementation and integration tests are complete.
