#!/usr/bin/env bash
set -u -o pipefail

# Run real-dependency probes against already-managed services. This script
# never starts, stops, or replaces a dependency.
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

EVIDENCE_DIR="${ROZE_REFERENCE_DIRECT_EVIDENCE_DIR:-$ROOT/target/reference-systems-direct}"
mkdir -p "$EVIDENCE_DIR"
LOG="$EVIDENCE_DIR/probes.log"
RUN_JSON="$EVIDENCE_DIR/run.json"
: >"$LOG"

started_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
started_epoch="$(date +%s)"
revision="$(git rev-parse HEAD 2>/dev/null || printf unknown)"
profile="${ROZE_REFERENCE_DIRECT_PROFILE:-managed-services}"
redis_endpoint="${ROZE_TEST_REDIS_URL:-redis://127.0.0.1:6379}"
redis_public_endpoint="${redis_endpoint##*@}"
s3_endpoint="${ROZE_TEST_S3_ENDPOINT:-http://127.0.0.1:9000}"
nats_endpoint="${ROZE_TEST_NATS_URL:-nats://127.0.0.1:4222}"
etcd_endpoint="${ROZE_TEST_ETCD_ENDPOINT:-http://127.0.0.1:12379}"
overall=passed
results=()

json_escape() {
  printf '%s' "$1" | python3 -c 'import json,sys; print(json.dumps(sys.stdin.read()))'
}

run_probe() {
  local name="$1"
  shift
  printf '\n[%s] %s\n' "$name" "$*" >>"$LOG"
  if "$@" >>"$LOG" 2>&1; then
    results+=("$(printf '{\"name\":%s,\"status\":\"passed\"}' "$(json_escape "$name")")")
    printf '[%s] passed\n' "$name" >>"$LOG"
  else
    overall=failed
    results+=("$(printf '{\"name\":%s,\"status\":\"failed\"}' "$(json_escape "$name")")")
    printf '[%s] failed\n' "$name" >>"$LOG"
  fi
}

run_rust_probe() {
  local name="$1"
  local package="$2"
  local test_name="$3"
  shift 3
  run_probe "$name" env "$@" cargo test -p "$package" "$test_name" -- --ignored --nocapture
}

run_rust_probe "nats-jetstream" roze-nats jetstream_round_trip_against_real_service ROZE_TEST_NATS_URL="$nats_endpoint"
run_rust_probe "etcd-registry" roze-rpc etcd_registry_registers_discovers_and_deregisters_against_real_service ROZE_TEST_ETCD_ENDPOINT="$etcd_endpoint"
run_rust_probe "etcd-config-watch" roze-config etcd_subscriber_reads_and_watches_real_service ROZE_TEST_ETCD_ENDPOINT="$etcd_endpoint"
run_rust_probe "redis-round-trip" roze-redis redis_round_trip_against_real_service ROZE_TEST_REDIS_URL="$redis_endpoint"
run_rust_probe "s3-round-trip" roze-storage s3_compatible_round_trip_against_real_service ROZE_TEST_S3_ENDPOINT="$s3_endpoint" ROZE_TEST_S3_BUCKET="${ROZE_TEST_S3_BUCKET:-roze}" ROZE_TEST_S3_ACCESS_KEY="${ROZE_TEST_S3_ACCESS_KEY:-minioadmin}" ROZE_TEST_S3_SECRET_KEY="${ROZE_TEST_S3_SECRET_KEY:-minioadmin}"

finished_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
elapsed="$(( $(date +%s) - started_epoch ))"
joined="$(IFS=,; printf '%s' "${results[*]}")"
printf '{\n  "schema_version": 1,\n  "status": "%s",\n  "revision": "%s",\n  "started_at": "%s",\n  "finished_at": "%s",\n  "elapsed_seconds": %s,\n  "results": [%s]\n}\n' "$overall" "$revision" "$started_at" "$finished_at" "$elapsed" "$joined" >"$RUN_JSON"

printf '{\n  "schema_version": 1,\n  "profile": %s,\n  "endpoints": {"nats": %s, "etcd": %s, "redis": %s, "s3": %s}\n}\n' "$(json_escape "$profile")" "$(json_escape "$nats_endpoint")" "$(json_escape "$etcd_endpoint")" "$(json_escape "$redis_public_endpoint")" "$(json_escape "$s3_endpoint")" >"$EVIDENCE_DIR/profile.json"

if command -v sha256sum >/dev/null 2>&1; then
  (cd "$EVIDENCE_DIR" && sha256sum probes.log run.json profile.json >SHA256SUMS)
fi
printf 'direct reference probes: %s (%s)\n' "$overall" "$EVIDENCE_DIR"
[[ "$overall" == passed ]]
