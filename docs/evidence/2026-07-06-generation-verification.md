# Roze Generation Verification

Target repo: <https://github.com/roze-team/roze.git>

## Last Verified

```text
roze git dependency: regenerated after user updated roze on 2026-07-06
roze locked commit after regeneration: e7c336d4
local Roze generator fixes after regeneration: see latest repository commit
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
- Fixed generated OpenAPI schema helpers so empty schema property maps are not
  emitted as mutable bindings.
- Locked generated Toasty and SeaORM pagination so `count()` uses a filter-only
  query before applying generated sort, limit, and offset to the page query.
- Verified `shop-catalog-rpc` and `shop-admin-api` with `cargo check`.

## Resolved Issues

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

### OpenAPI generator emits unused `mut` for empty schema properties

Observed after regenerating `shop-admin-api` with:

```powershell
powershell -ExecutionPolicy Bypass -File scripts\generate-roze.ps1
```

`cargo check --manifest-path services\shop-admin-api\Cargo.toml` succeeded, but
Rust reported:

```text
warning: variable does not need to be mutable
   --> src\openapi\mod.rs:430:13
430 |         let mut properties = BTreeMap::new();
```

The generated code appeared when a schema helper had no inserted properties:

```rust
fn upload_token_req_schema() -> Schema {
    let mut properties = BTreeMap::new();
    Schema::object(properties)
}
```

Expected Roze behavior:

- Generate `let properties = BTreeMap::new();` when no fields are inserted.
- Emit `let mut properties` only when generated code subsequently calls
  `properties.insert(...)`.
- Keep generated OpenAPI modules warning-clean under default Rust warnings.

Roze fix:

- Empty generated OpenAPI component schemas now use
  `let properties = BTreeMap::new();`.
- Non-empty generated OpenAPI component schemas still use
  `let mut properties = BTreeMap::new();` before inserting fields.

### Generated SQL pagination with generated sort can leak sort into count

The first observed runtime failure involved Toasty/Postgres pagination with a
generated sort field.

#### Toasty/Postgres observation

Observed through deployed `shop-admin-api` calling `shop-catalog-rpc` by etcd
service discovery on `192.168.1.166`.

Service discovery was successful:

```text
key:   /roze/services/shop-catalog-rpc/127.0.0.1:18800
value: {"name":"shop-catalog-rpc","addr":"127.0.0.1:18800","weight":1,"metadata":{}}
```

The API reached RPC, but RPC returned a database error:

```text
db error: ERROR: column "tbl_0_0.sort_order" must appear in the GROUP BY clause or be used in an aggregate function
```

The deployed server still had app code setting generated sort fields:

```rust
sort_by: Some(ProductSortField::SortOrder)
```

Expected Roze behavior:

- Generated repository `count()` queries should strip generated `sort_by`,
  `ORDER BY`, `LIMIT`, and `OFFSET`.
- Generated pagination SQL should remain valid on PostgreSQL when generated
  `sort_by` fields are set.
- Toasty and SeaORM generation should both count filter-only queries before
  applying generated sort and page bounds.
- Add regression tests for generated model repository pagination with
  `sort_by`.

Roze fix:

- Generated Toasty repositories calculate `total` with
  `Self::build_filter_query(&req).count().exec(db).await?`.
- Generated Toasty repositories apply `sort_by`, `limit`, and `offset` only to
  the page query created by `Self::build_query(&req)`.
- Generated SeaORM repositories calculate `total` with
  `select.clone().count(db).await?` before generated `order_by_*` calls and
  paginator creation.
- A regression test now covers a generated `products.sort_order` sort field and
  asserts Toasty count happens before the sorted page query.
- A SeaORM regression assertion verifies generated count happens before
  generated `order_by_*` and no longer uses `paginator.num_items()` after sort.

Deployment note:

- External mall services must regenerate and redeploy from the fixed Roze
  revision before the deployed `shop-admin-api` to `shop-catalog-rpc` path can
  prove the runtime issue is gone.

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
