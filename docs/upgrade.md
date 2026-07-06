# Upgrade Guide

Roze is pre-release. Treat every upgrade as a source upgrade until tagged
releases and crates.io publishing are available.

## Before Upgrading

- Pin the current Roze Git revision in the application repository.
- Read `CHANGELOG.md` for breaking changes and known gaps.
- Read `docs/maturity.md` for modules used by the application.
- Run the application's existing test suite before regenerating code.

## Generated REST/RPC Projects

`rozectl --update` preserves application-owned files:

- REST `src/logic/**`
- RPC `src/logic/**`
- REST/RPC `src/svc/mod.rs`
- custom REST middleware files under `src/middleware/`
- `config.yaml`

For REST services, group module indexes such as `src/logic/admin/mod.rs`
refresh generated handler exports while preserving extra app-owned
`mod ...;` declarations.

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
