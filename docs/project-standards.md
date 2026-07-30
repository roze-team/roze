# Roze 项目规范

本文定义 Roze 仓库和 `rozectl` 生成项目的默认工程边界。代码与测试是事实源；
文档、示例和计划必须跟随实现更新。

## 总原则

- IDL 优先：REST、RPC、DTO、model 与 search schema 先写入 `.api`、`.proto`、
  `.ent` 或 `.search`，再由 `rozectl` 生成边界代码。
- 约定优于配置：目录、命名、生命周期和错误语义保持稳定。
- Rust 原生：HTTP 使用 Roze native HTTP/Tower，RPC 使用 tonic/prost，数据层
  使用受支持的 Toasty/SeaORM 路径。
- 框架边界统一：HTTP、RPC、Gateway、MQ、Job 使用同一 Context、错误、治理、
  telemetry 和配置模型。
- 可重复生成：create、update、second update 应字节确定；失败原子回滚。
- 所有权明确：generator-owned 文件可刷新，application-owned 文件不得覆盖。
- 兼容优先：Roze 1.x 公共 API 只做向后兼容扩展；破坏性变更进入新 major。
- 证据诚实：单元测试、Windows smoke、真实依赖、固定 Linux benchmark 和
  24h/72h evidence 必须分级表述。

## 仓库结构

- `crates/roze-*`：可复用 runtime 与基础能力。
- `apps/rozectl`：parser、generator、模板、所有权规则和生成器测试。
- `apps/*`：真实使用示例或可运行基础设施；不能承载只为绕过 runtime 缺口的
  私有实现。
- `examples/`：可编译的集成示例。
- `docs/contracts/`：跨模块运行时契约。
- `docs/usage/`：用户命令与生成结果说明。
- `docs/evidence/`：绑定 revision、runner 和原始结果的证据索引。
- `scripts/`：release、smoke、soak、故障和证据 verifier。

除非直接关闭既定里程碑，不新增孤立 crate、生成语言或旁路工具。

## 生成文件所有权

生成器必须在模板或写入计划中声明文件归属：

- framework-owned：handler/server、route、pb、生成 client、DTO、OpenAPI、
  generated model/search repository、构建 glue。
- application-owned：业务 logic、自定义 middleware、扩展模块和本地
  `config.yaml`。
- marker-owned region：只更新显式 marker 之间的内容。

`--update` 只刷新 framework/marker-owned 内容并删除有 marker 的陈旧生成文件。
`--force` 只用于明确的全量重建。生成失败时，目标项目必须保持调用前状态。
任何生成输出变化都应修改 `apps/rozectl` 的 generator/template/test，不手改临时
生成目录。

## REST/API 项目

典型结构：

```text
config.yaml
Cargo.toml
src/
  main.rs
  config/mod.rs
  route/<group>.rs
  handler/<group>/<method>.rs
  logic/<group>/<method>.rs
  middleware/mod.rs
  middleware/<custom>.rs
  openapi/mod.rs
  svc/mod.rs
  types/mod.rs
```

- `logic/**` 是默认业务落点，可包含领域校验、授权、事务编排和对 repository/
  client 的调用；update 必须保留。
- `handler/**` 只负责协议提取、Context、校验、logic 调用和响应转换。
- `route/**` 只负责路由与 middleware/governance 组装。
- `svc/mod.rs` 只保存依赖，不放业务流程。
- `types/mod.rs` 与 `openapi/mod.rs` 来自 IDL，wire name 必须稳定。
- 自定义 middleware 文件与 `config.yaml` 默认由应用拥有。
- HTTP 成功响应使用 `roze-result::ApiResponse`，错误使用 `RozeError`。
- route/service middleware 的优先级和行为遵守
  [Middleware Contract](contracts/middleware.md)。
- `rozectl openapi generate` 与运行服务暴露的 schema 必须语义一致。
- API-only 生成输入不得混入 RPC method；无 request/response 时使用稳定的空类型。

## RPC 项目

典型结构：

```text
config.yaml
Cargo.toml
build.rs
proto/service.proto
src/
  main.rs
  client/mod.rs
  config/mod.rs
  pb/mod.rs
  server/mod.rs
  svc/mod.rs
  types/mod.rs
  logic/<method>.rs
```

- `logic/**` 由应用拥有。
- `server/mod.rs` 恢复 Context、执行校验、调用 logic、转换统一错误，并在 success
  response/status 返回剩余 retry budget。
- `client/mod.rs` 负责 Context metadata、deadline、请求级 retry budget、
  per-attempt discovery/P2C/EWMA、结果结算和治理。
- `pb/mod.rs`、规范化 proto、types 和构建 glue 由生成器拥有。
- RPC client 的第一个业务上下文参数保持为 `&roze_context::Context`。
- RPC error metadata 使用 `x-roze-error-code`、`x-roze-error-kind`、
  `x-request-id`、`x-trace-id`、可选 locale 与 retry budget。
- 服务注册与发现统一使用 `roze_rpc::registry`。
- `.api` RPC 输入不得混入 REST route；proto 不支持的字段必须 fail fast。

## Context、deadline 与 cancellation

标准契约见 [Context Contract](contracts/context.md)。

- 使用 `x-request-id`、`x-trace-id`、`x-roze-timeout-ms`、标准 auth/tenant/
  locale/idempotency metadata。
- 入站没有 request/trace ID 时由入口生成，并在适用响应中返回。
- deadline 传播剩余时间，不在每一跳重置。
- clone/fork 共享取消和同进程 retry budget。
- 跨进程 fan-out 原子划拨预算，不复制完整 remaining 值。
- success、error、timeout、cancel 和 panic 均须释放 lease、permit、connection
  和 task。

## 配置与热更新

- 应用配置统一使用 `roze_config::ServiceConfig`。
- 本地默认 `config.yaml`；环境与配置中心作为覆盖或热更新来源。
- 解析、校验或 listener 失败时保留最后有效配置。
- listener 有 timeout；变更按 section/signature 判断重建范围。
- listener address、数据库 schema 等不可安全热换的字段必须明确要求 restart。
- 新配置字段同步示例、serde 默认、契约、生成模板和测试。
- secret 不提交到示例真实值，不进入 diff 日志或 reload 审计正文。

## 校验、错误与类型

- REST/RPC DTO 在入口执行 `roze-validation`。
- Rust 字段使用 snake_case，serde/prost 保留 wire name。
- `required`、范围、长度、email/url/ip、跨字段、条件必填、collection dive、
  map keys/endkeys 等映射必须有生成器测试。
- HTTP、RPC、Gateway 与 MQ 的错误分类保持一致；协议 adapter 只负责映射。
- 禁止 handler/server 手写互不兼容的错误 JSON/status。

## 服务发现、Gateway 与治理

- registry 统一使用 Memory、DNS、Etcd、Consul 与 cached resolver。
- RPC 每个真实 attempt 重新选择实例，并反馈 latency、success、timeout、failure
  与 in-flight。
- Gateway upstream 可来自静态 URL 或 registry；route > service > global
  governance。
- 健康、outlier、权重、instance tags 与 route 灰度必须显式、可观测、有回收
  路径。
- retry 只统计真实执行的额外 attempt；deadline/cancel 阻止的计划不计 retry。
- timeout、rate limit、breaker、shedding、fallback、health、registry churn
  必须有成功、失败、恢复和资源释放测试。

## 数据、缓存、事务与消息

- model generation 保持 page/sort/filter/projection/aggregate、tenant、audit、
  soft-delete 和 optimistic concurrency 的稳定契约。
- 复杂 SQL、业务事务边界、领域授权与跨聚合编排属于 application logic。
- 本地缓存使用 `roze-local-cache`；分布式缓存使用 `roze-cache` /
  `roze-redis`；热点回源使用 `roze-singleflight`。
- cache key 必须包含必要 tenant/version scope，写路径遵守失效契约。
- 可靠事件优先使用 persistent outbox + inbox/idempotency；publisher 使用
  `roze_mq::Publisher` 抽象。
- 消费者明确 ack/nack、retry、DLQ、replay 与 duplicate-effect 语义。
- TCC 是默认分布式事务路径；Saga 是显式可选 workflow。
- 数据库、Redis、broker 和 search 的“通过”必须来自真实依赖流程。

## Search 项目

```text
src/search/mod.rs
src/search/<index>.rs
```

- `.search` DSL 或 JSON schema 是事实源。
- `rozectl search generate` 生成 document 与 repository。
- `search inspect` 从 Elasticsearch/OpenSearch mapping 或 Meilisearch
  settings/sample 恢复 schema。
- serde rename 保留原始 index field。
- ranking、boost、召回、组合过滤与重排属于应用模块。
- model 与 search generation 分离；update 只刷新生成文件。

## AI 模块

- `rozectl ai generate <name> --out <project>` 只向已有 REST/RPC 项目添加
  `src/ai/**`，不改变 API、RPC、model、search 或 stream 的生成入口与输出。
- `src/ai/mod.rs`、`src/ai/generated.rs` 由生成器拥有；`agent.rs`、
  `tools.rs` 与 `prompts/**` 由应用拥有，`--update` 必须保留。
- `--with-workflow` 与 `--with-rag` 可分步增加应用拥有的 `workflow.rs`
  和 `rag.rs`；功能启用后普通 `--update` 必须保留文件及模块声明。
- `--with-team` 可分步增加应用拥有的 `team.rs`；Agent 名称、任务上限和执行
  模式由应用显式配置。
- AI runtime 复用 `roze-context`、`roze-error`、`roze-service` 与现有治理、
  存储、缓存、MQ、search、job 模块，不新增平行基础设施。
- Provider 配置统一进入 `roze_config::ServiceConfig::ai`；API key 使用现有
  secret reference 解析与脱敏能力，不得写入生成的 Rust 源码或日志。
- OpenAI-compatible provider 适配器只负责协议映射；业务检索、缓存、存储、
  MQ 与异步任务分别通过现有 Roze 模块封装成 AI Tool，不复制对应实现。
- AI 工具权限来自入站 `Context`；Agent 循环必须有明确 `max_steps`，并遵守
  deadline 与 cancellation。
- Workflow 必须在启动前校验环、不可达节点和 START/END 路径；节点复用同一
  Roze Context。RAG 通过 `roze-search` 适配器检索和索引，ranking、filter、
  rerank 与文档字段映射属于应用逻辑。
- 可恢复 Workflow 必须校验 checkpoint version、graph revision、tenant 和
  subject。内存 CheckpointStore 仅用于开发测试；对象存储适配器必须复用
  `roze-storage`，敏感状态负责加密，并通过现有锁/租约机制串行化并发 resume。
- 并行 Workflow 仅并发同一拓扑层；多 Agent task 数和每个 Agent 的
  `max_steps` 都必须有界，并继续传播同一个 Roze Context。
- Workflow 事件流必须保持稳定拓扑顺序；模型选择的 Agent 委派必须走标准
  Tool 权限检查，且不得隐式向子 Agent 注入递归委派能力。
- 节点级 chunk 流必须传播同一 Context、提供全局有界 chunk 预算并保持背压；
  框架只自动组合严格线性 START/END 路径，分支、合并与 zip 语义由应用显式实现。
- AI 模块生成必须事务化；失败时目标项目保持调用前状态。

## 可观测性与基数

- 指标 label 只能来自有界配置枚举，例如 service、operation、boundary、
  outcome、decision、source。
- endpoint、instance ID、tenant、subject、request/trace ID、原始 path、offset、
  partition、错误正文不得进入 Prometheus label。
- 高基数字段进入受采样 trace event 或脱敏结构化日志。
- HTTP、RPC、Gateway、MQ、registry、outbox 和 config reload 必须有统一
  request/trace 关联。
- state map、watch task、breaker、outlier、retry budget 和缓存必须有容量上限
  或明确删除路径，并在 soak 报告记录起点、峰值和终点。

## 测试与验收

按变更范围至少运行：

```bash
cargo fmt --all -- --check
cargo test -p <changed-crate>
cargo clippy -p <changed-crate> --all-targets -- -D warnings
```

生成器变更运行：

```bash
cargo test -p rozectl -- --skip postgres --skip mysql --skip mongo
```

模板或生成依赖变化还要从空目录生成并编译对应工程。真实依赖测试使用仓库
integration/reference-system 脚本；固定 Linux benchmark、24h/72h soak 与发布
证据必须保留原始 artifact、revision、dependency digest 和 runner metadata。

## 文档同步

- 公共命令变化：更新 `docs/usage` 与相关 README。
- runtime 语义变化：更新 `docs/contracts`。
- 生成 surface 变化：更新模板说明、能力矩阵与 generated compile fixture。
- 配置变化：更新示例、契约和测试。
- 生产结论变化：更新 maturity/evidence，但只有 verifier 通过的对应 revision
  可以晋级。
- 所有跟踪的文本文件必须是严格 UTF-8，不得包含 Unicode replacement character。
