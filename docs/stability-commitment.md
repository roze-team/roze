# Stability Commitment

Roze 1.0 is the stable public channel for the Rust framework, `rozectl`,
generated Rust services, and TypeScript/JavaScript Web clients.

## Public Claim Rules

Allowed:

- "Roze 1.x provides stable public APIs and generated contracts."
- "Roze release gates verify deterministic generation, ownership, contract and
  migration safety, compile coverage, focused failure tests, and smoke paths."
- "A runtime area has passing long-run evidence" only when the exact signed
  report and artifact are linked.

Not allowed without evidence:

- "Roze is battle-tested in production."
- "Roze has completed 24h/72h validation" when the required run has not
  completed successfully.
- Performance, recovery, capacity, or leak claims that are not supported by a
  reproducible report.

## Stable Public Surface

Public API includes:

- Rust crate APIs;
- CLI flags, commands, exit codes, and diagnostics;
- generated file layout and ownership boundaries;
- generated configuration and IDL schemas;
- runtime retry, fallback, rollback, watch, lifecycle, and shutdown ordering;
- metrics, event envelopes, error codes, and observable result fields.

Breaking any of these after 1.0 requires a new major version. Additive behavior
uses a minor release; compatible fixes use a patch release. Generated code is
part of the public API.

## Evidence Is Independent

API stability and operational evidence are separate axes. A stable runtime API
may have `long-run pending` evidence in `docs/maturity.md`. That area can be
adopted with workload-specific validation, but it cannot be described as
battle-tested until `docs/production-evidence.md` is satisfied.

## Toolchain Policy

Roze 1.0 development and release gates are pinned to Rust 1.98.0, with a
scheduled latest-stable canary for forward compatibility. The compiler pin is
not an MSRV declaration. No fixed MSRV is claimed until CI continuously
verifies one; once declared, raising it in the 1.x line requires the
compatibility treatment documented by the release policy.

## Release Commitment

A 1.x release requires the release gate, accurate maturity/evidence labels,
upgrade and rollback notes, and a clean supply-chain check. Missing long-run
evidence does not downgrade API stability, but release notes must preserve the
`long-run pending` label and must not make battle-tested claims.
