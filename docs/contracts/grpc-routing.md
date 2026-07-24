# Native gRPC routing

Generated RPC services use `roze_grpc::GrpcRouter` to dispatch standard
`/{service}/{method}` paths. The router accepts cloneable Tonic
`NamedService` implementations and returns the standard gRPC `UNIMPLEMENTED`
status for unknown services.

Roze owns this routing boundary:

- generated services register the application service and
  `grpc.health.v1.Health` through `GrpcRouter`;
- `tonic` is built without its optional `router` feature;
- the dependency graph must not contain `axum` or `axum-core`;
- generated application code must not import third-party HTTP router types.

This keeps gRPC transport on Hyper/Tower/Tonic while preserving Roze's
framework-owned service lifecycle, health reporting, graceful shutdown, and
SemVer-governed generated layout.

Regression coverage includes router dispatch and unknown-service behavior,
RPC health synchronization, and compilation plus strict Clippy checks for a
freshly generated RPC project.
