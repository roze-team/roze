#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

AREA=""
DURATION=""
WORKLOAD=""
FAILURE_INJECTION=""
COMMAND=""
DEPENDENCIES=""
SUCCESS_CRITERIA=""
ERROR_BUDGET=""
VERDICT="inconclusive"
OUT=""
LIFECYCLE_SUMMARY=""

usage() {
  cat <<'EOF'
Usage:
  bash scripts/production-evidence.sh \
    --area gateway|mq|config-center|lifecycle|generated-services \
    --duration 24h|72h \
    --workload "..." \
    --failure-injection "..." \
    --command "..." \
    [--dependencies "..."] \
    [--success-criteria "..."] \
    [--error-budget "..."] \
    [--lifecycle-summary "roze_lifecycle_soak cycles=... worker_exits=..."] \
    [--verdict pass|fail|inconclusive] \
    [--out docs/evidence/<date>-<area>-<duration>.md]

This script creates a production evidence report scaffold. It does not run the
long workload and must not be used to fabricate results.
EOF
}

require_value() {
  local name="$1"
  local value="${2:-}"
  if [[ -z "$value" ]]; then
    echo "missing value for $name" >&2
    exit 2
  fi
}

require_unsigned_integer() {
  local name="$1"
  local value="$2"
  if [[ ! "$value" =~ ^[0-9]+$ ]]; then
    echo "$name must be an unsigned integer, got: $value" >&2
    exit 2
  fi
}

require_positive_integer() {
  local name="$1"
  local value="$2"
  require_unsigned_integer "$name" "$value"
  if (( value == 0 )); then
    echo "$name must be greater than zero" >&2
    exit 2
  fi
}

require_equal() {
  local name="$1"
  local actual="$2"
  local expected="$3"
  if (( actual != expected )); then
    echo "$name must be $expected, got: $actual" >&2
    exit 2
  fi
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --area)
      require_value "$1" "${2:-}"
      AREA="$2"
      shift 2
      ;;
    --duration)
      require_value "$1" "${2:-}"
      DURATION="$2"
      shift 2
      ;;
    --workload)
      require_value "$1" "${2:-}"
      WORKLOAD="$2"
      shift 2
      ;;
    --failure-injection)
      require_value "$1" "${2:-}"
      FAILURE_INJECTION="$2"
      shift 2
      ;;
    --command)
      require_value "$1" "${2:-}"
      COMMAND="$2"
      shift 2
      ;;
    --dependencies)
      require_value "$1" "${2:-}"
      DEPENDENCIES="$2"
      shift 2
      ;;
    --success-criteria)
      require_value "$1" "${2:-}"
      SUCCESS_CRITERIA="$2"
      shift 2
      ;;
    --error-budget)
      require_value "$1" "${2:-}"
      ERROR_BUDGET="$2"
      shift 2
      ;;
    --lifecycle-summary)
      require_value "$1" "${2:-}"
      LIFECYCLE_SUMMARY="$2"
      shift 2
      ;;
    --verdict)
      require_value "$1" "${2:-}"
      VERDICT="$2"
      shift 2
      ;;
    --out)
      require_value "$1" "${2:-}"
      OUT="$2"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "unknown argument: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

case "$AREA" in
  gateway|mq|config-center|lifecycle|generated-services) ;;
  "")
    echo "--area is required" >&2
    exit 2
    ;;
  *)
    echo "unsupported --area: $AREA" >&2
    exit 2
    ;;
esac

case "$VERDICT" in
  pass|fail|inconclusive) ;;
  *)
    echo "unsupported --verdict: $VERDICT" >&2
    exit 2
    ;;
esac

require_value "--duration" "$DURATION"
require_value "--workload" "$WORKLOAD"
require_value "--failure-injection" "$FAILURE_INJECTION"
require_value "--command" "$COMMAND"

if [[ -n "$LIFECYCLE_SUMMARY" && "$AREA" != "lifecycle" ]]; then
  echo "--lifecycle-summary can only be used with --area lifecycle" >&2
  exit 2
fi

LIFECYCLE_CYCLES="TBD"
LIFECYCLE_WORKER_EXITS="TBD"
LIFECYCLE_STOP_HOOKS="TBD"
LIFECYCLE_RUNNING_SNAPSHOTS="TBD"
LIFECYCLE_STOPPED_SNAPSHOTS="TBD"
LIFECYCLE_MAX_SERVICE_COUNT="TBD"

if [[ -n "$LIFECYCLE_SUMMARY" ]]; then
  read -r -a LIFECYCLE_SUMMARY_PARTS <<<"$LIFECYCLE_SUMMARY"
  if [[ "${LIFECYCLE_SUMMARY_PARTS[0]:-}" != "roze_lifecycle_soak" ]]; then
    echo "--lifecycle-summary must start with roze_lifecycle_soak" >&2
    exit 2
  fi

  for part in "${LIFECYCLE_SUMMARY_PARTS[@]:1}"; do
    case "$part" in
      cycles=*)
        LIFECYCLE_CYCLES="${part#cycles=}"
        ;;
      worker_exits=*)
        LIFECYCLE_WORKER_EXITS="${part#worker_exits=}"
        ;;
      stop_hooks=*)
        LIFECYCLE_STOP_HOOKS="${part#stop_hooks=}"
        ;;
      running_snapshots=*)
        LIFECYCLE_RUNNING_SNAPSHOTS="${part#running_snapshots=}"
        ;;
      stopped_snapshots=*)
        LIFECYCLE_STOPPED_SNAPSHOTS="${part#stopped_snapshots=}"
        ;;
      max_service_count=*)
        LIFECYCLE_MAX_SERVICE_COUNT="${part#max_service_count=}"
        ;;
      *)
        echo "unsupported lifecycle summary field: $part" >&2
        exit 2
        ;;
    esac
  done

  require_unsigned_integer "lifecycle summary cycles" "$LIFECYCLE_CYCLES"
  require_unsigned_integer "lifecycle summary worker_exits" "$LIFECYCLE_WORKER_EXITS"
  require_unsigned_integer "lifecycle summary stop_hooks" "$LIFECYCLE_STOP_HOOKS"
  require_unsigned_integer "lifecycle summary running_snapshots" "$LIFECYCLE_RUNNING_SNAPSHOTS"
  require_unsigned_integer "lifecycle summary stopped_snapshots" "$LIFECYCLE_STOPPED_SNAPSHOTS"
  require_unsigned_integer "lifecycle summary max_service_count" "$LIFECYCLE_MAX_SERVICE_COUNT"
  require_positive_integer "lifecycle summary cycles" "$LIFECYCLE_CYCLES"
  require_positive_integer "lifecycle summary max_service_count" "$LIFECYCLE_MAX_SERVICE_COUNT"

  LIFECYCLE_EXPECTED_WORKERS=$((LIFECYCLE_CYCLES * LIFECYCLE_MAX_SERVICE_COUNT))
  require_equal "lifecycle summary worker_exits" "$LIFECYCLE_WORKER_EXITS" "$LIFECYCLE_EXPECTED_WORKERS"
  require_equal "lifecycle summary stop_hooks" "$LIFECYCLE_STOP_HOOKS" "$LIFECYCLE_EXPECTED_WORKERS"
  require_equal "lifecycle summary running_snapshots" "$LIFECYCLE_RUNNING_SNAPSHOTS" "$LIFECYCLE_CYCLES"
  require_equal "lifecycle summary stopped_snapshots" "$LIFECYCLE_STOPPED_SNAPSHOTS" "$LIFECYCLE_CYCLES"
fi

DATE="$(date -u +%Y-%m-%d)"
if [[ -z "$OUT" ]]; then
  OUT="docs/evidence/${DATE}-${AREA}-${DURATION}.md"
fi

mkdir -p "$(dirname "$OUT")"

REVISION="$(git rev-parse HEAD)"
RUSTC_VERSION="$(rustc --version 2>/dev/null || printf 'missing')"
CARGO_VERSION="$(cargo --version 2>/dev/null || printf 'missing')"
OS_NAME="$(uname -a 2>/dev/null || printf 'unknown')"
GENERATED_AT="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
AREA_SPECIFIC_SECTIONS=""

if [[ "$AREA" == "lifecycle" ]]; then
  AREA_SPECIFIC_SECTIONS="$(cat <<EOF_LIFECYCLE

## Lifecycle Snapshot Summary

Copy the \`roze_lifecycle_soak\` summary line from the completed run:

\`\`\`text
${LIFECYCLE_SUMMARY:-roze_lifecycle_soak cycles=TBD worker_exits=TBD stop_hooks=TBD running_snapshots=TBD stopped_snapshots=TBD max_service_count=TBD}
\`\`\`

| Field | Result | Notes |
| --- | --- | --- |
| cycles | ${LIFECYCLE_CYCLES} | Completed lifecycle start/drain/stop cycles. |
| worker_exits | ${LIFECYCLE_WORKER_EXITS} | Must equal \`cycles * max_service_count\` for the default soak. |
| stop_hooks | ${LIFECYCLE_STOP_HOOKS} | Must equal \`cycles * max_service_count\` for the default soak. |
| running_snapshots | ${LIFECYCLE_RUNNING_SNAPSHOTS} | Must equal \`cycles\`. |
| stopped_snapshots | ${LIFECYCLE_STOPPED_SNAPSHOTS} | Must equal \`cycles\`. |
| max_service_count | ${LIFECYCLE_MAX_SERVICE_COUNT} | Expected \`4\` for the default lifecycle soak. |
EOF_LIFECYCLE
)"
fi

cat >"$OUT" <<EOF_REPORT
# Production Evidence: ${AREA} ${DURATION}

Generated at: ${GENERATED_AT}

## Verdict

${VERDICT}

## Environment

- Roze revision: \`${REVISION}\`
- Rust: \`${RUSTC_VERSION}\`
- Cargo: \`${CARGO_VERSION}\`
- OS: \`${OS_NAME}\`
- Dependencies/topology: ${DEPENDENCIES:-TBD}

## Run Command

\`\`\`bash
${COMMAND}
\`\`\`

## Workload

${WORKLOAD}

## Failure Injection

${FAILURE_INJECTION}

## Success Criteria

${SUCCESS_CRITERIA:-TBD}

## Error Budget

${ERROR_BUDGET:-TBD}

## Measurements

| Metric | Result | Notes |
| --- | --- | --- |
| Duration | ${DURATION} | Replace with observed start/end timestamps after the run. |
| Throughput | TBD | Required for Gateway, MQ, and generated services. |
| p50 latency | TBD | Required where request/response latency applies. |
| p95 latency | TBD | Required where request/response latency applies. |
| p99 latency | TBD | Required where request/response latency applies. |
| Error rate | TBD | Must fit the stated error budget for \`pass\`. |
| CPU trend | TBD | Include warm-up and steady-state notes. |
| Memory trend | TBD | Must show no unbounded growth for \`pass\`. |
| File descriptors/connections | TBD | Required for Gateway and long-lived streams. |
| Restart count | TBD | Include unexpected restarts. |
| Leak check | TBD | Required before any \`pass\` verdict. |
${AREA_SPECIFIC_SECTIONS}

## Failure Timeline

| Time | Injection | Expected recovery | Observed recovery | Result |
| --- | --- | --- | --- | --- |
| TBD | TBD | TBD | TBD | TBD |

## Logs And Artifacts

- Metrics export: TBD
- Logs: TBD
- Traces: TBD
- Raw artifacts: TBD

## Notes

Do not change \`verdict\` to \`pass\` unless every required measurement is filled
in and the pass criteria in \`docs/production-evidence.md\` are satisfied.
EOF_REPORT

echo "wrote $OUT"
