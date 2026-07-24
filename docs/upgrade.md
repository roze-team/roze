# Upgrade Guide

Roze 1.0 follows Semantic Versioning for Rust APIs, CLI commands, generated
contracts, configuration schemas, and generator-owned layouts. Review this
guide and run `rozectl gate check` before every regeneration or framework
upgrade.

## Before Upgrading

- Pin the current Roze Git revision in the application repository.
- Read `CHANGELOG.md` for breaking changes and known gaps.
- Read `docs/maturity.md` for modules used by the application.
- Run the application's existing test suite before regenerating code.

## Generated REST/RPC Projects

`rozectl --update` preserves application-owned files:

- REST `src/logic/<group>/<handler>.rs`
- RPC `src/logic/<method>.rs`
- REST/RPC `src/logic/prelude.rs`
- REST/RPC `src/application.rs`
- custom REST middleware files under `src/middleware/`
- `config.yaml`

`src/logic/mod.rs` and REST group indexes such as
`src/logic/admin/mod.rs` are generator-owned. Move custom module attributes,
declarations, imports, and re-exports into `src/logic/prelude.rs`, which is
preserved during updates.

Generated glue may be refreshed:

- route and handler adapters
- generated DTOs and OpenAPI modules
- RPC server/client/protobuf glue
- build files and manifest dependency wiring

Use `--force` only when intentionally rebuilding a generated project from
scratch.

## Breaking Change Checklist

When upgrading across a breaking change:

- Identify affected generated files.
- Check whether `--update` migrates the project safely.
- Review any changed config fields before deployment.
- Rebuild generated REST/RPC projects.
- Run smoke tests for gateway, config reload, MQ/Kafka, and auth paths used by
  the application.

## Rollback

Rollback should restore both:

- the application commit that pins the previous Roze revision
- any generated files changed by the upgrade

For config-center or gateway changes, keep the previous valid runtime config
available so services can restart or reload without depending on a new config
shape.

## 2026-07 Production Contract Reset

This upgrade intentionally has no compatibility adapters:

- Replace `auth.jwt_secret` with `jwt_keys`, `jwt_active_key_id`,
  `jwt_audience`, and `jwt_clock_skew_secs`. Issued claims now require `aud`
  and `jti`; JWT headers require `kid`.
- Replace lifecycle phase `Running` with `Ready`. Service hooks now execute in
  dependency order for start/readiness and reverse order for drain/stop.
- Regenerate stream services for the versioned Event Envelope fields. MQ,
  Kafka, NATS, inbox, and outbox metadata use event ID/type/version/schema,
  trace context, tenant, idempotency key, producer, attempt, and occurred time.
- Replace `GET /reports/export` and `GET /charts/query` with
  `POST /reports/exports` and `POST /charts/query`; regenerate OpenAPI and Web
  SDK clients. Generated endpoints no longer return successful empty datasets:
  register `ReportDataSource` from the preserved `src/application.rs` hook or
  they return `503`.
- Config admin publish/restore now requires a signing policy and bound
  signature. Partial rollout versions require explicit promotion.
- `src/svc/mod.rs` is now fully generator-owned and is replaced during REST/RPC
  `--update`. Move custom resources and initialization into
  `src/application.rs` before upgrading. No legacy context merger is provided.
  Run model generation after REST/RPC generation when a service uses generated
  model context wiring.

Run `rozectl gate check --manifest roze-gate.yaml` before regeneration or
deployment. Breaking API/Search/SQL changes require a non-expired,
digest-matched acknowledgment with migration and rollback plans.
