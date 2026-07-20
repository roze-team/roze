# Competitive benchmark

This directory is the machine-readable contract for comparing generated Roze
and go-zero services. It is not evidence by itself.

`baseline.yaml` intentionally uses JSON syntax, which is valid YAML 1.2, so the
repository verifier can parse it without adding a YAML dependency. Dependency
image digests are supplied by the fixed runner through the named environment
variables. A report-capable run rejects missing or non-`sha256:` digests.

Validate the structural contract locally:

```bash
node scripts/competitive-baseline-verify.js
node scripts/competitive-input-verify.js
```

The second command verifies the shared goctl-compatible REST IDL, proto,
deterministic 100,000-row PostgreSQL seed, event schema, generation commands,
transactional inbox/outbox/effect constraints, runtime settings, correctness
gates, and all six scenario mappings. All byte counts mean application payload
bytes; JSON, HTTP, Protobuf, gRPC, NATS, and Kafka framing is measured
separately and is never counted as payload. The verifier also
computes the only accepted `workloadDigest`; raw sample producers cannot choose
or self-report a different digest.

Validate the fixed runner before collecting samples:

```bash
ROZE_COMPETITIVE_REQUIRE_DIGESTS=1 \
  node scripts/competitive-baseline-verify.js
```

The benchmark entrypoint must never emit a passing report when an
implementation runner, digest, sample, SLO result, or environment field is
missing.

Fresh structural generation and compilation is a separate, deliberately
non-performance step:

```bash
ROZE_COMPETITIVE_GO_ZERO_SOURCE=/src/go-zero \
node scripts/competitive-source-build.js
```

The source checkout must be clean and at the pinned go-zero revision. The
script builds `goctl` from that checkout, requires the pinned toolchains
including `protoc`, creates a fresh temporary Roze workspace, runs the exact
generation commands from `scenario-contract.json`, compiles both REST/RPC
outputs, and writes per-file artifact digests. Its manifest remains
`semanticsReady: false`: the digest-bound application overlays currently cover
REST echo, unary RPC echo, and REST-to-RPC echo for both implementations, while
DB/cache, MQ persistence, cross-process context, fault, and correctness probes
remain open. Therefore this structural build can never be presented as
benchmark evidence.

The fixed runner must also expose the protobuf plugins used by the pinned
go-zero generator. For the current baseline these are installed with the
matching module versions:

```bash
GOBIN=/usr/local/bin go install google.golang.org/protobuf/cmd/protoc-gen-go@v1.36.11
GOBIN=/usr/local/bin go install google.golang.org/grpc/cmd/protoc-gen-go-grpc@v1.6.2
```

Missing plugins fail the structural build before generated output is accepted.

The same digest-bound artifact can run a local semantic smoke for the three
implemented scenarios:

```bash
node scripts/competitive-echo-smoke.js /path/to/fresh-build
```

This starts both generated implementations and applies the same 1024-byte gRPC
probe and HTTP requests. A passing smoke proves only response equivalence and
process wiring; it is not a fixed-runner performance result and never changes
`evidenceEligible`.

The fixed runner supplies one executable `ROZE_COMPETITIVE_EXECUTOR`. Both
framework adapters invoke that same harness with an implementation selector:

```bash
ROZE_COMPETITIVE_EXECUTOR=/opt/roze-bench/executor \
ROZE_COMPETITIVE_OUTPUT_DIR=/var/lib/roze-competitive/run-123 \
bash scripts/competitive-benchmark.sh --run
```

`--run` performs a fail-closed preflight first. The authoritative host must be
Linux/x86_64, every dependency digest environment variable must contain a
lowercase `sha256:` digest, the shared executor must be executable, and the
output directory must be fresh (the preflight never deletes stale files).
Use a new `ROZE_COMPETITIVE_OUTPUT_DIR` for every run; reusing a directory is
rejected so a partial or older sample cannot enter a new report.

The pair runner invokes the shared executor once with `--schedule pair`; the
executor must emit `schedule.json` plus both raw documents.  This is mandatory:
separate full runs for Roze and go-zero cannot establish adjacent,
counterbalanced samples and are rejected as non-evidence.

Each implementation must emit the raw JSON contract checked by
`scripts/competitive-sample-verify.js`: pinned revision, run/environment
identity, dependency digests, fixed-runner metadata, and at least five
non-overlapping 60-second samples for all six scenarios. Throughput-per-core is
recomputed from successful request counts and measured CPU core-seconds;
confirmed message throughput is recomputed from confirmed counts and duration.
The verifier also checks sample timestamps, available CPU time, error ratios,
SLOs, duplicate side effects, recovery objectives, and coefficient of
variation; producer-provided scores or a `pass` field are not trusted.
The pair verifier additionally requires exclusive, adjacent sample pairs and a
counterbalanced Roze/go-zero execution order so persistent first-run bias
cannot satisfy the comparison contract.

`scripts/competitive-report-verify.js` then rejects mismatched run,
environment, workload, runner, or dependency metadata. Its only positive
verdict requires a weighted geometric advantage of at least 1.10, at least four
scenarios at parity or better, and no p99, error-ratio, memory, or recovery-time
regression greater than 10%. The report includes median, MAD, CV, and a 95%
confidence interval for each primary metric. Any missing/invalid evidence or a
`not-surpassed` result exits non-zero.
