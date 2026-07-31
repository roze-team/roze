# Redis topology contract

Roze centralizes standalone and Cluster connections in `roze-redis`.
`roze-cache`, Redis idempotency, and `roze-rate-limit` reuse that connection
abstraction, so `MOVED`/`ASK`, slot discovery, and topology refresh are handled
by the upstream Redis Cluster client rather than reimplemented per module.

Standalone configuration remains backward compatible:

```yaml
cache:
  url: redis://user:${REDIS_PASSWORD}@redis:6379
  namespace: roze
  default_ttl_secs: 300
```

Cluster configuration provides one or more seed nodes:

```yaml
cache:
  cluster_urls:
    - rediss://user:${REDIS_PASSWORD}@redis-0:6379
    - rediss://user:${REDIS_PASSWORD}@redis-1:6379
    - rediss://user:${REDIS_PASSWORD}@redis-2:6379
  namespace: roze
  default_ttl_secs: 300

governance:
  rate_limiter:
    store: redis
    redis_cluster_urls:
      - rediss://user:${REDIS_PASSWORD}@redis-0:6379
      - rediss://user:${REDIS_PASSWORD}@redis-1:6379
    unavailable_policy: fail-closed
```

When rate limiting or idempotency inherits the service `cache` configuration,
both the standalone URL and `cluster_urls` are propagated. Existing
single-URL services require no changes.

Generated REST and RPC `ServiceContext` code passes `cache.cluster_urls` to
the primary `RedisCache` constructor as well as the Redis idempotency store.
Regeneration with `--update` therefore keeps cache, idempotency, rate limiting,
readiness, and application `roze-redis` clients on the same declared topology.

The current Lua operations use exactly one key per invocation, so they remain
single-slot safe. New multi-key scripts or transactions must use an explicit
shared hash tag such as `{tenant-id}` and validate it before execution.

Credential-gated integration evidence:

```bash
ROZE_TEST_REDIS_CLUSTER_URLS=redis://127.0.0.1:7000,redis://127.0.0.1:7001,redis://127.0.0.1:7002 \
  cargo test -p roze-redis redis_cluster_round_trip_against_real_service \
  -- --ignored --nocapture
```
