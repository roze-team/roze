#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

AREA="${1:-}"
DURATION="${2:-}"
OUT="${3:-target/production-evidence/${AREA}-${DURATION}}"

case "$AREA" in
  gateway|mq|config-center|lifecycle|generated-systems) ;;
  *) echo "unsupported soak area: $AREA" >&2; exit 2 ;;
esac
case "$DURATION" in
  24h) SECONDS_REQUIRED=86400 ;;
  72h) SECONDS_REQUIRED=259200 ;;
  *) echo "duration must be 24h or 72h" >&2; exit 2 ;;
esac

mkdir -p "$OUT"
STARTED_AT="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
STARTED_EPOCH="$(date +%s)"
REVISION="$(git rev-parse HEAD)"

printf '{"schema_version":1,"area":"%s","duration":"%s","revision":"%s","started_at":"%s"}\n' \
  "$AREA" "$DURATION" "$REVISION" "$STARTED_AT" >"$OUT/run.json"

sample_host() {
  while true; do
    now="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
    if [[ -r /proc/loadavg && -r /proc/meminfo ]]; then
      load="$(cut -d' ' -f1-3 /proc/loadavg)"
      memory_kib="$(awk '/MemAvailable:/ { print $2 }' /proc/meminfo)"
      tasks="$(ps -e --no-headers 2>/dev/null | wc -l | tr -d ' ')"
      printf '{"ts":"%s","load":"%s","memory_available_kib":%s,"tasks":%s}\n' \
        "$now" "$load" "${memory_kib:-0}" "${tasks:-0}" >>"$OUT/host.jsonl"
    fi
    sleep 30
  done
}

sample_host &
SAMPLER_PID=$!
cleanup() {
  kill "$SAMPLER_PID" 2>/dev/null || true
  wait "$SAMPLER_PID" 2>/dev/null || true
}
trap cleanup EXIT

case "$AREA" in
  gateway) bash scripts/production-soak-gateway.sh "$SECONDS_REQUIRED" ;;
  mq) bash scripts/production-soak-mq.sh "$SECONDS_REQUIRED" ;;
  config-center) bash scripts/production-soak-config-center.sh "$SECONDS_REQUIRED" ;;
  lifecycle) bash scripts/production-soak-lifecycle.sh "$SECONDS_REQUIRED" ;;
  generated-systems) bash scripts/production-soak-generated-systems.sh "$SECONDS_REQUIRED" ;;
esac 2>&1 | tee "$OUT/workload.log"

FINISHED_EPOCH="$(date +%s)"
ELAPSED=$((FINISHED_EPOCH - STARTED_EPOCH))
if (( ELAPSED < SECONDS_REQUIRED )); then
  echo "soak ended early: required=$SECONDS_REQUIRED actual=$ELAPSED" >&2
  exit 1
fi

sha256sum "$OUT"/* >"$OUT/SHA256SUMS"
printf '# Roze Production Soak Evidence\n\n- Area: `%s`\n- Duration: `%s`\n- Elapsed seconds: `%s`\n- Revision: `%s`\n- Started: `%s`\n- Finished: `%s`\n- Artifact checksums: `SHA256SUMS`\n' \
  "$AREA" "$DURATION" "$ELAPSED" "$REVISION" "$STARTED_AT" \
  "$(date -u +%Y-%m-%dT%H:%M:%SZ)" >"$OUT/summary.md"
