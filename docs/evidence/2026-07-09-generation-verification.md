# Roze Generation Verification

Target repo: <https://github.com/roze-team/roze.git>

## Last Verified

```text
date: 2026-07-09
rozectl --version: 0.1.0
rozectl path: C:\Users\xFc\.cargo\bin\rozectl.exe
roze git revision: 0e3dff9d
regeneration command: powershell -ExecutionPolicy Bypass -File scripts\generate-roze.ps1
project: multi-service mall generated from .api, .proto, and SQL schemas
```

## Current Status

`rozectl` was reinstalled and API/RPC/model regeneration was run again on
2026-07-09. Generation completed successfully with:

```powershell
powershell -ExecutionPolicy Bypass -File scripts\generate-roze.ps1
```

The generated services resolved Roze dependencies to git revision `0e3dff9d`.

Compile verification passed for generated REST/API services and all generated
RPC services:

- `shop-admin-api`
- `shop-app-api`
- `shop-search-api`
- `shop-user-rpc`
- `shop-catalog-rpc`
- `shop-inventory-rpc`
- `shop-cart-rpc`
- `shop-order-rpc`
- `shop-payment-rpc`
- `shop-promotion-rpc`
- `shop-fulfillment-rpc`
- `shop-content-rpc`
- `shop-file-rpc`
- `shop-notify-rpc`
- `shop-system-rpc`

## Open Roze Work Items

No open Roze generator/runtime issues are currently reproduced in this project
after regeneration and dependency update to `0e3dff9d`.

## Notes Kept

### `scripts/generate-roze.ps1` fails fast on `rozectl` errors

PowerShell does not stop automatically when an external executable returns a
non-zero exit code. The downstream script wraps every `rozectl` call with
`Invoke-Rozectl`, checks `$LASTEXITCODE`, and throws on failure.

This keeps future failed regeneration attempts from leaving the downstream
workspace in a partially regenerated state.

## Verification Commands

Regeneration:

```powershell
powershell -ExecutionPolicy Bypass -File scripts\generate-roze.ps1
# passed
```

Dependency update:

```powershell
Get-ChildItem services -Directory |
  Where-Object { Test-Path (Join-Path $_.FullName 'Cargo.toml') } |
  ForEach-Object {
    cargo update --manifest-path (Join-Path $_.FullName 'Cargo.toml')
  }
# passed; Roze dependencies updated to git revision 0e3dff9d
```

Compile verification:

```powershell
$env:CARGO_TARGET_DIR = Join-Path ([System.IO.Path]::GetTempPath()) 'shop-roze-check-20260709-rerun'
$failed = @()
Get-ChildItem services -Directory |
  Where-Object { Test-Path (Join-Path $_.FullName 'Cargo.toml') } |
  ForEach-Object {
    cargo check --quiet --manifest-path (Join-Path $_.FullName 'Cargo.toml')
    if ($LASTEXITCODE -ne 0) { $failed += $_.Name }
  }
# passed: shop-admin-api, shop-app-api, shop-search-api
# passed: all generated RPC services
```

## Evidence Boundary

This report verifies regeneration, dependency update, and compile behavior for
one downstream multi-service mall project. It supports internal pilot confidence
in `rozectl` revision `0e3dff9d`, but it is not a 24h/72h production-stability
report and does not by itself promote any Roze module to `stable`.
