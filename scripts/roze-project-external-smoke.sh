#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
COMPOSE_FILE="${ROOT_DIR}/docker/docker-compose.yml"
ENV_FILE="${ROOT_DIR}/docker/.env.example"

cleanup() {
  "${ROOT_DIR}/docker/cleanup.sh" >/dev/null
}

trap cleanup EXIT

cd "${ROOT_DIR}"

docker compose --env-file "${ENV_FILE}" -f "${COMPOSE_FILE}" up -d --wait
"${ROOT_DIR}/docker/verify.sh"

cargo run -p roze-example --bin external_verify
