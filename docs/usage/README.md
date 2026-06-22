# Usage Documentation

This folder contains user-facing guides for running Roze tools and generated
services.

## Guides

- [Project standards](../project-standards.md): repository layout, API/RPC
  project boundaries, generated file ownership, runtime contracts, metrics, and
  verification rules.
- [Roze vs go-zero comparison](../go-zero-comparison.md): current parity
  matrix, intentional differences, and remaining gaps.
- [Middleware contract](../contracts/middleware.md): REST middleware config,
  go-zero-compatible built-in names, adaptive shedding, and generated file
  ownership.
- [rozectl API generator](./rozectl-api.md): `.api` syntax, REST generation,
  client SDKs, OpenAPI output, type mapping, and validator tag support.
- [rozectl goctl compatibility](./rozectl-goctl-compat.md): direct mapping from
  common `goctl` commands to Rust-native `rozectl` commands.

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
cargo test -p rozectl -- --skip postgres --skip mysql
cargo test --workspace
```
