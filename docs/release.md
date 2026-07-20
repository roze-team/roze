# Release Policy

Roze 1.0 is the stable public channel. Rust APIs, CLI commands, generated
contracts, configuration schemas, and generator-owned layouts follow Semantic
Versioning. Operational evidence remains an independent adoption input.

## Current Install Status

Supported source installation:

```bash
cargo install --git https://github.com/roze-team/roze.git rozectl
cargo install --git https://github.com/roze-team/roze.git rozectl --force
cargo install --path apps/rozectl
```

Publication-dependent paths are supported only after their corresponding
release artifact is visible:

- `cargo install rozectl`
- `cargo install --git https://github.com/roze-team/roze.git --tag v1.0.0 rozectl`
- GitHub Release binary assets
- crates.io framework crates that have not yet been published

Recommended for production adoption:

- Pin a Git revision instead of tracking a branch.
- Record the Roze revision in generated service repositories.
- Read [Module Maturity Matrix](maturity.md) before adopting a crate in a
  production path.
- Read [Stability Commitment](stability-commitment.md) before making public
  production-readiness claims.
- Read [Production Evidence](production-evidence.md) before making long-run or
  battle-tested runtime claims.
- Read [Upgrade Guide](upgrade.md) before regenerating existing projects.

## Versioning

Roze 1.x uses Semantic Versioning:

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

Roze 1.0 supports the latest stable Rust channel used by CI. It does not claim
a fixed MSRV until a pinned compiler is continuously verified.

Toolchain policy:

| Channel | Purpose | 1.0 policy |
| --- | --- | --- |
| latest stable | Primary development and release build | Required |
| pinned MSRV | Proves a future minimum supported compiler | Not yet declared |
| beta | Early warning for upcoming Rust changes | Recommended |

After a fixed MSRV is published, raising it follows the compatibility policy
announced with that MSRV declaration.

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
git tag -s v1.0.0 -m "Roze v1.0.0"
git push origin v1.0.0
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

On Windows, run the non-authoritative preflight first:

```powershell
powershell -ExecutionPolicy Bypass -File scripts/release-preflight.ps1
```

The Windows preflight excludes `user-service` and `roze-example` because they
enable the vendored `rdkafka` build, whose Unix configure path is not a native
Windows release target. It must not be reported as a complete release gate.
Run `scripts/release-gate.sh` on Linux or WSL, where CI also verifies the
rdkafka-enabled applications and `cargo check -p roze-kafka --features
rdkafka`.

- Create a release tracking issue from the release checklist template.
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- The `supply-chain` CI job passes RustSec advisory, dependency license, and
  dependency source-policy checks against `Cargo.lock` and `deny.toml`.
- Security advisory exceptions must be narrowly scoped in both `audit.toml`
  and `deny.toml`, explain why the vulnerable operation is unreachable, and be
  removed when the upstream dependency provides a safe path.
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
- `docs/maturity.md` accurately labels contract stability and evidence state.
- `bash scripts/production-release-audit.sh --json-out target/production-release-audit.json`
  records the S6 candidate verdict for the exact release revision. Use
  `--require-long-run` for a publication that claims battle-tested runtime
  behavior.
- Runtime-critical modules marked `stable` have evidence reports that satisfy
  `docs/production-evidence.md`.
- New evidence reports are generated with `scripts/production-evidence.sh` or
  contain the same required fields.
- Public release language follows `docs/stability-commitment.md`.
- `README.md` states the stable version and only advertises artifacts that have
  actually been published.
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
