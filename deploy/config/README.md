# Deployment configuration

Repository `apps/*/config.yaml` files are development defaults. Production
deployments must provide a deployment-owned YAML file and point the process to
it with `ROZE_CONFIG_PATH`:

```text
ROZE_CONFIG_PATH=/etc/roze/service.yaml
```

Use `rest.production.yaml` for generated REST services,
`gateway.production.yaml` for `roze-gateway`, and `dtm.production.yaml` for the
standalone DTM service. Copy a template into the
deployment repository, adjust service names, routes, endpoints, capacity, and
timeouts, then review it as versioned deployment configuration. Do not modify
the source-tree development file during deployment.

## Secrets

Keep secret payloads out of YAML and Git. The templates use Roze references
such as `env://REDIS_URL` and `env://ROZE_JWT_SECRET`. Kubernetes deployments
should inject those values from a `Secret`; file-based secret managers can use
`file:///var/run/secrets/...`. A referenced secret that cannot be resolved
causes startup to fail before the listener binds.

## Containers and Kubernetes

Mount the non-secret YAML read-only and set the authoritative path:

```yaml
env:
  - name: ROZE_CONFIG_PATH
    value: /etc/roze/service.yaml
envFrom:
  - secretRef:
      name: user-api-secrets
volumeMounts:
  - name: service-config
    mountPath: /etc/roze/service.yaml
    subPath: service.yaml
    readOnly: true
volumes:
  - name: service-config
    configMap:
      name: user-api-config
```

Container templates intentionally use JSON on stdout with ANSI disabled.
Let the platform handle collection, rotation, retention, and compression. For
VM or systemd deployments, a `logging.file` sink may be added using the fields
documented in `docs/contracts/logging.md`.

Bind container listeners to `0.0.0.0`; keep `127.0.0.1` only in development
files. Production profiles enable strict unknown-field validation and reject a
memory rate-limit store when rate limiting is active.

## Overrides and validation

`ROZE__SECTION__FIELD` environment variables override scalar YAML values. Use
these for small platform-specific differences; keep structural configuration
in YAML so it remains reviewable. Dependency defaults from
`config/roze-dependencies.yaml` load first, the deployment YAML overrides them,
and environment values apply last.

Run environment and deployment preflight checks before rollout. Roze resolves
secrets and validates the complete production schema again during service
startup, before binding the listener:

```bash
rozectl doctor --config /etc/roze/service.yaml --port 3000
rozectl service sync --project services/user-api --check
```

`roze-dtm` uses the same `ROZE_CONFIG_PATH` resolution as other services. Its
typed settings live under `application.dtm`; production rejects the memory
store and requires a resolved SQLite `database_url`. The storage backend is
probed by `/readyz` before a deployment is considered ready.
