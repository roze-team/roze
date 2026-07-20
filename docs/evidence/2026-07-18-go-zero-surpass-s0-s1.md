# 2026-07-18 go-zero 超越计划 S0-S2 实施证据

## 结论

本批次完成了 S0 的可校验基线合同骨架、S1 的每真实 RPC attempt
P2C/EWMA 数据面，以及 S2 的请求级预算与资源释放实现。状态为
`implemented` / `evidence-pending`，
不能据此宣称整体性能或生产生成能力已经超越 go-zero。

## 绑定 revision

- Roze 基线：`d73f4ff01ea6d128b98d6e5c5b2b1166ebc266ab`
- go-zero 基线：`6a6b81ef20d5697f4fbe9c2a92c436e85d687be4`
- 工作区包含未提交实现，因此最终竞争和长稳证据必须重新绑定实现提交后的完整 revision。

## 已验证事项

- 竞争基线校验：六个场景，权重合计 100；严格模式拒绝缺失镜像 digest。
- `roze-rpc`：每 attempt 选择、EWMA latency、in-flight、结果回写、RAII cancel、
  stale grace 回收、registry watch、TTL/stale fallback 和动态 Channel 缓存。
- 生成客户端：真实 retry 闭包每次重新发现/选择，生成工程通过 `cargo check`
  与 `cargo clippy`。
- 指标：`service`、`method`、固定 outcome 标签；不包含 endpoint、tenant、ID
  或错误正文。
- 跨边界修复：标准 `Idempotency-Key` 可由 HTTP 入口进入 `Context`，由 RPC
  客户端发出并由下游恢复。
- 请求级 retry budget 通过双向 metadata 传播，只在真实 retry 前消费；生成客户端
  对并发下游原子划拨额度，response/status 仅能返还未用且不超过划拨值的额度，
  缺失响应保守消耗；task abort 会释放 attempt lease。
- Context fork 共享 cancellation；MQ、NATS 和 outbox→MQ fixture 验证 identity、
  locale、deadline、idempotency key 与 retry budget round-trip。
- 竞争样本/报告 verifier 的 10 个正反测试通过；三套 production reference
  systems 从空目录生成并编译通过。
- 修复 5 份既有损坏 UTF-8 文档；仓库文本 verifier 扫描 334 个源码文件通过，
  3 个非法字节/replacement-character 正反例通过并接入 release gate。
- `roze-admin` 的有界 reload history、Bearer 鉴权、API key 与未知路由测试
  3/3 通过；文档已与当前仅暴露 config reload HTTP endpoint 的实现对齐。

## 执行记录

```text
node scripts/competitive-baseline-verify.js
competitive baseline valid: 6 scenarios, weight=100

cargo test -p roze-rpc
63 passed; 0 failed; 2 ignored (需要真实 Etcd/Consul)

cargo test -p roze-metrics
12 passed; 0 failed

cargo test -p roze-context
11 passed; 0 failed

cargo test -p roze-query
5 passed; 0 failed

cargo test -p roze-gateway stream_connection_limit_releases_capacity_with_body_lifecycle
1 passed; 0 failed

cargo test -p roze-admin
3 passed; 0 failed

cargo clippy -p roze-rpc --all-targets -- -D warnings
passed

cargo check -p roze-rpc --benches
passed

cargo test -p rozectl generator::tests::generated_rpc_project_compiles -- --ignored --nocapture
passed (生成完整 RPC 工程后执行 cargo check + clippy)

cargo test -p rozectl -- --skip postgres --skip mysql --skip mongo
234 passed; 0 failed; 10 ignored (lib)
29 passed; 0 failed (CLI)

cargo test -p rozectl generated_production_reference_systems_compile -- --ignored --nocapture
passed (三套权威输入、五个生成 crate)

node scripts/competitive-verifier.test.js
10 passed; 0 failed

node scripts/text-utf8-verify.js
repository text UTF-8 valid: 334 files

node scripts/text-utf8-verifier.test.js
3 passed; 0 failed
```

### 双框架共同输入与新鲜生成补充

- 共同输入 digest 由 `competitive-input-verify.js --digest` 计算并与每次 fresh
  build manifest 强制一致；`full-015` 与共享 smoke 均为
  `sha256:85b50f289dc3d9810bc3c7d96ddbd72da3a97784abb45145dd6cf63bf6f46fee`。
- 固定 `go-zero@6a6b81e…` checkout 自编译 goctl；固定 `protoc 31.1`。
- Roze 与 go-zero 的 REST、RPC 均从空目录生成并编译；Roze REST 通过
  `roze-service.yaml` 托管 RPC 依赖和 `service sync --check`。
- 生成链路发现并修复 Proto `bytes` 类型、完整 package 名和 REST 裸响应选择问题；
  `rozectl` 门禁为 234 lib tests + 29 CLI tests 全通过。
- REST echo、unary RPC echo、REST→RPC echo 的双方应用覆盖均进入共同 digest；
  fresh build `full-015` 编译通过。共享探针随后启动双方真实服务进程，并验证直接
  gRPC、`/v1/echo`、`/v1/rpc-echo` 都原样返回 1024-byte payload。
- Roze 临时 workspace 保留仓库 lock 版本，go-zero REST/RPC 均固定到基线源码的
  `grpc 1.80.0` 与 `protobuf 1.36.11`。
- 结构 manifest 仍明确写入 `semanticsReady=false`、`evidenceEligible=false`；
  DB/cache、MQ、跨进程 context、故障恢复和真实依赖探针未完成，不能据此宣称胜出。

本机 Windows Criterion 快速样本：

```text
ewma_p2c_pick_and_finish_64  9.8905–9.9418 us
ewma_p2c_churn_64            13.231–13.568 us
```

该样本只用于发现局部回归，不满足固定 Linux runner、双方同机、固定 digest、
五样本和 CV 门槛，因此不得进入 S4 胜负计算。

## 未关闭风险

- 双方 adapter/verifier 与前三个 echo 场景的 freshly generated 编译和真实进程
  语义 smoke 已实现，但固定 Linux runner 的共享 benchmark executor、DB/cache、
  MQ 与故障场景尚未接入。
- competitive verifier 的 10 个正反例已通过；它会重算 throughput-per-core 与
  confirmed throughput，拒绝不可能 CPU 时间、重叠样本、非近邻配对和执行顺序
  偏置，并严格执行加权几何优势 1.10、四场景不落后及 10% 回退上限。
- 动态 registry 路径的 watch 与 TTL/stale fallback 已接线，但仍缺真实
  Etcd/Consul 中断、watch 重连和恢复产物。
- S2 的请求级 retry budget、RPC task-abort lease、gateway stream capacity 与
  QueryComposer 总超时 shutdown 释放已实现并有测试；真实 DB connection pool、
  生成系统进程级断连/后台任务 fixture 尚未完成。
- S3 真实依赖、S4 同条件竞赛、S5 24h/72h 与 S6 发布审计均未执行。
  本机没有 Docker，未产生任何真实依赖通过结论。
# Follow-up correctness fix (2026-07-20)

`roze-rpc::apply_request_context` now clamps a live sub-millisecond
remaining deadline to `1ms` before encoding timeout metadata. This prevents
integer truncation from turning a live downstream request into an immediate
deadline failure. The boundary helper has regression coverage in the full
`roze-rpc` test suite (64 passed, 2 ignored).
