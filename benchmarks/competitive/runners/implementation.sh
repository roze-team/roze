#!/usr/bin/env bash
set -euo pipefail

IMPLEMENTATION="${1:?implementation is required}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
EXECUTOR="${ROZE_COMPETITIVE_EXECUTOR:-}"
OUTPUT_DIR="${ROZE_COMPETITIVE_OUTPUT_DIR:-$ROOT/target/competitive}"

if [[ "$IMPLEMENTATION" != "roze" && "$IMPLEMENTATION" != "go-zero" ]]; then
  echo "unsupported competitive implementation: $IMPLEMENTATION" >&2
  exit 2
fi
if [[ -z "$EXECUTOR" || ! -x "$EXECUTOR" ]]; then
  echo "ROZE_COMPETITIVE_EXECUTOR must name the shared executable benchmark harness" >&2
  exit 2
fi

mkdir -p "$OUTPUT_DIR"
OUTPUT="$OUTPUT_DIR/$IMPLEMENTATION.json"
"$EXECUTOR" \
  --implementation "$IMPLEMENTATION" \
  --baseline "$ROOT/benchmarks/competitive/baseline.yaml" \
  --workloads "$ROOT/benchmarks/competitive/workloads.json" \
  --output "$OUTPUT"
node "$ROOT/scripts/competitive-sample-verify.js" "$OUTPUT" "$IMPLEMENTATION"
