# Roze 微服务架构框架能力矩阵

目标：按 Rust 原生方式提供完整的微服务底座。

## 核心边界

| 能力 | 状态 | 入口 |
| --- | --- | --- |
| HTTP 统一边界 | 已完成 | `roze-http`, `roze-middleware`, `roze-result`, `roze-error` |
| RPC 统一边界 | 已完成 | `roze-rpc`, `roze-grpc`, `roze-context` |
| 统一错误 | 已完成 | HTTP `RozeError`，RPC `status_from_error` metadata |
| Context 传递 | 已完成 | request id, trace id, auth, tenant, locale, timeout, metadata，统一 propagation headers，可跨 HTTP/RPC/MQ |
| 参数校验 | 已完成 | REST/RPC 生成入口均执行 `roze-validation`，支持 go-playground 风格标签、跨字段、条件必填、集合 `dive`、map key/value 校验 |
| 错误 i18n | 已完成基础版 | `x-roze-locale`, `Accept-Language`, validator code i18n |
| 服务注册发现 | 已完成代码层 | memory, dns, etcd lease 注册/续约, consul TTL, etcd watch, cached resolver |
| 配置中心 | 已完成代码层 | Etcd v3 原生 `/v3/watch`，断线按 revision 恢复 |
| Gateway | 已完成代码层 | axum/tower/tower-http, registry upstream, retry, health, outlier |
| MQ 抽象 | 已完成治理版 | publish/subscribe, retry, dead letter, idempotency, delay, stats, DLQ list/replay/purge, NATS JetStream, Context carrier |
| 对象存储 | 已完成基础契约 | local, S3 API, qiniu kodo, aliyun oss, tencent cos config, validation, presign boundary |
| 一致性工具 | 已完成基础版 | saga, in-memory outbox relay, inbox dedupe，outbox 可发布到任意 `roze_mq::Publisher` |
| DTM 基础服务 | 已完成基础版 | 默认 TCC，Saga 可选，HTTP 分支调用，分支屏障 |
| 认证授权 | 已完成基础版 | JWT, RBAC, tenant, ABAC attribute rule |
| ORM 默认契约 | 已完成基础版 | Toasty 默认生成；`--orm sea-orm` 可切换 SeaORM，通用 page/sort/filter/tenant/audit/soft-delete |
| 健康探针 | 已完成基础版 | liveness/readiness/startup probe report |
| 集成测试环境 | 已完成生产闭环入口 | `docker-compose.integration.yml`, `scripts/production-smoke.sh` |

## RPC 错误 metadata

RPC 错误统一使用 gRPC status + metadata：

- `x-roze-error-code`: HTTP 等价错误码，如 `400`, `401`, `404`, `500`
- `x-roze-error-kind`: `bad_request`, `unauthorized`, `not_found`, `internal`
- `x-roze-request-id`: 请求 ID
- `x-roze-trace-id`: Trace ID
- `x-roze-locale`: 当前 locale

生成器中的业务错误应通过 `roze_rpc::rpc::status_from_error(err, &request_ctx)` 返回。

## 配置中心 Etcd watch

Etcd source 使用 v3 原生接口：

- 初次读取：`/v3/kv/range`
- 热更新：`/v3/watch`
- 重连恢复：保存 watch event 的 `mod_revision` 或 header `revision`，重连时设置 `start_revision = last_revision + 1`
- 解析失败：保留旧配置并通知 reload listener

## 服务发现 Etcd watch

服务发现统一走 `roze_rpc::registry`：

- 注册：`EtcdRegistry` 使用 `/v3/lease/grant` 获取租约，再用 `/v3/kv/put` 写入 `/roze/services/{service}/{addr}`。
- 续约：后台任务按 `renew_interval_secs` 调用 `/v3/lease/keepalive`。
- 注销：停止续约任务并删除实例 key。
- 发现：`discover(service)` 使用 `/v3/kv/range` 读取 prefix 下所有实例。
- 动态刷新：`Registry::watch(service)` 对 prefix 调用 `/v3/watch`，收到 put/delete 事件后重新 discover 并推送完整实例快照。
- 缓存：`CachedRegistryResolver` 优先接收 watch 快照更新缓存，同时保留周期 refresh 作为兜底。

## 集成测试环境

本地启动：

```bash
docker compose -f docker-compose.integration.yml up -d
```

覆盖组件：

- Etcd: 配置中心 watch、服务注册发现
- Consul: 服务注册发现
- Kafka/NATS: MQ adapter smoke test
- Redis: 缓存/限流/幂等状态
- Postgres/MySQL: Toasty/SQL adapter smoke test
- Elasticsearch/OpenSearch/Meilisearch: search runtime 与 `rozectl search inspect/generate` 验收

生产闭环 smoke：

```bash
bash scripts/production-smoke.sh
bash scripts/production-smoke.sh --with-compose
```

`--with-compose` 会先启动真实依赖；默认只运行本地编译、生成器、crate 级测试和 generated project compile smoke。

## 后续验收要求

每个生产级 adapter 需要至少具备：

- 真实服务集成测试
- 断线重连测试
- 重试/超时/取消测试
- metrics 标签验证
- trace/context 透传验证
- i18n 错误响应验证

## Registry Prefix And Proxy Diagnostics

Etcd registry keys default to `/roze/services/{service}/{addr}`. Set
`registry.prefix`, for example `/shop/services`, to write/read/watch
`/shop/services/{service}/{addr}` while preserving the same lease, discover,
and watch behavior.

Registry and config-center HTTP clients use the process proxy environment that
`reqwest` honors. For private control-plane endpoints such as etcd or Consul on
`192.168.0.0/16`, `10.0.0.0/8`, `172.16.0.0/12`, loopback, or local hosts,
include the endpoint host in `NO_PROXY` or clear `HTTP_PROXY`, `HTTPS_PROXY`,
and `ALL_PROXY`; Roze adds a diagnostic hint to request failures when a proxy
environment is active and the endpoint is internal.
