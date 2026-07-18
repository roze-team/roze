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
      read -r _ cpu_user cpu_nice cpu_system cpu_idle cpu_iowait \
        cpu_irq cpu_softirq cpu_steal _ </proc/stat
      cpu_idle_ticks=$((${cpu_idle:-0} + ${cpu_iowait:-0}))
      cpu_total_ticks=$((
        ${cpu_user:-0} + ${cpu_nice:-0} + ${cpu_system:-0} +
        ${cpu_idle:-0} + ${cpu_iowait:-0} + ${cpu_irq:-0} +
        ${cpu_softirq:-0} + ${cpu_steal:-0}
      ))
      tcp_files=(/proc/net/tcp)
      if [[ -r /proc/net/tcp6 ]]; then
        tcp_files+=(/proc/net/tcp6)
      fi
      tcp_established="$(
        awk 'FNR > 1 && $4 == "01" { count += 1 } END { print count + 0 }' \
          "${tcp_files[@]}"
      )"
      allocated_file_handles="$(awk '{ print $1 }' /proc/sys/fs/file-nr)"
      printf '{"ts":"%s","load":"%s","memory_available_kib":%s,"tasks":%s,"tcp_established":%s,"allocated_file_handles":%s,"cpu_total_ticks":%s,"cpu_idle_ticks":%s}\n' \
        "$now" "$load" "${memory_kib:-0}" "${tasks:-0}" \
        "${tcp_established:-0}" "${allocated_file_handles:-0}" \
        "$cpu_total_ticks" "$cpu_idle_ticks" >>"$OUT/host.jsonl"
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

set +e
case "$AREA" in
  gateway) bash scripts/production-soak-gateway.sh "$SECONDS_REQUIRED" ;;
  mq) bash scripts/production-soak-mq.sh "$SECONDS_REQUIRED" ;;
  config-center) bash scripts/production-soak-config-center.sh "$SECONDS_REQUIRED" ;;
  lifecycle) bash scripts/production-soak-lifecycle.sh "$SECONDS_REQUIRED" ;;
  generated-systems) bash scripts/production-soak-generated-systems.sh "$SECONDS_REQUIRED" ;;
esac 2>&1 | tee "$OUT/workload.log"
WORKLOAD_STATUS=${PIPESTATUS[0]}
set -e

FINISHED_EPOCH="$(date +%s)"
FINISHED_AT="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
ELAPSED=$((FINISHED_EPOCH - STARTED_EPOCH))
cleanup
trap - EXIT

HOST_SAMPLES=0
MIN_MEMORY_AVAILABLE_KIB=0
MAX_TASKS=0
FIRST_MEMORY_AVAILABLE_KIB=0
LAST_MEMORY_AVAILABLE_KIB=0
MEMORY_GROWTH_KIB=0
MAX_TCP_ESTABLISHED=0
MAX_ALLOCATED_FILE_HANDLES=0
CPU_BUSY_BASIS_POINTS=0
if [[ -s "$OUT/host.jsonl" ]]; then
  HOST_SAMPLES="$(wc -l <"$OUT/host.jsonl" | tr -d ' ')"
  MIN_MEMORY_AVAILABLE_KIB="$(
    awk -F'"memory_available_kib":|,"tasks":' '
      NR == 1 { min = $2 }
      $2 < min { min = $2 }
      END { print min + 0 }
    ' "$OUT/host.jsonl"
  )"
  MAX_TASKS="$(
    awk -F'"tasks":|,"tcp_established":' '
      $2 > max { max = $2 }
      END { print max + 0 }
    ' "$OUT/host.jsonl"
  )"
  FIRST_MEMORY_AVAILABLE_KIB="$(
    awk -F'"memory_available_kib":|,"tasks":' 'NR == 1 { print $2 + 0; exit }' \
      "$OUT/host.jsonl"
  )"
  LAST_MEMORY_AVAILABLE_KIB="$(
    awk -F'"memory_available_kib":|,"tasks":' '{ value = $2 } END { print value + 0 }' \
      "$OUT/host.jsonl"
  )"
  if (( FIRST_MEMORY_AVAILABLE_KIB > LAST_MEMORY_AVAILABLE_KIB )); then
    MEMORY_GROWTH_KIB=$((FIRST_MEMORY_AVAILABLE_KIB - LAST_MEMORY_AVAILABLE_KIB))
  fi
  MAX_TCP_ESTABLISHED="$(
    awk -F'"tcp_established":|,"allocated_file_handles":' '
      $2 > max { max = $2 }
      END { print max + 0 }
    ' "$OUT/host.jsonl"
  )"
  MAX_ALLOCATED_FILE_HANDLES="$(
    awk -F'"allocated_file_handles":|,"cpu_total_ticks":' '
      $2 > max { max = $2 }
      END { print max + 0 }
    ' "$OUT/host.jsonl"
  )"
  CPU_BUSY_BASIS_POINTS="$(
    awk -F'"cpu_total_ticks":|,"cpu_idle_ticks":|}' '
      NR == 1 { first_total = $2; first_idle = $3 }
      { last_total = $2; last_idle = $3 }
      END {
        total = last_total - first_total
        idle = last_idle - first_idle
        if (total <= 0) print 0
        else printf "%.0f\n", ((total - idle) * 10000) / total
      }
    ' "$OUT/host.jsonl"
  )"
fi

STATUS="passed"
if (( WORKLOAD_STATUS != 0 )); then
  STATUS="failed"
elif (( ELAPSED < SECONDS_REQUIRED )); then
  STATUS="ended_early"
fi

BOUNDARY_SUMMARY="$(
  grep -E '^roze_[a-z_]+_soak ' "$OUT/workload.log" | tail -n 1 || true
)"
printf '%s\n' "${BOUNDARY_SUMMARY:-not_reported}" >"$OUT/boundary-summary.txt"

printf '{"schema_version":1,"area":"%s","duration":"%s","revision":"%s","status":"%s","workload_exit_code":%s,"required_seconds":%s,"elapsed_seconds":%s,"started_at":"%s","finished_at":"%s","host_samples":%s,"minimum_memory_available_kib":%s,"first_memory_available_kib":%s,"last_memory_available_kib":%s,"memory_growth_kib":%s,"maximum_host_tasks":%s,"maximum_tcp_established":%s,"maximum_allocated_file_handles":%s,"cpu_busy_basis_points":%s}\n' \
  "$AREA" "$DURATION" "$REVISION" "$STATUS" "$WORKLOAD_STATUS" \
  "$SECONDS_REQUIRED" "$ELAPSED" "$STARTED_AT" "$FINISHED_AT" \
  "$HOST_SAMPLES" "$MIN_MEMORY_AVAILABLE_KIB" \
  "$FIRST_MEMORY_AVAILABLE_KIB" "$LAST_MEMORY_AVAILABLE_KIB" \
  "$MEMORY_GROWTH_KIB" "$MAX_TASKS" "$MAX_TCP_ESTABLISHED" \
  "$MAX_ALLOCATED_FILE_HANDLES" "$CPU_BUSY_BASIS_POINTS" >"$OUT/run.json"

printf '# Roze Production Soak Evidence\n\n- Status: `%s`\n- Area: `%s`\n- Duration: `%s`\n- Required seconds: `%s`\n- Elapsed seconds: `%s`\n- Workload exit code: `%s`\n- Revision: `%s`\n- Started: `%s`\n- Finished: `%s`\n- Host samples: `%s`\n- Minimum available memory: `%s KiB`\n- First available memory: `%s KiB`\n- Last available memory: `%s KiB`\n- Observed memory growth: `%s KiB`\n- Maximum host task count: `%s`\n- Maximum established TCP connections: `%s`\n- Maximum allocated file handles: `%s`\n- Aggregate CPU busy: `%s basis points`\n- Artifact checksums: `SHA256SUMS`\n' \
  "$STATUS" "$AREA" "$DURATION" "$SECONDS_REQUIRED" "$ELAPSED" \
  "$WORKLOAD_STATUS" "$REVISION" "$STARTED_AT" "$FINISHED_AT" \
  "$HOST_SAMPLES" "$MIN_MEMORY_AVAILABLE_KIB" \
  "$FIRST_MEMORY_AVAILABLE_KIB" "$LAST_MEMORY_AVAILABLE_KIB" \
  "$MEMORY_GROWTH_KIB" "$MAX_TASKS" "$MAX_TCP_ESTABLISHED" \
  "$MAX_ALLOCATED_FILE_HANDLES" "$CPU_BUSY_BASIS_POINTS" >"$OUT/summary.md"

printf '\n## Boundary Summary\n\n```text\n%s\n```\n' \
  "${BOUNDARY_SUMMARY:-not_reported}" >>"$OUT/summary.md"

(
  cd "$OUT"
  find . -maxdepth 1 -type f ! -name SHA256SUMS -printf '%P\0' |
    sort -z |
    xargs -0 sha256sum >SHA256SUMS
)

if (( WORKLOAD_STATUS != 0 )); then
  echo "soak workload failed with exit code $WORKLOAD_STATUS; evidence archived at $OUT" >&2
  exit "$WORKLOAD_STATUS"
fi
if (( ELAPSED < SECONDS_REQUIRED )); then
  echo "soak ended early: required=$SECONDS_REQUIRED actual=$ELAPSED; evidence archived at $OUT" >&2
  exit 1
fi
