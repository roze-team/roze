# rozectl generator

The model generator's ent capability definition and remaining release blockers
are tracked in [Roze Model / ent Capability Parity](../model-ent-parity.md).

`rozectl api generate` reads a Roze `.api` contract and generates a
Rust-native Roze native HTTP REST service. The generated project keeps framework-owned files
separate from application logic so repeated generation can preserve
method-level logic, service context extensions, config module extensions,
custom handler adapters, custom middleware, and `config.yaml` when `--update`
is used.

Think of `rozectl` as a generator for project structure and integration code.
It can generate API layers, RPC layers, model scaffolds, documentation, client
code, and deployment files. Business logic is still written by developers:
handler-driven logic, complex SQL, domain checks, transactions, authorization,
permissions, and other product-specific behavior live in the application.

`rozectl` 1.0 is the stable generator for Rust REST/RPC/stream services,
models, search, operations assets, OpenAPI, and TypeScript/JavaScript clients.
Generated contract changes follow Semantic Versioning and the gate policy.
Runtime evidence is tracked independently in [Module Maturity Matrix](../maturity.md)
and [Production Evidence](../production-evidence.md).

Install `rozectl` first if the binary is not available:

```bash
cargo install --git https://github.com/roze-team/roze.git rozectl
```

Force reinstall or upgrade an existing binary:

```bash
cargo install --git https://github.com/roze-team/roze.git rozectl --force
```

See [usage documentation](./README.md#install-rozectl) for local install,
upgrade, and PATH troubleshooting.

Inspect the installed binary environment or upgrade through the CLI:

```bash
rozectl env
rozectl upgrade
rozectl upgrade --branch main
rozectl upgrade --dry-run
```

Generate shell completion scripts:

```bash
rozectl completion bash
rozectl completion zsh
rozectl completion fish
rozectl completion powershell
```

goctl-compatible utility commands are available for migration workflows:

```bash
rozectl bug
rozectl bug --verbose
rozectl config init --o rozectl.yaml
rozectl config show --file rozectl.yaml
rozectl config path
rozectl migrate api --from go-zero --api user.api --o roze.api
rozectl migrate api user.api --write
```

`bug` prints a bug-report template with local rozectl/runtime information.
`config` provides a Roze-style config bootstrap and inspection surface.
`migrate api` parses a go-zero/Roze `.api` contract through the compatibility
parser and emits the canonical Roze `.api` format; use `--check` to fail when
the input is not already canonical, `--write` to update the input in place, or
`--o/--out` to write a migrated copy.

Create a starter REST or RPC project quickly:

```bash
rozectl quickstart
rozectl quickstart user-api --kind api --o services/user-api
rozectl quickstart user-rpc --kind rpc --o services/user-rpc
rozectl quickstart user-api --kind api --home templates
rozectl quickstart user-rpc --kind rpc --remote https://example.com/roze-templates.git --branch main
```

`quickstart` is a goctl-style convenience wrapper over the same starter
generators used by `rozectl api new` and `rozectl rpc new`; it does not create a
separate project layout. For starter project creation, `quickstart`,
`rozectl api new`, and `rozectl rpc new` accept `--home`, `--remote`, and
`--branch` to read `api.api` or `rpc.api` starter templates before generation.

Generated REST/RPC services pin `edition = "2021"` in their own `Cargo.toml`
instead of inheriting `edition.workspace`. Toasty is the default SQL model ORM,
and generated Toasty dependencies use MySQL/PostgreSQL features only; sqlite
support remains available through the Roze SeaORM/sqlx stack.

## Commands

Generate a REST service:

```bash
cargo run -p rozectl -- api generate example/user.api --out apps/roze-example --roze-source path
rozectl api gen example/user.api -o apps/roze-example
rozectl api gen --api example/user.api --dir apps/roze-example
```

goctl-compatible REST generation is also accepted:

```bash
rozectl api go --api example/user.api --dir apps/roze-example
rozectl api go -api example/user.api -dir apps/roze-example -style go_zero
rozectl api go -api example/user.api -dir apps/roze-example -home templates
```

For goctl migration, `rozectl` accepts Go flag-style single-dash long options
for known compatibility flags such as `-api`, `-dir`, `-style`, `-home`,
`-remote`, `-branch`, `-src`, `-collection`, and `-db-url`. Native Roze
examples use standard POSIX-style `--api` and short options such as `-o`.

Regenerate framework-owned files while preserving user logic and config:

```bash
cargo run -p rozectl -- api generate example/user.api \
  --out apps/roze-example \
  --update \
  --roze-source path
```

`--update` preserves:

- REST `src/logic/<group>/<method>.rs`
- REST `src/config/mod.rs`
- REST `src/handler/<group>/<method>.rs`
- REST/RPC `src/svc/mod.rs`
- REST service-wide middleware hook `src/middleware/app.rs`
- REST custom middleware files under `src/middleware/<name>.rs`
- RPC `src/config/mod.rs`
- RPC `src/logic/<method>.rs`
- `config.yaml`

Cross-service dependencies for both generated API and generated RPC consumer
services should be managed separately through `roze-service.yaml`:

```bash
rozectl service dependency add order \
  --project services/payment \
  --crate shop-order-rpc \
  --path ../shop-order-rpc \
  --endpoint 127.0.0.1:4002

rozectl service sync --project services/payment --check
```

This synchronizes Cargo, `config/roze-dependencies.yaml`, and the generated
RPC-client sections inside the preserved `ServiceContext`. The main
`config.yaml` and `ROZE__...` environment variables override generated
dependency defaults. The manifest records `kind: api` or `kind: rpc`, and sync
rejects a mismatch with the generated project boundaries. See
[RPC Client Configuration](../contracts/rpc-client-config.md).

Generated glue such as route registration, handler module indexes, DTOs,
OpenAPI, RPC server/client adapters, protobuf include modules, `build.rs`, and
`proto/service.proto` is refreshed. Use `--force` when you want a full rebuild.

## WebSocket routes

Use `@websocket` on a bodyless GET route to host WebSocket and HTTP endpoints
on the generated REST listener:

```text
service realtime-api {
    @websocket
    @handler realtime
    get /ws
}
```

The generated handler uses `roze_http::ws::WebSocketUpgrade` and connects the
socket to the service group's shutdown signal. Application-owned frame
handling is generated in `src/logic/<group>/realtime.rs`; two or more
`rozectl api generate --update` runs preserve that file and rebuild route
registration from the `.api` contract. WebSocket routes are excluded from
OpenAPI and normal HTTP client SDKs. They must use `EmptyReq`/`EmptyResp` and
cannot use idempotency middleware. See the
[native HTTP WebSocket contract](../contracts/websocket.md).

## Permission annotations and request context

Use `@permission` immediately before a REST route or RPC method to declare all
permissions required by that operation:

```text
service user-api {
    @permission users:read, users:list
    get /users (ListUsersReq) returns (ListUsersResp)

    @permission users:write
    rpc CreateUser (CreateUserReq) returns (UserResp)
}
```

Generated REST handlers call `roze_middleware::enforce_permissions` after
authentication; generated RPC servers call `roze_rpc::rpc::enforce_permissions`.
Every declared permission is required. Permission values are read from the
Roze request context metadata and propagate as `x-roze-meta-permissions`.
REST OpenAPI operations expose the declaration as `x-roze-permissions`.

Generated `src/logic/mod.rs` exposes stable helpers for application-owned
logic: `current_subject`, `current_user_id`, `current_admin_id`,
`current_tenant`, `current_roles`, `current_permissions`, and `current_scope`.
They read `roze_context::Context`; no generated file needs to be patched after
regeneration. Applications remain responsible for authenticating a request and
populating the context's subject, tenant, roles, scopes, and permissions.

## Idempotency middleware

Use `@middleware idempotency` immediately before a mutating REST route or RPC
method to opt into generated duplicate-request handling:

```text
service order-api {
    @middleware idempotency
    post /orders (CreateOrderReq) returns (CreateOrderResp)

    @middleware idempotency
    rpc CreateOrder (CreateOrderReq) returns (CreateOrderResp)
}
```

Generated REST handlers require an `Idempotency-Key` header. Generated RPC
servers require `idempotency-key` metadata. The generated `ServiceContext`
holds `Arc<dyn roze_middleware::IdempotencyStore>` and provides
`with_idempotency_store` for a persistent Redis or database adapter. The
in-memory default is for local development and tests.

Roze provides `RedisIdempotencyStore` as the production Redis adapter. It uses
Lua for atomic begin/complete/fail transitions, validates the request
fingerprint, reclaims expired execution leases, persists completed JSON for
response replay, and applies a bounded record TTL:

```rust
let mut config = roze_middleware::RedisIdempotencyConfig::new(&redis_url);
config.key_prefix = "shop:idempotency:v1".to_string();
config.record_ttl_millis = 86_400_000;
let store = roze_middleware::RedisIdempotencyStore::connect(config)?;
let ctx = ctx.with_idempotency_store(std::sync::Arc::new(store));
```

Use a service/environment-specific key prefix and load the Redis URL from
secret configuration. Adapter debug output never includes that URL.

Each record contains the key scope, a canonical request fingerprint, a
processing lease, and the completed JSON response. A completed matching request
replays the response, a live processing lease returns conflict, an expired
lease can be reclaimed, and reusing a key for a different request returns a
distinct conflict. Failed logic releases its unfinished record. REST error
bodies and headers and RPC status metadata expose stable codes including
`IDEMPOTENCY_MISSING_KEY`, `IDEMPOTENCY_IN_FLIGHT`,
`IDEMPOTENCY_KEY_REUSED`, `IDEMPOTENCY_STORAGE_UNAVAILABLE`, and
`IDEMPOTENCY_REPLAY_INVALID`.

Preview regeneration before writing files:

```bash
rozectl diff api example/user.api --out apps/roze-example --roze-source path
rozectl diff rpc example/user.api --out services/user-rpc --roze-source path
rozectl diff model example/user.sql --out services/user-api --format sql
```

`rozectl diff` writes nothing to the target directory. If the target exists, it
copies the target to a temporary workspace, runs generation in update mode, and
prints file-level changes as `A`, `M`, and `D`. This mirrors `--update`
ownership rules, so business-owned logic files, service context extensions,
and preserved config are not reported as modified unless generation would
actually change them.

Validate an API contract before generating:

```bash
rozectl api validate example/user.api
rozectl api validate --api example/user.api
```

`api validate` parses the `.api` file and checks contract-level consistency
that can otherwise surface only during generation or compilation: duplicate
types, duplicate fields or wire names, generated Rust type/field name
collisions, duplicate REST routes, duplicate RPC methods, generated REST
handler/RPC method name collisions, unknown request/response or nested field
types, non-empty reserved `EmptyReq`/`EmptyResp` declarations, invalid
generated service/REST/RPC/middleware Rust identifiers, and mismatches between
route path parameters and request `path` fields, including duplicate route path
parameters and duplicate generated custom middleware names.
Names that normalize to a single `_` are rejected because Rust does not allow
`_` as a generated item or field identifier.
API-derived generation commands run these generation-blocking checks before
writing or previewing generated files, including `api generate`, `rpc generate`,
`diff api`, `diff rpc`, client generation, OpenAPI, mock servers, smoke tests,
stream workers, docs, and API plugins.

Format an API contract:

```bash
rozectl api format example/user.api
rozectl api format --api example/user.api
rozectl api format example/user.api --check
rozectl api format example/user.api --write
```

`api format` parses the `.api` file and prints a canonical form to stdout by
default. Pass `--check` in CI to fail when a contract is not formatted, or
`--write` to replace the input file in-place after review. The formatter is
AST-based, so it intentionally normalizes layout and annotations instead of
preserving comments or ad hoc spacing.

Inspect and compare built-in starter templates:

```bash
rozectl template list
rozectl template show api
rozectl template init --out templates
rozectl template init --home templates
rozectl template init --home templates --remote https://example.com/roze-templates.git --branch main
rozectl template diff api --dir templates
rozectl template diff api --home templates
rozectl template update api --dir templates
rozectl template update api --home templates --remote https://example.com/roze-templates.git --branch main
rozectl template update api --dir templates --force
rozectl template revert api --dir templates
rozectl template revert api --dir templates --no-backup
```

`template init` writes the built-in API, RPC, and model starter templates into
the target directory. `template diff` compares one local template file
(`api.api`, `rpc.api`, or `model.model`) against the current built-in template
and prints a small unified diff without modifying the local file. `template
update` creates a missing local template from the built-in copy, but refuses to
overwrite a changed local template unless `--force` is passed. `template
revert` restores a local template to the built-in copy and writes a `.bak`
backup first unless `--no-backup` is passed.
`--home` is accepted as a goctl-compatible alias for the local template
directory. `template init`, `diff`, `update`, and `revert` also accept
`--remote` plus optional `--branch`; rozectl clones the remote template
repository temporarily, reads `api.api`, `rpc.api`, or `model.model`, and then
removes the temporary checkout. The same starter template source options are
accepted by `rozectl api new`, `rozectl rpc new`, and `rozectl quickstart`;
starter generation reads `api.api` or `rpc.api` from the selected source before
writing the project. Existing-contract generation commands such as
`rozectl api generate`, `rozectl api go`, `rozectl rpc generate`, and
`rozectl rpc protoc` also accept and validate `--home`, `--remote`, and
`--branch` for goctl muscle-memory compatibility, while Roze's Rust code
templates remain generator-owned.

Check contract breaking changes before regenerating or releasing:

```bash
rozectl contract check --old example/user.v1.api --new example/user.v2.api
```

`contract check` is read-only and exits with a non-zero status when it detects
breaking changes. The MVP checks removed or changed REST routes, removed RPC
methods, request/response type changes, removed fields, field type/source
changes, and newly added required fields. Additive optional fields with
`validate:"optional"` or `validate:"omitempty"` are allowed.

Generate one semantic regeneration report across REST, RPC, OpenAPI, and the
TypeScript SDK surface:

Generated TypeScript and JavaScript clients throw `RozeApiError` for
non-success responses. The typed error preserves HTTP status, business error
code, message, trace ID, structured details, and `Retry-After`; non-JSON
upstream responses safely fall back to an `HTTP_<status>` code.

TypeScript and JavaScript `RequestOptions` also support `authToken`,
`timeoutMs`, an external `AbortSignal`, `beforeRequest`, `afterResponse`, and a
bounded retry policy. Automatic retries are restricted to GET and HEAD,
hard-capped at five attempts, and use `Retry-After` or full-jitter exponential
backoff for HTTP 429/502/503/504 and transport failures. Mutating methods are
never replayed automatically.

```bash
rozectl contract diff \
  --old example/user.v1.api \
  --new example/user.v2.api \
  --out contract-diff.md
```

The report lists added, removed, and changed route/method signatures, OpenAPI
operations and schemas, and exported SDK interfaces/functions. It is written
before command failure so CI can retain the artifact. Breaking changes return a
non-zero status; pass `--allow-breaking` for an explicitly review-only run.

Run the release-facing semantic gate across API/OpenAPI, search, and SQL schema
contracts:

```bash
rozectl gate check \
  --manifest roze-gate.yaml \
  --report target/roze-gate.json \
  --markdown target/roze-gate.md
```

The manifest has version `1`, a non-empty `checks` list, and optional
acknowledgement file paths. Each check declares `domain: api|search|sql` plus
`old` and `new` files relative to the manifest. The JSON report has stable
`code`, `domain`, `severity`, `path`, `before`, `after`, and `remediation`
fields. Exit code `0` means pass, `1` means an unacknowledged blocking change,
and `2` means an invalid manifest, acknowledgement, or input contract.

Blocking changes may be acknowledged only by a checked-in YAML record bound to
the exact lowercase SHA-256 digests:

```yaml
version: 1
id: remove-legacy-user-field
scope: api
old_digest: <64 lowercase hex characters>
new_digest: <64 lowercase hex characters>
owner: identity-platform
reason: legacy field retirement
migration_plan: regenerate and deploy all callers before the server change
rollback_plan: restore the old contract and generated service revision
expires_at: 2026-12-31
```

Empty plans, stale hashes, expired records, and bare `--allow-breaking` flags do
not bypass `rozectl gate check`. The repository release gate writes both JSON
and Markdown artifacts before deciding whether the release is allowed.

Generate a mock server from the API contract:

```bash
rozectl mock gen --api example/user.api --out mock-server
cd mock-server
cargo run
```

The generated mock server is a standalone Roze native HTTP project. It registers the REST
routes declared in `.api` and returns default JSON values derived from each
route response type. Pass `--force` to overwrite mock server files in an
existing output directory.

Generate HTTP smoke tests from the same contract:

```bash
rozectl test gen --api example/user.api --out contract-tests
cd contract-tests
ROZE_TEST_BASE_URL=http://127.0.0.1:3000 cargo test
```

The generated contract test project uses `reqwest` and `tokio`. It builds sample
path, query, header, form, and JSON requests from the `.api` request types,
asserts successful HTTP status codes, and verifies JSON responses. It also
generates smoke checks for the framework-owned production endpoints:
`/healthz`, `/readyz`, `/startupz`, `/metrics`, `/openapi.json`,
`POST /reports/exports` and `POST /charts/query`. The default base URL is
`http://127.0.0.1:3000`; pass `--base-url` when generating or set
`ROZE_TEST_BASE_URL` at runtime.

Regeneration refreshes `Cargo.toml`, `README.md`, `tests/http_smoke.rs`, and
`tests/multi_service_smoke.rs`. Application-owned `tests/fixtures.rs` and
`tests/assertions.rs` are created once and never overwritten; use them for auth,
seed identifiers, request overrides, and domain assertions. Set
`ROZE_E2E_SERVICES=name=http://host:port,...` to run the generated readiness
flow against several services with one test command.

Check the local development environment:

```bash
rozectl doctor --config apps/roze-example/config.yaml --port 3000
rozectl doctor --tcp 127.0.0.1:6379 --tcp 127.0.0.1:9092
rozectl doctor --tool helm --tool etcdctl
```

Start, stop, or inspect the local dependency stack:

```bash
rozectl dev up --detach
rozectl dev status
rozectl dev down
```

`rozectl dev` defaults to `docker-compose.integration.yml`. Pass
`--file compose.yml` to use another Compose file and repeat `--profile name`
to enable Docker Compose profiles.

`rozectl doctor` checks the default local tools `rustc`, `cargo`, `docker`, and
`kubectl`. Extra `--tool` values are checked with `--version`. `--config`
verifies that a config file exists, and each `--port` verifies that the port is
available on `127.0.0.1`. Each `--tcp host:port` verifies that a dependency
endpoint is reachable with a TCP connection, which is enough for local Redis,
Kafka, NATS, etcd, Consul, or database smoke checks. Missing optional tools are
reported as `WARN`; missing config files, unavailable ports, or unreachable TCP
targets are reported as `FAIL` and return a non-zero exit code.

For a project-level external dependency check, run the Roze example verifier:

```bash
scripts/roze-project-external-smoke.sh
```

That script starts `docker/docker-compose.yml`, waits for Postgres, MySQL,
Redis, Kafka, NATS, MongoDB, Elasticsearch, OpenSearch, Meilisearch, Etcd, and
Consul to become healthy, then runs `cargo run -p roze-example --bin
external_verify`. The verifier uses Roze runtime components instead of raw
container probes: `roze-db`, `roze-cache`, `roze-kafka`, `roze-nats`,
`roze-mongo`, `roze-search`, and `roze-rpc::registry`. On success it prints one
`PASS` line for each dependency and cleans up the Docker stack.

## Generated REST layout

```text
config.yaml
Cargo.toml
src/
  main.rs
  config/mod.rs
  route/
    mod.rs
    <group>.rs
  handler/
    mod.rs
    <group>/
      mod.rs
      <method>.rs
  logic/
    mod.rs
    <group>/
      mod.rs
      <method>.rs
  middleware/
    mod.rs
    <custom>.rs
  openapi/mod.rs
  svc/mod.rs
  types/mod.rs
```

Generated API crates do not depend on `roze-db`, `roze-mongo`, or Toasty by
default. API services can still call RPC clients, cache, MQ, NATS, outbox, auth,
metrics, OpenAPI, validation, and middleware crates.

Generated REST and RPC `ServiceContext` values expose
`Arc<dyn roze_transaction::OutboxStore>`. The default
`InMemoryOutbox` is intended for local development and tests. Production
services should inject a persistent adapter with `with_outbox_store`. A
persistent adapter implements asynchronous claim, publish/failure state, and
lease recovery through `OutboxStore`. Database adapters additionally implement
`TransactionalOutbox<Tx>` so business writes and outbox messages are inserted
in the same database transaction before it commits. `relay_outbox_batch` then
publishes claimed messages after commit and records retry state without
duplicating application sequencing code.

Generated REST services expose standard operational endpoints:

- `GET /healthz` for process liveness
- `GET /readyz` for readiness
- `GET /startupz` for startup state
- `GET /metrics` for Prometheus metrics
- `POST /reports/exports` to create a tenant-bound asynchronous CSV/XLSX export
- `GET /reports/exports/:id` to read export status and download metadata
- `DELETE /reports/exports/:id` to cancel an export
- `POST /charts/query` for a bounded structured chart-series query
- `GET /openapi.json` for OpenAPI

The default health handlers return `roze_health::ProbeReport` inside the
standard `ApiResponse` wrapper. Generated `ServiceContext` owns a
`roze_health::HealthRegistry`; dependency clients that are created during
startup are registered as readiness checks, and the registry tracks startup,
ready, and draining phases. Applications can add more dynamic checks with
`HealthRegistry::register_dependency` or `HealthRegistry::register_check`.
Registered checks execute concurrently and retain registration order in the
report. Each check has a two-second default timeout; a timeout is returned as
an unhealthy result instead of stalling the probe. Services can set a stricter
budget with `HealthRegistry::with_check_timeout(Duration)`.
Panics from application-provided checks are isolated into an unhealthy check,
so one faulty dependency probe cannot abort the complete readiness response.
Generated report exports run asynchronously, render real CSV or XLSX through
`roze-report`, escape spreadsheet formulas, enforce row/column/file limits,
write through `roze-storage`, and expose bounded status/cancel/download
resources. When JWT is configured, export and chart operations require a token
with a tenant claim; status and cancellation are restricted to the creating
subject and tenant. Export and chart execution use an application-owned
`Arc<dyn roze_report::ReportDataSource>`; an unconfigured source returns `503`
instead of a successful empty result. Register a `ReportCatalog` through the
preserved async `src/application.rs` hook:

```rust
pub async fn configure_context(ctx: ServiceContext) -> anyhow::Result<ServiceContext> {
    let reports = roze_report::ReportCatalog::new()
        .register_export("sales", |context, query| async move {
            context.ensure_active()?;
            load_tenant_sales(context.tenant_id, query).await
        })?
        .register_chart("sales-total", |context, query| async move {
            context.ensure_active()?;
            aggregate_tenant_sales(context.tenant_id, query).await
        })?;
    Ok(ctx.with_report_source(std::sync::Arc::new(reports)))
}
```

Handlers receive the authenticated subject, tenant, cancellation signal, and
bounded structured query rather than client-supplied SQL. The shared executor
enforces cancellation, chart timeout/result limits, export row/column/file
limits, and spreadsheet escaping. Set `ROZE_REPORT_SOURCE_CONFIGURED=1` when
running generated HTTP smoke tests against a service with this source.

## Read-model query composition

Generated REST and RPC projects include `roze-query`. Application-owned logic
can build named `QueryTask` values and execute them through `QueryComposer`.
`QueryCompositionConfig` defines a total request budget, per-upstream timeout,
maximum fan-out, maximum concurrent calls, and either strict or partial-failure
behavior. Results retain declaration order even when upstream calls complete
out of order. Partial mode returns successful values alongside structured
timeout, upstream, or cancellation failures; strict mode returns an error when
any upstream fails. Every call creates a `roze.query.call` tracing span with the
upstream name.

```rust
let batch = roze_query::QueryComposer::new(roze_query::QueryCompositionConfig {
    partial_failure: roze_query::PartialFailurePolicy::Allow,
    ..Default::default()
})
.execute(vec![
    roze_query::QueryTask::new("catalog", || async { Ok(load_catalog().await?) }),
    roze_query::QueryTask::new("inventory", || async { Ok(load_inventory().await?) }),
])
.await?;
```

## Object storage integration

Generated REST and RPC `ServiceContext` values expose an optional
`Arc<dyn roze_storage::ObjectStorage>`. Local storage is constructed from
`config.storage`; cloud/provider implementations can be injected with
`with_storage` without changing generated handlers or RPC adapters.

`issue_upload_token` returns a normalized key, expiration, upload policy, and
provider presigned request. `FileMetadata` is the stable metadata contract for
stored objects. `resolve_media_url`, also exposed as `ServiceContext::media_url`,
uses a public object URL when available and otherwise returns a time-bounded
provider URL plus required headers. Application logic therefore stores object
keys and metadata rather than constructing provider URLs itself.

## Cache consistency

Generated model repositories use versioned keys from
`roze_cache::model_cache_key`: `model:v1:<prefix>:<field>:<escaped-value>`.
Create, update, delete, and soft-delete paths invalidate every configured lookup
key after the database write succeeds. `InvalidationPlan` provides the same
deduplicated key convention to RPC methods and application-owned workflows that
perform writes outside a generated repository.

For read models that may tolerate bounded staleness,
`get_or_load_consistent_option` accepts `CacheConsistencyPolicy` with fresh,
stale, and negative TTLs. Fresh entries are returned immediately; expired-fresh
entries are reloaded, and may be returned as `CacheFreshness::Stale` only when
the reload fails and `stale_on_error` is enabled. The hard Redis TTL is the
fresh plus stale window, so stale values cannot survive beyond the configured
bound.

## Model fixtures and seeds

Every generated SeaORM, Toasty, and Mongo model exposes
`Model::fixture(index)` (or the generated Toasty model type's equivalent).
Fixture values are deterministic for the same index, generate distinct common
scalar values, choose the first declared enum value, and populate `Option` and
collection fields. Generated repositories expose `seed_fixtures(count)` to
insert a repeatable sequence with the model's normal write and cache
invalidation path. Put application-specific relationships, cleanup, and
assertions in the preserved `<model>_ext.rs` module rather than editing the
generated fixture code.

New generated REST, RPC, and stream worker entrypoints run under
`roze_service::ServiceGroup`. When shutdown starts, generated REST/RPC lifecycle
tasks mark the shared `HealthRegistry` as draining, so REST `/readyz` stops
reporting ready while the process exits through the unified shutdown path.
Generated stream consumers listen to the same shutdown signal and stop worker
tasks before returning.

Generated REST and RPC services also include `ops/production-evidence.md`,
`ops/governance-baseline.yaml`, `ops/prometheus-rules.yaml`, and
`ops/grafana-dashboard.json`, `ops/slo.yaml`, and
`ops/failure-injection-plan.yaml`, `ops/release-rollout.yaml`, and
`ops/incident-response.yaml`, `ops/capacity-plan.yaml`, and
`ops/security-readiness.yaml`, `ops/production-gate.yaml`, and
`ops/regeneration-policy.yaml`, `ops/client-contract.yaml`, and
`ops/config-governance.yaml`, `ops/reliable-events.yaml`, and
`ops/dependency-governance.yaml`, `ops/data-consistency.yaml`, and
`ops/observability-contract.yaml`, `ops/runtime-hardening.yaml`, and
`ops/error-contract.yaml`, `ops/deployment-topology.yaml`, and
`ops/service-communication.yaml`, `ops/cache-governance.yaml`, and
`ops/data-access-governance.yaml`, `ops/interface-governance.yaml`,
`ops/production-verify.ps1`, `ops/production-verify.sh`,
`ops/ci-evidence-policy.yaml`, `ops/evidence-manifest.yaml`, and
`.github/workflows/roze-production-verify.yml`. The runbook records the
generated service boundary, required production gates,
`scripts/production-evidence.sh --area generated-services`, and lifecycle
summary collection with `--lifecycle-summary`. Run
`powershell -ExecutionPolicy Bypass -File ops\production-verify.ps1` or
`bash ops/production-verify.sh` in CI to fail fast on missing generated ops
assets, format drift, compile errors, and test failures before collecting
long-run evidence. The generated GitHub Actions workflow runs the same gates on
Linux and Windows runners and uploads an `ops/**` evidence bundle. The CI
evidence policy records artifact naming, retention, required paths, blocking
conditions, and the rule that CI success is a precondition, not a replacement,
for soak and failure-injection evidence. Each verification run writes
`ops/production-verify-report.json`; both platform scripts validate its service
boundary, conservative verdict, required gate list, and long-run evidence
requirements before the report can be uploaded. The evidence manifest indexes
every generated ops contract, verification script, workflow, smoke surface, and
promotion evidence requirement uploaded in the CI artifact. The YAML baseline is
machine-readable for CI/platform checks and captures the go-zero inspired
architecture baseline Roze expects before broad rollout: simple IDL-first
ownership, timeout, rate limit, circuit breaker, load shedding, retry budget,
deadline propagation, discovery, load balancing, tracing, metrics, health
checks, structured logs, and explicit extension points. The Prometheus rules
cover service down, error rate, p99 latency, rate-limit rejection, breaker open,
load shedding, and restarts. The Grafana dashboard covers request rate, error
rate, p99 latency, resilience decisions, and restarts. Roze extends that
baseline with generated evidence gates. The SLO file defines default
availability, success-rate, p99-latency, resilience-rejection, burn-rate, and
promotion requirements. The failure-injection plan defines staging drills for
shutdown, slow dependencies, 5xx dependency failures, rate-limit pressure,
load-shedding pressure, invalid config reload, and restart recovery, with the
metrics, traces, logs, recovery time, and rollback notes required for each
scenario. The release rollout plan defines preflight, canary, progressive
rollout, full rollout, post-release observation, blue-green checks, and rollback
evidence. The incident response playbook maps generated alerts to severity,
confirmation queries, mitigation, rollback criteria, escalation, and postmortem
evidence. The capacity plan defines baseline characterization, step load, burst,
24h soak, 72h soak, scale-out, scale-in, resource trend, and owner signoff
evidence. The security readiness plan defines authentication, authorization,
tenant isolation, key rotation, mTLS, audit log, sensitive data, and dependency
security evidence. The production gate file ties every generated production
asset into a CI/platform-readable promotion contract with blocking rules and
controlled-production versus broad-production-stable levels. The regeneration
policy defines generator-owned files, preserved extension points, IDL drift
classification, breaking-change gates, and evidence refresh rules. The client
contract defines SDK/OpenAPI/proto projection, typed errors, auth injection,
timeout, retry budget, cancellation, and trace propagation evidence. The config
governance plan defines schema validation, diff/version, audit, canary reload,
rollback, listener isolation, and snapshot restore evidence. The reliable events
plan defines event envelopes, idempotency, outbox/inbox, DLQ, replay, lag
metrics, retry budget, and retry storm protection evidence. The dependency
governance plan defines downstream inventory, discovery, load balancing,
deadline propagation, circuit breakers, bulkheads, fallback, and outlier
evidence. The data consistency plan defines transaction boundaries, idempotent
writes, migrations, outbox/DTM/Saga, read-write consistency, reconciliation,
backup restore, and data rollback evidence. The observability contract defines
metrics, logs, traces, profiles, sampling, label cardinality, debug queries, and
evidence retention. The runtime hardening contract defines timeout, rate limit,
circuit breaker, load shedding, retry budget, deadline propagation, graceful
shutdown, backpressure, and resource guard evidence. The error contract defines
typed errors, transport status mapping, retryability, trace correlation, client
behavior, redaction, and failure metrics. The deployment topology contract
defines probes, resources, scaling, disruption budgets, config/secret wiring,
registry, network policy, image pinning, and rollback evidence. The service
communication contract defines downstream inventory, discovery, load balancing,
client deadlines, retry budget, circuit breakers, fallback, outlier handling,
and trace propagation evidence. The cache governance contract defines cache key
ownership, TTL, local/remote cache policy, singleflight, penetration/breakdown/
avalanche protection, invalidation, consistency, and cache observability
evidence. The data access governance contract defines query deadlines,
connection pools, slow-query budgets, pagination, index review, read/write
splitting, N+1 protection, and data-access observability evidence.
The interface governance contract defines framework-owned endpoints, IDL-owned
business routes or RPC methods, OpenAPI/proto projection, framework smoke
coverage, auth/error boundaries, and bounded observability labels.
Treat the runbook, YAML baseline, alert rules, dashboard, SLO file,
failure-injection plan, release rollout plan, incident response playbook,
capacity plan, security readiness plan, production gate, regeneration policy,
client contract, config governance plan, reliable events plan, and dependency
governance plan, data consistency plan, observability contract, runtime
hardening contract, error contract, deployment topology contract, and service
communication contract, cache governance contract, data access governance
contract, and interface governance contract as the default promotion checklist
for each generated service.

Generate client SDKs:

```bash
rozectl api client ts example/user.api --out sdk/user.ts
rozectl api client ts example/user.api --o sdk/user.ts
rozectl api client js example/user.api --out sdk/user.js
rozectl api ts --api example/user.api --dir sdk
rozectl api js --api example/user.api --dir sdk
```

Generate an OpenAPI 3 document:

```bash
rozectl openapi generate example/user.api --out openapi.json
rozectl openapi gen example/user.api --o openapi.json
rozectl openapi gen --api example/user.api --o openapi.json
rozectl api swagger --api example/user.api --dir docs
rozectl api swagger --api example/user.api --dir docs --yaml
```

Validator tags project required/optional fields, string and collection lengths,
numeric bounds, `oneof` enums, UUID/email/URI/IP formats, array element rules,
and map value/property constraints into component and inline request schemas.
Cross-field conditional and custom validators remain runtime validation rules
and are not misrepresented as static OpenAPI constraints. Their source rule is
preserved in `x-roze-validator`; map-key rules use
`x-roze-map-key-schema` because OpenAPI 3.0 does not standardize
`propertyNames`.

`api swagger` is the goctl-compatible entry point and writes
`swagger.json` under `--dir`; pass `--yaml` to write `swagger.yaml` instead.

Generate Markdown API documentation:

```bash
rozectl api doc --api example/user.api --dir . --out docs/api
rozectl api doc --api example/user.api --dir . --o docs/api
```

Run a custom API plugin:

```bash
rozectl api plugin --plugin ./tools/rozectl-plugin.sh --api example/user.api --dir generated
```

Generate an RPC service from a real `.proto` file:

```bash
rozectl rpc protoc example/user.proto --out services/user-rpc
rozectl rpc gen example/user.api --o services/user-rpc
rozectl rpc gen --api example/user.api --dir services/user-rpc
rozectl rpc gen --api example/user.api --dir services/user-rpc -m
rozectl rpc gen --api example/user.api --dir services/user-rpc --home templates
rozectl rpc protoc example/user.proto --out services/user-rpc --multiple
rozectl rpc protoc example/user.proto --out services/user-rpc --home templates
rozectl rpc template -o rpc.api
rozectl rpc template --o rpc.api
```

`rpc template` is provided for goctl command-shape compatibility. Without
`-o/--out`, it prints the built-in RPC `.api` starter template to stdout.
`-m/--multiple` is accepted for goctl-compatible RPC command shape; Roze RPC
projects already generate split server, client, protobuf, service context, and
logic modules. `rpc generate` and `rpc protoc` accept `--home`, `--remote`, and
`--branch` for goctl command compatibility and validate that the selected RPC
template source exists.

Generate models:

```bash
rozectl model generate example/user.sql --out services/user-api --format sql
rozectl model gen example/user.sql --o services/user-api --format sql
rozectl model inspect users --db-kind mysql --db-url mysql://root:root@127.0.0.1:3306/roze --out services/user-api
rozectl model inspect users --db-kind postgres --db-url postgres://postgres:postgres@127.0.0.1:5432/roze --schema public --out services/user-api
rozectl model generate example/user.model --out services/user-api --format mongo
rozectl model inspect users --db-kind mongo --db-url mongodb://127.0.0.1:27017/roze --out services/user-api
rozectl model mysql datasource --url mysql://root:root@127.0.0.1:3306/roze --table users --dir services/user-api
rozectl model pg ddl --src example/user.sql --dir services/user-api
rozectl model mongo --collection users --db-url mongodb://127.0.0.1:27017/roze --dir services/user-api
rozectl search generate example/user.search --engine elasticsearch --out services/user-api
rozectl search gen example/user.search --engine elasticsearch --o services/user-api
rozectl search inspect users --engine meilisearch --url http://127.0.0.1:7700 --out services/user-api
```

Generate deployment files:

```bash
rozectl docker --port 8080 --binary user-api
rozectl kube deploy --name user-api --image registry.example.com/user-api@sha256:<64-hex-digest> --port 8080
```

## Supported `.api` syntax

The parser accepts Roze contracts plus go-zero-compatible API forms:

- `syntax = "v1"` declarations.
- `info (...)` blocks.
- `type Name { ... }` and grouped `type (...)` blocks.
- Compact block starts such as `info(`, `type(`, and `@server(`.
- `service name` declarations and `service name { ... }` REST/RPC blocks.
- Multiple `service name { ... }` blocks with the same name; routes are merged
  in declaration order.
- `@server`, `@handler`, `@doc`, and `@middleware` annotations, including
  compact go-zero forms such as `@handler(getUser)`, `@doc("Get user")`, and
  `@middleware(auth, trace)`.
- `import (...)` blocks.
- Route/RPC signatures with either `returns (Resp)` or `returns(Resp)`,
  including legacy goctl spacing such as `/shorten(Req) returns(Resp)`.
- HTTP methods: `get`, `head`, `post`, `put`, `patch`, `delete`.
- Anonymous embedded fields inside types, rendered as flattened serde fields in
  generated Rust DTOs.
- Go-style field tags for `path`, `query`, `form`, `header`, `json`, and
  `validate`. JSON options such as `json:"name,optional"` keep `name` as the
  wire name, while `validate:"optional"` and `validate:"omitempty"` skip
  generated validation and are treated as additive-safe optional fields in
  contract checks.

Example:

```go
syntax = "v1"

info (
  title: "User API"
  desc: "Generated by rozectl"
)

@server (
  prefix: /api/v1
  middleware: trace
  jwt: Auth
)
service user-api {
  @handler getUser
  @doc "Get a user"
  get /users/:id (GetUserReq) returns (UserResp)

  @handler ping
  head /ping ()

  @handler logout
  post /logout
}
```

Multiple same-name service blocks are accepted and merged:

```go
service user-api {
  @handler getUser
  get /users/:id (GetUserReq) returns (UserResp)
}

service user-api {
  @handler createUser
  post /users (CreateUserReq) returns (UserResp)
}
```

go-zero-style anonymous embedding is accepted in grouped or standalone type
declarations:

```go
type (
  BaseReq {
    traceId string `json:"traceId,optional" validate:"optional"`
  }

  CreateUserReq {
    BaseReq
    name string `json:"name"`
  }
)
```

Generated Rust DTOs use `#[serde(flatten)]` for embedded fields, so JSON bodies
continue to use the embedded type's field names rather than a nested
`baseReq` object.

## Route-scoped `@server`

Multiple `@server` blocks inside a service apply to following routes until the
next `@server` block. Route-scoped values override the top-level server block
for path prefix, middleware, and JWT/OpenAPI security.
When `group` is set, it controls the generated REST handler/logic grouping
instead of deriving the group from the first route path segment. Multi-level
go-zero groups such as `admin/user` are accepted and normalized into safe Rust
module names such as `admin_user`.

```go
@server (
  prefix: /api
)
service user-api {
  @server (
    prefix: /api/v1
    group: admin/user
    jwt: Auth
  )
  @handler getUser
  get /users/:id (GetUserReq) returns (UserResp)

  @server (
    prefix: /internal
    middleware: audit
  )
  @handler stats
  get /stats (StatsReq) returns (StatsResp)
}
```

Generated routes:

- `GET /api/v1/users/:id`
- `GET /internal/stats`

## Middleware

Service-wide HTTP middleware is configured in generated `config.yaml` under
`rest.middlewares`:

```yaml
rest:
  addr: 127.0.0.1:3000
  register: false
  middlewares:
    recover: true
    trace: true
    stat: true
    prometheus: true
    cors: true
    # cors_config:
    #   allow_origins: ["*"]
    #   allow_methods: ["GET", "POST", "PUT", "PATCH", "DELETE"]
    #   allow_headers: ["authorization", "content-type", "x-request-id", "x-trace-id"]
    #   expose_headers: ["x-request-id", "x-trace-id"]
    #   allow_credentials: false
    #   max_age_seconds: 3600
    timeout: true
    # max_conns: 1000
    # shedding:
    #   concurrency: 1000
    #   window_ms: 1000
    #   min_samples: 100
    #   max_avg_latency_ms: 500
    #   max_failure_ratio_per_mille: 500
    #   cool_down_ms: 1000
    # gunzip: true
    # request_body_limit_bytes: 2097152
```

Route-scoped middleware declared in `.api` is resolved as either built-in or
custom. Built-in names include `auth`, `jwt`, `trace`, `recover`, `stat`,
`prometheus`, `metrics`, `cors`, `timeout`, `rate_limit`, `breaker`,
`max_conns`, `shedding`, `gunzip`, `body_limit`, and `idempotency`. Built-ins do
not generate custom files.

Unknown middleware names are application-owned hooks. For example,
`middleware: auth, audit` uses built-in auth and generates
`src/middleware/audit.rs`. Custom middleware files are preserved during
`--update`. The name `app` is reserved for the service-wide application hook.

Generated REST entrypoints call `middleware::app::apply` before installing
Roze common middleware. Edit `src/middleware/app.rs` to attach service-wide
application middleware; the file is preserved during `--update`. Roze common
middleware wraps this hook so request context restoration and CORS preflight
run before application authentication or other route processing.

See [Middleware Contract](../contracts/middleware.md) for the complete alias
table and adaptive shedding behavior.

`timeout: true` makes generated route glue apply the service-wide
`governance.timeout_ms` through Roze middleware. Generated handler adapters also
enforce route-specific timeout overrides from `governance.routes`. Set
`timeout: false` when you only want timeout metadata propagated through
`roze_context::Context` and do not want generated HTTP adapters to cancel
long-running logic.
Generated REST and RPC `config.yaml` files now include per-route or per-method
`governance.routes` entries by default: timeout, retry budget, rate limit, and
circuit breaker, and load-shedding settings are explicit for every generated
operation. REST `GET`/`HEAD` routes default to two retry attempts, while
mutating routes default to one attempt so non-idempotent writes are not retried
accidentally. REST and RPC runtimes consume those route/method settings:
concurrency pressure, high latency, or elevated failure ratio can shed load
before worker saturation turns into a wider outage.
Generated configs also include explicit global and per-route/per-method
`fallback` entries. They default to `enabled: false`, so services fail closed
until an operator enables a documented degradation response; the REST/RPC
runtime policy helpers resolve route/method fallback before global fallback and
ignore disabled entries. Generated adapters apply fallback only to server-side
errors, so validation and authorization failures are not hidden by degradation
responses. REST fallback responses use the configured status, JSON body, and
headers; RPC fallback responses surface fallback status/body/headers in gRPC
metadata, and `roze_rpc::rpc::error_from_status` restores them as
`RozeError::Fallback` for typed clients.
Generated RPC clients can also be bound to a `GovernanceConfig` with
`with_governance`; each client method then reads its `governance.routes`
retry policy, applies method-specific retry attempts and backoff caps, and uses
the generated retry budget to avoid retry storms.
`rpc_client.balancer` selects the client-side balancing strategy for discovery
mode, with `power_of_two_choices` as the default and `first_available`,
`round_robin`, `weighted_round_robin`, and `health_aware` available when the
registry supplies the needed instance metadata. Static `rpc_client.endpoints`
use the same balancer setting, so single-binary deployments and registry-backed
deployments exercise the same client selection contract.

Business logic should not pass or construct `trace_id` values. Use
`tracing::info!`, `tracing::warn!`, and `tracing::error!` directly in
`src/logic/**`; the request Span created by Roze middleware carries the
`trace_id`. Use `ServiceContext` for global resources and Roze native HTTP `Extension<T>`
for per-request user/session objects injected by custom middleware.

Generated REST, RPC, and Stream entrypoints emit structured lifecycle logs for
configuration readiness, dependency context initialization, registry changes,
listener/subscription readiness, shutdown, stop, and failure. Native HTTP logs
request start/completion with method, path, status, latency, request ID, and
trace ID. RPC governance logs method start/completion/cancellation with service,
method, code, latency, request ID, and trace ID. Generated logging never prints
request or message payloads; application logic remains responsible for
domain-specific events and must redact secrets and personal data.

With `RUST_LOG=debug`, Roze also reports safe framework decisions: REST router
construction, middleware plans, and route-match outcomes; RPC endpoint labels,
retry attempts, deadlines, and governance flags; Stream topic bindings,
message IDs, attempts, and ack/nack decisions; Model query kinds, pagination,
eager-load edge paths, and transaction phases; and ServiceGroup membership and
health phase changes. These events deliberately omit request/message bodies,
authorization values, SQL arguments, fallback payloads, and dependency error
messages. Endpoint labels are reduced to scheme and authority without userinfo,
path, query, or fragment.

Generated REST services pass their Router through
`roze_middleware::apply_common_with_config`. Its request-context layer restores
or creates `roze_context::Context` from incoming propagation headers, including
request IDs, trace IDs, metadata, and timeouts, before handler extraction.
Handlers extract `Extension<Context>`; bypassing the common middleware causes
requests to fail with `missing extension` before business logic runs.
When REST timeout middleware is enabled, the generated router passes the
service-wide `governance.timeout_ms` to `roze_middleware::apply_timeout`.
Expired requests cancel the in-flight handler future and return HTTP `504` with
`request timeout`; route-specific handler timeouts can still impose a shorter
effective deadline.
`rest.middlewares.request_body_limit_bytes` is enforced against the actual body
before extraction, including chunked requests without `Content-Length`.
Oversized requests return HTTP `413` with `request body too large`; accepted
bodies remain available to JSON, form, and custom extractors.

`cors: true` enables CORS. Without `cors_config`, generated services use a
permissive development default. Add `cors_config` to restrict browser
origins, methods, request headers, exposed response headers, credentials, and
preflight max age.
Preflight `OPTIONS` requests pass through the common CORS layer before default
method rejection. Credentialed wildcard policies mirror the request origin
instead of emitting the browser-invalid `Access-Control-Allow-Origin: *`.

## Empty request and response

Routes may omit request and/or response types.

```go
service health-api {
  @handler health
  get /health returns (HealthResp)

  @handler ping
  get /ping

  @handler logout
  post /logout (LogoutReq)
}
```

`rozectl` automatically supplies:

- `EmptyReq` when a route has no request.
- `EmptyResp` when a route has no response.
- TS/JS clients default empty request arguments to `{}`.

## Field sources

Fields can bind from path, query, form, header, or JSON body tags.

```go
type GetUserReq {
  id u64 `path:"id"`
  q string `query:"q"`
  token string `header:"X-Token"`
  name string `form:"name"`
  profile Profile `json:"profile"`
}
```

If no source tag is present, `rozectl` infers path parameters from route
segments such as `:id`. Remaining untagged fields become query fields for
`GET` and `DELETE` routes and JSON body fields for mutating routes.

Generated HTTP query extractor structs apply `serde(default)` to each query
field, so omitted filters deserialize to Rust defaults such as `None`, `0`,
`false`, an empty string, or an empty collection. Validator tags still run after
deserialization; use `validate:"required"` or a nonzero range rule when a query
parameter must be present.

During `--update`, generated REST projects preserve existing `src/svc/mod.rs`
so application-owned service clients and dependency accessors are not
overwritten. Group-level logic module indexes such as `src/logic/admin/mod.rs`
are refreshed for generated handlers while preserving extra app-owned
`mod ...;` declarations.

## Type mapping

Scalar types:

| `.api` type | Generated Rust type |
| --- | --- |
| `string` / `String` | `String` |
| `int` | `i64` |
| `int8`, `int16`, `int32`, `int64` | `i8`, `i16`, `i32`, `i64` |
| `uint` | `u64` |
| `uint8`, `uint16`, `uint32`, `uint64` | `u8`, `u16`, `u32`, `u64` |
| `bool` | `bool` |
| `i32`, `i64`, `u32`, `u64`, `f32`, `f64` | same Rust type |
| custom type | component/reference type |

Container types:

| `.api` type | Generated Rust type |
| --- | --- |
| `[]string` | `Vec<String>` |
| `[]int` | `Vec<i64>` |
| `[]int64` | `Vec<i64>` |
| `[]T` | `Vec<T>` |
| `Vec<T>` | `Vec<T>` |
| `map[string]string` | `std::collections::HashMap<String, String>` |
| `map[string]int` | `std::collections::HashMap<String, i64>` |
| `map[string]int64` | `std::collections::HashMap<String, i64>` |
| `map[K]V` | `std::collections::HashMap<K, V>` |
| `HashMap<K,V>` | `std::collections::HashMap<K, V>` |

Generated DTO fields use stable snake_case Rust names plus `serde(rename = "...")`
when wire names differ.

## Validator tags

`rozectl` reads Go validator-style tags from struct fields:

```go
type CreateUserReq {
  name string `json:"name" validate:"required,min=2,max=32"`
  email string `json:"email" validate:"required,email"`
  status string `json:"status" validate:"oneof=active disabled"`
}
```

Validation has two layers:

- Native Rust `validator` derive attributes for rules it supports directly.
- Generated request-level checks for Go validator rules that Rust derive does
  not support natively.

### Native derive mappings

| Go validator tag | Generated Rust validator |
| --- | --- |
| `required` on string/container | `length(min = 1)` |
| `min=N`, `max=N` on string/container | `length(min = N, max = N)` |
| `len=N` on string/container | `length(equal = N)` |
| `min=N`, `max=N` on number | `range(min = N, max = N)` |
| `gte=N`, `lte=N` on number | `range(min = N, max = N)` |
| `gt=N`, `lt=N` on number | `range(exclusive_min = N, exclusive_max = N)` |
| `nonnegative` on signed number | `range(min = 0)` |
| `page` on number | `range(min = 1)` |
| `limit` on number | `range(min = 1, max = 1000)` |
| `min_items=N`, `max_items=N` on container | `length(min = N, max = N)` |
| `email` | `email` |
| `url`, `uri` | `url` |
| `ip` | `ip` |
| `ipv4` | `ip(v4 = true)` |
| `ipv6` | `ip(v6 = true)` |
| `contains=value` | `contains = "value"` |
| `excludes=value` | `does_not_contain = "value"` |
| `optional`, `omitempty` | skip generated validation |

### Generated request-level checks

| Go validator tag | Behavior |
| --- | --- |
| `oneof=a b c` | value must match one of the listed values |
| `startswith=x` | string must start with `x` |
| `endswith=x` | string must end with `x` |
| `alpha` | string must contain alphabetic characters only |
| `alphanum` | string must contain alphabetic or numeric characters only |
| `ascii` | string must be ASCII |
| `code` | non-empty ASCII code containing only letters, digits, `_`, `-`, or `.` |
| `json` | string must parse as a JSON value |
| `numeric` | string must parse as `f64` |
| `lowercase` | string must not contain uppercase characters |
| `uppercase` | string must not contain lowercase characters |
| `eqfield=Other` | field must equal another field of the same generated Rust type |
| `nefield=Other` | field must not equal another field of the same generated Rust type |
| `gtfield=Other` | field must be greater than another field of the same generated Rust type |
| `gtefield=Other` | field must be greater than or equal to another field |
| `ltfield=Other` | field must be less than another field |
| `ltefield=Other` | field must be less than or equal to another field |
| `required_if=Other value` | string is required when another field equals `value` |
| `required_unless=Other value` | string is required unless another field equals `value` |
| `required_with=Other` | string is required when another field is non-empty |
| `required_without=Other` | string is required when another field is empty |

Cross-field comparisons are generated only when both fields map to the same Rust
type. Conditional required rules currently apply to string fields.

### `dive` for slices and maps

For slices, rules before `dive` validate the container length. Rules after
`dive` validate each item.

```go
type BatchReq {
  tags []string `json:"tags" validate:"min=1,dive,required,min=2,alphanum"`
  scores []int `json:"scores" validate:"len=2,dive,gte=1,lte=99"`
}
```

For maps, use `keys` and `endkeys` to split key rules from value rules.

```go
type LabelsReq {
  labels map[string]string `json:"labels" validate:"min=1,dive,keys,min=2,endkeys,required,min=1,alphanum"`
  weights map[string]int `json:"weights" validate:"dive,keys,oneof=gold silver,endkeys,gte=1,lte=10"`
}
```

Supported item/key/value rules are the same basic string and numeric rules used
for scalar request-level checks: `required`, `min`, `max`, `len`, `oneof`,
`alpha`, `alphanum`, `ascii`, `code`, `json`, `numeric`, `lowercase`,
`uppercase`, `nonnegative`, `page`, `limit`, `gte`, `lte`, `gt`, and `lt`.
Container rules before `dive` also accept `min_items` and `max_items`.

## Convention-first project layout

`rozectl` generated projects intentionally use one stable layout so every Rust
service has the same entry points and ownership boundaries.

REST services:

```text
config.yaml
Cargo.toml
src/
  main.rs
  config/mod.rs
  route/
    mod.rs
    <group>.rs
  handler/
    mod.rs
    <group>/
      mod.rs
      <method>.rs
  logic/
    mod.rs
    <group>/
      mod.rs
      <method>.rs
  middleware/
    mod.rs
    <custom>.rs
  openapi/mod.rs
  svc/mod.rs
  types/mod.rs
```

RPC services:

```text
config.yaml
Cargo.toml
build.rs
proto/
  service.proto
src/
  main.rs
  client/mod.rs
  config/mod.rs
  pb/mod.rs
  server/mod.rs
  svc/mod.rs
  types/mod.rs
  logic/
    mod.rs
    <method>.rs
```

Generated RPC servers register the standard `grpc.health.v1.Health` service.
The overall server name and generated protobuf service name are driven by the
same `HealthRegistry` used by framework readiness: starting, failed dependency,
and draining states report `NOT_SERVING`; ready reports `SERVING`. A generated
`grpc-health-sync` lifecycle task refreshes status every second and publishes
`NOT_SERVING` before shutdown.

Framework-owned files can be regenerated with `--update`. Business code should
live in REST `src/logic/<group>/<method>.rs` or RPC
`src/logic/<method>.rs`. Service config extensions live in
`src/config/mod.rs`, REST custom handler adapter code lives in
`src/handler/<group>/<method>.rs`, and REST custom middleware lives in
`src/middleware/<custom>.rs`. Service-wide REST layers belong in
`src/middleware/app.rs`. These application-owned files and `config.yaml`
are preserved on `--update`, while generated boundary files keep route indexes,
HTTP/RPC parsing, validation, context extraction, errors, tracing, and response
contracts consistent across services.

## OpenAPI output

`rozectl openapi generate` and generated REST services expose OpenAPI data with:

- component schemas for `.api` types
- route parameters for path/query/header/form fields
- JSON and form request bodies
- response schemas
- route tags
- bearer security when JWT is declared
- array schemas for `[]T` / `Vec<T>`
- object schemas for map fields

The generated service also serves the document at `/openapi.json`, including
route-scoped prefixes.

## Service document generation

Generate an AI- and team-readable service summary from a `.api` contract:

```bash
rozectl doc service --api user.api --out SERVICE.md
rozectl doc ai-context --api user.api --out AI_CONTEXT.md
```

The generated `SERVICE.md` includes the service name, REST/RPC surface,
generated-file ownership rules, common generation/diff commands, and AI editing
notes. Existing files are not overwritten unless `--force` is passed.
`AI_CONTEXT.md` is a shorter agent handoff document focused on ownership
boundaries, safe edit areas, generated files, and the regenerate workflow.

## RPC proto generation

`rozectl rpc protoc` accepts proto3 source directly. It parses package,
message, field, service, and `rpc` method declarations, then generates a
Rust-native tonic service project:

```text
Cargo.toml
build.rs
config.yaml
proto/service.proto
src/lib.rs
src/main.rs
src/config/mod.rs
src/pb/mod.rs
src/server/mod.rs
src/client/mod.rs
src/logic/<method>.rs
src/svc/mod.rs
```

`proto/service.proto` is the normalized build input used by the generated Rust
project.

When `src/model` already exists, `rpc generate --update` and
`rpc protoc --update` restore the generated `mod model;` declaration after
refreshing `src/main.rs`. RPC and model generation are therefore composable:
either command can be updated independently without requiring a compensating
model regeneration pass.

The proto parser supports line and block comments, multi-line `rpc`
signatures, qualified type names, `stream` request/response markers,
`optional`/`required` labels, `repeated` fields, and `map<K,V>` fields. The
generated normalized proto keeps `repeated` and `map` field shapes.

## Model generation

The model generator uses Roze-native `generate` and `inspect` commands.

SQL DDL:

```bash
rozectl model generate example/user.sql --out services/user-api --format sql
rozectl model generate example/user.sql --out services/user-api --format sql --orm sea-orm
rozectl model mysql ddl --src example/user.sql --dir services/user-api
rozectl model pg ddl --src example/user.sql --dir services/user-api
```

Entity schema:

```bash
rozectl model generate model/schema.ent --out services/user-api --format ent
rozectl model generate model/schema.ent --out services/user-api --format ent --orm sea-orm
```

Database inspection:

```bash
rozectl model inspect users --db-kind mysql --db-url mysql://root:root@127.0.0.1:3306/roze --out services/user-api
rozectl model inspect users --db-kind postgres --db-url postgres://postgres:postgres@127.0.0.1:5432/roze --schema public --out services/user-api
rozectl model inspect users --db-kind mysql --db-url mysql://root:root@127.0.0.1:3306/roze --out services/user-api --orm sea-orm
rozectl model inspect users --db-kind mongo --db-url mongodb://127.0.0.1:27017/roze --sample-size 100 --out services/user-api
rozectl model mysql datasource --url mysql://root:root@127.0.0.1:3306/roze --table users --dir services/user-api
rozectl model pg datasource --url postgres://postgres:postgres@127.0.0.1:5432/roze --table users --schema public --dir services/user-api
rozectl model mongo --collection users --db-url mongodb://127.0.0.1:27017/roze --dir services/user-api
```

Toasty is the default SQL ORM. `--orm sea-orm` switches model output to
SeaORM-style modules. Model generation first writes a complete
`src/model/schema.ent` file, then generates Rust model code from that `.ent`
schema. SQL, DSL, and database inspection are import paths into `.ent`; `.ent`
is the model codegen source.
Entity-level directives accept both lowercase `.ent` form and builder-style
calls such as `Table("users")`, `Schema("public")`, `Cache()`,
`CacheKey("id", "email")`, `Tenant("tenant_id")`, and
`SoftDelete("deleted")`; round-trip rendering normalizes them to lowercase
directives.
Entity-level ent metadata directives such as `Annotations(...)`, `Mixin(...)`,
`Policy(...)`, `Hooks(...)`, and `Interceptors(...)` are accepted as
parse-compatible no-ops and omitted from Roze round-trip rendering.
Fields can be declared with either `field name: type { ... }` or ent-style
builder headers such as `field String("email") { ... }`,
`field Int64("id") { ... }`, and `field fields.String("nickname") { ... }`;
field headers can also chain directives such as
`field String("email").Unique().NotEmpty() { ... }`. Round-trip rendering
normalizes them to `field name: type`.
Builder headers may include entgo's extra Go type sample argument, such as
`field JSON("metadata", map[string]any{})`,
`field JSON("metadata", map[string]interface{}{})`, or
`field UUID("public_id", uuid.UUID{})`; Roze uses the first argument as the
field name. JSON maps to `serde_json::Value` in generated SeaORM models, while
UUID remains string-backed. Ent `field Any("payload")` is accepted as the same
structured JSON type, including optional fields and static map/slice defaults.
Toasty 0.7 does not expose a JSON storage field, so Toasty output uses a
JSON-string compatibility representation while normalized `schema.ent` keeps
the field type as `json`.
Go map defaults such as `Default(map[string]any{})` and simple static literals
like `Default(map[string]any{"theme": "dark", "beta": true})` or
`Default(map[string]interface{}{"theme": "dark"})` normalize to JSON objects
for generated create builders.
Simple Go slice defaults such as `Default([]string{"new", "hot"})` and
`Default([]interface{}{true, 3, "ok", nil})` normalize to JSON arrays. Simple
custom Go slice defaults such as
`Default([]http.Dir{"/tmp"})` normalize the literal items the same way.
JSON defaults are recursive: nested typed maps, typed slices, elided inner
composite literals such as `[][]int{{1, 2}, {3, 4}}`, and `nil` values
normalize to nested JSON objects, arrays, and `null`.
Custom entgo `Other(...)` field builders are accepted with the same first-arg
name parsing and map to string-backed fields so generated repositories stay
compilable; keep domain-specific typed conversion in application extensions.
Common ent field builders map to Roze model types, including `Text(...)` to
`string`, `Int8(...)` to `i8`, `Int16(...)` to `i16`,
`Int(...)`/`Int32(...)` to `i32`, `Int64(...)` to `i64`,
`Uint(...)`/`Uint32(...)` to `u32`, `Uint8(...)` to `u8`,
`Uint16(...)` to `u16`, `Uint64(...)` to `u64`, `Float(...)`/`Float64(...)`
to `f64`, `Float32(...)` to `f32`, and `Bytes(...)` to `bytes`/`Vec<u8>`.
`Bytes(...)` fields also accept explicit Go byte-slice defaults such as
`Default([]byte{1, 2, 255})` and `Default([]byte("seed"))`, which generate
`Vec<u8>` create defaults.
Network-oriented builders such as `IP(...)`, `MAC(...)`, and `URL(...)` map to
string-backed fields; `IPs(...)` maps to `Vec<String>` and supports the same Go
slice defaults as other plural string builders.
Plural scalar builders map to Rust vectors, including `Strings(...)` to
`Vec<String>`, `Int8s(...)` to `Vec<i8>`, `Int16s(...)` to `Vec<i16>`,
`Ints(...)`/`Int32s(...)` to `Vec<i32>`, `Int64s(...)` to `Vec<i64>`,
`Uint8s(...)` to `Vec<u8>`, `Uint16s(...)` to `Vec<u16>`,
`Uints(...)`/`Uint32s(...)` to `Vec<u32>`, `Uint64s(...)` to `Vec<u64>`,
`Floats(...)`/`Float64s(...)` to `Vec<f64>`, `Float32s(...)` to `Vec<f32>`,
and `Bools(...)` to `Vec<bool>`; round-trip `.ent` rendering writes these as
`[]string`, `[]i32`, and similar array types. Because Roze treats
`Vec<u8>` as bytes, `Uint8s(...)` round-trips as `bytes`.
Go slice defaults for those plural scalar builders, such as
`Default([]string{"new", "hot"})` or `Default([]int{1, 2})`, are used by
generated Rust create builders as `vec![...]` defaults.
`Duration(...)` maps to an `i64` nanosecond value, matching Go
`time.Duration`; common defaults such as `Default(time.Second)` and
`Default(5 * time.Second)` are normalized to numeric nanoseconds for generated
Rust create builders.
`field ID("id")` maps to an `i64` primary field even when it is not the first
declared field. When an ent-style schema does not declare `primary`
explicitly, Roze prefers a field named `id` over the first declared field,
matching entgo's builtin-id/override convention for declarations such as
`field UUID("id", uuid.UUID{}).Default(uuid.New).StorageKey("oid")` or
`field String("id").MaxLen(25).NotEmpty().Unique().Immutable()`. Because the
primary key is already unique, a `Unique()` directive on the primary `id` field
does not generate a redundant `uniq_id` index.
Composite entgo IDs declared with schema annotations such as
`Annotations(field.ID("user_id", "tweet_id"))` are parsed, validated, and
preserved by canonical schema rendering, including SeaORM/Toasty key metadata.
Generated model modules define a typed `{Model}Key`; SeaORM key lookup/delete
uses its native primary-key tuple, while Toasty combines equality predicates
for every key field. The typed surface includes `find_by_key`, `update_by_key`,
`delete_by_key`, and `delete_many_by_keys`; update replaces every model key
field from the supplied key before persistence. Composite `update_one` and
`delete_one` builders also accept `{Model}Key`; SeaORM ActiveModel updates set
all key columns, and Toasty resolves the row with all key predicates.
Project generation supports composite-key create, lookup, update, delete, and
batch update/delete paths without falling back to unsafe first-column
operations. Legacy single-key convenience methods are omitted for composite
models in favor of the typed key surface.
`.ent` schemas can declare entity relationships with edge blocks:

```text
entity Order {
  table "orders"

  field id: i64 {
    primary
    auto_increment
  }

  field user_id: i64 {}

  field code: string {
    unique
  }

  field created_at: i64 {
    immutable
  }

  edge user {
    to User
    field user_id
    ref id
    unique
    required
  }
}
```

SQL `FOREIGN KEY (...) REFERENCES ...` constraints and inline `REFERENCES`
column attributes are imported as `.ent` edges. Single-column foreign keys are
supported; composite foreign keys are rejected with a clear error until the
relation generator grows composite edge semantics.
`.ent` edge `Unique()` follows Ent relationship cardinality: an edge bound to a
scalar local FK with `Field(...)` must be unique because each source row points
to at most one target. It does not make the FK column database-unique and does
not add cache-key or unique-lookup helpers. Declare `Unique()` on the field, or
use `index.Edges("edge").Unique()`, when the database column itself must be
unique. SQL foreign-key imports use unique edge cardinality while preserving a
database unique index only when it exists in the source DDL.
Edge blocks also accept ent-style builder calls such as `To("User")`,
`Field("user_id")`, `Ref("id")`, `Unique()`, `Required()`, and edge-level
`Immutable()`; round-trip rendering normalizes generated relationship
directives to lowercase `.ent` directives and preserves edge `Comment("...")`
as `comment "..."`. Immutable local-FK edges remain available on create
builders, but their relationship setter, clear method, and underlying FK field
setter are omitted from SeaORM and Toasty update/update-many builders.
An explicit local FK field and its owning edge must agree on Ent's create
optionality and immutability. An `Optional()` field pairs with an optional edge;
a `Required()` edge pairs with a non-nullable field or a `Nillable()` field,
which keeps an `Option<T>` representation while remaining required on create.
`Immutable()` must be declared on both the field and edge. Roze rejects these
metadata mismatches before generating code. Inverse `From(...).Ref(...)` and
`Through(...)` edges reuse the owning edge's storage contract rather than
revalidating a second local field.
For `Nillable()` local FK fields, `Required()` is enforced in generated mutation
builders: create rejects a missing or null relation, while update-one and
update-many allow the relation to remain untouched but reject explicitly
clearing it to null.
Edge headers can also use chained ent-style syntax such as
`edge To("user", User.Type).Field("user_id").Ref("id").Unique().Required()`;
round-trip rendering normalizes that form to `edge user { ... }`.
When a local-FK edge declares `StorageKey(edge.Column("post_author"))`, Roze
maps that column name onto the local field's `source`/SeaORM `column_name`
metadata. If a field-level `StorageKey(...)` declares a different column, model
generation fails fast with a conflict error. Ent many-to-many storage metadata
such as `StorageKey(edge.Table("memberships"), edge.Columns("user_id",
"group_id"))` is preserved as ordered edge metadata for
no-local-FK/`Through(...)` edges and survives canonical schema round trips.
On a concrete local-FK edge, Roze maps the listed local fields by position to
the target composite primary-key fields. Local and target names may differ.
Roze validates field presence, arity, order, and types, then generates
multi-predicate relationship queries and setters that write every FK component.
Inverse `From(...).Ref(...)` traversal reuses the owning mapping and filters the
target repository by every component.
Inverse ent-style edges such as `edge From("profile", Profile.Type).Ref("user")`
resolve their named owning edge after all entities are parsed. Roze preserves
them during canonical round-trip and generates reverse `query_<edge>` methods:
inverse `Unique()` edges return an optional target, while non-unique inverse
edges return a target list. `where_<edge>_with(...)` also works in the inverse
direction by projecting the remote owning FK, dropping null FK values, and
filtering the source ref field with a typed `IN` predicate. Inverse edges do not
generate local FK setters, indexes, or cache keys. If an inverse edge declares
a local field, such as `edge From("author", User.Type).Field("author_id").Unique()`,
Roze treats it as a concrete local-FK relationship and generates the same
repository helpers as an owning `To(...)` edge.
Many-to-many ent-style edges such as
`edge To("groups", Group.Type).Through("memberships", Membership.Type)` resolve
an explicit join entity when it has exactly one owning local-FK edge to the
source model and one to the target model. Generated `query_<edge>` methods
traverse through the join repository, and `where_<edge>_with(...)` projects
matching target keys through the join model before filtering source rows.
Both join-model owning edges that compose the relationship must declare
`Unique().Required()`: `Unique()` expresses their scalar to-one cardinality,
and `Required()` prevents incomplete edge-schema rows with a missing endpoint.
Roze reports the source or target join edge before generating code when this
contract is violated.
A single-field unique index on the join model's source FK narrows that Through
direction to to-one cardinality. Roze infers this after resolving owning or
inverse join directions, so generated `query_<edge>` returns `Option<T>` and
uses `first()` instead of returning a list. Compound unique indexes do not
trigger this inference because they still permit multiple rows for one source.
Inverse declarations such as
`edge From("liked_users", User.Type).Ref("liked_tweets").Through("likes", Like.Type)`
reuse the owning Through edge, reverse the join fields, survive canonical
round-trip rendering, and generate traversal and relation filters from the
opposite endpoint.
Self-referential edge schemas are also supported, matching Ent's friendship
pattern: the join entity must declare exactly two owning local-FK edges to the
same model, with the first edge treated as the source direction and the second
as the target direction.
The join entity remains a normal generated model, so additional edge fields are
available through its own CRUD API. Both SeaORM and Toasty endpoint models also
generate `add_<edge>`, `remove_<edge>`, and `clear_<edge>` methods backed by the
join repository. A self-referential join entity with any number of matching
owning edges other than two fails validation.
Matching Ent's edge-schema ownership rule, one join entity may belong to only
one owning Through relationship. Its resolved inverse Through endpoint is part
of the same relationship and does not count as a second owner; reuse by another
owning edge fails with both conflicting edge names.
Plain ent-style `edge To("groups", Group.Type)` declarations without local
`Field(...)`/`Ref(...)` are treated the same way; if either `Field(...)` or
`Ref(...)` is present, both must be present so Roze can generate a concrete
local-FK relationship.
Chained self O2O declarations such as
`edge To("next", Node.Type).Unique().From("prev").Unique()` generate an
implicit owning FK plus the inverse query edge. Canonical output may expand the
chain into equivalent owning `To` and inverse `From(...).Ref(...)` declarations.
Chained self O2M declarations such as
`edge To("children", Node.Type).From("parent").Unique()` place an implicit
`parent_id` FK on the single-value `parent` direction and generate a to-many
`children` inverse query. Cross-entity fieldless O2M pairs such as
`User.To("pets", Pet.Type)` with
`Pet.From("owner", User.Type).Ref("pets").Unique()` similarly synthesize
`owner_id` on the target entity. Paired fieldless M2M edges synthesize a
managed join model with endpoint fields typed from the real primary keys and a
compound unique index. This covers cross-entity `To`/`From(...).Ref(...)`
pairs and chained self-referential `To(...).From(...)` declarations. Use an
explicit `Through(...)` model when the join carries payload fields.
Owning ent-style edges with `Field("user_id")` but no `Ref(...)` default the
reference field to `id`, matching the common entgo convention of pointing to
the target primary key.
`.ent` fields can declare `unique`; Roze normalizes that into a single-field
unique index and generates the same unique lookup helpers as an explicit
`index ... { unique }` block.
`.ent` fields can declare `index`; Roze normalizes that into a single-field
non-unique index and generates `list_by_<field>` helpers for Toasty and SeaORM
repositories.
Index blocks accept ent-style builder calls such as `Fields("tenant_id",
"code")` and `Unique()`; round-trip rendering normalizes them to lowercase
`.ent` directives.
Roze accepts ent/Atlas index annotation helpers such as `Prefix(...)`,
`PrefixColumn(...)`, `Desc()`, `DescColumns(...)`, `IndexType(...)`,
`IndexTypes(...)`, `IncludeColumns(...)`, `IndexWhere(...)`, and
`OpClass(...)` as parse-compatible metadata no-ops, matching entgo schemas that
carry dialect-specific index hints.
Index headers can also use chained ent-style syntax such as
`index Fields("tenant_id", "code").Unique().StorageKey("tenant_code")`;
`StorageKey(...)` becomes the Roze index name, and unnamed indexes are
assigned stable `idx_<field>` or `uniq_<field>` names during normalization.
Repeated `Fields(...)` calls append fields in builder order, so
`Fields("tenant_id").Fields("region_id").Edges("user")` normalizes to the
local field list `tenant_id, region_id, user_id`.
Index builders with `Edges("user")` map to the owning edge's local FK field
when that edge declares one, such as `user_id`; unresolved edge-only indexes
are accepted as parse-compatible no-ops because Roze cannot generate a concrete
database index without a local field.
An owning `edge.To("group", Group.Type).Unique()` without `Field(...)`
generates an implicit `group_id` storage field typed from the target primary
key. It is nullable by default; `Required()` makes it required, `Immutable()`
makes the storage field create-only, and `StorageKey(edge.Column("..."))`
controls the physical column. Canonical `.ent` output keeps the implicit edge
form and does not expose the synthesized field. Non-unique fieldless to-many
edges are retained when paired as M2M and normalize to a generated
`Through(...)` join model; unmatched declarations are ignored.
`.ent` fields can declare `source <column>` or `storage_key <column>` when the
logical schema field name differs from the physical database column; SeaORM
models emit a matching field-level `column_name` attribute.
Field directives also accept ent-style builder calls such as `Optional()`,
`Nillable()`, `Unique()`, `Sensitive()`, `StorageKey("column")`,
`MinLen(3)`, and `ClientDefault("value")`; Roze normalizes them back to the
lowercase schema form during round-trip rendering.
`Optional()` makes a field nullable and optional during create. `Nillable()`
alone keeps Ent's distinct contract: the generated model uses `Option<T>`, but
create requires a non-null value unless the field has a default, explicit null
is rejected by create and update builders, and no `clear_*` method is emitted.
Roze preserves this distinction in canonical `.ent` output with
`required_on_create`. Combining `Optional().Nillable()` keeps the field
optional and allows explicit null.
Roze applies Ent-compatible field invariants before writing generated code:
primary/ID fields cannot be optional, fields with a static `Default(...)`
cannot also be `Unique()`, and `Sensitive()` fields cannot carry JSON
`StructTag(...)` metadata. Function defaults such as
`DefaultFunc(uuid.NewString)` remain valid on unique fields.
Enum field builders such as `field Enum("state").Values("active", "disabled")`
map to a Roze `string` field with generated enum-value validation.
`NamedValues("Active", "active", "Disabled", "disabled")` is also accepted and
normalizes to the stored values `active` and `disabled`.
Timestamp defaults accept ent-style `Default(time.Now)`,
`DefaultFunc(time.Now)`, `UpdateDefault(time.Now)`, and
`ClientDefault(time.Now)` and normalize them to Roze's numeric `now_millis`
timestamp default for `i64`/`u64` timestamp fields. Ent-style timestamp
closures in `Default(...)`, `DefaultFunc(...)`, `UpdateDefault(...)`, or
`ClientDefault(...)`, such as `func() int64 { return time.Now().Unix() }`,
`UnixMilli()`, `UnixMicro()`, and `UnixNano()`, normalize to `now_secs`,
`now_millis`, `now_micros`, and `now_nanos` respectively; the same mappings
apply to `time.Now().UTC().Unix*()` chains. Closures returning `time.Now()` or
`time.Now().UTC()` for `Time(...)` fields normalize to `now_millis`.
UUID defaults accept ent-style `DefaultFunc(uuid.NewString)`,
`DefaultFunc(uuid.New)`, and `DefaultFunc(uuid.NewV7)` for
`UUID(...)`/string-backed fields. UUID closures such as
`func() string { return uuid.NewString() }`, `uuid.New().String()`, and
`uuid.NewV7().String()` in `Default(...)`, `DefaultFunc(...)`, or
`ClientDefault(...)` normalize to the same `uuid_new_string` default; generated
Rust create builders use `uuid::Uuid::now_v7().to_string()` and model
generation adds the `uuid` dependency when needed.
The ent-style `field Time("created_at").Default(time.Now)` builder maps to a
Roze `i64` epoch-millis timestamp field and uses the same numeric timestamp
default generation.
Literal defaults such as `Default(true)`, `Default(18)`, and
`Default("member")` are used by generated create builders for bool, primitive
numeric, string, and nullable variants of those fields.
Roze also accepts common ent metadata directives on fields, edges, and indexes,
such as `SchemaType(...)`, `GoType(...)`,
`ValueScanner(...)`, `DefaultExpr(...)`, `DefaultExprs(...)`,
`Collation(...)`, `Charset(...)`, `Annotations(...)`, edge
`StorageKey(...)`, and index `Where(...)`/Atlas index annotation helpers, as
parse-compatible no-ops;
round-trip rendering keeps the
normalized Roze schema and omits those metadata-only directives. Partial index
conditions, database-side default expressions, collation/charset hints, and
index prefix/type/order/include/operator-class hints are accepted for entgo
input compatibility, but Roze does not emit partial-index, default-expression,
collation/charset, or dialect-specific index DDL from them yet.
Ent `Deprecated()` and `Deprecated("reason")` field metadata is preserved in
canonical `.ent` schemas as `deprecated` or `deprecated "reason"`. Generated
SeaORM and Toasty model fields receive Rust `#[deprecated]` attributes, including
the migration reason when provided. Complete ORM entities still hydrate the
column so existing data remains readable; unlike Ent's generated query selector,
Roze does not omit deprecated columns from full-model reads.
Ent `StructTag(...)` JSON metadata is preserved when it has a static Go tag:
`json:"name"` generates `serde(rename)`, `omitempty` generates
`serde(skip_serializing_if = "is_default")`, and `json:"-"` skips the field.
Other struct-tag keys remain parse-compatible metadata and are omitted.
Custom Go-side `Validate(...)` functions and dynamic `Match(...)` expressions
are accepted as parse-compatible no-ops. Static
`Match(regexp.MustCompile("..."))` expressions are validated during generation,
normalized to `match "..."`, and enforced with Rust `regex` in generated
mutation builders; the `regex` dependency is added only when needed. Other
supported validators include `NotEmpty()`, `MinLen(...)`, `MaxLen(...)`,
`MinRuneLen(...)`, `MaxRuneLen(...)`, `Enum(...)`, and numeric bounds when
generated Rust validation is required. `NotEmpty()` rejects zero-length strings
but permits whitespace-only strings, matching Ent's length-based behavior.
Ent `MinLen(...)`/`MaxLen(...)` use
UTF-8 byte length, while `MinRuneLen(...)`/`MaxRuneLen(...)` use Unicode
character count. They normalize to `min_byte_len`/`max_byte_len` and
`min_len`/`max_len` respectively so round-trip rendering preserves semantics.
The common ent
pattern `Validate(MaxRuneCount(n))` is recognized and normalized to Roze
`max_len n`, using Rust character-count validation in generated builders.
`.ent` fields can declare `immutable`; Roze keeps them available on create
builders but omits them from update, update-many, and edge update setters.
`.ent` fields can declare `optional`; Roze treats that as the nullable
equivalent of writing the field type with `?`, such as `string?`.
`.ent` fields can declare `sensitive`; Roze omits `Debug` derives for those
model rows and generates a manual `Debug` implementation that renders the
field as `<sensitive>`. Generated SeaORM and Toasty model fields also use
`#[serde(skip)]`, preventing sensitive values from appearing in serialized
output or being populated through model deserialization.
`.ent` string fields can declare `not_empty`, `min_len <n>`, `max_len <n>`,
`enum <value>, <value>`, `contains <value>`, `starts_with <value>`, and
`ends_with <value>`, plus `not_contains <value>`, `not_starts_with <value>`,
and `not_ends_with <value>`. String and bytes fields can also use explicit
`min_byte_len <n>` and `max_byte_len <n>` constraints. Entgo's `Size(n)`
builder is accepted as a `MaxLen(n)`/`max_byte_len n` alias. Roze
validates those constraints in generated create, update-one, and update-many
builders before writing through Toasty or SeaORM.
`.ent` primitive numeric fields can declare `positive`, `non_negative`,
`negative`, `non_positive`, `min <n>`, `max <n>`, and
`range <min>, <max>`; Roze validates those bounds in the same generated
mutation builders.
`.ent` `i64`/`u64` timestamp fields can declare `default now_secs`,
`default now_millis`, `default now_micros`, or `default now_nanos`; generated
create builders fill the field when the caller did not set it explicitly.
`.ent` fields can declare `client_default <value>` for generated create builder
defaults on `String`, `bool`, primitive numeric fields, and `now_*` timestamp
values; unlike `default`, this is applied by generated Rust builders rather
than relying on a database default.
`.ent` updateable `i64`/`u64` timestamp fields can declare
`update_default now_secs`, `update_default now_millis`,
`update_default now_micros`, or `update_default now_nanos`; generated update
builders fill the field when the caller did not set it explicitly.
SQL `JSON`/`JSONB` columns normalize to `.ent` `json`; PostgreSQL static defaults
such as `'{}'::jsonb` and `'[]'::json` become structured JSON defaults. SeaORM
output uses `serde_json::Value`, while Toasty uses its JSON-string compatibility
representation. Ordinary SQL `INT`/`INTEGER` columns generate `i32`, while
PostgreSQL `BIGINT`/`BIGSERIAL`/`INT8` columns generate signed `i64` fields.
MySQL `BIGINT` also generates `i64`; only an explicit `BIGINT UNSIGNED`
generates `u64`.
PostgreSQL `TIMESTAMP` and `TIMESTAMPTZ` preserve their distinction as `.ent`
`timestamp` and `timestamptz`. SeaORM output uses `DateTime` and `DateTimeUtc`
respectively, including their nullable forms, and model generation adds the
`with-chrono` SeaORM feature plus `clock` and `serde` to the `chrono` dependency
when needed, merging the features into an existing dependency if present.
`.ent` `datetime` is accepted as an alias for `timestamp`.
Model names and field names must generate valid, non-conflicting Rust module,
type, field, and field-enum identifiers. Names that normalize to a single `_`
are rejected.
Mongo inspection samples collection documents, maps `_id` to `id`, and emits
Mongo repository modules. It preserves Mongo index metadata, emits find helpers
for single-field unique indexes, emits compound-index find/list helpers, and
still emits an `id: ObjectId` model for empty collections. `--orm` does not
affect Mongo output.

Generated SQL repositories include single-table CRUD helpers. Toasty and SeaORM
outputs both generate primary-key lookup, cache-key lookup, `list`, `insert`,
`upsert`, `update`, `delete_by_<primary>`, and `count` methods. SeaORM `upsert`
uses a database `ON CONFLICT` statement over every primary-key column and is
atomic. Toasty 0.7 does not expose an equivalent conflict API, so its generated
compatibility method queries by every primary-key field and then inserts or
updates; callers that require concurrent atomicity must wrap the operation in
an appropriate transaction or use a database-specific implementation.

SQL repositories additionally generate:

- `<model>_fields.rs` files with table-name constants, `{Model}Field` enums,
  and field-name constants separated from repository logic
- `<model>_ext.rs` application-owned extension files for custom model or
  repository methods; these files are created once and preserved during
  `--update`
- `schema.ent`, the generated or user-authored entity schema used as the model
  codegen source
- `{Model}Predicate`, `{Model}Order`, `{Model}Query`, `{Model}Create`,
  `{Model}Update`, `{Model}Delete`, and `{Model}Page` types for ent-style
  single-table queries and mutations
- predicate helpers such as `name_contains`, `name_not_contains`,
  `name_icontains`, `name_not_icontains`, `name_equal_fold`,
  `name_not_equal_fold`, `name_not_starts_with`, `name_not_ends_with`,
  `id_in`, `status_between`, `nickname_is_null`, `and`, `or`, and `not`
- public `escape_like_pattern` and `contains_like_pattern` helpers for custom
  repository filters built with the same wildcard escaping as generated
  `contains` and `icontains` predicates
- `*_icontains` predicates render database-level `ILIKE` on supported SQL
  backends instead of applying keyword filtering after pagination
- `*_equal_fold` predicates provide ent-style case-insensitive equality with
  the same LIKE pattern escaping and no wildcard expansion
- `*_not_contains`, `*_not_icontains`, `*_not_equal_fold`,
  `*_not_starts_with`, and `*_not_ends_with` predicates generate negated
  LIKE/ILIKE filters before count and pagination
- nullable fields also get non-null value predicates such as `nickname_in`,
  `nickname_not_in`, and nullable numeric `gt/gte/lt/lte/between`
- order helpers are generated for every queryable field, including nullable
  fields such as `nickname_asc` and `nickname_desc`
- query builders with `where_`, `where_all`, `where_any`, `where_not`,
  `where_none`, `order`, `order_all`, `order_by_<field>_asc`,
  `order_by_<field>_desc`, `limit`, `offset`, `paginate`, `all`, `count`,
  `exists`, `ids`, `first_id`, `only_id`, `pluck_<field>`,
  `unique_<field>`, `count_by_<field>`, `first_<field>`, `only_<field>`,
  `sum_<field>`, `avg_<field>`, `min_<field>`, `max_<field>`, `first`,
  `only`, and `page`
- Ent-style grouped aggregates are generated in schema declaration order for
  at most eight grouping-field/numeric-value pairs per model as
  `sum_<value>_by_<group>()`,
  `avg_<value>_by_<group>()`, `min_<value>_by_<group>()`, and
  `max_<value>_by_<group>()`; they preserve the source query's predicates,
  ordering, pagination, and soft-delete scope and return typed tuples. Average,
  minimum, and maximum values are optional so groups containing only null
  values remain visible. This fixed budget keeps generated modules linear in
  schema size; use `into_select()` or `into_query()` for additional
  application-owned aggregate combinations
- SeaORM `count_by_<field>()`, `sum_<value>_by_<group>()`,
  `avg_<value>_by_<group>()`, `min_<value>_by_<group>()`, and
  `max_<value>_by_<group>()` execute as database `GROUP BY` queries and do not
  hydrate matching model rows. Toasty's grouped helpers remain on the
  compatibility path until its generated predicate compiler is connected to
  typed raw-SQL grouping
- SeaORM also generates `count_by_<field>_having_at_least`,
  `count_by_<field>_having_at_most`, and
  `count_by_<field>_having_between` using SQL `HAVING COUNT(...)`, plus the
  corresponding `sum_<value>_by_<group>_having_*` range helpers using SQL
  `HAVING SUM(...)`; at most eight pairwise
  `count_by_<left>_and_<right>()` helpers are emitted in schema declaration
  order using a two-column `GROUP BY`
- typed queries expose backend escape hatches for application-owned custom
  projections and aggregate scans: SeaORM `into_select()` and Toasty
  `into_query()`. Both preserve generated predicates, soft-delete scope,
  ordering, limit, and offset before returning the native query object
- create and `--update` generation run rustfmt only over framework-owned Rust
  files. Model extension files, logic, config, and custom middleware remain
  untouched. When generated model registries or managed RPC-client sections
  update `src/svc/mod.rs`, rozectl formats that mixed-ownership file while
  preserving application-owned declarations. Rustfmt child-module traversal
  is disabled so formatting cost is bounded by the explicitly touched files
- update-many and delete-many mutation builders also support the same
  `where_all`, `where_any`, `where_not`, and `where_none` predicate groups
- filtered SeaORM queries over numeric fields, including nullable numeric
  columns, expose atomic
  `add_<field>(delta)` and `subtract_<field>(delta)` mutations. SeaORM emits a
  column expression and returns the affected-row count; SQL null propagation
  means a null value remains null. Toasty uses its typed
  `stmt::add`/`stmt::subtract` assignments for supported non-null numeric
  fields. Toasty 0.7 does not implement its `Numeric` assignment trait for
  `Option<T>` or `rust_decimal::Decimal`, so generated Toasty models use the
  parameterized nullable SQL path where applicable and omit Decimal atomic
  helpers.
  Supported methods execute arithmetic in the database, so callers can target
  one row with a primary-key predicate or many rows with any generated
  predicate group
- update-many builders expose the same atomic operations as terminal methods,
  for example `update_many().where_(active_eq(true)).add_score(1).await`; these
  delegate to the filtered query mutation, including soft-delete scope and
  SeaORM cache invalidation, without hydrating rows
- single-primary-key update-one builders expose terminal atomic operations that
  return the reloaded model, for example `update_one(id).add_score(1).await`.
  Mixing pending `set_*`/`clear_*` changes with an atomic terminal operation is
  rejected explicitly so no mutation is silently discarded
- composite-primary-key update-one builders expose the same terminal atomic
  operations. Every key component is applied as an equality predicate and the
  updated row is reloaded through the generated typed key
- entity relation methods for `.ent` edges, such as
  `order.query_user(&ctx.model().user()).await?` on SeaORM and
  `order.query_user(&mut db).await?` on Toasty; nullable foreign-key edges
  return `Ok(None)` when the local edge field is `None`
- explicit relation-loading result types such as `OrderWithUser`, plus
  `all_with_user` and `first_with_user` query methods. These methods preserve
  the source query's predicates, ordering, pagination, and soft-delete scope,
  and support owning, inverse, composite-key, and through edges. The current
  implementation batches ordinary single-column owning and inverse edges into
  one source query plus one target `IN` query. Composite-key and through edges
  currently resolve each returned node through the normal generated edge query
- SeaORM and Toasty generate pairwise multi-edge loaders for ordinary single-column
  edges, such as `all_with_user_and_profile`. They return a typed
  `OrderWithUserAndProfile`, execute the source query once, and add one batched
  target `IN` query per requested edge. Through and composite-key edge pairs
  stay on their explicit loaders until they can meet the same bounded-query
  contract. Matching `first_with_<edge1>_and_<edge2>` helpers apply a source
  limit of one and reuse the same bounded loader
- two-level ordinary single-column paths generate nested loaders such as
  `all_with_profile_then_avatar`. The returned root wrapper contains the
  target model's typed `ProfileWithAvatar` wrapper, preserving target-to-edge
  association. SeaORM and Toasty execute exactly one query for roots, one for
  first-level targets, and one for nested targets
- single Through edge loaders execute one root query, one join-model query and
  one target query on SeaORM and Toasty. Composite-key edge loaders build an
  OR-of-AND typed predicate set and execute one root plus one target query;
  neither path performs per-root traversal queries
- relation-filter query methods provide Ent-style `HasXWith` behavior through
  `where_<edge>_with(...)`: target predicates are combined with AND, projected
  to the configured ref key, and applied to the source query as a typed local
  foreign-key `IN` predicate before ordering, counting, or pagination
- every generated ordinary, inverse, composite-key, and Through relationship
  also exposes an Ent-style `HasX` query method named `has_<edge>(...)`. It
  selects source rows that resolve to at least one target row and composes with
  the query's existing predicates, ordering, pagination, and soft-delete scope
- the same relationship set exposes `not_has_<edge>(...)` and
  `where_<edge>_without(...)`, matching Ent's `Not(HasX())` and
  `Not(HasXWith(...))` semantics. Nullable owning foreign keys are included in
  the negative result instead of being lost to SQL `NOT IN` null semantics
- nullable foreign-key edges also get `has_<edge>()` and `not_has_<edge>()`
  predicate helpers backed by the local edge field
- create and update builders also get ent-style edge setters such as
  `.set_user(&user)`, which assigns the configured local foreign-key field from
  the target edge ref field
- nullable edges also get clear helpers such as `.clear_user()`, which sets the
  configured local foreign-key field to `NULL`
- create, update, and update-many builders get `clear_<field>()` helpers for
  nullable fields, for example `.clear_nickname()`
- service projects with `src/svc/mod.rs` also get `src/model/client.rs`,
  `ModelClient`, and `ServiceContext::model()` as the ent-style model entry
  point
- Toasty and SeaORM query generation count with the filter-only query and apply
  `ORDER BY`, `LIMIT`, and `OFFSET` only to the list query
- composite-index helpers such as `find_by_tenant_id_and_name` for unique
  indexes and `list_by_status_and_created_at` for non-unique indexes
- `insert_many` and `delete_many_by_ids` batch helpers; Toasty uses safe
  per-row repository calls, SeaORM uses set-based ORM calls
- soft-delete scopes and `soft_delete_by_<primary>` helpers when a soft-delete
  column is configured or inferred
- tenant-scoped `find_by_<primary>_for_<tenant>` helpers when a tenant column is
  configured or inferred
- local transaction helpers for Toasty and SeaORM; Toasty repository methods
  accept `&mut dyn toasty::Executor`, so the same CRUD helpers can run against
  a `toasty::Db` or `toasty::Transaction`, while SeaORM passes a
  `&DatabaseTransaction` to the callback

SQL and inspect imports infer soft-delete columns from `deleted`, `is_deleted`,
`deleted_at`, `delete_time`, or `deleted_at_millis`, and tenant columns from
`tenant_id`, `org_id`, or `account_id`, then write those decisions into
`schema.ent`. `.ent` schemas can declare this with `soft_delete <field>` and
`tenant <field>`.

SeaORM service code can enter model queries through `ServiceContext::model()`:

```rust
let page = ctx
    .model()
    .user()
    .query()
    .where_(user::name_contains(keyword))
    .order(user::id_desc())
    .paginate(page, page_size)
    .page()
    .await?;

let ids = ctx.model().user().query().ids().await?;
let newest = ctx.model().user().query().order_by_id_desc().limit(10).all().await?;
let names = ctx.model().user().query().pluck_name().await?;
let unique_names = ctx.model().user().query().unique_name().await?;
let name_counts = ctx.model().user().query().count_by_name().await?;
let first_name = ctx.model().user().query().first_name().await?;
let only_name = ctx.model().user().query().where_(user::id(1)).only_name().await?;
let id_sum = ctx.model().user().query().sum_id().await?;
let avg_age = ctx.model().user().query().avg_age().await?;
let min_id = ctx.model().user().query().min_id().await?;
let max_id = ctx.model().user().query().max_id().await?;

let created = ctx
    .model()
    .user()
    .create()
    .set_name("alice".to_string())
    .save()
    .await?;

let updated = ctx
    .model()
    .user()
    .update_one(user_id)
    .set_name("alice-updated".to_string())
    .save()
    .await?;

ctx.model().user().delete_one(user_id).exec().await?;

let updated_many = ctx
    .model()
    .user()
    .update_many()
    .where_(user::name_contains("alice"))
    .set_name("alice-renamed".to_string())
    .save()
    .await?;

let deleted_many = ctx
    .model()
    .user()
    .delete_many()
    .where_(user::name_contains("old"))
    .exec()
    .await?;
```

Toasty service code can use the same model client to access the generated
repository entry and the configured Toasty executor:

```rust
let mut db = ctx.model().toasty_db()?;
let items = UserRepository::query(&mut db)
    .where_(user::name_contains(keyword))
    .all()
    .await?;

let ids = UserRepository::query(&mut db).ids().await?;
let newest = UserRepository::query(&mut db).order_by_id_desc().limit(10).all().await?;
let names = UserRepository::query(&mut db).pluck_name().await?;
let unique_names = UserRepository::query(&mut db).unique_name().await?;
let name_counts = UserRepository::query(&mut db).count_by_name().await?;
let first_name = UserRepository::query(&mut db).first_name().await?;
let only_name = UserRepository::query(&mut db).where_(user::id(1)).only_name().await?;
let id_sum = UserRepository::query(&mut db).sum_id().await?;
let avg_age = UserRepository::query(&mut db).avg_age().await?;
let min_id = UserRepository::query(&mut db).min_id().await?;
let max_id = UserRepository::query(&mut db).max_id().await?;

let created = UserRepository::create(&mut db)
    .set_name("alice".to_string())
    .save()
    .await?;

let updated = UserRepository::update_one(&mut db, user_id)
    .set_name("alice-updated".to_string())
    .save()
    .await?;

UserRepository::delete_one(&mut db, user_id).exec().await?;

let updated_many = UserRepository::update_many(&mut db)
    .where_(user::name_contains("alice"))
    .set_name("alice-renamed".to_string())
    .save()
    .await?;

let deleted_many = UserRepository::delete_many(&mut db)
    .where_(user::name_contains("old"))
    .exec()
    .await?;
```

Toasty transaction callbacks use the generated repository helper and can call
the same generated CRUD methods with the transaction executor:

```rust
UserRepository::transaction(&mut db, |tx| {
    Box::pin(async move {
        let user = UserRepository::find_by_id(tx, &1).await?;
        Ok(user)
    })
})
.await?;
```

SeaORM transaction callbacks follow SeaORM's boxed-future transaction shape:

```rust
repo.transaction(|tx| {
    Box::pin(async move {
        // call SeaORM operations with `tx`
        Ok(())
    })
})
.await?;
```

Model generation treats `schema.ent`, `mod.rs`, `<model>.rs`, and
`<model>_fields.rs` as schema-owned generated files. Re-running with `--update`
refreshes them from the current `.ent` schema or imported SQL/DSL/inspected
schema. Put handwritten model helpers and custom repository queries in
`<model>_ext.rs`; `--update` preserves existing extension files, while
`--force` rewrites them. During `--update`, rozectl also removes stale generated
model files that carry the `@generated by rozectl` marker and no longer
correspond to the current schema. Unmarked files and all `*_ext.rs` files are
left in place.

Generated REST, RPC, stream, Toasty, and SeaORM templates have ignored
compile-smoke tests that create temporary crates and run `cargo check` plus
`cargo clippy --all-targets -- -D warnings` where applicable:

```bash
cargo test -p rozectl -- --ignored --skip postgres --skip mysql --skip mongo
```

Mongo model generation uses the standard model generator:

```bash
rozectl model generate example/user.model --out services/user-api --format mongo
rozectl model mongo --schema example/user.model --dir services/user-api
```

The Mongo output creates a Rust module with a model type, repository, typed CRUD
helpers, cache helper stubs, and common error helpers. Use `model generate
--format mongo` for DSL-owned schemas and `model inspect --db-kind mongo` for
existing MongoDB collections.

## Search generation

Search support is separate from database model generation. Use `rozectl search`
for Elasticsearch, OpenSearch, and Meilisearch indexes. The generated code uses
the same repository shape across engines and delegates HTTP calls to
`roze-search`:

```bash
rozectl search generate example/user.search --engine elasticsearch --out services/user-api
rozectl search generate example/user.search --engine opensearch --out services/user-api
rozectl search generate example/user.search --engine meilisearch --out services/user-api
```

Supported engine names are `elasticsearch`, `opensearch`, and `meilisearch`.
Each command writes:

```text
src/search/mod.rs
src/search/<index>.rs
```

The generated index module contains a document struct, a repository struct,
`new`, `health`, `index`, `delete`, and `search_text` helpers. The repository
keeps the original index field names with `serde(rename = "...")`, so Rust
field names can stay idiomatic without changing the search engine contract.

The search schema DSL is intentionally small:

```text
index users
primary id
field id keyword primary filterable sortable
field name text searchable
field email keyword filterable
field age i64 filterable sortable
field created_at datetime filterable sortable
```

Supported field kinds are `keyword`, `text`, `i32`, `i64`, `u64`, `f64`,
`bool`, `datetime`, and `json`. The parser also accepts common engine aliases
such as `integer`, `long`, `unsigned_long`, `float`, `double`, `boolean`,
`date`, and `object`.

Search index and field names must generate valid, non-conflicting Rust module,
type, and field identifiers. Names that normalize to a single `_` are rejected.

The same schema can be written as JSON when a pipeline already owns structured
metadata:

```json
{
  "index": "users",
  "primary": "id",
  "fields": [
    { "name": "id", "kind": "keyword", "primary": true, "filterable": true, "sortable": true },
    { "name": "name", "kind": "text", "searchable": true },
    { "name": "email", "kind": "keyword", "filterable": true },
    { "name": "age", "kind": "i64", "filterable": true, "sortable": true }
  ]
}
```

Existing indexes can be inspected directly:

```bash
rozectl search inspect users --engine elasticsearch --url http://127.0.0.1:9200 --out services/user-api
rozectl search inspect users --engine opensearch --url http://127.0.0.1:9200 --out services/user-api
rozectl search inspect users --engine meilisearch --url http://127.0.0.1:7700 --sample-size 100 --out services/user-api
```

Elasticsearch and OpenSearch inspection reads `/<index>/_mapping`.
Meilisearch inspection reads `/indexes/<index>/settings`,
`/indexes/<index>`, and `/indexes/<index>/documents?limit=<sample-size>`.
Pass `--api-key` when the engine requires authentication. For Meilisearch,
`--sample-size` controls document sampling for field type inference.

Use `--update` to refresh generated search files in an existing service and
`--force` for a full rewrite. `src/search/mod.rs` and `src/search/<index>.rs`
are schema-owned generated files. Put handwritten ranking, boosting,
post-processing, or application-specific query composition in separate
application modules that call the generated repository.

## Dockerfile generation

`rozectl docker --binary <name>` writes a production-oriented multi-stage Rust
Dockerfile and validates it before returning success. The output is a stable
generated deployment baseline, not a certification of an application's image,
dependencies, capacity, or runtime configuration:

```bash
rozectl docker \
  --builder-image rust:1.87-bookworm \
  --base-image debian:bookworm-slim \
  --binary user-api \
  --port 8080 \
  --timezone Asia/Shanghai \
  --out Dockerfile
```

The generated Dockerfile builds the release binary in a builder stage, copies
the binary and `config.yaml` with non-root ownership, sets OCI image labels,
exposes the service port, and runs as `roze:roze`. The runtime binary is
controlled by `--binary`.

## Kubernetes generation

`rozectl kube deploy` writes a manifest with Deployment, Service, HPA, and
standard liveness/readiness/startup probes. The default output path is
`deploy/kubernetes.yaml`.

```bash
rozectl kube deploy \
  --name user-api \
  --namespace default \
  --image registry.example.com/user-api@sha256:<64-hex-digest> \
  --replicas 2 \
  --port 8080 \
  --cpu-request 100m \
  --memory-request 128Mi \
  --cpu-limit 500m \
  --memory-limit 512Mi \
  --min-replicas 2 \
  --max-replicas 5 \
  --target-cpu 70 \
  --target-memory 80 \
  --env-file .env \
  --config-map user-api-config \
  --secret user-api-secrets \
  --tls-secret user-api-upstream-tls \
  --image-pull-secret registry-credentials \
  --min-available 1
```

`--env KEY=VALUE` entries are validated before writing the manifest.
`--image` must use immutable `repository@sha256:<64 hex>` syntax; mutable tags
and `latest` are rejected before any deployment file is written.
`--name`, `--namespace`, `--config-map`, and `--tls-secret` are validated as
lowercase DNS-1123 labels before rendering. `--min-available` must be a positive
count no greater than the initial replica count, or a percentage from 1% to
100%; zero-replica deployments are rejected.
`--config-map` adds an `envFrom.configMapRef` reference. `--env-file` reads a
dotenv-style file, validates each `KEY=VALUE` line, emits a generated
`<name>-env` ConfigMap, and wires it through `envFrom`. The Pod template also
receives a stable `checksum/roze-env` annotation, so changing the generated
ConfigMap content creates a new Deployment revision and rolls the Pods.
`--secret` adds an `envFrom.secretRef` to an existing application Secret. The
generator stores only the Secret name and validates it as DNS-1123; secret data
never enters generated YAML.
`--tls-secret` mounts an existing Kubernetes Secret read-only at
`/var/run/secrets/roze/tls` with mode `0400`; the generator never writes CA,
certificate, or private-key material into the manifest.
`--image-pull-secret` references an existing registry credential Secret and
generates `imagePullSecrets`; its name is validated as DNS-1123 before output.
The manifest always includes a ServiceAccount, PodDisruptionBudget, and
NetworkPolicy. The policy allows ingress to the service port and limits egress
to same-namespace workloads, kube-system DNS on TCP/UDP 53, and external TLS on
TCP 443. `--min-available` controls the PodDisruptionBudget `minAvailable`
value.

HPA uses both CPU and memory utilization. `--target-cpu` defaults to 70% and
`--target-memory` defaults to 80%. Scale-up can double capacity or add four
Pods per minute, whichever is larger. Scale-down waits through a 300-second
stabilization window and removes at most 25% of replicas per minute.

Pod metadata includes Prometheus discovery annotations for `/metrics` on the
service port, connecting the generated runtime endpoint to cluster scraping
without a separate hand-written patch.

The generated Pod runs as UID/GID `10001` with `runAsNonRoot`, `RuntimeDefault`
Seccomp, and an `fsGroup` of `10001`. The service container disables privilege
escalation, uses a read-only root filesystem, and drops every Linux capability.
ServiceAccount token auto-mounting is disabled because generated services do
not require Kubernetes API access by default.

Deployments use a zero-unavailable rolling update (`maxUnavailable: 0`,
`maxSurge: 1`), wait 10 seconds before considering a Pod ready, fail a stalled
rollout after 600 seconds, and spread replicas across hostnames when capacity
allows. A five-second `preStop` window lets readiness draining and graceful
shutdown stop new work before Kubernetes terminates the process.

Generated probes target the standard service endpoints:

- liveness: `/healthz`
- readiness: `/readyz`
- startup: `/startupz`

`rozectl kube deploy` validates the generated manifest before returning
success. Re-run the same offline validation without requiring a Kubernetes
cluster:

```bash
rozectl kube validate --file deploy/kubernetes.yaml
```

The validator checks for Deployment, Service, HPA, ServiceAccount,
PodDisruptionBudget, NetworkPolicy, standard probes, resource requests/limits,
service account wiring, and service/HPA/PDB/NetworkPolicy key fields.
When a TLS Secret is present it also requires a read-only mount and `0400`
default mode; NetworkPolicy validation requires the DNS and TLS egress rules.
The validator also rejects manifests that omit the generated non-root,
Seccomp, no-privilege-escalation, read-only-root, or capability-drop controls.
Rolling-update limits, rollout deadlines, topology spreading, and the pre-stop
drain window are validated as required production fields.

## Helm chart generation

`rozectl helm chart` writes a production-oriented application chart with
`Chart.yaml`, `values.yaml`, `values.schema.json`, and Deployment, Service, HPA, ServiceAccount,
PodDisruptionBudget, and NetworkPolicy templates. It uses the same resource,
probe, autoscaling, image, env, and ConfigMap settings as `kube deploy`.
Review the generated chart against the production checklist before using it in
a real environment.

```bash
rozectl helm chart \
  --name user-api \
  --image registry.example.com/user-api@sha256:<64-hex-digest> \
  --replicas 2 \
  --port 8080 \
  --min-replicas 2 \
  --max-replicas 5 \
  --target-cpu 70 \
  --target-memory 80 \
  --env RUST_LOG=info \
  --config-map user-api-config \
  --secret user-api-secrets \
  --tls-secret user-api-upstream-tls \
  --image-pull-secret registry-credentials \
  --min-available 1 \
  --chart-version 0.1.0 \
  --app-version 1.2.3 \
  --out deploy/user-api-chart
```

The Helm chart always includes ServiceAccount, PodDisruptionBudget, and
NetworkPolicy templates. `values.yaml` exposes `serviceAccount.name` and
`podDisruptionBudget.minAvailable` for chart-level customization. It also
exposes `tlsSecret.name` and `tlsSecret.mountPath`; an empty name disables the
mount, while a configured Secret is mounted read-only with mode `0400`.
The Deployment template always applies the same Pod and container security
contexts as `kube deploy`.
`values.yaml` exposes `observability.prometheusScrape`, `metricsPath`, and
`metricsPort`; scraping is enabled for `/metrics` by default.
The image repository and SHA-256 digest are stored separately in values and
recombined as an immutable digest reference by the Deployment template.
Private registry credentials are exposed as `image.pullSecrets`, allowing
platform owners to append multiple pull secrets without editing the template.
Application Secret and ConfigMap references are exposed together through
`envFrom`; schema and offline validation reject malformed or ambiguous entries.
`values.schema.json` uses JSON Schema Draft 2020-12 and rejects malformed image
digests, out-of-range ports or HPA targets, invalid ServiceMonitor durations,
and unknown top-level values before Helm renders the chart.
`rozectl helm validate` also parses `values.yaml` offline and enforces the
critical schema semantics without requiring Helm: required and unknown keys,
numeric ranges, HPA min/max ordering, metrics/service port consistency, and a
scrape timeout shorter than the scrape interval.
It also exposes an optional `observability.serviceMonitor` block with
`enabled`, `interval`, `scrapeTimeout`, and additional labels. The generated
`ServiceMonitor` template is disabled by default so charts remain installable
without the Prometheus Operator CRDs; enabling it selects the generated Service
through its named `http` port.

`rozectl helm chart` validates the chart directory before returning success.
Re-run the same offline validation, then optionally render with Helm:

```bash
rozectl helm validate --chart deploy/user-api-chart
helm template user-api deploy/user-api-chart
```

`rozectl helm validate` checks the chart structure without requiring Helm. It
verifies `Chart.yaml`, `values.yaml`, parsed `values.schema.json`, Deployment,
Service, HPA, ServiceAccount,
PodDisruptionBudget, NetworkPolicy, optional ServiceMonitor wiring, and helper
templates.

## Plugin contract

`rozectl api plugin` parses `.api` once, serializes the normalized API spec to
JSON, then runs the plugin command.

```bash
rozectl api plugin --plugin ./tools/api-plugin.sh --api example/user.api --dir generated
```

The plugin receives the same JSON payload through stdin and environment:

| Name | Value |
| --- | --- |
| `ROZECTL_API_SPEC_JSON` | normalized API spec JSON |
| `ROZECTL_API_FILE` | source `.api` path |
| `ROZECTL_OUT_DIR` | output directory passed by `--dir` |

The plugin owns any files it writes under `ROZECTL_OUT_DIR`.

## CLI environment and upgrade

`rozectl env` prints the active binary path, version, `CARGO_HOME`,
`RUSTUP_HOME`, `RUST_LOG`, and `PATH`. This mirrors goctl-style environment
inspection while keeping Roze-specific names.

`rozectl completion <shell>` prints a shell completion script for `bash`,
`zsh`, `fish`, or `powershell`.

`rozectl upgrade` runs:

```bash
cargo install --git https://github.com/roze-team/roze.git rozectl --force
```

Use `--repo`, `--branch`, or `--rev` to pin another source, and `--dry-run` to
print the cargo command without executing it.

## Current limitations

- `dive` currently covers one collection level.
- Map OpenAPI schemas are emitted as generic objects; value constraints are not
  yet projected into OpenAPI `additionalProperties`.
- OpenAPI constraints for `min/max/len/oneof` are not yet emitted.
- Advanced validator tags such as `required_with_all`, `excluded_if`, `uuid`,
  custom validators, nested struct validation, and cross-struct comparison are
  not generated yet.
