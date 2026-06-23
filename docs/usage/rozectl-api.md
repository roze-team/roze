# rozectl API generator

`rozectl api generate` reads a go-zero/goctl-style `.api` file and generates a
Rust-native Axum REST service. The generated project keeps framework-owned files
separate from application logic so repeated generation can preserve
method-level logic, custom middleware, and `config.yaml` when `--update` is
used.

Think of `rozectl` as a generator for project structure and integration code.
It can generate API layers, RPC layers, model scaffolds, documentation, client
code, and deployment files. Business logic is still written by developers:
handler-driven logic, complex SQL, domain checks, transactions, authorization,
permissions, and other product-specific behavior live in the application.

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

The goctl-compatible alias is also supported:

```bash
rozectl api go -api example/user.api -dir apps/roze-example --roze-source path
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
ownership rules, so business-owned logic files and preserved config are not
reported as modified unless generation would actually change them.

Check the local development environment:

```bash
rozectl doctor --config apps/roze-example/config.yaml --port 3000
rozectl doctor --tcp 127.0.0.1:6379 --tcp 127.0.0.1:9092
rozectl doctor --tool helm --tool etcdctl
```

`rozectl doctor` checks the default local tools `rustc`, `cargo`, `docker`, and
`kubectl`. Extra `--tool` values are checked with `--version`. `--config`
verifies that a config file exists, and each `--port` verifies that the port is
available on `127.0.0.1`. Each `--tcp host:port` verifies that a dependency
endpoint is reachable with a TCP connection, which is enough for local Redis,
Kafka, NATS, etcd, Consul, or database smoke checks. Missing optional tools are
reported as `WARN`; missing config files, unavailable ports, or unreachable TCP
targets are reported as `FAIL` and return a non-zero exit code.

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

The default readiness and startup handlers report OK until dependency-specific
checks are wired by the application or future lifecycle helpers.

Generate client SDKs:

```bash
rozectl api client ts example/user.api --out sdk/user.ts
rozectl api client js example/user.api --out sdk/user.js
rozectl api client dart example/user.api --out sdk/user.dart
```

goctl-compatible client aliases:

```bash
rozectl api ts -api example/user.api -dir sdk
rozectl api dart -api example/user.api -dir sdk
```

Generate an OpenAPI 3 document:

```bash
rozectl openapi generate example/user.api --out openapi.json
rozectl api swagger -api example/user.api -dir docs/openapi --format json
rozectl api swagger -api example/user.api -dir docs/openapi --format yaml
```

Generate Markdown API documentation:

```bash
rozectl api doc -api example/user.api -dir . -o docs/api
```

Run a custom API plugin:

```bash
rozectl api plugin -p ./tools/rozectl-plugin.sh -api example/user.api -dir generated
```

Generate an RPC service from a real `.proto` file:

```bash
rozectl rpc protoc example/user.proto --zrpc_out services/user-rpc
```

Generate models:

```bash
rozectl model mysql ddl -src example/user.sql -dir services/user-api
rozectl model mysql datasource -url mysql://root:root@127.0.0.1:3306/roze -table users -dir services/user-api
rozectl model pg datasource -url postgres://postgres:postgres@127.0.0.1:5432/roze -schema public -table users -dir services/user-api
rozectl model mongo --type User -dir services/user-api
```

Generate deployment files:

```bash
rozectl docker -go main.go --port 8080 --binary user-api
rozectl kube deploy --name user-api --image registry.example.com/user-api:latest --port 8080
```

See [goctl compatibility](./rozectl-goctl-compat.md) for a direct command
mapping table.

## Supported `.api` syntax

The parser accepts these goctl-compatible forms:

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

`cors: true` enables CORS. Without `cors_config`, generated services keep the
compatibility default of permissive CORS. Add `cors_config` to restrict browser
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
segments such as `:id`; otherwise fields default to JSON body fields.

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

`rozectl openapi generate`, `rozectl api swagger`, and generated REST services
expose OpenAPI data with:

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

`rozectl api swagger` writes `swagger.json` by default and `swagger.yaml` when
`--format yaml` is used.

## RPC proto generation

`rozectl rpc protoc` accepts proto3 source directly. It parses package,
message, field, service, and `rpc` method declarations, then generates a
Rust-native tonic service project:

```text
Cargo.toml
build.rs
config.yaml
proto/service.proto
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

The model generator supports the original Roze commands and goctl-compatible
aliases.

SQL DDL:

```bash
rozectl model generate example/user.sql --out services/user-api --format sql
rozectl model mysql ddl -src example/user.sql -dir services/user-api
rozectl model generate example/user.sql --out services/user-api --format sql --orm sea-orm
```

Database inspection:

```bash
rozectl model inspect users --db-kind mysql --db-url mysql://root:root@127.0.0.1:3306/roze --out services/user-api
rozectl model mysql datasource -url mysql://root:root@127.0.0.1:3306/roze -table users -dir services/user-api
rozectl model pg datasource -url postgres://postgres:postgres@127.0.0.1:5432/roze -schema public -table users -dir services/user-api
rozectl model mysql datasource -url mysql://root:root@127.0.0.1:3306/roze -table users -dir services/user-api --orm sea-orm
```

Toasty is the default SQL ORM. `--orm sea-orm` switches SQL/DSL/inspection
output to SeaORM-style modules.
Mongo generation is separate and is not affected by `--orm`.

Generated SQL repositories include single-table CRUD helpers. Toasty and SeaORM
outputs both generate primary-key lookup, cache-key lookup, `list`, `insert`,
`update`, `delete_by_<primary>`, and `count` methods.

SQL repositories additionally generate:

- `{Model}Query`, `{Model}SortField`, and `{Model}Page` structs for paginated
  single-table queries
- `query` and `list_page` helpers with equality, `IN`, numeric min/max range,
  and typed sort fields for non-null columns
- `insert_many` and `delete_many_by_ids` batch helpers; Toasty uses safe
  per-row repository calls, SeaORM uses set-based ORM calls
- soft-delete scopes and `soft_delete_by_<primary>` helpers when a soft-delete
  column is configured or inferred
- tenant-scoped `find_by_<primary>_for_<tenant>` helpers when a tenant column is
  configured or inferred

SQL generation infers soft-delete columns from `deleted`, `is_deleted`,
`deleted_at`, `delete_time`, or `deleted_at_millis`, and tenant columns from
`tenant_id`, `org_id`, or `account_id`. DSL models can override this with
`soft_delete: <field>` and `tenant: <field>`. Transaction templates remain
application- or extension-owned for now.

Mongo model generation does not require a DSL file:

```bash
rozectl model mongo --type User -dir services/user-api
```

The Mongo output creates a Rust module with a model type, repository, typed CRUD
helpers, cache helper stubs, and common error helpers.

## Dockerfile generation

`rozectl docker -go main.go` is a compatibility command. Roze services are
Rust-native, so the generated file is a multi-stage Rust build:

```bash
rozectl docker -go main.go \
  --builder-image rust:1.87-bookworm \
  --base-image debian:bookworm-slim \
  --binary user-api \
  --port 8080 \
  --timezone Asia/Shanghai \
  --out Dockerfile
```

The `-go` value is accepted for goctl command shape compatibility; the runtime
binary is controlled by `--binary`.

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
  --config-map user-api-config
```

`--env KEY=VALUE` entries are validated before writing the manifest.
`--config-map` adds an `envFrom.configMapRef` reference. `--env-file` reads a
dotenv-style file, validates each `KEY=VALUE` line, emits a generated
`<name>-env` ConfigMap, and wires it through `envFrom`.

Generated probes target the standard service endpoints:

- liveness: `/healthz`
- readiness: `/readyz`
- startup: `/startupz`

## Plugin contract

`rozectl api plugin` parses `.api` once, serializes the normalized API spec to
JSON, then runs the plugin command.

```bash
rozectl api plugin -p ./tools/api-plugin.sh -api example/user.api -dir generated
```

The plugin receives the same JSON payload through stdin and environment:

| Name | Value |
| --- | --- |
| `ROZECTL_API_SPEC_JSON` | normalized API spec JSON |
| `ROZECTL_API_FILE` | source `.api` path |
| `ROZECTL_OUT_DIR` | output directory passed by `-dir` |

The plugin owns any files it writes under `ROZECTL_OUT_DIR`.

## Current limitations

- `dive` currently covers one collection level.
- Map OpenAPI schemas are emitted as generic objects; value constraints are not
  yet projected into OpenAPI `additionalProperties`.
- OpenAPI constraints for `min/max/len/oneof` are not yet emitted.
- Advanced validator tags such as `required_with_all`, `excluded_if`, `uuid`,
  custom validators, nested struct validation, and cross-struct comparison are
  not generated yet.
