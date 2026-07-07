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

Not supported yet:

- `cargo install rozectl`
- Installing framework crates from crates.io
- GitHub Release assets
- Signed release tags

Recommended for evaluation and internal pilots:

- Pin a Git revision instead of tracking a branch.
- Record the Roze revision in generated service repositories.
- Read [Module Maturity Matrix](maturity.md) before adopting a crate in a
  production path.
- Read [Stability Commitment](stability-commitment.md) before making public
  production-readiness claims.
- Read [Production Evidence](production-evidence.md) before marking runtime
  modules as stable.
- Read [Upgrade Guide](upgrade.md) before regenerating existing projects.

## Versioning

Roze should use Semantic Versioning after the first public release:

- `MAJOR`: breaking changes in public crate APIs, generated project layout,
  generated config schema, `.api` breaking-change policy, CLI flags, or runtime behavior.
- `MINOR`: additive framework features, new generator targets,
  additional middleware, new client SDKs, or new optional integrations.
- `PATCH`: bug fixes, documentation fixes, non-breaking generated code fixes,
  and test hardening.

Generated code is part of the public contract. A change is breaking if it
requires users to rewrite preserved `src/logic/**`, `src/svc/mod.rs`, custom
middleware, config files, or CI/deployment wiring.

## MSRV

The current workspace uses Rust 2021. Generated REST/RPC services also pin
`edition = "2021"` so they do not inherit a parent workspace's Rust 2024
edition.

Roze does not yet claim a fixed MSRV. Until CI proves an MSRV matrix, do not
make an MSRV guarantee and use the latest stable Rust toolchain for local
development and evaluation.

Planned MSRV policy before the first stable release:

| Channel | Purpose | Required before stable |
| --- | --- | --- |
| latest stable | Primary development and release build | Yes |
| pinned MSRV | Proves the minimum supported compiler | Yes |
| beta | Early warning for upcoming Rust changes | Recommended |

After a fixed MSRV is published, raising MSRV is a breaking change unless it
happens before `1.0.0`.

## crates.io and GitHub Release Plan

Before publishing externally:

1. Confirm crate names and ownership on crates.io.
2. Ensure every published crate has license, repository, README, description,
   categories, and keywords where appropriate.
3. Publish dependency crates before dependent crates.
4. Publish `rozectl` only after generated REST/RPC project smoke tests pass.
5. Create a signed Git tag for the same version.
6. Create a GitHub Release from `CHANGELOG.md` plus upgrade notes.
7. Verify install paths:
   - `cargo install rozectl --version <version>`
   - `cargo install --git https://github.com/roze-team/roze.git --tag v<version> rozectl`

Recommended tag command:

```bash
git tag -s v0.1.0 -m "Roze v0.1.0"
git push origin v0.1.0
```

Unsigned tags are acceptable only for local/internal dry runs and must not be
documented as production release tags.

## Release Checklist

Before cutting a release:

- The `release-gate` GitHub Actions workflow passes for the release commit or
  tag. It is the machine-enforced gate for Gateway, Config Center, MQ,
  Lifecycle, generator smoke, generated compile smoke, stream generator compile
  smoke, and production smoke.
- The same gate can be run locally with:

```bash
bash scripts/release-gate.sh
```

- Create a release tracking issue from the release checklist template.
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace`
- `cargo check --workspace`
- `cargo check -p roze-kafka --features rdkafka`
- `cargo test -p rozectl -- --skip postgres --skip mysql --skip mongo`
- `bash scripts/production-smoke.sh`
- `bash scripts/rozectl-smoke.sh`
- `bash scripts/production-soak-mq.sh 300` passes before starting a long MQ evidence run.
- `bash scripts/production-soak-config-center.sh 300` passes before starting a long Config Center evidence run.
- `bash scripts/production-soak-lifecycle.sh 300` passes before starting a long Lifecycle evidence run.
- `cargo test -p rozectl generated_rest_project_compiles_with_model_and_search -- --ignored`
- `cargo test -p rozectl generated_rpc_project_compiles -- --ignored`
- `cargo test -p rozectl generated_stream_project_compiles -- --ignored`
- `--update` ownership tests pass and prove user logic, service context
  extensions, custom middleware, and config are preserved.
- Gateway smoke tests cover rewrite, timeout, auth, rate limit, breaker, retry,
  fallback, and hot reload.
- MQ/Kafka tests cover ack, nack, retry, dead letter, and idempotency behavior.
- Config center tests cover diff, version, rollback, and subscriber failure
  isolation.
- `CHANGELOG.md` and upgrade notes are updated.
- `docs/maturity.md` accurately labels modules as stable/beta/scaffold/planned.
- Runtime-critical modules marked `stable` have evidence reports that satisfy
  `docs/production-evidence.md`.
- New evidence reports are generated with `scripts/production-evidence.sh` or
  contain the same required fields.
- Public release language follows `docs/stability-commitment.md`.
- `README.md` still states the current pre-release install path.
- Security-sensitive changes are checked against `SECURITY.md`.

The release gate intentionally runs the high-signal stability checks without
external Docker Compose dependencies. Full dependency validation remains
available through:

```bash
bash scripts/production-smoke.sh --with-compose
```

## Breaking Change Notes

Every breaking change must document:

- What changed.
- Why it changed.
- Which generated files are affected.
- Whether `--update` can migrate the project safely.
- Manual migration steps when generated files and user files overlap.
- Rollback strategy.
