# Roadmap

Roze's near-term priority is not to add more modules. The priority is to make
the existing framework pieces credible: tested, releasable, observable,
upgradeable, and recoverable.

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

- Extend the shared governance schema for timeout, retry, rate limit, breaker,
  shedding, and fallback across HTTP/RPC/Gateway/MQ where applicable; Gateway
  currently inherits timeout, retry, rate limit, and breaker from it.
- Add optional persistent state for breaker and rate limiter.
- Align metrics labels across HTTP route, RPC method, gateway route, and queue
  consumer.
- Framework lifecycle: SIGINT/SIGTERM, shutdown order, shutdown timeout,
  background task cancellation, readiness, and liveness.
- Standard `/healthz`, `/readyz`, `/metrics`, dependency details, and
  Kubernetes probe templates.

## P1: Generator and Contract Completeness

- More Roze `.api` parser edge fixtures: comments, imports, nested types, duplicate
  names, reserved words, mixed annotations, and compact syntax.

## P1: Admin API

- Control-plane models and adapters live in `roze-admin` for registry service
  instances, config reload history, and MQ DLQ snapshots/replay/purge.
- HTTP routes, OpenAPI, auth policy, and UI are still integration work.
- Golden tests for repeated generation and ownership preservation.
- Generated project compile tests for REST/RPC/model/client/docs.
- OpenAPI projection for validator constraints including `min`, `max`, `len`,
  `oneof`, map `additionalProperties`, nested struct validation, UUID, and
  custom validator boundaries.
- SDK error types, interceptors, retry/timeout config, auth injection, and
  regression tests.

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
