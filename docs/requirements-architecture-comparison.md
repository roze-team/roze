# 微服务框架需求与 Roze 当前架构对比

本文把“最核心�?12 类能力”整理成可执行的产品/架构需求，并对�?Roze 当前仓库里的实现状态。状态判断以现有 README、成熟度矩阵、契约文档和 `rozectl` 命令结构为准�?

## 总体判断

Roze 的方向与需求高度一致：已经采用 IDL first、生成器维护边界代码、业务代码落�?`logic`、HTTP/RPC/Gateway/MQ/Config/Registry/Observability 拆为独立 crate 的架构�?

当前主要问题不是“没有模块”，而是“模块成熟度和生产闭环不足”。多数核心模块处�?`beta`，生命周期、分布式事务、部署生成仍�?`scaffold`。下一阶段应该优先把已有能力做成可验证、可升级、可观测、可恢复的生产框架，而不是继续横向铺�?crate�?

## 本轮处理标记

处理日期�?026-06-24

以下需求已完成实现、文档同步和本地验证�?

- 已处理：移除兼容入口口径，CLI 和文档只保留 Roze 原生命令�?
- 已处理：`rozectl model generate <schema> --out <dir>` 原生模型生成�?
- 已处理：`rozectl model inspect <table> --db-kind <sqlite|postgres|mysql|mongo> --db-url <url> --out <dir>`，覆�?sqlite、Postgres、MySQL、MongoDB�?
- 已处理：`rozectl search generate <schema> --engine <elasticsearch|opensearch|meilisearch> --out <dir>`�?
- 已处理：`rozectl search inspect <index> --engine <elasticsearch|opensearch|meilisearch> --url <url> --out <dir>`�?
- 已处理：`roze-search` 运行�?health、index document、delete document、search 调用封装�?
- 已实现：`rozectl diff` 文件级生成预览，覆盖 API、RPC、model�?
- 已实现：`rozectl contract check` `.api` 破坏性变更检查�?
- 已实现：`rozectl mock gen` �?`.api` 生成独立 Roze native HTTP mock server�?
- 已实现：`rozectl test gen` �?`.api` 生成 HTTP smoke contract tests�?
- 已实现：`rozectl dev up/down/status` 通过 docker compose 管理本地依赖�?
- 已实现：`rozectl doctor` 检�?Rust/Cargo、Docker、kubectl、额外工具、端口、配置文件和 TCP 依赖地址�?
- 已实现：`rozectl doc service` �?`rozectl doc ai-context`�?
- 已实现：`rozectl docker`、`rozectl kube deploy/validate`、`rozectl helm chart/validate`�?
- 已实现：`roze-local-cache` 基于 Moka 的本地进程缓存，支持 TTL、容量淘汰、time-to-idle、命�?未命中统计�?
- 已实现：`roze-rpc::MemoryRegistry` 使用 DashMap 优化并发注册和发现路径�?
- 已实现：HTTP route rate-limit/breaker �?RPC method rate-limit/breaker 状态使�?DashMap，降低治理热路径全局锁竞争�?
- 已实现：`roze-metrics::MetricRegistry` 使用 DashMap 存储�?label 的指标状态，降低请求/队列指标记录的全局锁竞争�?
- 已实现：`roze-session`、`roze-ws`、`roze-eventbus`、`roze-mq` 内存态高频索引使�?DashMap/DashSet；MQ DLQ 顺序队列保留显式锁�?
- 已实现：Criterion 性能基线，覆�?`roze-metrics` registry、`roze-local-cache` async cache、`roze-singleflight` request coalescing、`roze-rpc` memory registry、session/WebSocket/eventbus/MQ 内存态热路径�?
- 已实现：generated REST/RPC project compile smoke ignored tests，覆�?REST + model + search 组合项目�?RPC 生成项目�?
- 已实现：真实依赖 integration compose 覆盖 Kafka、NATS、Redis、Postgres、MySQL、Elasticsearch、OpenSearch、Meilisearch、Etcd、Consul�?
- 已实现：`apps/roze-example` 项目�?`external_verify` 使用 Roze 运行时组件真实连�?Postgres、MySQL、Redis、Kafka、NATS、MongoDB、Elasticsearch、OpenSearch、Meilisearch、Etcd、Consul，并�?`scripts/roze-project-external-smoke.sh` 一键启动、验证、清理�?
- 已实现：`scripts/production-smoke.sh` 作为 production example/smoke 入口，串起格式化、生成器、generated compile smoke、核�?crate �?app check�?
- 已实现：`scripts/rozectl-smoke.sh` 覆盖 `rozectl` 命令面，包含 API/RPC/model/search/template/diff/contract/mock/test/doc/openapi/docker/kube/helm/doctor/dev�?
- 已处理：README、usage 文档、项目规范、成熟度矩阵和本文件已同步搜�?模型能力�?

## 覆盖度口�?

本文里的覆盖度不是按“有没有 crate”判断，而是按可交付程度判断�?

| 覆盖�?| 判断标准 |
| --- | --- |
| �?| 已有生成器或运行时主路径，用户可以按文档完成常规使用；仍可能缺少边界测试或高级能力�?|
| 中高 | 核心 runtime 已具备，但生产样例、故障测试、运维资产或跨模块一致性还不完整�?|
| �?| 有基础 API/抽象和部分实现，但还需要统一模型、生成器接入或真实依赖验证�?|
| 中低 | 有雏形或 report/helper，但尚未成为生成服务默认能力，也缺少端到端验收�?|
| �?| 只有方向或少量基础代码，不能作为框架能力对外承诺�?|

生产稳定度另�?`docs/maturity.md` �?`stable/beta/scaffold/planned` 标注判断。一个能力即使覆盖度是“中高”，只要缺少真实依赖测试、升级说明和生产示例，也不应该标�?`stable`�?

## 12 类能力对比矩�?

| # | 需求能�?| Roze 当前覆盖 | 主要入口 | 缺口/风险 |
| --- | --- | --- | --- | --- |
| 1 | API/RPC 契约优先 | �?| `apps/rozectl`, `.api`, proto, REST/RPC generator, OpenAPI, TS/JS SDK | 已实�?REST/RPC/OpenAPI/TS/JS SDK、`rozectl contract check`、`rozectl mock gen`、`rozectl test gen`；增强方向是 OpenAPI validator 完整投影、SDK error/interceptor/retry/timeout�?|
| 2 | Gateway 网关 | 中高 | `crates/roze-gateway`, `apps/roze-gateway`, `docs/contracts/gateway.md` | 已有路由、rewrite、auth、rate/breaker、retry、fallback、registry upstream、canary、health/outlier、hot reload、WebSocket upgrade 代理、SSE 流式代理；缺更完�?app 级示例、deploy smoke test、A/B、流量镜像�?|
| 3 | 服务注册与发�?| 中高 | `roze-rpc::registry`, cached resolver, etcd/consul/dns/memory | memory registry、DNS、etcd、Consul、watch、cached resolver 已具备；memory registry 已使�?DashMap 优化并发注册/发现路径；etcd/Consul 注册、发现、注销已有真实服务 ignored integration test，并纳入 `roze-example external_verify` 项目级验收；增强方向�?watch 断线/续约失败故障注入、Gateway/RPC/Job/MQ consumer 复用边界文档化�?|
| 4 | 统一治理模型 | �?| `roze-config`, `roze-middleware`, `roze-rpc`, `roze-gateway` | Gateway 已继�?timeout/retry/rate/breaker；HTTP route �?RPC method �?rate-limit/breaker 状态已使用 DashMap 优化热路径并发；MQ/Job 还需要同一 schema、同一指标标签、可选持久化 breaker/rate limiter�?|
| 5 | 配置中心 | 中高 | `crates/roze-config`, `docs/contracts/config-center.md` | Etcd watch、Env/File fallback、diff/version、section event、失败回滚已具备；仍需 listener timeout/failure isolation、灰度、签名、审计操作者模型�?|
| 6 | 可观测�?| �?| `roze-log`, `roze-metrics`, `roze-prometheus`, `roze-opentelemetry`, `deploy/observability` | tracing/metrics/Prometheus/OTel 已有；带 label �?metric registry 已使�?DashMap 优化并发记录，并�?Criterion 基准覆盖写入�?render；已提供 Gateway Prometheus scrape 示例、recording rules、alert rules、Grafana dashboard �?SLO 模板；增强方向是 trace 示例、日志查询示例和更完�?dashboard pack�?|
| 7 | 健康检查和生命周期 | 中高 | `roze-health`, `roze-bootstrap`, `roze-shutdown`, REST templates | probe report、`HealthRegistry`、startup/ready/draining phase、REST 标准 `/healthz`、`/readyz`、`/startupz`、`/metrics` 入口已有；生�?REST/RPC 项目会把已连接依赖注册进 readiness；`production-smoke.sh` 已纳�?health/bootstrap/shutdown crate 验收；Gateway/RPC 标准入口、协议级依赖 ping、HTTP/RPC/MQ/Job 统一启动关闭顺序还没收口�?|
| 8 | 安全能力 | �?| `roze-jwt`, `roze-auth`, `roze-permission`, Gateway auth | JWT/RBAC/tenant/ABAC primitives 已有；缺统一安全模型、OIDC/OAuth2、mTLS、permission 注解�?OpenAPI/SDK/test、key rotation、审计日志模板�?|
| 9 | MQ/EventBus/可靠事件 | 中高 | `roze-mq`, `roze-kafka`, `roze-nats`, `roze-eventbus`, outbox/inbox primitives | publish/subscribe、retry、DLQ、stats、NATS JetStream、Kafka ack/nack 已有；in-memory topic/offset/idempotency/eventbus topic 索引已使�?DashMap/DashSet 并有 Criterion 基线；`roze-example external_verify` 已用真实 Kafka/NATS 发布消息；增强方向是生产 replay/purge 示例、标准事�?envelope 完整化�?|
| 10 | 数据库、缓存、事务、搜�?| 中高 | `roze-db`, `roze-orm`, `roze-sqlx`, `roze-cache`, `roze-local-cache`, `roze-redis`, `roze-singleflight`, `roze-transaction`, `roze-dtm`, `roze-search`, `rozectl model`, `rozectl search` | SQL/Mongo model generate/inspect、Redis cache-aside、Moka 本地缓存、singleflight、TCC/Saga/outbox/inbox primitives、Elasticsearch/OpenSearch/Meilisearch search generate/inspect 已有；`roze-example external_verify` 已用 Roze 组件真实连接 Postgres/MySQL/Mongo/Redis/search 并执行最小读写；增强方向�?DB transaction + outbox + MQ + RPC 完整业务示例和边界测试�?|
| 11 | 部署和运�?| 中高 | `rozectl docker`, `rozectl kube`, `rozectl helm`, `rozectl dev`, `rozectl doctor`, `docker-compose.integration.yml`, `docker/docker-compose.yml`, `scripts/production-smoke.sh`, `scripts/roze-project-external-smoke.sh` | 已实�?Dockerfile、Kubernetes、Helm chart 生成�?validate，本�?doctor、docker compose dev up/down/status、真实依�?compose、production smoke �?Roze 项目级外部依�?smoke；增强方向是平台级发布集成�?|
| 12 | CLI/生成�?AI 友好 | 中高 | `rozectl api/rpc/model/search/openapi/client/docker/kube/helm/diff/doctor/dev/doc`, 稳定生成目录 | 已实现文件级 `diff` 预览、本�?`doctor` 检查、`SERVICE.md`/`AI_CONTEXT.md` 生成、SQL/Mongo model inspect �?Elasticsearch/OpenSearch/Meilisearch search inspect；增强方向是 `stream gen`、`ARCHITECTURE.md`/`DEPENDENCIES.md` 自动生成�?|

## 功能规格清单

这一节把 12 类能力拆成可实现功能。`MVP` 表示进入 P0/P1 前必须有的最小功能，`增强项` 表示生产完善或高级治理能力�?

### 1. API/RPC 契约优先

| 功能 | MVP | 增强�?| 推荐落点 | 验收 |
| --- | --- | --- | --- | --- |
| `.api` REST 生成 | route/handler/logic/types/config/openapi/svc 固定结构 | 更完�?Roze `.api` 语法、注释、import、validator tag | `apps/rozectl` | 生成项目可编译，`--update` 不覆�?`logic`�?|
| proto/RPC 生成 | server/client/pb/logic/config/svc 固定结构 | streaming、metadata 策略、proto fixture 覆盖 | `apps/rozectl`, `roze-rpc` | RPC 生成项目可编译，Context metadata 可透传�?|
| OpenAPI 生成 | paths、schemas、parameters、request/response、security | validator 约束完整投影、examples、error schema | `apps/rozectl`, `roze-openapi` | Swagger UI/主流 client generator 可消费�?|
| SDK 生成 | TS/JS baseUrl、headers、path/query/body | typed error、interceptor、retry、timeout、auth injection | `apps/rozectl/src/generator/client.rs` | SDK 能调�?mock server 并处理错误响应�?|
| mock 生成 | 已实现：�?`.api` 生成独立 Roze native HTTP mock server，并�?response type 返回默认 JSON | 示例数据、延�?错误注入、OpenAPI example 驱动 | `rozectl mock gen` | 本地无需业务实现即可返回契约响应�?|
| 接口测试生成 | 已实现：�?`.api` 生成 HTTP smoke contract tests，支�?base URL 配置 | 契约回归、鉴权场景、错误码场景、RPC smoke tests | `rozectl test gen` | 生成测试能在空逻辑�?mock 上运行�?|
| 生成预览 | 已实现：文件�?`A/M/D` diff，覆�?API、RPC、model 生成结果 | ownership-aware diff、语�?diff | `rozectl diff` | 默认不写盘，输出清晰�?|
| 契约破坏性变更检�?| 已实现：检�?route/method/path 删除或变更、RPC 方法删除、request/response 类型变更、字段删除、字段类�?source 变更、必填字段新�?| semver 建议、SDK breaking report、响应字段变更策略细�?| `rozectl contract check` | breaking changes 明确失败并给出原因�?|

### 2. Gateway 网关

| 功能 | MVP | 增强�?| 推荐落点 | 验收 |
| --- | --- | --- | --- | --- |
| 路由转发 | prefix/method 匹配、path rewrite、query/header/body 透传 | host/path/header 组合匹配 | `roze-gateway` | mock upstream 返回可原样透传�?|
| 鉴权 | JWT、API Key、CORS、body/header limit | OIDC、mTLS、请求签名、防重放 | `roze-gateway`, `roze-auth` | 未授权返回统一 401/403，成功注�?auth context header�?|
| 治理 | timeout、rate limit、breaker、retry、fallback | retry budget、adaptive shedding、bulkhead | `roze-gateway`, governance schema | 每项治理�?smoke test �?metrics�?|
| 灰度路由 | weight、instance_tags、registry upstream | header/cookie/tenant/user 分流、A/B、流量镜�?| `roze-gateway` | 相同路由可按权重或标签命中不同上游�?|
| 协议形�?| HTTP request/response、WebSocket upgrade 双向转发、SSE `text/event-stream` 流式转发、流式响应空闲超�?`stream_idle_timeout_ms`、长连接上限 `max_stream_connections`、连接级 opened/closed/rejected/duration/active 指标、连接级 dashboard/alert | gRPC-Web、更多连接级 runbook | `roze-gateway`, `deploy/observability` | HTTP、WS、SSE 可共用同一�?route/service/governance 配置，SSE 首个事件不等待完整响应结束，空闲流和活跃连接数可�?route/service/gateway 配置治理，并可按 route/protocol 观测连接生命周期�?|
| 上游健康 | 主动 health check、outlier ejection | 异常实例自动摘除、慢实例降权 | `roze-gateway` | 失败实例不参与路由，恢复后重新加入�?|
| 热更�?| 配置中心 reload 后重建路�?| 灰度配置、回滚、签名校�?| `apps/roze-gateway`, `roze-config` | invalid config 不替换旧路由�?|
| 观测 | request_id/trace_id、upstream metrics、retry metrics | dashboard、alert、route SLO | `roze-metrics`, `roze-prometheus` | 可按 route/upstream 查看延迟、错误、重试、熔断�?|

### 3. 服务注册与发�?

| 功能 | MVP | 增强�?| 推荐落点 | 验收 |
| --- | --- | --- | --- | --- |
| 静�?upstream | 已实现：支持固定 URL/address | �?upstream 加权 | `roze-rpc::registry`, `roze-gateway` | 无注册中心也能运行�?|
| DNS discovery | 解析 DNS/service name | TTL cache、失败回退 | `roze-rpc::registry` | DNS 变更�?resolver 可刷新�?|
| etcd/Consul | 注册、续约、下线、watch、discover | lease 续约失败处理、watch 断线恢复 | `roze-rpc::registry` | 实例上下线能�?Gateway/RPC client 感知�?|
| Kubernetes discovery | 支持 Service DNS �?API discovery | namespace/label selector | registry adapter | K8s 内无需手写 IP�?|
| 本地缓存 | 已实现：Cached resolver、周�?refresh、memory registry 并发 map | watch 优先 + refresh 兜底 | `CachedRegistryResolver`, `MemoryRegistry` | 注册中心短暂不可用时继续使用旧快照�?|
| 路由策略 | round_robin、weight、tag filter | latency-aware、zone-aware | registry/gateway shared policy | 标签不匹配时不会误回退到错误实例�?|

### 4. 统一治理模型

| 功能 | MVP | 增强�?| 推荐落点 | 验收 |
| --- | --- | --- | --- | --- |
| timeout/deadline | 全局、route/method、consumer �?timeout | deadline �?HTTP/RPC/MQ 传播 | `roze-config`, `roze-context` | 下游收到剩余 deadline�?|
| retry | max attempts、backoff、retryable error | retry budget、jitter、风暴保�?| shared governance + protocol adapters | 不可重试错误不重试，可重试错误有指标�?|
| rate limit | 已实现：HTTP route/RPC method local token bucket；状态使�?DashMap 降低全局锁竞�?| distributed limiter、租�?用户维度 | `roze-middleware`, `roze-rpc`, `roze-gateway` | 超限返回统一错误并记�?rejected metric�?|
| circuit breaker | 已实现：HTTP route/RPC method failure threshold、cool down；状态使�?DashMap 降低全局锁竞�?| half-open、持久化状态、按实例熔断 | `roze-middleware`, `roze-rpc`, `roze-gateway` | 熔断打开后快速失败，恢复窗口可探测�?|
| shedding/bulkhead | max concurrency、latency/failure shedding | adaptive policy、pool 隔离 | `roze-middleware` | 高负载下主动拒绝而不是拖垮服务�?|
| fallback | route/method fallback | typed fallback、cache fallback | gateway/runtime adapters | fallback 和上游透传响应边界清晰�?|
| cancel propagation | request cancel 传播 | MQ/Job cancel cooperative hooks | `roze-context`, `roze-shutdown` | 客户端断开�?shutdown 能取消下游工作�?|

### 5. 配置中心

| 功能 | MVP | 增强�?| 推荐落点 | 验收 |
| --- | --- | --- | --- | --- |
| 配置�?| File、Env、Etcd | Consul/Nacos、K8s ConfigMap watch | `roze-config` | 优先级和 fallback 明确�?|
| 热更�?| watch/poll、debounce、reload event | section listener、局部重�?helper | `roze-config` | Kafka 变更只重�?Kafka pipeline�?|
| 安全替换 | 解析失败保留旧配�?| listener timeout/failure isolation | `roze-config` | 一�?listener 失败不影响其�?listener�?|
| 审计 | version、hash、diff、source、time | operator、signature、approval、rollback | `roze-config`, `roze-admin` | 每次变更能查�?diff 和来源�?|
| 敏感配置 | 脱敏打印 | secret manager、加密、签�?| `roze-config` | 日志不泄�?secret�?|
| 灰度配置 | �?app/namespace key | tenant/user/instance 灰度 | config center/admin | 部分实例先应用配置，失败可回滚�?|

### 6. 可观测�?

| 功能 | MVP | 增强�?| 推荐落点 | 验收 |
| --- | --- | --- | --- | --- |
| 日志 | structured tracing、request_id、trace_id | 日志脱敏、采样、查询模�?| `roze-log`, `roze-trace` | 每条入口日志能关�?request/trace�?|
| Metrics | 已实现：HTTP/RPC/Gateway/MQ 基础指标；带 label �?registry 状态使�?DashMap 降低记录热路径锁竞争 | DB/Redis 指标、直方图 buckets、更�?SLO recording rules | `roze-metrics`, `roze-prometheus` | Prometheus scrape 后能�?p95/error rate�?|
| Tracing | OpenTelemetry trace context | baggage、sampling、Jaeger/OTLP 示例 | `roze-opentelemetry` | HTTP -> RPC -> MQ trace 可串联�?|
| 事件观测 | config reload、retry、breaker、rate limit、DLQ | audit event stream | metrics/log/admin | 治理事件有结构化字段�?|
| Dashboard | 已提�?Gateway 最�?Grafana dashboard | 服务、MQ、配置中�?dashboard pack | `deploy/observability` | 本地示例能导�?dashboard�?|
| Alert | 已提�?Gateway Prometheus alert rules �?recording rules | DLQ、breaker state、更�?burn-rate 窗口 | `deploy/observability` | 关键故障有默认告警规则�?|

### 7. 健康检查和生命周期

| 功能 | MVP | 增强�?| 推荐落点 | 验收 |
| --- | --- | --- | --- | --- |
| 标准 probes | 已实现：REST 生成服务已有 `/healthz`、`/readyz`、`/startupz`、`/metrics`，并返回结构�?`ProbeReport` | Gateway/RPC 模板统一、协议级依赖 ping | `roze-health`, templates | K8s probe 可直接使用�?|
| readiness | 已实现基础版：`HealthRegistry` 支持动态检查、startup/ready/draining phase，生�?`ServiceContext` 会注册已连接 DB/Mongo/Redis/NATS 依赖 | 权重 readiness、RPC/config/background tasks、协议级 ping | `roze-health` | 依赖不可用时实例不接流量�?|
| bootstrap | 统一启动 HTTP/RPC/MQ/Job | component dependency graph | `roze-bootstrap` | �?component 按顺序启动�?|
| shutdown | SIGINT/SIGTERM、deadline、graceful shutdown | drain mode、preStop hook、shutdown phases | `roze-shutdown` | 终止后不接新请求并在 deadline 内退出�?|
| background tasks | task manager、cancel token | restart policy、task health | `roze-bootstrap` | 后台任务失败会影�?readiness 或报警�?|

### 8. 安全能力

| 功能 | MVP | 增强�?| 推荐落点 | 验收 |
| --- | --- | --- | --- | --- |
| 认证 | JWT、API Key | OIDC/OAuth2、mTLS | `roze-auth`, `roze-jwt`, Gateway | 未认证和认证失败错误一致�?|
| 授权 | RBAC、tenant、ABAC primitives | object-level authorization、policy engine | `roze-permission` | 权限失败返回统一 403�?|
| 契约注解 | `.api` jwt/permission/tenant/audit | idempotent、rate_limit、body_limit、timeout | parser/generator | 注解能生�?middleware 绑定�?OpenAPI security�?|
| 审计 | 操作者、资源、动作、结�?| 审计日志 sink、查�?API | `roze-admin`, app templates | 敏感操作有审计事件�?|
| 输入和资源限�?| validation、body/header limit、rate limit | SSRF guard、field-level auth | middleware/gateway | OWASP API 常见风险有默认防线�?|
| Secret 处理 | 脱敏显示 | SecretString、rotation、external secret | `roze-config` | secret 不进入普通日志和错误�?|

### 9. MQ/EventBus/可靠事件

| 功能 | MVP | 增强�?| 推荐落点 | 验收 |
| --- | --- | --- | --- | --- |
| Event envelope | event_id/type/version/trace/idempotency/occurred_at | schema registry、breaking-change check | `roze-mq`, `roze-eventbus` | Kafka/NATS/in-memory metadata 一致�?|
| Producer | publish result、headers、trace carrier | transaction-aware publish | `roze-mq`, adapters | publish 返回 topic/partition/offset 或等�?metadata�?|
| Consumer | ack/nack/retry/DLQ | delayed retry、retry storm protection | `roze-mq`, `roze-kafka`, `roze-nats` | 失败超过上限进入 DLQ�?|
| Admin | DLQ list/replay/purge | UI、权限、审�?| `roze-admin`, adapters | 可重投指定死信并记录结果�?|
| Outbox/Inbox | outbox relay、inbox dedupe | DB transaction examples、cleanup policy | `roze-transaction`, `roze-dtm` | DB 状态和消息发布一致�?|
| Metrics | processed、failed、lag、offset、DLQ count | consumer SLO、partition dashboard | `roze-metrics` | topic/group/partition 维度可观测�?|

### 10. 数据库、缓存、事�?

| 功能 | MVP | 增强�?| 推荐落点 | 验收 |
| --- | --- | --- | --- | --- |
| Model 生成 | 已处理：Toasty 默认、SeaORM 可选，生成单表 CRUD、`count`、分页、等�?IN/范围条件、可空字段等�?IS NULL、排序、批量、软删、租户限定方法、独立字段文件、extension 保留文件、复合索引查询方法、Toasty/SeaORM 本地事务 helper；`model inspect` 覆盖 sqlite、Postgres、MySQL、MongoDB | schema namespace、关系、跨字段复杂条件、Unit of Work 示例、事务边界守�?| `apps/rozectl`, `roze-orm` | 默认生成 Toasty，`--orm sea-orm` 切换；字段元数据独立，`<model>_ext.rs` �?`--update` 保留，基础增删改查和增强查询可直接调用，Toasty/SeaORM 可显式开启本地事务边界�?|
| Search 生成 | 已处理：`rozectl search generate/inspect` 支持 Elasticsearch、OpenSearch、Meilisearch，生�?`src/search/mod.rs` �?`src/search/<index>.rs`，保留原始字段名，提�?health/index/delete/search repository | ranking/boosting query builder、搜索结果高亮、facets 聚合 helpers | `apps/rozectl`, `roze-search` | `.search` DSL、JSON schema 和已�?index inspect 都能生成同一 repository 形态；Elasticsearch/OpenSearch 读取 mapping，Meilisearch 读取 settings/index metadata 并采�?documents�?|
| DB 连接 | pool config、timeout | read/write split、多数据�?| `roze-db`, `roze-sqlx` | 连接失败影响 readiness�?|
| Migration | migration scaffold | rollback、dry-run、status | `roze-migration`, `rozectl migrate` | 本地示例能执�?migration�?|
| 事务上下�?| 本地事务 helper | Unit of Work、nested boundary guard | `roze-transaction` | 业务 logic 能显式控制事务�?|
| 缓存 | 已实现：Redis client、cache-aside、negative cache、TTL jitter、singleflight loading；本地缓存基�?Moka，支�?TTL、容量淘汰、time-to-idle 和命�?未命中统�?| read/write through、Bloom filter | `roze-cache`, `roze-redis`, `roze-local-cache` | 缓存击穿/雪崩有模板策略�?|
| singleflight/lock | 已实现：singleflight 防击穿，key lookup 使用 DashMap，Criterion 覆盖 unique-key/cached-key/reset 热路�?| distributed lock、fencing token | `roze-singleflight`, `roze-redis` | 并发 miss 只回源一次�?|
| 分布式事�?| TCC/Saga primitives | 状态查询、补偿任务、admin UI | `roze-dtm` | 示例覆盖 try/confirm/cancel �?saga compensation�?|

### 11. 部署和运�?

| 功能 | MVP | 增强�?| 推荐落点 | 验收 |
| --- | --- | --- | --- | --- |
| Dockerfile | 已实现：multi-stage build、port、timezone、non-root runtime、Dockerfile validation | distroless、SBOM | `rozectl docker` | 生成镜像可运�?healthz�?|
| Kubernetes YAML | 已实现：Deployment/Service/HPA、PDB、NetworkPolicy、ServiceAccount、resources、标�?probes、manifest validation | Gateway API/Ingress 平台集成 | `rozectl kube deploy/validate` | `rozectl kube validate` 可校验生�?YAML�?|
| Helm | 已实现：chart、values、Deployment/Service/HPA、PDB、NetworkPolicy、ServiceAccount、probes、chart validation | chart tests、values schema validation | `rozectl helm chart/validate` | helm chart 输出可部�?YAML�?|
| doctor | 已实现：本机工具、端口、配置、TCP 依赖地址 live probe | 权限、版本检查、协议级 probe | `rozectl doctor` | 缺依赖时给出明确修复建议�?|
| dev | 已实现：docker compose up/down/status、profile、detach、volumes | seed data、logs 聚合 | `rozectl dev` | 新用户一条命令启动本地依赖�?|
| 观测部署 | scrape config、dashboard、alerts | canary checks、runbook | `deploy/observability` | 示例环境可直接导入�?|

### 12. CLI/生成�?AI 友好

| 功能 | MVP | 增强�?| 推荐落点 | 验收 |
| --- | --- | --- | --- | --- |
| new/generate/update | api/rpc/model/search 主路�?| template customization、plugin hooks | `apps/rozectl` | 重复生成稳定、可预测�?|
| diff | 已实现：文件�?diff，覆�?API、RPC、model | ownership-aware diff、breaking change report | `rozectl diff` | 默认不写盘，输出清晰�?|
| doctor/dev | 已实现：`doctor` 本机工具、端口、配置和 TCP live probe；`dev` 通过 docker compose up/down/status 管理本地依赖 | 自动修复建议、协议级 probe、日志聚�?| `rozectl doctor`, `rozectl dev` | 本地 onboarding 时间可控�?|
| doc/sdk/test/mock | 已实现：文档、TS/JS      SDK、HTTP smoke tests、mock 生成 | 更多 SDK 语言、RPC contract tests | `apps/rozectl` | 从契约能生成可运行辅助资产�?|
| AI_CONTEXT | 已实现：`rozectl doc service` 生成 `SERVICE.md`；`rozectl doc ai-context` 生成 `AI_CONTEXT.md` | ARCHITECTURE/DEPENDENCIES、依赖图、topic/cache key 提取 | `rozectl doc service`, `rozectl doc ai-context` | AI 能知道哪些文件能改、哪些不能改�?|
| upgrade | update/diff/breaking-change check | migration guide automation | `rozectl update` | 框架升级能预览影响并保护业务代码�?|

## 当前架构资产盘点

### 已经形成优势的部�?

1. 生成器边界清晰：REST/RPC 生成项目已经区分框架拥有文件和业务拥有文件，`logic` 保留策略对人�?AI 都友好�?
2. crate 拆分方向正确：HTTP、RPC、Context、Error、Metrics、Config、Gateway、MQ、Registry、DTM、Storage 等能力已经不是散落在示例应用里�?
3. 生成器入口单一：CLI 只保�?Roze 原生命令，同时输�?Rust-native 项目结构�?
4. Gateway 和配置中心已经有运行契约文档，不只是 README 级描述�?
5. MQ/Kafka/NATS 已经开始明�?ack/nack/retry/DLQ/admin replay 等语义，方向比“简�?publish/subscribe”更接近生产�?

### 当前最容易误判的部�?

1. “已�?crate”不等于“生产稳定”：生命周期、部署、DTM、事务链路示例仍需要按 scaffold 看待�?
2. “生成器能生成”不等于“可安全升级”：文件�?`diff` �?REST/RPC generated compile smoke 已实现；语义 diff、更�?repeated generation tests 和更多边�?fixture 属于增强方向�?
3. “有 metrics/tracing crate”不等于“可观测闭环”：还缺 dashboard、alert、SLO、trace/log 查询示例和标签一致性验证�?
4. “有 JWT/RBAC primitives”不等于“安全模型完整”：还缺契约注解、OpenAPI 投影、测试骨架、key rotation、审计日志和对象级授权模板�?

## 关键能力拆解

### 1. 契约优先

目标形态：

- `.api` �?proto 是唯一接口事实来源�?
- route、handler、types、OpenAPI、SDK、测试骨架都从契约生成�?
- 业务只改 `logic`、自定义 middleware 和明确的 application module�?
- 重复生成默认�?`--update`，并能先 `diff` 再落盘�?

Roze 现状�?

- REST/RPC/OpenAPI/TS/JS SDK 主链路已具备�?
- `--update` 已保留业务逻辑和自定义 middleware�?
- 已实现文件级 `rozectl diff`、`rozectl mock gen`、`rozectl test gen` �?`rozectl contract check`；语�?diff 继续作为增强项�?

增强方向�?

- `rozectl diff` 已落地文件级 diff；语�?diff �?breaking change report 属于增强方向�?
- `rozectl test gen` 已生�?HTTP smoke cases；OpenAPI schema validation、鉴权和错误码场景属于增强方向�?
- `rozectl contract check` 已覆�?breaking changes：删�?route、改 method、改 path、删除字段、必填字段新增、响应类型变化�?

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

同一份模型应该被 HTTP server、RPC client/server、Gateway route、MQ consumer、Job runner 解释。各协议只负责“怎么执行”，不重新定义字段�?

当前差距�?

- Gateway 已经接入部分统一字段�?
- HTTP/RPC 已有治理能力，但字段、指标和 route/method/consumer 的标签口径还需要完全统一�?
- MQ/Job 的治理语义需要明确：哪些错误�?retry，哪些不�?retry，取消和 deadline 如何传播�?

### 3. 配置热更�?

目标形态：

- 新配置解析失败不替换旧配置�?
- section 级签名决定是否局部重建�?
- listener 失败不阻断其�?listener�?
- 每次变更�?version、hash、diff、source、operator、time�?
- 支持灰度、签名校验、回滚和审计�?

Roze 现状�?

- Etcd watch、Env/File fallback、diff/version、section event、失败回滚已有基础�?
- 仍需�?listener timeout/failure isolation、operator/audit 模型和灰度发布�?

建议边界�?

- `roze-config` 只负责安全地发布变更事件�?
- 具体 subsystem �?Kafka/Redis/DB/Gateway 自己决定是否重建�?
- 生成服务应默认注�?reload listener，但业务可选择只监听某�?section�?

### 4. 生命周期和健�?

目标形态：

- 统一 bootstrap 启动 HTTP/RPC/MQ/Job/background tasks�?
- 统一 SIGINT/SIGTERM 处理�?
- shutdown 分阶段：停止接流量、drain、取消后台任务、关闭连接池�?
- readiness 能检�?DB、Redis、MQ、RPC 下游、配置加载、后台任务�?

最低交付：

- `/healthz`：进程活着即可�?
- `/readyz`：依赖和后台任务 ready 才通过�?
- `/startupz`：慢启动期间可单独表达启动状态�?
- `/metrics`：Prometheus 文本输出�?

当前状态和差距�?

- `roze-health` 已有 `ProbeReport` �?`HealthRegistry`，支�?startup/ready/draining phase、静态检查和动�?dependency check�?
- REST 生成服务已有 `/healthz`、`/readyz`、`/startupz`、`/metrics` 默认入口，并返回结构�?probe report�?
- 生成 `ServiceContext` 会把启动时已连接�?DB、Mongo、Redis、NATS 注册�?readiness�?
- 协议�?ping、RPC/Gateway 标准 probe 入口、配置加�?后台任务状态和 HTTP/RPC/MQ/Job 统一关闭顺序还需要继续收口�?

### 5. 可靠事件

Roze 应明确承诺：

- 默认语义�?at-least-once�?
- 消费端必须幂等�?
- 失败�?retry，超过上限进�?DLQ�?
- Outbox/Inbox �?DB 状态和消息一致性的推荐路径�?
- 不承�?exactly-once�?

推荐事件 envelope�?

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

Roze 现状已经�?EventEnvelope/KafkaRecord/MQ Message，但需要收敛字段命名和�?adapter �?metadata 规范�?

### 6. AI 友好

AI 友好的重点不是“给 AI 写提示词”，而是让工程边界机器可读�?

文档生成状态：

- 已实�?`SERVICE.md`：职责、不负责、入口、依赖、运行命令�?
- 已实�?`AI_CONTEXT.md`：允许修�?禁止修改路径、生成器命令、测试命令、契约变更流程�?
- `ARCHITECTURE.md`：HTTP/RPC/MQ/DB/Redis/Config/Gateway 关系�?
- `DEPENDENCIES.md`：服务依赖、topic、database、cache key、外�?API�?

已实现的 `SERVICE.md` �?`AI_CONTEXT.md` �?`.api`、生成器模板和可选注释生成。`ARCHITECTURE.md`、`DEPENDENCIES.md` 后续应结�?config schema、MQ topic、DB/Redis 配置和外�?API 元数据生成，业务团队再补充领域语义�?

## 需求收敛后�?P0/P1/P2

### P0：让框架可信

P0 不应该继续扩模块，而应该补“可信闭环”�?

1. 锁定文档�?CLI 行为一致性：README/成熟度矩阵承�?Toasty 是默�?ORM，CLI 默认值和测试必须持续覆盖这一契约�?
2. 已实现多条生成器主路径测试：REST、RPC、model、search、OpenAPI、SDK、Docker/Kube/Helm 命令解析、核心生成逻辑、REST+model+search 组合 compile smoke �?RPC compile smoke 已有覆盖；增强方向是更多边界 fixture�?
3. 健康接口：REST 生成服务已默认暴�?`/healthz`、`/readyz`、`/startupz`、`/metrics`；Gateway/RPC 和依�?readiness 属于增强方向�?
4. 收口 lifecycle：统一 SIGINT/SIGTERM、shutdown timeout、background task manager、HTTP/RPC/MQ/Job 关闭顺序�?
5. �?Gateway/Config/MQ smoke tests：rewrite、timeout、auth、rate、breaker、retry、fallback、hot reload、DLQ replay�?
6. `rozectl doctor` 已实�?Rust/Cargo、Docker、kubectl、额外工具、端口、配置文件和 TCP 依赖地址检查；协议�?probe 属于增强方向�?
7. `rozectl diff` 已实�?API/RPC/model 文件级变化预览；语义 diff 和更细的 ownership-aware report 属于增强方向�?

### P1：让框架可上生产

1. 统一治理 schema 横跨 HTTP/RPC/Gateway/MQ/Job：timeout、retry、rate limit、breaker、shedding、bulkhead、fallback、deadline/cancel propagation�?
2. 安全模型进入契约层：`.api` 支持 jwt、permission、tenant、audit、idempotent、rate_limit、body_limit、timeout，并投影�?OpenAPI、middleware、测试骨架和 SDK�?
3. 配置中心补生产语义：签名、灰度、审计、操作者、回滚命令、listener timeout/failure isolation�?
4. 可观测资产交付：已提�?Gateway Grafana dashboard、Prometheus scrape/recording/alert rules �?SLO 模板；继续补 trace 示例、日志查询示例�?
5. MQ 可靠事件标准化：统一 envelope、schema version、idempotency key、outbox/inbox、DLQ 查看/重投/丢弃、consumer lag 指标�?
6. 生产部署模板：已实现 Docker/Kubernetes/Helm、HPA、PDB、ServiceAccount、NetworkPolicy、ConfigMap 接入；Secret、Gateway API/Ingress 和平台级发布集成属于增强方向�?

### P2：让框架成为团队�?AI 的默认协作底�?

1. 已实�?`rozectl mock gen`、`rozectl test gen`、`rozectl dev`；`rozectl stream gen` �?`rozectl bench` 属于增强方向�?
2. 已实�?`SERVICE.md` �?`AI_CONTEXT.md` 生成；`ARCHITECTURE.md`、`DEPENDENCIES.md` 属于增强方向�?
3. Admin API/UI：registry instances、config reload history、DLQ snapshots/replay/purge、breaker/rate limiter 状态�?
4. 高级流量治理：A/B testing、traffic mirror、blue/green、header/cookie/tenant/user routing�?
5. 完整示例：REST CRUD、REST+RPC+DB+Redis、Gateway+Registry+MQ+Outbox+DTM�?

## P0 里程碑拆�?

### M1：生成器可信

交付物：

1. `rozectl diff`：对 API/RPC/model 生成结果做文件级预览。（已实�?MVP�?
2. generated project tests：REST、RPC、model、search、OpenAPI、SDK、Docker/Kube/Helm 主路径已有生成器覆盖；REST+model+search 组合临时项目�?RPC 临时项目 compile smoke 已落地�?
3. ownership preservation tests：确�?`src/logic/**`、`src/svc/mod.rs`、自定义 middleware、`config.yaml` 不被 `--update` 覆盖�?4. `rozectl contract check`：对 `.api` 前后版本�?breaking change 检查。（已实�?MVP�?

验收�?

- 修改 `.api` 后可以先查看 diff，再执行 update�?
- 编译测试覆盖最�?REST/RPC 项目�?
- 生成器文档能明确列出框架拥有和业务拥有路径�?

### M2：运行时可信

交付物：

1. 统一 `/healthz`、`/readyz`、`/startupz`、`/metrics`。（REST 生成服务已实现默认入口）
2. 统一 lifecycle runtime：shutdown signal、drain、background task cancellation�?
3. Gateway smoke tests：rewrite、timeout、auth、rate、breaker、retry、fallback、hot reload�?
4. Config center smoke tests：invalid update rollback、section event、listener failure isolation�?

验收�?

- REST 生成服务�?`rozectl kube` 已实现标�?probes；Gateway/RPC 标准入口属于增强方向�?
- SIGTERM 后服务先停止接新流量，再�?deadline 内退出�?
- Gateway 热更新失败时继续使用旧配置�?

### M3：本地生产形态可�?

交付物：

1. `rozectl doctor` 最小版。（已实现本机工具、端口、配置文件和 TCP live probe�?
2. `rozectl dev` �?`docker-compose.integration.yml` 的一键入口。（已实�?up/down/status�?
3. 一�?`Gateway + Registry + MQ + DB + Redis` 示例�?
4. Prometheus scrape config 和最�?Grafana dashboard。（Gateway 观测资产已实现）

验收�?

- 新用户可以在本地跑起完整示例�?
- doctor 能提示缺失工具、端口冲突和配置缺项�?
- 示例能展�?trace id 贯穿 HTTP/RPC/MQ�?

## 架构分层建议

Roze 当前 crate 拆分基本合理，但需要把“统一模型”放到更清晰的位置�?

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

关键原则：HTTP、RPC、Gateway、MQ、Job 不应该各自拥有一套治�?观测/配置模型。它们应只做协议适配，统一调用 `roze-config`、`roze-context`、`roze-error`、`roze-metrics`、`roze-opentelemetry` 和治理组件�?

## 生成器所有权边界

这个边界已经�?Roze 的优势，应该继续强化�?

| 文件/目录 | 所有�?| 生成策略 |
| --- | --- | --- |
| `src/route/**` | 框架 | `--update` 刷新 |
| `src/handler/**` | 框架 | `--update` 刷新 |
| `src/server/**` / `src/client/**` | 框架 | `--update` 刷新 |
| `src/types/**` | 契约 | �?`.api`/proto 刷新 |
| `src/openapi/**` | 契约 | �?`.api` 刷新 |
| `src/logic/**` | 业务 | `--update` 保留 |
| `src/svc/mod.rs` | 依赖/应用 | `--update` 保留 |
| `src/middleware/<custom>.rs` | 业务 | `--update` 保留 |
| `config.yaml` | 部署/应用 | `--update` 保留 |
| Docker/K8s/Helm | 运维模板 | 已实现生成和 validate；diff/update 属于增强方向 |

## 推荐目录和模块归�?

| 能力 | 推荐归属 | 原因 |
| --- | --- | --- |
| Context/header propagation | `roze-context` | 协议无关，HTTP/RPC/MQ/Gateway 都复用�?|
| Error model/i18n | `roze-error`, `roze-result` | 避免 handler �?Gateway 手写不同错误结构�?|
| Governance config schema | `roze-config` 或独�?`roze-governance` | 配置结构应协议无关，runtime adapter 只解释执行�?|
| HTTP runtime middleware | `roze-middleware`, `roze-http` | Tower/Roze native HTTP 细节留在 HTTP 层�?|
| RPC retry/deadline/metadata | `roze-rpc`, `roze-grpc` | tonic/prost 细节留在 RPC 层�?|
| Gateway policy execution | `roze-gateway` | Gateway 只消费统一治理和注册发现，不自定义平行模型�?|
| MQ envelope/retry/DLQ | `roze-mq` | Kafka/NATS/RabbitMQ adapter 共享语义�?|
| Kafka adapter | `roze-kafka` | 只放 Kafka 具体实现�?rdkafka 适配�?|
| Config center source/watch | `roze-config` | Etcd/Env/File source �?reload event 属于配置层�?|
| Lifecycle/runtime | `roze-bootstrap`, `roze-shutdown`, `roze-health` | 服务启动、健康和关闭应统一�?|
| Generated templates | `apps/rozectl` | 生成器只消费公共 crate，不复制运行时逻辑�?|

## 已落地能力和增强方向

按收益和风险排序记录当前能力边界�?

1. 防止 ORM 默认值回归：Toasty 保持默认，`--orm sea-orm` 才切换到 SeaORM，并�?CLI 解析测试覆盖�?
2. 已实现：`rozectl doctor` 检�?Rust/Cargo、Docker、kubectl、额外工具、端口、配置文件和 TCP live probe；协议级依赖检查属于增强方向�?
3. 已实现：`rozectl diff` 文件级预览；语义 diff、breaking change report 和更详细�?ownership 说明属于增强方向�?
4. 已实现：REST 健康接口模板输出 `/healthz`、`/readyz`、`/startupz`、`/metrics`；RPC/Gateway 和依�?readiness 标准化属于增强方向�?
5. �?Gateway 增加 app �?smoke script：一键起 mock upstream + gateway，覆�?rewrite/auth/rate/breaker/retry/fallback�?
6. �?MQ 增加真实 Kafka/NATS integration profile：默认跳过，显式 env 开启�?
7. 已实现：`SERVICE.md` �?`AI_CONTEXT.md` 生成；依赖配置、MQ topic、DB/Redis 配置推导属于增强方向�?

## CLI 命令契约状�?

| 命令 | P 阶段 | 最小行�?|
| --- | --- | --- |
| `rozectl diff` | P0 | 已实现：生成到临时目录，对比目标目录，显示新�?修改/删除文件；默认不写盘�?|
| `rozectl doctor` | P0 | 已实现：检�?Rust/Cargo、Docker、kubectl、额外工具、端口、配置文件和 TCP 依赖地址；协议级依赖检查属于增强方向�?|
| `rozectl model generate` | P0 | 已处理：�?DSL/SQL/Mongo schema 生成 `src/model`，默�?Toasty，支�?`--orm sea-orm`，支�?`--update`/`--force`�?|
| `rozectl model inspect` | P0 | 已处理：支持 sqlite、Postgres、MySQL、MongoDB schema/collection inspect，并生成同一模型 scaffold�?|
| `rozectl search generate` | P0 | 已处理：支持 Elasticsearch、OpenSearch、Meilisearch，从 `.search` DSL �?JSON schema 生成 `src/search`�?|
| `rozectl search inspect` | P0 | 已处理：Elasticsearch/OpenSearch 读取 mapping，Meilisearch 读取 settings/index metadata 并采�?documents�?|
| `rozectl dev` | P1 | 已实现：通过 docker compose 执行 up/down/status，支�?profile、detach �?volumes�?|
| `rozectl contract check` | P0 | 已实现：`.api` 前后版本 breaking change 检查，覆盖 route/RPC/type/field 基础破坏性变更�?|
| `rozectl test gen` | P1 | 已实现：�?`.api` 生成 HTTP smoke contract tests 和最小断言�?|
| `rozectl mock gen` | P1 | 已实现：�?`.api` 生成独立 Roze native HTTP mock server，按 response type 返回默认 JSON�?|
| `rozectl doc service` | P1 | 已实现：�?`.api` 生成 `SERVICE.md`，包含接口清单、生成器所有权边界、常用命令和 AI editing notes�?|
| `rozectl doc ai-context` | P1 | 已实现：�?`.api` 生成 `AI_CONTEXT.md`，包含允�?禁止修改路径、生成器命令、测试命令和契约变更流程�?|
| `rozectl stream gen` | P2 | 从事件契约生�?producer/consumer skeleton、DLQ 配置�?envelope 类型�?|
| `rozectl bench` | P2 | 外部服务压测命令方向，输出延迟、错误率和吞吐；当前已先落地 crate �?Criterion 性能基线，不等同于压�?CLI�?|

`diff` �?`doctor` �?MVP 已落地。增强重点是让它们更�?Roze 项目语义：`diff` �?breaking change report，`doctor` 补协议级依赖检查和可执行修复建议�?

## 风险和决策记�?

| 决策/风险 | 当前结论 | 后续动作 |
| --- | --- | --- |
| 默认 ORM | Toasty 是默认，`--orm sea-orm` 切换 SeaORM�?| CLI 解析测试和文档持续覆盖，避免默认值漂移�?|
| exactly-once 消息语义 | 不承�?exactly-once�?| 文档明确 at-least-once + idempotency + outbox/inbox + DLQ + replay�?|
| Gateway �?service mesh 边界 | Roze Gateway 负责应用入口治理；不试图替代完整 service mesh�?| 高级 mTLS、sidecar 流量治理可作为集成方向，不进�?P0�?|
| 业务 SQL 和事�?| 不隐藏复杂业�?SQL；事务边界属�?application logic�?| 生成 repository scaffold，但完整业务事务通过示例和文档表达�?|
| 安全模型 | 认证授权 primitives 已有，但不能宣称完整安全平台�?| P1 统一 `.api` 注解、OpenAPI security、permission test templates�?|

## 验收标准

每个能力�?`beta`/`scaffold` 走向 `stable` 前至少满足：

1. 文档有运行契约和失败语义�?
2. 单元测试覆盖核心策略�?
3. 有端到端�?smoke test�?
4. �?metrics/log/trace 字段说明�?
5. 有生成器重复运行�?ownership preservation 测试�?
6. 有升级说明，破坏性生成变更能被用户预判�?
7. 有一个可以本地跑的生产形态示例�?

## 后续增强建议

最小可开工顺序：

1. 已实现：`rozectl diff`、`rozectl doctor`、`SERVICE.md`/`AI_CONTEXT.md` 生成�?
2. 已实现：REST/RPC/model/search/OpenAPI/SDK/Docker/Kube/Helm 生成器主路径覆盖�?
3. 增强方向：更�?generated-project 边界 fixture 和重复生成组合用例�?
4. 增强方向：Gateway/RPC probes、依�?readiness �?lifecycle runtime�?
5. 增强方向：Gateway/Config/MQ smoke tests�?

这条路径优先补“信任”和“可重复执行”，不会打乱现有 crate 结构，也能让后续 P1 的治理、安全、观测工作有稳定落点�?
