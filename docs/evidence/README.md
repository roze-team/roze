# Production Evidence Reports

This directory stores reproducible long-run evidence reports.

It can also store scoped generator verification notes when they clearly state
their evidence boundary. These notes support release confidence, but they do
not replace the long-run reports required before broad production-stability
claims.

Current scoped verification notes:

- [2026-07-17 production generation verification](2026-07-17-production-generation-verification.md)
- [2026-07-09 generation verification](2026-07-09-generation-verification.md)
- [2026-07-07 generation requirements](2026-07-07-generation-requirements.md)
- [2026-07-06 generation verification](2026-07-06-generation-verification.md)
- [2026-07-04 generation verification](2026-07-04-generation-verification.md)

Generate a new report scaffold with:

```bash
bash scripts/production-evidence.sh \
  --area gateway \
  --duration 24h \
  --workload "proxy traffic with retries, fallback, rate limit, breaker, load shedding, and hot reload" \
  --failure-injection "upstream 5xx, timeout, slow response, config reload, upstream recovery" \
  --command "bash scripts/production-soak-gateway.sh --duration 24h"
```

Reports are intentionally explicit about missing data. Do not mark a report
`pass` until measurements, failure timeline, leak checks, and artifacts are
filled in.

Current soak harnesses:

```bash
bash scripts/production-soak-gateway.sh 300
bash scripts/production-soak-mq.sh 300
bash scripts/production-soak-config-center.sh 300
bash scripts/production-soak-lifecycle.sh 300
```

Gateway soak iterations execute the real network smoke workflow rather than
re-running only unit tests. Its standardized summary includes cycle
p50/p95/p99 and retry, fallback, configuration-rejection, SSE, and WebSocket
recovery counts, plus cross-cycle HTTP request count, errors, and request
p50/p95/p99. A concurrent real Etcd/Consul workload registers Gateway
upstreams once, periodically restarts both registries, and requires automatic
re-registration. Its fault, disconnect, successful-route, recovery, route-p99,
and recovery-p99 fields are mandatory in promoted evidence.

MQ soak runs the bounded in-memory ack/nack/idempotency/DLQ workload beside
real NATS JetStream and Kafka publish/receive/ack workloads. The harness
periodically stops and restarts both brokers, then merges disconnect
observations, recoveries, throughput, delivery p99, and recovery p99 into the
final boundary summary. Reports missing either real broker are rejected.

Config Center soak runs the signed publish/validation/rollback workload and a
real Etcd value/watch workload concurrently. The harness periodically stops
and restarts Etcd and merges disconnect observations, recoveries, throughput,
operation p99, and recovery p99 into the final boundary summary.

Use `ROZE_GATEWAY_SOAK_SECONDS`, `ROZE_MQ_SOAK_SECONDS`, `ROZE_MQ_SOAK_MESSAGES`,
`ROZE_CONFIG_CENTER_SOAK_SECONDS`, `ROZE_CONFIG_CENTER_SOAK_UPDATES`,
`ROZE_LIFECYCLE_SOAK_SECONDS`, and `ROZE_LIFECYCLE_SOAK_CYCLES` for 24h/72h
runs.
The wrappers use an effectively unbounded operation cap unless one of the
message/update/cycle variables is explicitly set, preventing a nominal 24h/72h
job from stopping after a small default operation count.
MQ and Config Center harnesses monitor every child workload while running; an
unexpected early exit terminates the peer workloads instead of consuming the
remaining 24h/72h runner allocation.

CI evidence uses the fixed self-hosted workflow in
`.github/workflows/production-soak.yml`. Its artifact contains raw logs, host
samples, a Markdown summary, and `SHA256SUMS`; GitHub OIDC provenance attests
the checksum manifest. A 24h/72h job must finish before its artifact can be
reviewed as production evidence.

The runner finalizes evidence even when a workload fails or ends early.
`run.json` records the terminal status, workload exit code, required and
observed elapsed time, host sample count, minimum available memory, and maximum
host task count. The command returns failure only after `summary.md` and
`SHA256SUMS` have been written, so failed runs remain diagnosable and attestable.
Each bundle also contains `runner.json`, a snapshot of the fixed Linux
runner's OS, kernel, architecture, Rust/Cargo/Node/Docker/Compose and
checksum-tool versions.
The last standardized `roze_*_soak` line is also stored in
`boundary-summary.txt`. Generated-system runs report iteration throughput and
p50/p95/p99 workflow duration; protocol-level latency and throughput remain
separate required measurements.

The checksum manifest uses artifact-relative paths, so it remains verifiable
after download. A passing report must be generated with
`scripts/production-evidence-promote.sh`; manually changing an inconclusive
scaffold to `pass` does not satisfy the maturity gate.

For lifecycle reports, keep the `roze_lifecycle_soak` summary line in the
evidence artifact. It records elapsed time, cycle throughput, p50/p95/p99 cycle
latency, injected failed-task and drain-timeout detections, fault-detection
p99, cycles, worker exits, stop hooks, observed running/stopped snapshots, and
max service count. The report scaffold includes a lifecycle snapshot table
automatically when generated with `--area lifecycle`; pass the complete
fourteen-field numeric summary line with `--lifecycle-summary "..."` to prefill
it. The script rejects inconsistent lifecycle counts before writing the report.

For the S6 handoff, run `bash scripts/production-release-audit.sh` after the
release, evidence, and supply-chain checks. Its JSON output is the state
machine snapshot: each area is `pending` or `verified`, and a verified report
must be promoted for the exact audited Git revision. Add `--require-long-run`
when the release text intends to claim battle-tested runtime behavior.
