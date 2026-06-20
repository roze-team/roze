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

## MQ 治理接口

`roze-mq` 提供统一治理 trait：`MqAdmin`。

- `stats()`：返回发布、投递、ack、nack、重复、死信、重放、待处理死信数量。
- `dead_letters(offset, limit)`：分页查询死信记录。
- `replay_dead_letter(id)`：按死信 id 重放到原 topic，重置 attempt 并保留 trace header。
- `purge_dead_letter(id)`：按死信 id 删除记录。
- `clear_dead_letters()`：清空当前死信记录。

死信记录统一结构：

- `id`：治理侧唯一 id。
- `original_topic`：原始 topic。
- `reason`：进入死信原因，例如 `nack_max_attempts_exceeded`。
- `failed_at_millis`：进入死信时间。
- `replay_count`：人工重放次数。
- `message`：原始消息快照。

当前实现：

- `roze-mq::InMemoryBroker` 已实现 `MqAdmin`。
- `roze-kafka::InMemoryKafkaBroker` 已实现 `roze_mq::MqAdmin`，便于本地测试和控制面复用。
- `roze_mq::Message::with_context` 和 `roze_nats::NatsMessage::with_context` 使用统一 Context carrier，携带 request id、trace id、auth、tenant、locale、timeout 和 metadata。
- `roze-nats::NatsMessage` 默认带 `x-trace-id`，与 HTTP/RPC/Kafka/MQ 链路一致。

## NATS JetStream

`roze-nats` 提供真实 JetStream adapter：`NatsJetStream`。

配置结构：

```yaml
nats:
  servers: ["127.0.0.1:4222"]
  client_name: user-service
  subject_prefix: app
  jetstream:
    stream: ROZE
    subjects: ["orders", "orders.retry", "orders.dlq"]
    durable: user-orders
    max_messages: 10000
    max_retries: 3
    retry_subject: orders.retry
    dead_letter_subject: orders.dlq
    consumer_buffer: 256
```

能力：

- 真实 NATS client 连接：`async_nats::connect`。
- JetStream stream 初始化：`get_or_create_stream`。
- 发布：实现 `roze_mq::Publisher`，消息序列化为 JSON 并写入 JetStream。
- 订阅：实现 `roze_mq::Subscriber`，使用 pull durable consumer。
- ack：业务成功时调用 JetStream `ack()`。
- nack：业务失败时发送 `Nak`，并按 `max_retries` 进入 retry subject 或 dead-letter subject。
- durable consumer：通过 `jetstream.durable` 配置。
- 治理：实现 `roze_mq::MqAdmin`，支持 stats、DLQ list/replay/purge/clear。
- trace：所有 `NatsMessage` 默认携带 UUIDv7 `x-trace-id`。

## Outbox Relay

`roze-transaction` 提供基础 outbox relay：

- `OutboxMessage::with_context`：入 outbox 时固化当前 Context propagation headers。
- `InMemoryOutbox`：框架内置内存 outbox，适合本地开发、测试和上层持久化实现的契约参考。
- `relay_outbox_batch`：读取 pending/failed 消息并发布到任意 `roze_mq::Publisher`，包括 `NatsJetStream`。
- 发布成功：标记 `Published`。
- 发布失败：标记 `Failed`，并按指数退避写入 `next_attempt_millis`。

生产持久化层可按同一 `OutboxMessage` 结构落库，relay 侧保持 `roze_mq::Publisher` 抽象，不绑定具体 MQ。

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
