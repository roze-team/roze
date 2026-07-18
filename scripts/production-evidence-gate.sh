#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

require_evidence() {
  local matrix_area="$1"
  local evidence_area="$2"

  if grep -Eq "^\| ${matrix_area} \| stable \| long-run pending \|" docs/maturity.md; then
    printf 'long-run evidence pending: %s\n' "$matrix_area"
    return
  fi

  if ! grep -Eq "^\| ${matrix_area} \| stable \| long-run verified \|" docs/maturity.md; then
    echo "runtime area '${matrix_area}' must declare long-run pending or long-run verified" >&2
    exit 1
  fi

  local report
  local found=0
  shopt -s nullglob
  for report in docs/evidence/*-"${evidence_area}"-24h.md docs/evidence/*-"${evidence_area}"-72h.md; do
    if bash scripts/production-evidence-report-verify.sh \
      "$report" "$evidence_area" >/dev/null
    then
      found=1
      break
    fi
  done
  shopt -u nullglob

  if (( found == 0 )); then
    echo "long-run verified area '${matrix_area}' requires a complete passing 24h/72h ${evidence_area} report" >&2
    exit 1
  fi
}

require_evidence "Gateway" "gateway"
require_evidence "MQ/Kafka/NATS" "mq"
require_evidence "Config center" "config-center"
require_evidence "Lifecycle/bootstrap" "lifecycle"
require_evidence "Production smoke" "generated-services"

echo "production evidence maturity gate passed"
