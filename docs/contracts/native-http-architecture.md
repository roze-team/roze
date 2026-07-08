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
  `ApiResponse<T>`, `RozeError`, or `Result<T, E>` without tying core response
  logic to a web framework.
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
  route-aware middleware, and handlers
- method dispatch with `MethodRouter`
- `any` and `any_service` endpoints for method-independent handlers/services
- standard method helpers for GET, POST, PUT, PATCH, DELETE, HEAD, OPTIONS,
  TRACE, and CONNECT
- `405 Method Not Allowed` responses include an `Allow` header for known paths
- route nesting with `Router::nest(prefix, router)`
- service nesting with `Router::nest_service(prefix, service)`
- router composition with `Router::merge(router)`
- service-backed route registration
- handler-backed route registration
- Roze `Handler` conversion into Tower services
- `Router::layer` and `MethodRouter::layer` for Tower layers that preserve the
  current infallible route error model
- `Router::route_layer` for layers that apply only to matched routes and leave
  fallback responses untouched
- fallback handlers and services with `fallback_handler`, `fallback_service`,
  and `reset_fallback`
- route presence introspection with `has_routes`
- router and method-router state injection with `with_state`, consumed through
  `State<T>`
- typed handler argument extraction for zero, one, two, and three arguments
- `FromRequestParts` for extractors that do not consume the body, mirroring
  the parts/body split used by Axum while keeping Roze-owned traits
- minimal request extraction with `FromRequest`, `RawRequest`, `Parts`,
  `Path<T>`, `Query<T>`, `Form<T>`, and `Json<T>`
- fallback service and handler support
- `IntoResponse` conversion

The next growth steps are fallible layer error mapping and generated handler
integration.

## Non-Goals

- Reintroducing Axum as a dependency.
- Re-exporting Axum-compatible APIs.
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
