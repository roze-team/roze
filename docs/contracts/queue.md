# Roze 队列收口（Kafka，阶段1+2）

## Kafka Provider 与能力矩阵

`roze-kafka` 通过 `KafkaConfig.provider` 选择 `memory`、`rdkafka` 或
`rust-native`，并由 `build_runtime` 返回统一的
`Arc<dyn roze_mq::Publisher>` / `Arc<dyn roze_mq::Subscriber>`。

| Provider | Feature | 发布 | Consumer Group | 手工 ACK / Offset Commit | Rebalance | 事务 | 定位 |
| --- | --- | --- | --- | --- | --- | --- | --- |
| memory | `memory` | 是 | 否 | 仅进程内 | 否 | 否 | 本地开发、单测 |
| rdkafka | `rdkafka` | 是 | 是 | 是 | 是 | Roze 暂未暴露 | 生产推荐 |
| rdkafka | `rdkafka-cmake` | 是 | 是 | 是 | 是 | Roze 暂未暴露 | 使用 CMake 构建 librdkafka |
| rust-native | `rskafka` | 是 | 否 | 否 | 否 | 否 | Experimental、仅发布 |

`rskafka` 上游不支持 Consumer Group、Offset Tracking、Rebalance 或事务。
Roze 不模拟这些语义：`build_runtime` 选择 `rust-native` 时会在连接 Broker
之前 fail-fast；仅发布服务可使用 `build_publisher`。`rskafka` 关闭默认压缩
Feature，避免间接引入 LZ4/Zstandard 原生库。

仓库内的 `user-service` 与 `roze-example` 直接选择 `rdkafka-cmake`。原生
Windows 构建机需安装 CMake 与 Visual Studio C++ toolchain；配置完成后可直接
运行 `cargo test --workspace`，无需额外追加 feature。Linux/WSL 发布门禁仍单独
验证普通 `rdkafka` feature。

## 原生配置前提
- `Publisher`/`Subscriber` 接口保持稳定。
- `KafkaConfig` 使用 Roze 原生字段；配置解析统一走标准字段归一化。

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
- retry/dead-letter topic 在 publish 前只做一次 `topic_prefix` 归一化，避免 `app.retry` 被二次前缀成 `app.app.retry`。

## 标准消息 metadata

`roze_mq::Message` 和 `roze_kafka::KafkaRecord` 使用同一组可观测字段：

- `timestamp_millis`：框架创建或接收消息的时间。
- `attempt`：当前投递尝试次数。
- `dead_letter_topic`：消息级死信 topic 覆盖。
- `idempotency_key`：消费侧/本地 broker 去重键。
- `partition`：broker 分区；不适用的 broker 可以为空。
- `offset`：broker offset；不适用的 broker 可以为空。
- `group`：consumer group / durable 名称；不适用的 broker 可以为空。
- `headers`：Context carrier 和业务 header，必须保留 `x-request-id`、`x-trace-id` 等传播字段。

## Producer 返回值

`roze_kafka::Publisher` 提供两种发布方法：

- `publish(record) -> Result<()>`：只表达发布成功/失败。
- `publish_with_result(record) -> Result<PublishResult>`：返回 broker 元数据。

`PublishResult` 字段：

- `topic`：实际发布 topic；rdkafka 会返回加过 `topic_prefix` 的 topic。
- `partition`：broker partition，memory broker 固定为 `Some(0)`。
- `offset`：broker offset，memory broker 使用 topic 内单调递增 offset。
- `timestamp_millis`：框架发布时间。

## MQ 治理接口

`roze-mq` 提供统一治理 trait：`MqAdmin`。

- `stats()`：返回发布、投递、ack、nack、重复、死信、重放、待处理死信数量。
- `dead_letters(offset, limit)`：分页查询死信记录。
- `dead_letters_query(query)`：按 topic/group 分页过滤死信记录。
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

`Subscriber::subscribe(topic)` derives a stable durable name from the
configured base durable and the fully-prefixed filter subject. Therefore one
`NatsJetStream` instance can subscribe to several subjects without reusing an
incompatible consumer. `subscribe_with_options` additionally accepts an
explicit filter subject, durable name, ack policy, and deliver policy. An
explicit shared durable opts into competing-consumer semantics. Whenever a
durable already exists, Roze compares its filter/ack/deliver configuration and
returns a diagnostic conflict containing the durable and expected/actual
filter instead of silently reusing it.

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

## Inbox Pattern

`roze-transaction` 提供基础 inbox 状态模型：

- `InboxMessage`：按 `idempotency_key` 记录消费状态、topic、group、attempts、时间戳和失败原因。
- `InboxStatus`：`Processing`、`Processed`、`Failed`。
- `InMemoryInbox::begin`：
  - 首次消费返回 `Started`；
  - 已成功处理返回 `DuplicateProcessed`，调用方应直接 ack；
  - 正在处理或未到重试时间返回 `AlreadyProcessing`，调用方不应重复执行业务；
  - 失败且到达 `next_attempt_millis` 返回 `RetryStarted`。
- `mark_processed`：业务成功后标记完成。
- `mark_failed`：业务失败后记录错误和下一次可重试时间。
- `pending_retry`：查询已到期的失败消息，便于控制面或 worker 重试。

生产持久化层可按同一 `InboxMessage` 结构落库，保证 at-least-once 投递下业务消费幂等。

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
- Prometheus 指标
  - `roze_queue_events_total{system="kafka",topic,group,outcome}`：记录 Kafka/MQ 关键路径事件。
  - 当前 outcome 包括 `published`、`publish_failed`、`delivered`、`acked`、`nacked`、`retry_scheduled`、`dead_lettered`、`replayed`、`commit_failed`、`retry_topic_missing`、`dead_letter_missing` 和 `recover_dropped`。
  - `roze_queue_last_offset{system="kafka",topic,group,partition}`：记录最后观察到的 offset。offset 作为 gauge value，不进入 label，避免高基数。

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
