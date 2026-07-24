# roze-pink framework gap resolution - 2026-07-23

This report maps the four gaps reproduced by `roze-pink` against the Roze
worktree based on revision `d999ef6c7`.

## Implemented

- RZ-001: generated REST services enable `rest.connect_info`, retain the
  standard `RestService`/`ServiceGroup` lifecycle, and expose both the direct
  `ConnectInfo<SocketAddr>` peer and fail-closed `ClientIp` trusted-proxy
  resolution.
- RZ-002: `roze-transaction-sql` implements PostgreSQL/MySQL persistent
  Outbox storage, bundled migrations, transaction-local enqueue, concurrent
  lease claims, recovery, retries, dead letters, replay, cleanup, and metrics.
- RZ-003: generated REST/RPC contexts choose Redis idempotency from cache
  configuration, optionally fail fast on Redis startup health, and reject
  process-local stores for production routes declaring idempotency.
- RZ-004: `roze-config` resolves environment, file, and pluggable secret
  references before validation; JWT arrays can be merged by key ID through one
  environment value and all key diagnostics remain redacted.

## Verification boundaries

Runtime unit tests cover untrusted peer spoofing, multi-hop trusted proxies,
IPv4/IPv6, malformed chains, secret reference resolution, rotation merging,
short/missing JWT keys, Redis state-machine configuration, SQL migration
contracts, and generated configuration.

The ignored `roze-transaction-sql` tests require
`ROZE_TEST_POSTGRES_URL`/`ROZE_TEST_MYSQL_URL` and exercise real concurrent
claims, store reconstruction, failed-consumer lease recovery, transactional
enqueue, and completion state. Redis multi-instance evidence remains available
through the existing `ROZE_TEST_REDIS_URL` ignored test.

Passing compile/unit gates are recorded in the implementing Git commit. These
short tests are implementation evidence, not 24h/72h production-soak evidence.
