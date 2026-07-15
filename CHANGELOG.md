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
- `rozectl api client` generates governed TypeScript and JavaScript Web clients.
- `rozectl openapi generate` emits OpenAPI 3 documents from `.api` contracts.
- `apps/roze-gateway` supports registry-backed upstreams, weighted canary
  routing, route retries, health/outlier handling, unified governance defaults,
  JWT/API key auth, and config-center hot reload.
- `roze-config` config center emits reload metadata, field diffs, rollback
  events, and section-level `ConfigCenterChangeEvent` values.
- `roze-mq`, `roze-kafka`, `roze-nats`, and `roze-transaction` share standard
  message metadata for attempts, dead-letter topics, timestamp, partition,
  offset, and group where supported.
- `roze-kafka` includes retry/dead-letter recovery decisions, queue metrics,
  in-memory admin dead-letter replay, and rdkafka feature checks.
- `roze-admin` provides registry, config reload, and MQ/DLQ control-plane
  endpoints with optional token/API key protection.
- `roze-config` resolves one authoritative `GovernancePolicy` for REST, RPC,
  Gateway, MQ, and Job. Governed MQ consumers and jobs enforce timeout,
  full-jitter retry budgets, rate limits, breakers, and adaptive shedding.

### Changed

- REST `--update` preserves application-owned logic, custom middleware, and
  `config.yaml`, while refreshing generator-owned glue code.
- RPC `--update` preserves application-owned logic while refreshing
  generator-owned server/client/protobuf glue.
- The project documentation now separates production-ready, beta, scaffold, and
  planned modules.
- Gateway route fields inherit unified governance defaults when route-level
  fields are not set.
- Config reload listeners keep the last valid config when parsing a new value
  fails.
- Removed Dart, Java, Kotlin, Swift, iOS, and Android SDK generators to keep the
  product focused on Rust services and TypeScript/JavaScript Web clients.

### Known Gaps

- GitHub Releases and crates.io publishing are not enabled yet.
- Lifecycle orchestration, release automation, dashboards, and production
  examples still need hardening.
- Validator/OpenAPI projection does not yet cover every go-playground validator
  edge case.
