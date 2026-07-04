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

No unresolved Roze generation issues are currently tracked for this project.

## Resolved Issue

### API Rust generator mapping API/IDL `int64` to Rust `i64`

Status: fixed and verified after downstream regeneration.

Before the fix, regenerating `shop-admin-api` and `shop-app-api` produced Rust
API code that used `int64` directly in `src/types/mod.rs` and generated handler
request structs.

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

Rust has no built-in `int64` type alias, so the generated API crates failed
with:

```text
error[E0425]: cannot find type `int64` in this scope
```

The fix maps API/IDL `int64` to Rust `i64` in generated shared types and
handler-local request structs. Related scalar mappings are covered by generator
tests, including `int`, `int32`, `uint64`, `float`, and `double`.

Downstream verification:

```powershell
powershell -ExecutionPolicy Bypass -File scripts\generate-roze.ps1
$env:Path = "$env:USERPROFILE\.cargo\bin;$env:Path"
cargo check --manifest-path services\shop-admin-api\Cargo.toml
cargo check --manifest-path services\shop-app-api\Cargo.toml
```

Previously tracked issues have been fixed:

- Empty protobuf message generation.
- Non-empty response stubs using `Ok(Response { })`.
- `toasty_db()` inserted inside `read_db()`.
- `RpcService: Debug` requiring `ServiceContext: Debug`.
- Model generation escaping Rust reserved keywords such as `type`.
- RPC stubs using partial struct initializers instead of `Default::default()`.
- API Rust generator mapping API/IDL `int64` to Rust `i64`.

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
