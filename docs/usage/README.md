# Usage Documentation

This folder contains user-facing guides for running Roze tools and generated
services.

## Guides

- [Project standards](../project-standards.md): repository layout, API/RPC
  project boundaries, generated file ownership, runtime contracts, metrics, and
  verification rules.
- [rozectl API generator](./rozectl-api.md): `.api` syntax, REST generation,
  client SDKs, OpenAPI output, type mapping, and validator tag support.
- [rozectl goctl compatibility](./rozectl-goctl-compat.md): direct mapping from
  common `goctl` commands to Rust-native `rozectl` commands.

## Verification

The documentation in this folder should stay aligned with the generator tests:

```bash
cargo test -p rozectl -- --skip postgres --skip mysql
cargo test --workspace
```
