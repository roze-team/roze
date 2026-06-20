# Roze Context 契约（v1）

Roze Context 是 REST、RPC、Gateway、日志、链路追踪和治理规则共享的请求上下文模型。新代码应以 `roze_context::Context` 作为唯一请求上下文入口。

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
- `x-roze-subject`：认证主体。
- `x-roze-tenant`：租户。
- `x-roze-roles`：逗号分隔角色列表。
- `x-roze-meta-*`：业务 metadata，例如 `x-roze-meta-locale: zh-CN` 会进入 `metadata["locale"]`。

## REST 边界

- `roze-middleware::axum_request_context` 从 HTTP header 构造 `Context` 并写入 Axum request extensions。
- 未传入 `x-request-id` 或 `x-trace-id` 时，入口中间件生成新值并回写响应 header。
- `roze-middleware::axum_auth` 验证 JWT 后把 `subject/roles/tenant` 写入同一个 `Context`。

## RPC/gRPC 边界

- `roze_rpc::rpc::apply_request_context` 和 `roze_grpc::apply_context` 将 Context 写入 tonic metadata。
- `roze_rpc::rpc::request_context` 和 `roze_grpc::request_context` 从 tonic metadata 恢复完整 Context。
- 生成的 RPC client 应继续以 `&roze_context::Context` 作为第一个业务上下文参数。

## Gateway 边界

- Gateway 统一使用 `roze-context` 标准 header。
- JWT 鉴权通过后，Gateway 会向上游注入：
  - `x-roze-subject`
  - `x-roze-tenant`
  - `x-roze-roles`
- Gateway 保留并透传 `x-request-id`、`x-trace-id` 和其它非 hop-by-hop header。

