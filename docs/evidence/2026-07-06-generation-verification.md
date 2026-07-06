# Roze Generation Verification

Target repo: <https://github.com/roze-team/roze.git>

## Last Verified

```text
roze git dependency: regenerated after user updated roze on 2026-07-06
roze locked commit after regeneration: 6cc9eb67
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
- Verified `rozectl api generate --update` preserves app-owned service context
  extensions and extra logic module declarations after the Roze fix.
- Verified `shop-catalog-rpc` and `shop-admin-api` with `cargo check`.

## Resolved Issue

### API `--update` overwrites app-owned integration points

Observed on `shop-admin-api` after running:

```powershell
powershell -ExecutionPolicy Bypass -File scripts\generate-roze.ps1
```

Even with `rozectl api generate ... --update`, the generator rewrote:

```text
services/shop-admin-api/src/svc/mod.rs
services/shop-admin-api/src/logic/admin/mod.rs
```

That removed app-owned catalog RPC wiring:

```rust
catalog: Option<shop_catalog_rpc::client::RpcClient>
pub fn catalog(&self) -> anyhow::Result<shop_catalog_rpc::client::RpcClient>
mod catalog_map;
```

Result before restoring those integration points:

```text
no `catalog_map` in `logic::admin`
no method named `catalog` found for struct `svc::ServiceContext`
```

Expected Roze behavior:

- `--update` should preserve application-owned module declarations and
  service-context extension fields/methods, or provide explicit extension
  files/hooks that are not rewritten.
- Generated module indexes should include a safe app-owned section, such as
  marker comments, that the updater preserves.
- Service context customization should have a stable extension point so
  cross-service clients can be wired without editing generated-owned files.

Temporary restoration before the Roze fix:

- Restore `catalog` client wiring in `services/shop-admin-api/src/svc/mod.rs`.
- Restore `mod catalog_map;` in
  `services/shop-admin-api/src/logic/admin/mod.rs`.

Roze fix:

- `src/svc/mod.rs` is preserved during `--update` when it already exists.
- `src/logic/<group>/mod.rs` refreshes generated route logic exports while
  preserving additional app-owned `mod ...;` declarations such as
  `mod catalog_map;`.

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
cargo check --manifest-path services\shop-catalog-rpc\Cargo.toml
cargo check --manifest-path services\shop-admin-api\Cargo.toml
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
