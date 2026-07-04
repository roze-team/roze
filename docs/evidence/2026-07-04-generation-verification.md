# Roze Generation Verification

Target repo: <https://github.com/roze-team/roze.git>

## Last Verified

```text
roze git dependency: regenerated after user updated roze on 2026-07-04
rozectl --version: 0.1.0
OS: Windows / PowerShell
Rust: stable toolchain from local cargo
Project: multi-service mall generated from .api, .proto, and SQL schemas
```

## Current Status

This verification reported one Roze generation issue in generated API crates.
The repository now contains a fix for the Rust API type mapping; downstream
projects should regenerate and rerun the API compile checks to close the
external verification loop.

## Reported Issue

### API Rust generator left `int64` as a Rust type

Status: fixed in this repository; pending downstream regeneration confirmation.

After regenerating `shop-admin-api` and `shop-app-api`, the generated Rust API
code used `int64` directly in `src/types/mod.rs` and generated handler request
structs.

Examples:

```rust
// services/shop-admin-api/src/types/mod.rs
pub struct AdminLoginResp {
    pub token: String,
    pub admin_id: int64,
}
```

```rust
// services/shop-app-api/src/handler/app/list_areas.rs
pub(crate) struct ListAreasAreaListReqQuery {
    parent_id: int64,
}
```

Rust has no built-in `int64` type alias, so the generated API crates failed to
compile with `E0425`.

Reproduction:

```powershell
powershell -ExecutionPolicy Bypass -File scripts\generate-roze.ps1
$env:Path = "$env:USERPROFILE\.cargo\bin;$env:Path"
cargo check --manifest-path services\shop-admin-api\Cargo.toml
cargo check --manifest-path services\shop-app-api\Cargo.toml
```

Observed:

```text
error[E0425]: cannot find type `int64` in this scope
```

Expected:

- Rust API generator maps API/IDL `int64` to Rust `i64`.
- The mapping applies to generated shared types and generated handler-local
  request structs.
- Related integer and number aliases are covered, including `int`, `int32`,
  `uint64`, `float`, and `double`.

Previously tracked issues have been fixed:

- Empty protobuf message generation.
- Non-empty response stubs using `Ok(Response { })`.
- `toasty_db()` inserted inside `read_db()`.
- `RpcService: Debug` requiring `ServiceContext: Debug`.
- Model generation escaping Rust reserved keywords such as `type`.
- RPC stubs using partial struct initializers instead of `Default::default()`.

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
