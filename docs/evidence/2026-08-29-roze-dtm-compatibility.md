# Roze DTM compatibility baseline — 2026-08-29

Superseded by the
[2026-08-30 compatibility baseline](2026-08-30-roze-dtm-compatibility.md).
This file remains as the historical record for the revision pair inspected on
2026-08-29.

This review records the exact repository state used to verify the ownership and
integration contracts after `roze-dtm` was removed from the Roze workspace. It
is a static compatibility baseline, not runtime or production evidence.

## Revisions inspected

| Repository | Revision | Observation |
| --- | --- | --- |
| `roze-team/roze` | `39bb1afc8aaf759bf130c5008a61f092e7acbc46` | Main repository baseline before the pending integration-contract changes in this review. |
| `roze-team/roze-dtm` | `1705e1e1519b8bd47d1381ca0811d062b6a91093` | Local `main`, `origin/main`, and GitHub `refs/heads/main` matched during the final inspection. |
| Roze dependency pin used by `roze-dtm` | `217274a134068f174cbe4a266a011bf719e15d0d` | Every Roze Git dependency in the DTM workspace used this full revision. |

## Static findings

- `python scripts/check-roze-dtm-compatibility.py --dtm-dir ../roze-dtm`
  matched the checked-in compatibility baseline, returned
  `pinned_baseline_differs`, and identified 14 Roze Git dependencies at the
  same full revision.
- The fixed baseline verifies trusted private-CA branch TLS, retention
  compare-and-delete, cross-language HTTP/JSON-RPC/gRPC acceptance, gRPC
  callback recovery, official dtm-labs TCC rollback, and Redis Cluster
  fault-injection source gates.
- The coordinator is an independent Cargo workspace whose root package is
  `roze-dtm`; its service is the `service/` workspace member.
- Rust consumers use `roze_dtm::client::DtmHttpClient` or
  `roze_dtm::grpc_client::DtmGrpcClient`. TypeScript/JavaScript clients remain
  owned by the DTM repository's `sdk/` directory.
- `GET /api/dtmsvr/version` returns `release_revision`; production DTM
  configuration requires that revision to be a full, non-zero Git commit.
- The DTM workspace intentionally builds against an older pinned Roze revision,
  so compatibility with the current Roze head must not be inferred from source
  layout or API names.
- The DTM Cargo package version is `1.0.0` while OpenAPI `info.version` is
  `0.1.0`. Both are pinned explicitly until the DTM project either documents
  separate version schemes or aligns them.

## Required promotion evidence

Before either repository advances the shared compatibility baseline:

1. update all Roze dependency pins in `roze-dtm` to one reviewed full revision;
2. run the DTM workspace unit, protocol, backend, and recovery gates;
3. run consuming application HTTP/gRPC contract and ambiguous-timeout tests;
4. deploy with `ROZE_DTM_RELEASE_REVISION` set to the exact DTM revision and
   verify it through `/api/dtmsvr/version` before accepting traffic;
5. archive the commands, topology, result artifacts, and artifact digests.

Cargo/Rust/MSVC execution was explicitly paused during this review. Therefore
no build result is claimed for the revision pair above; only file structure,
dependency pins, public client names, and revision contracts were inspected.
