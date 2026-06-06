# Roze

Roze is a small Rust service framework scaffold with:

- `rozectl`: code generation for `.api` service definitions.
- `roze-core`: lightweight wrappers around REST, RPC, registry, and middleware primitives.
- `example/user-service`: a generated example service from `example/user.api`.

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
cargo run -p rozectl -- generate example/user.api --out example/user-service --force
cargo run -p user-api-service
```

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
