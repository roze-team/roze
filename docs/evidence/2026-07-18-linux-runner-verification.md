# Linux Runner Verification — 2026-07-18

This record captures a verification of the current uncommitted workspace on
the authorized Linux runner. It is diagnostic evidence, not a passing S3/S4/S5
promotion report.

## Repository commit anchor

The production-generation, routing, evidence-gate, and competitive-runner
changes described by this record were committed locally as
`1f1259e5f` (`feat: harden production generation and evidence gates`). The
Linux snapshot below remains intentionally identified by its own revision;
the running 24h diagnostic must finish and be re-run against this commit
before it can become promotion evidence.

## Snapshot

- Host: `alion@192.168.1.166`
- OS: Ubuntu 26.04 LTS, x86_64
- Docker: 29.4.3; Compose: v5.1.3
- Workspace: `/home/alion/roze-verify-3`
- Snapshot revision: `5ac168f7fc8472b05a23c8df747a95887a5cdef3`
- Snapshot method: current workspace archive, excluding `.git` and `target`,
  committed as an isolated verification snapshot; the original
  `/home/alion/roze-current` directory was not modified.

## Passing checks

- `bash scripts/production-soak-preflight.sh`
- `bash scripts/reference-systems-preflight.sh`
- `cargo test -p roze-storage`: 7 passed, 1 ignored
- `cargo test -p roze-rpc`: 63 passed, 2 ignored
- `cargo check -p rozectl`
- Competitive/input/schedule/binding/report/UTF-8 verifier suites
- `ROZE_TEST_ETCD_ENDPOINT=http://127.0.0.1:2379 cargo test -p roze-rpc registry::tests::etcd_registry_registers_discovers_and_deregisters_against_real_service -- --ignored`
- `ROZE_TEST_ETCD_ENDPOINT=http://127.0.0.1:2379 cargo test -p roze-config config_center::tests::etcd_subscriber_reads_and_watches_real_service -- --ignored`
- Gateway real Etcd route/deregister/reregister test within the ignored suite
- `cargo test -p roze-rpc balance::`: 7 passed (P2C/EWMA)
- `cargo test -p roze-middleware route_`: 7 passed
- `cargo test -p roze-context`: 11 passed
- Candidate S6 audit: `0 verified, 5 long-run pending`
- The machine-readable candidate audit was regenerated on the runner at
  `/home/alion/roze-verify-3/target/s6-audit-current.json`; it records verdict
  `api_stable_long_run_pending` for revision
  `5ac168f7fc8472b05a23c8df747a95887a5cdef3`.

## Explicitly incomplete or blocked

- MinIO image pulls (`minio/minio` and `minio/mc`) timed out against
  `registry-1.docker.io`; the real S3 round-trip test remains ignored.
- Full reference integration emitted a failed, checksum-valid bundle at
  `target/reference-systems-integration-remote`; it could not start all
  dependencies because MinIO/Mongo/Search images were unavailable.
- Gateway restart-recovery tests require an external coordinator that stops
  and restarts the registry. The ordinary Etcd route recovery test passed;
  the restart test correctly failed when no outage was injected. Consul was
  not running on the verification host.
- Strict S6 audit correctly failed with `pending=5`.
- A clean fixed-revision go-zero worktree is now available under the isolated
  cache and its revision is `6a6b81ef20d5697f4fbe9c2a92c436e85d687be4`.
  Structural generation reaches the toolchain gate but fails closed because
  the host has `rustc/cargo 1.96.0` vs baseline `1.96.1`, Go `1.26.0` vs
  `1.26.2`, and protoc `3.21.12` vs `31.1`.

## Fixed-toolchain structural diagnostic (2026-07-20)

After the user-authorized system upgrade, the isolated diagnostic worktree
used Rust/Cargo `1.96.1`, Go `1.26.2`, Node `24.14.1`, protoc `31.1`, and the
fixed revisions `d73f4ff01ea6d128b98d6e5c5b2b1166ebc266ab` and
`6a6b81ef20d5697f4fbe9c2a92c436e85d687be4`. The structural source build
completed successfully, including REST/RPC generation and compilation of both
Roze and go-zero outputs. Its artifact manifest is at
`/home/alion/roze-verify-3-baseline/target/competitive-source-1784381835732/artifact-manifest.json`.

The manifest intentionally remains `evidenceEligible: false` and
`semanticsReady: false`: only REST echo, unary RPC echo, and REST-to-RPC echo
overlays are implemented; database/cache, MQ persistence, context round-trip,
fault, and correctness probes are still required before S3/S4 performance
evidence can be promoted.

This artifact must not be used to claim real S3 correctness, a surpassed
performance result, or 24h/72h long-run verification. Repeat the blocked
commands after the pinned images and external fault coordinator are available.

The same generated artifact then passed the Linux three-scenario process smoke:
both Roze and go-zero returned exact 1024-byte payloads for gRPC Echo,
`POST /v1/echo`, and `POST /v1/rpc-echo`. The result is at
`/home/alion/roze-verify-3-baseline/target/competitive-source-1784381835732-echo-smoke/echo-smoke.json`
with SHA256
`e639d3dee68b4b47a993cb3b9c21a152c54b28025b5883ccd63974aa769887fc`.
This remains semantic smoke evidence only; it is not a six-scenario benchmark.

## Direct dependency diagnostic (2026-07-20)

`scripts/reference-systems-direct.sh` ran five real round-trip probes on the
server. NATS JetStream, Etcd registry, Etcd config watch, Redis, and S3 all
passed in one run. Redis 7.2.5 and MinIO `RELEASE.2024-06-13T22-53-53Z` were
temporary localhost-only diagnostic services; the existing managed services
were not modified. The machine-readable bundle is
`/home/alion/roze-verify-3/target/reference-systems-direct`, with a redacted
`profile.json` and SHA256SUMS. Because these are not the pinned Compose image
set and no failure/recovery cycle was exercised, this remains diagnostic S3
evidence rather than promotion evidence.

The subsequent recovery sequence exercised Redis and MinIO loss and restart:
both down phases failed only for the unavailable dependency, and both restored
phases returned to a five-probe pass. Its bundle is
`/home/alion/roze-verify-3/target/reference-systems-recovery`; expected failure
markers and recursive `SHA256SUMS` are included. This strengthens diagnostic
evidence but does not change the pinned-image promotion boundary.

## S5 diagnostic soak launch (2026-07-20)

`scripts/reference-systems-direct-soak.js` enforces `24h`/`72h` duration
names, a minimum 30-second interval, complete elapsed time, and zero failed
probe samples before writing `status: passed`. Promotion runs should set
`ROZE_DIRECT_SOAK_EXPECTED_REVISION` to the release commit; the runner now
fails closed if the checked-out `HEAD` differs. A 24h run was launched on the
Linux server with PID `1485792`, output directory
`/home/alion/roze-verify-3/target/reference-direct-soak-24h-run-002`, and a
five-minute interval. At the latest checkpoint, 13 samples had passed (the
thirteenth completed at `2026-07-18T16:08:56Z` on the runner); no long-run
claim is made until the run reaches 86400 seconds and its checksums verify.

The same run crossed a one-hour diagnostic health checkpoint at runner time
`2026-07-18T16:08:56Z` (`01:01:11` elapsed, 13 samples, all passed, process
still alive). This checkpoint is useful for early liveness confirmation only;
it is not S5 evidence and does not relax the 24h/72h completion gate.
