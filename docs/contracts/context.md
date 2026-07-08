# Roze Context 契约（v1�?

Roze Context �?REST、RPC、Gateway、日志、链路追踪和治理规则共享的请求元数据模型。它不是全局资源容器，也不是业务用户对象容器�?

新代码按职责选择上下文载体：

| 数据类型 | 实现方式 | 作用�?|
| --- | --- | --- |
| 全局资源，例�?DB、Redis、配置、RPC client | `ServiceContext` | 应用全局，通过 Roze native HTTP `State` 共享 |
| 请求元数据，例如 request_id、trace_id、deadline、认证主体、透传 metadata | `roze_context::Context` | 单次请求，框架中间件写入 Roze native HTTP `Extension<Context>`，跨 REST/RPC/Gateway 传播 |
| 业务用户对象，例如登录用户详情、权限快�?| Roze native HTTP `Extension<T>` | 单次 HTTP 请求，由认证/会话中间件注入，handler 提取后显式传�?logic |
| 链路追踪和日志上下文 | `tracing::Span` | 当前异步任务隐式传播，业务代码直接使�?`tracing` �?|
| 超时、限流、熔断、降载等横切逻辑 | middleware / route governance | 框架层处理，logic 不应自己实现通用治理 |

业务 logic 中需要日志时直接使用 `tracing::info!`、`tracing::warn!`、`tracing::error!` 等宏。只要请求入口已经经�?Roze tracing middleware，日志会自动关联当前请求 Span �?`trace_id`，不要为了打日志在业务函数签名里手动传�?`trace_id`�?

## 标准字段

- `request_id`：单次入口请�?ID，用于日志检索和错误响应定位�?
- `trace_id`：链路追�?ID，用于跨服务调用串联�?
- `deadline`：请求剩余时间，�?RPC 调用传递为剩余毫秒数�?
- `auth`�?
  - `subject`：认证主�?ID�?
  - `roles`：主体角色列表�?
  - `tenant`：租�?ID，可选�?
- `metadata`：业务透传元数据，使用字符�?key/value�?

## HTTP Header

- `x-request-id`：对�?`Context::request_id()`�?
- `x-trace-id`：对�?`Context::trace_id()`�?
- `x-roze-timeout-ms`：剩�?timeout 毫秒数�?
- `x-roze-locale`：请求语言，例�?`zh-CN`、`en-US`�?
- `Accept-Language`：未提供 `x-roze-locale` 时作为语言来源�?
- `x-roze-subject`：认证主体�?
- `x-roze-tenant`：租户�?
- `x-roze-roles`：逗号分隔角色列表�?
- `x-roze-meta-*`：业�?metadata，例�?`x-roze-meta-locale: zh-CN` 会进�?`metadata["locale"]`�?

## REST 边界

- `roze-middleware::roze_http_request_context` �?HTTP header 构�?`Context` 并写�?Roze native HTTP request extensions�?
- 未传�?`x-request-id` �?`x-trace-id` 时，入口中间件生成新值并回写响应 header�?
- `x-roze-locale` �?`Accept-Language` 会进�?`Context::locale()`�?
- `roze-middleware::roze_http_auth` 验证 JWT 后把 `subject/roles/tenant` 写入同一�?`Context`�?
- `roze-middleware::roze_http_trace` 为每个请求创建根 tracing Span，Span 字段包含 `trace_id`�?
- 需要完整业务用户对象时，自定义认证/会话中间件应把对象写�?Roze native HTTP `Extension<User>`，handler 通过 `Extension(user): Extension<User>` 提取，再把需要的字段显式传给 logic�?

`Context` 保留 `trace_id` 是为了跨协议传播、错误响应和 RPC metadata，不代表业务代码需要手动传递它来记录日志�?

## RPC/gRPC 边界

- `roze_rpc::rpc::apply_request_context` �?`roze_grpc::apply_context` �?Context 写入 tonic metadata�?
- `roze_rpc::rpc::request_context` �?`roze_grpc::request_context` �?tonic metadata 恢复完整 Context�?
- 生成�?RPC client 应继续以 `&roze_context::Context` 作为第一个业务上下文参数�?
- RPC 错误统一�?`roze_rpc::rpc::status_from_error` 生成，metadata 包含�?
  - `x-roze-error-code`
  - `x-roze-error-kind`
  - `x-request-id`
  - `x-trace-id`
  - `x-roze-locale`

## Gateway 边界

- Gateway 统一使用 `roze-context` 标准 header�?
- JWT 鉴权通过后，Gateway 会向上游注入�?
  - `x-roze-subject`
  - `x-roze-tenant`
  - `x-roze-roles`
- Gateway 保留并透传 `x-request-id`、`x-trace-id` 和其它非 hop-by-hop header�?
