# Roze 项目规范

本文档定义 Roze 仓库和 `rozectl` 生成项目的默认规范。目标是让 API、RPC、网关、MQ、配置中心和运维能力按同一套边界演进，避免每个服务重复定义自己的工程习惯。

## 总原则

- IDL 优先：REST 路由、RPC 方法、请求/响应 DTO 优先由 `.api` 或 `.proto` 描述，再由 `rozectl` 生成边界代码。
- 约定优于配置：生成项目保持稳定目录结构，业务代码只进入明确的扩展点。
- Rust 原生优先：兼容 goctl 命令形态，但输出保持 Axum、Tower、tonic、prost、Toasty/SeaORM 等 Rust 技术栈。
- 框架边界统一：HTTP、RPC、MQ、Gateway 都必须使用统一 Context、错误、日志、指标和配置模型。
- 可重复生成：生成器必须支持重复运行，默认保护业务逻辑和本地配置。

## 仓库结构

- `crates/roze-*`：可复用框架能力。新增通用能力优先进入 crate，而不是写死在示例应用。
- `apps/rozectl`：代码生成器和 goctl 兼容入口。生成逻辑、模板、解析规则和生成器测试放在这里。
- `apps/roze-example`、`apps/user`、`apps/roze-sample`：示例和验收应用，只承载真实使用方式，不承载框架专用逻辑。
- `apps/roze-gateway`、`apps/roze-dtm`：可独立运行的基础设施服务。
- `docs/contracts`：运行时契约。Context、Gateway、Queue、Config Center、DTM、Storage 等跨模块行为在这里固化。
- `docs/usage`：用户指南。命令、生成结果、兼容行为、SDK/OpenAPI 使用方式在这里说明。
- `docs/go-zero-alignment-action-plan.md`：go-zero 对齐计划和剩余任务清单。

## API 项目规范

API 项目由 `rozectl api generate` 或 goctl 兼容入口 `rozectl api go` 生成，面向 HTTP REST 服务。

固定结构：

```text
config.yaml
Cargo.toml
src/
  main.rs
  config.rs
  context.rs
  middleware.rs
  openapi.rs
  routes.rs
  svc.rs
  types.rs
  handler/
  logic/
```

文件归属：

- `src/logic`：业务逻辑唯一默认落点。复杂 SQL、领域校验、事务、授权和权限检查都写在这里或这里调用的业务模块。
- `src/handler`：HTTP handler 适配层，由生成器维护，只做请求解析、Context 提取、调用 logic 和响应包装。
- `src/routes.rs`：路由注册，由生成器维护。
- `src/types.rs`：请求/响应 DTO，由 `.api` 生成。
- `src/openapi.rs`：OpenAPI schema 和文档导出，由生成器维护。
- `src/middleware.rs`：应用 middleware 入口，可按生成器约定扩展，但不能承载业务流程。
- `src/svc.rs`：依赖注入和服务上下文，只放 DB、cache、MQ、配置、外部 client 等依赖。
- `config.yaml`：本地和部署配置，`--update` 默认保留。

API 运行边界：

- HTTP 响应统一使用 `roze-result::ApiResponse`。
- HTTP 错误统一使用 `RozeError`，禁止在 handler 中手写不一致的错误 JSON。
- 请求入口必须注入和传播 `roze-context` 标准 header。
- REST route 级别治理包括 timeout、JWT、middleware、rate limit、breaker 和 OpenAPI 安全声明。
- `rozectl openapi generate`、`rozectl api swagger` 和生成服务的 `/openapi.json` 必须保持同一 schema 语义。

API 生成策略：

- 使用 `--update` 重新生成框架拥有的文件，并保留 `src/logic/mod.rs` 和 `config.yaml`。
- 使用 `--force` 只适合全量重建或一次性脚手架验证。
- API 定义不能包含 RPC method；如果 `.api` 中存在 `rpc`，应使用 `rozectl rpc generate`。
- 无 request 的 REST 路由生成 `EmptyReq`，无 response 的 REST 路由生成 `EmptyResp`。

API 测试要求：

- parser/generator 改动必须补 `rozectl` 测试。
- validator tag、OpenAPI、SDK、route prefix/middleware/JWT 行为必须有覆盖。
- 改动 HTTP 运行时或 middleware 时，至少运行相关 crate 测试和 `cargo test -p rozectl -- --skip postgres --skip mysql`。

## RPC 项目规范

RPC 项目由 `rozectl rpc generate` 或 `rozectl rpc protoc` 生成，面向 tonic/prost gRPC 服务。

固定结构：

```text
config.yaml
Cargo.toml
build.rs
proto/
  service.proto
  source.proto
src/
  main.rs
  client.rs
  config.rs
  context.rs
  pb.rs
  rpc.rs
  svc.rs
  types.rs
  logic/
```

文件归属：

- `src/logic`：RPC 业务逻辑默认落点。
- `src/rpc.rs`：tonic server 适配层，由生成器维护，负责 Context 提取、参数校验、错误转换和调用 logic。
- `src/client.rs`：生成的 RPC client，由生成器维护，负责 Context metadata 注入、超时、retry 和 registry 连接。
- `src/pb.rs`：prost include 入口，由生成器维护。
- `src/types.rs`：共享类型或辅助类型，由生成器维护。
- `src/svc.rs`：依赖注入和服务上下文，只放依赖，不放业务流程。
- `config.yaml` 属于部署配置，`--update` 默认保留。
- `proto/source.proto` 保留用户输入，`proto/service.proto` 是生成器规范化后的构建输入。

RPC 运行边界：

- RPC server 必须使用 `roze_rpc::rpc::request_context` 恢复 Context。
- RPC client 必须以 `&roze_context::Context` 作为第一个业务上下文参数。
- RPC 错误统一通过 `roze_rpc::rpc::status_from_error(err, &request_ctx)` 转为 gRPC status。
- 错误 metadata 必须包含 `x-roze-error-code`、`x-roze-error-kind`、`x-roze-request-id`、`x-roze-trace-id` 和 `x-roze-locale`。
- 服务注册发现统一走 `roze_rpc::registry`。

RPC 生成策略：

- `rozectl rpc generate` 从 `.api` 的 `rpc` method 生成 Rust-native RPC 项目。
- `rozectl rpc protoc` 接受 proto3 源文件，`--go_out` 和 `--go-grpc_out` 只用于命令兼容，Rust RPC 项目文件生成到 `--zrpc_out`。
- RPC 定义不能包含 REST route；如果 `.api` 中存在 REST route，应使用 `rozectl api generate`。
- proto parser 遇到不支持的字段类型必须 fail fast。

RPC 测试要求：

- RPC 生成器改动必须覆盖 `rpc.rs`、`client.rs`、`proto/service.proto` 和保留 `proto/source.proto` 的行为。
- Context metadata、validation、retry、registry resolver、错误 metadata 必须有测试。
- 改动 RPC runtime 时至少运行 `cargo test -p roze-rpc` 和相关生成器测试。

## 配置规范

- 应用配置统一使用 `roze-config::ServiceConfig`。
- 本地文件默认使用 `config.yaml`；环境变量和配置中心只作为覆盖或热更新来源。
- 配置中心必须保留上一次有效配置；新配置解析失败时记录 reload failure，不替换运行态配置。
- 需要热更新的子系统必须按 section 或签名判断是否重建，避免无关配置变更触发全量重启。
- 配置字段新增时必须同步示例配置、契约文档和测试。

## Context、错误和响应

- HTTP、RPC、Gateway、MQ 入口必须传播 `x-roze-request-id`、`x-roze-trace-id`、tenant、locale、auth subject 和 metadata。
- 未传入 request id 或 trace id 时，由入口生成并回写。
- HTTP 错误统一走 `RozeError` 和 `roze-result::ApiResponse`。
- RPC 错误统一走 `roze_rpc::rpc::status_from_error(err, &request_ctx)`，metadata 必须包含等价 HTTP code、error kind、request id、trace id 和 locale。
- Gateway fallback 响应和上游透传响应必须保持边界清晰：上游成功响应原样透传，网关自身错误使用标准 fallback。

## 校验和类型

- 请求 DTO 必须派生验证能力，REST/RPC 生成入口都要执行 `roze-validation`。
- `.api` 字段名生成 Rust snake_case 字段，并通过 serde rename 保留 wire 名称。
- go-playground validator 常用 tag 应尽量映射到 Rust validator 或自定义请求级校验。
- `required`、范围、长度、email/url/ip、跨字段、条件必填、集合 `dive`、map `keys/endkeys` 等行为必须有生成器测试。

## 指标、日志和追踪

- HTTP 路由指标统一使用 `roze_http_route_*`。
- RPC 方法指标统一使用 `roze_rpc_method_*`。
- Gateway 路由、retry、upstream 事件统一使用 `roze_gateway_*`。
- MQ 指标必须包含 topic、group、partition、offset、attempt 和 outcome 等关键标签。
- 日志必须包含 request id 或 trace id；热更新、重试、熔断、限流、死信、回退等治理事件必须有结构化字段。
- 新增生产级 adapter 时，必须补 metrics 标签验证和 trace/context 透传测试。

## 服务发现、网关和治理

- 服务发现统一走 `roze_rpc::registry`，支持 memory、dns、etcd、consul 和 cached resolver。
- Gateway upstream 可以来自静态 `upstream` 或 registry 动态发现；动态发现优先使用健康状态和 outlier 状态过滤实例。
- route 级治理优先级高于 service 级和全局配置。
- retry 只记录真实发生的重试，不把最后一次失败计入 retry。
- 限流、熔断、超时、fallback、健康检查、outlier、registry 行为必须有 crate 级 smoke test；可运行 app 级示例脚本作为交付验收。

## 数据、事务和消息

- ORM 生成默认保持稳定 page/sort/filter/tenant/audit/soft-delete 契约。
- 复杂 SQL、事务边界、领域校验、授权校验属于业务逻辑，不由生成器自动实现。
- 可靠事件发布优先走 outbox relay；可发布到任意 `roze_mq::Publisher`。
- MQ 消费必须明确 ack/nack、retry、dead letter 和 idempotency 行为。
- DTM 默认使用 TCC；Saga 作为可选工作流，不应破坏默认 TCC 状态机。

## 测试和验收

提交前至少运行相关 crate 测试；涉及生成器时运行：

```bash
cargo test -p rozectl -- --skip postgres --skip mysql
```

涉及网关、RPC、配置、MQ 时运行对应测试：

```bash
cargo test -p roze-gateway -p roze-rpc -p roze-config -p roze-mq
```

需要真实依赖时使用集成环境：

```bash
docker compose -f docker-compose.integration.yml up -d
```

生产级 adapter 的验收至少覆盖：

- 真实服务集成测试
- 断线重连测试
- 重试、超时、取消测试
- metrics 标签验证
- trace/context 透传验证
- i18n 错误响应验证

## 文档同步

- 新增或修改公开命令时，同步 `docs/usage` 和 README。
- 新增运行时契约时，同步 `docs/contracts`。
- 新增 go-zero 对齐能力时，同步 `docs/go-zero-alignment-action-plan.md`。
- 新增配置字段时，同步示例 `config.yaml`、契约文档和测试。
- 文档中的能力状态必须以代码和测试为准，不以计划项为准。
