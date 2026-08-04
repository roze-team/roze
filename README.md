# Roze

Roze 1.0 is the stable release channel for the Rust-native framework,
`rozectl`, generated Rust services, and TypeScript/JavaScript Web clients.
Public APIs and generated contracts follow Semantic Versioning. Runtime
adoption must still review the independent evidence state in the
[maturity matrix](docs/maturity.md) and
[production evidence](docs/production-evidence.md); stable API does not mean a
deployment has inherited Roze's test environment or operational evidence.

Roze is a Rust service framework with:

- `crates/roze-core`: base types, errors, results, and shared response helpers.
- `crates/roze-http`: Roze native HTTP server, routing, extractors, responses,
  application-facing WebSocket upgrades/frames, and graceful shutdown.
- `crates/roze-middleware`: HTTP middleware helpers and route governance integration.
- `crates/roze-rate-limit`: shared memory/Redis token buckets, composite identity policies, and bounded failure behavior for REST, RPC, and Gateway.
- `crates/roze-validation`: request parameter validation helpers.
- `crates/roze-config`: YAML/TOML/env configuration loading.
- `crates/roze-log`: tracing and `trace_id` plumbing.
- `crates/roze-metrics`: in-process metrics registry; labeled metric state uses DashMap for concurrent hot paths.
- `crates/roze-auth`: JWT and auth helpers.
- `crates/roze-db`: database connection helpers.
- `crates/roze-transaction-sql`: PostgreSQL/MySQL persistent Outbox with
  transactional enqueue, lease-based claim, dead letters, and migrations.
- `crates/roze-orm`: common ORM contracts for pagination, filters, tenant scope, audit fields, and soft delete.
- `crates/roze-cache`: Redis cache helpers with cache-aside, negative cache, TTL jitter, and singleflight loading.
- `crates/roze-local-cache`: Moka-backed in-process cache with TTL, capacity eviction, and hit/miss statistics.
- `crates/roze-singleflight`: request coalescing for cache miss protection; key lookup uses DashMap for concurrent hot paths.
- `crates/roze-openapi`: Swagger/OpenAPI support.
- `crates/roze-rpc`: tonic gRPC helpers and registry primitives; in-memory registry plus method governance state use DashMap for concurrent hot paths.
- `crates/roze-job`: governed scheduled jobs and lifecycle integration.
- `crates/roze-mq`: reliable messaging primitives; in-memory topic, offset, and idempotency indexes use DashMap/DashSet.
- `crates/roze-eventbus`: event envelope and in-memory pub/sub; topic sender lookup uses DashMap.
- `crates/roze-session`: in-memory session store; session lookup uses DashMap.
- `crates/roze-ws`: WebSocket session hub; session lookup uses DashMap.
- `crates/roze-storage`: object storage contracts for local/S3 API/Qiniu/Alibaba/Tencent providers.
- `crates/roze-search`: unified search client for Elasticsearch, OpenSearch, and Meilisearch.
- `crates/roze-ai`: experimental provider-neutral AI messages, models, tools,
  bounded agents and teams, compiled/parallel DAG workflows, pluggable
  checkpoint/interrupt/resume with a `roze-storage` adapter, workflow event
  streams, bounded backpressure-aware node chunk streams, permission-aware
  delegation, RAG component contracts, `roze-search` adapters, and
  deterministic test doubles; it reuses Roze
  context, permissions, errors, lifecycle attachment, and governance
  boundaries instead of replacing them.

Hot-path crates keep Criterion benchmarks under `benches/` for regression baselines:
`roze-metrics` covers labeled writes and Prometheus rendering, `roze-local-cache`
covers async insert/get/get-or-insert, `roze-singleflight` covers unique-key,
cached-key, and reset paths, `roze-rpc` covers memory registry
register/discover/deregister, and session/WebSocket/eventbus/MQ in-memory
stores cover their lookup and publish/register hot paths.

Production smoke starts from one command:

```bash
bash scripts/production-smoke.sh
bash scripts/production-smoke.sh --with-compose
bash scripts/rozectl-smoke.sh
```

The smoke path includes generated REST/RPC compile tests, core runtime crate
tests, generated stream worker compile tests, and app-level checks.
`--with-compose` starts the integration profile for Etcd, Consul, Kafka, NATS,
Redis, Postgres, MySQL, MongoDB, Elasticsearch, OpenSearch, and Meilisearch.
`scripts/rozectl-smoke.sh` verifies the `rozectl` CLI command surface with
temporary files, fake Docker for `dev`, and a fake local search server for
search inspect.
- `crates/roze-dtm`: distributed transaction manager core, defaulting to TCC.
- `apps/rozectl`: code generation for API, RPC, model, search, OpenAPI, SDK, Docker, and Kubernetes assets.
- `apps/roze-dtm`: standalone DTM base service for TCC/Saga coordination.
- `apps/roze-example`: a generated example service from `example/user.api`.

The direction is Rust-native microservice ergonomics with explicit generated boundaries:

- IDL first: `.api` files define request/response types and routes.
- Generated layout: handlers, logic, service context, config, and proto are generated from IDL.
- REST: `roze_http`, `tower`, and `tower-http` with `roze-result::ApiResponse`, `roze-error::RozeError`, and Roze middleware boundaries.
- RPC: `roze-grpc` wraps tonic build/runtime APIs, and `rpc.rs` adapts gRPC requests into shared `logic`.
- ORM: Toasty is the default generated SQL model scaffold; `--orm sea-orm`
  switches model generation to SeaORM. Shared ORM request contracts live in
  `roze-orm`.
- DTM: built-in distributed transaction manager defaults to TCC and keeps Saga as an optional workflow.
- Governance: registry, balancing, middleware, config center, tracing, NATS JetStream, outbox relay, and error handling live across the `roze-*` crates.

The Loco/Rails lesson applied here is convention over configuration: generated services have a stable structure, and application code starts in `src/logic` instead of wiring boilerplate by hand.

Generated REST services always expose the same Rust project shape:
`src/main.rs`, `src/config/mod.rs`, `src/route/`, `src/handler/`,
`src/logic/`, `src/middleware/`, `src/openapi/mod.rs`, `src/svc/mod.rs`, and
`src/types/mod.rs`. Generated RPC services use `build.rs`,
`proto/service.proto`, `src/lib.rs`, `src/client/mod.rs`, `src/server/mod.rs`,
`src/pb/mod.rs`, `src/svc/mod.rs`, `src/types/mod.rs`, and `src/logic/`.
This keeps route registration, handler adaptation, business logic, context,
validation, errors, tracing, and response contracts uniform across teams.
`src/svc/mod.rs` is framework-owned and refreshed on REST/RPC updates;
application resources and background services belong in the preserved
`src/application.rs` hooks. `ServiceContext` includes a cloneable, type-safe
application extension store, so custom resources remain bound to the service
instance instead of process-global singletons.
Generated services also include optional NATS/outbox slots in `ServiceContext`,
so reliable event publishing follows the same convention in API and RPC
projects.
Model generation owns its context hook and `ServiceContext::model()` extension
under `src/model`; REST/RPC and model `--update` commands can therefore run in
either order without rewriting each other's files.
Ent models can opt into explicit database sharding with a `RozeShard`
annotation. Roze provides deterministic Jump Hash routing, per-shard
primary/replica pools, single-shard transactions, migration fan-out, health
checks, and bounded metrics; cross-shard queries remain application-owned. See
[Database Sharding Contract](docs/contracts/database-sharding.md).

`rozectl` generates the scaffold and glue code: API projects, RPC projects,
model modules, search repositories, documentation, client SDKs, Dockerfiles,
and Kubernetes manifests. Real business behavior still belongs in application
code, including logic handlers, complex SQL, domain validation, transactions,
authorization, permission checks, and search ranking rules.

Add an experimental AI module to an existing generated REST/RPC project
without changing its established generators:

```bash
cargo run -p rozectl -- ai generate assistant \
  --out services/support \
  --roze-source path
```

The command generates provider-neutral agent/tool/prompt composition under
`src/ai`, reuses the existing service context and Roze request context, and
preserves application-owned AI files on `--update`. Its generated
`config/ai.example.yaml` uses the existing Roze secret resolver and can build
multiple OpenAI-compatible providers through `AiRuntime::from_config`.
Add `--with-workflow` and `--with-rag` initially or during a later `--update`
to generate application-owned composition and Roze Search-backed RAG
scaffolds without replacing existing AI business files.
Add `--with-team` for a bounded multi-agent team scaffold. Workflow templates
also expose `CheckpointStore` and `roze-storage`-backed runners; team templates
expose standard permission-checked Agent delegation without adding parallel
storage, identity, or authorization subsystems.

Generated API and RPC services use one service manifest for governed
cross-service RPC dependencies instead of manual generated-code edits:

```bash
rozectl service dependency add order \
  --project services/payment \
  --crate shop-order-rpc \
  --path ../shop-order-rpc \
  --endpoint 127.0.0.1:4002
rozectl service sync --project services/payment --check
```

The manifest synchronizes Cargo, dependency configuration defaults, managed
clients, and readiness while preserving deployment overrides and business
logic.

## Usage Documentation

- [Project standards](docs/project-standards.md)
- [Requirements vs current architecture](docs/requirements-architecture-comparison.md)
- [Roadmap](docs/roadmap.md)
- [Production generation plan](docs/go-zero-surpass-plan.md)
- [Module maturity matrix](docs/maturity.md)
- [Stability commitment](docs/stability-commitment.md)
- [Production evidence](docs/production-evidence.md)
- [Release policy](docs/release.md)
- [Upgrade guide](docs/upgrade.md)
- [Production checklist](docs/production-checklist.md)
- [Usage documentation](docs/usage/README.md)
- [Middleware contract](docs/contracts/middleware.md)
- [AI runtime contract](docs/contracts/ai-runtime.md)
- [rozectl generator guide](docs/usage/rozectl-api.md)
- [Changelog](CHANGELOG.md)
- [Contributing](CONTRIBUTING.md)
- [Security policy](SECURITY.md)

## Install rozectl

Roze 1.0 supports signed-tag Git installation and local checkout installation.
The crates.io publication state is tracked separately in the
[release policy](docs/release.md).

Install from GitHub:

```bash
cargo install --git https://github.com/roze-team/roze.git rozectl
```

After the signed tag is published, install the exact stable 1.0 release with:

```bash
cargo install --git https://github.com/roze-team/roze.git --tag v1.0.0 rozectl
```

Force reinstall or upgrade an existing binary:

```bash
cargo install --git https://github.com/roze-team/roze.git rozectl --force
```

Install from a local checkout:

```bash
cargo install --path apps/rozectl
```

Force reinstall from a local checkout:

```bash
cargo install --path apps/rozectl --force
```

Verify the installation:

```bash
rozectl --help
```

Generated REST/RPC services pin their package edition to Rust 2021, including
when they are created inside a workspace. This keeps generated `build.rs`
aligned with current Roze templates even if the parent workspace moves to Rust
2024.

Roze keeps SeaORM/sqlx sqlite support in the framework, but generated Toasty
dependencies default to MySQL/PostgreSQL only. This avoids duplicate
`libsqlite3-sys` `links = "sqlite3"` conflicts when generated services also use
`roze-db`.

## Quick Start

```bash
cargo run -p rozectl -- api generate example/user.api --out apps/roze-example --roze-source path
cargo run -p roze-example
```

`rozectl api generate` creates a REST service from route declarations. `rozectl
rpc generate` creates a gRPC service from `rpc` method declarations. The two
commands intentionally reject mixed definitions so API and RPC projects keep
different layouts and dependency sets.

Regenerate framework-owned files while preserving application-owned logic,
custom middleware, and `config.yaml`:

```bash
cargo run -p rozectl -- api generate example/user.api \
  --out apps/roze-example \
  --update \
  --roze-source path
```

`--update` preserves `src/logic/prelude.rs`, REST
`src/logic/<group>/prelude.rs`, REST `src/logic/<group>/<method>.rs`, RPC
`src/logic/<method>.rs`, custom REST middleware files under `src/middleware/`,
and `config.yaml`. Put service-wide logic declarations and imports in the root
prelude, and REST group-local helper declarations and re-exports in the matching
group prelude; generated `logic/mod.rs` indexes are refreshed. Generated glue
such as `src/route/`, `src/handler/`, `src/server/`, `src/client/`, DTOs,
OpenAPI, and proto/build files is refreshed.
The first update of a legacy project transactionally adds a missing
`application::register_services` hook and moves resolvable custom module/use
declarations from old logic indexes into the matching application-owned
prelude; later updates preserve those files unchanged.
Use `--force` only for a full rebuild. New projects use
`https://github.com/roze-team/roze.git` dependencies by default; pass
`--roze-source path` for projects inside this repository.

`rozectl api new user` and `rozectl rpc new user` create `user/` in the current
directory by default. Use `--out services/user` to choose another location.
Projects created outside a Cargo workspace receive a standalone manifest with
explicit package metadata and dependency versions. The same applies to
projects listed by a parent workspace's `exclude` entries; generated standalone
manifests include their own empty workspace boundary and remain stable across
repeated Stream `--update` runs.

`rozectl api client ts example/user.api --out sdk/user.ts` generates a typed
TypeScript `fetch` client from REST routes and request/response types. The
generated SDK supports `baseUrl`, per-call headers, injected `fetch`, path
parameters, query parameters, headers, and JSON bodies. Declared custom object
types, nested arrays, optional fields, and JSON field renames remain typed
across the generated interface graph. Idempotent routes require a typed,
reusable `idempotencyKey` option and forward it as `Idempotency-Key`.
Standard Roze `{ code, msg, data }` responses are unwrapped to `data`;
non-zero envelope codes raise `RozeApiError` with the server `msg`, while
non-envelope endpoints retain their raw response shape.
Use `rozectl api client js example/user.api --out sdk/user.js` for a plain ESM
JavaScript client with JSDoc typedefs and the same request-building behavior.

`rozectl openapi generate example/user.api --out openapi.json` writes an
OpenAPI 3 document with component schemas, route parameters, JSON/form request
bodies, response schemas, tags, and bearer security when JWT is declared.

Roze-native commands are available for the common generator flow:

```bash
rozectl api generate example/user.api --out apps/roze-example
rozectl rpc protoc example/user.proto --out apps/user-rpc
rozectl model generate example/user.sql --out apps/roze-example --format sql
rozectl model generate example/user.model --out apps/roze-example --format mongo
rozectl docker --binary user-api --port 8080
rozectl kube deploy --name user-api --image registry.example.com/user-api@sha256:<64-hex-digest>
rozectl openapi generate example/user.api --out docs/openapi.json
rozectl search generate example/user.search --engine elasticsearch --out apps/roze-example
rozectl search inspect users --engine meilisearch --url http://127.0.0.1:7700 --out apps/roze-example
rozectl api doc --api example/user.api --dir . --out docs/api
rozectl doc service --api example/user.api --out SERVICE.md
```

`rozectl model generate example/user.model --out apps/roze-example` writes a
Toasty model scaffold into an existing service. The model generator
supports both the existing DSL and SQL DDL via `--format auto|dsl|sql`.
The DSL supports `table`, `primary`, `cache`, `cache_ttl_secs`, and repeated
`field` lines.

`rozectl model inspect users --db-kind sqlite --db-url sqlite::memory: --out apps/roze-example`
inspects an existing SQL table and emits the same Toasty-based model scaffold.
Postgres, MySQL, and MongoDB are also supported through `--db-kind`. Toasty
remains the default SQL ORM for generated database code. Pass `--orm sea-orm`
to generate SeaORM-style SQL modules instead:

```bash
rozectl model generate example/user.sql \
  --out apps/roze-example \
  --format sql \
  --orm sea-orm
```

The default Toasty output uses `#[derive(toasty::Model)]`, preserves
auto-increment primary keys with `#[auto]`, marks generated cache-key lookups as
`#[unique]`, and adds `toasty` to the target service manifest when a
`Cargo.toml` is present. Generated Toasty repository methods accept
`&mut dyn toasty::Executor`, so the same CRUD helpers can run against a
configured `toasty::Db` or a `toasty::Transaction`. Generated repositories
include single-table primary/cache-key
lookup, `list`, `insert`, `update`, `delete_by_<primary>`, `count`, paginated
query, equality/IN/range filters, nullable equality/`IS NULL` filters, typed sorting, batch insert/delete,
soft-delete helpers, and tenant-scoped lookup when matching columns are
configured or inferred. SeaORM output uses the same repository surface with
set-based batch operations. Toasty and SeaORM repositories both include local
transaction helpers; SeaORM passes `sea_orm::DatabaseTransaction` to the
callback. Field metadata is generated separately as
`src/model/<model>_fields.rs`, exporting table constants and `{Model}Field`
enums for codegen, validation, and AI-readable context. Handwritten model or
repository extensions belong in `src/model/<model>_ext.rs`; `--update`
preserves that file and `--force` rewrites it.

Use `--schema` to make the target schema explicit when the table name is
shared across namespaces:

```bash
rozectl model inspect users \
  --schema public \
  --db-kind postgres \
  --db-url postgres://postgres:postgres@localhost:5432/roze \
  --out apps/roze-example
```

Schema-qualified table names are supported for inspection, for example:
`public.users` on Postgres and `db.users` on MySQL.
For MongoDB, `--schema` is the database name when the database is not included
in `--db-url`; `--sample-size` controls how many collection documents are
sampled for field/type inference. Mongo inspection also maps `_id` to `id`,
keeps unique/index metadata, emits find helpers for single-field unique indexes,
emits compound-index find/list helpers, and generates an `id: ObjectId` model
for empty collections.

Examples:

```bash
rozectl model inspect public.users \
  --db-kind postgres \
  --db-url postgres://postgres:postgres@localhost:5432/roze \
  --out apps/roze-example

rozectl model inspect db.users \
  --db-kind mysql \
  --db-url mysql://root:root@localhost:3306/roze \
  --out apps/roze-example

rozectl model inspect users \
  --db-kind mongo \
  --db-url mongodb://127.0.0.1:27017/roze \
  --sample-size 100 \
  --out apps/roze-example
```

The generated SeaORM model keeps the schema name in the entity attributes.
For Toasty, choose the schema/database through the Toasty driver configuration.

`rozectl search generate example/user.search --engine elasticsearch --out apps/roze-example`
generates `src/search/mod.rs` and `src/search/users.rs` for Elasticsearch,
OpenSearch, or Meilisearch. Generated repositories use `roze-search` for
health checks, document indexing, document deletion, and text search. The DSL
declares the index name, primary field, field types, and searchable/filterable/
sortable flags.

`rozectl search inspect users --engine opensearch --url http://127.0.0.1:9200 --out apps/roze-example`
reads an existing Elasticsearch/OpenSearch index mapping and emits the same
Roze search scaffold. Meilisearch inspection reads index settings and samples
documents to infer field types. Use `--api-key` for secured engines and
`--sample-size` to control Meilisearch sampling.

Example SQL input:

```bash
rozectl model generate example/user.sql --out apps/roze-example --format sql
```

Supported SQL input focuses on common MySQL and Postgres DDL:

- `CREATE TABLE` with a single primary key
- `AUTO_INCREMENT`, `SERIAL`, `BIGSERIAL`, and inline `PRIMARY KEY`
- `DEFAULT` and column comments, including `COMMENT ON COLUMN ... IS ...`
- common scalar column types such as integers, booleans, text, JSON, timestamps, UUIDs, and blobs
- standalone `CREATE INDEX` and `CREATE UNIQUE INDEX`, including composite and PostgreSQL partial indexes
- table-level named or anonymous `PRIMARY KEY`, `UNIQUE`, `FOREIGN KEY`, and `CHECK` constraints; checks are recognized but are not projected into generated validation
- unsupported features such as composite primary and foreign keys fail fast with a clear error

PostgreSQL `BIGINT`, `BIGSERIAL`, and `INT8` generate signed `i64` fields;
unsigned `u64` is reserved for explicitly unsigned types such as MySQL
`BIGINT UNSIGNED`. PostgreSQL `TIMESTAMP` and `TIMESTAMPTZ` remain distinct as
`.ent` `timestamp` and `timestamptz` fields and generate SeaORM `DateTime` and
`DateTimeUtc` fields with chrono support enabled automatically.

Generated RPC service layout:

```text
src/
  lib.rs
  main.rs
  config/mod.rs
  logic/
  pb/mod.rs
  svc/mod.rs
  types/mod.rs
  server/mod.rs
  client/mod.rs
build.rs
proto/service.proto
config.yaml
```

### Configuration validation

Generated services load `ServiceConfig` through `roze_config::load_service`.
Secrets are resolved before semantic validation and before listeners start.
Production rejects unknown fields, invalid governance ranges, and an enabled
rate limit without Redis; development and test profiles report unknown fields
as warnings. Service configuration debug output redacts database, cache,
broker, storage, registry, and RPC-client credentials.

Generated REST and RPC entrypoints honor `ROZE_CONFIG_PATH` before source-tree
or working-directory `config.yaml` defaults. Deployment units should point
that variable at the deployment-owned YAML instead of copying configuration
into the build checkout.

Generated rate limiting uses `store: auto`: an explicit
`governance.rate_limiter.redis_url` wins, then `cache.url`, with memory allowed
only outside production. Redis keys are scoped by the service profile unless an
explicit rate-limit namespace is configured.

### Rozectl verification

The `rozectl` generator is tested in three slices:

- SQLite and local parser/generator tests:

```bash
cargo test -p rozectl -- --skip postgres --skip mysql
```

- PostgreSQL-specific inspect tests:

```bash
export ROZECTL_TEST_POSTGRES_URL=postgres://postgres:postgres@127.0.0.1:5432/postgres
cargo test -p rozectl postgres
```

- MySQL-specific inspect tests:

```bash
export ROZECTL_TEST_MYSQL_URL=mysql://root:root@127.0.0.1:3306/roze
cargo test -p rozectl mysql
```

The CI workflow runs these slices separately and also checks the `rozectl`
sources with `rustfmt` and `clippy`.
