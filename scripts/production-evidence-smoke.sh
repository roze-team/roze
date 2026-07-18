#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

SMOKE_DIR="${ROZE_EVIDENCE_SMOKE_DIR:-$ROOT/target/production-evidence-smoke}"
rm -rf "$SMOKE_DIR"
mkdir -p "$SMOKE_DIR"
trap 'rm -rf "$SMOKE_DIR"' EXIT

VALID_SUMMARY="roze_lifecycle_soak elapsed_ms=2000 cycles=2 cycles_per_second_milli=1000 p50_cycle_us=100 p95_cycle_us=200 p99_cycle_us=300 failed_task_detections=1 drain_timeout_detections=1 p99_fault_detection_us=500 worker_exits=8 stop_hooks=8 running_snapshots=2 stopped_snapshots=2 max_service_count=4"
VALID_REPORT="$SMOKE_DIR/lifecycle-valid.md"

bash scripts/production-evidence.sh \
  --area lifecycle \
  --duration 24h \
  --workload "start, drain, shutdown, failed task, timeout hooks" \
  --failure-injection "stuck task, signal shutdown, hook timeout" \
  --command "bash scripts/production-soak-lifecycle.sh" \
  --lifecycle-summary "$VALID_SUMMARY" \
  --out "$VALID_REPORT" >/dev/null

grep -F "$VALID_SUMMARY" "$VALID_REPORT" >/dev/null
grep -F "| elapsed_ms | 2000 |" "$VALID_REPORT" >/dev/null
grep -F "| cycles_per_second_milli | 1000 |" "$VALID_REPORT" >/dev/null
grep -F "| p99_cycle_us | 300 |" "$VALID_REPORT" >/dev/null
grep -F "| failed_task_detections | 1 |" "$VALID_REPORT" >/dev/null
grep -F "| drain_timeout_detections | 1 |" "$VALID_REPORT" >/dev/null
grep -F "| worker_exits | 8 |" "$VALID_REPORT" >/dev/null
grep -F "| max_service_count | 4 |" "$VALID_REPORT" >/dev/null

expect_failure() {
  local description="$1"
  shift

  if "$@" >"$SMOKE_DIR/failure.out" 2>"$SMOKE_DIR/failure.err"; then
    echo "expected failure: $description" >&2
    exit 1
  fi
}

expect_failure "lifecycle summary on non-lifecycle area" \
  bash scripts/production-evidence.sh \
    --area gateway \
    --duration 24h \
    --workload "proxy traffic" \
    --failure-injection "timeout" \
    --command "bash scripts/production-soak-gateway.sh" \
    --lifecycle-summary "roze_lifecycle_soak cycles=1" \
    --out "$SMOKE_DIR/non-lifecycle.md"

expect_failure "missing lifecycle summary fields" \
  bash scripts/production-evidence.sh \
    --area lifecycle \
    --duration 24h \
    --workload "start" \
    --failure-injection "stuck task" \
    --command "bash scripts/production-soak-lifecycle.sh" \
    --lifecycle-summary "roze_lifecycle_soak cycles=2 worker_exits=8" \
    --out "$SMOKE_DIR/missing-fields.md"

expect_failure "non-numeric lifecycle summary field" \
  bash scripts/production-evidence.sh \
    --area lifecycle \
    --duration 24h \
    --workload "start" \
    --failure-injection "stuck task" \
    --command "bash scripts/production-soak-lifecycle.sh" \
    --lifecycle-summary "roze_lifecycle_soak elapsed_ms=2000 cycles=two cycles_per_second_milli=1000 p50_cycle_us=100 p95_cycle_us=200 p99_cycle_us=300 failed_task_detections=1 drain_timeout_detections=1 p99_fault_detection_us=500 worker_exits=8 stop_hooks=8 running_snapshots=2 stopped_snapshots=2 max_service_count=4" \
    --out "$SMOKE_DIR/non-numeric.md"

expect_failure "inconsistent lifecycle summary counts" \
  bash scripts/production-evidence.sh \
    --area lifecycle \
    --duration 24h \
    --workload "start" \
    --failure-injection "stuck task" \
    --command "bash scripts/production-soak-lifecycle.sh" \
    --lifecycle-summary "roze_lifecycle_soak elapsed_ms=2000 cycles=2 cycles_per_second_milli=1000 p50_cycle_us=100 p95_cycle_us=200 p99_cycle_us=300 failed_task_detections=1 drain_timeout_detections=1 p99_fault_detection_us=500 worker_exits=7 stop_hooks=8 running_snapshots=2 stopped_snapshots=2 max_service_count=4" \
    --out "$SMOKE_DIR/inconsistent.md"

expect_failure "passing scaffold without a verified CI artifact" \
  bash scripts/production-evidence.sh \
    --area gateway \
    --duration 24h \
    --workload "proxy traffic" \
    --failure-injection "timeout" \
    --command "bash scripts/production-soak-gateway.sh" \
    --verdict pass \
    --out "$SMOKE_DIR/unverified-pass.md"

echo "production evidence smoke passed"
