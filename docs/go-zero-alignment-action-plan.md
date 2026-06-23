# Roze 对齐 go-zero 的实施计划（可执行版）

## 总则
- 目标：按模块把行为落到“可验收”的粒度，与 go-zero 的核心使用语义对齐。
- 交付标准：每个模块都要有配置、运行时行为、监控、错误码、示例、回归测试。
- 当前优先策略：先补齐发布可信度、网关 v2、配置热更新和 MQ 语义，再统一治理模型，最后补充生产示例、观测资产和安全体系。

## 模块状态与细化任务

### 0) 发布体系与项目可信度（最高优先级）
- 当前状态：项目仍处于 pre-release；安装以 Git/local checkout 为主，GitHub Releases、crates.io、MSRV matrix、升级指南和稳定发布节奏尚未完成。
- 目标状态：用户能判断每个模块成熟度，能按 SemVer 升级，能从 crates.io 或 GitHub Release 安装，并能看到破坏性变更说明。
- 主要任务：
  1. [x] 维护 `CHANGELOG.md`、SemVer 规则、MSRV 说明和 release checklist。
  2. [ ] 补齐 GitHub Releases、crates.io 发布、tag 签名和升级指南（流程文档与升级指南已补，真实外部发布尚未启用）。
  3. [x] 维护模块成熟度矩阵，明确 stable/beta/scaffold/planned。
  4. [x] 补齐 Contributing、Security Policy、Code of Conduct、Issue/PR 模板。
  5. [x] 在 README 明确当前 pre-release 状态和推荐安装方式。
- 验收：
  1. 可以从一个 tag 或 crates.io 版本安装 `rozectl`。
  2. 每次 release 都有 changelog、升级说明和 breaking changes。
  3. README 不把 scaffold 模块描述成生产稳定模块。

### 1) HTTP 网关（高优先级）
- 当前状态：v1 已落地。`apps/roze-gateway` 基于 Axum/Tower HTTP 提供独立网关，已支持静态上游、路由映射、路径重写、超时、限流/熔断、鉴权、CORS、fallback 和配置中心热更新。
- 目标状态：补齐 registry 动态上游、重试、指标和更完整的治理联动。
- 主要任务：
  1. 支持从 `registry` 动态发现上游目标服务。
  2. 每条路由支持 `retries`、退避策略和重试指标。
  3. 统一响应结构与错误码映射，明确代理透传响应与网关 fallback 响应边界。
  4. 增加最小治理度量（网关请求计数、成功率、延迟、重试次数）。
  5. 增加 smoke test，覆盖 rewrite、timeout、auth、rate limit、breaker 和热更新。
- 验收：
  1. 可配置路由到上游，并返回上游 JSON 原样。
  2. 路由映射变更可无重启更新（热读配置）。
  3. 上游超时触发时返回标准错误码。
- 关联参考：
  - [go-zero gateway/http]（你提供的官方文档）
  - [crates/roze-http/src/rest.rs](/Users/yangcuiwang/go/src/hualiang/roze/crates/roze-http/src/rest.rs)
  - [crates/roze-rpc/src/registry.rs](/Users/yangcuiwang/go/src/hualiang/roze/crates/roze-rpc/src/registry.rs)
  - [crates/roze-middleware/src/lib.rs](/Users/yangcuiwang/go/src/hualiang/roze/crates/roze-middleware/src/lib.rs)

### 2) 配置中心与热更新（高优先级）
- 当前状态：基本完成，需固化接口一致性与端到端验证。
- 目标状态：配置变更影响最小重建，带签名比对和变更审计。
- 主要任务：
  1. [x] 定义 `ConfigCenterChangeEvent`，按 `section` 推送。
  2. [x] 规范字段可空边界，修正 `kafka.client_id` 的一致性。
  3. [x] 为关键子系统加版本签名，避免不必要重建。
  4. [x] 添加配置变更失败回退策略（保持上次有效配置）。
  5. [x] 在 `apps/user` 提供日志：`config_updated`, `section=...`。
- 验收：
  1. kafka 配置更新只影响消息子系统。
  2. 无有效值时继续沿用旧配置运行。
- 关联参考：
  - [crates/roze-config/src/config_center.rs](/Users/yangcuiwang/go/src/hualiang/roze/crates/roze-config/src/config_center.rs)
  - [apps/user/src/config.rs](/Users/yangcuiwang/go/src/hualiang/roze/apps/user/src/config.rs)
  - [apps/user/src/kafka.rs](/Users/yangcuiwang/go/src/hualiang/roze/apps/user/src/kafka.rs)

### 3) Kafka 与队列统一抽象（高优先级）
- 当前状态：已实现 rdkafka + in-memory，需稳定化。
- 目标状态：生产/消费语义一致，可观测、可回放、可死信。
- 主要任务：
  1. [x] 统一 `KafkaRecord` 元数据：`attempt`、`dead_letter_topic`、`timestamp`。
  2. [x] 明确手工提交场景的 commit/nack 行为。
  3. [x] 统一 `Producer` 返回值语义（包含 `partition` 与 `offset` 可选）。
  4. [x] 补偿：消息消费失败时可按配置延迟重试。
  5. [x] 记录 `topic/group/partition/offset` 指标（offset 作为 gauge value，不进入 label，避免高基数）。
- 验收：
  1. `enable_auto_commit=false` 时手工 ack 生效。
  2. nack 达到最大重试后进入 dead letter topic。
- 关联参考：
  - [crates/roze-kafka/src/lib.rs](/Users/yangcuiwang/go/src/hualiang/roze/crates/roze-kafka/src/lib.rs)
  - [apps/user/src/kafka.rs](/Users/yangcuiwang/go/src/hualiang/roze/apps/user/src/kafka.rs)

### 4) HTTP/RPC 治理统一（中高优先级）
- 当前状态：已具备统一治理实现。REST 入口支持 recover、trace、stat、prometheus、cors、timeout、max_conns、adaptive shedding、gunzip、request body limit；route 级治理支持 auth/JWT、timeout、rate limit、breaker 和自定义 middleware；RPC client/method 侧已具备 trace/stat/prometheus/breaker 与方法级 rate limit/breaker。
- 目标状态：REST 与 RPC 共用相同策略模型。
- 主要任务：
  1. [x] 统一 REST 服务级 middleware 配置：`rest.middlewares`。
  2. [x] 统一 `.api` 内建 middleware 名称解析，避免把 go-zero 常见 middleware 误生成成自定义 stub。
  3. [x] HTTP timeout 从 Context metadata 升级为框架层执行：服务级 timeout 走 Tower middleware，route 覆盖由生成 handler adapter 兜底。
  4. [x] Adaptive shedding 支持并发上限、窗口样本数、平均延迟阈值、失败率阈值和冷却时间。
  5. [ ] `breaker`/`ratelimit` 状态持久化可选。
  6. [ ] 与 `roze-event`、`roze-http`、`roze-rpc` 指标口径进一步统一。
- 验收：
  1. [x] REST 生成服务可通过 `rest.middlewares` 启停服务级 middleware。
  2. [x] `.api` 中声明 `trace/cors/recover/stat/prometheus/max_conns/shedding/gunzip/body_limit` 等内建名称不会生成自定义 middleware 文件。
  3. [x] 同一 governance timeout 字段在 HTTP route 产生实际超时行为。
- 关联参考：
  - [crates/roze-middleware/src/lib.rs](/Users/yangcuiwang/go/src/hualiang/roze/crates/roze-middleware/src/lib.rs)
  - [crates/roze-rpc/src/rpc.rs](/Users/yangcuiwang/go/src/hualiang/roze/crates/roze-rpc/src/rpc.rs)
  - [crates/roze-config/src/lib.rs](/Users/yangcuiwang/go/src/hualiang/roze/crates/roze-config/src/lib.rs)
  - [docs/contracts/middleware.md](/Users/yangcuiwang/go/src/hualiang/roze/docs/contracts/middleware.md)

### 5) 启动生命周期与优雅停机（中优先级）
- 当前状态：各应用各自实现。
- 目标状态：统一 `bootstrap` 生命周期。
- 主要任务：
  1. 整理 `roze-bootstrap` 公共启动入口。
  2. 定义标准信号处理（SIGINT/SIGTERM）。
  3. 统一 background tasks 的关闭顺序与超时。
  4. 统一就绪/存活探针事件。
- 验收：
  1. 发起终止后，HTTP、RPC、消费者均能优雅退出。

### 6) API/RPC 生成与代码骨架（中优先级）
- 当前状态：核心生成器已按 go-zero 风格目录拆分。REST 生成 `src/route/<group>.rs`、`src/handler/<group>/<method>.rs`、`src/logic/<group>/<method>.rs`、`src/middleware/<custom>.rs`、`src/config/mod.rs`、`src/openapi/mod.rs`、`src/types/mod.rs` 和 `src/svc/mod.rs`；RPC 生成 `src/server/mod.rs`、`src/client/mod.rs`、`src/pb/mod.rs`、`src/logic/<method>.rs`、`src/config/mod.rs`、`src/types/mod.rs` 和 `src/svc/mod.rs`。`--update` 保留业务逻辑文件、REST 自定义 middleware 文件和 `config.yaml`，刷新生成器拥有的 glue 文件。API 层默认不链接 DB/Mongo/Toasty；数据库默认示例为 PostgreSQL。生成服务固定 `edition = "2021"`。已新增 `rozectl api client ts/js/dart`，可从 REST `.api` 生成 TypeScript SDK、JSDoc JavaScript SDK 或 Dart `package:http` SDK；`rozectl openapi generate` 已输出参数、请求体、响应和组件 schema；service block 内多个 `@server` 分组已能按 route 生效到 prefix、middleware、JWT、OpenAPI 和 SDK 路径；parser 已兼容 `syntax = "v1"`、`info(`/`type(`/`@server(` 紧凑块、`returns(Resp)` 紧凑签名、`@handler(...)`/`@doc(...)`/`@middleware(...)` 注解和 `import (...)` 导入块；REST/OpenAPI/SDK 已支持 `patch` 方法；无 request 的 `get /path returns (Resp)` 路由会自动补 `EmptyReq`，无 response 的 `post /path (Req)` 或 `get /path` 路由会自动补 `EmptyResp`，并正常生成项目、OpenAPI 和 SDK；TS/JS SDK 对空请求方法已默认 `req = {}`，调用方不再需要手写空对象；生成的 Rust DTO 已派生 `Default`，并使用稳定 snake_case 字段名加 serde rename 保留 wire 名称；REST/types/OpenAPI 已支持 goctl 风格 `[]T`、`map[K]V` 与 Rust 风格 `Vec<T>`、`HashMap<K,V>` 容器类型；REST partial struct 已参考 Go validator tag 映射 `required/min/max/len/email/url/uri/ip/ipv4/ipv6/contains/excludes/gte/lte/gt/lt/optional/omitempty`，按 Rust validator 支持的 `length`、`range`、`email`、`url`、`ip`、`contains`、`does_not_contain` 生成属性；生成器自定义请求级校验已补 `oneof/startswith/endswith/alpha/alphanum/ascii/numeric/eqfield/nefield/gtfield/gtefield/ltfield/ltefield/required_if/required_unless/required_with/required_without`，覆盖 Rust validator derive 暂不支持但 Go validator 常用的单字段、跨字段和条件必填 tag；`dive` 已支持切片元素基础校验以及 map 的 `keys/endkeys` 基础校验，`dive` 前 `min/max/len/required` 作用于容器长度，`dive` 后规则作用于每个元素或 key/value。
- 目标状态：生成器行为和goctl语义更接近。
- 主要任务：
  1. 继续扩展注释、更多 goctl 边界语法和更完整 validator tag 的解析兼容性。
  2. [x] 保持用户自定义业务逻辑和自定义 middleware 的覆盖策略不变。
  3. [ ] 加入网关专用模板和示例。
  4. [ ] 继续补齐 Java/Kotlin 等客户端生成。
- 验收：
  1. 不丢失用户逻辑文件的更新。
  2. `rozectl api client ts/js/dart` 能生成可注入 base URL、全局 headers、按调用 headers 的客户端 SDK。
  3. `rozectl openapi generate` 输出的 OpenAPI 3 文档可被 Swagger UI/客户端生成器消费。
- 关联参考：
  - [apps/rozectl/src/parser.rs](/Users/yangcuiwang/go/src/hualiang/roze/apps/rozectl/src/parser.rs)
  - [apps/rozectl/src/generator/rest.rs](/Users/yangcuiwang/go/src/hualiang/roze/apps/rozectl/src/generator/rest.rs)
  - [apps/rozectl/src/generator/rpc.rs](/Users/yangcuiwang/go/src/hualiang/roze/apps/rozectl/src/generator/rpc.rs)

### 7) 运维与观测（中优先级）
- 当前状态：可观测组件已就位。
- 目标状态：指标命名统一，支持采样和告警联动。
- 主要任务：
  1. 统一 route/method/queue 指标标签。
  2. 增加关键链路 trace-id 追踪字段。
  3. 输出标准健康检查接口与依赖检查。
- 验收：
  1. Prometheus 拉取可直接画出 p95、error、queue depth。

### 8) 安全与鉴权（中低优先级）
- 当前状态：已提供 JWT 和权限组件。
- 目标状态：统一策略和错误响应。
- 主要任务：
  1. 统一鉴权 middleware 的错误码。
  2. 与 OpenAPI 自动声明权限。
  3. 会话与角色校验接口标准化。

### 9) 数据/缓存/事务（中低优先级）
- 当前状态：多方案并存。
- 目标状态：统一上下文绑定与错误策略。
- 主要任务：
  1. 统一 DB/Redis 会话注入。
  2. 默认重试与幂等策略文档化。
  3. 增加故障场景 fallback。

## 第一期实施范围（仅建议可开工）
- 任务A：网关 v2（目标文件）
  1. [x] [apps/roze-gateway] 新建服务框架
  2. [x] 路由配置模型与配置加载
  3. [x] 代理转发、重写、超时、鉴权、限流、熔断、fallback
  4. [x] registry 动态上游发现
  5. [x] 重试、错误码统一和治理指标
  6. [ ] app 级示例脚本；crate 级 smoke test 已覆盖 registry、retry、health/outlier
- 任务B：配置中心变更事件完善
  1. [x] 变更事件结构和日志
  2. [x] 变更失败回退
  3. [x] `kafka` 重启验证
- 任务C：Kafka 手工提交稳定化
  1. [x] 重试/回退策略
  2. [x] 指标与错误路径补齐

## 执行次序
- 周期0：发布体系、成熟度矩阵、GitHub 元信息和贡献/安全入口
- 周期1：网关 v2、配置中心收敛、Kafka/MQ 语义稳定化
- 周期2：HTTP/RPC/Gateway/MQ 治理统一
- 周期3：生成器边界测试、OpenAPI/validator 完整性、SDK 扩展
- 周期4：生产示例、部署清单、观测 dashboard、安全模型
