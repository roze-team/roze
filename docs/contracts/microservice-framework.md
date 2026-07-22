# Roze 微服务框架能力矩阵

本矩阵记录 Roze 当前代码能力及其证据等级。`已实现` 只表示代码与确定性测试
存在；`已集成` 要求真实依赖测试；`证据待补` 表示仍缺固定 runner、故障或长稳
产物。不得把计划项或短时本地 smoke 写成生产通过。

## 核心边界

| 能力 | 状态 | 入口与证据 |
| --- | --- | --- |
| HTTP 统一边界 | 已实现 | `roze-http`、`roze-middleware`、`roze-result`、`roze-error` |
| RPC 统一边界 | 已实现 | `roze-rpc`、`roze-grpc`、`roze-context` |
| Context 传播 | 已实现 | request/trace、deadline、共享 cancellation、auth、tenant、locale、idempotency、retry budget |
| 参数校验 | 已实现 | REST/RPC 生成入口使用 `roze-validation` |
| 错误与 i18n | 已实现 | `RozeError`、RPC error metadata、locale |
| 服务治理 | 已实现 | timeout、rate limit、breaker、shedding、retry budget、低基数指标 |
| RPC 数据面 | 已实现 | 每 attempt P2C、EWMA latency/success、in-flight、结果反馈、watch/cache |
| 配置中心 | 已实现；真实长稳待补 | Etcd watch、revision 恢复、last-known-good reload |
| 服务发现 | 已集成；故障证据待补 | Memory、DNS、Etcd、Consul、cached resolver |
| 数据一致性 | 已实现；真实故障待补 | transaction、outbox/inbox、idempotency、Saga/TCC |
| ORM 生成 | 已实现 | Toasty 默认；`--orm sea-orm` 可选；tenant/audit/soft-delete |
| MQ | 已实现；真实 broker 长稳待补 | in-memory、NATS、Kafka、retry、DLQ、outbox relay |
| Gateway | 已实现；长连接长稳待补 | HTTP/WebSocket/SSE、registry、health、outlier、热更新 |
| Search | 已实现；真实恢复待补 | Elasticsearch、OpenSearch、Meilisearch |
| 生产生成资产 | 已实现；部署证据待补 | Docker、Kubernetes、Helm、SLO、告警、runbook、证据脚本 |

生产成熟度与功能存在是两个维度。权威缺口和退出条件见
[go-zero 超越计划](../go-zero-surpass-plan.md)。

## RPC 错误 metadata

`roze_rpc::rpc::status_from_error` 至少发出：

- `x-roze-error-code`
- `x-roze-error-kind`
- `x-request-id`
- `x-trace-id`
- `x-roze-locale`（存在 locale 时）
- `x-roze-retry-budget-remaining`（存在请求预算时）

客户端使用统一解码器恢复 `RozeError` 语义。错误正文、tenant、endpoint 和实例
地址不得进入 metrics label。

## Retry budget 与下游选址

生成 RPC client 在每个真实 attempt 前重新发现并选择实例。picker 使用实时
EWMA、in-flight 与成功率；attempt 完成、timeout、cancel、panic 和 connect
failure 都必须结算或由 RAII Drop 释放。

请求级 retry budget 不是每跳复制的 max retry。生成客户端从共享原子池划拨
child 额度，下游仅返还未使用且不超过划拨值的 credit；缺失响应保守消耗。
详细规则见 [RPC Client Config](rpc-client-config.md) 与
[Context 契约](context.md)。

## 配置中心 Etcd watch

- 初次读取使用 Etcd v3 range API。
- 热更新使用 watch API。
- 保存 event `mod_revision` 或 response header revision。
- 重连从 `last_revision + 1` 继续。
- 解析、校验或 listener 失败时保留最后有效配置并记录 reload failure。
- listener 必须有 timeout；无关 section 变化不得重建整个 runtime。

## 服务发现 Etcd watch

- 注册通过 lease grant + KV put 写入
  `{prefix}/{service}/{addr}`。
- 后台 keepalive 续租；shutdown 停止续租并删除实例 key。
- `discover(service)` 读取 prefix 快照。
- `watch(service)` 处理 put/delete 并发布完整实例快照。
- `CachedRegistryResolver` 优先使用 watch，同时保留 TTL refresh 与有界 stale
  fallback。
- 实例 remove/re-add 后，picker 状态只能在 grace period 内保留，不能永久增长。

默认 prefix 是 `/roze/services`。可通过 `registry.prefix` 隔离环境或应用。
RPC server 默认注册 `rpc.addr`；绑定 wildcard/loopback 但客户端需要可路由地址时，
必须设置 `rpc.advertise_addr`。

Etcd registry 的 `registry.user` / `registry.pass` 启用用户认证；
`registry.ca_cert_file` 配置私有 CA，`registry.cert_file` 与
`registry.cert_key_file` 成对启用 mTLS。注册、发现、watch、续租、摘除与重注册
共用同一 TLS 客户端和认证 token；服务端返回 401/403 时 token 会被清除、重新获取
并将原请求重试一次。`insecure_skip_verify` 只用于受控诊断，启用时会记录警告。

生成服务将数据库、MongoDB、Redis、NATS、registry 和托管 RPC client 注册为
带统一超时的动态 readiness check。`/healthz` 只反映进程 liveness，`/readyz`
并发执行依赖检查，`/startupz` 只反映 startup/draining 阶段；运行期间依赖失联
会立即使 readiness 失败，但不会把进程 liveness 误报为失败。

## 连接模式与代理诊断

RPC client 必须且只能选择一种连接模式：

- `rpc_client.target`
- `rpc_client.endpoints`
- `rpc_client.etcd`

混合配置在连接前拒绝，防止静态 endpoint 无意绕过 registry。

Etcd/Consul 等 HTTP client 遵循 `reqwest` 的 proxy 环境。访问 loopback 或私网
控制面时，应正确配置 `NO_PROXY`，或清除不适用的 `HTTP_PROXY`、
`HTTPS_PROXY`、`ALL_PROXY`。Roze 在检测到私网 endpoint 与 proxy 环境组合时
附加诊断提示，但不会偷偷修改进程环境。

## 集成与证据入口

```bash
# 不启动真实依赖的确定性预检
bash scripts/production-smoke.sh

# 启动仓库集成依赖后运行
bash scripts/production-smoke.sh --with-compose

# 从权威输入生成三类参考系统
bash scripts/generated-reference-systems.sh

# 真实依赖参考系统流程
bash scripts/reference-systems-integration.sh
```

真实 adapter 的验收至少包含：

- 成功路径和数据正确性；
- 断线、重启、timeout、cancel、retry 与恢复；
- Context 多跳传播和全链路 retry 上限；
- 资源、task、permit、connection 与状态表回收；
- 低基数 metrics 与 trace 关联；
- 固定 revision、依赖 digest、runner metadata 和原始 artifact。

本机无 Docker、凭据或固定 runner 时必须报告为未执行，不能生成通过结论。
