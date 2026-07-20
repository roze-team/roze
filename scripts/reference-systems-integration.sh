#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

if ! command -v docker >/dev/null 2>&1; then
  echo "reference systems integration requires Docker on a Linux runner" >&2
  exit 2
fi
if ! docker compose version >/dev/null 2>&1; then
  echo "reference systems integration requires the Docker Compose v2 plugin" >&2
  exit 2
fi

COMPOSE_FILE="${ROZE_REFERENCE_COMPOSE_FILE:-docker-compose.integration.yml}"
COMPOSE_PROJECT="${ROZE_REFERENCE_COMPOSE_PROJECT:-}"
KEEP_STACK="${ROZE_REFERENCE_KEEP_STACK:-0}"
TOPIC="roze-reference-events"
EVIDENCE_DIR="${ROZE_REFERENCE_EVIDENCE_DIR:-target/reference-systems-integration}"
STARTED_EPOCH="$(date +%s)"
STARTED_AT="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
RUN_STATUS="failed"
mkdir -p "$EVIDENCE_DIR"
exec > >(tee "$EVIDENCE_DIR/integration.log") 2>&1

if [[ ! -f "$COMPOSE_FILE" ]]; then
  echo "reference systems compose file not found: $COMPOSE_FILE" >&2
  exit 2
fi
ROZE_REFERENCE_COMPOSE_FILE="$COMPOSE_FILE" bash scripts/reference-systems-preflight.sh

compose() {
  if [[ -n "$COMPOSE_PROJECT" ]]; then
    docker compose -p "$COMPOSE_PROJECT" -f "$COMPOSE_FILE" "$@"
  else
    docker compose -f "$COMPOSE_FILE" "$@"
  fi
}

# Resolve the host port published by the active compose file.  The CI
# integration stack deliberately uses non-default ports, so fixed localhost
# endpoints silently exercised the wrong process (or no process at all).
host_port() {
  local service="$1"
  local container_port="$2"
  local published
  published="$(compose port "$service" "$container_port" | tail -n 1)"
  published="${published##*:}"
  if [[ -z "$published" || ! "$published" =~ ^[0-9]+$ ]]; then
    echo "unable to resolve published port for $service:$container_port" >&2
    return 1
  fi
  printf '%s' "$published"
}

cleanup() {
  if [[ "$KEEP_STACK" != "1" ]]; then
    compose down --remove-orphans
  fi
}
finalize() {
  local rc="$?"
  trap - EXIT
  if ! cleanup; then
    rc=1
    RUN_STATUS="failed"
  fi
  local finished_epoch="$(date +%s)"
  local finished_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  local revision="$(git rev-parse HEAD 2>/dev/null || printf unknown)"
  printf '%s\n' \
    "{\"schema_version\":1,\"status\":\"$RUN_STATUS\",\"revision\":\"$revision\",\"started_at\":\"$STARTED_AT\",\"finished_at\":\"$finished_at\",\"elapsed_seconds\":$((finished_epoch - STARTED_EPOCH)),\"compose_file\":\"$COMPOSE_FILE\",\"compose_project\":\"$COMPOSE_PROJECT\",\"ports\":{\"redis\":\"${REDIS_PORT:-}\",\"nats\":\"${NATS_PORT:-}\",\"etcd\":\"${ETCD_PORT:-}\",\"consul\":\"${CONSUL_PORT:-}\",\"postgres\":\"${POSTGRES_PORT:-}\",\"mysql\":\"${MYSQL_PORT:-}\",\"elasticsearch\":\"${ELASTICSEARCH_PORT:-}\",\"opensearch\":\"${OPENSEARCH_PORT:-}\",\"meilisearch\":\"${MEILISEARCH_PORT:-}\",\"minio\":\"${MINIO_PORT:-}\"}}" \
    >"$EVIDENCE_DIR/run.json"
  printf 'status=%s revision=%s elapsed_seconds=%s\n' "$RUN_STATUS" "$revision" \
    "$((finished_epoch - STARTED_EPOCH))" >"$EVIDENCE_DIR/summary.txt"
  if command -v sha256sum >/dev/null 2>&1; then
    (cd "$EVIDENCE_DIR" && sha256sum integration.log run.json summary.txt >SHA256SUMS)
  fi
  exit "$rc"
}
trap finalize EXIT

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
  etcd consul redis postgres mysql mongo zookeeper kafka nats \
  elasticsearch opensearch meilisearch minio minio-init

REDIS_PORT="$(host_port redis 6379)"
NATS_PORT="$(host_port nats 4222)"
ETCD_PORT="$(host_port etcd 2379)"
CONSUL_PORT="$(host_port consul 8500)"
POSTGRES_PORT="$(host_port postgres 5432)"
MYSQL_PORT="$(host_port mysql 3306)"
ELASTICSEARCH_PORT="$(host_port elasticsearch 9200)"
OPENSEARCH_PORT="$(host_port opensearch 9200)"
MEILISEARCH_PORT="$(host_port meilisearch 7700)"
MINIO_PORT="$(host_port minio 9000)"

wait_until "Redis" compose exec -T redis redis-cli ping
wait_until "PostgreSQL" compose exec -T postgres \
  pg_isready -U postgres -d roze
wait_until "MySQL" compose exec -T mysql \
  mysqladmin ping -h 127.0.0.1 -uroot -proot
wait_until "Etcd" compose exec -T etcd etcdctl endpoint health
wait_until "Consul" curl -fsS "http://127.0.0.1:${CONSUL_PORT}/v1/status/leader"
wait_until "NATS" bash -c "</dev/tcp/127.0.0.1/${NATS_PORT}"
wait_until "Kafka" compose exec -T kafka \
  kafka-topics --bootstrap-server 127.0.0.1:9092 --list
wait_until "Elasticsearch" curl -fsS "http://127.0.0.1:${ELASTICSEARCH_PORT}/_cluster/health"
wait_until "OpenSearch" curl -fsS "http://127.0.0.1:${OPENSEARCH_PORT}/_cluster/health"
wait_until "Meilisearch" curl -fsS "http://127.0.0.1:${MEILISEARCH_PORT}/health"
wait_until "MongoDB" compose exec -T mongo \
  mongosh --quiet --eval "db.adminCommand('ping').ok"
wait_until "MinIO" curl -fsS "http://127.0.0.1:${MINIO_PORT}/minio/health/ready"

bash scripts/generated-reference-systems.sh

export ROZE_TEST_REDIS_URL="redis://127.0.0.1:${REDIS_PORT}"
export ROZE_TEST_NATS_URL="nats://127.0.0.1:${NATS_PORT}"
export ROZE_TEST_ETCD_ENDPOINT="http://127.0.0.1:${ETCD_PORT}"
export ROZE_TEST_CONSUL_ENDPOINT="http://127.0.0.1:${CONSUL_PORT}"
export ROZE_TEST_S3_ENDPOINT="http://127.0.0.1:${MINIO_PORT}"
export ROZE_TEST_S3_BUCKET="roze"
export ROZE_TEST_S3_ACCESS_KEY="minioadmin"
export ROZE_TEST_S3_SECRET_KEY="minioadmin"
export ROZECTL_TEST_POSTGRES_URL="postgres://postgres:postgres@127.0.0.1:${POSTGRES_PORT}/roze"
export ROZECTL_TEST_MYSQL_URL="mysql://root:root@127.0.0.1:${MYSQL_PORT}/roze"

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
cargo test -p roze-storage s3_compatible_round_trip_against_real_service -- --ignored --nocapture
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

curl -fsS -X PUT "http://127.0.0.1:${ELASTICSEARCH_PORT}/roze-reference" \
  -H 'content-type: application/json' \
  -d '{"mappings":{"properties":{"tenant_id":{"type":"keyword"},"name":{"type":"text"}}}}'
curl -fsS -X POST "http://127.0.0.1:${ELASTICSEARCH_PORT}/roze-reference/_doc/reference-1?refresh=true" \
  -H 'content-type: application/json' \
  -d '{"tenant_id":"tenant-1","name":"reference product"}'
curl -fsS "http://127.0.0.1:${ELASTICSEARCH_PORT}/roze-reference/_search" \
  -H 'content-type: application/json' \
  -d '{"query":{"term":{"tenant_id":"tenant-1"}}}' |
  grep -F '"reference-1"'
curl -fsS -X DELETE "http://127.0.0.1:${ELASTICSEARCH_PORT}/roze-reference" >/dev/null

curl -fsS -X PUT "http://127.0.0.1:${OPENSEARCH_PORT}/roze-reference" \
  -H 'content-type: application/json' \
  -d '{"mappings":{"properties":{"tenant_id":{"type":"keyword"},"name":{"type":"text"}}}}' >/dev/null
curl -fsS -X POST "http://127.0.0.1:${OPENSEARCH_PORT}/roze-reference/_doc/reference-1?refresh=true" \
  -H 'content-type: application/json' \
  -d '{"tenant_id":"tenant-1","name":"reference product"}' >/dev/null
curl -fsS "http://127.0.0.1:${OPENSEARCH_PORT}/roze-reference/_search" \
  -H 'content-type: application/json' \
  -d '{"query":{"term":{"tenant_id":"tenant-1"}}}' |
  grep -F 'reference-1'
curl -fsS -X DELETE "http://127.0.0.1:${OPENSEARCH_PORT}/roze-reference" >/dev/null

compose exec -T mongo mongosh --quiet --eval \
  'db = db.getSiblingDB("roze"); db.reference.deleteMany({}); db.reference.insertOne({_id:"reference-1",tenant_id:"tenant-1",name:"reference product"}); if (db.reference.countDocuments({tenant_id:"tenant-1"}) !== 1) quit(1); db.reference.deleteMany({});'

curl -fsS -X POST "http://127.0.0.1:${MEILISEARCH_PORT}/indexes" \
  -H 'content-type: application/json' \
  -H 'Authorization: Bearer roze_meili_master_key' \
  -d '{"uid":"roze-reference","primaryKey":"id"}' >/dev/null
curl -fsS -X POST "http://127.0.0.1:${MEILISEARCH_PORT}/indexes/roze-reference/documents" \
  -H 'content-type: application/json' \
  -H 'Authorization: Bearer roze_meili_master_key' \
  -d '[{"id":"reference-1","tenant_id":"tenant-1","name":"reference product"}]' >/dev/null
curl -fsS -X POST "http://127.0.0.1:${MEILISEARCH_PORT}/indexes/roze-reference/search" \
  -H 'content-type: application/json' \
  -H 'Authorization: Bearer roze_meili_master_key' \
  -d '{"q":"reference","filter":"tenant_id = tenant-1"}' |
  grep -F 'reference-1'
curl -fsS -X DELETE "http://127.0.0.1:${MEILISEARCH_PORT}/indexes/roze-reference" \
  -H 'Authorization: Bearer roze_meili_master_key' >/dev/null

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
wait_until "NATS recovery" bash -c "</dev/tcp/127.0.0.1/${NATS_PORT}"
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
wait_until "Consul recovery" curl -fsS "http://127.0.0.1:${CONSUL_PORT}/v1/status/leader"
cargo test -p roze-gateway gateway_routes_and_recovers_through_real_consul_registry -- --ignored --nocapture

# Exercise dependency-loss and restart paths for the SQL and search systems
# used by the generated production topologies.  This is intentionally kept in
# the real-dependency script so a green compile cannot be mistaken for
# database/search recovery evidence.
compose stop postgres
expect_failure "PostgreSQL is disconnected" \
  bash -c "</dev/tcp/127.0.0.1/${POSTGRES_PORT}"
compose start postgres
wait_until "PostgreSQL recovery" compose exec -T postgres pg_isready -U postgres -d roze

compose stop mysql
expect_failure "MySQL is disconnected" \
  bash -c "</dev/tcp/127.0.0.1/${MYSQL_PORT}"
compose start mysql
wait_until "MySQL recovery" compose exec -T mysql mysqladmin ping -h 127.0.0.1 -uroot -proot

compose stop elasticsearch
expect_failure "Elasticsearch is disconnected" \
  curl -fsS "http://127.0.0.1:${ELASTICSEARCH_PORT}/_cluster/health"
compose start elasticsearch
wait_until "Elasticsearch recovery" \
  curl -fsS "http://127.0.0.1:${ELASTICSEARCH_PORT}/_cluster/health"

compose stop minio
expect_failure "MinIO is disconnected" \
  curl -fsS "http://127.0.0.1:${MINIO_PORT}/minio/health/ready"
compose start minio
wait_until "MinIO recovery" \
  curl -fsS "http://127.0.0.1:${MINIO_PORT}/minio/health/ready"
cargo test -p roze-storage s3_compatible_round_trip_against_real_service -- --ignored --nocapture

RUN_STATUS="passed"
echo "production reference systems integration passed"
