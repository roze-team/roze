# Changelog

## Unreleased

- Pinned development, generated production verification, and release CI to
  Rust 1.98.0; added a scheduled latest-stable compatibility canary, resolved
  Rust 1.98 Clippy findings, and synchronized generated cache cluster wiring.
- Added always-on Veil `Debug` redaction and a `roze_config::redaction`
  re-export; AI provider keys, JWT signing secrets, and newly generated typed
  application configuration now use fixed-length masking, generated manifests
  declare Veil directly, and runtime bypass features remain disabled.
- Critical lifecycle, REST, RPC, and Stream boundaries now emit stable
  structured `event` fields at production-visible levels, including startup,
  readiness, application logic, completion, cancellation, settlement, shutdown,
  and failure; payloads and credentials remain excluded.
- REST and RPC error boundaries now emit structured warning/error events with
  status, error kind, request ID, and trace ID; REST error rendering remains
  inside the request context scope, and tracing initialization failures are no
  longer silently ignored.
- Generated Toasty PostgreSQL `TIMESTAMP` and `TIMESTAMPTZ` fields now use
  native `jiff::civil::DateTime` and `jiff::Timestamp` values across models,
  mutations, predicates, ordering, projections, and grouped counts.
- Standardized generated REST and RPC error handling on numeric response codes:
  success remains `code: 0`, framework errors use numeric codes, and the
  parallel string business-code contract has been removed.
- Configured REST request-body limits now raise the matching native extractor
  limit, and optional gzip decoding runs before decompressed-size enforcement.
- Added backward-compatible `tokens_per_refill` governance rate limits with
  matching REST, RPC, Gateway, memory, and Redis token-bucket semantics.
- SQL model import now recognizes PostgreSQL named and anonymous table-level
  primary-key, unique, foreign-key, and check constraints without generating
  phantom `constraint` columns.
- Added `rozectl stream gen --broker rdkafka-cmake` for native Windows
  librdkafka builds while retaining `provider: rdkafka` at runtime.
- Persistent SQL outbox enqueue now participates in either SeaORM or Toasty
  PostgreSQL/MySQL transactions, and the relay accepts dynamically dispatched
  `Publisher` values.
- Generated REST/RPC manifests now inherit only dependencies actually declared
  by a parent workspace and fall back to explicit versions for missing entries.
- Added semantic `Conflict` and `FailedPrecondition` errors with stable HTTP,
  gRPC, metadata, localization, and round-trip mappings.
- Generated SeaORM queries now support `.primary()` and
  `.read_from(ReadSource)` while retaining replica reads by default, and emit
  bounded `primary`/`replica` source labels.
- Generated SeaORM repositories now expose `update_where()` with a
  single-statement conditional `execute()` terminal returning `UpdateResult`.
- Added redaction-safe typed application configuration, generated as a
  preserved `src/application_config.rs`, with normal Roze secret resolution
  and `ServiceConfig` validation.
- REST/RPC `--update` now migrates exact legacy generated config loaders to
  typed application configuration, preserves unrecognized custom loaders with
  an actionable warning, and keeps config loading reusable from extra binary
  targets without duplicate `application_config` declarations.
- Added the experimental `roze-ai` runtime with provider-neutral model, tool,
  agent, and event contracts; OpenAI-compatible invoke, SSE, and tool-call
  support; and validated, secret-safe provider configuration.
- Added independent transactional `rozectl ai generate` module generation with
  framework-owned glue, preserved application-owned agents/tools/prompts, an
  AI configuration example, and generated compile coverage. Existing REST,
  RPC, model, search, and stream generators are unchanged.
- Added experimental AI DAG composition, graph-as-tool adaptation, prompt
  templates, standard document/embedding/retriever/indexer contracts, bounded
  RAG pipelines, normalized `roze-search` hits, and incremental
  `--with-workflow`/`--with-rag` generation.
- Added parallel workflow layers, pluggable per-node checkpoints,
  interrupt/resume with graph and identity-scope validation, bounded
  sequential/parallel multi-agent task coordination, and incremental
  `--with-team` generation.
- Added deterministic workflow execution-event streams, a tenant/subject
  scoped `roze-storage` checkpoint adapter, and permission-aware model-selected
  delegation to registered Agent teams. Workflow/team generation now exposes
  these reusable adapters without changing existing generator behavior.
- Added `WorkflowNode::stream`, native `FnStreamNode` composition, and bounded
  backpressure-aware `CompiledGraph::stream_chunks` for strict linear
  workflows. Generated workflow modules expose the chunk stream without
  changing existing REST/RPC/model/search/stream generation.
- Added `roze-rate-limit`, providing shared memory and atomic Redis token-bucket
  stores, composite route/client-IP/subject/tenant/header keys, bounded store
  timeouts, fail-open/fail-closed behavior, and identity-safe observability.
- Unified generated REST, RPC, and Gateway rate-limit enforcement. HTTP
  rejections now include `Retry-After`, RPC uses `ResourceExhausted` with
  `retry-after` metadata, and generated production services reject accidental
  process-local rate limiting.
- Added transactional REST/RPC `--update` migration for projects created before
  the application lifecycle hook and logic preludes. Missing
  `register_services` hooks are added once, and resolvable custom module
  declarations are moved into application-owned preludes without update-time
  compatibility code in generated entrypoints.
- Added validated service configuration loading with production-strict unknown
  field detection, governance range checks, secret-safe debug output, automatic
  rate-limit Redis selection from an explicit URL or `cache.url`, and
  profile-scoped rate-limit namespaces.
- Updated the JWT signing and verification backend to `jsonwebtoken` 11 while
  retaining the explicit AWS-LC cryptography provider and existing JWT/OIDC
  validation contract.
- Generated REST/RPC services now honor `ROZE_CONFIG_PATH` before local
  `config.yaml` defaults. Prefixed `@websocket` routes now publish and apply
  their exact HTTP-upgrade auth exemption while leaving session authentication
  to application-owned frame logic.

All notable changes to Roze should be recorded in this file.

The format follows Keep a Changelog and Semantic Versioning.

## Unreleased

### Added

- Added a whitelisted asynchronous `ReportDataSource`/`ReportCatalog` boundary,
  shared export/chart executors, real SQLite tenant aggregation tests,
  in-flight cancellation, bounded result enforcement, and sanitized failures.
- Added preserved API/RPC `src/application.rs` hooks for attaching report data
  sources and other application resources without editing generated context or
  bootstrap files.
- Added a typed, cloneable application extension store to generated REST/RPC
  service contexts, and moved model context wiring under `src/model` so RPC,
  REST, and model updates no longer compete for `src/svc/mod.rs`.
- Added preserved root and REST group logic preludes. Custom module
  declarations now keep their original module level while generated logic
  indexes remain fully generator-owned.
- Added explicit ent database sharding with deterministic Jump Hash routing,
  per-shard primary/replica pools, pinned single-shard transactions, generated
  SeaORM/Toasty routing entry points, migration fan-out reports, readiness, and
  bounded shard metrics.
- Bound passing long-run evidence reports to verified fixed-runner artifacts,
  portable SHA-256 manifests, real elapsed duration, and GitHub provenance.
- Added promotion smoke coverage that rejects modified artifacts and shortened
  runs before a passing evidence report can be created.
- Added an independently testable evidence-report verifier used by the maturity
  gate for duration, resource, artifact, checksum, provenance, and boundary
  summary validation.
- Made MQ, Config Center, and Lifecycle soak wrappers time-bound by default
  instead of allowing small implicit operation caps to terminate 24h/72h runs
  early, and added elapsed-time throughput fields to their summaries.
- Changed the Gateway long-run harness to repeat the real network smoke
  topology and report cycle percentiles plus retry, fallback, reload, SSE, and
  WebSocket recovery counts.
- Added real HTTP load sampling to each Gateway soak cycle and aggregate
  request count, error count, and p50/p95/p99 latency to the boundary summary.
- Added automatic Etcd and Consul service re-registration after keepalive
  failure, preserving explicit deregistration while recovering transparently
  from registry process restarts.
- Added real Gateway routing probes that register an upstream once, coordinate
  external Etcd/Consul restarts, require visible disconnect and automatic
  recovery, and report route/recovery p99 latency to the evidence gate.
- Upgraded the Gateway soak to run static policy/stream checks beside isolated
  real Etcd and Consul fault injection. Promotion now rejects evidence without
  both registry workloads and recovery from every injected outage.
- Added a fixed-memory logarithmic latency histogram and used it to report
  p50/p95/p99 delivery, update, and lifecycle-cycle latency during long runs.
- Expanded fixed-runner host evidence with aggregate CPU busy time, first/last
  and minimum available memory, memory growth, tasks, established TCP
  connections, and allocated file handles.
- Added periodic failed-task and drain-hook-timeout injection to Lifecycle
  soak runs with detection counts and p99 fault-detection latency.
- Hardened promoted-report validation with area-specific boundary schemas,
  counter invariants, percentile ordering, fault-scenario counts, sampler
  continuity, and bounded available-memory decline.
- Added explicit fixed-runner p99 objectives for Gateway, MQ, Config Center,
  Lifecycle cycles, and Lifecycle fault detection.
- Added first-cycle MQ DLQ replay and Config Center rollback measurements with
  explicit recovery objectives in the promoted-report gate.
- Upgraded MQ soak to run in-memory reliability, real NATS JetStream, and real
  Kafka workloads concurrently while periodically restarting both brokers,
  with merged disconnect, recovery, throughput, and p99 evidence.
- Changed real Kafka delivery acknowledgment to wait for the broker offset
  commit result, so commit failures are returned to consumers instead of being
  reported as successful in-memory enqueue operations.
- Isolated MQ and Config Center Compose projects so fixed-runner cleanup cannot
  tear down another evidence workload with the same default project name.
- Made MQ and Config Center soak harnesses terminate peer workloads when one
  child exits unexpectedly, avoiding wasted long-run runner allocations.
- Added a real Etcd Config Center value/watch integration test and exercised it
  across healthy, disconnected, and recovered phases in the reference-system
  workflow.
- Upgraded Config Center soak to run admin rollback and real Etcd value/watch
  workloads concurrently while periodically stopping and restarting Etcd, with
  merged disconnect, recovery, throughput, and p99 evidence.
- Added `roze-service.yaml` and `rozectl service dependency add/list/remove`
  plus `service sync --check` as the single source of truth for managed RPC
  dependencies, Cargo entries, connection defaults, readiness, and generated
  `ServiceContext` clients in both API and RPC consumer services. Manifests
  record and validate the generated consumer kind.
- Added same-volume transactional generation for API, RPC, stream, model and
  search outputs. Rendering, dependency synchronization and formatting happen
  in a staging project, and failed generation leaves the existing project
  unchanged.
- Added three authoritative production reference-system inputs and a generated
  compile matrix covering REST/SQL/search, managed REST-to-RPC dependencies,
  stream workers, repeated updates, dependency synchronization, and generated
  operations assets.
- Added real Redis and NATS integration tests plus a Docker-backed reference
  systems workflow covering registry, migration rollback, Kafka, Elasticsearch,
  and dependency disconnect/recovery.
- Production soak jobs now finalize terminal run metadata, host resource
  aggregates, Markdown summaries, and checksums for successful, failed, and
  prematurely ended workloads before returning the workload status.

### Fixed

- Ensure standalone Toasty and SeaORM model generation declares the direct
  `serde` dependency with derive support required by generated model structs;
  repeated `--update` runs preserve the normalized dependency.
- Preserve PostgreSQL `BIGINT`/`BIGSERIAL`/`INT8` as signed `i64` and map
  `TIMESTAMP`/`TIMESTAMPTZ` to SeaORM's native chrono-backed datetime types in
  SQL generation and database inspection. MySQL `BIGINT UNSIGNED` remains
  `u64`, and generated SeaORM manifests enable `with-chrono` plus chrono's
  `clock` and `serde` features automatically, including existing dependencies.
- Apply configured service JWT authentication in generated REST common
  middleware, populate request context only from verified claims, strip
  untrusted identity headers by default, and support explicit public-route and
  trusted-proxy configuration.
- Accept standalone SQL `CREATE INDEX` and `CREATE UNIQUE INDEX` declarations
  in model generation, including composite and PostgreSQL partial indexes.
- Made the first `service dependency add` canonicalize adopted Cargo path
  dependencies immediately, so a following `service sync --check` no longer
  reports ordering drift.
- Kept managed API/RPC dependencies canonical after API, RPC, model, and search
  regeneration by running the authoritative service synchronizer inside the
  generation transaction before commit.

## [1.0.0] - 2026-07-16

Roze 1.0 establishes the stable public contract for the Rust framework,
`rozectl`, generated Rust services, and TypeScript/JavaScript Web clients.
Runtime evidence remains independently reported and must not be inferred from
the API stability label.

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
