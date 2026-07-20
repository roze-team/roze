#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

COMPOSE_FILE="${ROZE_REFERENCE_COMPOSE_FILE:-docker-compose.integration.yml}"
[[ -f "$COMPOSE_FILE" ]] || { echo "missing compose file: $COMPOSE_FILE" >&2; exit 2; }
command -v docker >/dev/null 2>&1 || { echo "Docker is required" >&2; exit 2; }
docker compose version >/dev/null 2>&1 || { echo "Docker Compose v2 is required" >&2; exit 2; }

services="$(docker compose -f "$COMPOSE_FILE" config --services)"
for service in etcd consul redis postgres mysql mongo zookeeper kafka nats \
  elasticsearch opensearch meilisearch minio minio-init; do
  grep -Fx "$service" <<<"$services" >/dev/null || {
    echo "compose service is missing: $service" >&2
    exit 1
  }
done

docker compose -f "$COMPOSE_FILE" config --quiet
echo "reference systems preflight passed: $COMPOSE_FILE"
