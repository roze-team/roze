# Roze

Roze is a small Rust service framework scaffold with:

- `crates/roze-core`: base types, errors, results, and shared response helpers.
- `crates/roze-http`: Poem router, extractors, and middleware wrappers.
- `crates/roze-validation`: request parameter validation helpers.
- `crates/roze-config`: YAML/TOML/env configuration loading.
- `crates/roze-log`: tracing and `trace_id` plumbing.
- `crates/roze-auth`: JWT and auth helpers.
- `crates/roze-db`: SeaORM and database helpers.
- `crates/roze-cache`: Redis helpers.
- `crates/roze-openapi`: Swagger/OpenAPI support.
- `crates/roze-rpc`: tonic gRPC helpers.
- `crates/roze-job`: scheduled job scaffolding.
- `crates/roze-mq`: messaging scaffolding.
- `apps/rozectl`: code generation for `.api` service definitions.
- `apps/roze-example`: a generated example service from `example/user.api`.

The direction is go-zero style microservice ergonomics with Rust-native building blocks:

- IDL first: `.api` files define request/response types and routes.
- Generated layout: handlers, logic, service context, config, and proto are generated from IDL.
- REST: `poem` plus `roze-core::rest::{ApiResponse, AppError}` and Poem-native middleware.
- RPC: `roze-grpc` wraps tonic build/runtime APIs, and `rpc.rs` adapts gRPC requests into shared `logic`.
- ORM: `SeaORM` is the default database layer; generated services get an optional `database.url` config and `ServiceContext::db`.
- Governance: registry, balancing, middleware, config, tracing, and error handling live in `roze-core`.

The Loco/Rails lesson applied here is convention over configuration: generated services have a stable structure, and application code starts in `src/logic` instead of wiring boilerplate by hand.

`rozectl` generates the scaffold and glue code: API projects, RPC projects,
model modules, documentation, client SDKs, Dockerfiles, and Kubernetes
manifests. Real business behavior still belongs in application code, including
logic handlers, complex SQL, domain validation, transactions, authorization,
and permission checks.

## Usage Documentation

- [Usage documentation](docs/usage/README.md)
- [rozectl API generator guide](docs/usage/rozectl-api.md)
- [rozectl goctl compatibility guide](docs/usage/rozectl-goctl-compat.md)

## Quick Start

```bash
cargo run -p rozectl -- api generate example/user.api --out apps/roze-example --roze-source path
cargo run -p roze-example
```

`rozectl api generate` creates a REST service from route declarations. `rozectl
rpc generate` creates a gRPC service from `rpc` method declarations. The two
commands intentionally reject mixed definitions so API and RPC projects keep
different layouts and dependency sets.

Regenerate framework-owned files while preserving `src/logic/mod.rs` and
`config.yaml`:

```bash
cargo run -p rozectl -- api generate example/user.api \
  --out apps/roze-example \
  --update \
  --roze-source path
```

Use `--force` only for a full rebuild. New projects use
`https://github.com/roze-team/roze.git` dependencies by default; pass
`--roze-source path` for projects inside this repository.

`rozectl api new user` and `rozectl rpc new user` create `user/` in the current
directory by default. Use `--out services/user` to choose another location.
Projects created outside a Cargo workspace receive a standalone manifest with
explicit package metadata and dependency versions.

`rozectl api client ts example/user.api --out sdk/user.ts` generates a typed
TypeScript `fetch` client from REST routes and request/response types. The
generated SDK supports `baseUrl`, per-call headers, injected `fetch`, path
parameters, query parameters, headers, and JSON bodies.
Use `rozectl api client js example/user.api --out sdk/user.js` for a plain ESM
JavaScript client with JSDoc typedefs and the same request-building behavior.
Use `rozectl api client dart example/user.api --out sdk/user.dart` for a Dart
client that uses `package:http`, typed models, JSON serialization, route path
parameters, query parameters, headers, and JSON bodies.

`rozectl openapi generate example/user.api --out openapi.json` writes an
OpenAPI 3 document with component schemas, route parameters, JSON/form request
bodies, response schemas, tags, and bearer security when JWT is declared.

goctl-compatible aliases are available for the common generator flow:

```bash
rozectl api go -api example/user.api -dir apps/roze-example
rozectl rpc protoc example/user.proto --zrpc_out apps/user-rpc
rozectl model mysql ddl -src example/user.sql -dir apps/roze-example
rozectl model mongo --type User -dir apps/roze-example
rozectl docker -go main.go --binary user-api --port 8080
rozectl kube deploy --name user-api --image registry.example.com/user-api:latest
rozectl api swagger -api example/user.api -dir docs/openapi --format yaml
rozectl api doc -api example/user.api -dir . -o docs/api
```

`rozectl model generate example/user.model --out apps/roze-example` writes a
SeaORM-style model scaffold into an existing service. The model generator
supports both the existing DSL and SQL DDL via `--format auto|dsl|sql`.
The DSL supports `table`, `primary`, `cache`, `cache_ttl_secs`, and repeated
`field` lines.

`rozectl model inspect users --db-kind sqlite --db-url sqlite::memory: --out apps/roze-example`
inspects an existing database schema and emits the same SeaORM-based model
scaffold. SeaORM remains the default ORM for generated database code.
Pass `--orm toasty` to generate Toasty model structs and repository helpers
instead:

```bash
rozectl model generate example/user.sql \
  --out apps/roze-example \
  --format sql \
  --orm toasty

rozectl model mysql ddl \
  -src example/user.sql \
  -dir apps/roze-example \
  --orm toasty
```

The Toasty output uses `#[derive(toasty::Model)]`, preserves auto-increment
primary keys with `#[auto]`, marks generated cache-key lookups as `#[unique]`,
and adds `toasty` to the target service manifest when a `Cargo.toml` is
present. It expects application code to pass a configured `toasty::Db`.

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
```

The generated SeaORM model keeps the schema name in the entity attributes.
For Toasty, choose the schema/database through the Toasty driver configuration.

Example SQL input:

```bash
rozectl model generate example/user.sql --out apps/roze-example --format sql
```

Supported SQL input focuses on common MySQL and Postgres DDL:

- `CREATE TABLE` with a single primary key
- `AUTO_INCREMENT`, `SERIAL`, `BIGSERIAL`, and inline `PRIMARY KEY`
- `DEFAULT` and column comments, including `COMMENT ON COLUMN ... IS ...`
- common scalar column types such as integers, booleans, text, JSON, timestamps, UUIDs, and blobs
- unsupported features such as composite keys and foreign keys fail fast with a clear error

Generated service layout:

```text
src/
  config.rs
  handler/mod.rs
  logic/mod.rs
  pb.rs
  svc/mod.rs
  types.rs
  rpc.rs
build.rs
proto/service.proto
  config.yaml
```

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
