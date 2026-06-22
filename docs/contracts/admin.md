# Roze Admin 控制面契约

`roze-admin` 提供控制面模型、适配器和可挂载的 axum Router。应用可以把这些模型挂到 Gateway、独立 admin API 或内部运维接口。

## HTTP Router

`admin_router(AdminState)` 暴露以下 JSON 端点：

- `GET /admin/registry/{service}`：查询服务实例。
- `GET /admin/config/reloads?offset=0&limit=100`：查询配置 reload 历史。
- `GET /admin/mq/stats`：查询 MQ 统计。
- `GET /admin/mq/dead-letters?topic=&group=&offset=0&limit=100`：查询 DLQ。
- `POST /admin/mq/dead-letters/{id}/replay`：重放 DLQ 消息。
- `DELETE /admin/mq/dead-letters/{id}`：删除 DLQ 记录。

未配置对应能力时返回 `404`；底层查询失败时返回 `502` 和 `{ "error": "..." }`。

## Auth

`AdminState::with_auth(AdminAuthConfig)` 可为所有 `/admin/...` 路由启用统一鉴权：

- Bearer token：`Authorization: Bearer <token>`
- API key：默认 header `x-api-key: <key>`，可自定义 header 名称

应用可用环境变量启用：

- `ROZE_ADMIN_TOKEN`
- `ROZE_ADMIN_API_KEY`
- `ROZE_ADMIN_API_KEY_HEADER`（默认 `x-api-key`）

未配置 auth 时 admin router 不做鉴权，适合本地开发或由外层 Ingress/Gateway 负责访问控制的部署。

## Registry

- `RegistryAdmin::service(name)`：从 `roze_rpc::registry::Registry` 查询服务实例。
- 返回 `RegistryServiceSnapshot`：
  - `service`
  - `instances[].name`
  - `instances[].addr`
  - `instances[].weight`
  - `instances[].metadata`

## Config Reload History

- `ConfigReloadHistory`：固定容量的 reload 审计环形历史。
- `record(&ReloadResult<T>)`：从配置中心 reload 事件提取审计字段，不保存完整 config。
- `list(offset, limit)`：按最新优先分页查询。
- `ConfigReloadAuditRecord` 包含：
  - version / old_version
  - hash / old_hash
  - source / namespace / app / key
  - changed / success / error
  - diff

## MQ

- `MqAdminView<A>` 复用 `roze_mq::MqAdmin`。
- `snapshot(query)` 返回：
  - `stats`
  - 按 `DeadLetterQuery` 过滤后的 `dead_letters`
- `replay_dead_letter(id)`：重放 DLQ 消息。
- `purge_dead_letter(id)`：删除 DLQ 记录。

## 当前边界

- 当前 crate 固化控制面数据、操作语义和基础 HTTP 路由。
- 可选 Bearer/API-Key 鉴权已内置；更复杂的 RBAC/OIDC、审计写入、OpenAPI 和 UI 由接入方实现。
- 后续 Admin API 可以直接复用这些模型，避免每个应用重复定义服务实例、配置历史和 DLQ 管理结构。
