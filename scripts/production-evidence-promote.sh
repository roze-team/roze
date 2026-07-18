#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

BUNDLE=""
ARTIFACT_ID=""
ARTIFACT_DIGEST=""
ARTIFACT_URL=""
ATTESTATION_URL=""
OUT=""

usage() {
  cat <<'EOF'
Usage:
  bash scripts/production-evidence-promote.sh \
    --bundle target/production-evidence/gateway-24h \
    --artifact-id 123456 \
    --artifact-digest sha256:<hex> \
    --artifact-url https://github.com/<owner>/<repo>/actions/runs/<id> \
    --attestation-url https://github.com/<owner>/<repo>/attestations/<id> \
    [--out docs/evidence/<date>-<area>-<duration>.md]

The bundle must be downloaded from the fixed production-soak runner. This
command verifies its checksums and terminal run metadata before creating a
committable passing report.
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
    --bundle)
      require_value "$1" "${2:-}"
      BUNDLE="$2"
      shift 2
      ;;
    --artifact-id)
      require_value "$1" "${2:-}"
      ARTIFACT_ID="$2"
      shift 2
      ;;
    --artifact-digest)
      require_value "$1" "${2:-}"
      ARTIFACT_DIGEST="$2"
      shift 2
      ;;
    --artifact-url)
      require_value "$1" "${2:-}"
      ARTIFACT_URL="$2"
      shift 2
      ;;
    --attestation-url)
      require_value "$1" "${2:-}"
      ATTESTATION_URL="$2"
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

require_value "--bundle" "$BUNDLE"
require_value "--artifact-id" "$ARTIFACT_ID"
require_value "--artifact-digest" "$ARTIFACT_DIGEST"
require_value "--artifact-url" "$ARTIFACT_URL"
require_value "--attestation-url" "$ATTESTATION_URL"

for command in node sha256sum; do
  if ! command -v "$command" >/dev/null 2>&1; then
    echo "required command is missing: $command" >&2
    exit 2
  fi
done

if [[ ! "$ARTIFACT_ID" =~ ^[1-9][0-9]*$ ]]; then
  echo "--artifact-id must be a positive integer" >&2
  exit 2
fi
if [[ ! "$ARTIFACT_DIGEST" =~ ^sha256:[0-9a-f]{64}$ ]]; then
  echo "--artifact-digest must be sha256 followed by 64 lowercase hex characters" >&2
  exit 2
fi
for url in "$ARTIFACT_URL" "$ATTESTATION_URL"; do
  if [[ ! "$url" =~ ^https://github\.com/ ]]; then
    echo "artifact and attestation URLs must use https://github.com/" >&2
    exit 2
  fi
done

for file in run.json summary.md boundary-summary.txt workload.log host.jsonl SHA256SUMS; do
  if [[ ! -s "$BUNDLE/$file" ]]; then
    echo "evidence bundle is missing non-empty $file" >&2
    exit 1
  fi
done

(
  cd "$BUNDLE"
  sha256sum --check --strict SHA256SUMS
)

RUN_FIELDS="$(
  node - "$BUNDLE/run.json" <<'NODE'
const fs = require("fs");
const path = process.argv[2];
const run = JSON.parse(fs.readFileSync(path, "utf8"));
const stringFields = [
  "area",
  "duration",
  "revision",
  "status",
  "started_at",
  "finished_at",
];
const integerFields = [
  "workload_exit_code",
  "required_seconds",
  "elapsed_seconds",
  "host_samples",
  "minimum_memory_available_kib",
  "first_memory_available_kib",
  "last_memory_available_kib",
  "memory_growth_kib",
  "maximum_host_tasks",
  "maximum_tcp_established",
  "maximum_allocated_file_handles",
  "cpu_busy_basis_points",
];
for (const field of stringFields) {
  if (typeof run[field] !== "string" || run[field].length === 0) {
    throw new Error(`run.json field ${field} must be a non-empty string`);
  }
}
for (const field of integerFields) {
  if (!Number.isSafeInteger(run[field]) || run[field] < 0) {
    throw new Error(`run.json field ${field} must be a non-negative safe integer`);
  }
}
console.log([
  run.area,
  run.duration,
  run.revision,
  run.status,
  run.workload_exit_code,
  run.required_seconds,
  run.elapsed_seconds,
  run.started_at,
  run.finished_at,
  run.host_samples,
  run.minimum_memory_available_kib,
  run.first_memory_available_kib,
  run.last_memory_available_kib,
  run.memory_growth_kib,
  run.maximum_host_tasks,
  run.maximum_tcp_established,
  run.maximum_allocated_file_handles,
  run.cpu_busy_basis_points,
].join("\t"));
NODE
)"
IFS=$'\t' read -r \
  AREA DURATION REVISION STATUS WORKLOAD_EXIT_CODE REQUIRED_SECONDS \
  ELAPSED_SECONDS STARTED_AT FINISHED_AT HOST_SAMPLES \
  MINIMUM_MEMORY_AVAILABLE_KIB FIRST_MEMORY_AVAILABLE_KIB \
  LAST_MEMORY_AVAILABLE_KIB MEMORY_GROWTH_KIB MAXIMUM_HOST_TASKS \
  MAXIMUM_TCP_ESTABLISHED MAXIMUM_ALLOCATED_FILE_HANDLES \
  CPU_BUSY_BASIS_POINTS <<<"$RUN_FIELDS"

case "$AREA" in
  gateway|mq|config-center|lifecycle) EVIDENCE_AREA="$AREA" ;;
  generated-systems) EVIDENCE_AREA="generated-services" ;;
  *) echo "unsupported evidence area in run.json: $AREA" >&2; exit 1 ;;
esac
case "$DURATION" in
  24h) EXPECTED_SECONDS=86400 ;;
  72h) EXPECTED_SECONDS=259200 ;;
  *) echo "unsupported duration in run.json: $DURATION" >&2; exit 1 ;;
esac
if [[ "$STATUS" != "passed" || "$WORKLOAD_EXIT_CODE" != "0" ]]; then
  echo "only a successful workload can be promoted" >&2
  exit 1
fi
if [[ "$REQUIRED_SECONDS" != "$EXPECTED_SECONDS" ]] ||
   (( ELAPSED_SECONDS < EXPECTED_SECONDS )); then
  echo "run did not satisfy the required real elapsed duration" >&2
  exit 1
fi
if (( HOST_SAMPLES == 0 || MINIMUM_MEMORY_AVAILABLE_KIB == 0 ||
      FIRST_MEMORY_AVAILABLE_KIB == 0 || LAST_MEMORY_AVAILABLE_KIB == 0 ||
      MAXIMUM_HOST_TASKS == 0 || MAXIMUM_ALLOCATED_FILE_HANDLES == 0 )); then
  echo "run is missing host resource samples" >&2
  exit 1
fi
if [[ ! "$REVISION" =~ ^[0-9a-f]{40}$ ]]; then
  echo "run revision must be a full Git commit SHA" >&2
  exit 1
fi

BOUNDARY_SUMMARY="$(<"$BUNDLE/boundary-summary.txt")"
if [[ ! "$BOUNDARY_SUMMARY" =~ ^roze_[a-z_]+_soak[[:space:]] ]]; then
  echo "run is missing a standardized boundary summary" >&2
  exit 1
fi

CHECKSUMS_SHA256="$(sha256sum "$BUNDLE/SHA256SUMS" | awk '{ print $1 }')"
DATE="${FINISHED_AT%%T*}"
if [[ -z "$OUT" ]]; then
  OUT="docs/evidence/${DATE}-${EVIDENCE_AREA}-${DURATION}.md"
fi
mkdir -p "$(dirname "$OUT")"

cat >"$OUT" <<EOF
---
schema_version: 1
area: ${EVIDENCE_AREA}
duration: ${DURATION}
verdict: pass
revision: ${REVISION}
run_status: ${STATUS}
required_seconds: ${REQUIRED_SECONDS}
elapsed_seconds: ${ELAPSED_SECONDS}
started_at: ${STARTED_AT}
finished_at: ${FINISHED_AT}
host_samples: ${HOST_SAMPLES}
minimum_memory_available_kib: ${MINIMUM_MEMORY_AVAILABLE_KIB}
first_memory_available_kib: ${FIRST_MEMORY_AVAILABLE_KIB}
last_memory_available_kib: ${LAST_MEMORY_AVAILABLE_KIB}
memory_growth_kib: ${MEMORY_GROWTH_KIB}
maximum_host_tasks: ${MAXIMUM_HOST_TASKS}
maximum_tcp_established: ${MAXIMUM_TCP_ESTABLISHED}
maximum_allocated_file_handles: ${MAXIMUM_ALLOCATED_FILE_HANDLES}
cpu_busy_basis_points: ${CPU_BUSY_BASIS_POINTS}
artifact_id: ${ARTIFACT_ID}
artifact_digest: ${ARTIFACT_DIGEST}
artifact_url: ${ARTIFACT_URL}
attestation_url: ${ATTESTATION_URL}
checksums_sha256: ${CHECKSUMS_SHA256}
---

# Production Evidence: ${EVIDENCE_AREA} ${DURATION}

## Verdict

pass

## Verified Run

- Roze revision: \`${REVISION}\`
- Started: \`${STARTED_AT}\`
- Finished: \`${FINISHED_AT}\`
- Required seconds: \`${REQUIRED_SECONDS}\`
- Elapsed seconds: \`${ELAPSED_SECONDS}\`
- Host samples: \`${HOST_SAMPLES}\`
- Minimum available memory: \`${MINIMUM_MEMORY_AVAILABLE_KIB} KiB\`
- First available memory: \`${FIRST_MEMORY_AVAILABLE_KIB} KiB\`
- Last available memory: \`${LAST_MEMORY_AVAILABLE_KIB} KiB\`
- Observed memory growth: \`${MEMORY_GROWTH_KIB} KiB\`
- Maximum host task count: \`${MAXIMUM_HOST_TASKS}\`
- Maximum established TCP connections: \`${MAXIMUM_TCP_ESTABLISHED}\`
- Maximum allocated file handles: \`${MAXIMUM_ALLOCATED_FILE_HANDLES}\`
- Aggregate CPU busy: \`${CPU_BUSY_BASIS_POINTS} basis points\`
- Artifact: [GitHub Actions artifact](${ARTIFACT_URL})
- Provenance: [GitHub attestation](${ATTESTATION_URL})
- Artifact digest: \`${ARTIFACT_DIGEST}\`
- SHA256SUMS digest: \`${CHECKSUMS_SHA256}\`

## Boundary Summary

\`\`\`text
${BOUNDARY_SUMMARY}
\`\`\`

## Artifact Contents

The linked artifact contains the terminal \`run.json\`, workload log, host
samples, boundary summary, Markdown run summary, and a portable
\`SHA256SUMS\` manifest. The report was generated only after every checksum,
the terminal success state, elapsed duration, resource samples, and boundary
summary were validated by \`scripts/production-evidence-promote.sh\`.
EOF

echo "wrote verified passing report $OUT"
