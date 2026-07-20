#!/usr/bin/env bash
set -euo pipefail

# The authoritative benchmark must schedule each Roze/go-zero sample as an
# exclusive adjacent pair.  Running the two implementation adapters one after
# another cannot prove that property, so the shared executor owns the schedule
# and emits both raw documents from one invocation.
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
EXECUTOR="${ROZE_COMPETITIVE_EXECUTOR:-}"
OUTPUT_DIR="${ROZE_COMPETITIVE_OUTPUT_DIR:-$ROOT/target/competitive}"

if [[ -z "$EXECUTOR" || ! -x "$EXECUTOR" ]]; then
  echo "ROZE_COMPETITIVE_EXECUTOR must name the shared executable benchmark harness" >&2
  exit 2
fi
mkdir -p "$OUTPUT_DIR"

"$EXECUTOR" \
  --schedule pair \
  --implementations roze,go-zero \
  --baseline "$ROOT/benchmarks/competitive/baseline.yaml" \
  --workloads "$ROOT/benchmarks/competitive/workloads.json" \
  --roze-output "$OUTPUT_DIR/roze.json" \
  --go-zero-output "$OUTPUT_DIR/go-zero.json" \
  --schedule-output "$OUTPUT_DIR/schedule.json"

[[ -s "$OUTPUT_DIR/schedule.json" ]] || {
  echo "shared executor did not emit schedule.json" >&2
  exit 1
}
node "$ROOT/scripts/competitive-sample-verify.js" "$OUTPUT_DIR/roze.json" roze
node "$ROOT/scripts/competitive-sample-verify.js" "$OUTPUT_DIR/go-zero.json" go-zero
node "$ROOT/scripts/competitive-schedule-bind.js" \
  "$OUTPUT_DIR/schedule.json" \
  "$OUTPUT_DIR/roze.json" \
  "$OUTPUT_DIR/go-zero.json" \
  "$ROOT/benchmarks/competitive/workloads.json"
node "$ROOT/scripts/competitive-schedule-verify.js" \
  "$OUTPUT_DIR/schedule.json" \
  "$OUTPUT_DIR/roze.json" \
  "$OUTPUT_DIR/go-zero.json" \
  "$ROOT/benchmarks/competitive/workloads.json"
