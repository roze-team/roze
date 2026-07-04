#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings

cargo test -p roze-gateway
cargo test -p roze-config config_center
cargo test -p roze-mq
cargo test -p roze-service -p roze-bootstrap -p roze-shutdown

bash scripts/rozectl-smoke.sh

cargo test -p rozectl generated_rest_project_compiles_with_model_and_search -- --ignored
cargo test -p rozectl generated_rpc_project_compiles -- --ignored

bash scripts/production-smoke.sh --skip-generated

echo "release gate passed"
