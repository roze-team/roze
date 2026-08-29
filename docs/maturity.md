# Module Maturity Matrix

Roze 1.0 separates public-contract stability from operational evidence. Areas
below have a stable 1.x API or generated contract unless explicitly marked
experimental. The evidence column says what has actually been exercised; it
must not be upgraded by declaration.

Status legend:

- `stable`: covered by Semantic Versioning, upgrade notes, and the contract gate.
- `experimental`: available for adoption, but its API or generated contract may
  change before promotion to stable.
- `verified`: focused tests plus generated compile/smoke coverage exist.
- `integration`: real dependency integration coverage exists.
- `long-run pending`: the 24h/72h signed evidence required for a
  battle-tested claim has not yet been archived.

| Area | Contract | Evidence | Notes |
| --- | --- | --- | --- |
| REST generation | stable | verified | Deterministic create/update generation preserves application-owned logic, context extensions, middleware, and configuration. |
| RPC generation | stable | verified | Split server/client/protobuf generation covers unary and streaming contracts, governance, health, and ownership-preserving updates. |
| Stream worker generation | stable | verified | Producer, consumer, envelope, lifecycle, shutdown, and generated compile coverage use the shared event contract. |
| OpenAPI generation | stable | verified | Runtime constraints, extensions, auth declarations, reports, and chart contracts are projected into OpenAPI 3. |
| TypeScript/JavaScript SDK generation | stable | verified | Web clients include typed errors, auth injection, interceptors, cancellation, timeout, and bounded retry. Non-Web SDKs are out of scope. |
| AI runtime and module generation | experimental | verified | Provider-neutral model/tool/agent contracts, compiled and parallel-layer DAG workflows, deterministic workflow event streaming, bounded backpressure-aware linear node chunk streams, checkpoint/interrupt/resume with a `roze-storage` adapter, bounded task teams and permission-aware model delegation, standard RAG components, `roze-search` adapters, OpenAI-compatible invoke/SSE/tool calls, transactional generation, and compile smoke are implemented. Automatic branch/join stream merge semantics and CAS/lease-safe concurrent resume remain application-explicit. |
| HTTP middleware | stable | verified | Trace, recovery, metrics, CORS, timeout, connection/body limits, rate limit, breaker, retry budget, and shedding share bounded operation labels. |
| Gateway | stable | long-run pending | Registry routing, canary/blue-green/A-B selection, mirroring, health/outlier handling, retry, fallback, SSE, WebSocket, TLS, auth, and hot reload are implemented. |
| Config center | stable | long-run pending | Signed publish, staged rollout, audit, permissions, listener isolation, snapshot restore, promotion, rejection, and rollback are implemented. |
| MQ/Kafka/NATS | stable | long-run pending | Shared event envelope, governed consumers, retry/DLQ, replay/purge, lag, inbox/outbox, idempotency, and transport metadata are implemented. |
| EventBus | stable | verified | Versioned envelopes and concurrent in-memory publish/subscribe have focused tests and hot-path benchmarks. |
| Service discovery | stable | integration | Memory, DNS, Etcd, and Consul discovery include watch/cache and failure behavior. |
| Health checks | stable | verified | REST probes, gRPC health, dependency readiness, startup, and draining behavior are generated consistently. |
| Lifecycle/bootstrap | stable | long-run pending | Starting/Ready/Draining/Stopped/Failed ordering, bounded hooks, failed-task reporting, and reverse dependency drain are implemented. |
| DB/ORM/model generation | stable | integration | Toasty and SeaORM generation, SQL/Mongo inspection, migration boundaries, and model smoke coverage are supported. |
| Cache | stable | integration | Redis cache-aside, negative cache, TTL jitter, local cache, and singleflight behavior are covered. |
| Search generation | stable | integration | Elasticsearch, OpenSearch, and Meilisearch generation/inspection share stable document and repository contracts. |
| Transactions/outbox | stable | integration | Local transactions, migration, inbox, transactional outbox, relay, recovery, and idempotency contracts are production APIs; complete reference-system long-run evidence remains pending. Distributed TCC/Saga coordination is maintained in the independent [`roze-dtm`](https://github.com/roze-team/roze-dtm) project. |
| Auth/JWT/permission/session | stable | verified | OIDC/OAuth2, mTLS identity, JWT rotation/revocation, RBAC/ABAC, tenant scope, session, and audit contracts are implemented. |
| Reporting/charts | stable | verified | Bounded chart queries and asynchronous CSV/XLSX exports include tenant/auth binding, cancellation, expiry, object storage, audit, and Web projection. |
| WebSocket | stable | verified | Native REST upgrades/frames, generated `@websocket` routes, session hub, and Gateway tunneling have stable lifecycle, bounds, shutdown, and TLS boundaries. |
| Observability | stable | verified | Structured tracing, bounded metrics, Prometheus/OpenTelemetry export, dashboards, alerts, and generated queries are supported. |
| Docker/Kubernetes generation | stable | verified | Immutable deployment, probes, resources, HPA, PDB, NetworkPolicy, service identity, Helm, and offline validation assets are generated. |
| Production smoke | stable | long-run pending | Release gates run the generated target matrix and runtime smoke; signed generated-system 24h/72h artifacts remain pending. |

## Stable Contract Requirements

The 1.x contract includes Rust crate APIs, CLI commands, generated layouts,
configuration schemas, metrics/events, and documented runtime ordering.
Breaking changes require a new major version, migration and rollback notes, and
the hash-bound contract/migration gate. Roze 1.0 does not include compatibility
shims for the former 0.x surface.

## Evidence Boundary

`stable` means users can build against the public contract with normal SemVer
expectations. It does not mean every workload, dependency topology, or failure
mode is battle-tested. Claims about long-run behavior require linked passing
artifacts defined by [Production Evidence](production-evidence.md).
