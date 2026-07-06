# rozectl generator

`rozectl api generate` reads a Roze `.api` contract and generates a
Rust-native Axum REST service. The generated project keeps framework-owned files
separate from application logic so repeated generation can preserve
method-level logic, service context extensions, custom middleware, and
`config.yaml` when `--update` is used.

Think of `rozectl` as a generator for project structure and integration code.
It can generate API layers, RPC layers, model scaffolds, documentation, client
code, and deployment files. Business logic is still written by developers:
handler-driven logic, complex SQL, domain checks, transactions, authorization,
permissions, and other product-specific behavior live in the application.

`rozectl` is currently a pre-release generator. It is appropriate for
evaluation, internal pilots, and controlled production paths where the Roze Git
revision is pinned, generated diffs are reviewed, smoke checks pass, and
application teams own runtime configuration, observability, rollback,
authorization, and dependency governance. It should not be presented as a
broadly production-stable platform until [Release Policy](../release.md),
[Module Maturity Matrix](../maturity.md), and
[Production Evidence](../production-evidence.md) support that claim.

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

Generated REST/RPC services pin `edition = "2021"` in their own `Cargo.toml`
instead of inheriting `edition.workspace`. Toasty is the default SQL model ORM,
and generated Toasty dependencies use MySQL/PostgreSQL features only; sqlite
support remains available through the Roze SeaORM/sqlx stack.

## Commands

Generate a REST service:

```bash
cargo run -p rozectl -- api generate example/user.api --out apps/roze-example --roze-source path
```

Regenerate framework-owned files while preserving user logic and config:

```bash
cargo run -p rozectl -- api generate example/user.api \
  --out apps/roze-example \
  --update \
  --roze-source path
```

`--update` preserves:

- REST `src/logic/<group>/<method>.rs`
- REST/RPC `src/svc/mod.rs`
- REST custom middleware files under `src/middleware/<name>.rs`
- RPC `src/logic/<method>.rs`
- `config.yaml`

Generated glue such as route registration, handler adapters, DTOs, OpenAPI,
RPC server/client adapters, protobuf include modules, `build.rs`, and
`proto/service.proto` is refreshed. Use `--force` when you want a full rebuild.

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
```

`api validate` parses the `.api` file and checks contract-level consistency
that can otherwise surface only during generation or compilation: duplicate
types, duplicate fields or wire names, generated Rust type/field name
collisions, duplicate REST routes, duplicate RPC methods, generated REST
handler/RPC method name collisions, unknown request/response or nested field
types, non-empty reserved `EmptyReq`/`EmptyResp` declarations, invalid
generated service/REST/RPC/middleware Rust identifiers, and mismatches between
route path parameters and request `path` fields.

Format an API contract:

```bash
rozectl api format example/user.api
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
rozectl template diff api --dir templates
rozectl template update api --dir templates
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

Check contract breaking changes before regenerating or releasing:

```bash
rozectl contract check --old example/user.v1.api --new example/user.v2.api
```

`contract check` is read-only and exits with a non-zero status when it detects
breaking changes. The MVP checks removed or changed REST routes, removed RPC
methods, request/response type changes, removed fields, field type/source
changes, and newly added required fields. Additive optional fields with
`validate:"optional"` or `validate:"omitempty"` are allowed.

Generate a mock server from the API contract:

```bash
rozectl mock gen --api example/user.api --out mock-server
cd mock-server
cargo run
```

The generated mock server is a standalone Axum project. It registers the REST
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
asserts successful HTTP status codes, and verifies JSON responses. The default
base URL is `http://127.0.0.1:3000`; pass `--base-url` when generating or set
`ROZE_TEST_BASE_URL` at runtime.

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

Generated REST services expose standard operational endpoints:

- `GET /healthz` for process liveness
- `GET /readyz` for readiness
- `GET /startupz` for startup state
- `GET /metrics` for Prometheus metrics
- `GET /openapi.json` for OpenAPI

The default health handlers return `roze_health::ProbeReport` inside the
standard `ApiResponse` wrapper. Generated `ServiceContext` owns a
`roze_health::HealthRegistry`; dependency clients that are created during
startup are registered as readiness checks, and the registry tracks startup,
ready, and draining phases. Applications can add more dynamic checks with
`HealthRegistry::register_dependency` or `HealthRegistry::register_check`.

Generate client SDKs:

```bash
rozectl api client ts example/user.api --out sdk/user.ts
rozectl api client js example/user.api --out sdk/user.js
rozectl api client dart example/user.api --out sdk/user.dart
```

Generate an OpenAPI 3 document:

```bash
rozectl openapi generate example/user.api --out openapi.json
```

Generate Markdown API documentation:

```bash
rozectl api doc --api example/user.api --dir . --out docs/api
```

Run a custom API plugin:

```bash
rozectl api plugin --plugin ./tools/rozectl-plugin.sh --api example/user.api --dir generated
```

Generate an RPC service from a real `.proto` file:

```bash
rozectl rpc protoc example/user.proto --out services/user-rpc
```

Generate models:

```bash
rozectl model generate example/user.sql --out services/user-api --format sql
rozectl model inspect users --db-kind mysql --db-url mysql://root:root@127.0.0.1:3306/roze --out services/user-api
rozectl model inspect users --db-kind postgres --db-url postgres://postgres:postgres@127.0.0.1:5432/roze --schema public --out services/user-api
rozectl model generate example/user.model --out services/user-api --format mongo
rozectl model inspect users --db-kind mongo --db-url mongodb://127.0.0.1:27017/roze --out services/user-api
rozectl search generate example/user.search --engine elasticsearch --out services/user-api
rozectl search inspect users --engine meilisearch --url http://127.0.0.1:7700 --out services/user-api
```

Generate deployment files:

```bash
rozectl docker --port 8080 --binary user-api
rozectl kube deploy --name user-api --image registry.example.com/user-api:latest --port 8080
```

## Supported `.api` syntax

The parser accepts these Roze contract forms:

- `syntax = "v1"` declarations.
- `info (...)` blocks.
- `type Name { ... }` and grouped `type (...)` blocks.
- Compact block starts such as `info(`, `type(`, and `@server(`.
- `service name { ... }` REST route blocks.
- `@server`, `@handler`, `@doc`, and `@middleware` annotations.
- `import (...)` blocks.
- Route signatures with either `returns (Resp)` or `returns(Resp)`.
- HTTP methods: `get`, `post`, `put`, `patch`, `delete`.

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

  @handler logout
  post /logout
}
```

## Route-scoped `@server`

Multiple `@server` blocks inside a service apply to following routes until the
next `@server` block. Route-scoped values override the top-level server block
for path prefix, middleware, and JWT/OpenAPI security.

```go
@server (
  prefix: /api
)
service user-api {
  @server (
    prefix: /api/v1
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
`--update`.

See [Middleware Contract](../contracts/middleware.md) for the complete alias
table and adaptive shedding behavior.

`timeout: true` makes generated route glue apply the service-wide
`governance.timeout_ms` through Roze middleware. Generated handler adapters also
enforce route-specific timeout overrides from `governance.routes`. Set
`timeout: false` when you only want timeout metadata propagated through
`roze_context::Context` and do not want generated HTTP adapters to cancel
long-running logic.

Business logic should not pass or construct `trace_id` values. Use
`tracing::info!`, `tracing::warn!`, and `tracing::error!` directly in
`src/logic/**`; the request Span created by Roze middleware carries the
`trace_id`. Use `ServiceContext` for global resources and Axum `Extension<T>`
for per-request user/session objects injected by custom middleware.

`cors: true` enables CORS. Without `cors_config`, generated services use a
permissive development default. Add `cors_config` to restrict browser
origins, methods, request headers, exposed response headers, credentials, and
preflight max age.

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
| `uint` | `u64` |
| `bool` | `bool` |
| `i32`, `i64`, `u32`, `u64`, `f32`, `f64` | same Rust type |
| custom type | component/reference type |

Container types:

| `.api` type | Generated Rust type |
| --- | --- |
| `[]string` | `Vec<String>` |
| `[]int` | `Vec<i64>` |
| `[]T` | `Vec<T>` |
| `Vec<T>` | `Vec<T>` |
| `map[string]string` | `std::collections::HashMap<String, String>` |
| `map[string]int` | `std::collections::HashMap<String, i64>` |
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
`alpha`, `alphanum`, `ascii`, `numeric`, `lowercase`, `uppercase`, `gte`,
`lte`, `gt`, and `lt`.

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

Framework-owned files can be regenerated with `--update`. Business code should
live in REST `src/logic/<group>/<method>.rs` or RPC
`src/logic/<method>.rs`. REST custom middleware lives in
`src/middleware/<custom>.rs`. These application-owned files and `config.yaml`
are preserved on `--update`, while generated boundary files keep HTTP/RPC
parsing, validation, context extraction, errors, tracing, and response
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
```

Database inspection:

```bash
rozectl model inspect users --db-kind mysql --db-url mysql://root:root@127.0.0.1:3306/roze --out services/user-api
rozectl model inspect users --db-kind postgres --db-url postgres://postgres:postgres@127.0.0.1:5432/roze --schema public --out services/user-api
rozectl model inspect users --db-kind mysql --db-url mysql://root:root@127.0.0.1:3306/roze --out services/user-api --orm sea-orm
rozectl model inspect users --db-kind mongo --db-url mongodb://127.0.0.1:27017/roze --sample-size 100 --out services/user-api
```

Toasty is the default SQL ORM. `--orm sea-orm` switches SQL/DSL/inspection
output to SeaORM-style modules.
Mongo inspection samples collection documents, maps `_id` to `id`, and emits
Mongo repository modules. It preserves Mongo index metadata, emits find helpers
for single-field unique indexes, emits compound-index find/list helpers, and
still emits an `id: ObjectId` model for empty collections. `--orm` does not
affect Mongo output.

Generated SQL repositories include single-table CRUD helpers. Toasty and SeaORM
outputs both generate primary-key lookup, cache-key lookup, `list`, `insert`,
`update`, `delete_by_<primary>`, and `count` methods.

SQL repositories additionally generate:

- `<model>_fields.rs` files with table-name constants, `{Model}Field` enums,
  and field-name constants separated from repository logic
- `<model>_ext.rs` application-owned extension files for custom model or
  repository methods; these files are created once and preserved during
  `--update`
- `{Model}Query`, `{Model}SortField`, and `{Model}Page` structs for paginated
  single-table queries
- `query` and `list_page` helpers with equality, `IN`, numeric min/max range,
  nullable equality, `IS NULL`, and typed sort fields for non-null columns
- Toasty query generation counts with the filter-only query and applies
  `ORDER BY`, `LIMIT`, and `OFFSET` only to the list query, avoiding invalid
  PostgreSQL count SQL when a sort field is present
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

SQL generation infers soft-delete columns from `deleted`, `is_deleted`,
`deleted_at`, `delete_time`, or `deleted_at_millis`, and tenant columns from
`tenant_id`, `org_id`, or `account_id`. DSL models can override this with
`soft_delete: <field>` and `tenant: <field>`.

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

Model generation treats `mod.rs`, `<model>.rs`, and `<model>_fields.rs` as
schema-owned generated files. Re-running with `--update` refreshes them from the
current DSL, SQL, or inspected schema. Put handwritten model helpers and custom
repository queries in `<model>_ext.rs`; `--update` preserves existing extension
files, while `--force` rewrites them. During `--update`, rozectl also removes
stale generated model files that carry the `@generated by rozectl` marker and no
longer correspond to the current schema. Unmarked files and all `*_ext.rs`
files are left in place.

Generated REST, RPC, stream, Toasty, and SeaORM templates have ignored
compile-smoke tests that create temporary crates and run `cargo check` plus
`cargo clippy --all-targets -- -D warnings` where applicable:

```bash
cargo test -p rozectl -- --ignored --skip postgres --skip mysql --skip mongo
```

Mongo model generation uses the standard model generator:

```bash
rozectl model generate example/user.model --out services/user-api --format mongo
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
Dockerfile and validates it before returning success. Because Roze is
pre-release, treat the output as reviewed deployment scaffolding rather than a
complete production certification:

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
  --image registry.example.com/user-api:latest \
  --replicas 2 \
  --port 8080 \
  --cpu-request 100m \
  --memory-request 128Mi \
  --cpu-limit 500m \
  --memory-limit 512Mi \
  --min-replicas 2 \
  --max-replicas 5 \
  --target-cpu 70 \
  --env-file .env \
  --config-map user-api-config \
  --min-available 1
```

`--env KEY=VALUE` entries are validated before writing the manifest.
`--config-map` adds an `envFrom.configMapRef` reference. `--env-file` reads a
dotenv-style file, validates each `KEY=VALUE` line, emits a generated
`<name>-env` ConfigMap, and wires it through `envFrom`.
The manifest always includes a ServiceAccount, PodDisruptionBudget, and
namespace-wide ingress NetworkPolicy for the service port. `--min-available`
controls the PodDisruptionBudget `minAvailable` value.

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

## Helm chart generation

`rozectl helm chart` writes a production-oriented application chart with
`Chart.yaml`, `values.yaml`, and Deployment, Service, HPA, ServiceAccount,
PodDisruptionBudget, and NetworkPolicy templates. It uses the same resource,
probe, autoscaling, image, env, and ConfigMap settings as `kube deploy`.
Review the generated chart against the production checklist before using it in
a real environment.

```bash
rozectl helm chart \
  --name user-api \
  --image registry.example.com/user-api:1.2.3 \
  --replicas 2 \
  --port 8080 \
  --min-replicas 2 \
  --max-replicas 5 \
  --target-cpu 70 \
  --env RUST_LOG=info \
  --config-map user-api-config \
  --min-available 1 \
  --chart-version 0.1.0 \
  --app-version 1.2.3 \
  --out deploy/user-api-chart
```

The Helm chart always includes ServiceAccount, PodDisruptionBudget, and
NetworkPolicy templates. `values.yaml` exposes `serviceAccount.name` and
`podDisruptionBudget.minAvailable` for chart-level customization.

`rozectl helm chart` validates the chart directory before returning success.
Re-run the same offline validation, then optionally render with Helm:

```bash
rozectl helm validate --chart deploy/user-api-chart
helm template user-api deploy/user-api-chart
```

`rozectl helm validate` checks the chart structure without requiring Helm. It
verifies `Chart.yaml`, `values.yaml`, Deployment, Service, HPA, ServiceAccount,
PodDisruptionBudget, NetworkPolicy, and helper templates.

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

## Current limitations

- `dive` currently covers one collection level.
- Map OpenAPI schemas are emitted as generic objects; value constraints are not
  yet projected into OpenAPI `additionalProperties`.
- OpenAPI constraints for `min/max/len/oneof` are not yet emitted.
- Advanced validator tags such as `required_with_all`, `excluded_if`, `uuid`,
  custom validators, nested struct validation, and cross-struct comparison are
  not generated yet.
