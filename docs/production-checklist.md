# Production Checklist

This checklist is the baseline for considering a Roze service production-ready.
The implementation sequence and completion rules are defined in
[Roze Production Generation Plan](go-zero-surpass-plan.md).
Roze 1.0 provides stable public contracts. Passing this checklist means a
specific service has completed its controlled production review; framework API
stability does not replace deployment-specific capacity, failure, security,
and recovery evidence.

## Release and Upgrade

- Service is built from a tagged Roze release or a pinned Git revision.
- `CHANGELOG.md` and upgrade notes have been reviewed.
- `bash scripts/production-smoke.sh` passes.
- On Windows, `scripts/release-preflight.ps1` passes before the authoritative
  Linux/WSL `scripts/release-gate.sh`; the Windows preflight alone is not
  release evidence.
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
- `bash scripts/production-evidence-gate.sh` passes and no stable runtime area
  relies on an incomplete or inconclusive report.
- `bash scripts/production-release-audit.sh --json-out
  target/production-release-audit.json` is archived with the release gate; use
  `--require-long-run` when the release claims battle-tested runtime behavior.
- Public production-readiness wording follows
  [Stability Commitment](stability-commitment.md).

## Configuration and Secrets

- Runtime config is loaded from a controlled source.
- Config changes have a version, diff, author/source, and rollback path.
- Config Center management semantics, audit history, watch status, rollback,
  permission checks, and snapshot backup/restore or external control-plane
  integration are verified before treating it as stable.
- Secret values are not stored in generated config files.
- Application credentials are injected through validated Secret references;
  generated manifests contain names only, never secret payloads.
- Generated ConfigMap content is represented in the Pod template revision so
  configuration updates trigger a controlled rollout.
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
- Every network dependency declares its plaintext or TLS policy; sensitive or
  privileged internal dependencies use private-CA mutual TLS.
- TLS server identity validation is enabled, private keys come from managed
  secrets, and WSS/streaming transports cannot downgrade to plaintext.
- Certificate rotation and invalid TLS hot reload are tested; invalid updates
  retain the last valid runtime snapshot without interrupting in-flight calls.
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
- Generated Pod discovery annotations or an equivalent ServiceMonitor expose
  the framework `/metrics` endpoint on the configured service port.
- Prometheus Operator deployments enable the generated ServiceMonitor with an
  explicit scrape interval, timeout, and platform discovery labels.
- Kubernetes liveness/readiness/startup probes are configured.
- Generated workloads run as a fixed non-root UID/GID, use RuntimeDefault
  Seccomp, disable privilege escalation, mount a read-only root filesystem, and
  drop all Linux capabilities.
- Workload images are pinned by SHA-256 digest and unnecessary ServiceAccount
  token auto-mounting is disabled.
- Private registry credentials are referenced through validated
  `imagePullSecrets`; credentials never appear in generated values or manifests.
- Helm values are constrained by the generated JSON Schema before rendering or
  deployment.
- Offline Helm validation parses values and verifies cross-field HPA, metrics
  port, and scrape interval/timeout invariants.
- HPA/PDB behavior is known for rolling deploys.
- HPA covers CPU and memory pressure, caps scale velocity, and uses a
  stabilization window to prevent recovery-time replica thrashing.
- Rolling updates guarantee zero configured unavailability, define surge and
  rollout deadlines, spread replicas across hosts, and allow readiness draining
  during the generated pre-stop window.
- Kubernetes resource references satisfy DNS-1123 constraints and PDB
  availability cannot exceed the initial replica capacity.
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
- The required `supply-chain` CI job passes RustSec advisory, dependency
  license, and registry/source checks; exceptions are documented in
  `deny.toml` with an owner and review date.

## Capacity, Upgrade, and Disaster Recovery

- A capacity model records expected RPS/concurrency, CPU and memory budgets,
  connection-pool limits, queue lag, and the load-test threshold for each
  generated service.
- Load tests include warm-up, steady state, burst, dependency degradation,
  and recovery phases; the raw samples and environment digest are archived.
- API/proto, SQL, and search changes pass compatibility gates.  Dual-read,
  dual-write, gray rollout, and rollback steps are documented for changes
  that cannot be made atomically.
- Backup schedules, encryption, retention, restore verification, RPO, and RTO
  are explicit.  A restore drill is run on an isolated environment before a
  release is promoted.
- Cross-zone or cross-region recovery ownership is assigned, and the runbook
  records the dependency order for registry, config, database, broker, cache,
  and object storage recovery.

## Developer Experience and Migration

- The quickstart, `doctor`, environment, upgrade, completion, and template
  plugin commands are version-pinned and tested from a clean checkout.
- A go-zero-to-Roze migration sample passes generation, compile, shadow
  traffic, compatibility, and rollback checks; differences in SDK scope or
  middleware semantics are recorded in the migration matrix.
