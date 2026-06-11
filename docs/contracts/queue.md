# Roze 队列收口（Kafka，阶段1+2）

## 兼容前提
- `Publisher`/`Subscriber` 接口不变。
- `KafkaConfig` 保留兼容字段，优先级：新字段优先。

## rdkafka Producer 映射
- `bootstrap` / `bootstrap_servers` / `brokers`
  - 统一入口：`normalized_brokers()`
- `acks`
- `linger.ms`
- `batch.size`
- `message.send.max.retries`（重试次数）
- `retry.backoff.ms`
- `flush_timeout_ms`

## rdkafka Consumer 映射
- `group`
  - alias: `group_id`
- `auto.offset.reset`
- `session.timeout.ms`
- `heartbeat.interval.ms`
- `max.poll.interval.ms`
- `enable.auto.commit`：当 `enable_manual_ack=true` 时强制 `false`
- `manual ack` 优先级：`enable_manual_ack=true` 时忽略 `enable_auto_commit` 的常规提交行为。

## 消息失败策略
- `enable_manual_ack=true`：业务处理成功后提交（`delivery.ack().await`），失败走 `nack`
- `should_fail=true` 或 `type=nack`（示例负载）会触发失败分支
- `max_retries` 达上限后进入死信
- 失败 topic 配置
  - `retry_topic`
  - `dead_letter_topic`
  - `retry_backoff_ms`
- 在 `roze-kafka` 层会记录以下事件（`tracing`）
  - `kafka.message.retry_topic_missing`
  - `kafka.message.dead_letter_missing`
  - `kafka.message.dead_lettered`
  - `kafka.message.dead_lettered`（`max_retries=0`）
  - `kafka.message.recover_dropped`
  - `kafka.message.requeue_retry`

## 手工提交验证（应用层）
- 成功路径
  - 消费消息 -> 业务成功 -> `ack`
- 失败路径
  - 消费消息 -> `nack`
  - 触发重试或 dead letter 逻辑

## 变更重建（热更新）
- `apps/user` 对 `kafka` 配置签名进行比对，变更时执行
  1. 停掉旧 consumer handles
  2. `producer.close()`
  3. 重建 producer + subscriber + worker

## 错误码与观测
- 关键事件（`tracing`）
  - `kafka.message.received`
  - `kafka.message.acked`
  - `kafka.message.nack`
  - `kafka.message.nack_failed`
  - `kafka.message.nack_recovered`
  - `kafka.message.ack_failed`
  - `kafka.pipeline.disabled`
  - `kafka.pipeline.restarting`
  - `kafka.pipeline.started`
  - `kafka.pipeline.startup_degraded`
  - `kafka.pipeline.restart_failed`
  - `kafka.pipeline.restarted`
  - `kafka.pipeline.create_failed`
  - `kafka.consumer.spawn_failed`
  - `kafka.message.retry_topic_missing`
  - `kafka.message.dead_letter_missing`
  - `kafka.message.requeue_retry`
  - `kafka.message.dead_lettered`
  - `kafka.message.recover_dropped`
  - `kafka.startup_publish_ok`
  - `kafka.startup_publish_failed`
  - `kafka.runtime.stopped`

### apps/user 观测字段约定（新增）
- `kafka.pipeline.restarting`
  - `version`/`bootstrap`/`topic`/`previous_signature`/`signature`
- `kafka.pipeline.startup_degraded`
  - `version`/`bootstrap`/`topic`/`group`/`signature`/`workers`/`workers_started`/`spawn_failed`/`reason`/`elapsed_ms`
- `kafka.pipeline.restart_failed`
  - `version`/`bootstrap`/`topic`/`group`/`previous_signature`/`signature`/`workers`/`reason`/`elapsed_ms`
- `kafka.message.nack_recovered`
  - `app`/`worker`/`topic`/`attempt`/`group`/`signature`
- `kafka.message.ack_failed`
  - `app`/`worker`/`topic`/`attempt`/`group`/`signature`/`error`

## 配置示例（`apps/user/config.yaml` 注释段）
- `bootstrap` / `bootstrap_servers`
- `group` / `group_id`
- `acks`
- `enable_manual_ack`
- `auto_offset_reset`
- `session_timeout_ms`
- `heartbeat_interval_ms`
- `max_poll_interval_ms`
- `retry_topic`
- `dead_letter_topic`
