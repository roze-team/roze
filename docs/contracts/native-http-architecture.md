# Roze Native HTTP Architecture

Roze native HTTP takes inspiration from the public structure of
`tokio-rs/axum`, especially its separation of routing, extraction, response
conversion, middleware, body handling, and serving. Roze does not depend on
Axum and must not expose Axum types in generated services or framework crates.

## Borrowed Design Ideas

- Keep the transport edge thin. Hyper accepts connections, then requests are
  converted into Roze-owned `Request<Body>` before application code sees them.
- Make everything a Tower `Service` where practical. Routers, generated
  handlers, gateway services, and admin services should compose through Tower
  instead of bespoke runtime glue.
- Use small conversion traits for ergonomics. Roze provides
  `roze_http::IntoResponse` so handlers can return plain text, JSON wrappers,
  `ApiResponse<T>`, `RozeError`, `Result<T, E>`, or
  `roze_http::response::Result<T>` without tying core response logic to a web
  framework.
- Keep router construction macro-free. Route registration should remain an
  explicit builder API that generated code can write and tests can inspect.
- Split path matching from method dispatch. Roze uses `matchit` directly for
  route templates such as `/users/{id}`, then dispatches to a Roze-owned
  `MethodRouter`.
- Keep path definitions explicit. Route paths and nest prefixes must be
  non-empty and start with `/`; Roze rejects invalid paths during router
  construction instead of silently normalizing them. Path validation, legacy
  capture rejection, nest-prefix validation, and nested path composition are
  isolated in `router/path.rs` rather than mixed into runtime dispatch.
- Keep middleware separate from routing. Cross-cutting behavior belongs in
  Tower layers or Roze middleware primitives, not inside business handlers.

## Roze-Specific Boundaries

- `roze-http` owns HTTP request/response/body types exposed to applications.
  Hyper body types are an implementation detail of `RestServer`.
- `roze_http::body` is the stable body utility namespace for generated code
  and applications, exporting `Body`, `Bytes`, `empty`, `full`, and
  limit-aware `to_bytes` without exposing Hyper request bodies.
- `roze-result` and `roze-error` stay transport-neutral. HTTP serialization is
  performed by `roze-http`.
- `rozectl` must generate Roze-native handlers and router registration. It must
  not generate imports from external HTTP frameworks.
- `roze-middleware` owns context, tracing, metrics, auth, rate-limit, breaker,
  shedding, and idempotency contracts in a runtime-neutral form.
- `roze-gateway` and `roze-admin` are Tower services. They may use `roze-http`
  helpers, but must not become special router implementations.

## Current Native HTTP Surface

```rust
use http::StatusCode;
use roze_http::{routing::get, Router};

let app = Router::new()
    .route("/healthz", get(|| async { "ok" }))
    .route("/readyz", get(|| async { StatusCode::OK }));
```

`Router` is intentionally small at this stage:

- `matchit` route-template matching
- route templates use modern `{param}` and `{*wildcard}` captures; legacy
  `:param` and `*wildcard` segments are rejected
- path parameter capture through request extensions
- matched route-template extraction with `MatchedPath` for metrics, tracing,
  route-aware middleware, and handlers; `MatchedPath` supports `as_str`,
  `Deref<Target = str>`, display formatting, and synchronous extension reads;
  construction remains internal so middleware cannot forge route metadata;
  immutable route templates use shared `Arc<str>` storage so request dispatch
  and extractor clones do not copy the template string
- fallback handlers and services preserve `OriginalUri` but do not expose
  `MatchedPath`, keeping route-aware observability scoped to matched routes
- original request URI extraction with `OriginalUri`, preserving a stable
  request URI view in extensions before route dispatch
- method dispatch with `MethodRouter`
- `any` and `any_service` endpoints for method-independent handlers/services;
  these endpoints serve otherwise-unmatched methods directly and skip default
  `Allow` header generation
- `MethodFilter` plus `on` and `on_service` for registering one
  handler/service against multiple standard HTTP methods
- `MethodFilter::ALL`, `from_method`, `matches`, `methods`, `intersects`,
  `without`, `complement`, and bitwise union/intersection/difference/not
  operators make the same
  standard-method set reusable in gateway policy, middleware, tests, and
  generated route inspection; method-set representation and conversion live in
  the dedicated `router/method_filter.rs` module instead of the route graph
- `MethodRouter` implements Tower `Service` directly and exposes
  `into_make_service` / `into_make_service_with_connect_info::<T>` for
  standalone method-only services that do not need path routing
- `Router` stores its immutable runtime graph in `Arc<RouterInner>`; clones and
  ordinary `IntoMakeService` connection services share the graph, while
  consuming builder methods use copy-on-write when a clone is still alive
- request dispatch splits the request into HTTP parts and body, matches by
  borrowing `parts.uri.path()`, writes route context into `parts.extensions`,
  and then recombines the original body; routing does not allocate a path copy
- the Hyper/Tower server boundary uses a concrete `BoxError` newtype and a
  dedicated `TowerToHyperService` adapter, keeping body-error lifetimes and
  spawned connection futures explicit instead of relying on closure inference
- both native `RestServer` connection loops enable Hyper upgrades, allowing
  Tower services such as `roze-gateway` to take ownership of WebSocket and
  other HTTP/1.1 upgraded connections without exposing Hyper bodies publicly
- connect-info make services wrap Router, MethodRouter, or Handler services in
  `ConnectInfoService` and inject connection metadata at request entry; they do
  not apply a state layer to every route or copy the route graph per connection;
  Router and MethodRouter make-service construction lives behind the dedicated
  `router/into_make_service.rs` boundary
- erased route storage and Tower layer application live behind
  `router/route.rs`, keeping service normalization out of Router composition
- top-level HTTP method handler and service constructors live behind
  `router/method_routing.rs` and are generated from shared method macros
- MethodRouter's standard chained handler and service methods use the same
  method-routing macro boundary, leaving custom `on` and `any` behavior explicit
- Router's path-bound `get`, `post`, and other standard method shortcuts are
  generated by the same internal method-routing macro boundary
- MethodRouter storage, composition, fallback, layering, make-service adapters,
  and Tower Service dispatch live in `router/method_routing.rs`
- router behavior tests live in `router/tests.rs`; compile-time assertions pin
  Clone, Send, Sync, Tower Service, future, and make-service boundaries
- per-path route groups and their cached Allow metadata live behind
  `router/path_router.rs`, separating path-owned state from Router composition
- matchit nodes, route groups, and the exact-path index are owned together by
  `PathRouter`, preventing their indices from drifting as Router evolves
- PathRouter owns route-group creation and merge invariants, including method
  overlap checks, fallback conflicts, and cached Allow metadata refresh
- request path matching, route-parameter extension insertion, HEAD-to-GET
  selection, and path-local 405 resolution are performed by PathRouter
- route presence, path-local 405 fallback propagation, and route-layer
  application are PathRouter operations; Router separately layers global fallback
- `Router::route` normalizes the public path then delegates MethodRouter endpoint
  insertion, overlap rejection, fallback merge, and Allow refresh to PathRouter
- nested path graph rewriting, strip-prefix wrapping, and path graph merging are
  PathRouter operations; Router retains ownership of global fallback conflicts
- PathRouter's matcher, route groups, and exact-path index are private; Router
  diagnostics and copy-on-write tests use narrow read-only query methods
- global fallback state is represented by `Fallback::{Default, Custom}` rather
  than a service plus boolean, making merge, reset, layer, and dispatch coherent
- handler, routed service, and layer values are `Clone + Send + Sync + 'static`
  and are erased through Tower `BoxCloneSyncService`, keeping the shared router
  graph safely usable across runtime worker threads
- `MethodRouter::merge` composes distinct method endpoints and rejects
  overlapping method handlers or overlapping method-not-allowed fallbacks
- standard method helpers for GET, POST, PUT, PATCH, DELETE, HEAD, OPTIONS,
  TRACE, and CONNECT
- typed and raw path extractors percent-decode captures exactly once; form
  delimiters such as `&`, `=`, and `+` remain part of the captured path value
  instead of being reinterpreted as fields
- route matching stores decoded path captures or an invalid-UTF-8 marker in a
  request-scoped shared `RouteParams`; repeated typed/raw extraction borrows or
  clones that shared storage instead of decoding and allocating again
- `Path<T>` uses a path-specific Serde deserializer and supports one-capture
  scalar values, positional tuples/sequences, and named structs/maps
- `Option<Path<T>>` has identical request and request-parts semantics, allowing
  one handler to serve routes both with and without path captures
- `RawPathParams::from_request_extensions` and its optional counterpart expose
  the same decoded parameter view synchronously to Tower middleware; async
  request extractors delegate to this shared core
- GET routes implicitly satisfy HEAD requests when no explicit HEAD route is
  registered, and all HEAD responses preserve status/headers while omitting the
  response body
- a shared `RouteFuture` applies protocol-level response finalization for both
  `Router` and standalone `MethodRouter`: HEAD records a known content length
  before removing the body, while successful CONNECT responses remove ordinary
  body framing headers and body content; this protocol boundary lives in the
  dedicated `router/future.rs` module so route matching and method selection do
  not own response-finalization policy
- service helper variants for each standard method, such as `get_service` and
  `MethodRouter::post_service`, so generated routes can attach Tower services
  without wrapping them as handlers
- `405 Method Not Allowed` responses include an `Allow` header for known paths
  and standalone method routers; empty method routers return an empty `Allow`
  header rather than omitting it; the internal `MethodNotAllowed` Tower service
  and `AllowHeader` cache in `router/method_not_allowed.rs` build and validate
  the header when routes are registered or merged, so 405 dispatch only clones
  an existing `HeaderValue`
- route-level and router-level `method_not_allowed_fallback` /
  `method_not_allowed_fallback_service` hooks for generated services that need
  custom 405 payloads from handlers or Tower services
- `layer` wraps route endpoints and fallbacks, while `route_layer` wraps only
  registered route endpoints and leaves both 404 and method-not-allowed
  fallbacks untouched
- route nesting with `Router::nest(prefix, router)`; nested handlers receive a
  prefix-stripped current URI while `MatchedPath` keeps the complete external
  route template and both `OriginalUri` and `Option<OriginalUri>` keep the
  complete external request URI
- service nesting with `Router::nest_service(prefix, service)`; mounting at
  `/prefix` serves `/prefix`, `/prefix/`, and `/prefix/...`; nested services
  receive the same prefix-stripped current URI semantics while `OriginalUri`
  preserves the external request URI
- nested routers and services expose a composable `NestedPath` extractor with
  the complete mount prefix; this keeps redirect and mount-aware URL building
  independent from both the stripped current URI and the external
  `OriginalUri`; URI rewriting and nested-path accumulation are isolated in the
  internal `router/strip_prefix.rs` service boundary; captured nest prefixes
  such as `/{tenant}` are matched segment-by-segment and strip the corresponding
  concrete request segment while preserving query parameters
- router fallbacks clear stale `MatchedPath` and route parameters inherited
  from an outer router while preserving `OriginalUri` and `NestedPath`
- the default 404 policy is a zero-sized internal Tower `NotFound` service in
  `router/not_found.rs`; router construction and `reset_fallback` share the
  same explicit fallback implementation
- nesting prefixes may contain named captures but cannot contain catch-all
  wildcard captures
- root nesting is rejected: compose routers with `Router::merge`, and use
  `Router::fallback_service` for root-level service fallback
- router composition with `Router::merge(router)`
- `Router::merge` follows explicit fallback composition rules: a default
  fallback can be replaced by the merged router's custom fallback, while merging
  two routers that both define custom fallbacks is rejected instead of silently
  choosing one
- `Router::merge` accepts any `Into<Router>`, and `MethodRouter` converts into
  a root-path router for compact composition
- service-backed route registration
- Axum-style `Router::route_service(path, service)` for attaching a Tower
  service to all methods for one path; method-specific services use
  `route(path, get_service(service))` and the other method service helpers
- service registration, method service helpers, and service-backed fallback
  APIs reject `Router` values as services; use `Router::nest` when composing
  routers so child route matching remains explicit
- route-builder APIs, including handler helpers, service helpers, fallback
  hooks, and method-not-allowed hooks that can reject invalid composition, use
  caller-tracked panics so diagnostics point at the route registration site
- `Router`, `MethodRouter`, and their owned service/make-service adapters are
  `must_use`; accidentally discarding a consuming builder result is reported by
  the compiler instead of silently dropping route configuration
- handler-backed route registration
- Roze `Handler` conversion into Tower services
- handler-level `Handler::layer` for applying Tower layers to one handler
  before it is registered on a route
- handler-level `Handler::with_state` for injecting state into a single
  handler service without wrapping an entire router
- handler-level `Handler::with_state_from_ref::<Outer, Inner>` for injecting
  both an application state and an explicit substate into one handler service
- handler-level `Handler::into_make_service` and
  `Handler::into_make_service_with_connect_info::<T>` for serving a standalone
  Roze handler through the same make-service boundary as routers, including
  connection metadata injection through `ConnectInfo<T>`
- `Router::as_service` and `Router::into_service` adapters for tests and
  Tower call sites that need an explicit service value; their borrowed and
  owned Tower implementations live in `router/service.rs`, outside path and
  method dispatch
- `Router::into_make_service` and `RestServer::from_make_service` for the
  serve boundary, separating request handling from per-connection service
  construction while keeping `RestServer::new(addr, service)` available for
  existing request services
- `ConnectInfo<T>` extraction with `Router::into_make_service_with_connect_info`
  so peer/connection metadata can be injected at the make-service boundary and
  consumed by handlers without becoming global application state
- `Router::layer` and `MethodRouter::layer` for Tower layers that preserve the
  current infallible route error model
- Router and MethodRouter state injection share the internal Tower services in
  `router/state.rs`; top-level state and explicit `FromRef` substates are added
  at request entry without coupling state storage to path or method matching
- `handle_error(layer, handler)` and `HandleErrorLayer`, also available under
  `roze_http::error_handling`, adapt fallible Tower layers back into Roze's
  infallible HTTP service boundary by mapping handler, route, and router layer
  errors into `IntoResponse` values
- `Router::route_layer` for layers that apply only to matched routes and leave
  fallback responses untouched; calling it before any routes have been
  registered is rejected because it would otherwise be a silent no-op
- `MethodRouter::route_layer` for layers that apply only to method endpoints
  and leave method-not-allowed fallback responses untouched; calling it before
  any method endpoint has been registered is rejected for the same reason
- `middleware::from_fn` and `middleware::Next` for Axum-style function
  middleware over Roze-owned extractors, supporting parts extractors before the
  body-consuming request extractor and explicit continuation through
  `next.run(request).await`
- `middleware::from_fn_with_state` for function middleware with explicit
  middleware-local state injected into request extensions before extractor
  execution, making it available through `State<T>` and `Extension<T>`
- `middleware::map_response` for response-only middleware that can inspect or
  replace downstream responses while returning any Roze `IntoResponse` value
- `middleware::map_request` and `IntoMapRequestResult` for request-only
  middleware that can rewrite requests before routing continues or short-circuit
  with any Roze `IntoResponse` rejection
- `middleware::from_extractor::<E>()` for extractor-backed middleware that
  validates request parts, discards successful extractor values, and returns
  extractor rejections as responses without calling downstream services
- `middleware::from_extractor_with_state::<E, S>(state)` for extractor-backed
  middleware with explicit middleware-local state injected before validation,
  so reusable guard extractors can read `State<T>` or `Extension<T>` without
  wrapping an entire router
- `Extension<T>` implements `Layer`, and `middleware::AddExtensionLayer` /
  `AddExtension` can insert cloned values into request extensions for
  downstream `Extension<T>` and `State<T>`-style extraction
- fallback handlers and services with Axum-style `fallback`,
  `fallback_service`, and `reset_fallback`
- route presence introspection with `has_routes`
- `Debug` summaries for `Router` and `MethodRouter` expose route/method
  structure without depending on handler or service internals
- `MethodRouter::method_filter` reports only the methods that still use the
  default 405 behavior; it returns `None` when `any` or a custom
  method-not-allowed fallback changes that default method contract
- router and method-router state injection with `with_state`, consumed through
  `State<T>`
- `FromRef` plus router, method-router, and handler
  `with_state_from_ref::<Outer, Inner>` for explicit substate injection from a
  larger application state while preserving Roze's request-extension based
  state model
- typed handler argument extraction for zero through eight arguments, generated
  by a Roze-owned tuple macro that keeps parts extractors before the final
  body-consuming extractor
- `FromRequestParts` for extractors that do not consume the body, mirroring
  the parts/body split used by Axum while keeping Roze-owned traits
- `RequestPartsExt` adds `parts.extract::<T>().await` and
  `parts.extract_optional::<T>().await` convenience methods for composing
  Roze-owned parts extractors inside custom extractors and middleware
- `RequestExt` adds `request.extract::<T>().await`,
  `request.extract_optional::<T>().await`, `request.extract_parts::<T>().await`,
  and `request.extract_optional_parts::<T>().await`, letting custom extractors
  compose body-consuming and parts-only extractors without losing the request
  body
- `OptionalFromRequestParts` and `OptionalFromRequest` power `Option<T>`
  extractors, so missing optional state, extensions, connection info, empty
  query/body payloads, and similar absence cases can be represented without
  turning them into handler rejections
- `Result<T, T::Rejection>` extractors let handlers handle extractor failures
  explicitly instead of forcing immediate framework rejection responses
- minimal request extraction with `FromRequest`,
  `roze_http::extract::Request` / `roze_http::Request` aliases, `RawRequest`,
  `Parts`, `Path<T>`, `RawPathParams`, `RawQuery`, `RawForm`, `Query<T>`,
  `Form<T>`, `Json<T>`, and `OriginalUri`
- `Path<T>::from_request_extensions` and its optional counterpart expose typed
  path parsing synchronously to Tower middleware and tests; the internal route
  parameter representation stays private to `roze-http`
- `RawPathParams` exposes named route captures without deserializing them,
  useful for route-aware middleware, audit logs, and generic gateway policy;
  it supports `iter()` and `for (key, value) in &params`
- `RawQuery` exposes the unparsed URI query string for signature checks,
  gateway policy, observability, and proxy-style handlers
- `Query<T>::try_from_uri(&Uri)` and `Query<T>::optional_from_uri(&Uri)`
  expose typed query parsing outside request extraction, so middleware,
  gateway policy, and tests reuse the same parser as the extractor
- `RawForm` exposes raw urlencoded form data, reading the query string for GET
  requests and requiring `application/x-www-form-urlencoded` bodies for other
  methods
- raw body extraction with `Bytes` and `String` for webhook, signature,
  proxy, and debugging handlers that need the body without DTO parsing
- `DefaultBodyLimit` provides Axum-style extractor-local body limits: `Bytes`,
  `String`, `Json`, `Form`, and `RawForm` honor the default 2MiB limit, route
  layers can override it with `DefaultBodyLimit::max(bytes)`, and trusted
  endpoints can call `DefaultBodyLimit::disable()`
- `Json<T>::from_bytes(&[u8])` exposes the same JSON decoding path used by the
  request extractor for middleware, pre-buffered request flows, and focused
  tests
- `Form<T>::from_bytes(&[u8])` exposes the same urlencoded decoding path used
  by the request extractor, and `RawForm` follows Axum's GET/HEAD query-string
  semantics before falling back to urlencoded request bodies for other methods
- direct extraction of common HTTP request parts such as `Method`, `Uri`,
  `Version`, and `HeaderMap` without consuming the body
- `Host` extraction from the `Host` header or URI authority for
  multi-tenant routing, gateway policy, and observability use cases
- extractor newtypes dereference to their inner values where appropriate,
  keeping handler code terse without depending on Axum's helper macros
- fallback service and handler support
- `IntoResponse` conversion, including `()` for empty `200 OK` responses and
  header arrays as header-only responses
- `roze_http::response::{Response, Result, ErrorResponse}` gives generated
  handlers a concise response contract and a default error wrapper that accepts
  any `IntoResponse` error through `?`
- binary `IntoResponse` bodies for `Bytes`, `Vec<u8>`, byte slices, and byte
  arrays with `application/octet-stream`
- `roze_http::body` centralizes body construction and collection helpers,
  including `to_bytes(body, limit)` for the bounded reads used by extractors,
  middleware, tests, and low-level handlers
- `Html<T>` response helper for HTML bodies with `text/html; charset=utf-8`
  content type
- `Form<T>` can be used as a response wrapper for
  `application/x-www-form-urlencoded` bodies, matching its extractor role
- `IntoResponseParts` plus `ResponseParts` for composing response metadata,
  including `(StatusCode, R)`, `(HeaderMap, R)`, `(StatusCode, HeaderMap, R)`,
  and header-array tuples such as `([("x-roze", "yes")], R)`
- response parts can be optional with `Option<P>` and grouped as tuples so
  handlers can compose headers, extensions, and other metadata conditionally
- flat response tuples such as `(headers, Extension(trace), body)` and
  `(StatusCode, headers, Extension(trace), body)` are supported for concise
  handler returns
- `AppendHeaders` preserves duplicate header values for `Set-Cookie` and
  similar multi-value response headers
- `Extension<T>` can be used as response parts to attach typed response
  extensions for middleware and tests
- `Redirect` response helper with `to`, `temporary`, and `permanent`
  constructors for common `Location` responses

The next growth step is generated handler integration.

## Connection Concurrency And Drain

`RestServer` accepts TCP connections continuously and runs each Hyper HTTP/1
connection in an independent Tokio task. Connection handling must never be
awaited inline in the accept loop; doing so serializes clients and prevents
rate limits, bulkheads, and adaptive shedding from observing real concurrency.

Both direct-service and make-service entry points track connection tasks. On
shutdown they stop accepting new connections, wait up to
`graceful_shutdown_timeout` for active connections, then abort and reap any
remaining tasks. Concrete Router and boxed-service adapters erase generic
service futures before spawning, keeping the connection tasks `Send + 'static`.

## Non-Goals

- Reintroducing Axum as a dependency.
- Re-exporting Axum APIs or keeping compatibility aliases instead of Roze-owned
  API names.
- Copying Axum internals or implementation details.
- Making business logic depend on HTTP framework types.

## Migration Direction

1. Keep `RestServer` as the only Hyper accept loop.
2. Move generated route files to `roze_http::Router`.
3. Add typed extractors for path, query, headers, JSON, form, state, and
   context.
4. Rebuild middleware as Tower layers over `roze_http::IncomingRequest`.
5. Reattach admin, gateway, DTM, and generated API routes through the same
   Router/Service surface.
