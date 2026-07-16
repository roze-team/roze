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

cleanup() {
  docker compose -f docker-compose.integration.yml down --remove-orphans || true
}
trap cleanup EXIT

while (( $(date +%s) < DEADLINE )); do
  ITERATION=$((ITERATION + 1))
  printf 'generated-systems iteration=%s started_at=%s\n' \
    "$ITERATION" "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  bash scripts/production-smoke.sh --with-compose
  printf 'generated-systems iteration=%s finished_at=%s\n' \
    "$ITERATION" "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
done

ELAPSED=$(( $(date +%s) - STARTED ))
if (( ELAPSED < SECONDS_REQUIRED )); then
  echo "generated systems soak ended early: required=$SECONDS_REQUIRED actual=$ELAPSED" >&2
  exit 1
fi

printf 'generated-systems completed iterations=%s elapsed_seconds=%s\n' \
  "$ITERATION" "$ELAPSED"
