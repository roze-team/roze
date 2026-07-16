# RPC Client Configuration

## Service Dependency Manifest

The preferred 1.x workflow declares cross-service dependencies once in
`roze-service.yaml`. Both generated API services and generated RPC services can
consume these dependencies. Internal service-to-service dependencies use the
governed RPC client path; do not hand-edit generated client fields or accessors.

For a payment service that calls the order service:

```bash
rozectl service dependency add order \
  --project services/shop-payment-rpc \
  --crate shop-order-rpc \
  --path ../shop-order-rpc \
  --contract ../shop-order-rpc/order.api \
  --etcd-host http://127.0.0.1:2379 \
  --etcd-key shop-order-rpc \
  --timeout-ms 2000
```

The command validates the upstream path crate and contract, writes the
manifest, and immediately synchronizes:

- the `*-rpc` path dependency in `Cargo.toml`;
- non-secret connection defaults in `config/roze-dependencies.yaml`;
- managed RPC client fields, startup connections, readiness registration, and
  accessors in `src/svc/mod.rs`.

The manifest records the generated consumer kind explicitly:

```yaml
version: 1
service: shop-payment-rpc
kind: rpc
dependencies:
  order:
    protocol: rpc
    crate: shop-order-rpc
    path: ../shop-order-rpc
    target: http://127.0.0.1:4002
    timeout_ms: 2000
```

Use `kind: api` for a generated API consumer. `dependency add` detects the kind
when it first creates the manifest, and `service sync` rejects a manifest whose
kind no longer matches the generated API or RPC boundaries.

The generated dependency config is loaded before the service's `config.yaml`.
The final precedence is:

1. `config/roze-dependencies.yaml` generated defaults;
2. `config.yaml` deployment configuration;
3. `ROZE__...` environment overrides.

This allows production endpoints and deployment-specific values to override
the manifest defaults without putting tokens, passwords, or certificates in
the dependency manifest.

Use these commands for lifecycle operations:

```bash
rozectl service dependency list --project services/shop-payment-rpc
rozectl service dependency remove order --project services/shop-payment-rpc
rozectl service sync --project services/shop-payment-rpc
rozectl service sync --project services/shop-payment-rpc --check
```

`service sync --check` writes nothing and fails when Cargo dependencies,
dependency defaults, or managed `ServiceContext` sections have drifted. The
release pipeline should run it for every service containing
`roze-service.yaml`.

On first adoption, `dependency add` imports existing local `*-rpc` path
dependencies and their named `rpc_clients` configuration. It refuses migration
when an existing dependency has no usable connection configuration instead of
silently removing that client. Once present, `roze-service.yaml` is the source
of truth for managed cross-service dependencies.

## Runtime Configuration

`roze-config::ServiceConfig` supports both a default RPC client and named RPC
clients.

Use `rpc_client` for services that call one upstream RPC service:

```yaml
rpc_client:
  etcd:
    hosts:
      - http://127.0.0.1:2379
    key: user-rpc
  timeout_ms: 2000
```

Use `rpc_clients.<name>` for REST or RPC services that aggregate multiple RPC
services:

```yaml
rpc_clients:
  user:
    etcd:
      hosts:
        - http://127.0.0.1:2379
      key: user-rpc
  order:
    endpoints:
      - 127.0.0.1:4002
```

Each named entry uses the same fields and connection-mode rules as
`rpc_client`: select exactly one of `target`, `endpoints`, or `etcd`.

`ServiceConfig::rpc_client_config(name)` returns a cloned named config when one
exists. If the named entry is absent, it falls back to `rpc_client`, which keeps
single-client services simple while allowing generated multi-RPC service
contexts to use names.

`ServiceConfig::rpc_client_config_ref(name)` provides the same lookup without
cloning.

## Generated service dependencies

`rozectl api generate` and `rozectl rpc generate`/`rpc protoc` discover local
RPC service dependencies declared as `*-rpc` path dependencies in the target
service's `Cargo.toml`. The final name segment before `-rpc` is the named client
key, so `shop-catalog-rpc` maps to `rpc_clients.catalog`. REST `.api` imports of
pure RPC contracts declare the same dependency surface automatically.

Generated `ServiceContext` code connects each declared client once during
startup, stores the cloneable client in a framework-owned field, registers an
`rpc:<name>` readiness dependency after connection succeeds, and exposes a
`<name>()` accessor. Client channels are owned by the service context and are
dropped through the normal service shutdown lifecycle.

## Retry Backoff And Deadlines

Generated RPC clients pass the inbound `roze_context::Context` into the shared
retry executor. Retryable failures use exponential full-jitter backoff: attempt
`n` samples a delay from zero through
`min(backoff_ms * 2^(n-1), max_backoff_ms)`. The retry budget is isolated by
service and method.

Before sleeping, the executor rejects a retry when its sampled delay would
consume the remaining deadline. It checks cancellation before and after the
sleep, and `roze_resilience_decisions_total` records `attempt` only immediately
before a real retry call. Budget exhaustion, deadline exhaustion, and
cancellation use the bounded decisions `budget_exhausted`,
`deadline_exhausted`, and `cancelled`.

The following manual flow remains available for projects that have not adopted
`roze-service.yaml`:

```toml
[dependencies]
shop-catalog-rpc = { path = "../shop-catalog-rpc" }
```

```yaml
rpc_clients:
  catalog:
    endpoints:
      - 127.0.0.1:4002
```
