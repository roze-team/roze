#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MODE="${1:---verify}"

case "$MODE" in
  --verify)
    node "$ROOT/scripts/competitive-baseline-verify.js"
    node "$ROOT/scripts/competitive-input-verify.js"
    ;;
  --run)
    PAIR_RUNNER="$ROOT/benchmarks/competitive/runners/pair.sh"
    if [[ ! -f "$PAIR_RUNNER" ]]; then
      echo "competitive pair runner is missing; refusing to emit a report" >&2
      exit 2
    fi
    # Fail closed before either implementation can write evidence. This also
    # rejects stale output files, preventing a partial run from being paired
    # with samples left by an earlier invocation.
    node "$ROOT/scripts/competitive-runner-preflight.js"
    ROZE_COMPETITIVE_REQUIRE_DIGESTS=1 \
      node "$ROOT/scripts/competitive-baseline-verify.js"
    node "$ROOT/scripts/competitive-input-verify.js"
    # A single executor invocation is mandatory: separate implementation runs
    # cannot establish the exclusive adjacent/counterbalanced pairing required
    # by the report verifier.
    bash "$PAIR_RUNNER"
    OUTPUT_DIR="${ROZE_COMPETITIVE_OUTPUT_DIR:-$ROOT/target/competitive}"
    node "$ROOT/scripts/competitive-report-verify.js" \
      "$OUTPUT_DIR/roze.json" \
      "$OUTPUT_DIR/go-zero.json" \
      "$OUTPUT_DIR/report.json"
    ;;
  *)
    echo "usage: $0 [--verify|--run]" >&2
    exit 2
    ;;
esac
