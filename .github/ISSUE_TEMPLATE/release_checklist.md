---
name: Release checklist
about: Track a Roze release from verification through publishing
title: "Release vX.Y.Z"
labels: release
assignees: ""
---

## Scope

- Version:
- Release owner:
- Target date:

## Preflight

- [ ] `CHANGELOG.md` updated
- [ ] `docs/upgrade.md` updated for breaking changes
- [ ] `docs/maturity.md` reviewed
- [ ] README install status reviewed
- [ ] Security-sensitive changes reviewed against `SECURITY.md`

## Verification

- [ ] `release-gate` workflow passed on the release commit or tag

```bash
bash scripts/release-gate.sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo check --workspace
cargo check -p roze-kafka --features rdkafka
cargo test -p rozectl -- --skip postgres --skip mysql
```

- [ ] Generated REST project compiles
- [ ] Generated RPC project compiles
- [ ] Gateway smoke tests pass
- [ ] Config-center rollback tests pass
- [ ] MQ/Kafka ack/nack/retry/dead-letter tests pass

## Publishing

- [ ] crates.io dry run completed for publishable crates
- [ ] signed tag created
- [ ] GitHub Release created from changelog and upgrade notes
- [ ] `cargo install` path verified
