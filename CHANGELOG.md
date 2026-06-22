# Changelog

All notable changes to Roze should be recorded in this file.

Roze has not published a stable release yet. Until the first tagged release,
entries are grouped under `Unreleased`, and users should install `rozectl` from
Git or a local checkout.

The format follows Keep a Changelog style, and version numbers should follow
Semantic Versioning once releases begin.

## Unreleased

### Added

- REST generator layout now separates route, handler, logic, middleware,
  config, OpenAPI, service context, and type modules.
- RPC generator layout now separates server, client, protobuf, service context,
  types, and one logic file per method.
- Generated REST API crates avoid DB/Mongo/Toasty dependencies by default.
- Generated REST/RPC service manifests pin `edition = "2021"`.
- Generated Toasty dependencies default to MySQL/PostgreSQL features and do not
  enable sqlite.
- REST middleware config covers recover, trace, stat, prometheus, CORS,
  timeout, max connections, adaptive shedding, request gunzip, and request body
  limits.
- Service-wide HTTP timeout is applied through Tower HTTP middleware; route
  timeout overrides are enforced by generated handler adapters.
- `rozectl api client` can generate TypeScript, JavaScript, and Dart clients.
- `rozectl openapi generate` emits OpenAPI 3 documents from `.api` contracts.

### Changed

- REST `--update` preserves application-owned logic, custom middleware, and
  `config.yaml`, while refreshing generator-owned glue code.
- RPC `--update` preserves application-owned logic while refreshing
  generator-owned server/client/protobuf glue.
- The project documentation now separates production-ready, beta, scaffold, and
  planned modules.

### Known Gaps

- GitHub Releases and crates.io publishing are not enabled yet.
- Gateway, config hot reload, MQ/Kafka semantics, and lifecycle orchestration
  still need production hardening and end-to-end tests.
- Validator/OpenAPI projection does not yet cover every go-playground validator
  edge case.
- Java/Kotlin SDK generation is not implemented.
