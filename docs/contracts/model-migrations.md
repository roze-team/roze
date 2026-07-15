# Model migration lifecycle

`roze-migration` provides the model schema migration lifecycle for SQLite,
PostgreSQL and MySQL. A migration has a stable numeric version, name, forward
SQL and optional reverse SQL.

Use `plan_apply` to diff the checked-in migration set against ledger records
without changing the database. Planning rejects duplicate versions, unknown
applied versions, and version/name drift. `plan_rollback` emits reverse-ordered
steps down to an applied version boundary and rejects migrations without reverse
SQL before execution begins.

The backend lifecycle entry points are:

- `migrate_sqlite`, `migrate_postgres`, `migrate_mysql`
- `rollback_sqlite`, `rollback_postgres`, `rollback_mysql`
- `execute_sqlite_plan`, `execute_postgres_plan`, `execute_mysql_plan`
- `<backend>_migration_records` for status and dry-run planning

The compatibility entry points `apply_sqlite_migrations`,
`apply_postgres_migrations`, and `apply_mysql_migrations` use this same sorted,
drift-checked plan executor; they are not a separate best-effort execution path.

Plan execution applies every SQL statement and matching ledger update in one
database transaction. The returned `MigrationPlan` is serializable and may be
stored as CI/release evidence or presented as a dry-run artifact before calling
an `execute_*_plan` function.

Rollback target `0` removes all applied migrations. Any other target retains
that version and rolls back later versions. Production migrations should supply
reverse SQL unless an explicitly irreversible release policy is documented.
