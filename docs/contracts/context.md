# Roze Context 契约（v1）

Roze Context 是 REST、RPC、Gateway、日志、链路追踪和治理规则共享的请求元数据模型。它不是全局资源容器，也不是业务用户对象容器。

新代码按职责选择上下文载体：

| 数据类型 | 实现方式 | 作用域 |
| --- | --- | --- |
| 全局资源，例如 DB、Redis、配置、RPC client | `ServiceContext` | 应用全局，通过 Axum `State` 共享 |
| 请求元数据，例如 request_id、trace_id、deadline、认证主体、透传 metadata | `roze_context::Context` | 单次请求，框架中间件写入 Axum `Extension<Context>`，跨 REST/RPC/Gateway 传播 |
| 业务用户对象，例如登录用户详情、权限快照 | Axum `Extension<T>` | 单次 HTTP 请求，由认证/会话中间件注入，handler 提取后显式传给 logic |
| 链路追踪和日志上下文 | `tracing::Span` | 当前异步任务隐式传播，业务代码直接使用 `tracing` 宏 |
| 超时、限流、熔断、降载等横切逻辑 | middleware / route governance | 框架层处理，logic 不应自己实现通用治理 |

业务 logic 中需要日志时直接使用 `tracing::info!`、`tracing::warn!`、`tracing::error!` 等宏。只要请求入口已经经过 Roze tracing middleware，日志会自动关联当前请求 Span 和 `trace_id`，不要为了打日志在业务函数签名里手动传递 `trace_id`。

## 标准字段

- `request_id`：单次入口请求 ID，用于日志检索和错误响应定位。
- `trace_id`：链路追踪 ID，用于跨服务调用串联。
- `deadline`：请求剩余时间，跨 RPC 调用传递为剩余毫秒数。
- `auth`：
  - `subject`：认证主体 ID。
  - `roles`：主体角色列表。
  - `tenant`：租户 ID，可选。
- `metadata`：业务透传元数据，使用字符串 key/value。

## HTTP Header

- `x-request-id`：对应 `Context::request_id()`。
- `x-trace-id`：对应 `Context::trace_id()`。
- `x-roze-timeout-ms`：剩余 timeout 毫秒数。
- `x-roze-locale`：请求语言，例如 `zh-CN`、`en-US`。
- `Accept-Language`：未提供 `x-roze-locale` 时作为语言来源。
- `x-roze-subject`：认证主体。
- `x-roze-tenant`：租户。
- `x-roze-roles`：逗号分隔角色列表。
- `x-roze-meta-*`：业务 metadata，例如 `x-roze-meta-locale: zh-CN` 会进入 `metadata["locale"]`。

## REST 边界

- `roze-middleware::axum_request_context` 从 HTTP header 构造 `Context` 并写入 Axum request extensions。
- 未传入 `x-request-id` 或 `x-trace-id` 时，入口中间件生成新值并回写响应 header。
- `x-roze-locale` 或 `Accept-Language` 会进入 `Context::locale()`。
- `roze-middleware::axum_auth` 验证 JWT 后把 `subject/roles/tenant` 写入同一个 `Context`。
- `roze-middleware::axum_trace` 为每个请求创建根 tracing Span，Span 字段包含 `trace_id`。
- 需要完整业务用户对象时，自定义认证/会话中间件应把对象写入 Axum `Extension<User>`，handler 通过 `Extension(user): Extension<User>` 提取，再把需要的字段显式传给 logic。

`Context` 保留 `trace_id` 是为了跨协议传播、错误响应和 RPC metadata，不代表业务代码需要手动传递它来记录日志。

## RPC/gRPC 边界

- `roze_rpc::rpc::apply_request_context` 和 `roze_grpc::apply_context` 将 Context 写入 tonic metadata。
- `roze_rpc::rpc::request_context` 和 `roze_grpc::request_context` 从 tonic metadata 恢复完整 Context。
- 生成的 RPC client 应继续以 `&roze_context::Context` 作为第一个业务上下文参数。
- RPC 错误统一由 `roze_rpc::rpc::status_from_error` 生成，metadata 包含：
  - `x-roze-error-code`
  - `x-roze-error-kind`
  - `x-request-id`
  - `x-trace-id`
  - `x-roze-locale`

## Gateway 边界

- Gateway 统一使用 `roze-context` 标准 header。
- JWT 鉴权通过后，Gateway 会向上游注入：
  - `x-roze-subject`
  - `x-roze-tenant`
  - `x-roze-roles`
- Gateway 保留并透传 `x-request-id`、`x-trace-id` 和其它非 hop-by-hop header。
