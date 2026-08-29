# Requirements And Architecture Comparison

This document records the Roze 1.0 architecture baseline. Contract stability
and runtime evidence are intentionally separate: all listed public surfaces are
stable in 1.x, while long-run verification remains visible in
[Module Maturity](maturity.md).

## Product Boundary

Roze is a Rust-native, IDL-first framework and generator for:

- REST, RPC, stream workers, models, migrations, and search;
- OpenAPI and TypeScript/JavaScript Web clients;
- governance, lifecycle, health, security, metrics, tracing, and configuration;
- Gateway, registry, MQ, inbox/outbox, cache, and storage, with TCC/Saga
  coordination supplied by the independent
  [`roze-dtm`](https://github.com/roze-team/roze-dtm) project;
- reports, chart queries, deployment assets, release gates, and evidence jobs.

Non-Web language SDKs are outside the product boundary.

## Stable Generation Architecture

`rozectl` owns transport and operations glue. Application code owns business
logic, domain validation, authorization decisions, complex queries, and
workflow policy. Regeneration preserves documented application-owned files and
refreshes generator-owned files deterministically.

The contract gate normalizes API, OpenAPI, search, and SQL schemas. It
classifies additive, behavioral, online-risk, destructive, and breaking
changes, and requires a hash-bound, expiring migration/rollback acknowledgment
for blocked changes.

## Runtime Architecture

One governance policy resolves timeout, deadline, cancellation, retry budget,
rate limit, circuit breaker, adaptive shedding, and fallback behavior for HTTP,
RPC, Gateway, MQ, and jobs. Generated operation keys keep metrics bounded.

`ServiceGroup` provides Starting, Ready, Draining, Stopped, and Failed states,
dependency-ordered startup, reverse draining, bounded hooks, and failed-task
reporting. REST, RPC, consumers, outbox relays, and background tasks use the
same shutdown boundary.

Reliable events use one versioned envelope across Kafka, NATS, in-memory MQ,
inbox, outbox, and generated stream workers. The envelope carries identity,
schema, trace, tenant, idempotency, producer, attempt, occurrence time, and
payload metadata.

## Data And Transaction Architecture

Model generation supports Toasty and SeaORM, SQL and Mongo inspection,
migrations, tenant scope, optimistic concurrency, cache consistency, and
search repositories. Transactions and outbox are stable production APIs:

- local transaction and persistent outbox boundaries;
- inbox idempotency and relay recovery;
- migration risk detection, rollback, and release acknowledgment gates.

Distributed TCC/Saga state, barriers, retry, and compensation are maintained
in the independent [`roze-dtm`](https://github.com/roze-team/roze-dtm)
project.

Applications still define their own domain transaction boundaries and
compensation behavior.

## Gateway, Configuration, And Security

Gateway supports registry and static upstreams, canary/blue-green/A-B routing,
traffic mirroring, active health, passive outlier ejection, retries, fallback,
SSE, WebSocket, TLS/mTLS, auth, and atomic reload.

Config Center supports signed configuration, staged rollout, promotion,
rejection, audit, permissions, listener timeout isolation, snapshots, restore,
and rollback.

Security contracts cover OIDC discovery, OAuth2 policy, mTLS identity, JWT key
rotation, issuer/audience/clock-skew checks, revocation, RBAC, ABAC, tenant
scope, sessions, and audit projection.

## Reporting And Operations

Generated reporting uses asynchronous CSV/XLSX export resources and bounded
structured chart queries. It includes authorization and tenant binding,
cancellation, expiry, object storage, formula-injection protection, metrics,
audit, OpenAPI, and Web clients.

Generated operations assets include immutable deployment manifests, Helm,
probes, dashboards, alerts, SLOs, trace/log queries, runbooks, backup/restore,
migration, rollback, release gates, and soak workflows.

## Remaining Evidence

The stable 1.x contract does not fabricate operational history. Signed 24h/72h
evidence for Gateway, MQ, Config Center, Lifecycle, and generated systems is
still `long-run pending`. Until those artifacts pass, Roze may be described as
API-stable and release-gated, but not as battle-tested in production.

See [Production Generation Plan](go-zero-surpass-plan.md) and
[Production Evidence](production-evidence.md) for the remaining evidence work.
