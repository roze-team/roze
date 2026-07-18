#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

grep -F 'uses: actions/attest@v4' .github/workflows/production-soak.yml >/dev/null
grep -F 'steps.attest.outputs.attestation-url' .github/workflows/production-soak.yml >/dev/null
grep -F 'scripts/production-evidence-promote.sh' .github/workflows/production-soak.yml >/dev/null
grep -F 'scripts/production-evidence-report-verify.sh' .github/workflows/production-soak.yml >/dev/null
grep -F 'name: Upload raw evidence for attestation' .github/workflows/production-soak.yml >/dev/null
grep -F 'name: Upload complete evidence' .github/workflows/production-soak.yml >/dev/null
grep -F 'scripts/production-soak-preflight.sh' .github/workflows/production-soak.yml >/dev/null

SMOKE_ROOT="${ROZE_EVIDENCE_PROMOTION_SMOKE_DIR:-$ROOT/target/production-evidence-promotion-smoke}"
BUNDLE="$SMOKE_ROOT/bundle"
REPORT="$SMOKE_ROOT/smoke-gateway-24h.md"
REVISION="$(git rev-parse HEAD)"
DIGEST="sha256:$(printf 'a%.0s' {1..64})"

rm -rf "$SMOKE_ROOT"
mkdir -p "$BUNDLE"
trap 'rm -rf "$SMOKE_ROOT"' EXIT

write_run() {
  local area="$1"
  local elapsed="$2"
  printf '{"schema_version":1,"area":"%s","duration":"24h","revision":"%s","status":"passed","workload_exit_code":0,"required_seconds":86400,"elapsed_seconds":%s,"started_at":"2026-07-16T00:00:00Z","finished_at":"2026-07-17T00:00:00Z","host_samples":2880,"minimum_memory_available_kib":1048576,"first_memory_available_kib":2097152,"last_memory_available_kib":2087152,"memory_growth_kib":10000,"maximum_host_tasks":128,"maximum_tcp_established":32,"maximum_allocated_file_handles":4096,"cpu_busy_basis_points":2500}\n' \
    "$area" "$REVISION" "$elapsed" >"$BUNDLE/run.json"
}

write_checksums() {
  (
    cd "$BUNDLE"
    sha256sum \
      run.json summary.md boundary-summary.txt workload.log host.jsonl \
      >SHA256SUMS
  )
}

write_run gateway 86400
printf '# completed fixed-runner smoke fixture\n' >"$BUNDLE/summary.md"
printf 'roze_gateway_soak elapsed_seconds=86400 cycles=42 cycles_per_hour=1 requests=4200 request_errors=0 p50_request_us=100 p95_request_us=200 p99_request_us=300 p50_cycle_ms=1000 p95_cycle_ms=1200 p99_cycle_ms=1500 retry_recoveries=42 timeout_fallbacks=42 config_rejections=42 websocket_checks=42 sse_checks=42 registry_elapsed_seconds=86400 registry_fault_injections=12 etcd_attempts=4200 etcd_successful_routes=4100 etcd_disconnect_observations=100 etcd_recoveries=12 etcd_p99_route_us=400 etcd_p99_recovery_us=2000000 consul_attempts=4200 consul_successful_routes=4100 consul_disconnect_observations=100 consul_recoveries=12 consul_p99_route_us=500 consul_p99_recovery_us=3000000\n' \
  >"$BUNDLE/boundary-summary.txt"
printf 'completed\n' >"$BUNDLE/workload.log"
printf '{"ts":"2026-07-16T00:00:00Z","load":"0.1 0.1 0.1","memory_available_kib":1048576,"tasks":128,"tcp_established":32,"allocated_file_handles":4096,"cpu_total_ticks":1000,"cpu_idle_ticks":750}\n' \
  >"$BUNDLE/host.jsonl"
write_checksums

bash scripts/production-evidence-promote.sh \
  --bundle "$BUNDLE" \
  --artifact-id 123456 \
  --artifact-digest "$DIGEST" \
  --artifact-url https://github.com/roze-team/roze/actions/runs/123456 \
  --attestation-url https://github.com/roze-team/roze/attestations/123456 \
  --out "$REPORT" >/dev/null

grep -F 'verdict: pass' "$REPORT" >/dev/null
grep -F "revision: $REVISION" "$REPORT" >/dev/null
grep -F 'elapsed_seconds: 86400' "$REPORT" >/dev/null
grep -F 'roze_gateway_soak elapsed_seconds=86400 cycles=42' "$REPORT" >/dev/null
bash scripts/production-evidence-report-verify.sh "$REPORT" gateway >/dev/null
sed 's/ registry_elapsed_seconds=/ legacy_registry_elapsed_seconds=/' \
  "$REPORT" >"$SMOKE_ROOT/invalid-gateway-registry-24h.md"
expect_failure() {
  local description="$1"
  shift
  if "$@" >"$SMOKE_ROOT/failure.out" 2>"$SMOKE_ROOT/failure.err"; then
    echo "expected evidence promotion failure: $description" >&2
    exit 1
  fi
}
expect_failure "Gateway report without real registry elapsed evidence" \
  bash scripts/production-evidence-report-verify.sh \
    "$SMOKE_ROOT/invalid-gateway-registry-24h.md" gateway

printf 'tampered\n' >>"$BUNDLE/workload.log"
expect_failure "checksum mismatch" \
  bash scripts/production-evidence-promote.sh \
    --bundle "$BUNDLE" \
    --artifact-id 123456 \
    --artifact-digest "$DIGEST" \
    --artifact-url https://github.com/roze-team/roze/actions/runs/123456 \
    --attestation-url https://github.com/roze-team/roze/attestations/123456 \
    --out "$REPORT"

sed 's/artifact_digest: sha256:a/artifact_digest: sha256:z/' \
  "$REPORT" >"$SMOKE_ROOT/invalid-gateway-24h.md"
expect_failure "invalid promoted report metadata" \
  bash scripts/production-evidence-report-verify.sh \
    "$SMOKE_ROOT/invalid-gateway-24h.md" gateway

printf 'completed\n' >"$BUNDLE/workload.log"
write_run gateway 86399
write_checksums
expect_failure "shortened elapsed duration" \
  bash scripts/production-evidence-promote.sh \
    --bundle "$BUNDLE" \
    --artifact-id 123456 \
    --artifact-digest "$DIGEST" \
    --artifact-url https://github.com/roze-team/roze/actions/runs/123456 \
    --attestation-url https://github.com/roze-team/roze/attestations/123456 \
    --out "$REPORT"

verify_area() {
  local run_area="$1"
  local evidence_area="$2"
  local summary="$3"
  local report="$SMOKE_ROOT/smoke-${evidence_area}-24h.md"

  write_run "$run_area" 86400
  printf '%s\n' "$summary" >"$BUNDLE/boundary-summary.txt"
  write_checksums
  bash scripts/production-evidence-promote.sh \
    --bundle "$BUNDLE" \
    --artifact-id 123456 \
    --artifact-digest "$DIGEST" \
    --artifact-url https://github.com/roze-team/roze/actions/runs/123456 \
    --attestation-url https://github.com/roze-team/roze/attestations/123456 \
    --out "$report" >/dev/null
  bash scripts/production-evidence-report-verify.sh \
    "$report" "$evidence_area" >/dev/null
}

verify_area \
  mq \
  mq \
  'roze_mq_soak elapsed_ms=86400000 sent=1000 acked=900 nacked=100 messages_per_second_milli=11574 p50_delivery_us=10 p95_delivery_us=20 p99_delivery_us=30 replayed=1 replay_recovery_us=100 published=1010 duplicated=10 dead_lettered=100 nats_elapsed_ms=86400000 nats_attempts=1000 nats_delivered=900 nats_disconnect_observations=100 nats_recoveries=10 nats_messages_per_second_milli=10416 nats_p99_delivery_us=500 nats_p99_recovery_us=10000000 nats_fault_injections=10 kafka_elapsed_ms=86400000 kafka_attempts=1000 kafka_delivered=850 kafka_disconnect_observations=150 kafka_recoveries=10 kafka_messages_per_second_milli=9837 kafka_p99_delivery_us=1000 kafka_p99_recovery_us=20000000 kafka_fault_injections=10'
sed 's/ nats_elapsed_ms=/ legacy_nats_elapsed_ms=/' \
  "$SMOKE_ROOT/smoke-mq-24h.md" >"$SMOKE_ROOT/invalid-mq-24h.md"
expect_failure "MQ report without real NATS elapsed evidence" \
  bash scripts/production-evidence-report-verify.sh \
    "$SMOKE_ROOT/invalid-mq-24h.md" mq
sed 's/ kafka_elapsed_ms=/ legacy_kafka_elapsed_ms=/' \
  "$SMOKE_ROOT/smoke-mq-24h.md" >"$SMOKE_ROOT/invalid-mq-kafka-24h.md"
expect_failure "MQ report without real Kafka elapsed evidence" \
  bash scripts/production-evidence-report-verify.sh \
    "$SMOKE_ROOT/invalid-mq-kafka-24h.md" mq
verify_area \
  config-center \
  config-center \
  'roze_config_center_soak elapsed_ms=86400000 accepted=1000 rejected=60 rollbacks=35 updates_per_second_milli=11574 p50_update_us=100 p95_update_us=200 p99_update_us=300 p99_rollback_us=400 versions=1001 audit_records=2095 etcd_elapsed_ms=86400000 etcd_attempts=1000 etcd_writes=900 etcd_reads=900 etcd_watch_updates=850 etcd_disconnect_observations=100 etcd_recoveries=10 etcd_operations_per_second_milli=11574 etcd_p99_operation_us=500 etcd_p99_recovery_us=10000000 etcd_fault_injections=10'
verify_area \
  lifecycle \
  lifecycle \
  'roze_lifecycle_soak elapsed_ms=86400000 cycles=1000 cycles_per_second_milli=11 p50_cycle_us=100 p95_cycle_us=200 p99_cycle_us=300 failed_task_detections=8 drain_timeout_detections=4 p99_fault_detection_us=500 worker_exits=4000 stop_hooks=4000 running_snapshots=1000 stopped_snapshots=1000 max_service_count=4'
verify_area \
  generated-systems \
  generated-services \
  'roze_generated_systems_soak iterations=10 elapsed_seconds=86400 iterations_per_hour=1 p50_iteration_ms=1000 p95_iteration_ms=2000 p99_iteration_ms=3000'

echo "production evidence promotion smoke passed"
