#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

DURATION="${ROZE_LIFECYCLE_SOAK_SECONDS:-${1:-300}}"
CYCLES="${ROZE_LIFECYCLE_SOAK_CYCLES:-1000000}"

export ROZE_LIFECYCLE_SOAK_SECONDS="$DURATION"
export ROZE_LIFECYCLE_SOAK_CYCLES="$CYCLES"

echo "running Lifecycle production soak: seconds=$ROZE_LIFECYCLE_SOAK_SECONDS cycles=$ROZE_LIFECYCLE_SOAK_CYCLES"
cargo test -p roze-service production_soak_service_group_lifecycle -- --ignored --nocapture
