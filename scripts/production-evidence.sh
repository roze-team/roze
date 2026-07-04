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
