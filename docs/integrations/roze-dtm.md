# Independent Roze DTM integration

Distributed TCC, Saga, Message, Workflow, and XA coordination is owned by the
independent [`roze-dtm`](https://github.com/roze-team/roze-dtm) repository. The
Roze workspace keeps local transactions, in-process compensation plans,
inbox/outbox, MQ, request-context propagation, configuration, and governance
primitives. It does not mount or deploy the coordinator.

## Version contract

Pin both repositories to full Git revisions. A production evidence bundle must
record:

- the full Roze revision used by `roze-dtm` dependencies
- the full deployed `roze-dtm` revision
- the HTTP/OpenAPI and gRPC protocol revisions consumed by applications
- coordinator topology, worker count, and persistent storage configuration
- verification commands, result artifacts, and artifact digests

When a public Roze crate used by `roze-dtm` changes, run the DTM repository's
compatibility and recovery gates before promoting Roze. When `roze-dtm`
advances its Roze pin, run the consuming application's contract and failure
tests before deploying the new coordinator.

Run the repository-side static contract check from the Roze checkout:

```bash
python scripts/check-roze-dtm-compatibility.py --dtm-dir ../roze-dtm
```

Pass `--output <path>` to retain the JSON report as release evidence. The
release workflow archives this report beside the main production release audit.

The checked-in
[`roze-dtm-compatibility.json`](roze-dtm-compatibility.json) pins the accepted
DTM revision, its Roze dependency revision, package/OpenAPI versions, and the
required security, recovery, interoperability, and retention surfaces. Release
CI checks out that exact revision; it does not follow the moving DTM `main`
branch. The baseline also records the successful upstream workflow run and
requires its head revision and run URL to agree with the pinned DTM revision.
This is immutable provenance recorded during review, not a live GitHub status
query by the offline checker.

Use `--require-roze-head` in a coordinated upgrade gate when the DTM dependency
pin is required to equal the current Roze commit. The default report remains
non-failing when an older, intentionally accepted pin is detected, but labels
the result `pinned_baseline_differs` so it cannot be mistaken for alignment.
The current DTM package and OpenAPI `info.version` are recorded separately in
the baseline because they differ; this is visible contract state, not evidence
that the two version schemes are interchangeable.

## Application dependency

Use a full-revision Git dependency rather than a moving branch:

```toml
[dependencies]
roze-dtm = { git = "https://github.com/roze-team/roze-dtm.git", rev = "<full-dtm-revision>" }
```

The Rust client entry points are `roze_dtm::client::DtmHttpClient` and
`roze_dtm::grpc_client::DtmGrpcClient`. Browser and Node.js clients are
maintained in the independent repository's `sdk/` directory. Do not invent a
separate client package name or copy SDK sources into this workspace.

Keep coordinator connectivity in typed application configuration and secrets,
not in generated transport code. A deployment contract should provide values
equivalent to:

```yaml
application:
  dtm_client:
    http_endpoint: https://dtm.internal.example
    grpc_endpoint: https://dtm-grpc.internal.example
    control_token: ${secret:roze-dtm/control-token}
    expected_revision: <full-dtm-revision>
    timeout_ms: 3000
```

Applications must propagate Roze request context and trace metadata, assign a
stable idempotency key to each logical mutation, and query transaction state
after an ambiguous timeout instead of assuming success or failure.
Before accepting traffic, compare `expected_revision` with the
`release_revision` returned by `GET /api/dtmsvr/version`; production
configuration in `roze-dtm` requires a full, non-zero 40-character Git
revision.

## Deployment boundary

Deploy `roze-dtm` as a separate workload. The application and coordinator must
agree on:

- allowed HTTP origins, bearer-token scope, TLS, and network policy
- a read-only `branch_tls_ca_file` secret mount for private HTTPS and `grpcs`
  trust roots, with normal certificate-chain, expiry, and hostname validation
- persistent storage, worker ownership, retry limits, and recovery scheduling
- readiness, liveness, metrics, tracing, audit, and revision endpoints
- transaction retention windows, compare-and-delete behavior, and persistent
  audit retention independent from coordinator-record cleanup
- whether endpoints are static deployment configuration or resolved through
  an explicitly configured service registry

Do not infer or auto-mount a coordinator endpoint from the presence of Roze
crates. Missing configuration must fail closed for workflows that require
distributed coordination.

## Evidence boundary

Generated `ops/data-consistency.yaml` assets classify DTM/Saga as an external
coordinator dependency and require its exact revision and recovery evidence.
The main repository's event-commerce reference system demonstrates local
transactions, outbox/inbox, idempotency, and application compensation only.
Saga/TCC success, failure, restart recovery, and idempotent compensation
evidence belongs to the adjacent `roze-dtm` production report.

For delayed Message transactions, evidence must additionally show that the
commit decision and delivery time survive coordinator termination, a different
worker takes over only after the previous recovery lease expires, and the
branch is delivered exactly once in the tested scenario. SQLite restart
acceptance does not establish PostgreSQL/Redis multi-instance, network
partition, or sustained-load maturity.

Do not claim production-ready distributed transactions without failure
injection, coordinator crash/restart recovery, cross-language protocol
compatibility, and sustained 24-hour/72-hour evidence tied to the exact Roze
and `roze-dtm` revisions under test.

The latest dated repository-to-repository review is recorded in
[Roze DTM compatibility baseline](../evidence/2026-08-30-roze-dtm-compatibility.md).
