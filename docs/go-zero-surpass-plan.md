# Roze 生产生成能力超越 go-zero 计划

本文是 Roze 生产生成能力的唯一执行计划。目标不是宣称 Roze 已经拥有
go-zero 多年的生产历史，而是在可重复、可审计、可验收的工程能力上超越
go-zero，同时吸收其最成熟的架构设计。

## 1. 对标基线

- Roze：`d73f4ff01`（2026-07-18，本仓库当前 HEAD）。
- go-zero：`6a6b81ef20d5697f4fbe9c2a92c436e85d687be4`
  （2026-07-17 拉取的官方仓库 HEAD）。
- 基线日期：2026-07-18。
- 对标范围：服务生成、运行时默认能力、数据一致性、可观测性、交付运维和
  生产证据。
- 不对标：社区规模、历史用户量、Go 与 Rust 的语言流行度，以及 Roze 已明确
  排除的 Kotlin、Swift、Dart、Java、iOS、Android SDK。

官方参考：

- [go-zero 仓库](https://github.com/zeromicro/go-zero)
- [go-zero 架构](https://go-zero.dev/concepts/architecture/)
- [go-zero 设计原则](https://go-zero.dev/concepts/design-principles/)
- [go-zero 服务发现](https://go-zero.dev/guides/microservice/service-discovery/)
- [go-zero 组件](https://go-zero.dev/components/)

版本比较必须固定到 Git revision。后续 go-zero HEAD 变化时，先更新基线和差异
报告，再调整计划，不能用浮动的 `latest` 作为验收对象。

## 2. “超越”的严格定义

只有以下条件全部满足，才能称为“生产生成能力超越”：

| 维度 | 验收结果 |
| --- | --- |
| 生成覆盖 | 一个 `rozectl` 覆盖 REST、RPC、stream、model、search、OpenAPI、Web SDK、部署、可观测、报表和证据资产。 |
| 可重复生成 | create、update、second-update 字节确定；所有应用所有权文件均不被覆盖；失败生成原子回滚。 |
| 默认韧性 | REST、RPC、Gateway、MQ、Job 使用同一治理模型，统一 deadline、cancellation、retry budget、rate limit、breaker、shedding 和低基数标签。 |
| 下游客户端 | 服务发现、健康状态、P2C、实时 EWMA、在途负载、异常实例剔除、超时、重试预算、熔断和遥测构成一个闭环。 |
| 上下文 | deadline、cancellation、W3C trace、tenant、subject、locale、idempotency key、retry budget 跨越所有生成边界。 |
| 数据正确性 | tenant scope、事务、乐观并发、缓存失效、outbox/inbox、迁移和回滚均有生成契约与真实故障测试。 |
| 运维交付 | 每个生成服务都包含不可变部署、探针、SLO、仪表盘、告警、诊断查询、容量策略、备份恢复和回滚演练。 |
| 发布验证 | 一个 release gate 编译并冒烟全部支持目标，阻止破坏性 API、SQL、Search 和部署变更。 |
| 性能证据 | 与固定 go-zero 基线在相同机器、依赖、数据集、并发和 SLO 下运行；报告吞吐、p50/p95/p99、CPU、内存、连接、错误率和恢复时间。 |
| 长稳证据 | Gateway、MQ、Config Center、Lifecycle、生成参考系统均有绑定 revision、校验和和证明材料的 24h/72h 通过报告。 |

功能数量、单元测试数量、短时 smoke 或稳定 API 声明，均不能单独满足“超越”。

## 3. 必须借鉴的 go-zero 架构

### 3.1 统一且稳定的生成路径

借鉴 `goctl` 的固定分层：协议适配属于 handler/server，业务属于 logic，共享依赖
属于 ServiceContext，数据访问属于 model。Roze 对应边界继续保持：

- `.api` / `.proto` / `.ent` / search schema 是事实源；
- generator-owned 文件可重建；
- `src/logic/**`、扩展文件、自定义中间件和应用配置由应用所有；
- 所有生成操作必须事务化，失败不留下半生成项目。

### 3.2 稳定性默认开启

借鉴 go-zero 将 timeout、breaker、rate limit、adaptive shedding、metrics 和 tracing
放在框架边界的做法。Roze 的进一步要求是同一策略必须覆盖 REST、RPC、Gateway、
MQ 和 Job，且策略解析优先级、取消语义和指标标签完全一致。

### 3.3 下游调用闭环

借鉴 go-zero `zrpc` 的服务发现、P2C、EWMA、熔断和遥测组合。Roze 不能只根据
静态 registry metadata 选实例，必须用真实调用结果持续更新：

- EWMA 延迟；
- 在途请求数；
- 成功、超时、连接失败和 5xx 结果；
- 主动健康与被动异常实例剔除；
- retry budget 和上游 deadline。

### 3.4 数据与缓存工程

借鉴 go-zero model 的生成/扩展文件分离、缓存、singleflight、负缓存和写后失效。
Roze 在此基础上增加 tenant scope、乐观并发、显式事务、持久化
outbox/inbox、迁移风险门禁和数据库/搜索一致性验证。

### 3.5 自动可观测与简单运维

借鉴 go-zero 的低接入成本：日志、指标、trace、健康检查和优雅退出无需每个服务
重复接线。Roze 的超越点是生成 SLO、告警、runbook、离线部署校验、故障演练和
证据晋级资产，并通过 release gate 强制验证。

## 4. 当前能力与真实差距

状态只使用以下四种：

- `已实现`：代码、聚焦测试和生成编译验证存在；
- `已集成`：真实外部依赖测试存在；
- `证据待补`：实现存在，但缺少规定的 CI、24h/72h 或故障证据；
- `未完成`：验收条件仍缺实现或自动化验证。

| 领域 | Roze 当前状态 | 相对 go-zero 的判断 | 剩余缺口 |
| --- | --- | --- | --- |
| REST/RPC 分层生成 | 已实现 | 已吸收其固定分层和 ServiceContext 思路 | 保持回归门禁 |
| stream/search/report/evidence 生成 | 已实现 | Roze 生成面更广 | 补真实部署证据 |
| 确定性更新与所有权保护 | 已实现 | Roze 目标更严格 | 保持跨平台字节确定 |
| 合同与迁移风险门禁 | 已实现 | Roze 具备明确超越点 | 扩充真实数据库演练 |
| 统一治理 | 已实现 | Roze 覆盖边界更广 | 补端到端取消与标签基数测试 |
| RPC P2C | 已实现 | 已接入实时 EWMA、在途负载、结果反馈和逐 attempt 重选 | 补固定 Linux 同条件基准和真实慢节点恢复证据 |
| 服务发现 | 已集成 | Memory/DNS/Etcd/Consul 覆盖较广 | 补长稳 churn、watch、重连和数据面存活证据 |
| Cache/singleflight | 已集成 | 能力基本对齐并扩展一致性策略 | 补高并发失效与故障矩阵 |
| MQ/outbox/inbox | 证据待补 | 生成契约更完整 | 真实 NATS/Kafka 24h/72h 与重启证据 |
| Gateway/Config Center | 证据待补 | 功能覆盖较广 | 真实注册中心、热更新和流协议长稳证据 |
| Lifecycle | 证据待补 | 生命周期契约更显式 | 任务泄漏、反向 drain 和超时 hook 长稳证据 |
| Security | 证据待补 | OIDC/mTLS/JWT/RBAC/ABAC 已有契约 | 跨 REST/RPC/Gateway/MQ 的租户隔离证据 |
| 运维资产 | 已实现 | 生成面优于 goctl 默认输出 | 在参考系统中执行备份、恢复、回滚和容量演练 |
| 生产成熟度 | 证据待补 | go-zero 明显领先 | 真实用户负载不能用仓库声明替代 |

当前最重要的事实是：Roze 已经完成大量“能力实现”，但尚未完成足以支撑
“超越”结论的客户端数据面闭环、同条件竞争基准和长时间生产证据。

## 5. 已完成基础工作

下列工作不再进入新增功能队列，只保留回归门禁：

| ID | 基础能力 | 状态 | 固定门禁 |
| --- | --- | --- | --- |
| F00 | API/RPC 统一服务依赖图与 `service sync --check` | 已实现 | 首次 add、update、remove、失败回滚、字节确定 |
| F01 | API/OpenAPI/Search 破坏性合同 diff | 已实现 | 稳定路径级诊断和退出码 |
| F02 | SQL 迁移风险分类 | 已实现 | drop、rename、narrow、nullability、constraint、index、lock、rewrite |
| F03 | 迁移/回滚确认门禁 | 已实现 | hash、owner、reason、expiry 缺失或过期即失败 |
| F04 | 全生成目标矩阵 | 已实现 | REST、RPC、stream、model、search、OpenAPI、TS、JS |
| F05 | REST/RPC/Gateway/MQ/Job 统一治理模型 | 已实现 | 共享策略解析和有界标签 |
| F06 | 生命周期与优雅退出契约 | 已实现 | startup、ready、drain、shutdown、failed task |
| F07 | MQ envelope、retry、DLQ、outbox/inbox | 已实现 | 单元与短时真实依赖恢复测试 |
| F08 | 报表、图表和 CSV/XLSX 异步导出 | 已实现 | SQLite 集成、权限、租户、取消、过期 |
| F09 | Gateway 和 Config Center 治理 | 已实现 | smoke、热更新、回滚、SSE、WebSocket |
| F10 | 安全契约 | 已实现 | OIDC/OAuth2、mTLS、JWT 轮换/撤销、脱敏 |
| F11 | 三类生成参考系统及运维资产 | 已实现 | 重新生成、编译、依赖脚本和资产检查 |
| F12 | 固定 runner 证据生成/晋级/校验链 | 已实现 | revision、checksum、artifact digest、attestation |

## 6. 权威执行计划

只按下表顺序推进。除非直接关闭一个退出条件，否则不新增 crate、生成语言或孤立
功能。

| 阶段 | 目标 | 依赖 | 当前状态 | 退出条件 |
| --- | --- | --- | --- | --- |
| S0 | 固定竞争基线与可复现实验协议 | F00-F12 | 未完成 | 固定双方 revision、工具链、机器、依赖镜像、数据集、工作负载、SLO 和报告 schema |
| S1 | 下游客户端数据面闭环 | S0 | 未完成 | P2C 使用实时 EWMA、在途负载和结果反馈；取消/超时不泄漏状态；异常实例可剔除并恢复 |
| S2 | 跨边界上下文、取消和低基数证明 | S1 | 未完成 | REST→RPC→DB/cache/MQ 测试证明上下文传播、重试放大有界、资源 permit 全释放、标签基数有界 |
| S3 | 真实依赖参考系统与恢复演练 | S2 | 证据待补 | 三类 freshly generated 系统在真实 DB、Redis、Etcd/Consul、NATS/Kafka、Search 上通过成功与故障流程 |
| S4 | Roze/go-zero 同条件竞争基准 | S3 | 未完成 | 发布可复现报告；Roze 在约定的核心指标和资源约束上达到下述胜出规则 |
| S5 | 24h/72h 长稳与故障注入 | S3-S4 | 证据待补 | 五个关键区域均产生可晋级、可独立验证的通过报告 |
| S6 | 发布候选与结论审计 | S5 | 未完成 | release gate、证据 gate、安全供应链 gate 全通过；差异矩阵逐项有证据链接 |

### S0. 固定基线

产物：

- `benchmarks/competitive/baseline.yaml`：双方 revision、Rust/Go 版本、OS、
  CPU、内存、内核参数和镜像 digest；
- 同一套 REST、RPC、REST→RPC、cache-aside、DB CRUD、MQ/outbox 场景；
- 固定请求/响应大小、数据分布、连接池、并发阶梯和预热时间；
- JSON 原始结果和 Markdown 摘要；
- 禁止只挑选对 Roze 有利的场景。

退出条件：

- 任意维护者可用一个命令复现实验；
- 双方服务对外语义、依赖和 SLO 相同；
- 比较生成后二进制，而不是手工优化样例。

### S1. 下游客户端数据面闭环

实现范围：

- 当前 `connect_channel_from_config` / `connect_via_registry_with_options`
  只在建立 `Channel` 时选择一次实例；必须改为每次逻辑 RPC 调用及其真实重试前选择，
  不能把“连接时 P2C”当作“调用时 P2C/EWMA”；
- 将 `roze-rpc` P2C 从 registry metadata 打分升级为实时调用状态；
- 每个实例维护 EWMA latency、in-flight、success/error/timeout 和最近样本；
- picker 返回带完成句柄的 lease，pick、done、cancel/drop 形成完整生命周期；
- breaker、outlier ejection、active health 与 registry churn 协同；
- retry 前重新发现和选择实例，但不能突破传播的 retry budget/deadline；
- 以新增兼容接口和内部 adapter 扩展现有 `Balancer`，不破坏 Roze 1.x 已公开的
  `Balancer::pick`、配置枚举或生成客户端调用面；
- 状态以逻辑 service + 稳定 instance identity 为键，实例离开注册表后按有界
  grace period 清理，不能形成永久状态表；
- 对高并发状态表增加 Criterion 基准和基数上限。

退出条件：

- 慢节点、超时节点、连接失败节点和恢复节点的确定性测试通过；
- cancellation、panic、deadline 和连接失败均归还 in-flight；
- 每次完成只结算一次，重试的每个真实 attempt 独立结算，未发出的 attempt 不计数；
- endpoint churn 后状态条目在规定 grace period 内回收；
- 在混合快慢节点场景，p99 和错误率不劣于固定 go-zero 基线；
- 控制面不可用时，已缓存实例的数据面仍能按策略工作。

### S2. 跨边界正确性

建立一个生成的端到端测试：

```text
REST -> managed RPC -> DB/cache -> outbox -> MQ consumer
```

测试必须证明：

- W3C trace、deadline、cancellation、tenant、subject、locale、
  idempotency key 和 retry budget 不丢失；
- 客户端断开会取消下游工作；
- breaker probe、shedding permit、stream capacity、DB connection 和后台任务
  均被释放；
- 任意 URL、ID、tenant 和错误消息不能制造无界 metrics label；
- 重试总次数受全链路预算而非每跳独立预算约束。

### S3. 真实参考系统

持续生成并执行：

1. REST CRUD + PostgreSQL/MySQL + migration/rollback + Redis cache；
2. REST + RPC + discovery + tracing + governed client；
3. Gateway + Registry + MQ + outbox/inbox + TCC/Saga + object storage。

每套系统执行 startup、readiness、dependency loss、timeout、duplicate event、
retry exhaustion、DLQ replay、config rollback、graceful drain、migration rollback、
backup/restore 和 update regeneration。

退出条件是 CI 首次从空目录生成、编译、部署、注入故障、恢复并清理成功，且每次
故障都能由生成的 metrics、logs、traces 和 runbook 解释。

### S4. 同条件竞争基准

基准分为三层：

- 微基准：router、middleware、P2C、breaker、metrics、cache、singleflight；
- 服务基准：REST、RPC、REST→RPC、DB/cache、MQ；
- 故障基准：慢实例、实例抖动、注册中心中断、broker 重启、配置回滚。

胜出规则：

- 所有正确性与恢复 SLO 必须先通过，不能以吞吐换错误；
- 核心服务场景至少 70% 的加权指标胜出；
- 任一场景不得出现超过 10% 的 p99、错误率或内存回退；
- Roze 额外生成能力必须通过 release gate，而不是仅列功能清单；
- 原始数据、命令、配置、火焰图和环境信息全部归档。

若结果未达到规则，报告必须写“未超越”，并将差距返回 S1-S3，禁止调整口径隐藏
失败。

### S5. 24h/72h 生产证据

使用现有固定 runner 和证据晋级链，完成：

- Gateway：真实 HTTP/SSE/WebSocket、Etcd、Consul 故障与恢复；
- MQ：内存语义 + NATS JetStream + Kafka 硬重启；
- Config Center：签名发布、监听、Etcd 中断、回滚；
- Lifecycle：启动、反向 drain、失败任务、超时 hook、泄漏检查；
- Generated systems：三类参考系统的真实依赖长稳运行。

短时 30 秒或 5 分钟运行只验证 harness，不能晋级为通过证据。24h/72h 报告必须
绑定完整 Git revision、真实 elapsed duration、主机采样、边界摘要、checksum、
artifact digest 和 GitHub attestation。

### S6. 发布与结论

最终发布必须同时通过：

```bash
bash scripts/release-gate.sh
bash scripts/reference-systems-integration.sh
bash scripts/production-evidence-gate.sh
```

并完成：

- `cargo fmt`、Clippy、全部非外部条件测试；
- 生成目标矩阵；
- API/SQL/Search/部署 diff gate；
- RustSec、许可证和依赖来源策略；
- Windows 预检与 Linux 权威发布路径；
- `docs/maturity.md`、`docs/production-evidence.md` 和本计划状态同步。

只有审计表中每个“超越”条件都链接到可复现实物，README 才能使用“生产生成能力
超越 go-zero”的表述。在此之前，只能表述为“以超越 go-zero 为目标”或“在某个
已验证维度领先”。

## 7. 代码落点与交付物

### 7.1 代码落点

| 阶段 | 主要落点 | 要求 |
| --- | --- | --- |
| S0 | `benchmarks/competitive/`、`example/competitive/` | 固定 schema、双方生成输入、镜像 digest、runner metadata 和原始结果格式 |
| S1 | `crates/roze-rpc/src/balance.rs` | 实时 picker、EWMA、in-flight、完成 lease、有界实例状态 |
| S1 | `crates/roze-rpc/src/rpc.rs` | 每调用/每 attempt 选址、deadline/retry budget、完成结算 |
| S1 | `crates/roze-rpc/src/registry.rs` | watch/cache/churn 与 picker 状态生命周期协同 |
| S1 | `crates/roze-config/src/lib.rs` | 仅在确有必要时增加向后兼容的调优字段与默认值 |
| S1 | `crates/roze-rpc/benches/` | pick、done、churn、并发状态更新和回收基准 |
| S1-S2 | `apps/rozectl/src/generator/rpc.rs`、`generator/mod.rs` | 生成客户端使用受治理的每调用路径；更新模板测试 |
| S2 | `example/production-systems/service-mesh/` | REST→RPC→DB/cache→outbox→MQ 权威输入 |
| S2-S3 | `scripts/generated-reference-systems.sh`、`reference-systems-integration.sh` | 从空目录生成、编译、部署、故障注入、恢复、清理 |
| S4 | `scripts/competitive-benchmark.sh` | 单一入口执行 Roze/go-zero 基准并输出机器可读结果 |
| S4 | `.github/workflows/competitive-benchmark.yml` | 固定 runner、禁止浮动依赖、上传原始和摘要 artifact |
| S5 | `.github/workflows/production-soak.yml`、`scripts/production-soak-*.sh` | 复用现有证据链，不另建无法晋级的旁路报告 |
| S6 | `scripts/release-gate.sh`、`docs/maturity.md`、`docs/evidence/` | 发布阻断、成熟度同步和最终证据索引 |

生成输出发生变化时，必须修改 `apps/rozectl` 的 generator/template/test，不能手工
修改 `target` 或临时生成项目中的 glue。业务流程仍放在生成的应用扩展点。

### 7.2 可关闭任务清单

| Task | 阶段 | 交付物 | 完成判据 |
| --- | --- | --- | --- |
| T001 | S0 | `baseline.yaml` 和 JSON Schema | 双方 revision、工具链、硬件、依赖 digest 均为必填 |
| T002 | S0 | 等价 REST/RPC/cache/DB/MQ 输入 | 请求语义、数据集、连接池、超时和 SLO 可机器校验 |
| T003 | S0 | benchmark runner contract | 同一命令可跑双方，失败不会产生 `pass` 摘要 |
| T101 | S1 | 每调用 picker/lease 契约 | 保持现有 `Balancer::pick` 兼容，新增路径可追踪 completion |
| T102 | S1 | EWMA + in-flight + success 状态 | 并发安全、无负 in-flight、时间计算饱和且无溢出 |
| T103 | S1 | registry churn 状态回收 | add/remove/re-add、watch 丢失和缓存过期测试通过 |
| T104 | S1 | 每 attempt 重选与结算 | timeout、cancel、panic、connect error、gRPC status 全覆盖 |
| T105 | S1 | 指标、日志、trace 和 Criterion | 无 endpoint/tenant/error 动态 metric label，基准可重复 |
| T201 | S2 | 生成端到端上下文 fixture | 八类标准上下文逐边界一致 |
| T202 | S2 | 取消与资源释放 fixture | permit、lease、connection、task 在所有出口归还 |
| T203 | S2 | retry amplification fixture | 总 attempt 不超过传播预算，backoff 不越过 deadline |
| T204 | S2 | cardinality adversarial fixture | 随机 path/ID/tenant/error 不增加无界 time series |
| T301 | S3 | REST CRUD 真实依赖流程 | migration、cache、backup、restore、rollback 全通过 |
| T302 | S3 | REST→RPC 真实发现流程 | Etcd/Consul loss、slow node、恢复和 drain 全通过 |
| T303 | S3 | event-commerce 真实事件流程 | broker restart、duplicate、DLQ replay、TCC/Saga 不重复副作用 |
| T401 | S4 | 微基准套件 | router、governance、picker、cache、metrics 均输出统一 schema |
| T402 | S4 | 服务/故障基准套件 | 五类服务场景和五类故障场景可重复 |
| T403 | S4 | 竞争报告 verifier | 缺样本、环境漂移、SLO 失败或方差过大均拒绝结论 |
| T501 | S5 | 24h 五区域报告 | 全部由固定 runner 晋级并独立验证 |
| T502 | S5 | 72h 关键区域报告 | Gateway、MQ、Config Center、Generated systems 通过 |
| T601 | S6 | 最终差异审计表 | 每个结论链接代码、测试、原始 artifact 和报告 |
| T602 | S6 | 发布候选 | release/evidence/supply-chain gate 同一 revision 全绿 |

任务状态只能是 `todo`、`in-progress`、`implemented`、`evidence-pending`、
`verified` 或 `blocked-external`。`implemented` 不能自动视为 `verified`。

### 7.3 当前执行台账（2026-07-18）

| Task | 状态 | 当前证据 | 尚缺条件 |
| --- | --- | --- | --- |
| T001 | `implemented` | `benchmarks/competitive/baseline.yaml`、Schema 和严格校验器 | 固定 Linux runner 上解析并归档六个真实镜像 digest |
| T002 | `in-progress` | 六场景共享 API/proto、100,000 行 DB seed、inbox/outbox/effect SQL、事件 Schema、payload 计量口径、托管 RPC 接线和双方应用覆盖均进入单一 digest；固定 revision 的 Roze/go-zero REST+RPC 已从空目录生成并编译；共享 1024-byte 探针验证双方直接 gRPC、REST echo、REST→RPC echo 真实进程链路均通过 | DB/cache 与 MQ 应用覆盖、跨进程 context 正确性探针、Linux 可执行镜像 |
| T003 | `implemented` | 双适配器改为单次 shared pair executor；pair schedule manifest、样本/报告 verifier 拒绝缺字段、CV/SLO/环境漂移、>10% 回退和伪成功；吞吐由原始计数/CPU 时间推导，独占样本近邻配对且执行顺序均衡 | 固定 Linux runner 上接入真实 executor |
| T101 | `implemented` | `AttemptLease`、兼容的 `Balancer::pick`、生成客户端逐 attempt 选址 | 固定 runner 竞争证据 |
| T102 | `implemented` | latency/success EWMA、in-flight、RAII 结算及确定性测试 | 故障基准与长稳状态上限证据 |
| T103 | `implemented` | stale grace prune、动态 watch 接线及 add/remove/re-add 测试 | 真实 Etcd/Consul 中断与恢复证据 |
| T104 | `implemented` | retry 闭包内重新选址；success/failure/timeout/cancel/connect error 结算；task panic 的 RAII lease 释放测试通过 | 真实慢节点/恢复节点集成证据 |
| T105 | `implemented` | 无 endpoint 标签的 attempt 指标、Criterion pick/churn 基准 | 固定 Linux runner 原始样本和 go-zero 对照 |
| T201 | `implemented` | request/trace、deadline、共享 cancellation、tenant/subject、locale、idempotency、retry budget 已覆盖 HTTP/RPC/MQ/NATS/outbox round-trip | S3 生成服务真实进程多跳证据 |
| T202 | `in-progress` | task abort 会 Drop attempt lease 并归还 in-flight；method/shedding guard、stream capacity 生命周期已有释放测试；QueryComposer 总超时会等待全部任务 shutdown，Drop 计数证明返回前资源归零 | 真实 DB connection pool 与生成系统进程级断连/后台任务 fixture |
| T203 | `implemented` | 请求级 retry budget 双向 metadata；生成客户端原子划拨而非复制额度，响应返还不超过划拨值，缺失响应保守消耗；并发 fan-out 守恒、伪造返还、deadline 与生成编译门禁通过 | S3 真实多进程 fan-out 故障证据 |
| T204 | `implemented` | 1000 个动态 tenant/path/ID/error outcome 归一后只产生 1 条 RPC attempt series；endpoint 不作为标签 | S3 生成服务 metrics 抓取与全边界 series-count 证据 |
| T301-T303 | `in-progress` | 三套权威输入从空目录生成并编译；参考集成脚本覆盖动态端口、DB/cache/search/broker/registry/MinIO 成功与断连恢复，并生成 run.json/SHA256SUMS | Linux/Docker 真实依赖启动、业务接口、故障与恢复产物 |
| T401 | `in-progress` | picker pick/finish 与 churn Criterion 基准已存在 | router、governance、cache、metrics 统一原始 schema 与固定 Linux 样本 |
| T402 | `in-progress` | 固定 revision source builder 从 go-zero checkout 自编译 goctl，锁定 Rust/Go/protoc 与双方依赖图；pair runner 现在要求单次 shared executor、schedule manifest 和六场景配对；Windows manifest 仍明确 `evidenceEligible=false` | DB/cache、MQ、context/fault correctness runner 与 Linux/Docker executor |
| T403 | `implemented` | 输入校验器 8 个正反例、报告器 10 个正反例通过；报告器执行几何优势/场景数/回退规则，并输出 median、MAD、CV、95% CI | 固定 Linux runner 原始样本 |
| T501-T602 | `todo` | 仅复用仓库既有基础和旧证据 | 按阶段退出条件取得新 revision 的权威实物 |

本台账中的本机测试只证明实现、模板和前三条等价语义链路闭环；Windows 微基准、
短时 smoke 和旧 revision 报告均不能替代 S4 竞争结论或 S5 24h/72h 证据。

## 8. 竞争基准协议

### 8.1 固定环境

- 使用同一台裸机或同一固定 runner，不允许一方运行在共享云机器、另一方运行在
  独占机器；
- 固定 CPU governor、CPU affinity、内存上限、文件描述符、内核和网络参数；
- 固定 Rust、Go、protoc、数据库、Redis、Etcd/Consul、NATS/Kafka 和 Search
  镜像 digest；
- release build，关闭 debug log；双方保留生产所需的 metrics/tracing；
- 每个场景至少 1 次预热和 5 次测量，记录全部样本，不只保留最好结果；
- 报告 median、p95/p99、MAD/CV 和置信区间；CV 超过 10% 时结果为
  `inconclusive`，必须重跑或解释噪声。

### 8.2 场景与权重

| 场景 | 权重 | 主指标 | 约束指标 |
| --- | ---: | --- | --- |
| REST JSON | 15 | SLO 内 throughput/core | p99、错误率、RSS |
| unary RPC | 20 | SLO 内 throughput/core | p99、错误率、RSS |
| REST→RPC | 20 | end-to-end p99 | throughput/core、重试放大 |
| DB + cache-aside | 15 | p99 和 DB QPS | cache hit、连接、正确性 |
| MQ + outbox/inbox | 15 | confirmed throughput | duplicate effect、lag、恢复 |
| registry/slow-node fault | 15 | recovery time | error budget、p99、状态回收 |

正确性、数据一致性、安全隔离和恢复 SLO 是先决 gate，不参与性能加分。任一先决
gate 失败，整个场景得分为零且最终结论不得为“超越”。

### 8.3 胜出计算

每个性能指标先按“越大越好”或“越小越好”归一化为
`Roze / go-zero` 优势比，再按场景权重计算几何平均，防止一个极端高分掩盖严重
回退。

最终“超越”要求同时满足：

- 所有先决 gate 通过；
- 加权几何平均优势至少 1.10；
- 至少四个场景优势比不低于 1.00；
- 任一场景 p99、错误率、峰值 RSS 或恢复时间不得比 go-zero 回退超过 10%；
- Roze 独有的生成、合同门禁和证据能力通过独立功能审计。

同一 revision 的原始 JSON、摘要、环境清单和 verifier 结果必须一起归档。修改
权重或阈值属于证据策略变更，必须在运行前评审，不能在看到结果后调整。

## 9. 可观测与状态上限

S1-S4 至少生成以下低基数指标；名称最终以现有 Roze 指标约定审查为准：

| 指标语义 | 允许标签 | 禁止标签 |
| --- | --- | --- |
| client picks | `service`、`outcome` | endpoint、instance id、tenant |
| client attempts | `service`、`operation`、`outcome` | raw method/path、error text |
| client latency | `service`、`operation`、`outcome` | request id、trace id |
| client inflight | `service` | endpoint、subject |
| registry refresh | `service`、`source`、`outcome` | raw registry key |
| outlier decisions | `service`、`decision`、`reason` | address、exception message |

实例级 address、EWMA、in-flight 和 eject-until 进入结构化 debug log、trace event
和有界 admin snapshot，不进入 Prometheus 标签。

状态上限：

- picker 状态条目不得超过“当前发现实例 + grace period 内离开实例”的有界集合；
- service key 必须来自生成依赖图或已验证配置，拒绝请求输入构造的任意 key；
- state map、watch task、refresh task、breaker、retry budget 和 outlier state 均要有
  删除路径和 churn 测试；
- 24h/72h 报告必须记录这些状态表的起点、峰值和终点。

## 10. 完成定义、回退与风险

### 10.1 Definition of Done

每个任务只有在适用项全部满足后才能关闭：

1. runtime/generator 实现；
2. 单元、并发、取消、panic 和故障路径测试；
3. freshly generated project 编译和端到端 smoke；
4. 配置、API、生成所有权和失败语义文档；
5. 低基数 metrics、结构化 logs 和 trace；
6. 兼容性评审、迁移和回滚说明；
7. release gate 接线；
8. 要求外部依赖或长稳运行时，附可验证 evidence artifact。

### 10.2 兼容和回退

- Roze 1.x 不破坏现有 `Balancer`、`RpcClientConfig` 和生成客户端公共调用面；
- 新实时 picker 先以内部实现或新增兼容接口接入，并提供显式旧策略回退；
- 新默认值启用前运行生成 diff、配置兼容和性能回归 gate；
- hot reload 必须保留 picker 统计和连接池，失败 reload 保留最后有效 runtime；
- 发现严重回归时回退到已验证的上一策略，不能关闭 timeout/breaker/retry budget
  来换取性能。

### 10.3 风险登记

| 风险 | 影响 | 缓解 |
| --- | --- | --- |
| tonic Channel 在连接时固定选址 | 无法实现每调用 P2C | 在生成/受治理客户端层建立每 attempt picker 与 completion hook |
| 新 picker 破坏 1.x API | 下游升级失败 | additive adapter、默认兼容、生成 diff 和升级测试 |
| endpoint churn 导致状态泄漏 | 长稳内存增长 | 稳定 identity、grace TTL、容量上限、回收测试 |
| benchmark 偏向某语言 | 结论失真 | 等价语义、同依赖、同 SLO、多层基准、公开原始数据 |
| tracing/metrics 配置不等价 | 性能比较无效 | 双方保留同等级生产遥测并纳入 baseline schema |
| Windows 本地通过但 Linux 失败 | 发布门禁失真 | Windows 预检，Linux/Docker 为权威结果 |
| 外部 runner 或 SSH 不稳定 | 长稳证据无法晋级 | workflow artifact 为事实源，断连不手工补报告 |

## 11. 执行优先级

立即工作顺序：

1. S0：冻结双方 revision 和竞争实验协议；
2. S1：完成实时 EWMA/in-flight P2C 客户端闭环；
3. S2：完成跨 REST/RPC/DB/cache/MQ 的取消和上下文证明；
4. S3：在 Linux/Docker CI 取得真实参考系统通过产物；
5. S4：运行并发布同条件竞争基准；
6. S5：取得 24h/72h 签名证据；
7. S6：完成发布候选审计。

当前最高优先级不是增加功能，而是把已有生成能力变成可比较、可恢复、可长时间
验证的生产证据。

首个实施批次应关闭 `T001-T003` 和 `T101-T105`。在这些任务完成前，不启动
24h/72h 正式运行，因为旧客户端选址行为会使长稳结果无法证明最终架构。

## 12. 外部依赖与阻塞

以下事项需要维护者凭据或外部系统，仓库只能生成并验证流程：

- GitHub Actions、自托管 Linux/Docker runner 和 artifact attestation；
- crates.io owner、签名 tag 和公开发布；
- 24h/72h 独占执行窗口；
- 与生产相近的数据库、注册中心、broker、search 和对象存储拓扑。

外部阻塞不能被记录为通过，也不能通过手工修改 evidence report 绕过。阻塞解除
后从对应阶段继续，不回退已通过的确定性仓库门禁。
