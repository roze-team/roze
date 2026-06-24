# Roze DTM 基础服务契约

Roze DTM 是内置分布式事务管理基础服务，设计参考 DTM 的核心思想：由独立协调器保存全局事务状态，并驱动各业务服务的分支动作。

参考来源：[dtm-labs/dtm helper README-cn](https://github.com/dtm-labs/dtm/blob/main/helper/README-cn.md)。DTM README 描述了多语言分布式事务管理器，支持 Saga、TCC、XA、二阶段消息、多存储和高可用；Roze 当前先内置默认 TCC + Saga 的 Rust 原生基础实现。

## 默认模式

默认事务模式是 **TCC**。

- Try: 预留资源，例如冻结余额、锁库存、占用额度。
- Confirm: 提交资源变更。
- Cancel: 释放 Try 阶段预留资源。

Saga 作为可选模式保留，适合长流程和最终一致场景。

## 服务入口

默认监听：

```yaml
rest:
  addr: 127.0.0.1:8090
  register: false
governance: {}
dtm:
  recover_interval_ms: 1000
  recovery_lease_ttl_ms: 5000
  worker_id: roze-dtm-local
  store:
    kind: memory
    # kind: sqlite
    # database_url: sqlite://roze-dtm.db?mode=rwc
  max_attempts: 5
  retry_backoff_ms: 1000
  max_retry_backoff_ms: 30000
  branch_call_timeout_ms: 5000
  transaction_timeout_ms: 60000
```

API：

- `POST /v1/tcc`：提交默认 TCC 全局事务。
- `POST /v1/tcc/{gid}/prepare`：执行所有 Try 分支。
- `POST /v1/tcc/{gid}/confirm`：执行所有 Confirm 分支。
- `POST /v1/tcc/{gid}/cancel`：执行所有 Cancel 分支。
- `POST /v1/saga`：提交 Saga 全局事务。
- `POST /v1/saga/{gid}/start`：执行 Saga 正向分支。
- `POST /v1/saga/{gid}/abort`：反向执行 Saga 补偿分支。
- `GET /v1/transactions`：列出事务，支持 `gid`、`kind`、`status`、`offset`、`limit` 过滤分页。
- `GET /v1/transactions/{gid}`：查询单个事务。
- `POST /v1/transactions/{gid}/recover`：人工推进单个未终态事务。
- `POST /v1/recover`：人工触发一次全局恢复 tick。
- `GET /v1/stats`：查询事务状态统计。
- `GET /healthz`、`GET /readyz`：健康检查。

## TCC 提交格式

```json
{
  "gid": "order-1001",
  "branches": [
    {
      "id": "inventory",
      "kind": "TccTry",
      "action": "http://inventory/try",
      "confirm": "http://inventory/confirm",
      "cancel": "http://inventory/cancel",
      "payload": { "sku": "A", "count": 1 }
    }
  ]
}
```

`POST /v1/tcc` 不传 `kind` 时即为 TCC。

## Saga 提交格式

```json
{
  "gid": "transfer-1001",
  "branches": [
    {
      "id": "out",
      "kind": "SagaAction",
      "action": "http://account/trans-out",
      "compensate": "http://account/trans-out-compensate",
      "payload": { "amount": 30 }
    }
  ]
}
```

## 分支屏障

Roze DTM 内置分支屏障：

- 同一个 `gid + branch_id + op` 只能执行一次。
- `cancel` 早于 `try` 到达时识别为空回滚并跳过。
- 重复 `confirm/cancel/compensate` 会被跳过，保证幂等边界。

## 当前实现范围

已实现：

- 默认 TCC。
- Saga/TCC 全局事务状态机。
- HTTP 分支调用器。
- 内存存储。
- SQLite 持久化存储。
- 分支屏障。
- 分支失败指数退避重试计划。
- 分支调用超时。
- 全局事务超时后自动 Cancel/Compensate。
- 后台恢复 worker。
- 恢复 worker 租约，避免多实例重复调度。
- 控制面 API：查询、过滤分页、统计、人工恢复。
- 独立基础服务 `apps/roze-dtm`。

后续扩展：

- Redis/etcd 持久化或租约后端。
- XA。
- 二阶段消息。
- Dashboard。

## 可靠性边界

当前服务具备可靠事务协调的核心状态机。默认内存存储只适合开发和单实例测试；生产部署应切换到 SQLite 或后续 SQL/Redis 后端，并为多副本使用恢复租约。

已保证：

- 分支屏障防重复执行。
- Cancel 早于 Try 到达时识别为空回滚。
- 失败分支记录 `next_retry_millis`。
- 失败分支按指数退避重试，并受 `max_retry_backoff_ms` 限制。
- HTTP 分支调用受 `branch_call_timeout_ms` 限制。
- 后台 worker 周期恢复未终态事务。
- 后台 worker 获取租约后才执行恢复 tick。
- 全局超时触发 TCC Cancel 或 Saga Compensate。
- SQLite store 提供事务、barrier、recovery lease 持久表。
- 控制面支持按 gid 查询、按状态/类型过滤、分页、统计和人工恢复。

生产增强方向：

- PostgreSQL/MySQL/Redis 后端。
- 基于 etcd/Redis 的跨进程租约后端。
- 分支调用熔断、限流和观测指标。
- 更完整的人工补偿审批流。
- 管理 Dashboard。
