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
- RPC: `tonic-build` compiles generated proto files, and `rpc.rs` adapts gRPC requests into shared `logic`.
- ORM: `SeaORM` is the default database layer; generated services get an optional `database.url` config and `ServiceContext::db`.
- Governance: registry, balancing, middleware, config, tracing, and error handling live in `roze-core`.

The Loco/Rails lesson applied here is convention over configuration: generated services have a stable structure, and application code starts in `src/logic` instead of wiring boilerplate by hand.

## Quick Start

```bash
cargo run -p rozectl -- generate example/user.api --out apps/roze-example --roze-source path
cargo run -p roze-example
```

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

`rozectl model generate example/user.model --out apps/roze-example` writes a
SeaORM-style model scaffold into an existing service. The model generator
supports both the existing DSL and SQL DDL via `--format auto|dsl|sql`.
The DSL supports `table`, `primary`, `cache`, `cache_ttl_secs`, and repeated
`field` lines.

`rozectl model inspect users --db-kind sqlite --db-url sqlite::memory: --out apps/roze-example`
inspects an existing database schema and emits the same SeaORM-based model
scaffold. SeaORM remains the default ORM for generated database code.

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

The generated model keeps the schema name in the SeaORM entity attributes.

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
