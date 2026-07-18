#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

DURATION="${ROZE_MQ_SOAK_SECONDS:-${1:-300}}"
MESSAGES="${ROZE_MQ_SOAK_MESSAGES:-18446744073709551615}"
FAULT_INTERVAL="${ROZE_MQ_FAULT_INTERVAL_SECONDS:-300}"
OUTAGE_SECONDS="${ROZE_MQ_OUTAGE_SECONDS:-10}"
RECOVERY_MARGIN="${ROZE_MQ_RECOVERY_MARGIN_SECONDS:-60}"
HARD_FAULT="${ROZE_MQ_HARD_FAULT:-1}"
NATS_HOST="${ROZE_NATS_SOAK_HOST:-127.0.0.1}"
NATS_PORT="${ROZE_NATS_SOAK_PORT:-4222}"
COMPOSE_FILE="${ROZE_MQ_COMPOSE_FILE:-docker-compose.integration.yml}"
COMPOSE_PROJECT="${ROZE_MQ_COMPOSE_PROJECT:-roze-mq-soak-${GITHUB_RUN_ID:-local}-$$}"

for value in "$DURATION" "$MESSAGES" "$FAULT_INTERVAL" "$OUTAGE_SECONDS" "$RECOVERY_MARGIN" "$NATS_PORT"; do
  case "$value" in
    ''|*[!0-9]*) echo "MQ soak values must be positive integers" >&2; exit 2 ;;
    0) echo "MQ soak values must be greater than zero" >&2; exit 2 ;;
  esac
done
case "$HARD_FAULT" in
  0|1) ;;
  *) echo "ROZE_MQ_HARD_FAULT must be 0 or 1" >&2; exit 2 ;;
esac

DEBUG_DIR="${ROZE_MQ_SOAK_DEBUG_DIR:-}"
if [[ -n "$DEBUG_DIR" ]]; then
  mkdir -p "$DEBUG_DIR"
  MEMORY_LOG="$DEBUG_DIR/memory.log"
  NATS_LOG="$DEBUG_DIR/nats.log"
  KAFKA_LOG="$DEBUG_DIR/kafka.log"
  : >"$MEMORY_LOG"
  : >"$NATS_LOG"
  : >"$KAFKA_LOG"
else
  MEMORY_LOG="$(mktemp)"
  NATS_LOG="$(mktemp)"
  KAFKA_LOG="$(mktemp)"
fi
KAFKA_BROKERS="${ROZE_KAFKA_SOAK_BROKERS:-127.0.0.1:9092}"
KAFKA_CONTAINER=""
NATS_CONTAINER=""
MEMORY_PID=""
NATS_PID=""
KAFKA_PID=""

compose() {
  docker compose -p "$COMPOSE_PROJECT" -f "$COMPOSE_FILE" "$@"
}

cleanup() {
  if [[ -n "$MEMORY_PID" ]]; then
    kill "$MEMORY_PID" 2>/dev/null || true
  fi
  if [[ -n "$NATS_PID" ]]; then
    kill "$NATS_PID" 2>/dev/null || true
  fi
  if [[ -n "$KAFKA_PID" ]]; then
    kill "$KAFKA_PID" 2>/dev/null || true
  fi
  compose down --remove-orphans || true
  if [[ -z "$DEBUG_DIR" ]]; then
    rm -f "$MEMORY_LOG" "$NATS_LOG" "$KAFKA_LOG"
  fi
}
trap cleanup EXIT

wait_for_nats() {
  if [[ -z "$NATS_CONTAINER" ]]; then
    NATS_CONTAINER="$(compose ps -q nats)"
  fi
  for _ in $(seq 1 120); do
    if timeout 2 bash -c "echo >/dev/tcp/$NATS_HOST/$NATS_PORT" \
      >/dev/null 2>&1; then
      sleep 1
      return
    fi
    sleep 1
  done
  echo "timed out waiting for NATS JetStream" >&2
  return 1
}

wait_for_kafka() {
  if [[ -z "$KAFKA_CONTAINER" ]]; then
    KAFKA_CONTAINER="$(compose ps -q kafka)"
  fi
  if [[ -z "$KAFKA_CONTAINER" ]]; then
    echo "Kafka container is not running" >&2
    return 1
  fi
  for _ in $(seq 1 180); do
    if docker exec "$KAFKA_CONTAINER" \
      kafka-topics --bootstrap-server 127.0.0.1:9092 --list >/dev/null 2>&1
    then
      return
    fi
    sleep 1
  done
  echo "timed out waiting for Kafka" >&2
  return 1
}

compose up -d nats zookeeper kafka
NATS_CONTAINER="$(compose ps -q nats)"
KAFKA_CONTAINER="$(compose ps -q kafka)"
wait_for_nats
wait_for_kafka
cargo test -p roze-mq -p roze-nats --no-run
cargo test -p roze-kafka --features rdkafka --no-run

export ROZE_MQ_SOAK_SECONDS="$DURATION"
export ROZE_MQ_SOAK_MESSAGES="$MESSAGES"
export ROZE_NATS_SOAK_SECONDS="$DURATION"
export ROZE_NATS_REQUIRE_DISCONNECT=1
export ROZE_TEST_NATS_URL="${ROZE_TEST_NATS_URL:-nats://$NATS_HOST:$NATS_PORT}"
export ROZE_KAFKA_SOAK_SECONDS="$DURATION"
export ROZE_KAFKA_REQUIRE_DISCONNECT=1
export ROZE_TEST_KAFKA_BROKERS="${ROZE_TEST_KAFKA_BROKERS:-$KAFKA_BROKERS}"

cargo test -p roze-mq \
  production_soak_in_memory_broker \
  -- --ignored --nocapture >"$MEMORY_LOG" 2>&1 &
MEMORY_PID=$!
cargo test -p roze-nats \
  production_soak_jetstream_disconnect_recovery \
  -- --ignored --nocapture >"$NATS_LOG" 2>&1 &
NATS_PID=$!
cargo test -p roze-kafka --features rdkafka \
  production_soak_rdkafka_disconnect_recovery \
  -- --ignored --nocapture >"$KAFKA_LOG" 2>&1 &
KAFKA_PID=$!

WORKLOAD_START_DEADLINE=$(( $(date +%s) + 300 ))
while true; do
  if ! kill -0 "$NATS_PID" 2>/dev/null && ! grep -q '^running 1 test' "$NATS_LOG"; then
    echo "NATS workload exited before entering its test" >&2
    cat "$NATS_LOG" >&2
    exit 1
  fi
  if ! kill -0 "$KAFKA_PID" 2>/dev/null && ! grep -q '^running 1 test' "$KAFKA_LOG"; then
    echo "Kafka workload exited before entering its test" >&2
    cat "$KAFKA_LOG" >&2
    exit 1
  fi
  if grep -q '^running 1 test' "$NATS_LOG" &&
     grep -q '^running 1 test' "$KAFKA_LOG"; then
    break
  fi
  if (( $(date +%s) >= WORKLOAD_START_DEADLINE )); then
    echo "timed out waiting for broker workloads to start" >&2
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
  # A workload may finish early (the in-memory case is intentionally short)
  # while the broker-backed tests are still starting. Let all workloads finish
  # and collect their real exit statuses below instead of turning that normal
  # condition into SIGTERM/143 failures.
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
  FAULT_MODE=graceful
  if [[ "$HARD_FAULT" == 1 ]]; then
    FAULT_MODE=hard
  fi
  echo "MQ fault injection #$FAULT_INJECTIONS mode=$FAULT_MODE" >&2
  if [[ "$HARD_FAULT" == 1 ]]; then
    docker kill "$NATS_CONTAINER" "$KAFKA_CONTAINER" >/dev/null
  else
    compose stop nats kafka
  fi
  sleep "$OUTAGE_SECONDS"
  compose start nats kafka
  wait_for_nats
  wait_for_kafka
  echo "MQ fault recovery #$FAULT_INJECTIONS complete" >&2
  NEXT_FAULT=$(($(date +%s) + FAULT_INTERVAL))
done

set +e
wait "$MEMORY_PID"
MEMORY_STATUS=$?
MEMORY_PID=""
wait "$NATS_PID"
NATS_STATUS=$?
NATS_PID=""
wait "$KAFKA_PID"
KAFKA_STATUS=$?
KAFKA_PID=""
set -e

cat "$MEMORY_LOG"
cat "$NATS_LOG"
cat "$KAFKA_LOG"
if (( MEMORY_STATUS != 0 || NATS_STATUS != 0 || KAFKA_STATUS != 0 )); then
  echo "MQ soak workload failed: memory=$MEMORY_STATUS nats=$NATS_STATUS kafka=$KAFKA_STATUS" >&2
  exit 1
fi
if (( FAULT_INJECTIONS == 0 )); then
  echo "MQ soak completed without NATS fault injection" >&2
  exit 1
fi

MEMORY_SUMMARY="$(grep -E '^roze_mq_soak ' "$MEMORY_LOG" | tail -n 1 || true)"
NATS_SUMMARY="$(grep -E '^roze_nats_soak ' "$NATS_LOG" | tail -n 1 || true)"
KAFKA_SUMMARY="$(grep -E '^roze_kafka_soak ' "$KAFKA_LOG" | tail -n 1 || true)"
if [[ -z "$MEMORY_SUMMARY" || -z "$NATS_SUMMARY" || -z "$KAFKA_SUMMARY" ]]; then
  echo "MQ soak workload did not emit all boundary summaries" >&2
  exit 1
fi
printf 'roze_mq_soak %s %s %s nats_fault_injections=%s kafka_fault_injections=%s\n' \
  "${MEMORY_SUMMARY#roze_mq_soak }" \
  "${NATS_SUMMARY#roze_nats_soak }" \
  "${KAFKA_SUMMARY#roze_kafka_soak }" \
  "$FAULT_INJECTIONS" \
  "$FAULT_INJECTIONS"
