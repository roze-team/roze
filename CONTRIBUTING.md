# Contributing

Roze is still pre-release, so contributions should prioritize framework
credibility over new surface area.

## Current Priorities

1. Release system, maturity matrix, and generated-project compatibility.
2. Gateway v2, config hot reload, and MQ/Kafka semantics.
3. Unified governance across HTTP/RPC/Gateway/MQ.
4. Generator edge tests and OpenAPI/validator completeness.
5. Production examples, observability assets, and security model hardening.

## Development Checks

Run the focused generator tests before changing `rozectl`:

```bash
cargo test -p rozectl -- --skip postgres --skip mysql
```

Run the workspace before larger changes:

```bash
cargo fmt --all -- --check
cargo test --workspace
```

Changes that affect generated project layout must include tests proving:

- `--force` rebuilds generated projects.
- `--update` preserves user-owned logic files.
- `--update` preserves custom REST middleware.
- `--update` preserves `config.yaml`.
- Generated REST/RPC projects compile.

## Ownership Rules

Generated code is split into:

- Framework-owned glue: routes, handlers, generated DTOs, OpenAPI, protobuf,
  server/client glue, and build files.
- Application-owned files: REST `src/logic/**`, RPC `src/logic/**`, custom
  middleware, and `config.yaml`.

Do not change this ownership boundary without documenting migration behavior.
