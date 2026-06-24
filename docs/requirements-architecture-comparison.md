# 微服务框架需求与 Roze 当前架构对比

本文把“最核心的 12 类能力”整理成可执行的产品/架构需求，并对比 Roze 当前仓库里的实现状态。状态判断以现有 README、成熟度矩阵、契约文档和 `rozectl` 命令结构为准。

## 总体判断

Roze 的方向与需求高度一致：已经采用 IDL first、生成器维护边界代码、业务代码落在 `logic`、HTTP/RPC/Gateway/MQ/Config/Registry/Observability 拆为独立 crate 的架构。

当前主要问题不是“没有模块”，而是“模块成熟度和生产闭环不足”。多数核心模块处于 `beta`，生命周期、分布式事务、部署生成仍是 `scaffold`。下一阶段应该优先把已有能力做成可验证、可升级、可观测、可恢复的生产框架，而不是继续横向铺新 crate。

## 本轮处理标记

处理日期：2026-06-24

以下需求已完成实现、文档同步和本地验证：

- 已处理：移除兼容入口口径，CLI 和文档只保留 Roze 原生命令。
- 已处理：`rozectl model generate <schema> --out <dir>` 原生模型生成。
- 已处理：`rozectl model inspect <table> --db-kind <sqlite|postgres|mysql|mongo> --db-url <url> --out <dir>`，覆盖 sqlite、Postgres、MySQL、MongoDB。
- 已处理：`rozectl search generate <schema> --engine <elasticsearch|opensearch|meilisearch> --out <dir>`。
- 已处理：`rozectl search inspect <index> --engine <elasticsearch|opensearch|meilisearch> --url <url> --out <dir>`。
- 已处理：`roze-search` 运行时 health、index document、delete document、search 调用封装。
- 已处理：README、usage 文档、项目规范、成熟度矩阵和本文件已同步搜索/模型能力。

## 覆盖度口径

本文里的覆盖度不是按“有没有 crate”判断，而是按可交付程度判断。

| 覆盖度 | 判断标准 |
| --- | --- |
| 高 | 已有生成器或运行时主路径，用户可以按文档完成常规使用；仍可能缺少边界测试或高级能力。 |
| 中高 | 核心 runtime 已具备，但生产样例、故障测试、运维资产或跨模块一致性还不完整。 |
| 中 | 有基础 API/抽象和部分实现，但还需要统一模型、生成器接入或真实依赖验证。 |
| 中低 | 有雏形或 report/helper，但尚未成为生成服务默认能力，也缺少端到端验收。 |
| 低 | 只有方向或少量基础代码，不能作为框架能力对外承诺。 |

生产稳定度另按 `docs/maturity.md` 的 `stable/beta/scaffold/planned` 标注判断。一个能力即使覆盖度是“中高”，只要缺少真实依赖测试、升级说明和生产示例，也不应该标成 `stable`。

## 12 类能力对比矩阵

| # | 需求能力 | Roze 当前覆盖 | 主要入口 | 缺口/风险 |
| --- | --- | --- | --- | --- |
| 1 | API/RPC 契约优先 | 高 | `apps/rozectl`, `.api`, proto, REST/RPC generator, OpenAPI, TS/JS/Dart SDK | 已具备 `rozectl contract check` 和 `rozectl mock gen` MVP；仍缺接口测试生成；OpenAPI validator 投影仍有缺口；SDK 缺 error/interceptor/retry/timeout。 |
| 2 | Gateway 网关 | 中高 | `crates/roze-gateway`, `apps/roze-gateway`, `docs/contracts/gateway.md` | 已有路由、rewrite、auth、rate/breaker、retry、fallback、registry upstream、canary、health/outlier、hot reload、WebSocket upgrade 代理、SSE 流式代理；缺更完整 app 级示例、deploy smoke test、A/B、流量镜像。 |
| 3 | 服务注册与发现 | 中高 | `roze-rpc::registry`, cached resolver, etcd/consul/dns/memory | 代码层已具备；还需要失败模式测试、生产配置示例、Gateway/RPC/Job/MQ consumer 复用边界文档化。 |
| 4 | 统一治理模型 | 中 | `roze-config`, `roze-middleware`, `roze-rpc`, `roze-gateway` | Gateway 已继承 timeout/retry/rate/breaker；HTTP/RPC/MQ/Job 还需要同一 schema、同一指标标签、可选持久化 breaker/rate limiter。 |
| 5 | 配置中心 | 中高 | `crates/roze-config`, `docs/contracts/config-center.md` | Etcd watch、Env/File fallback、diff/version、section event、失败回滚已具备；仍需 listener timeout/failure isolation、灰度、签名、审计操作者模型。 |
| 6 | 可观测性 | 中 | `roze-log`, `roze-metrics`, `roze-prometheus`, `roze-opentelemetry`, `deploy/observability` | tracing/metrics/Prometheus/OTel 已有；已提供 Gateway Prometheus scrape 示例、recording rules、alert rules、Grafana dashboard 和 SLO 模板；仍缺 trace 示例、日志查询示例和更完整 dashboard pack。 |
| 7 | 健康检查和生命周期 | 中 | `roze-health`, `roze-bootstrap`, `roze-shutdown`, REST templates | probe report 和 REST 标准 `/healthz`、`/readyz`、`/startupz`、`/metrics` 入口已有；依赖 readiness、Gateway/RPC 标准入口、HTTP/RPC/MQ/Job 统一启动关闭顺序还没收口。 |
| 8 | 安全能力 | 中 | `roze-jwt`, `roze-auth`, `roze-permission`, Gateway auth | JWT/RBAC/tenant/ABAC primitives 已有；缺统一安全模型、OIDC/OAuth2、mTLS、permission 注解到 OpenAPI/SDK/test、key rotation、审计日志模板。 |
| 9 | MQ/EventBus/可靠事件 | 中高 | `roze-mq`, `roze-kafka`, `roze-nats`, `roze-eventbus`, outbox/inbox primitives | publish/subscribe、retry、DLQ、stats、NATS JetStream、Kafka ack/nack 已有；缺真实 broker 集成覆盖、生产 replay/purge 示例、标准事件 envelope 完整化。 |
| 10 | 数据库、缓存、事务、搜索 | 中高 | `roze-db`, `roze-orm`, `roze-sqlx`, `roze-cache`, `roze-redis`, `roze-singleflight`, `roze-transaction`, `roze-dtm`, `roze-search`, `rozectl model`, `rozectl search` | SQL/Mongo model generate/inspect、cache、singleflight、TCC/Saga/outbox/inbox primitives、Elasticsearch/OpenSearch/Meilisearch search generate/inspect 已有；缺 DB transaction + outbox + MQ + RPC 完整示例和边界测试。 |
| 11 | 部署和运维 | 中低 | `rozectl docker`, `rozectl kube`, `docker-compose.integration.yml` | Docker/K8s 生成已有；缺 Helm、doctor、manifest validation、NetworkPolicy/ServiceAccount/PDB/HPA 更完整模板和生产验收脚本。 |
| 12 | CLI/生成器/AI 友好 | 中高 | `rozectl api/rpc/model/search/openapi/client/docker/kube/diff/doctor/doc`, 稳定生成目录 | 生成主干很强；已具备文件级 `diff` 预览、本机 `doctor` 检查、`SERVICE.md` 生成、SQL/Mongo model inspect 和 Elasticsearch/OpenSearch/Meilisearch search inspect；仍缺 `dev`、`stream gen`、`AI_CONTEXT.md`/`ARCHITECTURE.md`/`DEPENDENCIES.md` 自动生成。 |

## 功能规格清单

这一节把 12 类能力拆成可实现功能。`MVP` 表示进入 P0/P1 前必须有的最小功能，`增强项` 表示生产完善或高级治理能力。

### 1. API/RPC 契约优先

| 功能 | MVP | 增强项 | 推荐落点 | 验收 |
| --- | --- | --- | --- | --- |
| `.api` REST 生成 | route/handler/logic/types/config/openapi/svc 固定结构 | 更完整 Roze `.api` 语法、注释、import、validator tag | `apps/rozectl` | 生成项目可编译，`--update` 不覆盖 `logic`。 |
| proto/RPC 生成 | server/client/pb/logic/config/svc 固定结构 | streaming、metadata 策略、proto fixture 覆盖 | `apps/rozectl`, `roze-rpc` | RPC 生成项目可编译，Context metadata 可透传。 |
| OpenAPI 生成 | paths、schemas、parameters、request/response、security | validator 约束完整投影、examples、error schema | `apps/rozectl`, `roze-openapi` | Swagger UI/主流 client generator 可消费。 |
| SDK 生成 | TS/JS/Dart baseUrl、headers、path/query/body | typed error、interceptor、retry、timeout、auth injection | `apps/rozectl/src/generator/client.rs` | SDK 能调用 mock server 并处理错误响应。 |
| mock 生成 | 已支持从 `.api` 生成独立 Axum mock server，并按 response type 返回默认 JSON | 示例数据、延迟/错误注入、OpenAPI example 驱动 | `rozectl mock gen` | 本地无需业务实现即可返回契约响应。 |
| 接口测试生成 | HTTP smoke test、RPC smoke test、OpenAPI schema validation | 契约回归、鉴权场景、错误码场景 | `rozectl test gen` | 生成测试能在空逻辑或 mock 上运行。 |
| 生成预览 | 文件级 `A/M/D` diff，按 update 规则保护业务文件 | ownership-aware diff、语义 diff | `rozectl diff` | 默认不写盘，输出清晰。 |
| 契约破坏性变更检查 | 已支持检查 route/method/path 删除或变更、RPC 方法删除、request/response 类型变更、字段删除、字段类型/source 变更、必填字段新增 | semver 建议、SDK breaking report、响应字段变更策略细化 | `rozectl contract check` | breaking changes 明确失败并给出原因。 |

### 2. Gateway 网关

| 功能 | MVP | 增强项 | 推荐落点 | 验收 |
| --- | --- | --- | --- | --- |
| 路由转发 | prefix/method 匹配、path rewrite、query/header/body 透传 | host/path/header 组合匹配 | `roze-gateway` | mock upstream 返回可原样透传。 |
| 鉴权 | JWT、API Key、CORS、body/header limit | OIDC、mTLS、请求签名、防重放 | `roze-gateway`, `roze-auth` | 未授权返回统一 401/403，成功注入 auth context header。 |
| 治理 | timeout、rate limit、breaker、retry、fallback | retry budget、adaptive shedding、bulkhead | `roze-gateway`, governance schema | 每项治理有 smoke test 和 metrics。 |
| 灰度路由 | weight、instance_tags、registry upstream | header/cookie/tenant/user 分流、A/B、流量镜像 | `roze-gateway` | 相同路由可按权重或标签命中不同上游。 |
| 协议形态 | HTTP request/response、WebSocket upgrade 双向转发、SSE `text/event-stream` 流式转发、流式响应空闲超时 `stream_idle_timeout_ms`、长连接上限 `max_stream_connections`、连接级 opened/closed/rejected/duration/active 指标、连接级 dashboard/alert | gRPC-Web、更多连接级 runbook | `roze-gateway`, `deploy/observability` | HTTP、WS、SSE 可共用同一套 route/service/governance 配置，SSE 首个事件不等待完整响应结束，空闲流和活跃连接数可按 route/service/gateway 配置治理，并可按 route/protocol 观测连接生命周期。 |
| 上游健康 | 主动 health check、outlier ejection | 异常实例自动摘除、慢实例降权 | `roze-gateway` | 失败实例不参与路由，恢复后重新加入。 |
| 热更新 | 配置中心 reload 后重建路由 | 灰度配置、回滚、签名校验 | `apps/roze-gateway`, `roze-config` | invalid config 不替换旧路由。 |
| 观测 | request_id/trace_id、upstream metrics、retry metrics | dashboard、alert、route SLO | `roze-metrics`, `roze-prometheus` | 可按 route/upstream 查看延迟、错误、重试、熔断。 |

### 3. 服务注册与发现

| 功能 | MVP | 增强项 | 推荐落点 | 验收 |
| --- | --- | --- | --- | --- |
| 静态 upstream | 支持固定 URL/address | 多 upstream 加权 | `roze-rpc::registry`, `roze-gateway` | 无注册中心也能运行。 |
| DNS discovery | 解析 DNS/service name | TTL cache、失败回退 | `roze-rpc::registry` | DNS 变更后 resolver 可刷新。 |
| etcd/Consul | 注册、续约、下线、watch、discover | lease 续约失败处理、watch 断线恢复 | `roze-rpc::registry` | 实例上下线能被 Gateway/RPC client 感知。 |
| Kubernetes discovery | 支持 Service DNS 或 API discovery | namespace/label selector | registry adapter | K8s 内无需手写 IP。 |
| 本地缓存 | Cached resolver、周期 refresh | watch 优先 + refresh 兜底 | `CachedRegistryResolver` | 注册中心短暂不可用时继续使用旧快照。 |
| 路由策略 | round_robin、weight、tag filter | latency-aware、zone-aware | registry/gateway shared policy | 标签不匹配时不会误回退到错误实例。 |

### 4. 统一治理模型

| 功能 | MVP | 增强项 | 推荐落点 | 验收 |
| --- | --- | --- | --- | --- |
| timeout/deadline | 全局、route/method、consumer 级 timeout | deadline 跨 HTTP/RPC/MQ 传播 | `roze-config`, `roze-context` | 下游收到剩余 deadline。 |
| retry | max attempts、backoff、retryable error | retry budget、jitter、风暴保护 | shared governance + protocol adapters | 不可重试错误不重试，可重试错误有指标。 |
| rate limit | local token bucket | distributed limiter、租户/用户维度 | `roze-middleware`, `roze-gateway` | 超限返回统一错误并记录 rejected metric。 |
| circuit breaker | failure threshold、cool down | half-open、持久化状态、按实例熔断 | `roze-middleware`, `roze-rpc`, `roze-gateway` | 熔断打开后快速失败，恢复窗口可探测。 |
| shedding/bulkhead | max concurrency、latency/failure shedding | adaptive policy、pool 隔离 | `roze-middleware` | 高负载下主动拒绝而不是拖垮服务。 |
| fallback | route/method fallback | typed fallback、cache fallback | gateway/runtime adapters | fallback 和上游透传响应边界清晰。 |
| cancel propagation | request cancel 传播 | MQ/Job cancel cooperative hooks | `roze-context`, `roze-shutdown` | 客户端断开或 shutdown 能取消下游工作。 |

### 5. 配置中心

| 功能 | MVP | 增强项 | 推荐落点 | 验收 |
| --- | --- | --- | --- | --- |
| 配置源 | File、Env、Etcd | Consul/Nacos、K8s ConfigMap watch | `roze-config` | 优先级和 fallback 明确。 |
| 热更新 | watch/poll、debounce、reload event | section listener、局部重建 helper | `roze-config` | Kafka 变更只重建 Kafka pipeline。 |
| 安全替换 | 解析失败保留旧配置 | listener timeout/failure isolation | `roze-config` | 一个 listener 失败不影响其它 listener。 |
| 审计 | version、hash、diff、source、time | operator、signature、approval、rollback | `roze-config`, `roze-admin` | 每次变更能查询 diff 和来源。 |
| 敏感配置 | 脱敏打印 | secret manager、加密、签名 | `roze-config` | 日志不泄露 secret。 |
| 灰度配置 | 按 app/namespace key | tenant/user/instance 灰度 | config center/admin | 部分实例先应用配置，失败可回滚。 |

### 6. 可观测性

| 功能 | MVP | 增强项 | 推荐落点 | 验收 |
| --- | --- | --- | --- | --- |
| 日志 | structured tracing、request_id、trace_id | 日志脱敏、采样、查询模板 | `roze-log`, `roze-trace` | 每条入口日志能关联 request/trace。 |
| Metrics | HTTP/RPC/Gateway/MQ/DB/Redis 基础指标 | 统一 labels、直方图 buckets、SLO recording rules | `roze-metrics`, `roze-prometheus` | Prometheus scrape 后能画 p95/error rate。 |
| Tracing | OpenTelemetry trace context | baggage、sampling、Jaeger/OTLP 示例 | `roze-opentelemetry` | HTTP -> RPC -> MQ trace 可串联。 |
| 事件观测 | config reload、retry、breaker、rate limit、DLQ | audit event stream | metrics/log/admin | 治理事件有结构化字段。 |
| Dashboard | 已提供 Gateway 最小 Grafana dashboard | 服务、MQ、配置中心 dashboard pack | `deploy/observability` | 本地示例能导入 dashboard。 |
| Alert | 已提供 Gateway Prometheus alert rules 和 recording rules | DLQ、breaker state、更多 burn-rate 窗口 | `deploy/observability` | 关键故障有默认告警规则。 |

### 7. 健康检查和生命周期

| 功能 | MVP | 增强项 | 推荐落点 | 验收 |
| --- | --- | --- | --- | --- |
| 标准 probes | REST 生成服务已有 `/healthz`、`/readyz`、`/startupz`、`/metrics` | dependency details、JSON report、Gateway/RPC 模板统一 | `roze-health`, templates | K8s probe 可直接使用。 |
| readiness | DB/Redis/MQ/RPC/config/background tasks | 权重 readiness、drain 状态 | `roze-health` | 依赖不可用时实例不接流量。 |
| bootstrap | 统一启动 HTTP/RPC/MQ/Job | component dependency graph | `roze-bootstrap` | 各 component 按顺序启动。 |
| shutdown | SIGINT/SIGTERM、deadline、graceful shutdown | drain mode、preStop hook、shutdown phases | `roze-shutdown` | 终止后不接新请求并在 deadline 内退出。 |
| background tasks | task manager、cancel token | restart policy、task health | `roze-bootstrap` | 后台任务失败会影响 readiness 或报警。 |

### 8. 安全能力

| 功能 | MVP | 增强项 | 推荐落点 | 验收 |
| --- | --- | --- | --- | --- |
| 认证 | JWT、API Key | OIDC/OAuth2、mTLS | `roze-auth`, `roze-jwt`, Gateway | 未认证和认证失败错误一致。 |
| 授权 | RBAC、tenant、ABAC primitives | object-level authorization、policy engine | `roze-permission` | 权限失败返回统一 403。 |
| 契约注解 | `.api` jwt/permission/tenant/audit | idempotent、rate_limit、body_limit、timeout | parser/generator | 注解能生成 middleware 绑定和 OpenAPI security。 |
| 审计 | 操作者、资源、动作、结果 | 审计日志 sink、查询 API | `roze-admin`, app templates | 敏感操作有审计事件。 |
| 输入和资源限制 | validation、body/header limit、rate limit | SSRF guard、field-level auth | middleware/gateway | OWASP API 常见风险有默认防线。 |
| Secret 处理 | 脱敏显示 | SecretString、rotation、external secret | `roze-config` | secret 不进入普通日志和错误。 |

### 9. MQ/EventBus/可靠事件

| 功能 | MVP | 增强项 | 推荐落点 | 验收 |
| --- | --- | --- | --- | --- |
| Event envelope | event_id/type/version/trace/idempotency/occurred_at | schema registry、breaking-change check | `roze-mq`, `roze-eventbus` | Kafka/NATS/in-memory metadata 一致。 |
| Producer | publish result、headers、trace carrier | transaction-aware publish | `roze-mq`, adapters | publish 返回 topic/partition/offset 或等价 metadata。 |
| Consumer | ack/nack/retry/DLQ | delayed retry、retry storm protection | `roze-mq`, `roze-kafka`, `roze-nats` | 失败超过上限进入 DLQ。 |
| Admin | DLQ list/replay/purge | UI、权限、审计 | `roze-admin`, adapters | 可重投指定死信并记录结果。 |
| Outbox/Inbox | outbox relay、inbox dedupe | DB transaction examples、cleanup policy | `roze-transaction`, `roze-dtm` | DB 状态和消息发布一致。 |
| Metrics | processed、failed、lag、offset、DLQ count | consumer SLO、partition dashboard | `roze-metrics` | topic/group/partition 维度可观测。 |

### 10. 数据库、缓存、事务

| 功能 | MVP | 增强项 | 推荐落点 | 验收 |
| --- | --- | --- | --- | --- |
| Model 生成 | 已处理：Toasty 默认、SeaORM 可选，生成单表 CRUD、`count`、分页、等值/IN/范围条件、可空字段等值/IS NULL、排序、批量、软删、租户限定方法、独立字段文件、extension 保留文件、复合索引查询方法、Toasty/SeaORM 本地事务 helper；`model inspect` 覆盖 sqlite、Postgres、MySQL、MongoDB | schema namespace、关系、跨字段复杂条件、Unit of Work 示例、事务边界守卫 | `apps/rozectl`, `roze-orm` | 默认生成 Toasty，`--orm sea-orm` 切换；字段元数据独立，`<model>_ext.rs` 在 `--update` 保留，基础增删改查和增强查询可直接调用，Toasty/SeaORM 可显式开启本地事务边界。 |
| Search 生成 | 已处理：`rozectl search generate/inspect` 支持 Elasticsearch、OpenSearch、Meilisearch，生成 `src/search/mod.rs` 和 `src/search/<index>.rs`，保留原始字段名，提供 health/index/delete/search repository | ranking/boosting query builder、搜索结果高亮、facets 聚合 helpers | `apps/rozectl`, `roze-search` | `.search` DSL、JSON schema 和已有 index inspect 都能生成同一 repository 形态；Elasticsearch/OpenSearch 读取 mapping，Meilisearch 读取 settings/index metadata 并采样 documents。 |
| DB 连接 | pool config、timeout | read/write split、多数据源 | `roze-db`, `roze-sqlx` | 连接失败影响 readiness。 |
| Migration | migration scaffold | rollback、dry-run、status | `roze-migration`, `rozectl migrate` | 本地示例能执行 migration。 |
| 事务上下文 | 本地事务 helper | Unit of Work、nested boundary guard | `roze-transaction` | 业务 logic 能显式控制事务。 |
| 缓存 | Redis client、cache aside helper | read/write through、TTL jitter、Bloom filter | `roze-cache`, `roze-redis` | 缓存击穿/雪崩有模板策略。 |
| singleflight/lock | singleflight、防击穿 | distributed lock、fencing token | `roze-singleflight`, `roze-redis` | 并发 miss 只回源一次。 |
| 分布式事务 | TCC/Saga primitives | 状态查询、补偿任务、admin UI | `roze-dtm` | 示例覆盖 try/confirm/cancel 或 saga compensation。 |

### 11. 部署和运维

| 功能 | MVP | 增强项 | 推荐落点 | 验收 |
| --- | --- | --- | --- | --- |
| Dockerfile | multi-stage build、port、timezone | distroless、SBOM、non-root | `rozectl docker` | 生成镜像可运行 healthz。 |
| Kubernetes YAML | Deployment/Service/resources/HPA 和标准 liveness/readiness/startup probes | PDB、NetworkPolicy、ServiceAccount、Gateway API | `rozectl kube` | `kubectl apply --dry-run=client` 可通过。 |
| Helm | values、templates、probes、resources | chart tests、schema validation | `rozectl helm` | helm template 输出可部署 YAML。 |
| doctor | 本机工具、端口、配置、TCP 依赖地址 live probe | 权限、版本检查、协议级 probe | `rozectl doctor` | 缺依赖时给出明确修复建议。 |
| dev | docker compose up/down、依赖状态 | profiles、seed data、logs | `rozectl dev` | 新用户一条命令启动本地依赖。 |
| 观测部署 | scrape config、dashboard、alerts | canary checks、runbook | `deploy/observability` | 示例环境可直接导入。 |

### 12. CLI/生成器/AI 友好

| 功能 | MVP | 增强项 | 推荐落点 | 验收 |
| --- | --- | --- | --- | --- |
| new/generate/update | api/rpc/model/search 主路径 | template customization、plugin hooks | `apps/rozectl` | 重复生成稳定、可预测。 |
| diff | 文件级 diff | ownership-aware diff、breaking change report | `rozectl diff` | 默认不写盘，输出清晰。 |
| doctor/dev | `doctor` 已有本机工具、端口、配置和 TCP live probe；本地依赖启动以 `docker compose` 和生成部署资产为入口 | 自动修复建议、profile 管理、协议级 probe | `rozectl doctor`, `docker-compose.integration.yml` | 本地 onboarding 时间可控。 |
| doc/sdk/test/mock | 文档、SDK、测试、mock 生成 | multi-language SDK、contract tests | `apps/rozectl` | 从契约能生成可运行辅助资产。 |
| AI_CONTEXT | `rozectl doc service` 已可从 `.api` 生成 `SERVICE.md`，包含接口、所有权边界、常用命令和 AI notes | AI_CONTEXT/ARCHITECTURE/DEPENDENCIES、依赖图、topic/cache key 提取 | `rozectl doc service` / `rozectl doc gen` | AI 能知道哪些文件能改、哪些不能改。 |
| upgrade | update/diff/breaking-change check | migration guide automation | `rozectl update` | 框架升级能预览影响并保护业务代码。 |

## 当前架构资产盘点

### 已经形成优势的部分

1. 生成器边界清晰：REST/RPC 生成项目已经区分框架拥有文件和业务拥有文件，`logic` 保留策略对人和 AI 都友好。
2. crate 拆分方向正确：HTTP、RPC、Context、Error、Metrics、Config、Gateway、MQ、Registry、DTM、Storage 等能力已经不是散落在示例应用里。
3. 生成器入口单一：CLI 只保留 Roze 原生命令，同时输出 Rust-native 项目结构。
4. Gateway 和配置中心已经有运行契约文档，不只是 README 级描述。
5. MQ/Kafka/NATS 已经开始明确 ack/nack/retry/DLQ/admin replay 等语义，方向比“简单 publish/subscribe”更接近生产。

### 当前最容易误判的部分

1. “已有 crate”不等于“生产稳定”：生命周期、部署、DTM、事务链路示例仍需要按 scaffold 看待。
2. “生成器能生成”不等于“可安全升级”：还需要 `diff`、更多 repeated generation tests、generated-project compile tests。
3. “有 metrics/tracing crate”不等于“可观测闭环”：还缺 dashboard、alert、SLO、trace/log 查询示例和标签一致性验证。
4. “有 JWT/RBAC primitives”不等于“安全模型完整”：还缺契约注解、OpenAPI 投影、测试骨架、key rotation、审计日志和对象级授权模板。

## 关键能力拆解

### 1. 契约优先

目标形态：

- `.api` 和 proto 是唯一接口事实来源。
- route、handler、types、OpenAPI、SDK、测试骨架都从契约生成。
- 业务只改 `logic`、自定义 middleware 和明确的 application module。
- 重复生成默认走 `--update`，并能先 `diff` 再落盘。

Roze 现状：

- REST/RPC/OpenAPI/TS/JS/Dart SDK 主链路已具备。
- `--update` 已保留业务逻辑和自定义 middleware。
- 已具备文件级 `rozectl diff`、`rozectl mock gen`、`rozectl test gen` 和 `rozectl contract check`；语义 diff 继续作为增强项。

建议下一步：

- `rozectl diff` 已先落地文件级 diff；下一步再补语义 diff 和 breaking change report。
- `rozectl test gen` 先生成 HTTP smoke cases 和 OpenAPI schema validation cases。
- `rozectl contract check` 已覆盖 breaking changes：删除 route、改 method、改 path、删除字段、必填字段新增、响应类型变化。

### 2. 统一治理

目标形态：

```yaml
governance:
  timeout_ms: 3000
  retry:
    max_attempts: 2
    backoff_ms: 50
    max_backoff_ms: 500
    retry_budget: true
  rate_limit:
    qps: 1000
    burst: 2000
  breaker:
    failure_threshold: 0.5
    min_requests: 100
    cool_down_ms: 30000
  shedding:
    max_concurrency: 1000
    max_avg_latency_ms: 500
  fallback:
    enabled: true
```

同一份模型应该被 HTTP server、RPC client/server、Gateway route、MQ consumer、Job runner 解释。各协议只负责“怎么执行”，不重新定义字段。

当前差距：

- Gateway 已经接入部分统一字段。
- HTTP/RPC 已有治理能力，但字段、指标和 route/method/consumer 的标签口径还需要完全统一。
- MQ/Job 的治理语义需要明确：哪些错误可 retry，哪些不可 retry，取消和 deadline 如何传播。

### 3. 配置热更新

目标形态：

- 新配置解析失败不替换旧配置。
- section 级签名决定是否局部重建。
- listener 失败不阻断其它 listener。
- 每次变更有 version、hash、diff、source、operator、time。
- 支持灰度、签名校验、回滚和审计。

Roze 现状：

- Etcd watch、Env/File fallback、diff/version、section event、失败回滚已有基础。
- 仍需补 listener timeout/failure isolation、operator/audit 模型和灰度发布。

建议边界：

- `roze-config` 只负责安全地发布变更事件。
- 具体 subsystem 如 Kafka/Redis/DB/Gateway 自己决定是否重建。
- 生成服务应默认注册 reload listener，但业务可选择只监听某些 section。

### 4. 生命周期和健康

目标形态：

- 统一 bootstrap 启动 HTTP/RPC/MQ/Job/background tasks。
- 统一 SIGINT/SIGTERM 处理。
- shutdown 分阶段：停止接流量、drain、取消后台任务、关闭连接池。
- readiness 能检查 DB、Redis、MQ、RPC 下游、配置加载、后台任务。

最低交付：

- `/healthz`：进程活着即可。
- `/readyz`：依赖和后台任务 ready 才通过。
- `/startupz`：慢启动期间可单独表达启动状态。
- `/metrics`：Prometheus 文本输出。

当前状态和差距：

- probe report 有基础。
- REST 生成服务已有 `/healthz`、`/readyz`、`/startupz`、`/metrics` 默认入口。
- readiness/startup 当前是进程级 OK，依赖检查还需要接入 DB、Redis、MQ、RPC 下游、配置加载和后台任务状态。
- Gateway/RPC 的标准接口、依赖检查、K8s probe 模板还需要统一。

### 5. 可靠事件

Roze 应明确承诺：

- 默认语义是 at-least-once。
- 消费端必须幂等。
- 失败可 retry，超过上限进入 DLQ。
- Outbox/Inbox 是 DB 状态和消息一致性的推荐路径。
- 不承诺 exactly-once。

推荐事件 envelope：

```json
{
  "event_id": "uuid",
  "event_type": "order.created",
  "schema_version": "1",
  "tenant_id": "t1",
  "actor": "user-123",
  "trace_id": "trace-xxx",
  "idempotency_key": "order-123-created",
  "occurred_at": "2026-06-23T10:00:00Z",
  "data": {}
}
```

Roze 现状已经有 EventEnvelope/KafkaRecord/MQ Message，但需要收敛字段命名和跨 adapter 的 metadata 规范。

### 6. AI 友好

AI 友好的重点不是“给 AI 写提示词”，而是让工程边界机器可读。

建议生成：

- `SERVICE.md`：职责、不负责、入口、依赖、运行命令。
- `AI_CONTEXT.md`：允许修改/禁止修改路径、生成器命令、测试命令、契约变更流程。
- `ARCHITECTURE.md`：HTTP/RPC/MQ/DB/Redis/Config/Gateway 关系。
- `DEPENDENCIES.md`：服务依赖、topic、database、cache key、外部 API。

这些文件应从 `.api`、config schema、生成器模板和可选注释里生成，业务团队再补充领域语义。

## 需求收敛后的 P0/P1/P2

### P0：让框架可信

P0 不应该继续扩模块，而应该补“可信闭环”。

1. 锁定文档和 CLI 行为一致性：README/成熟度矩阵承诺 Toasty 是默认 ORM，CLI 默认值和测试必须持续覆盖这一契约。
2. 补 generated project compile tests：REST、RPC、model、OpenAPI、SDK、Docker/Kube 最小项目都要能生成后编译或被工具消费。
3. 完善健康接口：REST 生成服务已默认暴露 `/healthz`、`/readyz`、`/startupz`、`/metrics`；下一步补 Gateway/RPC 和依赖 readiness。
4. 收口 lifecycle：统一 SIGINT/SIGTERM、shutdown timeout、background task manager、HTTP/RPC/MQ/Job 关闭顺序。
5. 补 Gateway/Config/MQ smoke tests：rewrite、timeout、auth、rate、breaker、retry、fallback、hot reload、DLQ replay。
6. 完善 `rozectl doctor`：当前已检查 Rust/Cargo、Docker、kubectl、额外工具、端口、配置文件和 TCP 依赖地址；下一步补数据库、Redis、Kafka/NATS、etcd/Consul 协议级 probe。
7. 完善 `rozectl diff`：当前已能预览文件级变化；下一步补语义 diff 和更细的 ownership-aware report。

### P1：让框架可上生产

1. 统一治理 schema 横跨 HTTP/RPC/Gateway/MQ/Job：timeout、retry、rate limit、breaker、shedding、bulkhead、fallback、deadline/cancel propagation。
2. 安全模型进入契约层：`.api` 支持 jwt、permission、tenant、audit、idempotent、rate_limit、body_limit、timeout，并投影到 OpenAPI、middleware、测试骨架和 SDK。
3. 配置中心补生产语义：签名、灰度、审计、操作者、回滚命令、listener timeout/failure isolation。
4. 可观测资产交付：已提供 Gateway Grafana dashboard、Prometheus scrape/recording/alert rules 和 SLO 模板；继续补 trace 示例、日志查询示例。
5. MQ 可靠事件标准化：统一 envelope、schema version、idempotency key、outbox/inbox、DLQ 查看/重投/丢弃、consumer lag 指标。
6. 生产部署模板：Helm Chart、HPA、PDB、ServiceAccount、NetworkPolicy、ConfigMap/Secret、Gateway API/Ingress。

### P2：让框架成为团队和 AI 的默认协作底座

1. `rozectl mock gen`、`rozectl test gen`、`rozectl stream gen`、`rozectl dev`、`rozectl bench`。
2. `SERVICE.md` 已支持从 `.api` 生成；继续自动生成 `AI_CONTEXT.md`、`ARCHITECTURE.md`、`DEPENDENCIES.md`。
3. Admin API/UI：registry instances、config reload history、DLQ snapshots/replay/purge、breaker/rate limiter 状态。
4. 高级流量治理：A/B testing、traffic mirror、blue/green、header/cookie/tenant/user routing。
5. 完整示例：REST CRUD、REST+RPC+DB+Redis、Gateway+Registry+MQ+Outbox+DTM。

## P0 里程碑拆解

### M1：生成器可信

交付物：

1. `rozectl diff`：对 API/RPC/model 生成结果做文件级预览。（已具备 MVP）
2. generated project compile tests：REST、RPC、model、OpenAPI、SDK 主路径。
3. ownership preservation tests：确认 `src/logic/**`、自定义 middleware、`config.yaml` 不被 `--update` 覆盖。
4. `rozectl contract check`：对 `.api` 前后版本做 breaking change 检查。（已具备 MVP）

验收：

- 修改 `.api` 后可以先查看 diff，再执行 update。
- 编译测试覆盖最小 REST/RPC 项目。
- 生成器文档能明确列出框架拥有和业务拥有路径。

### M2：运行时可信

交付物：

1. 统一 `/healthz`、`/readyz`、`/startupz`、`/metrics`。（REST 生成服务已具备默认入口）
2. 统一 lifecycle runtime：shutdown signal、drain、background task cancellation。
3. Gateway smoke tests：rewrite、timeout、auth、rate、breaker、retry、fallback、hot reload。
4. Config center smoke tests：invalid update rollback、section event、listener failure isolation。

验收：

- REST 生成服务和 `rozectl kube` 已具备标准 probes；Gateway/RPC 待统一。
- SIGTERM 后服务先停止接新流量，再在 deadline 内退出。
- Gateway 热更新失败时继续使用旧配置。

### M3：本地生产形态可信

交付物：

1. `rozectl doctor` 最小版。（已具备本机工具、端口、配置文件和 TCP live probe）
2. `rozectl dev` 或 `docker-compose.integration.yml` 的一键说明。
3. 一套 `Gateway + Registry + MQ + DB + Redis` 示例。
4. Prometheus scrape config 和最小 Grafana dashboard。

验收：

- 新用户可以在本地跑起完整示例。
- doctor 能提示缺失工具、端口冲突和配置缺项。
- 示例能展示 trace id 贯穿 HTTP/RPC/MQ。

## 架构分层建议

Roze 当前 crate 拆分基本合理，但需要把“统一模型”放到更清晰的位置。

```text
Contract Layer
  .api / proto / OpenAPI / generated SDK / generated tests

Runtime Boundary
  HTTP / RPC / Gateway / MQ / Job
  Context / Error / Response / Validation

Governance Layer
  timeout / retry / rate limit / breaker / shedding / fallback
  deadline / cancellation / bulkhead / retry budget

Infrastructure Layer
  registry / config center / metrics / tracing / logging / health
  db / redis / mq / storage / dtm

Operations Layer
  docker / kube / helm / doctor / dev / dashboards / alerts

AI Collaboration Layer
  generated ownership rules / SERVICE.md / AI_CONTEXT.md / diff/update
```

关键原则：HTTP、RPC、Gateway、MQ、Job 不应该各自拥有一套治理/观测/配置模型。它们应只做协议适配，统一调用 `roze-config`、`roze-context`、`roze-error`、`roze-metrics`、`roze-opentelemetry` 和治理组件。

## 生成器所有权边界

这个边界已经是 Roze 的优势，应该继续强化。

| 文件/目录 | 所有者 | 生成策略 |
| --- | --- | --- |
| `src/route/**` | 框架 | `--update` 刷新 |
| `src/handler/**` | 框架 | `--update` 刷新 |
| `src/server/**` / `src/client/**` | 框架 | `--update` 刷新 |
| `src/types/**` | 契约 | 从 `.api`/proto 刷新 |
| `src/openapi/**` | 契约 | 从 `.api` 刷新 |
| `src/logic/**` | 业务 | `--update` 保留 |
| `src/middleware/<custom>.rs` | 业务 | `--update` 保留 |
| `config.yaml` | 部署/应用 | `--update` 保留 |
| Docker/K8s/Helm | 运维模板 | 应支持 diff/update |

## 推荐目录和模块归属

| 能力 | 推荐归属 | 原因 |
| --- | --- | --- |
| Context/header propagation | `roze-context` | 协议无关，HTTP/RPC/MQ/Gateway 都复用。 |
| Error model/i18n | `roze-error`, `roze-result` | 避免 handler 或 Gateway 手写不同错误结构。 |
| Governance config schema | `roze-config` 或独立 `roze-governance` | 配置结构应协议无关，runtime adapter 只解释执行。 |
| HTTP runtime middleware | `roze-middleware`, `roze-http` | Tower/Axum 细节留在 HTTP 层。 |
| RPC retry/deadline/metadata | `roze-rpc`, `roze-grpc` | tonic/prost 细节留在 RPC 层。 |
| Gateway policy execution | `roze-gateway` | Gateway 只消费统一治理和注册发现，不自定义平行模型。 |
| MQ envelope/retry/DLQ | `roze-mq` | Kafka/NATS/RabbitMQ adapter 共享语义。 |
| Kafka adapter | `roze-kafka` | 只放 Kafka 具体实现和 rdkafka 适配。 |
| Config center source/watch | `roze-config` | Etcd/Env/File source 和 reload event 属于配置层。 |
| Lifecycle/runtime | `roze-bootstrap`, `roze-shutdown`, `roze-health` | 服务启动、健康和关闭应统一。 |
| Generated templates | `apps/rozectl` | 生成器只消费公共 crate，不复制运行时逻辑。 |

## 已落地能力和增强方向

按收益和风险排序记录当前能力边界：

1. 防止 ORM 默认值回归：Toasty 保持默认，`--orm sea-orm` 才切换到 SeaORM，并用 CLI 解析测试覆盖。
2. `rozectl doctor`：Rust/Cargo、Docker、kubectl、额外工具、端口、配置文件和 TCP live probe 已具备；协议级依赖检查属于增强方向。
3. `rozectl diff`：文件级预览已具备；语义 diff、breaking change report 和更详细的 ownership 说明属于增强方向。
4. 健康接口模板：REST 已输出 `/healthz`、`/readyz`、`/startupz`、`/metrics`；RPC/Gateway 和依赖 readiness 标准化属于增强方向。
5. 给 Gateway 增加 app 级 smoke script：一键起 mock upstream + gateway，覆盖 rewrite/auth/rate/breaker/retry/fallback。
6. 给 MQ 增加真实 Kafka/NATS integration profile：默认跳过，显式 env 开启。
7. 生成 `SERVICE.md`：已支持从 `.api` 服务名、REST/RPC 接口和所有权边界生成；依赖配置、MQ topic、DB/Redis 配置推导属于增强方向。

## 建议新增 CLI 命令契约

| 命令 | P 阶段 | 最小行为 |
| --- | --- | --- |
| `rozectl diff` | P0 | 已支持生成到临时目录，对比目标目录，显示新增/修改/删除文件；默认不写盘。 |
| `rozectl doctor` | P0 | 已支持检查 Rust/Cargo、Docker、kubectl、额外工具、端口、配置文件和 TCP 依赖地址；协议级依赖检查属于增强方向。 |
| `rozectl model generate` | P0 | 已处理：从 DSL/SQL/Mongo schema 生成 `src/model`，默认 Toasty，支持 `--orm sea-orm`，支持 `--update`/`--force`。 |
| `rozectl model inspect` | P0 | 已处理：支持 sqlite、Postgres、MySQL、MongoDB schema/collection inspect，并生成同一模型 scaffold。 |
| `rozectl search generate` | P0 | 已处理：支持 Elasticsearch、OpenSearch、Meilisearch，从 `.search` DSL 或 JSON schema 生成 `src/search`。 |
| `rozectl search inspect` | P0 | 已处理：Elasticsearch/OpenSearch 读取 mapping，Meilisearch 读取 settings/index metadata 并采样 documents。 |
| `rozectl dev` | P1 | 启动本地依赖或提示 docker compose 命令；输出服务端口和健康检查地址。 |
| `rozectl contract check` | P0 | 已支持 `.api` 前后版本 breaking change 检查，覆盖 route/RPC/type/field 基础破坏性变更。 |
| `rozectl test gen` | P1 | 从 `.api` 生成 HTTP/RPC smoke tests 和最小断言。 |
| `rozectl mock gen` | P1 | 已支持从 `.api` 生成独立 Axum mock server，按 response type 返回默认 JSON。 |
| `rozectl doc service` | P1 | 已支持从 `.api` 生成 `SERVICE.md`，包含接口清单、生成器所有权边界、常用命令和 AI editing notes。 |
| `rozectl stream gen` | P2 | 从事件契约生成 producer/consumer skeleton、DLQ 配置和 envelope 类型。 |
| `rozectl bench` | P2 | 生成或运行基础压测脚本，输出延迟、错误率和吞吐。 |

`diff` 和 `doctor` 的 MVP 已落地。下一步重点是让它们更懂 Roze 项目语义：`diff` 补 breaking change report，`doctor` 补协议级依赖检查和可执行修复建议。

## 风险和决策记录

| 决策/风险 | 当前结论 | 后续动作 |
| --- | --- | --- |
| 默认 ORM | Toasty 是默认，`--orm sea-orm` 切换 SeaORM。 | CLI 解析测试和文档持续覆盖，避免默认值漂移。 |
| exactly-once 消息语义 | 不承诺 exactly-once。 | 文档明确 at-least-once + idempotency + outbox/inbox + DLQ + replay。 |
| Gateway 与 service mesh 边界 | Roze Gateway 负责应用入口治理；不试图替代完整 service mesh。 | 高级 mTLS、sidecar 流量治理可作为集成方向，不进入 P0。 |
| 业务 SQL 和事务 | 不隐藏复杂业务 SQL；事务边界属于 application logic。 | 生成 repository scaffold，但完整业务事务通过示例和文档表达。 |
| 安全模型 | 认证授权 primitives 已有，但不能宣称完整安全平台。 | P1 统一 `.api` 注解、OpenAPI security、permission test templates。 |

## 验收标准

每个能力从 `beta`/`scaffold` 走向 `stable` 前至少满足：

1. 文档有运行契约和失败语义。
2. 单元测试覆盖核心策略。
3. 有端到端或 smoke test。
4. 有 metrics/log/trace 字段说明。
5. 有生成器重复运行和 ownership preservation 测试。
6. 有升级说明，破坏性生成变更能被用户预判。
7. 有一个可以本地跑的生产形态示例。

## 下一步建议

最小可开工顺序：

1. `rozectl diff`。
2. generated project compile tests。
3. Gateway/RPC probes、依赖 readiness 和 lifecycle runtime。
4. Gateway/Config/MQ smoke tests。
5. 完善 `rozectl doctor`。
6. `SERVICE.md`/`AI_CONTEXT.md` 生成。

这条路径优先补“信任”和“可重复执行”，不会打乱现有 crate 结构，也能让后续 P1 的治理、安全、观测工作有稳定落点。
