# Roze Production Generation Plan

This plan defines how Roze production generation can exceed go-zero in
repeatable engineering capability while learning from its strongest design
decisions. It does not claim that Roze already has go-zero's years of production
history. Stable claims remain blocked by the evidence gate in
[production-evidence.md](production-evidence.md).

Baseline reviewed: 2026-07-15.

Official references:

- [go-zero repository](https://github.com/zeromicro/go-zero)
- [go-zero architecture](https://go-zero.dev/concepts/architecture/)
- [go-zero design principles](https://go-zero.dev/concepts/design-principles/)
- [go-zero resilience components](https://go-zero.dev/components/)

## Product Boundary

Roze is a Rust-native, IDL-first production service generator. The product is
the generated service path plus the runtime contracts needed to operate it:

- REST, RPC, stream, model, search, OpenAPI, TypeScript, and JavaScript;
- lifecycle, governance, context, errors, security, telemetry, and reliable
  event delivery;
- Docker, Kubernetes, Helm, dashboards, alerts, runbooks, release gates, and
  production evidence assets.

Kotlin, Swift, Dart, Java, iOS, Android, and a cross-language SDK error codec
are intentionally out of scope. New languages do not outrank a reliable Rust
service path and Web SDK path.

Roze is pre-release. No compatibility shim is required for pre-release public
contracts; breaking changes must still include regeneration, migration, and
rollback instructions.

## Architecture To Adopt

The following go-zero principles are mandatory design inputs:

1. **One generated path.** Contract, handler, logic, service context, client,
   configuration, and operations assets follow one convention.
2. **Business ownership stays explicit.** Generated handlers adapt protocols;
   application-owned logic and extension files are never overwritten.
3. **Stability by default.** Timeout, cancellation, rate limiting, circuit
   breaking, adaptive shedding, tracing, and metrics are framework boundaries,
   not per-service reinventions.
4. **Downstream calls are governed.** Discovery, health, load balancing,
   deadline, retry budget, breaker, and telemetry act as one client pipeline.
5. **Low-cardinality observability is automatic.** Logs, metrics, and traces
   correlate without custom wiring in every service.
6. **One obvious operational workflow.** Generation, regeneration, validation,
   deployment, rollback, and diagnosis use predictable commands and files.

Roze extends this baseline with Rust ownership and type guarantees, one tool for
transport/data/search/operations generation, byte-deterministic regeneration,
an authoritative governance policy across REST/RPC/Gateway/MQ/Job, offline
deployment validation, and evidence-aware release promotion.

## Definition Of "Exceed"

Roze exceeds go-zero production generation only when all statements below are
supported by repository evidence:

| Dimension | Required result |
| --- | --- |
| Generated surface | REST, RPC, stream, model, search, OpenAPI, Web SDK, deployment, observability, reporting, and evidence assets are generated through one CLI. |
| Regeneration | Repeating create/update is byte-deterministic and preserves every documented application-owned file. |
| Default resilience | Generated inbound and downstream paths enforce shared timeout, cancellation, retry budget, rate limit, breaker, and adaptive shedding semantics. |
| Context | Deadline, cancellation, W3C trace context, tenant, subject, locale, idempotency key, and retry budget cross every generated boundary. |
| Data correctness | Tenant scope, transactions, optimistic concurrency, cache invalidation, outbox/inbox, pagination, and migration boundaries have generated contracts and failure tests. |
| Operations | Every generated service has immutable deployment assets, probes, SLO dashboard, alerts, trace/log queries, capacity policy, backup/restore, and rollback instructions. |
| Verification | The release gate compiles and smoke-tests every supported generated target and rejects unsafe contract or migration changes. |
| Evidence | Runtime-critical areas have reproducible 24h and 72h reports with latency, throughput, resource trends, fault recovery, and leak results. |

Feature count alone cannot satisfy this definition. A beta/scaffold capability
without failure semantics and evidence does not outrank a mature capability.

## Completion Rules

An implementation item is complete only when all applicable evidence exists:

1. runtime or generator implementation;
2. focused unit, cancellation, and failure-path tests;
3. generated-project compile and end-to-end smoke tests;
4. public contract, ownership, and failure-semantics documentation;
5. bounded-cardinality metrics, structured logs, and trace fields;
6. regeneration/migration and rollback notes for breaking output changes;
7. long-run evidence when the maturity matrix requires it.

`Implemented` means repository work and automated verification exist.
`Evidence pending` means a real 24h/72h run is still required. A shortened or
synthetic run can validate the harness but cannot become production evidence.

## Authoritative Workstream Plan

The following workstreams are the authoritative implementation order. Later
sections describe the same work by architecture area; they do not change this
order. A workstream advances only when its completion gate is automated in the
repository.

| ID | Workstream | Depends on | State | Completion gate |
| --- | --- | --- | --- | --- |
| W01 | API/OpenAPI/Search breaking contract diff | M2 generation matrix | Implemented | The CLI classifies additive, behavioral, and breaking changes; breaking changes fail the release gate with stable path-level diagnostics. |
| W02 | SQL schema and migration risk detection | W01 diff model | Implemented | Drop, rename, narrowing, nullability, constraint, index, lock, and data-rewrite risks are classified for every supported database. |
| W03 | Explicit migration/rollback acknowledgment gate | W02 | Implemented | Every destructive change requires a generated, reviewable acknowledgment containing migration, rollback, owner, reason, and expiry; missing or stale records fail the gate. |
| W04 | Diff-gate tests and CLI diagnostics | W01-W03 | Implemented | Fixtures cover accepted and blocked changes, exit codes are stable, diagnostics identify source paths, and the Linux release path invokes the gate. |
| W05 | Unified service governance model | W04 | Implemented | HTTP, RPC, Gateway, MQ, and Job resolve one policy for deadline, cancellation, retry budget, rate limit, breaker, shedding, and bounded metric labels. |
| W06 | Lifecycle and graceful shutdown | W05 | Implemented | Generated services enforce startup/readiness/drain/shutdown ordering, bounded hooks, cancellation propagation, failed-task reporting, and dependency-aware draining. |
| W07 | Reliable MQ event lifecycle | W05-W06 | Implemented | Versioned envelope, idempotent inbox, transactional outbox, bounded retry, DLQ query/replay/purge, lag telemetry, and restart/duplicate tests preserve business invariants. |
| W08 | Report export and chart query | W04-W07 | Implemented; integration evidence pending | Typed bounded chart queries and asynchronous CSV/XLSX exports include authorization, tenant isolation, cancellation, expiry, object storage, audit, metrics, OpenAPI, and Web SDK projection. |
| W09 | Gateway and Config Center production governance | W05-W07 | In progress; dependency evidence pending | Canary/blue-green/mirror traffic, stream protocols, signed configuration, staged rollout, audit, listener isolation, snapshots, rollback, and dependency-backed smoke tests pass. |
| W10 | Security closure | W05-W09 | In progress; cross-boundary evidence pending | OIDC/OAuth2, mTLS, JWT rotation, revocation, redaction, least privilege, audit projection, and cross-tenant isolation tests cover every generated boundary. |
| W11 | Complete production examples and operations assets | W06-W10 | In progress | Three generated reference systems ship deployable manifests, dashboards, alerts, trace/log queries, SLOs, runbooks, backup/restore, migration, and rollback drills. |
| W12 | 24h/72h soak and fault-injection evidence | W11 | Evidence pending | Reproducible signed reports prove latency, throughput, bounded resources, retry amplification, leak safety, and recovery objectives before stable promotion. |

Execution rules:

1. Finish W01-W04 before treating generated contract changes as release-safe.
2. W05-W10 may reuse existing implementations, but remain incomplete until
   their cross-boundary failure tests and generated defaults pass.
3. W11 is the shared integration surface for all runtime capabilities.
4. W12 records real elapsed-time evidence; shortened smoke runs cannot satisfy
   its 24h/72h completion gate.
5. Do not introduce compatibility shims or restore non-Web SDK targets.

### P0. Release Gate For Every Generated Target

Status: **implemented**. The unified `scripts/generated-target-matrix.sh`
entrypoint now runs a non-ignored deterministic structural matrix for every
supported target plus REST/model/search, RPC, stream, and generated HTTP/
multi-service smoke compile checks. `rozectl gate check` and the release gate
block unsafe contract and migration changes unless a hash-bound acknowledgment
is valid.

- Compile generated REST, RPC, stream, HTTP/multi-service smoke, model, search,
  OpenAPI, TypeScript, and JavaScript outputs in the release gate.
- Run create, update, and second-update determinism checks.
- Verify generated ownership manifests and reject overwritten application files.
- Run contract diff and migration diff; block destructive changes unless the
  generated migration/rollback record explicitly acknowledges them.
- Exercise both Windows-native validation and Linux CI packaging paths.

Acceptance:

- one command verifies every supported target;
- no supported generator is represented only by an ignored test;
- the same input produces the same generated bytes;
- an unsafe API, database, or search contract change fails the gate.

### P0. End-To-End Context And Governance

Status: **in progress**; shared policy resolution is implemented.

- Propagate deadline, cancellation, W3C trace context, tenant, subject, locale,
  idempotency key, and retry budget through REST -> RPC -> DB/cache/MQ paths.
- Make cancellation release breaker probes, shedding permits, stream capacity,
  and background tasks on every exit path.
- Standardize `service`, `boundary`, `operation`, `kind`, `decision`, and
  `outcome` labels; operation labels must come from generated bounded names.
- Add P2C/EWMA health-aware balancing benchmarks and failure tests for managed
  RPC clients, borrowing go-zero's client-pipeline discipline.
- Add optional persisted limiter/breaker snapshots with local fail-safe state;
  request paths must not depend on control-plane availability.

Acceptance:

- one cross-transport test proves context propagation and cancellation;
- retry amplification is bounded by a propagated budget;
- metric cardinality remains bounded under arbitrary paths, IDs, tenants, and
  error messages;
- control-plane loss does not stop the data path.

### P0. Generated Production Systems

Status: **in progress**. Generated compile/smoke coverage and the dedicated
`generated-systems` soak runner exist; real dependency-backed failure workflows
and the three complete reference-system evidence sets remain open.

Generate and continuously execute three reference systems:

1. REST CRUD + SQL migration + cache consistency;
2. REST + RPC + DB + Redis + tracing + governed client;
3. Gateway + Registry + MQ + Outbox/Inbox + Saga/TCC + object storage.

Each system must test startup, readiness, dependency loss, timeout, duplicate
event, retry exhaustion, DLQ replay, config rollback, graceful drain, migration
rollback, and regenerated update.

Acceptance:

- projects compile from freshly generated sources;
- representative success and failure workflows execute against real
  dependencies in CI;
- restart/replay does not duplicate committed business effects;
- generated runbooks explain every injected failure from metrics/logs/traces.

### P1. Data, Event, And Transaction Reliability

Status: **in progress**.

- Standardize event envelope fields: event ID, type, version, occurred time,
  trace context, tenant, idempotency key, producer, and schema revision.
- Finish at-least-once consumer semantics, delayed retry, storm protection,
  poison-message classification, DLQ query/replay/purge, and consumer lag.
- Generate idempotent inbox and transactional outbox wiring with relay health.
- Finish Saga/TCC state-machine persistence, timeout recovery, compensation
  idempotency, operator query, and replay safety.
- Generate migration policy, online migration checks, backup/restore, and
  rollback assets for supported databases and search indexes.

Acceptance:

- broker restart and duplicate delivery tests preserve business invariants;
- transaction commit and event publication cannot silently diverge;
- compensation can be retried safely after process restart;
- destructive schema changes are blocked before deployment.

### P1. Report Export And Chart Query Contracts

Status: **implemented; integration evidence pending**.

- Add IDL/schema declarations for report dimensions, measures, filters,
  grouping, sorting, time buckets, pagination, and maximum result cost.
- Generate typed chart-query endpoints returning series, categories, units,
  timezone, aggregation metadata, and trace ID.
- Generate asynchronous export jobs for CSV and XLSX with progress, cancel,
  expiry, object-storage delivery, audit record, and tenant isolation.
- Reuse generated model/query builders; prohibit raw client-supplied SQL,
  unbounded scans, arbitrary metric labels, and synchronous large exports.
- Project report/chart contracts into OpenAPI and TypeScript/JavaScript clients.
- Generate query latency, scanned-row, result-row, export-size, queue-delay,
  failure, and cancellation metrics plus dashboards and alerts.

Acceptance:

- generated report/chart examples compile and run against a real database;
- query complexity, row count, duration, and export size are bounded;
- cancelled/expired exports release resources and remove temporary objects;
- tenant-isolation, formula-injection, CSV/XLSX escaping, and authorization
  tests pass.

### P1. Security By Generated Contract

Status: **in progress**.

- Project auth, permission, tenant, and audit declarations into OpenAPI,
  generated route/RPC adapters, Web SDKs, tests, and operation runbooks.
- Add OIDC discovery with bounded cache/stale policy, OAuth2 flows, JWT signing
  key rotation, clock-skew policy, revocation semantics, and mTLS identity.
- Generate least-privilege Kubernetes identities, secret references, audit
  events, and sensitive-field redaction defaults.
- Add cross-tenant isolation tests for REST, RPC, cache, search, MQ, reports,
  exports, and object storage.

Acceptance:

- missing, expired, rotated, wrong-issuer, wrong-audience, and revoked
  credentials have stable typed failures;
- identity-provider outage behavior is documented and tested;
- no generated log, metric, trace, OpenAPI example, or manifest leaks secrets.

### P1. Operations And Recovery Assets

Status: **in progress**; core Kubernetes/Helm hardening is implemented.

- Generate Grafana dashboards and Prometheus alerts for golden signals,
  retries, breakers, shedding, pools, cache, MQ, outbox, config, reports, and
  lifecycle.
- Generate trace queries, log queries, SLO/error-budget policy, capacity model,
  dependency matrix, incident runbooks, and rollback drills.
- Validate immutable image references, probes, resources, HPA, PDB,
  NetworkPolicy, ServiceAccount, secrets, config rollout checksums, and topology
  spread in offline render tests.
- Generate signed SBOM/provenance and dependency/license/vulnerability gates.

Acceptance:

- a new generated service can be deployed and diagnosed without handwritten
  baseline operations files;
- rollback and backup/restore drills are executable, not prose-only;
- alerts link directly to relevant dashboards, traces, logs, and runbooks.

### P2. Performance And Production Evidence

Status: **evidence pending**.

- Maintain Criterion baselines for router, middleware, context, metrics,
  registry, balancing, cache, MQ, model/query, and report aggregation hot paths.
- Add generated-service load scenarios with fixed hardware/toolchain metadata.
- Run required 24h reports before beta-to-stable promotion and 72h reports
  before broad stable claims for Gateway, MQ, Config Center, Lifecycle, and
  generated systems.
- Record p50/p95/p99, throughput, CPU, memory, descriptors/connections, restart
  count, task leaks, retry amplification, and recovery objectives.
- Publish signed artifacts, crates, changelog, migration guide, and rollback
  notes only after release-gate and evidence-policy checks pass.

Acceptance:

- no unbounded memory, task, state-map, queue, or cardinality growth;
- injected failures recover within declared objectives without unexplained
  manual intervention;
- performance regressions over the approved threshold block release;
- maturity entries move to `stable` only with linked evidence reports.

## Current Milestone Board

| Milestone | Exit condition | State |
| --- | --- | --- |
| M0 deterministic contracts | API/proto edge fixtures, OpenAPI constraints, typed Web SDK, byte-identical update | Implemented |
| M1 unified governance | one resolved policy for REST/RPC/Gateway/MQ/Job with shared precedence | Implemented |
| M2 complete release matrix | every supported generated target compiled and smoked by release gate | Implemented |
| M3 context and observability | end-to-end propagation, cancellation safety, bounded labels | In progress |
| M4 reference systems | three real dependency-backed generated systems and failure workflows | Pending |
| M5 reporting | typed chart queries and asynchronous governed CSV/XLSX exports | Implemented; integration evidence pending |
| M6 security and recovery | identity rotation/isolation plus executable operations drills | In progress |
| M7 production evidence | passing 24h/72h reports and signed release | Evidence pending |

## Immediate Work Order

1. W01: API/OpenAPI/Search breaking contract diff.
2. W02: SQL schema and migration risk detection.
3. W03: explicit migration/rollback acknowledgment gate.
4. W04: diff-gate tests and CLI diagnostics.
5. W05: unified service governance model.
6. W06: lifecycle and graceful shutdown.
7. W07: reliable MQ event lifecycle.
8. W08: report export and chart query.
9. W09: Gateway and Config Center production governance.
10. W10: security closure.
11. W11: complete production examples and operations assets.
12. W12: 24h/72h soak and fault-injection evidence.

Do not add another crate, generator language, or isolated feature unless it
directly closes one of these milestone exits.

## External Dependencies

GitHub metadata, signing keys, crates.io ownership, public release publication,
and long-running infrastructure require maintainer credentials or external
systems. Roze may generate and validate their workflows, but completion is
recorded only after those actions and evidence runs actually succeed.
