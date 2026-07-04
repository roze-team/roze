# Stability Commitment

Roze uses explicit stability labels. A module is not stable just because it is
present in the repository.

## Public Claim Rules

Allowed today:

- "Roze is pre-release."
- "Gateway, MQ, Config Center, and most generators are in beta and are suitable
  for internal pilots."
- "Lifecycle/bootstrap and Transactions/outbox/DTM are scaffolds that still
  need full orchestration or end-to-end examples before stable claims."
- "Release-gate, smoke tests, and generated compile tests are available."
- "Some contracts are documented and being hardened."

Not allowed today:

- "All Roze crates are production-stable."
- "Roze is battle-tested in production" without a linked production evidence
  report.
- "Config Center is stable" before management semantics, audit, rollback,
  watch status, permission checks, snapshot backup/restore or external
  control-plane integration, and long-run evidence are implemented and tested.
- "MSRV is guaranteed" before the release policy names a fixed MSRV and CI
  proves it.

## Stable Module Requirements

A module can be marked `stable` in `docs/maturity.md` only when all of the
following are true:

- Public API and behavior are documented.
- Configuration keys, defaults, and precedence are documented.
- Runtime ordering is documented.
- Failure semantics are documented.
- Migration and rollback notes exist for breaking changes.
- Unit tests and at least one end-to-end or smoke test exist.
- CI release gate runs the relevant tests.
- Production evidence exists when the module is runtime-critical.

## API Stability

Public API includes:

- Rust crate APIs.
- CLI flags and subcommands.
- Generated file layout.
- Generated configuration schema.
- Runtime behavior such as retry, fallback, rollback, watch, and shutdown
  ordering.
- Metrics, event names, and observable result fields.

Generated code is part of the public API. A generator change is breaking when
it requires users to rewrite preserved logic, custom middleware, config,
deployment manifests, or CI wiring.

## Experimental Surface

Any module marked `beta`, `scaffold`, or `planned` may change before `1.0.0`.
Breaking changes still require release notes and upgrade documentation, but
they do not carry a stable compatibility promise.

## MSRV Commitment

Roze does not currently guarantee a fixed MSRV. Until the release policy names
one and CI proves it, use the latest stable Rust toolchain.

Once an MSRV is declared:

- The value must be listed in `docs/release.md`.
- CI must test the declared MSRV.
- Raising MSRV after `1.0.0` is a breaking change.

## Release Commitment

No release should be described as stable unless:

- `.github/workflows/release-gate.yml` passes on the release commit or tag.
- `docs/maturity.md` marks the affected modules accurately.
- `docs/production-evidence.md` requirements are satisfied for stable runtime
  modules.
- `docs/upgrade.md` covers breaking changes.
