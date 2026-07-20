# Roze 超越 go-zero：执行分解与验收矩阵

本文是 [`go-zero-surpass-plan.md`](./go-zero-surpass-plan.md) 的执行附录。
主计划定义“超越”的边界；本文把边界拆成可并行、可追踪、可回退的工作包，
并明确每个阶段必须提交的代码、测试和证据。若主计划与本文冲突，以主计划的
固定 revision、兼容性要求和证据边界为准。

## 1. 阶段总览

| 阶段 | 目标 | 预计顺序 | 退出门槛 | 主要产物 |
| --- | --- | --- | --- | --- |
| P0 基线 | 固定 Roze/go-zero revision、机器和负载协议 | 先行 | 任意维护者可一键复现 | `baseline.yaml`、输入数据、运行脚本 |
| P1 客户端闭环 | 每次 RPC 调用实时 P2C/EWMA、重试预算和熔断 | P0 后 | 慢实例/故障实例场景通过，in-flight 无泄漏 | `roze-rpc` 实现、并发/取消测试、基准 |
| P2 全链路语义 | REST→RPC→DB/cache→outbox→MQ 上下文和取消一致 | P1 后 | trace/deadline/tenant/idempotency 可追踪且资源回收 | 参考系统、故障注入脚本、trace 样本 |
| P3 真实依赖 | PostgreSQL/Redis/registry/broker/search 参考系统稳定运行 | P2 后 | 启停、断依赖、恢复、迁移回滚全通过 | compose、smoke、恢复报告 |
| P4 竞争基准 | 与固定 go-zero 在同机同依赖下比较 | P3 后 | 正确性先通过，核心指标达到胜出规则 | 原始 JSON、统计摘要、图表 |
| P5 长稳证据 | 24h/72h Gateway、MQ、Config、Lifecycle 和生成系统 | P3/P4 后 | 签名 artifact、校验和、异常处置记录齐全 | evidence report、runbook、attestation |
| P6 发布审计 | 汇总 release/contract/security/evidence gates | 最后 | 所有 gate 绿；未通过项显式阻断发布 | RC 清单、差异报告、回滚包 |

阶段必须按顺序晋级；P4/P5 不能用功能数量或短时 smoke 替代前置正确性门槛。

## 2. 工作包与验收矩阵

### P0：可复现实验基线

- 冻结双方 Git revision、Rust/Go 版本、OS/内核、CPU/内存、容器镜像 digest。
- 固定 REST CRUD、REST→RPC、DB/cache、MQ/outbox 四类等价场景；记录请求体、
  响应体、并发阶梯、连接池、预热时间和 SLO。
- 脚本必须产生原始 JSON 和环境快照；报告只能引用脚本产物，禁止手工填数。
- 退出检查：干净工作区执行一条命令可重建输入、运行双方并生成摘要；随机抽取
  一次运行结果与报告 checksum 一致。

### P1：受治理 RPC 客户端

- 在 `roze-rpc` 增加每次 attempt 的 picker lease/completion 生命周期；连接建立时
  的一次性选址不能代表调用时 P2C。
- 实例状态至少维护 EWMA 延迟、in-flight、成功/超时/连接失败/5xx、最近样本，
  并对 endpoint churn 设置 grace TTL 和容量上限。
- 重试必须受上游 deadline 与 retry budget 约束；取消、panic、连接失败均归还
  in-flight；breaker、outlier ejection 与主动健康探测协同。
- 退出检查：慢实例、故障实例、恢复实例、取消和超时测试稳定通过；Criterion
  基准记录吞吐、p99 和状态表峰值；旧 `Balancer`/`RpcClientConfig` API 回归通过。

### P2：端到端上下文与资源语义

- 生成参考系统覆盖 REST→managed RPC→DB/cache→outbox→MQ consumer。
- 验证 W3C trace、deadline、cancellation、tenant、subject、locale、idempotency
  key、retry budget 在每个边界保持；取消时释放连接、permit、stream capacity 和
  后台任务。
- 约束 metrics 低基数：URL、实例地址、异常消息仅进入 debug log/trace/admin
  snapshot，不进入 Prometheus label。
- 退出检查：重复事件只消费一次；超时不会留下 in-flight；故障注入后 traces、
  logs、metrics 能解释根因，且状态表在 grace period 后回收。

### P3：真实依赖与恢复演练

- 使用固定 compose/CI 拓扑运行 PostgreSQL/MySQL、Redis、Etcd/Consul、NATS/Kafka、
  Search 和对象存储；不得仅以 in-memory 替代生产依赖。
- 覆盖启动/readiness、依赖中断、broker 重启、DLQ replay、配置回滚、优雅 drain、
  migration rollback、backup/restore 和二次生成。
- 每次演练归档命令、输入、时间线、原始遥测、恢复时长和最终状态；失败样本保留。
- 退出检查：从空目录生成、编译、部署、注入故障、恢复并清理成功；runbook 可由
  非作者执行。

### P4：同条件竞争基准

- Roze 与 go-zero 使用等价业务语义、依赖版本、数据集、遥测等级和资源限制；
  预先注册胜出规则，禁止事后调权重。
- 正确性和恢复 SLO 为硬门槛；在硬门槛通过后比较吞吐、p50/p95/p99、CPU、内存、
  连接数、错误率和恢复时间。核心场景至少 5 次重复并报告置信区间。
- 原始结果、统计脚本、火焰图和环境快照一并发布；任何偏差须在报告中声明。
- 退出检查：核心场景加权指标胜出且无单项超过 10% 的 p99/错误率/内存回退；否则
  标记“未超越”并回到 P1–P3 修复。

### P5：长稳与证据晋级

- Gateway、MQ、Config Center、Lifecycle、三个生成参考系统分别运行 24h，再对
  关键路径运行 72h；记录 churn、重启、断依赖、配置变更和 DLQ 操作。
- 报告绑定 Roze/go-zero revision、镜像 digest、runner、起止时间、artifact checksum
  和签名；断连或缺失窗口不得手工补齐。
- 退出检查：无未解释的数据丢失、重复副作用、资源单调增长或恢复超时；每个异常
  都有 issue、runbook 和复测链接。

### P6：发布审计与回退

- release gate 同时执行格式、编译、生成二次更新、契约/迁移/search diff、security、
  smoke、benchmark 和 evidence 校验。
- 生成变更必须附兼容性说明、迁移步骤、回滚命令和上一稳定策略；失败 reload 保留
  最后有效 runtime，禁止通过关闭 timeout/breaker/retry budget 换取指标。
- 退出检查：所有 gate 绿色；任何长稳证据缺失时，发布说明只能写“API 稳定、长期
  证据待补”，不得宣称 battle-tested 或已超越。

## 3. 每周执行节奏

1. 周初：更新任务状态、阻塞项、固定 revision 和 runner 可用性。
2. 开发：每个工作包先补失败测试，再实现，再接入 release gate；生成器改动必须
   更新模板和 golden test，不能手改生成输出。
3. 周末：运行最小 smoke 和 contract gate，归档原始 artifact；未通过项保留失败
   证据并回滚到上一个已验证策略。
4. 阶段评审：维护者依据本矩阵逐项勾选，记录证据链接、commit 和剩余风险；禁止
   以口头结论关闭阶段。

## 4. 状态标签与责任

任务状态只允许：`planned`、`in_progress`、`blocked_external`、`verified`、
`long_run_pending`、`long_run_verified`。`verified` 只表示代码和短时测试通过；
`long_run_verified` 必须有签名 24h/72h artifact。外部系统不可用时标记
`blocked_external`，不得改写为通过。

建议每个工作包记录：负责人、审阅人、输入 revision、验收命令、artifact 路径、
回滚策略和下一复测时间。该记录可放在 issue/项目板，但 release 报告必须复制最终
状态和证据链接。

## 5. 与现有文档的关系

- 目标、对标范围和“超越”定义：[`go-zero-surpass-plan.md`](./go-zero-surpass-plan.md)
- 简明产品路线：[`roadmap.md`](./roadmap.md)
- 模块契约与证据边界：[`maturity.md`](./maturity.md)、[`production-evidence.md`](./production-evidence.md)
- 运行与发布检查：[`production-checklist.md`](./production-checklist.md)、[`release.md`](./release.md)

## 6. go-zero parity matrix and migration track

The comparison must record capability, evidence, and ecosystem maturity as
separate columns.  A capability is not marked as surpassed until its evidence
artifact is reproducible on the pinned runner.

| Capability family | go-zero reference behaviour | Roze target | Required evidence |
| --- | --- | --- | --- |
| IDL and layering | `.api` + `goctl`; handler/logic/ServiceContext/model ownership | `.api`/`.proto`/`.ent`/search; generated boundaries plus application-owned logic | fresh generation, byte-stable second update, compile gate |
| Resilience | chained timeout, concurrency, rate limit, adaptive breaker/shedding | one policy resolver across REST/RPC/Gateway/MQ/Job with deadline and retry budget propagation | fault matrix, bounded retries, p99/error/RSS comparison |
| RPC data plane | service discovery, load balancing, tracing | per-attempt P2C + EWMA + in-flight feedback, outlier ejection and recovery | slow-node, churn, cancellation and recovery samples |
| Data and events | memory/Redis cache, singleflight, Kafka/RabbitMQ integrations | tenant scope, optimistic concurrency, cache consistency, inbox/outbox and migration gates | duplicate-event, rollback, restore and DLQ replay evidence |
| Operations | probes, metrics, tracing, profiling and deployment helpers | generated probes/SLO/dashboards/alerts/runbooks and signed evidence | immutable artifact, checksum, 24h/72h report |

The migration sample is part of P0/P1 and must remain executable:

1. Freeze a go-zero `.api` contract and generate the reference service with
   `goctl`; archive the generated tree and command manifest.
2. Map handler → `roze_http` route, logic → `src/logic/**`, ServiceContext →
   `src/application.rs`/`roze_context`, and model → generated repository plus
   an application-owned extension module.
3. Map error codes, validators, middleware order, timeout and retry settings;
   every intentional semantic difference is recorded in the compatibility
   matrix rather than hidden in generated code.
4. Run dual traffic in shadow mode, compare correctness and bounded metrics,
   then promote by route.  Keep the previous service available for rollback
   until migration and restore drills pass.

The migration gate fails when the source contract, generated ownership, error
semantics, or rollback command is missing.  Non-Web SDKs remain outside the
Roze product boundary; teams needing them must use the documented gateway or
protoc-based client path and record that decision.

## 7. Current evidence blockers

- S3 remains `evidence_pending`: the S3-compatible runtime
  `put/get/delete/stat` and SigV4 presign adapter is now wired and unit-tested,
  but real MinIO/S3 success and failure/recovery evidence has not yet been
  produced.  A health check alone must not be promoted as object-storage
  correctness evidence.  `scripts/reference-systems-direct.sh` now records
  per-dependency results against already-managed services; the 2026-07-20
  server run passed NATS JetStream, Etcd registry, Etcd config watch, Redis,
  and S3 using temporary localhost-only diagnostic services. The evidence
  profile records Redis 7.2.5 and MinIO `RELEASE.2024-06-13T22-53-53Z`; these
  are not the fixed Compose image set, so the result remains diagnostic rather
  than promotion-grade. A follow-up local fault sequence stopped and restored
  both diagnostic services; down phases failed only for the unavailable
  dependency and restored phases returned to green.
- S4 remains `evidence_pending`: the pair runner now requires one shared
  executor invocation and an emitted schedule manifest.  Until a fixed Linux
  executor produces six scenarios with exclusive adjacent and counterbalanced
  samples, no performance verdict is valid.
- S5 remains `long_run_pending`: only signed 24h/72h artifacts from the fixed
  runner can change maturity status.

## 8. Local verification boundary

The Windows development environment is useful for deterministic generator,
contract, and focused Rust tests, but it is not an evidence runner. The full
workspace gate requires Linux because `rdkafka-sys` builds bundled
`librdkafka` with a POSIX configure executable. S3/S4 integration and S5
soak runs additionally require Docker Compose and the pinned dependency
images. A Windows green subset must therefore remain `implemented` evidence,
never `verified` or `long-run verified`.

The current workstation has neither a Docker CLI/daemon nor an installed WSL
distribution. Before promoting S3--S5, provision a runner with all of the
following and record the exact versions in its artifact: Linux x86_64, Rust
stable, Node 22, Docker Engine plus Compose v2, GNU `sha256sum`, the pinned
dependency images, and the shared competitive executor. The first permitted
commands on that runner are `bash scripts/reference-systems-preflight.sh`,
`bash scripts/reference-systems-integration.sh`, and the fixed-duration soak
workflow; their raw output and checksums are the authoritative evidence.
The release workflow uploads `target/reference-systems-integration` even when
the integration job fails, so disconnect/recovery failures remain reviewable
instead of disappearing with the ephemeral Compose project.
Before upload, `scripts/reference-systems-evidence-verify.js` validates the
portable `run.json`/summary/checksum bundle; `--require-passed` is reserved for
promotion gates, while ordinary integration runs retain failed evidence for
diagnosis.
