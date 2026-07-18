#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

DURATION="${ROZE_GATEWAY_REGISTRY_RECOVERY_SECONDS:-${1:-30}}"
COMPOSE_FILE="${ROZE_GATEWAY_REGISTRY_COMPOSE_FILE:-docker-compose.integration.yml}"
MANAGE_STACK="${ROZE_GATEWAY_REGISTRY_MANAGE_STACK:-1}"
if [[ "$MANAGE_STACK" == "1" ]]; then
  COMPOSE_PROJECT="${ROZE_GATEWAY_REGISTRY_COMPOSE_PROJECT:-roze-gateway-registry-$$}"
else
  COMPOSE_PROJECT="${ROZE_GATEWAY_REGISTRY_COMPOSE_PROJECT:-}"
fi
OUTAGE_SECONDS="${ROZE_GATEWAY_REGISTRY_OUTAGE_SECONDS:-5}"
FAULT_INTERVAL_SECONDS="${ROZE_GATEWAY_REGISTRY_FAULT_INTERVAL_SECONDS:-300}"
CONSUL_HEALTH_URL="${ROZE_GATEWAY_CONSUL_HEALTH_URL:-http://127.0.0.1:8500}"

for value in "$DURATION" "$OUTAGE_SECONDS" "$FAULT_INTERVAL_SECONDS"; do
  if [[ ! "$value" =~ ^[1-9][0-9]*$ ]]; then
    echo "registry recovery durations must be positive integers, got: $value" >&2
    exit 2
  fi
done
if (( DURATION < 10 )); then
  echo "registry recovery duration must be at least 10 seconds" >&2
  exit 2
fi

WORK="$(mktemp -d)"
ETCD_LOG="$WORK/etcd.log"
CONSUL_LOG="$WORK/consul.log"
ETCD_READY="$WORK/etcd.ready"
CONSUL_READY="$WORK/consul.ready"
PIDS=()

compose() {
  if [[ -n "$COMPOSE_PROJECT" ]]; then
    docker compose -p "$COMPOSE_PROJECT" -f "$COMPOSE_FILE" "$@"
  else
    docker compose -f "$COMPOSE_FILE" "$@"
  fi
}

cleanup() {
  for pid in "${PIDS[@]:-}"; do
    kill "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
  done
  if [[ "$MANAGE_STACK" == "1" ]]; then
    compose down --remove-orphans >/dev/null 2>&1 || true
  fi
  rm -rf "$WORK"
}
trap cleanup EXIT

wait_until() {
  local description="$1"
  shift
  for _ in $(seq 1 120); do
    if "$@" >/dev/null 2>&1; then
      return 0
    fi
    sleep 1
  done
  echo "timed out waiting for $description" >&2
  return 1
}

wait_for_ready_file() {
  local label="$1"
  local file="$2"
  local pid="$3"
  for _ in $(seq 1 120); do
    if [[ -s "$file" ]]; then
      return 0
    fi
    if ! kill -0 "$pid" 2>/dev/null; then
      wait "$pid" || true
      echo "$label recovery probe ended before becoming ready" >&2
      return 1
    fi
    sleep 1
  done
  echo "timed out waiting for $label recovery probe" >&2
  return 1
}

if [[ "$MANAGE_STACK" == "1" ]]; then
  compose up -d etcd consul
fi
wait_until "Etcd" compose exec -T etcd etcdctl endpoint health
wait_until "Consul" curl -fsS "$CONSUL_HEALTH_URL/v1/status/leader"

cargo test -p roze-gateway --no-run >/dev/null

ROZE_TEST_ETCD_ENDPOINT="${ROZE_TEST_ETCD_ENDPOINT:-http://127.0.0.1:2379}" \
ROZE_GATEWAY_REGISTRY_RECOVERY_SECONDS="$DURATION" \
ROZE_GATEWAY_REGISTRY_READY_FILE="$ETCD_READY" \
  cargo test -p roze-gateway \
    gateway_automatically_reregisters_after_real_etcd_restart \
    -- --ignored --nocapture >"$ETCD_LOG" 2>&1 &
ETCD_PID=$!
PIDS+=("$ETCD_PID")

ROZE_TEST_CONSUL_ENDPOINT="${ROZE_TEST_CONSUL_ENDPOINT:-http://127.0.0.1:8500}" \
ROZE_GATEWAY_REGISTRY_RECOVERY_SECONDS="$DURATION" \
ROZE_GATEWAY_REGISTRY_READY_FILE="$CONSUL_READY" \
  cargo test -p roze-gateway \
    gateway_automatically_reregisters_after_real_consul_restart \
    -- --ignored --nocapture >"$CONSUL_LOG" 2>&1 &
CONSUL_PID=$!
PIDS+=("$CONSUL_PID")

wait_for_ready_file "Etcd" "$ETCD_READY" "$ETCD_PID"
wait_for_ready_file "Consul" "$CONSUL_READY" "$CONSUL_PID"

STARTED_AT="$(date +%s)"
DEADLINE=$((STARTED_AT + DURATION))
FAULTS=0
while kill -0 "$ETCD_PID" 2>/dev/null || kill -0 "$CONSUL_PID" 2>/dev/null; do
  NOW="$(date +%s)"
  if (( NOW + OUTAGE_SECONDS + 5 >= DEADLINE )); then
    break
  fi
  compose stop etcd consul >/dev/null
  sleep "$OUTAGE_SECONDS"
  compose start etcd consul >/dev/null
  wait_until "Etcd recovery" compose exec -T etcd etcdctl endpoint health
  wait_until "Consul recovery" curl -fsS "$CONSUL_HEALTH_URL/v1/status/leader"
  FAULTS=$((FAULTS + 1))
  NOW="$(date +%s)"
  REMAINING=$((DEADLINE - NOW))
  if (( REMAINING <= OUTAGE_SECONDS + 5 )); then
    break
  fi
  SLEEP_FOR="$FAULT_INTERVAL_SECONDS"
  if (( SLEEP_FOR > REMAINING - OUTAGE_SECONDS - 5 )); then
    SLEEP_FOR=$((REMAINING - OUTAGE_SECONDS - 5))
  fi
  if (( SLEEP_FOR > 0 )); then
    sleep "$SLEEP_FOR"
  fi
done

set +e
wait "$ETCD_PID"
ETCD_STATUS=$?
wait "$CONSUL_PID"
CONSUL_STATUS=$?
set -e
PIDS=()
cat "$ETCD_LOG"
cat "$CONSUL_LOG"
if (( ETCD_STATUS != 0 || CONSUL_STATUS != 0 )); then
  echo "registry recovery probe failed: etcd=$ETCD_STATUS consul=$CONSUL_STATUS" >&2
  exit 1
fi

ETCD_SUMMARY="$(grep -E '^roze_gateway_registry_recovery registry=etcd ' "$ETCD_LOG" | tail -n 1)"
CONSUL_SUMMARY="$(grep -E '^roze_gateway_registry_recovery registry=consul ' "$CONSUL_LOG" | tail -n 1)"
if [[ -z "$ETCD_SUMMARY" || -z "$CONSUL_SUMMARY" ]]; then
  echo "registry recovery probe did not emit both summaries" >&2
  exit 1
fi

summary_value() {
  local summary="$1"
  local key="$2"
  awk -v key="$key" '{
    for (i = 1; i <= NF; i++) {
      split($i, pair, "=")
      if (pair[1] == key) {
        print pair[2]
        exit
      }
    }
  }' <<<"$summary"
}

printf 'roze_gateway_registry_soak elapsed_seconds=%s fault_injections=%s etcd_attempts=%s etcd_successful_routes=%s etcd_disconnect_observations=%s etcd_recoveries=%s etcd_p99_route_us=%s etcd_p99_recovery_us=%s consul_attempts=%s consul_successful_routes=%s consul_disconnect_observations=%s consul_recoveries=%s consul_p99_route_us=%s consul_p99_recovery_us=%s\n' \
  "$DURATION" "$FAULTS" \
  "$(summary_value "$ETCD_SUMMARY" attempts)" \
  "$(summary_value "$ETCD_SUMMARY" successful_routes)" \
  "$(summary_value "$ETCD_SUMMARY" disconnect_observations)" \
  "$(summary_value "$ETCD_SUMMARY" recoveries)" \
  "$(summary_value "$ETCD_SUMMARY" p99_route_us)" \
  "$(summary_value "$ETCD_SUMMARY" p99_recovery_us)" \
  "$(summary_value "$CONSUL_SUMMARY" attempts)" \
  "$(summary_value "$CONSUL_SUMMARY" successful_routes)" \
  "$(summary_value "$CONSUL_SUMMARY" disconnect_observations)" \
  "$(summary_value "$CONSUL_SUMMARY" recoveries)" \
  "$(summary_value "$CONSUL_SUMMARY" p99_route_us)" \
  "$(summary_value "$CONSUL_SUMMARY" p99_recovery_us)"
