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
CYCLE_LATENCIES="$(mktemp)"
REQUEST_LATENCIES="$(mktemp)"
REGISTRY_LOG="$(mktemp)"
REGISTRY_PID=""

cleanup() {
  if [[ -n "$REGISTRY_PID" ]]; then
    kill "$REGISTRY_PID" 2>/dev/null || true
    wait "$REGISTRY_PID" 2>/dev/null || true
  fi
  rm -f "$CYCLE_LATENCIES" "$REQUEST_LATENCIES" "$REGISTRY_LOG"
}
trap cleanup EXIT

ROZE_GATEWAY_REGISTRY_RECOVERY_SECONDS="$DURATION" \
  bash scripts/gateway-registry-recovery.sh "$DURATION" >"$REGISTRY_LOG" 2>&1 &
REGISTRY_PID=$!

echo "running Gateway production soak: seconds=$DURATION"
while (( $(date +%s) < DEADLINE )); do
  if ! kill -0 "$REGISTRY_PID" 2>/dev/null; then
    set +e
    wait "$REGISTRY_PID"
    REGISTRY_STATUS=$?
    set -e
    cat "$REGISTRY_LOG"
    if (( REGISTRY_STATUS != 0 )); then
      echo "Gateway registry recovery workload failed early" >&2
      exit "$REGISTRY_STATUS"
    fi
    REGISTRY_PID=""
  fi
  CYCLE_STARTED_NS="$(date +%s%N)"
  ROZE_GATEWAY_SMOKE_METRICS_FILE="$REQUEST_LATENCIES" \
    bash scripts/gateway-smoke.sh
  CYCLE_FINISHED_NS="$(date +%s%N)"
  CYCLE_MILLIS=$(((CYCLE_FINISHED_NS - CYCLE_STARTED_NS) / 1000000))
  printf '%s\n' "$CYCLE_MILLIS" >>"$CYCLE_LATENCIES"
  CYCLES=$((CYCLES + 1))
done

if [[ -n "$REGISTRY_PID" ]]; then
  set +e
  wait "$REGISTRY_PID"
  REGISTRY_STATUS=$?
  set -e
  REGISTRY_PID=""
  cat "$REGISTRY_LOG"
  if (( REGISTRY_STATUS != 0 )); then
    echo "Gateway registry recovery workload failed" >&2
    exit "$REGISTRY_STATUS"
  fi
fi

FINISHED_AT="$(date +%s)"
ELAPSED=$((FINISHED_AT - STARTED_AT))
if (( CYCLES == 0 || ELAPSED < DURATION )) || [[ ! -s "$REQUEST_LATENCIES" ]]; then
  echo "gateway soak ended before completing a measurable cycle and duration" >&2
  exit 1
fi

percentile() {
  local percent="$1"
  sort -n "$CYCLE_LATENCIES" |
    awk -v percent="$percent" '
      { values[NR] = $1 }
      END {
        rank = int((NR * percent + 99) / 100)
        if (rank < 1) rank = 1
        print values[rank] + 0
      }
    '
}

P50_MILLIS="$(percentile 50)"
P95_MILLIS="$(percentile 95)"
P99_MILLIS="$(percentile 99)"
REQUEST_COUNT="$(wc -l <"$REQUEST_LATENCIES" | tr -d ' ')"
request_percentile() {
  local percent="$1"
  sort -n "$REQUEST_LATENCIES" |
    awk -v percent="$percent" '
      { values[NR] = $1 }
      END {
        rank = int((NR * percent + 99) / 100)
        if (rank < 1) rank = 1
        print values[rank] + 0
      }
    '
}
P50_REQUEST_US="$(request_percentile 50)"
P95_REQUEST_US="$(request_percentile 95)"
P99_REQUEST_US="$(request_percentile 99)"
CYCLES_PER_HOUR=$((CYCLES * 3600 / ELAPSED))

REGISTRY_SUMMARY="$(
  grep -E '^roze_gateway_registry_soak ' "$REGISTRY_LOG" | tail -n 1
)"
if [[ -z "$REGISTRY_SUMMARY" ]]; then
  echo "Gateway registry recovery summary is missing" >&2
  exit 1
fi
summary_value() {
  local key="$1"
  awk -v key="$key" '{
    for (i = 1; i <= NF; i++) {
      split($i, pair, "=")
      if (pair[1] == key) {
        print pair[2]
        exit
      }
    }
  }' <<<"$REGISTRY_SUMMARY"
}

printf 'roze_gateway_soak elapsed_seconds=%s cycles=%s cycles_per_hour=%s requests=%s request_errors=0 p50_request_us=%s p95_request_us=%s p99_request_us=%s p50_cycle_ms=%s p95_cycle_ms=%s p99_cycle_ms=%s retry_recoveries=%s timeout_fallbacks=%s config_rejections=%s websocket_checks=%s sse_checks=%s registry_elapsed_seconds=%s registry_fault_injections=%s etcd_attempts=%s etcd_successful_routes=%s etcd_disconnect_observations=%s etcd_recoveries=%s etcd_p99_route_us=%s etcd_p99_recovery_us=%s consul_attempts=%s consul_successful_routes=%s consul_disconnect_observations=%s consul_recoveries=%s consul_p99_route_us=%s consul_p99_recovery_us=%s\n' \
  "$ELAPSED" "$CYCLES" "$CYCLES_PER_HOUR" "$REQUEST_COUNT" \
  "$P50_REQUEST_US" "$P95_REQUEST_US" "$P99_REQUEST_US" \
  "$P50_MILLIS" "$P95_MILLIS" "$P99_MILLIS" "$CYCLES" "$CYCLES" \
  "$CYCLES" "$CYCLES" "$CYCLES" \
  "$(summary_value elapsed_seconds)" "$(summary_value fault_injections)" \
  "$(summary_value etcd_attempts)" "$(summary_value etcd_successful_routes)" \
  "$(summary_value etcd_disconnect_observations)" "$(summary_value etcd_recoveries)" \
  "$(summary_value etcd_p99_route_us)" "$(summary_value etcd_p99_recovery_us)" \
  "$(summary_value consul_attempts)" "$(summary_value consul_successful_routes)" \
  "$(summary_value consul_disconnect_observations)" "$(summary_value consul_recoveries)" \
  "$(summary_value consul_p99_route_us)" "$(summary_value consul_p99_recovery_us)"
