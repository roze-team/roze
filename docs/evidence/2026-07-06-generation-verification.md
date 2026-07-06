# Roze Generation Verification

Target repo: <https://github.com/roze-team/roze.git>

## Last Verified

```text
roze git dependency: regenerated after user updated roze on 2026-07-06
roze locked commit after regeneration: 14833e22
rozectl --version: 0.1.0
OS: Windows / PowerShell
Rust: stable toolchain from local cargo
Project: multi-service mall generated from .api, .proto, and SQL schemas
Runtime registry check: shop-catalog-rpc registered in etcd on 2026-07-04
```

## Current Status

No unresolved Roze generation issues are currently blocking this project.

## Verification Notes

- Re-ran model/API/RPC generation with the reinstalled `rozectl`.
- Switched local generation script API calls from `--force` to `--update` to
  preserve app-owned logic.
- Updated generated service `Cargo.lock` files so Roze runtime crates match the
  new generator output.
- Verified `shop-catalog-rpc` and `shop-admin-api` with `cargo check`.

## Operational Note

After reinstalling `rozectl` from a newer Roze commit, run:

```powershell
cargo update --manifest-path <service>\Cargo.toml -p roze-rpc -p roze-config
```

for regenerated services. Otherwise generated code may call newer runtime APIs,
such as `ServiceRegistrationGuard::start_with_advertise_addr` or
`rpc.advertise_addr`, while the service lockfile still points at an older Roze
commit.

## Verification

Regeneration command:

```powershell
powershell -ExecutionPolicy Bypass -File scripts\generate-roze.ps1
```

Compile checks passed:

```powershell
cargo check --manifest-path services\shop-system-rpc\Cargo.toml
cargo check --manifest-path services\shop-payment-rpc\Cargo.toml
cargo check --manifest-path services\shop-promotion-rpc\Cargo.toml
cargo check --manifest-path services\shop-fulfillment-rpc\Cargo.toml
cargo check --manifest-path services\shop-admin-api\Cargo.toml
cargo check --manifest-path services\shop-app-api\Cargo.toml
```

Spot checks:

```rust
// services/shop-fulfillment-rpc/src/logic/create_aftersales.rs
Ok(AftersalesOrder::default())
```

```rust
// services/shop-fulfillment-rpc/src/model/aftersales_order.rs
pub r#type: String,
pub r#type: Option<String>,
```

## Evidence Boundary

This report verifies generation and compile behavior for one regenerated
multi-service project. It supports internal pilot confidence in `rozectl`, but
it is not a 24h/72h production-stability report and does not by itself promote
any module to `stable`.
