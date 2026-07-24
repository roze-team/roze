# Persistent SQL Outbox

`roze-transaction-sql` is the official PostgreSQL/MySQL implementation of
`roze_transaction::OutboxStore` and
`TransactionalOutbox<sea_orm::DatabaseTransaction>`.

It provides:

- idempotent enqueue by event ID;
- transaction-local enqueue alongside business writes;
- concurrent claim using `FOR UPDATE SKIP LOCKED`;
- publishing leases and expired-lease recovery;
- exponential retry scheduling through `relay_outbox_batch`;
- bounded attempts, dead-letter queries, replay, and published-row cleanup;
- PostgreSQL and MySQL migrations;
- bounded `roze_outbox_events_total{driver,outcome}` metrics.

Generated service configuration:

```yaml
profile: production
database:
  url: env://DATABASE_URL
outbox:
  enabled: true
  store: auto
  table: roze_outbox
  max_attempts: 16
  migrate: true
  batch_size: 100
  interval_ms: 1000
```

`auto` selects SQL when a database is configured. `sql` requires a database.
An enabled memory store is rejected in the production profile and produces a
warning in development. Generated `ServiceContext::sql_outbox()` returns the
concrete store when application code must call `enqueue_in_transaction`.

The bundled migrations are available as `POSTGRES_MIGRATION` and
`MYSQL_MIGRATION`. `SqlOutboxStore::migrate` applies the migration to the
configured table. Production deployments may instead execute the checked-in
SQL through their normal migration approval process and set `migrate: false`.

Ignored real-database tests use `ROZE_TEST_POSTGRES_URL` and
`ROZE_TEST_MYSQL_URL`. They verify concurrent claim exclusion, lease recovery,
retry scheduling, persistence across store reconstruction, transactional
enqueue, and that a failed consumer transaction does not mark an event
published.
