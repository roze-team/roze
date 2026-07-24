# Usage Documentation

This folder contains user-facing guides for running Roze tools and generated
services.

## Guides

- [Project standards](../project-standards.md): repository layout, API/RPC
  project boundaries, generated file ownership, runtime contracts, metrics, and
  verification rules.
- [Requirements vs current architecture](../requirements-architecture-comparison.md):
  maps the 12 core microservice-framework requirements to Roze's current
  modules, gaps, and P0/P1/P2 execution plan.
- [Roadmap](../roadmap.md): prioritized P0/P1/P2 work across release maturity,
  gateway, config center, MQ, governance, generation, docs, and security.
- [Module maturity matrix](../maturity.md): stable contract and independent evidence status
  for each framework area.
- [Stability commitment](../stability-commitment.md): public claim rules,
  stable-module requirements, experimental surface, and MSRV commitment.
- [Production evidence](../production-evidence.md): required long-run,
  failure-injection, and leak-report evidence before long-run runtime claims.
- [Release policy](../release.md): SemVer, MSRV, crates.io/GitHub Release
  expectations, and breaking-change rules.
- [Production checklist](../production-checklist.md): deployment, config,
  governance, MQ, observability, and security checklist.
- [Middleware contract](../contracts/middleware.md): REST middleware config,
  Roze built-in names, adaptive shedding, and generated file ownership.
- [Native HTTP WebSocket contract](../contracts/websocket.md): stable upgrade,
  frame, limits, shutdown, metrics, and `@websocket` generation behavior.
- [Native gRPC routing](../contracts/grpc-routing.md): Roze-owned multi-service
  dispatch without third-party HTTP router dependencies.
- [Client address and trusted proxies](../contracts/client-address.md):
  connection peer injection and fail-closed proxy-chain resolution.
- [Persistent SQL Outbox](../contracts/persistent-outbox.md): PostgreSQL/MySQL
  migrations, transactional enqueue, lease claims, retry, and dead letters.
- [Configuration secrets](../contracts/configuration-secrets.md): environment,
  file, and custom secret providers plus production idempotency selection.
- [rozectl generator](./rozectl-api.md): `.api` syntax, REST/RPC generation,
  SQL/Mongo model generation and inspection, Elasticsearch/OpenSearch/
  Meilisearch search generation and inspection, client SDKs, OpenAPI output,
  Docker/Kubernetes manifests, type mapping, and validator tag support.

## Install rozectl

`rozectl` is the Roze code generator. It is a Rust binary in `apps/rozectl`.

Install the latest version from GitHub:

```bash
cargo install --git https://github.com/roze-team/roze.git rozectl
```

Upgrade or overwrite an existing installation:

```bash
cargo install --git https://github.com/roze-team/roze.git rozectl --force
```

Install from a local checkout:

```bash
git clone https://github.com/roze-team/roze.git
cd roze
cargo install --path apps/rozectl
```

Force reinstall from a local checkout:

```bash
cargo install --path apps/rozectl --force
```

During local framework development, you can run without installing:

```bash
cargo run -p rozectl -- --help
cargo run -p rozectl -- api generate example/user.api --out apps/roze-example --roze-source path
```

Verify the binary:

```bash
rozectl --help
rozectl api --help
rozectl rpc --help
rozectl model --help
rozectl search --help
```

If `rozectl` is not found after installation, make sure Cargo's bin directory is
on `PATH`:

```bash
export PATH="$HOME/.cargo/bin:$PATH"
```

Rust and Cargo are required. Install them with rustup if needed:

```bash
curl https://sh.rustup.rs -sSf | sh
```

## Generated Service Compatibility

Generated REST/RPC service manifests always use `edition = "2021"` directly.
They do not inherit `edition.workspace`, so generated RPC `build.rs` files keep
working when the parent workspace uses Rust 2024.

Generated Toasty dependencies use MySQL/PostgreSQL features by default and do
not enable Toasty sqlite. Roze's SeaORM/sqlx stack still supports sqlite, and
keeping Toasty sqlite disabled avoids `libsqlite3-sys` link conflicts in
generated services that also depend on `roze-db`.

## Verification

The documentation in this folder should stay aligned with the generator tests:

```bash
cargo test -p rozectl -- --skip postgres --skip mysql --skip mongo
cargo test --workspace
```
