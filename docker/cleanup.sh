#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

docker compose \
  --env-file "$ROOT/docker/.env.example" \
  -f "$ROOT/docker/docker-compose.yml" \
  down -v --remove-orphans
