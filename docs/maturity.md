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
| REST generation | beta | Generates split route/handler/logic/middleware layout and preserves user-owned files on `--update`; needs more generated-project compile tests and goctl edge fixtures. |
| RPC generation | beta | Generates split server/client/pb/logic layout; needs more proto compatibility and generated-project compile tests. |
| OpenAPI generation | beta | Produces OpenAPI 3 documents; validator constraint projection still has known gaps. |
| TS/JS/Dart SDK generation | beta | Covers route/path/query/header/body generation; needs richer error/interceptor/retry/timeout support. |
| HTTP middleware | beta | Covers trace, recover, metrics, CORS, timeout, max connections, shedding, gunzip, and body limit; needs more end-to-end service tests. |
| Gateway | beta | Supports static and registry upstreams, weighted canary routes, retries, fallback, health/outlier handling, JWT/API key auth, governance inheritance, metrics, and config hot reload; still needs production example scripts and broader deploy smoke tests. |
| Config center | beta | Supports Etcd watch, Env/File fallback, diff/version metadata, section-level change events, reload audit history, and failed-update rollback; still needs subscriber timeout/failure isolation hardening. |
| MQ/Kafka/NATS | beta | Publish/subscribe, retry, DLQ, stats, queue metrics, Kafka manual ack/nack/retry/dead-letter decisions, in-memory admin replay, NATS JetStream, and outbox/inbox primitives exist; still needs live broker integration coverage and production examples. |
| Service discovery | beta | memory, DNS, etcd, Consul, watch, and cache primitives exist; needs more failure-mode tests. |
| Health checks | beta | Probe report types exist; `/healthz`, `/readyz`, dependency details, and K8s templates need standardization. |
| Lifecycle/bootstrap | scaffold | Several helpers exist, but HTTP/RPC/consumer/job shutdown ordering is not yet one unified lifecycle. |
| DB/ORM/model generation | beta | Toasty is the default generated SQL model scaffold; SeaORM is optional with `--orm sea-orm`. Cross-schema ownership and production examples need more polish. |
| Transactions/outbox/DTM | scaffold | TCC/Saga/outbox/inbox primitives exist; needs full HTTP + DB transaction + outbox + MQ + RPC examples. |
| Auth/JWT/permission/session | beta | JWT, RBAC, tenant, and ABAC primitives exist; needs a unified security model, OpenAPI permission declarations, key rotation, and test templates. |
| Observability | beta | tracing, metrics, Prometheus, OpenTelemetry, gateway metrics, and queue event metrics exist; needs dashboards, scrape config, and query examples. |
| Docker/Kubernetes generation | scaffold | Generator commands exist; production checklist and manifest validation need to be added. |

## Production-Ready Criteria

A module can move to `stable` only when it has:

- Documented public API and generated-file ownership.
- Unit tests and at least one end-to-end test.
- Metrics, logs, and trace/context propagation documented.
- Failure behavior documented, including retry/rollback/fallback boundaries.
- Upgrade notes for breaking generated-code changes.
- A production example or smoke test that a user can run locally.
