# Roze 与 go-zero 对比

本文档按工程能力对比 Roze 当前实现与 go-zero 的常见使用语义。目标不是逐行复刻 go-zero，而是在 Rust 技术栈里保留 go-zero 的高效工程边界：IDL 优先、稳定目录、生成器驱动、治理内建、业务代码可重复生成保护。

go-zero 官方定位是 “web and rpc framework with lots of builtin engineering practices”，并强调内建 timeout、concurrency control、rate limit、adaptive circuit breaker、adaptive load shedding、middleware、参数校验和多语言代码生成。

参考：

- [zeromicro/go-zero](https://github.com/zeromicro/go-zero)
- [Roze middleware contract](contracts/middleware.md)
- [Roze project standards](project-standards.md)

## 总体结论

Roze 当前已经接近 go-zero 的核心项目骨架和基础治理体验：

- `.api` / `.proto` 优先生成 API/RPC 服务。
- REST/RPC 按稳定目录拆分 handler、logic、svc、types、middleware。
- `--update` 保留业务 logic、自定义 middleware 和本地配置。
- HTTP 已支持 recover、trace、stat、prometheus、cors、timeout、rate_limit、breaker、max_conns、adaptive shedding、gunzip、body limit、auth/JWT、idempotency。
- RPC 已支持 tonic/prost 生成、Context metadata、错误 metadata、client governance、rate limit、breaker。
- API 层默认不链接数据库，避免把业务存储依赖拉进 HTTP 边界。

主要差距仍在生产成熟度和生态广度：

- go-zero 的 breaker/load shedding 是长期生产验证的 Go 实现；Roze 已有可运行版本，但还需要真实高压场景校准。
- go-zero 的 goctl 生态和多语言生成更完整；Roze 已有 TS/JS/Dart/OpenAPI，Java/Kotlin/更多 SDK 还未补齐。
- go-zero 的 model/cache 生成链路更成熟；Roze model 生成仍需要继续围绕数据库 schema、owner 和跨库边界打磨。
- go-zero 的服务治理默认值和文档案例更多；Roze 还需要更多真实服务示例和压测基线。

## 代码结构对比

### API 项目

go-zero 常见 API 项目结构：

```text
etc/
internal/
  config/
  handler/
  logic/
  svc/
  types/
  middleware/
*.api
*.go
```

Roze 当前 API 生成结构：

```text
config.yaml
Cargo.toml
src/
  main.rs
  config/mod.rs
  route/
    mod.rs
    <group>.rs
  handler/
    mod.rs
    <group>/
      mod.rs
      <method>.rs
  logic/
    mod.rs
    <group>/
      mod.rs
      <method>.rs
  middleware/
    mod.rs
    <custom>.rs
  openapi/mod.rs
  svc/mod.rs
  types/mod.rs
```

| 维度 | go-zero | Roze 当前状态 |
| --- | --- | --- |
| 路由定义 | `.api` | `.api` |
| handler/logic 分离 | 是 | 是 |
| 每个方法一个 logic 文件 | 是 | 是 |
| middleware 目录 | 是 | 是 |
| route 单独目录 | 通常由 handler 注册承担 | Roze 独立 `src/route/`，让路由 glue 更清晰 |
| 配置目录 | `etc` + internal config | `config.yaml` + `src/config/mod.rs` |
| 类型目录 | `internal/types` | `src/types/mod.rs` |
| OpenAPI | goctl 插件/命令 | 内建 `openapi` 模块和 `rozectl openapi` |

Roze 比 go-zero 多拆了 `route/`，这是有意设计：handler 只做请求适配，route 只做路由注册，logic 只做业务实现。

### RPC 项目

go-zero 常见 RPC 项目结构：

```text
etc/
internal/
  config/
  logic/
  server/
  svc/
*.proto
*.go
```

Roze 当前 RPC 生成结构：

```text
config.yaml
Cargo.toml
build.rs
proto/
  service.proto
src/
  main.rs
  client/mod.rs
  config/mod.rs
  pb/mod.rs
  server/mod.rs
  svc/mod.rs
  types/mod.rs
  logic/
    mod.rs
    <method>.rs
```

| 维度 | go-zero | Roze 当前状态 |
| --- | --- | --- |
| proto 优先 | 是 | 是 |
| server/logic/svc 分离 | 是 | 是 |
| 每个方法一个 logic 文件 | 是 | 是 |
| client 生成 | zrpc/client 生态 | `src/client/mod.rs` |
| proto 构建 | protoc + Go plugin | `build.rs` + prost/tonic |
| edition/语言版本 | Go module | 生成 crate 固定 Rust 2021 |

## Middleware 与治理对比

| 能力 | go-zero | Roze 当前状态 |
| --- | --- | --- |
| Recover | 内建 | 已支持 `recover` |
| Trace | 内建 | 已支持 `trace` |
| Stat / Metrics | 内建 | 已支持 `stat`、`prometheus`、`/metrics` |
| CORS | 支持 | 已支持 `cors` 和 `cors_config` 精细配置 |
| JWT/Auth | 支持 | 已支持 `auth`、`jwt` |
| Timeout | 链式控制 | 已支持 route governance；`rest.middlewares.timeout=true` 时服务级超时走 Tower middleware，route 覆盖由生成 handler adapter 兜底 |
| Concurrency / MaxConns | 内建并发控制 | 已支持 `max_conns` |
| Rate limit | 内建 | 已支持 route/method token bucket |
| Breaker | 自适应熔断 | 已支持 breaker；自适应策略还需继续校准 |
| Load shedding | 自适应负载保护 | 已支持 adaptive shedding：并发上限、窗口、样本、平均延迟、失败率、冷却时间 |
| Body limit | 支持 | 已支持 `request_body_limit_bytes` |
| Gunzip | 支持 | 已支持 `gunzip` |
| Idempotency | 业务常见能力 | Roze 额外内建 `idempotency` |
| Custom middleware | 支持 | 已支持并在 `--update` 保留 |

Roze 的 service-wide middleware 配置在：

```yaml
rest:
  middlewares:
    recover: true
    trace: true
    stat: true
    prometheus: true
    cors: true
    # cors_config:
    #   allow_origins: ["*"]
    #   allow_methods: ["GET", "POST", "PUT", "PATCH", "DELETE"]
    #   allow_headers: ["authorization", "content-type", "x-request-id", "x-trace-id"]
    #   expose_headers: ["x-request-id", "x-trace-id"]
    #   allow_credentials: false
    #   max_age_seconds: 3600
    timeout: true
    # max_conns: 1000
    # shedding:
    #   concurrency: 1000
    #   window_ms: 1000
    #   min_samples: 100
    #   max_avg_latency_ms: 500
    #   max_failure_ratio_per_mille: 500
    #   cool_down_ms: 1000
    # gunzip: true
    # request_body_limit_bytes: 2097152
```

Route-scoped middleware 仍通过 `.api` 声明：

```go
@server (
  prefix: /api/v1
  middleware: auth, trace, audit
)
service user-api {
  @handler getUser
  get /users/:id (GetUserReq) returns (UserResp)
}
```

`auth` 和 `trace` 是内建项；`audit` 是自定义 middleware，会生成并保留 `src/middleware/audit.rs`。

## 生成覆盖策略对比

| 文件类型 | go-zero 常见行为 | Roze 当前策略 |
| --- | --- | --- |
| Handler/server glue | 生成器维护 | 覆盖刷新 |
| Route glue | 生成器维护 | 覆盖刷新 |
| DTO/types | 生成器维护 | 覆盖刷新 |
| OpenAPI/proto/build glue | 生成器维护 | 覆盖刷新 |
| Logic | 用户实现业务 | `--update` 保留 |
| Custom middleware | 用户实现横切逻辑 | `--update` 保留 |
| Config | 部署/本地配置 | `--update` 保留 |
| Model | schema 拥有的生成物 | 可覆盖刷新 |

Roze 当前比 go-zero 更明确地区分了“框架拥有文件”和“用户拥有文件”，这是为了适配 Rust 项目里更强的模块边界和编译约束。

## 配置与依赖边界

| 维度 | go-zero | Roze 当前状态 |
| --- | --- | --- |
| API 配置 | yaml | yaml/toml/env/config center |
| RPC 配置 | yaml | yaml/toml/env/config center |
| API 默认数据库依赖 | 由项目模板/业务决定 | 默认不链接 DB/Mongo/Toasty |
| DB 默认示例 | MySQL 常见 | PostgreSQL 默认示例，MySQL 可选 |
| SQLite | Go 驱动按需 | Toasty 默认不启用 sqlite，避免 `libsqlite3-sys` links 冲突 |
| 服务版本 | Go module | 生成 crate 固定 Rust 2021，避免 Rust 2024 build script 行为差异 |

## 生成器能力对比

| 能力 | goctl | rozectl 当前状态 |
| --- | --- | --- |
| API service generation | 成熟 | 已支持 |
| RPC service generation | 成熟 | 已支持 `.api rpc` 和 `rpc protoc` |
| OpenAPI/Swagger | 支持 | 已支持 |
| TypeScript SDK | 支持 | 已支持 |
| JavaScript SDK | 支持 | 已支持 |
| Dart SDK | 支持 | 已支持 |
| Java/Kotlin SDK | 支持 | 未完成 |
| Model generation | 成熟 | 已支持 SQL/inspect/SeaORM/Toasty，但仍需继续打磨 |
| Docker/Kubernetes | 支持 | 已支持基础生成 |
| Plugin | 支持 | 已支持 API plugin 入口 |

## 当前差距清单

高优先级：

- 用真实压测校准 adaptive shedding 默认值。
- 继续统一 HTTP/RPC breaker 和 rate limit 的指标标签。
- 增加 middleware 行为的端到端生成服务测试。
- 补更多真实业务示例，尤其是 API + RPC + MQ + DB 的组合。

中优先级：

- 补 Java/Kotlin client generation。
- 补 gateway 专用模板。
- model 生成继续围绕 PostgreSQL/MySQL owner/schema 边界完善。
- breaker/rate limit 状态持久化可选。

低优先级：

- middleware 配置热更新后的运行时替换策略。
- 将文档里的 go-zero 对齐矩阵自动纳入 release checklist。

## Roze 不打算复刻的部分

- 不复刻 Go runtime 和 net/http 生态细节；Roze 使用 Axum、Tower、tower-http、tonic、prost。
- 不把 API 层默认绑定数据库；API 可以很薄，存储边界优先放在 RPC/model/业务模块。
- 不鼓励在 handler 写业务逻辑；handler 是生成器维护的适配层。
- 不把所有模板做成单文件；Rust 项目更需要清晰 module 边界和可增量编译的文件结构。
