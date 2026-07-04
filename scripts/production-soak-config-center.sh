#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

DURATION="${ROZE_CONFIG_CENTER_SOAK_SECONDS:-${1:-300}}"
UPDATES="${ROZE_CONFIG_CENTER_SOAK_UPDATES:-100000}"

export ROZE_CONFIG_CENTER_SOAK_SECONDS="$DURATION"
export ROZE_CONFIG_CENTER_SOAK_UPDATES="$UPDATES"

echo "running Config Center production soak: seconds=$ROZE_CONFIG_CENTER_SOAK_SECONDS updates=$ROZE_CONFIG_CENTER_SOAK_UPDATES"
cargo test -p roze-config production_soak_admin_store_validation_rollback_and_snapshot -- --ignored --nocapture
