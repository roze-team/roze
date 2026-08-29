# Database Sharding Contract

Roze supports explicit database sharding with one deterministic routing model:
an application supplies a shard key, Roze hashes it with the versioned
`fnv1a64 + jump-consistent-hash` contract, and the selected shard owns the
operation and transaction.

Strings use their UTF-8 bytes, byte keys use their bytes unchanged, and integer
keys use little-endian bytes of their declared fixed width. `isize` and `usize`
are normalized to 64 bits so routing does not depend on the build target.
Changing this encoding or the routing algorithm is a data-placement
compatibility change.

Roze does not silently scatter a query across shards and does not emulate a
cross-shard transaction. Applications compose cross-shard reads explicitly.
Physical table sharding can be delegated to a ShardingSphere/Vitess-compatible
proxy by using `database.mode: proxy`.

## Configuration

Direct and proxy modes use one logical primary plus optional read replicas:

```yaml
database:
  mode: direct # or proxy
  url: ${DATABASE_URL}
  replicas:
    - ${DATABASE_REPLICA_URL}
  policy: round-robin
  max_connections: 20
  min_connections: 2
```

Native sharding uses one named topology. Shard IDs are stable operational
identifiers and are sorted before routing, so reordering YAML does not remap
keys:

```yaml
database:
  mode: sharded
  topology:
    name: commerce
    routing: fnv1a64-jump-v1
    shards:
      - id: shard-00
        primary: ${DATABASE_SHARD_00_URL}
        replicas:
          - ${DATABASE_SHARD_00_REPLICA_URL}
      - id: shard-01
        primary: ${DATABASE_SHARD_01_URL}
        replicas:
          - ${DATABASE_SHARD_01_REPLICA_URL}
  policy: round-robin
  max_connections: 20
  min_connections: 2
```

`routing` is required and makes the data-placement version reviewable in
configuration. `url` and top-level `replicas` must be absent in sharded mode.
Every shard must have one primary. The pool limits apply per primary or replica
connection. Duplicate, empty, or unsafe shard IDs fail during startup.

Changing the shard set changes placement. Because shard IDs are sorted into
stable bucket positions, a newly added shard ID must sort after every existing
ID (for example, append `shard-03` after `shard-02`). Under that constraint,
Jump Consistent Hash moves approximately `1 / new_shard_count` of keys. Inserting
an ID into the middle or removing/renaming an ID is a full placement change.
Roze does not move data automatically. Operators must complete an
application-owned migration or dual-read/write plan before publishing the new
topology.

## Ent Model Declaration

Sharded entities declare one `RozeShard` annotation:

```text
entity Order {
  table "orders"
  Annotations(RozeShard("tenant_id", "commerce", "order"))

  field id: i64 {
    primary
  }
  field tenant_id: i64 {
    immutable
  }
}
```

Arguments are shard-key field, topology name, and co-location group. The shard
key must exist, be non-null, and be immutable. Models in the same group must
use the same topology, key name, and key type.

SeaORM repositories for sharded entities require explicit routing:

```rust
let orders = ctx.model().order_for_key(&tenant_id)?;
let order = orders.query().where_id_eq(order_id).only().await?;
```

Calling `ctx.model().order()` for a sharded entity fails before executing SQL.
Non-sharded entities continue to use `ctx.model().order()` and the direct
connection.

Toasty callers explicitly select the shard primary before constructing a
query:

```rust
let mut db = ctx.model().toasty_db_for_key(&tenant_id)?;
let order = OrderRepository::query(&mut db)
    .where_id_eq(order_id)
    .only()
    .await?;
```

The generated Toasty sharding path currently routes to shard primaries.
SeaORM uses the configured per-shard read-replica policy. Applications that
need Toasty replica reads must expose that choice explicitly instead of
assuming read-after-write consistency.

## Transactions

`ShardedDatabase::transaction_for_key` resolves one shard before starting the
transaction. `ShardTransaction::ensure_key` rejects any key that resolves to a
different shard:

```rust
ctx.sharded_db()?
    .transaction_for_key(&tenant_id, |transaction| {
        Box::pin(async move {
            transaction.ensure_key(&tenant_id)?;
            // Execute all statements through transaction.connection().
            Ok(())
        })
    })
    .await?;
```

Generated SeaORM services expose the same one-shard boundary through the model
client while retaining normal generated repositories and builders:

```rust
ctx.model()
    .transaction_for_key(&tenant_id, |tx| {
        Box::pin(async move {
            tx.order().update_one(order_id).set_status(status).save().await?;
            tx.audit_event()
                .create()
                .set_order_id(order_id)
                .save()
                .await?;
            Ok(())
        })
    })
    .await?;
```

Reads made by repositories from this scoped client always use the selected
transaction primary, even when a query requests the replica source. Cache
invalidations are applied after commit and discarded on rollback.

Roze does not provide an implicit distributed transaction. Outbox and
application compensation remain explicit cross-shard orchestration; TCC/Saga
coordination is available from the independent
[`roze-dtm`](https://github.com/roze-team/roze-dtm) project.

## Migrations And Observability

`roze-migration` provides PostgreSQL, MySQL, and SQLite shard fan-out helpers.
They execute in declared order and return a `ShardMigrationReport` containing
the plan for every completed shard. Failure returns the failed shard plus the
completed outcomes; shard fan-out is not globally atomic.

Runtime metrics use bounded topology and shard IDs:

- `roze_database_shard_routes_total{topology,shard}`
- `roze_database_shard_health_checks_total{topology,shard,outcome}`

Readiness checks every primary and replica in the configured topology. Tenant
IDs, raw shard keys, database URLs, SQL, and credentials are never metric
labels.
