#!/usr/bin/env bash
set -euo pipefail

# S6 audit: make the relationship between the release revision, maturity
# declarations, and promoted S5 evidence explicit and machine-readable.
# This is intentionally lighter than release-gate.sh: it does not rerun the
# full build/test suite, but it is the final predicate over their outputs.

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

REVISION="$(git rev-parse HEAD)"
JSON_OUT=""
REQUIRE_LONG_RUN=0

usage() {
  cat <<'EOF'
Usage: bash scripts/production-release-audit.sh [options]

Options:
  --revision <sha>       audit this full revision (default: HEAD)
  --json-out <path>      write the machine-readable audit result
  --require-long-run     fail unless every S5 area is long-run verified
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --revision)
      [[ -n "${2:-}" ]] || { echo "missing value for --revision" >&2; exit 2; }
      REVISION="$2"
      shift 2
      ;;
    --json-out)
      [[ -n "${2:-}" ]] || { echo "missing value for --json-out" >&2; exit 2; }
      JSON_OUT="$2"
      shift 2
      ;;
    --require-long-run)
      REQUIRE_LONG_RUN=1
      shift
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

if [[ ! "$REVISION" =~ ^[0-9a-f]{40}$ ]]; then
  echo "revision must be a full 40-character lowercase Git SHA" >&2
  exit 2
fi
if ! git cat-file -e "${REVISION}^{commit}" 2>/dev/null; then
  echo "revision is not an existing Git commit: $REVISION" >&2
  exit 1
fi

if [[ ! -f docs/maturity.md || ! -f scripts/production-evidence-gate.sh ]]; then
  echo "maturity/evidence gate inputs are missing" >&2
  exit 1
fi

declare -a MATRIX_AREAS=("Gateway" "MQ/Kafka/NATS" "Config center" "Lifecycle/bootstrap" "Production smoke")
declare -a EVIDENCE_AREAS=(gateway mq config-center lifecycle generated-services)

statuses=()
verified_count=0
pending_count=0
for i in "${!MATRIX_AREAS[@]}"; do
  matrix_area="${MATRIX_AREAS[$i]}"
  evidence_area="${EVIDENCE_AREAS[$i]}"
  row="$(grep -E "^\\| ${matrix_area} \\| stable \\| (long-run pending|long-run verified) \\|" docs/maturity.md || true)"
  if [[ -z "$row" ]]; then
    echo "maturity row missing or has an invalid status: $matrix_area" >&2
    exit 1
  fi
  status="$(awk -F'|' '{gsub(/^ +| +$/, "", $4); print $4}' <<<"$row")"
  case "$status" in
    "long-run pending")
      pending_count=$((pending_count + 1))
      statuses+=("$evidence_area=pending")
      ;;
    "long-run verified")
      if ! bash scripts/production-evidence-gate.sh >/dev/null; then
        echo "canonical evidence gate failed for verified maturity row: $matrix_area" >&2
        exit 1
      fi
      report=""
      shopt -s nullglob
      for candidate in docs/evidence/*-"${evidence_area}"-24h.md docs/evidence/*-"${evidence_area}"-72h.md; do
        if bash scripts/production-evidence-report-verify.sh "$candidate" "$evidence_area" >/dev/null 2>&1; then
          report="$candidate"
          break
        fi
      done
      shopt -u nullglob
      if [[ -z "$report" ]]; then
        echo "verified maturity row has no passing report: $matrix_area" >&2
        exit 1
      fi
      report_revision="$(awk -F': ' '$1 == "revision" { print $2; exit }' "$report")"
      if [[ "$report_revision" != "$REVISION" ]]; then
        echo "evidence revision mismatch for $matrix_area: report=$report_revision audit=$REVISION" >&2
        exit 1
      fi
      verified_count=$((verified_count + 1))
      statuses+=("$evidence_area=verified")
      ;;
    *)
      echo "unsupported maturity status '$status' for $matrix_area" >&2
      exit 1
      ;;
  esac
done

if (( REQUIRE_LONG_RUN == 1 && pending_count != 0 )); then
  echo "strict S6 audit requires all five long-run evidence areas; pending=$pending_count" >&2
  exit 1
fi

if [[ -n "$JSON_OUT" ]]; then
  mkdir -p "$(dirname "$JSON_OUT")"
  {
    printf '{\n  "schema_version": 1,\n  "revision": "%s",\n' "$REVISION"
    mode=candidate
    (( REQUIRE_LONG_RUN == 1 )) && mode=strict
    printf '  "mode": "%s",\n' "$mode"
    printf '  "verified_count": %d,\n  "pending_count": %d,\n  "areas": {\n' "$verified_count" "$pending_count"
    for i in "${!statuses[@]}"; do
      entry="${statuses[$i]}"
      key="${entry%%=*}"
      value="${entry#*=}"
      comma=,
      (( i == ${#statuses[@]} - 1 )) && comma=
      printf '    "%s": "%s"%s\n' "$key" "$value" "$comma"
    done
    verdict=api_stable_long_run_pending
    (( pending_count == 0 )) && verdict=pass
    printf '  },\n  "verdict": "%s"\n}\n' "$verdict"
  } >"$JSON_OUT"
fi

if (( pending_count == 0 )); then
  echo "S6 production release audit passed: all five long-run areas verified at $REVISION"
else
  echo "S6 production release audit passed in candidate mode: $verified_count verified, $pending_count long-run pending at $REVISION"
fi
