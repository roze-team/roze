# Module Maturity Matrix

Roze contains many crates. This matrix prevents users from assuming every
module has the same production readiness.

Status legend:

- `stable`: API and behavior are suitable for production use with normal test
  coverage.
- `beta`: usable for pilots, but needs more integration tests, metrics, or
  upgrade hardening before broad production adoption.
- `scaffold`: generated/project-shaping code exists, but business behavior or
  operational semantics are intentionally left to applications.
- `planned`: design direction exists, implementation is incomplete.

| Area | Status | Notes |
| --- | --- | --- |
| REST generation | beta | Generates split route/handler/logic/middleware layout and preserves user-owned logic, service context extensions, custom middleware, and config on `--update`; generated REST + model + search compile smoke exists as an ignored `rozectl` test; needs more Roze `.api` edge fixtures. |
| RPC generation | beta | Generates split server/client/pb/logic layout. Method rate-limit and breaker state use DashMap for concurrent hot paths; memory registry has Criterion hot-path baselines; generated RPC compile smoke exists as an ignored `rozectl` test; needs more proto fixture coverage. |
| Stream worker generation | beta | `rozectl stream gen` creates producer, consumer, envelope, config, type, and README scaffolding from RPC methods. Generated stream workers run under `ServiceGroup`, propagate shutdown into consumer tasks, and have generated compile smoke as an ignored `rozectl` test; needs live broker examples and richer event-contract fixtures. |
| OpenAPI generation | beta | Produces OpenAPI 3 documents; validator constraint projection still has known gaps. |
| TS/JS/Dart SDK generation | beta | Covers route/path/query/header/body generation; needs richer error/interceptor/retry/timeout support. |
| HTTP middleware | beta | Covers trace, recover, metrics, CORS, timeout, max connections, shedding, gunzip, and body limit. Route rate-limit and breaker state use DashMap for concurrent hot paths; needs more end-to-end service tests. |
| Gateway | beta | Supports static and registry upstreams, weighted canary routes, retries, fallback, health/outlier handling, JWT/API key auth, governance inheritance, metrics, and config hot reload; still needs production example scripts and broader deploy smoke tests. |
| Config center | beta | Supports Etcd watch, Env/File fallback, diff/version metadata, section-level change events, listener failure isolation, and admin semantics for read, publish validation, audit, rollback, watch status, permissions, and JSON snapshot persistence; still needs deployment integration and long-run evidence before `stable`. |
| MQ/Kafka/NATS | beta | Publish/subscribe, retry, DLQ, stats, queue metrics, Kafka manual ack/nack/retry/dead-letter decisions, in-memory admin replay, NATS JetStream, and outbox/inbox primitives exist. In-memory topic, offset, and idempotency indexes use DashMap/DashSet and have Criterion baselines; still needs live broker integration coverage and production examples. |
| EventBus | beta | In-memory event envelope pub/sub exists. Topic sender lookup uses DashMap and has Criterion baselines for subscribe/publish hot paths. |
| Service discovery | beta | memory, DNS, etcd, Consul, watch, and cache primitives exist. The in-memory registry uses DashMap for concurrent register/discover paths; needs more failure-mode tests. |
| Health checks | beta | Probe report types and `HealthRegistry` exist. Generated REST services expose `/healthz`, `/readyz`, `/startupz`, and `/metrics`; generated service contexts register connected DB/Mongo/Redis/NATS dependencies as readiness checks. Gateway/RPC probes and protocol-level dependency pings still need standardization. |
| Lifecycle/bootstrap | scaffold | `ServiceGroup` primitives exist for HTTP/RPC/consumer/job/background services behind one shutdown signal, including starting/running/draining/stopped/failed phases, phase waiting, lifecycle snapshots, stop-hook/task shutdown timeouts, generated REST/RPC/stream lifecycle entrypoints, health draining on shutdown, a `roze-job::JobService` adapter, and a short/long production soak harness. End-to-end production examples are still required before beta or stable claims. |
| DB/ORM/model generation | beta | Toasty is the default generated SQL model scaffold; SeaORM is optional with `--orm sea-orm`. `model inspect` supports sqlite, postgres, mysql, and mongo; Mongo inspection samples documents, maps `_id`, captures index metadata, and emits single-field plus compound-index helpers. |
| Cache | beta | Redis cache helpers cover cache-aside, negative cache, TTL jitter, and singleflight loading. `roze-local-cache` uses Moka for in-process TTL/capacity eviction and hit/miss statistics, with Criterion baselines for async insert/get/get-or-insert. `roze-singleflight` uses DashMap for key lookup and has Criterion baselines for unique-key, cached-key, and reset paths. |
| Search generation | beta | `rozectl search generate/inspect` supports Elasticsearch, OpenSearch, and Meilisearch with generated `src/search` document/repository modules backed by `roze-search` health/index/delete/search calls. Elasticsearch/OpenSearch inspect reads mappings; Meilisearch inspect reads settings/index metadata and samples documents for field inference. Generated search modules are included in REST compile smoke. |
| Transactions/outbox/DTM | scaffold | TCC/Saga/outbox/inbox primitives exist; needs full HTTP + DB transaction + outbox + MQ + RPC examples. |
| Auth/JWT/permission/session | beta | JWT, RBAC, tenant, ABAC, and in-memory session primitives exist. Session lookup uses DashMap and has Criterion baselines for upsert/get paths; needs a unified security model, OpenAPI permission declarations, key rotation, and test templates. |
| WebSocket | beta | In-memory WebSocket hub primitives exist. Session lookup uses DashMap and has Criterion baselines for register/get/disconnect paths; needs broader gateway/app integration coverage. |
| Observability | beta | tracing, metrics, Prometheus, OpenTelemetry, gateway metrics, and queue event metrics exist. Labeled metric state uses DashMap for concurrent hot paths, with Criterion baselines for write/render paths; needs broader dashboards and query examples. |
| Docker/Kubernetes generation | beta | Generator commands emit production-oriented Dockerfiles, Kubernetes manifests, and Helm charts with offline validation. |
| Production smoke | beta | `scripts/production-smoke.sh` runs formatting checks, `rozectl` tests, generated REST/RPC compile smoke, core runtime tests, and app checks. `--with-compose` starts the real dependency profile for Etcd, Consul, Kafka, NATS, Redis, Postgres, MySQL, Elasticsearch, OpenSearch, and Meilisearch. |

## Production-Ready Criteria

A module can move to `stable` only when it has:

- Documented public API and generated-file ownership.
- Unit tests and at least one end-to-end test.
- Metrics, logs, and trace/context propagation documented.
- Failure behavior documented, including retry/rollback/fallback boundaries.
- Upgrade notes for breaking generated-code changes.
- A production example or smoke test that a user can run locally.
- Reproducible production evidence for runtime-critical modules, following
  [Production Evidence](production-evidence.md).

## Public Commitment Boundary

Public stability claims must follow [Stability Commitment](stability-commitment.md).
In short: beta modules can be used for pilots, but a module is not production
stable until this matrix says `stable` and the release policy evidence exists.
