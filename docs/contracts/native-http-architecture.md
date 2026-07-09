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
- path parameter capture through request extensions
- matched route-template extraction with `MatchedPath` for metrics, tracing,
  route-aware middleware, and handlers; `MatchedPath` supports `as_str`,
  `Deref<Target = str>`, and display formatting for concise observability code
- original request URI extraction with `OriginalUri`, preserving a stable
  request URI view in extensions before route dispatch
- method dispatch with `MethodRouter`
- `any` and `any_service` endpoints for method-independent handlers/services
- `MethodFilter` plus `on` and `on_service` for registering one
  handler/service against multiple standard HTTP methods
- standard method helpers for GET, POST, PUT, PATCH, DELETE, HEAD, OPTIONS,
  TRACE, and CONNECT
- GET routes implicitly satisfy HEAD requests when no explicit HEAD route is
  registered, and all HEAD responses preserve status/headers while omitting the
  response body
- service helper variants for each standard method, such as `get_service` and
  `MethodRouter::post_service`, so generated routes can attach Tower services
  without wrapping them as handlers
- `405 Method Not Allowed` responses include an `Allow` header for known paths
- route-level and router-level `method_not_allowed_fallback` hooks for
  generated services that need custom 405 payloads
- route nesting with `Router::nest(prefix, router)`
- service nesting with `Router::nest_service(prefix, service)`
- router composition with `Router::merge(router)`
- `Router::merge` accepts any `Into<Router>`, and `MethodRouter` converts into
  a root-path router for compact composition
- service-backed route registration
- Axum-style `Router::route_service(path, service)` for attaching a Tower
  service to all methods for one path; method-specific services use
  `route(path, get_service(service))` and the other method service helpers
- handler-backed route registration
- Roze `Handler` conversion into Tower services
- handler-level `Handler::layer` for applying Tower layers to one handler
  before it is registered on a route
- handler-level `Handler::with_state` for injecting state into a single
  handler service without wrapping an entire router
- handler-level `Handler::with_state_from_ref::<Outer, Inner>` for injecting
  both an application state and an explicit substate into one handler service
- `Router::as_service` and `Router::into_service` adapters for tests and
  Tower call sites that need an explicit service value
- `Router::into_make_service` and `RestServer::from_make_service` for the
  serve boundary, separating request handling from per-connection service
  construction while keeping `RestServer::new(addr, service)` available for
  existing request services
- `ConnectInfo<T>` extraction with `Router::into_make_service_with_connect_info`
  so peer/connection metadata can be injected at the make-service boundary and
  consumed by handlers without becoming global application state
- `Router::layer` and `MethodRouter::layer` for Tower layers that preserve the
  current infallible route error model
- `handle_error(layer, handler)` and `HandleErrorLayer`, also available under
  `roze_http::error_handling`, adapt fallible Tower layers back into Roze's
  infallible HTTP service boundary by mapping handler, route, and router layer
  errors into `IntoResponse` values
- `Router::route_layer` for layers that apply only to matched routes and leave
  fallback responses untouched
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
- `Extension<T>` implements `Layer`, and `middleware::AddExtensionLayer` /
  `AddExtension` can insert cloned values into request extensions for
  downstream `Extension<T>` and `State<T>`-style extraction
- fallback handlers and services with Axum-style `fallback`,
  `fallback_service`, and `reset_fallback`
- route presence introspection with `has_routes`
- `Debug` summaries for `Router` and `MethodRouter` expose route/method
  structure without depending on handler or service internals
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
