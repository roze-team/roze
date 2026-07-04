# Config Center Contract

This document defines the stable behavior expected from Roze Config Center before it can be described as production-ready.

## Maturity

Status: `beta`.

Config Center is usable for internal pilots and controlled deployments. It must not be marketed as fully stable until admin snapshot persistence or an external control plane is wired into deployment, watch isolation is covered by integration tests, and long-run release evidence exists.

## Source Priority

Configuration is loaded in this order:

1. Etcd, when `ROZE_CONFIG_CENTER_ETCD_ENDPOINTS` is set.
2. Environment payload, when `ROZE_CONFIG_CENTER_ENV_KEY` is set and Etcd is not active.
3. File fallback, when `ROZE_CONFIG_CENTER_FILE` is set or a service-local config file exists.

A lower-priority source must not override a successfully loaded higher-priority source during the same reload cycle.

## Etcd Behavior

Required environment variables:

- `ROZE_CONFIG_CENTER_ETCD_ENDPOINTS`: comma-separated Etcd endpoints.
- `ROZE_CONFIG_CENTER_KEY` or `ROZE_CONFIG_CENTER_ETCD_KEY`: exact config key.
- `ROZE_CONFIG_CENTER_NAMESPACE` and `ROZE_CONFIG_CENTER_APP`: optional namespace/app values used to derive a default key.

Runtime behavior:

- Initial load uses Etcd v3 range read.
- Native watch uses Etcd v3 watch for the same key.
- Watch records `mod_revision` or header `revision`.
- Reconnect resumes from `last_revision + 1` when possible.
- If native watch is unavailable or the stream closes, Config Center falls back to polling.
- Poll interval defaults to 5 seconds and can be overridden with `ROZE_CONFIG_CENTER_POLL_SECS`.

## Reload Semantics

A reload has four possible outcomes:

| Outcome | Memory config | Event | Required behavior |
| --- | --- | --- | --- |
| unchanged valid payload | unchanged | success with `changed = false` | no dependent rebuild required |
| changed valid payload | replaced | success with `changed = true` | emit diff and section signatures |
| invalid payload | preserved | failure | keep the last valid config |
| source read failure | preserved | failure | log and retry according to source policy |

Invalid config must never replace the last valid in-memory config.

## ReloadResult Fields

`ReloadResult` is the observable reload event. It must include:

- `version` and `old_version`
- `hash` and `old_hash`
- `ts_millis`
- `source`: `etcd`, `env`, or `file`
- `namespace`, `app`, and `key` when known
- `changed`
- `diff`
- `section_signatures`
- `success`
- `error` on failure
- `config` on success

Hashes must be stable for semantically identical structured config values.

## Section Events

`ReloadResult::change_events()` exposes section-scoped events.

Each event contains:

- `section`: top-level section name, `root`, or `*` for failure events.
- `paths`: changed field paths under the section.
- `diff`: section-local diff entries.
- `section_hash`: stable section hash on success.
- inherited reload metadata: version, source, namespace, app, key, changed, success, and error.

Section events are intended for audit logs and targeted subsystem rebuilds.

## Failure Isolation

Listener behavior is part of the public contract:

- A slow listener must not block config reload forever.
- A failing listener must be logged and isolated from other listeners.
- Listener timeout defaults must be documented and configurable before stable.
- Failed listener delivery must not roll back a successfully parsed config.

Current status: listener failure and timeout handling exists, but production-stable status requires broader tests and explicit timeout configuration docs.

## Management API Semantics

`ConfigCenterAdminStore` provides the management semantics required by the
stable contract. It can persist and restore a JSON snapshot with version
history, audit records, active version, and watch status.

| Capability | Required behavior |
| --- | --- |
| read current config | expose version, hash, source, namespace, app, and key |
| publish/update config | validate before commit and reject invalid payloads |
| audit history | record who/what/when/source/version/hash |
| rollback | restore a previous valid version by version/hash |
| watch status | expose native watch vs polling, last revision, and last error |
| permission model | separate read, write, rollback, and audit permissions |

The snapshot store is enough to prove behavior in tests and can support simple
single-node deployments. Production-stable status for broader deployments still
requires one of the following:

- a deployed snapshot persistence strategy with backup/restore procedures, or
- an external control plane that implements the same fields and permissions.

An HTTP/admin API can be added on top of these semantics, but it must not weaken
validation, audit, rollback, or permission behavior.

## Migration Contract

Breaking Config Center changes include:

- changing source priority
- changing reload failure semantics
- changing hash/diff format
- changing event field names
- changing default watch or polling behavior
- removing environment variables

Every breaking change must be documented in `docs/upgrade.md` and release notes.

## Release Gate Requirements

Before a release can claim Config Center stability:

- `cargo test -p roze-config config_center` passes.
- Failure isolation tests cover slow and failing listeners.
- Rollback behavior is tested with invalid update payloads.
- Watch fallback from native watch to polling is tested or documented as a manual integration test.
- Management semantics and permission model are implemented and tested.
- Snapshot persistence or external control-plane integration is exercised in deployment before the module moves to `stable`.
