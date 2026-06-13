#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
LOG_FILE="${LOG_FILE:-${SCRIPT_DIR}/user.log}"
ETCD_ENDPOINTS="${ROZE_CONFIG_CENTER_ETCD_ENDPOINTS:-127.0.0.1:2379}"
ETCD_ENDPOINT="${ETCD_ENDPOINTS%%,*}"
ETCD_KEY="${ROZE_CONFIG_CENTER_KEY:-${ROZE_CONFIG_CENTER_ETCD_KEY:-roze/user/config}}"
KAFKA_BROKERS="${ROZE_KAFKA_BROKERS:-127.0.0.1:9092}"
KAFKA_CONFIG_BROKER="${KAFKA_BROKERS%%,*}"
KAFKA_TOPIC="${ROZE_KAFKA_TOPIC:-user.events}"

info() {
  echo "[INFO] $*"
}

put_etcd_json() {
  local key="$1"
  local payload="$2"
  local value_b64
  value_b64="$(printf '%s' "$payload" | base64 | tr -d '\n')"
  curl -sS -X POST "http://${ETCD_ENDPOINT}/v3/kv/put" \
    -H 'Content-Type: application/json' \
    -d "{\"key\":\"$(printf '%s' "$key" | base64 | tr -d '\n')\",\"value\":\"${value_b64}\"}" >/dev/null
}

assert_log_contains() {
  local pattern="$1"
  local message="$2"
  if rg -q "$pattern" "$LOG_FILE"; then
    info "✓ ${message}"
  else
    info "✗ ${message}"
    echo "   pattern: $pattern"
    echo "   log: $LOG_FILE"
    return 1
  fi
}

assert_log_optional() {
  local pattern="$1"
  local message="$2"
  if rg -q "$pattern" "$LOG_FILE"; then
    info "✓ ${message}"
  else
    info "○ ${message} (optional)"
  fi
}

require_kcat() {
  if ! command -v kcat >/dev/null 2>&1; then
    info "skip kcat step: kcat not installed"
    return 1
  fi
}

produce_kafka_json() {
  local value="$1"
  printf '%s\n' "$value" | kcat -P -b "$KAFKA_BROKERS" -t "$KAFKA_TOPIC" >/dev/null
}

base_valid_config() {
  printf '{"name":"user","rest":{"addr":"127.0.0.1:3000","register":false},"registry":{"kind":"memory","endpoints":[],"ttl_seconds":10,"renew_interval_secs":3},"governance":{"timeout_ms":5000,"rate_limit":{"burst":100,"refill_ms":10},"breaker":{"failure_threshold":5,"reset_timeout_ms":30000},"routes":{}},"kafka":{"brokers":["%s"],"bootstrap":"%s","topic_prefix":"user","topic_regex":"^user\\\\.","consumer_workers":1,"group":"user-service-a","enable_manual_ack":true,"enable_auto_commit":false,"max_retries":1,"retry_topic":"retry","dead_letter_topic":"dlq","retry_backoff_ms":300,"session_timeout_ms":10000,"heartbeat_interval_ms":3000,"max_poll_interval_ms":300000}}\n' "$KAFKA_CONFIG_BROKER" "$KAFKA_BROKERS"
}

base_valid_config_b() {
  printf '{"name":"user","rest":{"addr":"127.0.0.1:3000","register":false},"registry":{"kind":"memory","endpoints":[],"ttl_seconds":10,"renew_interval_secs":3},"governance":{"timeout_ms":5000,"rate_limit":{"burst":100,"refill_ms":10},"breaker":{"failure_threshold":5,"reset_timeout_ms":30000},"routes":{}},"kafka":{"brokers":["%s"],"bootstrap":"%s","topic_prefix":"user","topic_regex":"^user\\\\.","consumer_workers":1,"group":"user-service-b","enable_manual_ack":false,"enable_auto_commit":true,"max_retries":2,"retry_topic":"retry","dead_letter_topic":"dlq","retry_backoff_ms":300,"session_timeout_ms":10000,"heartbeat_interval_ms":3000,"max_poll_interval_ms":300000}}\n' "$KAFKA_CONFIG_BROKER" "$KAFKA_BROKERS"
}

scenario_invalid_config_rollover() {
  info "Scenario A: invalid etcd config should not restart app"
  put_etcd_json "${ETCD_KEY}" "not-a-valid-json-config"
  sleep 2
  assert_log_contains 'config.reload.failed' '配置下发失败事件'
  assert_log_contains 'kafka.signature.unchanged|config.reload.applied|kafka.pipeline.started' '服务仍维持已有运行链路'
}

scenario_toggle_manual_ack() {
  info "Scenario B: 切换 manual ack + max_retries 并触发重建"
  put_etcd_json "${ETCD_KEY}" "$(base_valid_config)"
  sleep 3
  assert_log_contains 'kafka.pipeline.restarting' '开始重建'
  assert_log_contains 'kafka.pipeline.restarted' '重建完成'
  assert_log_optional 'kafka.pipeline.startup_degraded' '可选：观察到部分 worker 重建失败的降级告警'
  put_etcd_json "${ETCD_KEY}" "$(base_valid_config_b)"
  sleep 3
  assert_log_contains 'kafka.pipeline.restart_failed|kafka.pipeline.restarted' '重建行为可观测'
}

scenario_nack_retry_path() {
  if ! require_kcat; then
    info "Scenario C skipped: kcat missing"
    return
  fi

  info "Scenario C: 发送失败消息，期望 nack/retry/DLQ 可见"
  produce_kafka_json '{"should_fail":true}'
  sleep 2
  assert_log_contains 'kafka.message.nack' '触发 nack'
  assert_log_contains 'kafka.message.nack_recovered' '手工提交 nack 分支恢复完成'
  assert_log_optional 'kafka.message.requeue_retry|kafka.message.dead_lettered' '可选：失败消息重试或死信分支事件'
}

scenario_success_path() {
  if ! require_kcat; then
    info "Scenario D skipped: kcat missing"
    return
  fi
  
  info "Scenario D: 发送正常消息，观察 ack 回写"
  produce_kafka_json '{"id":"success-path","value":"ok"}'
  sleep 2
  assert_log_contains 'kafka.message.acked' '成功处理并提交/确认'
}

usage() {
  cat <<'USAGE'
Usage:
  ROZE_CONFIG_CENTER_KEY=roze/user/config \
  ROZE_CONFIG_CENTER_ETCD_ENDPOINTS=127.0.0.1:2379 \
  ./ops/reload-e2e.sh all

Commands:
  invalid         invalid config + 回滚验收
  reload          切换 manual ack/max_retries 重建验收
  nack           发送 should_fail 消息并检查失败分支
  success        发送成功消息并检查 ack 分支
  all            执行全部流程
USAGE
}

cmd="${1:-}"
case "${cmd}" in
  invalid) scenario_invalid_config_rollover ;;
  reload) scenario_toggle_manual_ack ;;
  nack) scenario_nack_retry_path ;;
  success) scenario_success_path ;;
  all)
    scenario_invalid_config_rollover
    scenario_toggle_manual_ack
    scenario_nack_retry_path
    scenario_success_path
    ;;
  *) usage; exit 1 ;;
esac
