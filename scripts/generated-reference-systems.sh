#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

cargo test -p rozectl generated_production_reference_systems_compile -- --ignored --nocapture

echo "generated production reference systems passed"
