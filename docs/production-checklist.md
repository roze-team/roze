# Production Checklist

This checklist is the baseline for considering a Roze service production-ready.
Roze itself is currently pre-release, so passing this checklist means a
specific service has a controlled production path. It does not mean every Roze
crate, generator, or runtime module is broadly production-stable.

## Release and Upgrade

- Service is built from a tagged Roze release or a pinned Git revision.
- `CHANGELOG.md` and upgrade notes have been reviewed.
- `bash scripts/production-smoke.sh` passes.
- `bash scripts/rozectl-smoke.sh` passes.
- `bash scripts/roze-project-external-smoke.sh` passes when validating the full
  external dependency profile locally.
- Generated REST and RPC compile smoke tests pass:
  - `cargo test -p rozectl generated_rest_project_compiles_with_model_and_search -- --ignored`
  - `cargo test -p rozectl generated_rpc_project_compiles -- --ignored`
  - `cargo test -p rozectl generated_stream_project_compiles -- --ignored`
- Generated-code changes were applied with `--update` and reviewed as a diff.
- User-owned files under `src/logic/**`, `src/svc/mod.rs`, custom
  middleware, and `config.yaml` were not overwritten.
- Rollback command and previous binary/image are available.
- Runtime-critical modules marked `stable` have a production evidence report
  that follows [Production Evidence](production-evidence.md).
- Public production-readiness wording follows
  [Stability Commitment](stability-commitment.md).

## Configuration and Secrets

- Runtime config is loaded from a controlled source.
- Config changes have a version, diff, author/source, and rollback path.
- Config Center management semantics, audit history, watch status, rollback,
  permission checks, and snapshot backup/restore or external control-plane
  integration are verified before treating it as stable.
- Secret values are not stored in generated config files.
- JWT keys and external credentials have rotation procedures.
- Config hot reload failure keeps the last valid config.

## Database and Transactions

- Migrations are versioned and reversible where practical.
- Connection pool sizes and timeouts are explicit.
- Transaction boundaries live in application logic, not generated handlers.
- Outbox is used for reliable event publishing when DB state and messages must
  be consistent.
- Idempotency is defined for retries and message consumption.

## HTTP/RPC/Gateway Governance

- Timeout defaults are set.
- Retry policy and retry budget are set where retries are enabled.
- Rate limit, breaker, shedding, and fallback behavior are documented.
- Gateway routes have explicit upstreams, rewrite rules, auth expectations, and
  fallback boundaries.
- Error codes are stable and documented.

## MQ and Background Work

- Consumer group, topic, partition, offset, and retry labels are observable.
- ack, nack, retry, and dead-letter behavior is configured.
- Dead-letter replay and purge procedures are documented.
- Background workers shut down within a configured deadline.

## Health and Deployment

- `/healthz` reports process liveness.
- `/readyz` reports dependency readiness.
- `/startupz` reports startup probe state where generated HTTP services are used.
- `/metrics` is scraped by Prometheus.
- Kubernetes liveness/readiness/startup probes are configured.
- HPA/PDB behavior is known for rolling deploys.
- SIGINT/SIGTERM graceful shutdown is verified.

## Observability

- Logs include request id or trace id through tracing Span fields.
- HTTP route, RPC method, gateway route, and queue metrics use consistent
  labels.
- Dashboards cover p50/p95/p99 latency, error rate, retry count, breaker state,
  queue depth, dead letters, and dependency failures.
- Trace sampling and log retention are configured.

## Security

- Auth middleware behavior and unauthorized/forbidden error codes are tested.
- JWT claims, key rotation, tenant isolation, and role/permission checks are
  documented.
- OpenAPI security declarations match runtime auth behavior.
- Dependency audit and license checks run in CI.
