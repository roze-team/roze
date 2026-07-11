#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

DURATION="${ROZE_GATEWAY_SOAK_SECONDS:-${1:-300}}"
if [[ ! "$DURATION" =~ ^[1-9][0-9]*$ ]]; then
  echo "gateway soak duration must be a positive integer, got: $DURATION" >&2
  exit 2
fi

STARTED_AT="$(date +%s)"
DEADLINE=$((STARTED_AT + DURATION))
CYCLES=0

echo "running Gateway production soak: seconds=$DURATION"
while (( $(date +%s) < DEADLINE )); do
  cargo test -p roze-gateway
  CYCLES=$((CYCLES + 1))
done

FINISHED_AT="$(date +%s)"
echo "roze_gateway_soak duration_seconds=$((FINISHED_AT - STARTED_AT)) cycles=$CYCLES"
