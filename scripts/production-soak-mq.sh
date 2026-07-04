#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

DURATION="${ROZE_MQ_SOAK_SECONDS:-${1:-300}}"
MESSAGES="${ROZE_MQ_SOAK_MESSAGES:-100000}"

export ROZE_MQ_SOAK_SECONDS="$DURATION"
export ROZE_MQ_SOAK_MESSAGES="$MESSAGES"

echo "running MQ production soak: seconds=$ROZE_MQ_SOAK_SECONDS messages=$ROZE_MQ_SOAK_MESSAGES"
cargo test -p roze-mq production_soak_in_memory_broker -- --ignored --nocapture
