# Roze Generation Requirements

Target repo: <https://github.com/roze-team/roze.git>

## Last Verified

```text
date: 2026-07-07
rozectl --version: 0.1.0
rozectl path: C:\Users\xFc\.cargo\bin\rozectl.exe
regeneration command: powershell -ExecutionPolicy Bypass -File scripts\generate-roze.ps1
project: multi-service mall generated from .api, .proto, and SQL schemas
```

## Current Status

`rozectl` was reinstalled and regeneration was run again on 2026-07-07. API,
RPC, and model generation completes successfully, and every service under
`services/*` passes `cargo check`.

The items below are Roze generator/runtime requirements discovered by downstream
regeneration and smoke verification. Implementation status is tracked per item;
downstream projects should regenerate before removing local workarounds.

## Implementation Summary

Implemented in this checkout:

- PostgreSQL `smallint` / `INT2` maps to `i16` in generated SQL models and
  query/filter surfaces.
- PostgreSQL `NUMERIC` / `DECIMAL` / `money` maps to
  `rust_decimal::Decimal`.
- Generated Toasty services add `rust_decimal` and Toasty's `rust_decimal`
  feature, including update mode when an older inline Toasty dependency already
  exists.
- Generated SeaORM services with decimal model fields add `rust_decimal` and
  SeaORM's `with-rust_decimal` feature.
- RPC update generation preserves app-owned declarations in `src/logic/mod.rs`
  by reusing the same module-index merge strategy used by REST logic groups.
- Generated Toasty repositories already expose `query_with_filter`, which lets
  application code add pre-count and pre-pagination predicates safely.

Not yet closed by this checkout alone:

- PostgreSQL runtime insert/query smoke tests for `smallint` and decimal
  columns.
- Native generated `*_contains` fields for string filters. This remains blocked
  on a stable Toasty LIKE/ILIKE predicate surface or a Roze-owned abstraction
  over it.

## Closure Matrix

| Item | Generator change | Generator tests | Runtime/database smoke | Downstream workaround removable |
| --- | --- | --- | --- | --- |
| P0 `smallint` / `INT2` | done | done | pending | after downstream regeneration and smoke pass |
| P0 `NUMERIC` / `DECIMAL` | done | done | pending | after downstream regeneration and smoke pass |
| P1 RPC logic modules | done | done | not required beyond regenerated compile | after downstream regeneration and compile pass |
| P2 string contains filters | extension hook available; native fields pending | existing hook covered | pending for custom predicate usage | only for endpoints migrated to hook/custom SQL |

## Execution Order

1. Land the generator changes and tests in `roze` / `rozectl`.
2. Reinstall or rebuild `rozectl` from the fixed checkout.
3. Regenerate the downstream mall services with normal `--update` regeneration.
4. Run compile checks for every generated service.
5. Run focused PostgreSQL smoke checks for coupon insert/list paths.
6. Remove downstream workarounds only after the generated repository path passes
   the same smoke checks that originally failed.
7. Keep native string contains filters as a separate follow-up unless a stable
   Toasty or Roze query predicate API is added.

## Downstream Migration Steps

Use this sequence for the mall project after installing the fixed `rozectl`:

```powershell
powershell -ExecutionPolicy Bypass -File scripts\generate-roze.ps1
```

Then inspect the generated promotion RPC model:

```text
services/shop-promotion-rpc/src/model/coupon.rs
```

Expected generated changes:

- `status smallint` becomes `pub status: i16`.
- `discount_amount NUMERIC` and `min_order_amount NUMERIC` become
  `rust_decimal::Decimal` or `Option<rust_decimal::Decimal>`.
- `services/shop-promotion-rpc/Cargo.toml` includes `rust_decimal` and Toasty's
  `rust_decimal` feature.
- SeaORM-generated services with decimal columns include `rust_decimal` and
  SeaORM's `with-rust_decimal` feature.
- `services/shop-promotion-rpc/src/logic/mod.rs` keeps `mod coupon_map;`.
- `services/shop-user-rpc/src/logic/mod.rs` keeps `mod user_map;`.

Do not remove the current SeaORM/raw-SQL coupon workaround until a generated
Toasty repository query has passed the runtime smoke checks below.

## Runtime Smoke Contract

The runtime smoke should prove the exact path that failed before:

```text
shop-admin-api -> service discovery -> shop-promotion-rpc -> CouponRepository::query
```

Minimum smoke cases:

- Create or seed a coupon with `status = 1`, `discount_amount = 5.00`, and
  `min_order_amount = 10.00`.
- Query the coupon through the generated Toasty repository directly inside
  `shop-promotion-rpc`.
- Query the coupon list through `shop-admin-api` using service discovery.
- Confirm the RPC service is reached and no panic occurs in Toasty PostgreSQL
  decode paths.
- Confirm the API response still serializes monetary values according to the
  public contract, even if the repository model uses `rust_decimal::Decimal`.

## Risk And Rollback

Known compatibility risks:

- Mapping `NUMERIC` / `DECIMAL` from `f64` to `rust_decimal::Decimal` is a
  generated Rust type change. Application logic that directly consumed generated
  model fields as `f64` must be updated.
- Mapping `smallint` from `i32` to `i16` is also a generated Rust type change.
  API/RPC DTO conversion code may need explicit widening to `i64` or `i32`.
- Existing generated services with inline Toasty dependencies need the
  `rust_decimal` feature added during update; workspace services need the
  workspace dependency available.
- Existing generated SeaORM services with decimal fields need the
  `with-rust_decimal` feature added during update.

Rollback strategy:

- Keep downstream workaround logic in place until runtime smoke passes.
- If regenerated model type changes break application logic, keep generated
  repository changes and adapt only app-owned mapping code under `src/logic/**`.
- If a production hotfix is needed before downstream migration is complete, keep
  the current raw-SQL workaround and defer generated repository usage.

## Verification Record Template

Fill this in when downstream verification is run:

```text
date:
roze commit:
rozectl path:
rozectl --version:
regeneration command:
services checked:
postgres smoke database:
smallint smoke result:
numeric smoke result:
admin coupon list result:
workarounds removed:
remaining workarounds:
```

## Requirements

### P0: PostgreSQL `smallint` / `INT2` generation must be runtime-safe

Status: implemented in `rozectl` generator tests on 2026-07-07; downstream
database smoke verification is still required after regeneration.

#### Problem

Generated Toasty models currently map PostgreSQL `smallint` / `INT2` columns to
Rust `i32`. Toasty PostgreSQL decoding then panics when it receives an `INT2`
value where the generated repository expects an `i32`.

Observed downstream path:

```text
shop-admin-api -> shop-promotion-rpc -> CouponRepository::query
```

Observed example:

```text
table: coupons
column: status smallint
generated model: pub status: i32
runtime panic: unexpected type for INT2: I32
source: toasty-driver-postgresql/src/value.rs
```

#### Expected Behavior

Roze must generate and execute PostgreSQL `smallint` fields consistently across
all generated model and repository surfaces.

Preferred behavior:

- Map PostgreSQL `smallint` / `INT2` to Rust `i16`.
- Use `i16` consistently in generated model fields, query structs,
  insert/update builders, and comparison filters.
- Preserve API/RPC response mapping behavior separately, so application-facing
  DTOs can still expose wider integer types such as `i64` when the contract
  requires them.

Acceptable alternative:

- If Roze intentionally maps `smallint` to `i32`, generated Toasty PostgreSQL
  binding and decode paths must cast/read `INT2` as `i32` without panicking.
- The behavior must be documented because it differs from the database storage
  width.

#### Implementation Scope

- SQL schema inspection type mapping.
- Generated Toasty model structs.
- Generated query/filter structs.
- Generated insert and update builders.
- Generated repository comparisons and parameter binding.
- Regression fixtures and tests for PostgreSQL `smallint`.

#### Acceptance Criteria

Generator-level:

- A SQL fixture with a `SMALLINT` column generates without manual edits.
- Generated Rust types are consistent across model, query, insert/update, and
  filter surfaces: `SMALLINT` / `INT2` now maps to `i16`.
- Generated code passes `cargo check`.
- A regression test covers the generated model/query/filter types.

Runtime-level:

- Insert plus query against PostgreSQL succeeds without panic.
- The downstream `shop-admin-api -> shop-promotion-rpc -> CouponRepository`
  path can list coupons without the previous Toasty `INT2` decode panic.

#### Downstream Workaround

`services/shop-promotion-rpc/src/logic/list_coupons.rs` uses SeaORM raw SQL,
reads `status` as `i16`, and maps it to the RPC response type.

### P0: PostgreSQL `NUMERIC` / `DECIMAL` generation must be decimal-safe

Status: implemented in `rozectl` generator tests on 2026-07-07; downstream
database smoke verification is still required after regeneration.

#### Problem

Generated Toasty repositories can panic when reading normal PostgreSQL
`NUMERIC` / `DECIMAL` money columns. The downstream project observed generation
to `f64` while generated dependencies did not enable the Toasty decimal support
needed to decode PostgreSQL `NUMERIC`.

Observed example:

```text
table: coupons
columns: discount_amount NUMERIC, min_order_amount NUMERIC
generated model: f64
generated Cargo.toml: toasty features did not include rust_decimal
runtime panic: NUMERIC requires rust_decimal feature to be enabled
```

#### Expected Behavior

Roze must generate PostgreSQL `NUMERIC` / `DECIMAL` fields so normal
`NUMERIC(12,2)` money columns can be inserted and queried without project-side
dependency edits.

Preferred behavior:

- Generate decimal-safe Rust fields, such as `rust_decimal::Decimal`, for
  PostgreSQL `NUMERIC` / `DECIMAL`.
- Enable required dependencies and features automatically in generated
  `Cargo.toml`.
- Avoid floating-point drift for money-oriented schema fields.

Acceptable compatibility behavior:

- If Roze continues to generate `f64`, generated `Cargo.toml` must include all
  Toasty features required to decode PostgreSQL `NUMERIC`.
- The generator should document the precision tradeoff and recommend decimal
  contracts for money fields.

#### Implementation Scope

- SQL schema type mapping for `NUMERIC`, `DECIMAL`, precision, and scale.
- Generated model fields and optional fields.
- Generated insert/update/query/filter values.
- Generated `Cargo.toml` dependency features.
- Repository decode and bind behavior.
- OpenAPI/API/RPC mapping guidance when database decimal fields are exposed as
  strings.

#### Acceptance Criteria

Generator-level:

- A SQL fixture with `NUMERIC(12,2)` and `DECIMAL(12,2)` generates without
  manual edits.
- Generated fields use `rust_decimal::Decimal`.
- Generated dependencies include `rust_decimal` and Toasty's `rust_decimal`
  feature.
- Generated code passes `cargo check`.
- A regression test covers both non-null and nullable decimal columns.

Runtime-level:

- Insert plus query for a value such as `5.00` succeeds without panic.
- Decimal values are not forced through `f64` in generated Toasty model or query
  types.
- The downstream coupon list path can read monetary columns without the previous
  Toasty `NUMERIC` feature panic.

#### Downstream Workaround

`services/shop-promotion-rpc/Cargo.toml` enables Toasty `rust_decimal`.
Coupon-list logic currently casts numeric columns to text in SQL and returns
string monetary values to the API layer.

### P1: `rozectl rpc protoc --update` must preserve custom logic modules

Status: implemented and covered by `rozectl` generator tests on 2026-07-07.

#### Problem

RPC regeneration preserves helper files under `src/logic/**`, but rewrites
`src/logic/mod.rs` and drops custom `mod ...;` declarations required for the
service to compile.

Observed examples:

```text
preserved file: services/shop-user-rpc/src/logic/user_map.rs
required declaration: mod user_map;

preserved file: services/shop-promotion-rpc/src/logic/coupon_map.rs
required declaration: mod coupon_map;
```

#### Expected Behavior

`rozectl rpc protoc --update` must preserve application-owned logic module
declarations or provide a stable extension include that is never overwritten by
regeneration.

Acceptable designs:

- Preserve custom `mod ...;` declarations in `src/logic/mod.rs` during update.
- Generate and include a project-owned extension file such as
  `src/logic/mod_ext.rs` or `src/logic_ext.rs`.
- Use marker sections in generated module indexes so app-owned declarations can
  survive regeneration.

#### Implementation Scope

- RPC generator update mode.
- Logic module index writer.
- Preservation or extension-hook strategy for app-owned modules.
- Regression tests that combine generated logic and project-owned helper
  modules.

#### Acceptance Criteria

Generator-level:

- Generate an RPC service.
- Add `src/logic/foo_map.rs`.
- Add the required module declaration or extension include.
- Run `rozectl rpc protoc --update`.
- The helper module remains reachable from generated logic.
- The regression test verifies `src/logic/mod.rs` keeps app-owned
  declarations such as `mod coupon_map;`.

Downstream-level:

- `cargo check` still passes after regeneration.
- Manual restoration of `mod user_map;` and `mod coupon_map;` is no longer
  required.

#### Downstream Workaround

`mod user_map;` and `mod coupon_map;` are manually restored after regeneration.

### P2: Generated repositories should support string contains filters

Status: partially satisfied by the existing generated `query_with_filter`
extension hook. Native generated `*_contains` fields remain an open
improvement until Toasty exposes a stable LIKE/ILIKE predicate API suitable for
generated code.

Follow-up owner: Roze repository/query abstraction. Do not implement native
contains generation by emitting raw SQL fragments directly from model templates.

#### Problem

Admin list APIs commonly need keyword search across fields such as `username`,
`mobile`, `nickname`, `code`, and `name`. Generated repositories currently
support exact-match string filters only. If applications filter after generated
pagination, `total` becomes inaccurate and pages can be incomplete.

#### Expected Behavior

Generated repositories should support safe pre-pagination string contains
filters.

Preferred behavior:

- Generate optional contains filters for eligible string columns, for example
  `name_contains`, `username_contains`, and `mobile_contains`.
- Apply contains filters before count, sort, limit, and offset.
- For PostgreSQL, generate safe `LIKE` / `ILIKE` predicates with escaped user
  input.

Acceptable alternative:

- Expose a stable repository query extension hook that lets application logic
  add safe custom predicates before count and pagination.
- The generated `query_with_filter` hook satisfies this alternative when the
  application can express the predicate with the underlying query API.

#### Implementation Scope

- Generated query structs.
- Generated repository filter builders.
- PostgreSQL string predicate generation and escaping.
- Count-before-pagination behavior.
- Optional configuration or annotations if contains filters should not be
  generated for every string field.

#### Proposed Native Design

Native contains support should be added only after Roze has one of these stable
building blocks:

- A Toasty field predicate API for escaped `LIKE` / `ILIKE`.
- A Roze repository predicate abstraction that can lower safely to database
  engines.
- A generated extension trait that lets app-owned code add predicates without
  editing generated repositories.

Suggested generated query fields:

```rust
pub name_contains: Option<String>,
pub username_contains: Option<String>,
pub mobile_contains: Option<String>,
```

Suggested behavior:

- Ignore empty strings after trimming.
- Escape `%`, `_`, and the escape character before constructing SQL patterns.
- Use case-insensitive matching only when the target database and collation
  behavior are explicit.
- Apply contains predicates to both count and page queries before generated
  sort, limit, and offset.
- Keep native contains generation opt-in if broad generation for every string
  column creates too much API surface.

#### Acceptance Criteria

Current extension-hook acceptance:

- Generated repositories expose `query_with_filter`.
- Custom predicates are applied to the count query before sort, limit, and
  offset.
- Custom predicates are also applied to the page query before sort, limit, and
  offset.
- `items` and `total` can remain consistent for custom pre-pagination filters.

Future native `*_contains` acceptance:

- A generated model with string fields supports contains filtering.
- Contains filtering happens before pagination.
- `items` and `total` reflect the filtered result set.
- User input containing `%`, `_`, and escape characters is handled safely.
- Regression tests cover both matching and non-matching keyword searches.
- Tests cover empty-string input and mixed wildcard input such as `a%_b`.
- Tests verify count and page queries use the same predicate set.

#### Downstream Workaround

Some admin keyword filters are postponed or require hand-written SQL/custom
repository logic until generated repositories support this.

## Non-Goals

- Do not move business logic into generated handlers, RPC adapters, DTOs, or
  repository glue.
- Do not require downstream services to hand-edit generated files after normal
  `--update` regeneration.
- Do not solve keyword search by filtering after generated pagination.
- Do not require application projects to know Toasty driver internals to avoid
  normal PostgreSQL decode panics.

## Verification Plan

Run generator-level regression tests for each fixed behavior:

```powershell
cargo test -p rozectl generator::
cargo test -p rozectl -- --skip postgres --skip mysql --skip mongo
cargo check -p rozectl
```

Then verify the downstream multi-service project:

```powershell
powershell -ExecutionPolicy Bypass -File scripts\generate-roze.ps1
```

```powershell
$failed = @()
Get-ChildItem services -Directory |
  Where-Object { Test-Path (Join-Path $_.FullName 'Cargo.toml') } |
  ForEach-Object {
    cargo check --quiet --manifest-path (Join-Path $_.FullName 'Cargo.toml')
    if ($LASTEXITCODE -ne 0) { $failed += $_.Name }
  }
$failed
```

Expected result:

```text
API/RPC/model generation succeeds.
All generated services pass cargo check.
PostgreSQL smallint and numeric repository smoke tests pass without panic.
RPC update regeneration preserves app-owned logic helper modules.
String contains filters return correct items and total before pagination when
implemented through query_with_filter or a future native generated contains
filter.
```

Runtime smoke cases still needed before removing downstream workarounds:

- Insert a coupon with `status SMALLINT` and query it through the generated
  Toasty repository.
- Insert a coupon with `NUMERIC(12,2)` monetary values and query it through the
  generated Toasty repository.
- Regenerate RPC services with custom helper modules under `src/logic/**` and
  verify no manual `mod ...;` restoration is needed.
- Exercise one keyword-search endpoint implemented with `query_with_filter` or
  custom repository logic and verify `items` and `total` are filtered before
  pagination.

## Already Verified Fixed Or Not Currently Reproducing

- API validator accepts `.api` scalar `int64` and `[]int64` fields.
- RPC crates include reusable `src/lib.rs` exports for generated `client` and
  `pb` modules.
- `shop-user-rpc` generated server adapter wraps the nested `LoginResponse.user`
  message with `Some(...)`.
- `shop-admin-api` regeneration preserves the project-owned `AdminTokenReq`
  re-export in `src/logic/admin/mod.rs`.
- Generated service `Cargo.toml` files do not contain stale Roze `rev = ...`
  entries.
- Generated model files do not contain Toasty-incompatible
  `serde_json::Value` model fields; remaining `serde_json::Value` usages are
  OpenAPI document values only.
- Generated Toasty repositories use `deleted_at().is_none()` for the default
  soft-delete filter instead of `deleted_at().eq(None::<T>)`.

## Project-Side Fix Kept

### `scripts/generate-roze.ps1` fails fast on `rozectl` errors

PowerShell does not stop automatically when an external executable returns a
non-zero exit code. The downstream generation script wraps every `rozectl` call
with `Invoke-Rozectl`, checks `$LASTEXITCODE`, and throws on failure.

This keeps future failed regeneration attempts from leaving the workspace in a
partially regenerated state.

## Evidence Boundary

This document turns downstream findings into Roze-side requirements,
implementation status, and acceptance criteria. Generator-level fixes are
covered by local `rozectl` tests in this checkout. Runtime/database closure
requires downstream regeneration plus PostgreSQL smoke verification before
project-side workarounds are removed.
