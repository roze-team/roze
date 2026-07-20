# Roze Admin 控制面契约

`roze-admin` 提供控制面数据模型、registry/config/MQ 适配器，以及可挂载到
Roze native HTTP 的 Tower service。它不自动开放公网端口，接入方必须显式选择
listener、网络策略和鉴权方式。

## HTTP Service

使用 `admin_service(AdminState)` 构造 service。当前 HTTP surface 只有：

- `GET /admin/config/reloads`

该端点返回最近 100 条配置 reload 审计记录；未配置
`ConfigReloadHistory` 时返回空数组。其他 path 返回 404。`RegistryAdmin` 和
`MqAdminView` 当前是程序化适配器，尚未由这个基础 service 暴露为 HTTP endpoint。
新增 endpoint 时必须同步本契约、鉴权测试和 OpenAPI/运维说明。

## 鉴权

`AdminState::with_auth(AdminAuthConfig)` 为全部 admin path 启用一种鉴权：

- `AdminAuthConfig::bearer(token)`：
  `Authorization: Bearer <token>`。
- `AdminAuthConfig::api_key(key)`：固定 header `x-api-key: <key>`。

`AdminAuthConfig::from_env()` 按以下优先级读取：

1. `ROZE_ADMIN_BEARER`
2. `ROZE_ADMIN_API_KEY`

两者都未设置且应用未显式调用 `with_auth` 时，service 不鉴权。这只适合本地开发
或已有强制网络隔离与外层鉴权的内部 listener。生产环境不得把未鉴权的 admin
service 挂到公网 listener。

## Registry 适配器

`RegistryAdmin::new(Arc<dyn Registry>)` 包装 `roze_rpc::registry::Registry`。
`service(name)` 返回 `RegistryServiceSnapshot`：

- `service`
- `instances[].name`
- `instances[].addr`
- `instances[].weight`
- `instances[].metadata`

调用方负责校验 service name 来源、限制返回 metadata，并避免把 endpoint 或实例
标识用作 Prometheus label。

## 配置 reload 历史

`ConfigReloadHistory` 是固定容量的内存环形历史：

- `new(capacity)` 创建有界历史。
- `record(&ReloadResult<T>)` 提取审计字段，不保存完整配置或 secret。
- `push(record)` 写入预构造记录。
- `list(offset, limit)` 按最新优先分页。

`ConfigReloadAuditRecord` 包含 version、old_version、hash、old_hash、source、
namespace、app、key、changed、success、error 和 diff。接入方若需要持久审计，
应将这些记录写入受保护的审计存储，而不是无限扩大内存容量。

## MQ 适配器

`MqAdminView<A>` 包装实现 `roze_mq::MqAdmin` 的 adapter：

- `snapshot(query)` 返回 `stats` 与按 `DeadLetterQuery` 过滤的
  `dead_letters`。
- `replay_dead_letter(id)` 请求重放一条 DLQ 消息。

当前基础 API 不提供 purge 方法，也不自动暴露 MQ HTTP endpoint。重放属于有副
作用的管理动作；未来接入 HTTP 时必须使用独立权限、幂等审计和防重放保护。

## 安全边界

- admin listener 与业务 listener 默认分离。
- token/API key 只从 secret 或受保护环境注入，不写入日志、错误正文或指标。
- 所有写操作必须有主体、权限、request ID、trace ID 和审计结果。
- 列表 endpoint 必须有有界分页；不得返回未脱敏配置或 broker payload。
- 更复杂的 RBAC、OIDC、mTLS、持久审计、UI 与 OpenAPI 由接入方显式实现。

## 验证

```bash
cargo test -p roze-admin
```

HTTP surface、鉴权方式或审计字段变化时，必须同时更新测试和本契约。
