#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

COMPOSE_FILE="${ROZE_REFERENCE_COMPOSE_FILE:-docker-compose.integration.yml}"
COMPOSE_PROJECT="${ROZE_REFERENCE_COMPOSE_PROJECT:-}"
KEEP_STACK="${ROZE_REFERENCE_KEEP_STACK:-0}"
TOPIC="roze-reference-events"

compose() {
  if [[ -n "$COMPOSE_PROJECT" ]]; then
    docker compose -p "$COMPOSE_PROJECT" -f "$COMPOSE_FILE" "$@"
  else
    docker compose -f "$COMPOSE_FILE" "$@"
  fi
}

cleanup() {
  if [[ "$KEEP_STACK" != "1" ]]; then
    compose down --remove-orphans
  fi
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

expect_failure() {
  local description="$1"
  shift
  if "$@" >/dev/null 2>&1; then
    echo "expected failure while $description" >&2
    return 1
  fi
}

compose up -d \
  etcd consul redis postgres mysql zookeeper kafka nats elasticsearch

wait_until "Redis" compose exec -T redis redis-cli ping
wait_until "PostgreSQL" compose exec -T postgres \
  pg_isready -U postgres -d roze
wait_until "MySQL" compose exec -T mysql \
  mysqladmin ping -h 127.0.0.1 -uroot -proot
wait_until "Etcd" compose exec -T etcd etcdctl endpoint health
wait_until "Consul" curl -fsS http://127.0.0.1:8500/v1/status/leader
wait_until "NATS" bash -c "</dev/tcp/127.0.0.1/4222"
wait_until "Kafka" compose exec -T kafka \
  kafka-topics --bootstrap-server 127.0.0.1:9092 --list
wait_until "Elasticsearch" curl -fsS http://127.0.0.1:9200/_cluster/health

bash scripts/generated-reference-systems.sh

export ROZE_TEST_REDIS_URL="redis://127.0.0.1:6379"
export ROZE_TEST_NATS_URL="nats://127.0.0.1:4222"
export ROZE_TEST_ETCD_ENDPOINT="http://127.0.0.1:2379"
export ROZE_TEST_CONSUL_ENDPOINT="http://127.0.0.1:8500"
export ROZECTL_TEST_POSTGRES_URL="postgres://postgres:postgres@127.0.0.1:5432/roze"
export ROZECTL_TEST_MYSQL_URL="mysql://root:root@127.0.0.1:3306/roze"

compose exec -T mysql \
  mysql -uroot -proot -e 'DROP DATABASE IF EXISTS roze; CREATE DATABASE roze;'
for schema in \
  example/production-systems/rest-crud/inventory.sql \
  example/production-systems/service-mesh/checkout.sql \
  example/production-systems/event-commerce/order.sql
do
  compose exec -T mysql mysql -uroot -proot roze <"$schema"
done
compose exec -T mysql mysql -uroot -proot roze -Nse \
  "SELECT table_name FROM information_schema.tables WHERE table_schema='roze'" |
  grep -F 'inventory_items'
compose exec -T mysql mysql -uroot -proot roze -Nse \
  "SELECT table_name FROM information_schema.tables WHERE table_schema='roze'" |
  grep -F 'checkouts'
compose exec -T mysql mysql -uroot -proot roze -Nse \
  "SELECT table_name FROM information_schema.tables WHERE table_schema='roze'" |
  grep -F 'outbox_events'
compose exec -T mysql mysql -uroot -proot roze -e \
  "INSERT INTO inbox_events (consumer, event_id) VALUES ('reference', 'event-1');"
expect_failure "a duplicate inbox event is inserted" \
  compose exec -T mysql mysql -uroot -proot roze -e \
    "INSERT INTO inbox_events (consumer, event_id) VALUES ('reference', 'event-1');"

cargo test -p roze-redis redis_round_trip_against_real_service -- --ignored --nocapture
cargo test -p roze-nats jetstream_round_trip_against_real_service -- --ignored --nocapture
cargo test -p roze-rpc etcd_registry_registers_discovers_and_deregisters_against_real_service -- --ignored --nocapture
cargo test -p roze-rpc consul_registry_registers_discovers_and_deregisters_against_real_service -- --ignored --nocapture
cargo test -p roze-gateway gateway_routes_and_recovers_through_real_etcd_registry -- --ignored --nocapture
cargo test -p roze-gateway gateway_routes_and_recovers_through_real_consul_registry -- --ignored --nocapture
ROZE_GATEWAY_REGISTRY_MANAGE_STACK=0 \
ROZE_GATEWAY_REGISTRY_RECOVERY_SECONDS=30 \
ROZE_GATEWAY_REGISTRY_FAULT_INTERVAL_SECONDS=60 \
  bash scripts/gateway-registry-recovery.sh 30
cargo test -p roze-config etcd_subscriber_reads_and_watches_real_service -- --ignored --nocapture
cargo test -p roze-report sqlite_catalog_enforces_tenant_queries_and_renders_real_exports -- --nocapture
bash scripts/model-parity-gate.sh postgres
bash scripts/model-parity-gate.sh mysql

compose exec -T kafka \
  kafka-topics --bootstrap-server 127.0.0.1:9092 --create --if-not-exists \
  --topic "$TOPIC" --partitions 1 --replication-factor 1
printf '{"event_id":"reference-1","type":"OrderCreated"}\n' |
  compose exec -T kafka \
    kafka-console-producer --bootstrap-server 127.0.0.1:9092 --topic "$TOPIC"
compose exec -T kafka \
  kafka-console-consumer --bootstrap-server 127.0.0.1:9092 --topic "$TOPIC" \
  --from-beginning --max-messages 1 --timeout-ms 10000 |
  grep -F '"event_id":"reference-1"'

curl -fsS -X PUT http://127.0.0.1:9200/roze-reference \
  -H 'content-type: application/json' \
  -d '{"mappings":{"properties":{"tenant_id":{"type":"keyword"},"name":{"type":"text"}}}}'
curl -fsS -X POST http://127.0.0.1:9200/roze-reference/_doc/reference-1?refresh=true \
  -H 'content-type: application/json' \
  -d '{"tenant_id":"tenant-1","name":"reference product"}'
curl -fsS http://127.0.0.1:9200/roze-reference/_search \
  -H 'content-type: application/json' \
  -d '{"query":{"term":{"tenant_id":"tenant-1"}}}' |
  grep -F '"reference-1"'
curl -fsS -X DELETE http://127.0.0.1:9200/roze-reference >/dev/null

compose stop redis
expect_failure "Redis is disconnected" \
  timeout 60 cargo test -p roze-redis redis_round_trip_against_real_service -- --ignored
compose start redis
wait_until "Redis recovery" compose exec -T redis redis-cli ping
cargo test -p roze-redis redis_round_trip_against_real_service -- --ignored --nocapture

compose stop nats
expect_failure "NATS is disconnected" \
  timeout 60 cargo test -p roze-nats jetstream_round_trip_against_real_service -- --ignored
compose start nats
wait_until "NATS recovery" bash -c "</dev/tcp/127.0.0.1/4222"
cargo test -p roze-nats jetstream_round_trip_against_real_service -- --ignored --nocapture

compose stop etcd
expect_failure "Etcd is disconnected" \
  timeout 60 cargo test -p roze-rpc etcd_registry_registers_discovers_and_deregisters_against_real_service -- --ignored
expect_failure "Config Center Etcd subscriber is disconnected" \
  timeout 60 cargo test -p roze-config etcd_subscriber_reads_and_watches_real_service -- --ignored
compose start etcd
wait_until "Etcd recovery" compose exec -T etcd etcdctl endpoint health
cargo test -p roze-rpc etcd_registry_registers_discovers_and_deregisters_against_real_service -- --ignored --nocapture
cargo test -p roze-config etcd_subscriber_reads_and_watches_real_service -- --ignored --nocapture

compose stop consul
expect_failure "Consul is disconnected" \
  timeout 60 cargo test -p roze-gateway gateway_routes_and_recovers_through_real_consul_registry -- --ignored
compose start consul
wait_until "Consul recovery" curl -fsS http://127.0.0.1:8500/v1/status/leader
cargo test -p roze-gateway gateway_routes_and_recovers_through_real_consul_registry -- --ignored --nocapture

echo "production reference systems integration passed"
