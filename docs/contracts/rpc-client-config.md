# RPC Client Configuration

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

For an existing service, declare both surfaces before running `--update`:

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
