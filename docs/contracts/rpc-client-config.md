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

Use `rpc_clients.<name>` for API services that aggregate multiple RPC services:

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
