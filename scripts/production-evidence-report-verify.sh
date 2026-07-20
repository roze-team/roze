#!/usr/bin/env bash
set -euo pipefail

REPORT="${1:-}"
EXPECTED_AREA="${2:-}"

if [[ -z "$REPORT" || -z "$EXPECTED_AREA" ]]; then
  echo "usage: bash scripts/production-evidence-report-verify.sh <report> <area>" >&2
  exit 2
fi
if [[ ! -f "$REPORT" ]]; then
  echo "evidence report does not exist: $REPORT" >&2
  exit 2
fi

frontmatter_value() {
  local key="$1"
  awk -F': ' -v key="$key" '
    NR == 1 && $0 != "---" { exit 1 }
    NR > 1 && $0 == "---" { exit }
    $1 == key { print substr($0, length(key) + 3); found = 1 }
    END { if (!found) exit 1 }
  ' "$REPORT"
}

SCHEMA_VERSION="$(frontmatter_value schema_version)"
AREA="$(frontmatter_value area)"
DURATION="$(frontmatter_value duration)"
VERDICT="$(frontmatter_value verdict)"
REVISION="$(frontmatter_value revision)"
RUN_STATUS="$(frontmatter_value run_status)"
REQUIRED_SECONDS="$(frontmatter_value required_seconds)"
ELAPSED_SECONDS="$(frontmatter_value elapsed_seconds)"
STARTED_AT="$(frontmatter_value started_at)"
FINISHED_AT="$(frontmatter_value finished_at)"
HOST_SAMPLES="$(frontmatter_value host_samples)"
MINIMUM_MEMORY_AVAILABLE_KIB="$(frontmatter_value minimum_memory_available_kib)"
FIRST_MEMORY_AVAILABLE_KIB="$(frontmatter_value first_memory_available_kib)"
LAST_MEMORY_AVAILABLE_KIB="$(frontmatter_value last_memory_available_kib)"
MEMORY_GROWTH_KIB="$(frontmatter_value memory_growth_kib)"
MAXIMUM_HOST_TASKS="$(frontmatter_value maximum_host_tasks)"
MAXIMUM_TCP_ESTABLISHED="$(frontmatter_value maximum_tcp_established)"
MAXIMUM_ALLOCATED_FILE_HANDLES="$(frontmatter_value maximum_allocated_file_handles)"
CPU_BUSY_BASIS_POINTS="$(frontmatter_value cpu_busy_basis_points)"
ARTIFACT_ID="$(frontmatter_value artifact_id)"
ARTIFACT_DIGEST="$(frontmatter_value artifact_digest)"
ARTIFACT_URL="$(frontmatter_value artifact_url)"
ATTESTATION_URL="$(frontmatter_value attestation_url)"
CHECKSUMS_SHA256="$(frontmatter_value checksums_sha256)"
RUNNER_OS="$(frontmatter_value runner_os)"
RUNNER_KERNEL="$(frontmatter_value runner_kernel)"
RUNNER_ARCH="$(frontmatter_value runner_arch)"
RUNNER_RUSTC="$(frontmatter_value runner_rustc)"
RUNNER_CARGO="$(frontmatter_value runner_cargo)"
RUNNER_NODE="$(frontmatter_value runner_node)"
RUNNER_DOCKER="$(frontmatter_value runner_docker)"
RUNNER_COMPOSE="$(frontmatter_value runner_compose)"
RUNNER_SHA256SUM="$(frontmatter_value runner_sha256sum)"

[[ "$SCHEMA_VERSION" == "1" ]] || {
  echo "unsupported evidence report schema: $SCHEMA_VERSION" >&2
  exit 1
}
[[ "$AREA" == "$EXPECTED_AREA" ]] || {
  echo "evidence area mismatch: expected=$EXPECTED_AREA actual=$AREA" >&2
  exit 1
}
[[ "$VERDICT" == "pass" && "$RUN_STATUS" == "passed" ]] || {
  echo "evidence report is not a passing completed run" >&2
  exit 1
}
[[ "$REVISION" =~ ^[0-9a-f]{40}$ ]] || {
  echo "evidence revision is not a full Git SHA" >&2
  exit 1
}
[[ "$STARTED_AT" =~ ^[0-9]{4}-[0-9]{2}-[0-9]{2}T ]] || {
  echo "evidence start timestamp is invalid" >&2
  exit 1
}
[[ "$FINISHED_AT" =~ ^[0-9]{4}-[0-9]{2}-[0-9]{2}T ]] || {
  echo "evidence finish timestamp is invalid" >&2
  exit 1
}
[[ "$ARTIFACT_ID" =~ ^[1-9][0-9]*$ ]] || {
  echo "evidence artifact ID is invalid" >&2
  exit 1
}
[[ "$ARTIFACT_DIGEST" =~ ^sha256:[0-9a-f]{64}$ ]] || {
  echo "evidence artifact digest is invalid" >&2
  exit 1
}
[[ "$CHECKSUMS_SHA256" =~ ^[0-9a-f]{64}$ ]] || {
  echo "evidence checksum-manifest digest is invalid" >&2
  exit 1
}
for runner_value in "$RUNNER_OS" "$RUNNER_KERNEL" "$RUNNER_ARCH" "$RUNNER_RUSTC" \
  "$RUNNER_CARGO" "$RUNNER_NODE" "$RUNNER_DOCKER" "$RUNNER_COMPOSE" "$RUNNER_SHA256SUM"; do
  [[ -n "$runner_value" ]] || {
    echo "evidence report is missing fixed-runner metadata" >&2
    exit 1
  }
done
[[ "$ARTIFACT_URL" =~ ^https://github\.com/.+/actions/runs/[1-9][0-9]* ]] || {
  echo "evidence artifact URL is invalid" >&2
  exit 1
}
[[ "$ATTESTATION_URL" =~ ^https://github\.com/.+/attestations/[1-9][0-9]* ]] || {
  echo "evidence attestation URL is invalid" >&2
  exit 1
}

case "$DURATION" in
  24h) [[ "$REQUIRED_SECONDS" == "86400" ]] ;;
  72h) [[ "$REQUIRED_SECONDS" == "259200" ]] ;;
  *) false ;;
esac || {
  echo "evidence duration and required seconds are inconsistent" >&2
  exit 1
}
[[ "$(basename "$REPORT")" == *-"$EXPECTED_AREA"-"$DURATION".md ]] || {
  echo "evidence filename does not match its area and duration" >&2
  exit 1
}

for value in \
  "$ELAPSED_SECONDS" "$HOST_SAMPLES" "$MINIMUM_MEMORY_AVAILABLE_KIB" \
  "$FIRST_MEMORY_AVAILABLE_KIB" "$LAST_MEMORY_AVAILABLE_KIB" \
  "$MEMORY_GROWTH_KIB" "$MAXIMUM_HOST_TASKS" "$MAXIMUM_TCP_ESTABLISHED" \
  "$MAXIMUM_ALLOCATED_FILE_HANDLES" "$CPU_BUSY_BASIS_POINTS"
do
  [[ "$value" =~ ^[0-9]+$ ]] || {
    echo "evidence report contains a non-numeric measurement" >&2
    exit 1
  }
done
(( ELAPSED_SECONDS >= REQUIRED_SECONDS )) || {
  echo "evidence run ended before the required duration" >&2
  exit 1
}
(( HOST_SAMPLES > 0 && MINIMUM_MEMORY_AVAILABLE_KIB > 0 &&
   FIRST_MEMORY_AVAILABLE_KIB > 0 && LAST_MEMORY_AVAILABLE_KIB > 0 &&
   MAXIMUM_HOST_TASKS > 0 && MAXIMUM_ALLOCATED_FILE_HANDLES > 0 &&
   CPU_BUSY_BASIS_POINTS <= 10000 )) || {
  echo "evidence report is missing resource measurements" >&2
  exit 1
}
if grep -Eq 'TBD|inconclusive|not_reported' "$REPORT"; then
  echo "evidence report still contains incomplete values" >&2
  exit 1
fi
BOUNDARY_SUMMARY="$(grep -E '^roze_[a-z_]+_soak[[:space:]]' "$REPORT" | tail -n 1 || true)"
if [[ -z "$BOUNDARY_SUMMARY" ]]; then
  echo "evidence report is missing its standardized boundary summary" >&2
  exit 1
fi

summary_value() {
  local key="$1"
  local part
  for part in $BOUNDARY_SUMMARY; do
    case "$part" in
      "$key"=*)
        printf '%s\n' "${part#*=}"
        return
        ;;
    esac
  done
  echo "boundary summary is missing $key" >&2
  return 1
}

summary_integer() {
  local key="$1"
  local value
  value="$(summary_value "$key")"
  if [[ ! "$value" =~ ^[0-9]+$ ]]; then
    echo "boundary summary field $key is not an unsigned integer" >&2
    exit 1
  fi
  printf '%s\n' "$value"
}

require_positive_summary() {
  local key="$1"
  local value
  value="$(summary_integer "$key")"
  if (( value == 0 )); then
    echo "boundary summary field $key must be positive" >&2
    exit 1
  fi
  printf '%s\n' "$value"
}

require_ordered_percentiles() {
  local p50="$1"
  local p95="$2"
  local p99="$3"
  if (( p50 > p95 || p95 > p99 )); then
    echo "boundary latency percentiles are not monotonic" >&2
    exit 1
  fi
}

if (( HOST_SAMPLES * 300 < REQUIRED_SECONDS * 9 )); then
  echo "host sampling covered less than 90 percent of the run" >&2
  exit 1
fi
if (( MINIMUM_MEMORY_AVAILABLE_KIB > FIRST_MEMORY_AVAILABLE_KIB ||
      MINIMUM_MEMORY_AVAILABLE_KIB > LAST_MEMORY_AVAILABLE_KIB )); then
  echo "minimum available memory is inconsistent with endpoint samples" >&2
  exit 1
fi
EXPECTED_MEMORY_GROWTH=0
if (( FIRST_MEMORY_AVAILABLE_KIB > LAST_MEMORY_AVAILABLE_KIB )); then
  EXPECTED_MEMORY_GROWTH=$((FIRST_MEMORY_AVAILABLE_KIB - LAST_MEMORY_AVAILABLE_KIB))
fi
if (( MEMORY_GROWTH_KIB != EXPECTED_MEMORY_GROWTH )); then
  echo "reported memory growth is inconsistent with endpoint samples" >&2
  exit 1
fi
if (( MEMORY_GROWTH_KIB * 5 > FIRST_MEMORY_AVAILABLE_KIB )); then
  echo "available memory declined by more than the 20 percent evidence limit" >&2
  exit 1
fi

case "$EXPECTED_AREA" in
  gateway)
    [[ "$BOUNDARY_SUMMARY" == roze_gateway_soak\ * ]] || exit 1
    GATEWAY_ELAPSED="$(require_positive_summary elapsed_seconds)"
    GATEWAY_CYCLES="$(require_positive_summary cycles)"
    GATEWAY_REQUESTS="$(require_positive_summary requests)"
    GATEWAY_ERRORS="$(summary_integer request_errors)"
    GATEWAY_P50="$(require_positive_summary p50_request_us)"
    GATEWAY_P95="$(require_positive_summary p95_request_us)"
    GATEWAY_P99="$(require_positive_summary p99_request_us)"
    GATEWAY_CYCLE_P50="$(require_positive_summary p50_cycle_ms)"
    GATEWAY_CYCLE_P95="$(require_positive_summary p95_cycle_ms)"
    GATEWAY_CYCLE_P99="$(require_positive_summary p99_cycle_ms)"
    require_ordered_percentiles "$GATEWAY_P50" "$GATEWAY_P95" "$GATEWAY_P99"
    require_ordered_percentiles \
      "$GATEWAY_CYCLE_P50" "$GATEWAY_CYCLE_P95" "$GATEWAY_CYCLE_P99"
    (( GATEWAY_ELAPSED >= REQUIRED_SECONDS && GATEWAY_ERRORS == 0 )) || exit 1
    (( GATEWAY_P99 <= 250000 )) || {
      echo "gateway request p99 exceeds the 250ms evidence objective" >&2
      exit 1
    }
    for field in retry_recoveries timeout_fallbacks config_rejections websocket_checks sse_checks; do
      value="$(require_positive_summary "$field")"
      (( value == GATEWAY_CYCLES )) || {
        echo "gateway field $field must equal completed cycles" >&2
        exit 1
      }
    done
    (( GATEWAY_REQUESTS >= GATEWAY_CYCLES )) || exit 1
    REGISTRY_ELAPSED="$(require_positive_summary registry_elapsed_seconds)"
    REGISTRY_FAULTS="$(require_positive_summary registry_fault_injections)"
    (( REGISTRY_ELAPSED >= REQUIRED_SECONDS && REGISTRY_FAULTS > 0 )) || exit 1
    for registry in etcd consul; do
      attempts="$(require_positive_summary "${registry}_attempts")"
      successes="$(require_positive_summary "${registry}_successful_routes")"
      disconnects="$(require_positive_summary "${registry}_disconnect_observations")"
      recoveries="$(require_positive_summary "${registry}_recoveries")"
      route_p99="$(require_positive_summary "${registry}_p99_route_us")"
      recovery_p99="$(require_positive_summary "${registry}_p99_recovery_us")"
      (( attempts >= successes && successes > recoveries )) || exit 1
      (( disconnects >= recoveries && recoveries >= REGISTRY_FAULTS )) || {
        echo "$registry did not recover from every injected registry outage" >&2
        exit 1
      }
      (( route_p99 <= 250000 )) || {
        echo "$registry Gateway route p99 exceeds the 250ms evidence objective" >&2
        exit 1
      }
      (( recovery_p99 <= 60000000 )) || {
        echo "$registry Gateway recovery p99 exceeds the 60s evidence objective" >&2
        exit 1
      }
    done
    ;;
  mq)
    [[ "$BOUNDARY_SUMMARY" == roze_mq_soak\ * ]] || exit 1
    MQ_ELAPSED="$(require_positive_summary elapsed_ms)"
    MQ_SENT="$(require_positive_summary sent)"
    MQ_ACKED="$(summary_integer acked)"
    MQ_NACKED="$(summary_integer nacked)"
    MQ_PUBLISHED="$(require_positive_summary published)"
    MQ_DUPLICATED="$(require_positive_summary duplicated)"
    MQ_DEAD_LETTERED="$(require_positive_summary dead_lettered)"
    MQ_P50="$(require_positive_summary p50_delivery_us)"
    MQ_P95="$(require_positive_summary p95_delivery_us)"
    MQ_P99="$(require_positive_summary p99_delivery_us)"
    MQ_REPLAYED="$(require_positive_summary replayed)"
    MQ_REPLAY_RECOVERY="$(require_positive_summary replay_recovery_us)"
    MQ_NATS_ELAPSED="$(require_positive_summary nats_elapsed_ms)"
    MQ_NATS_ATTEMPTS="$(require_positive_summary nats_attempts)"
    MQ_NATS_DELIVERED="$(require_positive_summary nats_delivered)"
    MQ_NATS_DISCONNECTS="$(require_positive_summary nats_disconnect_observations)"
    MQ_NATS_RECOVERIES="$(require_positive_summary nats_recoveries)"
    MQ_NATS_DELIVERY_P99="$(require_positive_summary nats_p99_delivery_us)"
    MQ_NATS_RECOVERY_P99="$(require_positive_summary nats_p99_recovery_us)"
    MQ_NATS_FAULTS="$(require_positive_summary nats_fault_injections)"
    MQ_KAFKA_ELAPSED="$(require_positive_summary kafka_elapsed_ms)"
    MQ_KAFKA_ATTEMPTS="$(require_positive_summary kafka_attempts)"
    MQ_KAFKA_DELIVERED="$(require_positive_summary kafka_delivered)"
    MQ_KAFKA_DISCONNECTS="$(require_positive_summary kafka_disconnect_observations)"
    MQ_KAFKA_RECOVERIES="$(require_positive_summary kafka_recoveries)"
    MQ_KAFKA_DELIVERY_P99="$(require_positive_summary kafka_p99_delivery_us)"
    MQ_KAFKA_RECOVERY_P99="$(require_positive_summary kafka_p99_recovery_us)"
    MQ_KAFKA_FAULTS="$(require_positive_summary kafka_fault_injections)"
    require_positive_summary messages_per_second_milli >/dev/null
    require_positive_summary nats_messages_per_second_milli >/dev/null
    require_positive_summary kafka_messages_per_second_milli >/dev/null
    require_ordered_percentiles "$MQ_P50" "$MQ_P95" "$MQ_P99"
    (( MQ_P99 <= 1000000 )) || {
      echo "MQ delivery p99 exceeds the 1s evidence objective" >&2
      exit 1
    }
    (( MQ_REPLAYED == 1 && MQ_REPLAY_RECOVERY <= 1000000 )) || {
      echo "MQ DLQ replay did not satisfy the 1s recovery objective" >&2
      exit 1
    }
    (( MQ_ELAPSED >= REQUIRED_SECONDS * 1000 )) || exit 1
    (( MQ_ACKED + MQ_NACKED == MQ_SENT )) || exit 1
    (( MQ_DEAD_LETTERED == MQ_NACKED )) || exit 1
    (( MQ_PUBLISHED >= MQ_SENT && MQ_PUBLISHED <= MQ_SENT * 2 )) || exit 1
    (( MQ_NATS_ELAPSED >= REQUIRED_SECONDS * 1000 &&
       MQ_NATS_DELIVERED <= MQ_NATS_ATTEMPTS &&
       MQ_NATS_DISCONNECTS >= MQ_NATS_FAULTS &&
       MQ_NATS_RECOVERIES > 0 &&
       MQ_NATS_DELIVERY_P99 <= 5000000 &&
       MQ_NATS_RECOVERY_P99 <= 30000000 )) || {
      echo "MQ NATS disconnect/recovery evidence is outside policy" >&2
      exit 1
    }
    (( MQ_KAFKA_ELAPSED >= REQUIRED_SECONDS * 1000 &&
       MQ_KAFKA_DELIVERED <= MQ_KAFKA_ATTEMPTS &&
       MQ_KAFKA_DISCONNECTS >= MQ_KAFKA_FAULTS &&
       MQ_KAFKA_RECOVERIES > 0 &&
       MQ_KAFKA_DELIVERY_P99 <= 10000000 &&
       MQ_KAFKA_RECOVERY_P99 <= 60000000 )) || {
      echo "MQ Kafka disconnect/recovery evidence is outside policy" >&2
      exit 1
    }
    ;;
  config-center)
    [[ "$BOUNDARY_SUMMARY" == roze_config_center_soak\ * ]] || exit 1
    CONFIG_ELAPSED="$(require_positive_summary elapsed_ms)"
    require_positive_summary accepted >/dev/null
    require_positive_summary rejected >/dev/null
    require_positive_summary rollbacks >/dev/null
    require_positive_summary updates_per_second_milli >/dev/null
    require_positive_summary versions >/dev/null
    require_positive_summary audit_records >/dev/null
    CONFIG_P50="$(require_positive_summary p50_update_us)"
    CONFIG_P95="$(require_positive_summary p95_update_us)"
    CONFIG_P99="$(require_positive_summary p99_update_us)"
    CONFIG_ROLLBACK_P99="$(require_positive_summary p99_rollback_us)"
    CONFIG_ETCD_ELAPSED="$(require_positive_summary etcd_elapsed_ms)"
    CONFIG_ETCD_ATTEMPTS="$(require_positive_summary etcd_attempts)"
    CONFIG_ETCD_WRITES="$(require_positive_summary etcd_writes)"
    CONFIG_ETCD_READS="$(require_positive_summary etcd_reads)"
    CONFIG_ETCD_WATCH_UPDATES="$(require_positive_summary etcd_watch_updates)"
    CONFIG_ETCD_DISCONNECTS="$(require_positive_summary etcd_disconnect_observations)"
    CONFIG_ETCD_RECOVERIES="$(require_positive_summary etcd_recoveries)"
    CONFIG_ETCD_FAULTS="$(require_positive_summary etcd_fault_injections)"
    CONFIG_ETCD_OPERATION_P99="$(require_positive_summary etcd_p99_operation_us)"
    CONFIG_ETCD_RECOVERY_P99="$(require_positive_summary etcd_p99_recovery_us)"
    require_positive_summary etcd_operations_per_second_milli >/dev/null
    require_ordered_percentiles "$CONFIG_P50" "$CONFIG_P95" "$CONFIG_P99"
    (( CONFIG_P99 <= 5000000 && CONFIG_ROLLBACK_P99 <= 5000000 )) || {
      echo "Config Center update or rollback p99 exceeds the 5s evidence objective" >&2
      exit 1
    }
    (( CONFIG_ELAPSED >= REQUIRED_SECONDS * 1000 )) || exit 1
    (( CONFIG_ETCD_ELAPSED >= REQUIRED_SECONDS * 1000 )) || exit 1
    (( CONFIG_ETCD_WRITES <= CONFIG_ETCD_ATTEMPTS &&
       CONFIG_ETCD_READS <= CONFIG_ETCD_WRITES &&
       CONFIG_ETCD_WATCH_UPDATES > 0 &&
       CONFIG_ETCD_DISCONNECTS >= CONFIG_ETCD_FAULTS &&
       CONFIG_ETCD_RECOVERIES > 0 &&
       CONFIG_ETCD_OPERATION_P99 <= 5000000 &&
       CONFIG_ETCD_RECOVERY_P99 <= 30000000 )) || {
      echo "Config Center Etcd disconnect/recovery evidence is outside policy" >&2
      exit 1
    }
    ;;
  lifecycle)
    [[ "$BOUNDARY_SUMMARY" == roze_lifecycle_soak\ * ]] || exit 1
    LIFECYCLE_ELAPSED="$(require_positive_summary elapsed_ms)"
    LIFECYCLE_CYCLES="$(require_positive_summary cycles)"
    LIFECYCLE_P50="$(require_positive_summary p50_cycle_us)"
    LIFECYCLE_P95="$(require_positive_summary p95_cycle_us)"
    LIFECYCLE_P99="$(require_positive_summary p99_cycle_us)"
    LIFECYCLE_WORKER_EXITS="$(require_positive_summary worker_exits)"
    LIFECYCLE_STOP_HOOKS="$(require_positive_summary stop_hooks)"
    LIFECYCLE_RUNNING="$(require_positive_summary running_snapshots)"
    LIFECYCLE_STOPPED="$(require_positive_summary stopped_snapshots)"
    LIFECYCLE_SERVICE_COUNT="$(require_positive_summary max_service_count)"
    require_positive_summary cycles_per_second_milli >/dev/null
    require_positive_summary failed_task_detections >/dev/null
    require_positive_summary drain_timeout_detections >/dev/null
    LIFECYCLE_FAULT_P99="$(require_positive_summary p99_fault_detection_us)"
    require_ordered_percentiles "$LIFECYCLE_P50" "$LIFECYCLE_P95" "$LIFECYCLE_P99"
    (( LIFECYCLE_P99 <= 1000000 && LIFECYCLE_FAULT_P99 <= 2000000 )) || {
      echo "lifecycle latency exceeds its evidence objective" >&2
      exit 1
    }
    (( LIFECYCLE_ELAPSED >= REQUIRED_SECONDS * 1000 )) || exit 1
    (( LIFECYCLE_RUNNING == LIFECYCLE_CYCLES &&
       LIFECYCLE_STOPPED == LIFECYCLE_CYCLES )) || exit 1
    EXPECTED_HOOKS=$((LIFECYCLE_CYCLES * LIFECYCLE_SERVICE_COUNT))
    (( LIFECYCLE_WORKER_EXITS == EXPECTED_HOOKS &&
       LIFECYCLE_STOP_HOOKS == EXPECTED_HOOKS )) || exit 1
    ;;
  generated-services)
    [[ "$BOUNDARY_SUMMARY" == roze_generated_systems_soak\ * ]] || exit 1
    GENERATED_ELAPSED="$(require_positive_summary elapsed_seconds)"
    require_positive_summary iterations >/dev/null
    require_positive_summary iterations_per_hour >/dev/null
    GENERATED_P50="$(require_positive_summary p50_iteration_ms)"
    GENERATED_P95="$(require_positive_summary p95_iteration_ms)"
    GENERATED_P99="$(require_positive_summary p99_iteration_ms)"
    require_ordered_percentiles "$GENERATED_P50" "$GENERATED_P95" "$GENERATED_P99"
    (( GENERATED_ELAPSED >= REQUIRED_SECONDS )) || exit 1
    ;;
  *)
    echo "unsupported evidence area: $EXPECTED_AREA" >&2
    exit 1
    ;;
esac

echo "verified production evidence report: $REPORT"
