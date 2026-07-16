#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

# Fast, non-ignored coverage for every supported generator plus repeated update
# determinism. Heavy compile checks remain explicit so ordinary unit-test runs
# do not recursively build temporary workspaces.
cargo test -p rozectl supported_generation_matrix_is_deterministic_and_complete

cargo test -p rozectl generated_rest_project_compiles_with_model_and_search -- --ignored
cargo test -p rozectl generated_rpc_project_compiles -- --ignored
cargo test -p rozectl generated_stream_project_compiles -- --ignored
cargo test -p rozectl generated_http_smoke_project_compiles -- --ignored

echo "generated target matrix passed"
