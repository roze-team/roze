# Production Generation Verification

Date: 2026-07-18

This record covers the local verification of the current Roze production
generation work. It is not a 24h/72h stability report and does not promote
long-run maturity.

## Passed

- `cargo fmt --all -- --check`
- 220 `rozectl` generator tests, with 10 intentionally ignored integration
  tests
- 8 service dependency synchronization tests, including first dependency add
  and manifest/Cargo/ServiceContext rollback on failed add and sync
  with an existing path dependency
- REST, RPC, stream, and HTTP generated compile fixtures
- generated production reference-system compile fixture
- Gateway, Config Center, MQ, lifecycle, auth/JWT, and report tests
- `scripts/rozectl-smoke.sh`
- `scripts/production-evidence-smoke.sh`
- `scripts/production-evidence-promotion-smoke.sh`
- deterministic supported-generation matrix test
- Windows release preflight (`scripts/release-preflight.ps1`), including
  workspace checks with Kafka-enabled example applications excluded as
  documented
- `rozectl gate check --manifest roze-gate.yaml` with API, Search, and SQL
  checks
- Gateway security propagation tests preserve verified JWT/API-key
  permissions and scopes and remove spoofed identity headers before building
  downstream context
- Auth, JWT, and service configuration debug output redacts API keys and JWT
  secrets, with regression tests in `roze-auth`, `roze-jwt`, and `roze-config`
- Full Gateway test suite: 27 passed, 4 externally coordinated registry tests
  intentionally ignored; middleware suite: 18 passed
- production evidence gate correctly retaining Gateway, MQ, Config Center,
  Lifecycle, and generated-system areas as `long-run pending`
- fixed-runner workflow wiring statically verifies attestation output,
  promotion/report verification, and the two-stage raw/complete artifact flow
- fixed-runner preflight wiring is present for Linux, Docker Compose, procfs,
  toolchain, and full-revision checks
- Linux `apps/user` compile after migrating its hand-written health, metrics,
  and OpenAPI routes to the native `roze_http::Router` contract
- `roze-service` lifecycle unit suite: 7 passed, 1 long-run test ignored
- `roze-gateway` unit suite: 27 passed, 4 externally coordinated tests ignored
- `roze-report` unit suite: 5 passed
- `roze-auth` unit suite: 6 passed; `roze-jwt` unit suite: 5 passed
- Config Center listener timeout and panic isolation regressions: both passed,
  followed by the complete 34-pass unit suite and ten repeated timeout-isolation
  runs

## Remote Short Validation

On 2026-07-18, the isolated Linux/Docker runner at `192.168.1.166` executed a
30-second MQ harness run against dedicated NATS JetStream and Kafka containers.
The run passed with one hard broker fault injection and reported:

- in-memory: `5,000` sent, `4,615` acknowledged, `385` dead-lettered
- NATS: `5` disconnect observations, `1` recovery
- Kafka: `102` transport disconnect observations, `1` recovery, `193` delivered

The same isolated Linux runner also completed a 30-second Config Center run
with one hard Etcd fault injection:

- config admin store: `673` accepted updates, `40` rejected, `24` rollbacks
- Etcd subscriber: `81` disconnect observations, `1` recovery, `233` watch updates

The short run validates the harness and recovery semantics only. It is not a
24h/72h production evidence artifact and does not change the long-run status.

The generator plan workspace now includes a process-local sequence component
in addition to the timestamp and process ID. Parallel generation tests cannot
reuse a staging directory and remove another test's generated files.

## Environment limits

The unfiltered workspace Cargo test/check/clippy commands could not complete
on this Windows machine because the workspace example services enable
`rdkafka` and the `rdkafka-sys` static configure step requires a compatible
native build tool. The documented Windows preflight passed after excluding
those two Kafka-enabled example applications. The release workflow runs on
Ubuntu and remains the authoritative full workspace gate.

The generated-target matrix was also split locally because its complete
reference-system compile path exceeds the short interactive command window;
each heavy fixture passed individually.

Docker is not installed on this machine, so the real dependency integration
script could not be started. Its authoritative Linux/Docker workflow remains
required for the reference-system recovery evidence.

The fixed-runner preflight is intentionally not counted as passed locally: this
Windows environment lacks the Linux and Docker prerequisites it is designed to
validate before a long run begins.

## Current Blocker

The dedicated Linux runner was reachable at the TCP layer, but its SSH service
did not complete the protocol banner during repeated retrieval attempts. The
MQ, Config Center, and Lifecycle long-run processes had already been started
there, but their terminal `run.json`, `SHA256SUMS`, boundary summaries, and
attestation metadata could not be retrieved or verified. This is an external
execution blocker, not a passing evidence result.

## Still required

- Linux release-gate run with the complete workspace and real dependencies.
- Real Gateway, MQ, Config Center, lifecycle, and generated-system 24h and 72h
  soak artifacts with checksums and attestation.
- Dependency-backed failure and recovery evidence for the reference systems.
