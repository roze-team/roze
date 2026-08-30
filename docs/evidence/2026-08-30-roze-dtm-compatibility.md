# Roze DTM compatibility baseline — 2026-08-30

This review advances the repository-to-repository compatibility baseline after
`roze-dtm` added a real delayed Message restart gate. It records static contract
alignment and upstream CI provenance; it is not 24-hour/72-hour production
evidence.

## Revisions inspected

| Repository | Revision | Observation |
| --- | --- | --- |
| `roze-team/roze` | `1945a037558717ae9253fa61060fe900567e52de` | Roze head containing the external DTM integration and compatibility checker. |
| `roze-team/roze-dtm` | `efd2e8a1ae4d48d1e8fe2c010862ca325825ae25` | Local checkout and GitHub `refs/heads/main` matched during inspection. |
| Roze dependency pin used by `roze-dtm` | `217274a134068f174cbe4a266a011bf719e15d0d` | All 14 Roze Git dependencies use the same reviewed full revision. |

## Verified compatibility surfaces

- trusted private-CA branch TLS with certificate and hostname validation;
- bounded transaction retention with compare-and-delete behavior;
- cross-language HTTP, JSON-RPC, and gRPC acceptance;
- gRPC callback recovery and official dtm-labs TCC rollback;
- Redis Cluster ASK/MOVED, failover, and recovery fault injection;
- delayed Message recovery across coordinator termination and worker change.

The delayed Message gate persists a transaction in file-backed SQLite, stops
the first coordinator before its delivery time, waits past both the delivery
time and old recovery lease, starts a coordinator with a different worker ID,
and requires the transaction to reach `Succeeded` with exactly one branch
call. Commit `244f02beb053a4b0a141ad2e43c39b4ecf98462a` introduced the gate; GitHub
Actions run `33284169721` passed it together with the protocol and real-backend
matrix. The documentation commit pinned here was independently validated by
successful run `33284484555`.

The machine-readable baseline records run `33284484555`, its workflow name,
head revision, conclusion observed during this review, and canonical GitHub
Actions URL. The offline compatibility checker validates their internal
consistency and includes the provenance in its JSON report; it does not query
GitHub or reinterpret the recorded conclusion as long-run evidence.

## Conservative boundary

The DTM workspace intentionally remains pinned to an older accepted Roze
revision, so the static report remains `pinned_baseline_differs`; this is not a
runtime incompatibility finding. The Roze changes since that pin affecting DTM
dependencies are workspace extraction/test cleanup and documentation of the
in-process `roze_transaction::Saga` alias, not a required coordinator API
upgrade.

The passing restart gate covers file-backed SQLite and a controlled worker
handoff. PostgreSQL/Redis multi-instance restart, network partition, broader
crash-point matrices, and sustained 24-hour/72-hour behavior remain
`inconclusive` and must not be inferred from this baseline.

Cargo/Rust/MSVC commands were not run in this Roze review. Runtime claims above
refer only to the cited `roze-dtm` GitHub Actions runs; the local verification
checks repository structure, pinned revisions, protocol surfaces, and evidence
markers.
