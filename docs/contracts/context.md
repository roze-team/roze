# Roze Context 契约（v1）

`roze_context::Context` 是 REST、RPC、Gateway、MQ、NATS、outbox、日志、
链路追踪和治理规则共享的请求级元数据模型。它不是全局资源容器，也不是业务
用户对象容器。

## 职责边界

| 数据 | 载体 | 生命周期 |
| --- | --- | --- |
| DB、Redis、配置、RPC client 等全局资源 | `ServiceContext` | 应用进程 |
| request ID、trace ID、deadline、认证主体、透传 metadata | `roze_context::Context` | 单次逻辑请求 |
| 登录用户详情、权限快照等业务对象 | HTTP `Extension<T>` 或显式 logic 参数 | 单次协议请求 |
| 日志与 span | `tracing::Span` | 当前异步任务 |
| timeout、rate limit、breaker、shedding | middleware / governance | 框架边界 |

业务逻辑直接使用 `tracing` 宏。入口 middleware 已建立请求 span 时，日志自动
关联 trace；不要为了日志在业务函数之间重复传递字符串形式的 trace ID。

## 标准字段

- `request_id`：入口请求标识，用于日志检索和错误定位。
- `trace_id`：跨服务链路标识。
- `deadline`：绝对截止时刻；跨边界传播剩余毫秒数。
- `auth.subject`：认证主体。
- `auth.roles`：主体角色。
- `auth.tenant`：可选租户。
- `metadata`：受控的字符串键值，包括 locale、permissions、scope、
  idempotency key 等。
- `retry_budget`：整个逻辑请求尚可使用的重试额度，上限为 64。
- `cancellation`：共享的取消状态和 `Canceled` / `DeadlineExceeded` 原因。

`Context::clone()` 和由 `with_*` 产生的 fork 共享 cancellation 与 retry budget；
身份、metadata 和 deadline 以 fork 时的值复制。父 Context 在 fork 后取消时，
子 Context 必须立即可见。

## 标准 Header / RPC Metadata

| 名称 | 语义 |
| --- | --- |
| `x-request-id` | request ID |
| `x-trace-id` | Roze trace ID |
| `x-roze-timeout-ms` | 剩余 deadline，毫秒 |
| `x-roze-retry-budget-remaining` | 当前被委派的剩余重试额度 |
| `Idempotency-Key` | 幂等键；解析时大小写不敏感 |
| `x-roze-subject` | 认证主体 |
| `x-roze-tenant` | 租户 |
| `x-roze-roles` | 角色列表 |
| `x-roze-locale` / `Accept-Language` | locale |
| `x-roze-meta-*` | 受控业务 metadata |

标准名称优先于兼容别名。现有 `x-hula-*` 别名只用于迁移兼容，生成的新服务应
发出标准名称。非法数字、非法 metadata 和无法解析的可选字段不得导致 panic；
不可信的 retry budget 必须截断到 64。

## Retry budget 所有权

Retry budget 约束的是完整逻辑请求的总重试放大，不是每一跳各自获得一份额度。

- 没有入站预算时，第一个受治理 RPC client 使用有效 `max_attempts - 1`
  初始化预算。
- 初次调用不消费额度；只在 deadline、cancellation、backoff 和进程级 retry
  policy 均允许后、即将执行真实 retry 前消费一个 credit。
- 同进程 clone/fork 使用同一个原子计数器，并发消费不得下溢。
- 生成客户端向下游调用时原子划拨最多一半当前额度到隔离的 child Context，
  不能复制完整数值。
- 下游 response/status 只可返还尚未使用的额度；上游最多恢复本次实际划拨量。
- transport failure、取消或缺失 metadata 时不返还已划拨额度；伪造的超额返还
  会被截断。

因此，本地剩余额度、所有在途 child 额度和已经消费的额度之和不会超过请求的
初始预算。真实多进程 fan-out 与故障恢复仍必须由参考系统集成测试验证。

## Deadline 与 cancellation

- 使用 `with_timeout` / `with_deadline` 设置截止时刻。
- 出站边界传播 `remaining_timeout()`，不得重新开始一个完整 timeout。
- deadline 到期后使用 `with_expiration_reason()` 或治理入口将状态标记为
  `DeadlineExceeded`。
- `cancel()` / `cancel_with_reason()` 对所有 clone/fork 可见。
- async task、attempt lease、shedding permit、stream permit 和数据库连接应依靠
  RAII，在成功、错误、timeout、cancel 和 panic 出口全部释放。

## 传播矩阵

| 边界 | 写出 | 恢复 |
| --- | --- | --- |
| HTTP | middleware 将 Context 写入 request extension 和标准 headers | HTTP 入口从 headers 构造 Context |
| RPC | `client_request` 写入 tonic metadata | `request_context` 恢复 Context |
| MQ / NATS | envelope headers / trace context | message `context()` |
| outbox | `OutboxMessage::with_context` 持久化传播字段 | relay 发布到 MQ 后由消费者恢复 |

新增边界必须复用 `propagation_headers()` / `from_propagation_headers()` 或等价的
类型安全适配器，并增加 request、trace、deadline、subject、tenant、locale、
idempotency key 和 retry budget 的 round-trip 测试。

## 安全与可观测约束

- 不把 token、cookie、authorization 或任意用户输入自动放入 metadata。
- tenant、subject、request ID、trace ID、endpoint、原始 path 和错误正文不得
  成为 Prometheus label。
- 高基数字段只进入受采样 trace 或结构化日志，并遵守脱敏策略。
- `ContextKey<T>` 只用于进程内类型安全扩展；其值不自动跨网络传播。
- `Context` 不应长期存入全局缓存、后台单例或跨请求复用。

## 最小示例

```rust
use roze_context::{AuthContext, Context};
use std::time::Duration;

let context = Context::background()
    .with_timeout(Duration::from_secs(2))
    .with_auth(AuthContext {
        subject: "user-42".to_string(),
        roles: vec!["reader".to_string()],
        tenant: Some("tenant-a".to_string()),
    })
    .with_locale("zh-CN")
    .with_idempotency_key("order-20260718")
    .with_retry_budget(3);

let headers = context.propagation_headers();
let restored = Context::from_propagation_headers(&headers);
assert_eq!(restored.retry_budget_remaining(), Some(3));
```
