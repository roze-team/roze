#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

SECONDS_REQUIRED="${1:-}"
case "$SECONDS_REQUIRED" in
  ''|*[!0-9]*) echo "duration must be a positive number of seconds" >&2; exit 2 ;;
  0) echo "duration must be greater than zero" >&2; exit 2 ;;
esac

STARTED="$(date +%s)"
DEADLINE=$((STARTED + SECONDS_REQUIRED))
ITERATION=0
ITERATION_LATENCIES="$(mktemp)"

cleanup() {
  if [[ -n "${ROZE_REFERENCE_COMPOSE_PROJECT:-}" ]]; then
    docker compose -p "$ROZE_REFERENCE_COMPOSE_PROJECT" \
      -f "${ROZE_REFERENCE_COMPOSE_FILE:-docker-compose.integration.yml}" \
      down --remove-orphans || true
  else
    docker compose -f docker-compose.integration.yml down --remove-orphans || true
  fi
  rm -f "$ITERATION_LATENCIES"
}
trap cleanup EXIT

while (( $(date +%s) < DEADLINE )); do
  ITERATION=$((ITERATION + 1))
  ITERATION_STARTED_NS="$(date +%s%N)"
  printf 'generated-systems iteration=%s started_at=%s\n' \
    "$ITERATION" "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  bash scripts/reference-systems-integration.sh
  bash scripts/production-smoke.sh --skip-generated
  ITERATION_FINISHED_NS="$(date +%s%N)"
  ITERATION_MILLIS=$(((ITERATION_FINISHED_NS - ITERATION_STARTED_NS) / 1000000))
  printf '%s\n' "$ITERATION_MILLIS" >>"$ITERATION_LATENCIES"
  printf 'generated-systems iteration=%s finished_at=%s\n' \
    "$ITERATION" "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
done

ELAPSED=$(( $(date +%s) - STARTED ))
if (( ELAPSED < SECONDS_REQUIRED )); then
  echo "generated systems soak ended early: required=$SECONDS_REQUIRED actual=$ELAPSED" >&2
  exit 1
fi
if (( ITERATION == 0 || ELAPSED == 0 )); then
  echo "generated systems soak produced no measurable iteration" >&2
  exit 1
fi

percentile() {
  local percent="$1"
  sort -n "$ITERATION_LATENCIES" |
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
ITERATIONS_PER_HOUR=$((ITERATION * 3600 / ELAPSED))

printf 'roze_generated_systems_soak iterations=%s elapsed_seconds=%s iterations_per_hour=%s p50_iteration_ms=%s p95_iteration_ms=%s p99_iteration_ms=%s\n' \
  "$ITERATION" "$ELAPSED" "$ITERATIONS_PER_HOUR" \
  "$P50_MILLIS" "$P95_MILLIS" "$P99_MILLIS"
