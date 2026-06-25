#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
COMPOSE=(docker compose --env-file "$ROOT/docker/.env.example" -f "$ROOT/docker/docker-compose.yml")

pass() {
  printf 'PASS %s\n' "$1"
}

"${COMPOSE[@]}" exec -T postgres pg_isready -U postgres -d roze >/dev/null
"${COMPOSE[@]}" exec -T postgres psql -U postgres -d roze -c 'SELECT 1;' >/dev/null
pass postgres

"${COMPOSE[@]}" exec -T mysql mysqladmin ping -h 127.0.0.1 -uroot -proot >/dev/null
"${COMPOSE[@]}" exec -T mysql mysql -uroot -proot roze -e 'SELECT 1;' >/dev/null
pass mysql

"${COMPOSE[@]}" exec -T redis redis-cli ping | grep -q PONG
pass redis

"${COMPOSE[@]}" exec -T kafka kafka-broker-api-versions --bootstrap-server kafka:29092 >/dev/null
"${COMPOSE[@]}" exec -T kafka kafka-topics --bootstrap-server kafka:29092 --list >/dev/null
pass kafka

"${COMPOSE[@]}" exec -T nats wget -qO- http://127.0.0.1:8222/healthz >/dev/null
pass nats

"${COMPOSE[@]}" exec -T mongo mongosh roze --quiet --eval "db.adminCommand('ping').ok" | grep -q 1
pass mongo

"${COMPOSE[@]}" exec -T elasticsearch curl -fsS http://127.0.0.1:9200/_cluster/health >/dev/null
pass elasticsearch

"${COMPOSE[@]}" exec -T opensearch curl -fsS http://127.0.0.1:9200/_cluster/health >/dev/null
pass opensearch

"${COMPOSE[@]}" exec -T meilisearch wget -qO- http://127.0.0.1:7700/health >/dev/null
pass meilisearch

"${COMPOSE[@]}" exec -T etcd etcdctl endpoint health >/dev/null
pass etcd

"${COMPOSE[@]}" exec -T consul consul members >/dev/null
pass consul

printf 'ALL PASSED\n'
