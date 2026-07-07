#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WITH_COMPOSE=0
RUN_IGNORED=1

for arg in "$@"; do
  case "$arg" in
    --with-compose)
      WITH_COMPOSE=1
      ;;
    --skip-generated)
      RUN_IGNORED=0
      ;;
    *)
      echo "unknown argument: $arg" >&2
      exit 2
      ;;
  esac
done

cd "$ROOT"

if [[ "$WITH_COMPOSE" == "1" ]]; then
  docker compose -f docker-compose.integration.yml up -d
fi

cargo fmt --all -- --check
cargo test -p rozectl

if [[ "$RUN_IGNORED" == "1" ]]; then
  cargo test -p rozectl generated_rest_project_compiles_with_model_and_search -- --ignored
  cargo test -p rozectl generated_rpc_project_compiles -- --ignored
  cargo test -p rozectl generated_stream_project_compiles -- --ignored
fi

cargo test -p roze-cache
cargo test -p roze-redis
cargo test -p roze-search
cargo test -p roze-mq
cargo test -p roze-kafka
cargo test -p roze-nats
cargo test -p roze-gateway
cargo test -p roze-health
cargo test -p roze-service
cargo test -p roze-bootstrap
cargo test -p roze-shutdown
cargo test -p roze-job

cargo check -p user-service
cargo check -p roze-gateway-app

echo "production smoke passed"
