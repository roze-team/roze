# Roadmap

Roze's near-term priority is not to add more modules. The priority is to make
the existing framework pieces credible: tested, releasable, observable,
upgradeable, and recoverable.

The executable cross-area plan, acceptance rules, go-zero design baseline, and
evidence boundary are tracked in
[Roze Production Generation Plan](go-zero-surpass-plan.md). This roadmap keeps
the concise backlog; the execution plan is the source of truth for completion.

## P0: Maturity and Trust

### Release System

- Add GitHub Releases and signed tags.
- Publish crates and `rozectl` to crates.io.
- Keep `CHANGELOG.md` current.
- Define SemVer and MSRV policy.
- Add upgrade guides and breaking-change notes for generated code changes.

### Project Metadata

- Fill GitHub description, topics, website/docs link, and repository About
  fields.
- Add architecture diagram, public roadmap, contributing guide, code of
  conduct, security policy, and issue/PR templates.

### Maturity Marking

- Keep [Module Maturity Matrix](./maturity.md) current.
- Mark each crate as `stable`, `beta`, `scaffold`, or `planned`.
- Do not present scaffold modules as production-stable until they have tests,
  docs, failure semantics, and examples.

## P0: Gateway, Config Center, MQ

### Gateway v2

- Registry-backed dynamic upstreams with instance weights and metadata tags.
- Route-level retries and backoff.
- Weighted gray routes for blue/green and canary traffic.
- Retry metrics and latency/error counters.
- Unified gateway error mapping.
- Clear proxy passthrough vs fallback response boundaries.
- Smoke tests for rewrite, timeout, auth, rate limit, breaker, retry, fallback,
  and hot reload.

### Config Center and Hot Reload

- `ConfigCenterChangeEvent` with section-level changes.
- Config diff, version number, and change log.
- Optional signature/checksum for critical sections.
- Failed update rollback to last valid config.
- Gray rollout support.
- Subscriber timeout and failure isolation.
- Hot reload end-to-end tests.

### MQ/Kafka Semantics

- Standard message metadata: attempt, dead letter topic, timestamp, partition,
  offset, and context carrier.
- Producer result semantics.
- Manual commit, ack, nack, retry, and dead-letter behavior.
- Delayed retry policy and retry-storm protection.
- topic/group/partition/offset metrics.

## P1: Unified Governance

- Keep `roze-config` as the only global/scoped policy resolver for
  REST/RPC/Gateway/MQ/Job; Gateway explicit route fields remain highest
  priority.
- Complete deadline, cancellation, trace, tenant, idempotency-key, and retry
  budget propagation across every generated downstream call.
- Add optional persistent state for breaker and rate limiter.
- Align bounded metric labels across HTTP route, RPC method, gateway route,
  queue consumer, and job.
- Framework lifecycle: SIGINT/SIGTERM, shutdown order, shutdown timeout,
  background task cancellation, readiness, and liveness.
- Standard `/healthz`, `/readyz`, `/metrics`, dependency details, and
  Kubernetes probe templates.

## P1: Generator and Contract Completeness

- Put REST, RPC, stream, model, search, OpenAPI, TypeScript, and JavaScript
  generation into one non-ignored release-gate matrix. The unified structural
  and compile entrypoint is implemented; unsafe contract/migration diff gates
  remain.
- Keep deterministic second-update and generated ownership checks mandatory.
- Block unsafe API, migration, and search contract changes through generated
  diff gates and rollback records.

## P1: Admin API

- Control-plane models and adapters live in `roze-admin` for registry service
  instances, config reload history, and MQ DLQ snapshots/replay/purge.
- HTTP routes, OpenAPI, auth policy, and UI are still integration work.
- Golden tests for repeated generation and ownership preservation.
- Generated project compile tests for REST/RPC/model/client/docs.
- OpenAPI projection for validator constraints including `min`, `max`, `len`,
  `oneof`, map `additionalProperties`, nested struct validation, UUID, and
  custom validator boundaries.
- Keep TypeScript/JavaScript SDK typed errors, interceptors, bounded retries,
  timeout/cancellation, auth injection, and regression tests release-gated.

## P1: Reporting and Charts

- Generate bounded chart-query contracts for dimensions, measures, filters,
  grouping, sorting, time buckets, pagination, and query cost.
- Generate asynchronous CSV/XLSX export jobs with progress, cancel, expiry,
  object-storage delivery, audit records, and tenant isolation.
- Project report/chart contracts into OpenAPI and TypeScript/JavaScript clients.
- Generate report/export metrics, dashboards, alerts, and failure runbooks.

## P1: Data Boundary

- Keep Toasty as the default generated SQL model scaffold unless the CLI flag
  requests SeaORM.
- Document transaction boundaries, domain validation, authorization checks, and
  reliable event publishing as application-owned code.
- Provide full examples for DB transaction + outbox + MQ publish + RPC call +
  TCC compensation.

## P2: Production Documentation and Examples

- End-to-end examples:
  - REST CRUD monolith.
  - REST + RPC + DB + Redis.
  - Gateway + Registry + MQ + Outbox + DTM.
- Production checklist for config, secrets, JWT rotation, migrations,
  connection pools, timeout/retry defaults, Prometheus, tracing, probes, HPA,
  PDB, logs, error codes, gray release, and rollback.
- Grafana dashboards, Prometheus scrape config, trace examples, and log query
  examples.
- Unified security model: JWT key rotation, claims, RBAC/ABAC, tenant isolation,
  i18n error codes, and permission test templates.
- Real 24h/72h evidence for Gateway, MQ, Config Center, Lifecycle, and generated
  reference systems before stable promotion.
