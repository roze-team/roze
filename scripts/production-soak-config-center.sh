#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

DURATION="${ROZE_CONFIG_CENTER_SOAK_SECONDS:-${1:-300}}"
UPDATES="${ROZE_CONFIG_CENTER_SOAK_UPDATES:-18446744073709551615}"
FAULT_INTERVAL="${ROZE_CONFIG_ETCD_FAULT_INTERVAL_SECONDS:-300}"
OUTAGE_SECONDS="${ROZE_CONFIG_ETCD_OUTAGE_SECONDS:-10}"
RECOVERY_MARGIN="${ROZE_CONFIG_RECOVERY_MARGIN_SECONDS:-30}"
HARD_FAULT="${ROZE_CONFIG_HARD_FAULT:-1}"
COMPOSE_FILE="${ROZE_CONFIG_ETCD_COMPOSE_FILE:-docker-compose.integration.yml}"
COMPOSE_PROJECT="${ROZE_CONFIG_ETCD_COMPOSE_PROJECT:-roze-config-soak-${GITHUB_RUN_ID:-local}-$$}"

for value in "$DURATION" "$UPDATES" "$FAULT_INTERVAL" "$OUTAGE_SECONDS" "$RECOVERY_MARGIN"; do
  case "$value" in
    ''|*[!0-9]*) echo "Config Center soak values must be positive integers" >&2; exit 2 ;;
    0) echo "Config Center soak values must be greater than zero" >&2; exit 2 ;;
  esac
done
case "$HARD_FAULT" in
  0|1) ;;
  *) echo "ROZE_CONFIG_HARD_FAULT must be 0 or 1" >&2; exit 2 ;;
esac

DEBUG_DIR="${ROZE_CONFIG_SOAK_DEBUG_DIR:-}"
if [[ -n "$DEBUG_DIR" ]]; then
  mkdir -p "$DEBUG_DIR"
  ADMIN_LOG="$DEBUG_DIR/admin.log"
  ETCD_LOG="$DEBUG_DIR/etcd.log"
  : >"$ADMIN_LOG"
  : >"$ETCD_LOG"
else
  ADMIN_LOG="$(mktemp)"
  ETCD_LOG="$(mktemp)"
fi
ADMIN_PID=""
ETCD_PID=""
ETCD_CONTAINER=""

compose() {
  docker compose -p "$COMPOSE_PROJECT" -f "$COMPOSE_FILE" "$@"
}

cleanup() {
  if [[ -n "$ADMIN_PID" ]]; then
    kill "$ADMIN_PID" 2>/dev/null || true
  fi
  if [[ -n "$ETCD_PID" ]]; then
    kill "$ETCD_PID" 2>/dev/null || true
  fi
  compose down --remove-orphans || true
  if [[ -z "$DEBUG_DIR" ]]; then
    rm -f "$ADMIN_LOG" "$ETCD_LOG"
  fi
}
trap cleanup EXIT

wait_for_etcd() {
  if [[ -z "$ETCD_CONTAINER" ]]; then
    ETCD_CONTAINER="$(compose ps -q etcd)"
  fi
  if [[ -z "$ETCD_CONTAINER" ]]; then
    echo "Etcd container is not running" >&2
    return 1
  fi
  for _ in $(seq 1 120); do
    if docker exec "$ETCD_CONTAINER" \
      etcdctl endpoint health >/dev/null 2>&1
    then
      return
    fi
    sleep 1
  done
  echo "timed out waiting for Config Center Etcd" >&2
  return 1
}

compose up -d etcd
ETCD_CONTAINER="$(compose ps -q etcd)"
wait_for_etcd
cargo test -p roze-config --no-run

export ROZE_CONFIG_CENTER_SOAK_SECONDS="$DURATION"
export ROZE_CONFIG_CENTER_SOAK_UPDATES="$UPDATES"
export ROZE_CONFIG_ETCD_SOAK_SECONDS="$DURATION"
export ROZE_CONFIG_ETCD_REQUIRE_DISCONNECT=1
export ROZE_TEST_ETCD_ENDPOINT="${ROZE_TEST_ETCD_ENDPOINT:-http://127.0.0.1:2379}"

cargo test -p roze-config \
  production_soak_admin_store_validation_rollback_and_snapshot \
  -- --ignored --nocapture >"$ADMIN_LOG" 2>&1 &
ADMIN_PID=$!
cargo test -p roze-config \
  production_soak_etcd_subscriber_disconnect_recovery \
  -- --ignored --nocapture >"$ETCD_LOG" 2>&1 &
ETCD_PID=$!

WORKLOAD_START_DEADLINE=$(( $(date +%s) + 300 ))
while true; do
  if ! kill -0 "$ETCD_PID" 2>/dev/null && ! grep -q '^running 1 test' "$ETCD_LOG"; then
    echo "Etcd workload exited before entering its test" >&2
    cat "$ETCD_LOG" >&2
    exit 1
  fi
  if grep -q '^running 1 test' "$ETCD_LOG"; then
    break
  fi
  if (( $(date +%s) >= WORKLOAD_START_DEADLINE )); then
    echo "timed out waiting for Config Center workload to start" >&2
    exit 1
  fi
  sleep 1
done

SOAK_STARTED="$(date +%s)"
SOAK_DEADLINE=$((SOAK_STARTED + DURATION))
if (( FAULT_INTERVAL * 3 >= DURATION )); then
  FAULT_INTERVAL=$((DURATION / 3))
  if (( FAULT_INTERVAL == 0 )); then
    FAULT_INTERVAL=1
  fi
fi

FAULT_INJECTIONS=0
NEXT_FAULT=$((SOAK_STARTED + FAULT_INTERVAL))
while true; do
  NOW="$(date +%s)"
  if (( NOW >= SOAK_DEADLINE )); then
    break
  fi
  if (( NOW < NEXT_FAULT )); then
    sleep 5
    continue
  fi
  if (( NOW + OUTAGE_SECONDS + RECOVERY_MARGIN >= SOAK_DEADLINE )); then
    break
  fi
  FAULT_INJECTIONS=$((FAULT_INJECTIONS + 1))
  if [[ "$HARD_FAULT" == 1 ]]; then
    docker kill "$ETCD_CONTAINER" >/dev/null
  else
    compose stop etcd
  fi
  sleep "$OUTAGE_SECONDS"
  compose start etcd
  wait_for_etcd
  NEXT_FAULT=$(($(date +%s) + FAULT_INTERVAL))
done

set +e
wait "$ADMIN_PID"
ADMIN_STATUS=$?
ADMIN_PID=""
wait "$ETCD_PID"
ETCD_STATUS=$?
ETCD_PID=""
set -e

cat "$ADMIN_LOG"
cat "$ETCD_LOG"
if (( ADMIN_STATUS != 0 || ETCD_STATUS != 0 )); then
  echo "Config Center soak workload failed: admin=$ADMIN_STATUS etcd=$ETCD_STATUS" >&2
  exit 1
fi
if (( FAULT_INJECTIONS == 0 )); then
  echo "Config Center soak completed without Etcd fault injection" >&2
  exit 1
fi

ADMIN_SUMMARY="$(grep -E '^roze_config_center_soak ' "$ADMIN_LOG" | tail -n 1 || true)"
ETCD_SUMMARY="$(grep -E '^roze_config_etcd_soak ' "$ETCD_LOG" | tail -n 1 || true)"
if [[ -z "$ADMIN_SUMMARY" || -z "$ETCD_SUMMARY" ]]; then
  echo "Config Center soak workload did not emit both boundary summaries" >&2
  exit 1
fi
printf 'roze_config_center_soak %s %s etcd_fault_injections=%s\n' \
  "${ADMIN_SUMMARY#roze_config_center_soak }" \
  "${ETCD_SUMMARY#roze_config_etcd_soak }" \
  "$FAULT_INJECTIONS"
