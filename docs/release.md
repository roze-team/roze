# Release Policy

Roze is currently pre-release. The repository can be used for evaluation and
internal pilots, but a team should not treat every crate as production-stable
until the maturity matrix says so.

## Current Install Status

Supported today:

```bash
cargo install --git https://github.com/roze-team/roze.git rozectl
cargo install --git https://github.com/roze-team/roze.git rozectl --force
cargo install --path apps/rozectl
```

Planned before the first stable adoption path:

- Publish `rozectl` and framework crates to crates.io.
- Create signed Git tags and GitHub Releases.
- Attach release notes generated from `CHANGELOG.md`.
- Publish upgrade notes for generated project layout changes.
- Add a release checklist to CI.

## Versioning

Roze should use Semantic Versioning after the first public release:

- `MAJOR`: breaking changes in public crate APIs, generated project layout,
  generated config schema, `.api` compatibility, CLI flags, or runtime behavior.
- `MINOR`: backward-compatible framework features, new generator targets,
  additional middleware, new client SDKs, or new optional integrations.
- `PATCH`: bug fixes, documentation fixes, non-breaking generated code fixes,
  and test hardening.

Generated code is part of the public contract. A change is breaking if it
requires users to rewrite preserved `src/logic/**`, custom middleware, config
files, or CI/deployment wiring.

## MSRV

The current workspace uses Rust 2021. Generated REST/RPC services also pin
`edition = "2021"` so they do not inherit a parent workspace's Rust 2024
edition.

Roze does not yet claim a fixed MSRV. Before publishing stable releases, add an
MSRV matrix to CI and record the supported compiler version here. After that,
raising MSRV is a breaking change unless it happens before `1.0.0`.

## Release Checklist

Before cutting a release:

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace`
- `cargo test -p rozectl -- --skip postgres --skip mysql`
- Generated REST project compiles.
- Generated RPC project compiles.
- `--update` ownership tests pass and prove user logic is preserved.
- Gateway smoke tests cover rewrite, timeout, auth, rate limit, breaker, retry,
  fallback, and hot reload.
- MQ/Kafka tests cover ack, nack, retry, dead letter, and idempotency behavior.
- Config center tests cover diff, version, rollback, and subscriber failure
  isolation.
- `CHANGELOG.md` and upgrade notes are updated.

## Breaking Change Notes

Every breaking change must document:

- What changed.
- Why it changed.
- Which generated files are affected.
- Whether `--update` can migrate the project safely.
- Manual migration steps when generated files and user files overlap.
- Rollback strategy.
