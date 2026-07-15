use serde::{Deserialize, Serialize};
use sqlx::{mysql::MySqlPool, postgres::PgPool, sqlite::SqlitePool, AssertSqlSafe};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SqlMigration {
    pub version: i64,
    pub name: String,
    pub up_sql: String,
    #[serde(default)]
    pub down_sql: Option<String>,
}

impl SqlMigration {
    pub fn new(
        version: i64,
        name: impl Into<String>,
        up_sql: impl Into<String>,
        down_sql: Option<impl Into<String>>,
    ) -> Self {
        Self {
            version,
            name: name.into(),
            up_sql: up_sql.into(),
            down_sql: down_sql.map(Into::into),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MigrationRecord {
    pub version: i64,
    pub name: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MigrationDirection {
    Up,
    Down,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MigrationStep {
    pub version: i64,
    pub name: String,
    pub direction: MigrationDirection,
    pub sql: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct MigrationPlan {
    pub steps: Vec<MigrationStep>,
}

impl MigrationPlan {
    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum MigrationPlanError {
    #[error("duplicate migration version {0}")]
    DuplicateVersion(i64),
    #[error("database contains unknown migration version {0}")]
    UnknownAppliedVersion(i64),
    #[error("migration {version} name drift: database has `{applied}`, source has `{expected}`")]
    NameDrift {
        version: i64,
        applied: String,
        expected: String,
    },
    #[error("migration {version} (`{name}`) has no down SQL")]
    MissingDownSql { version: i64, name: String },
    #[error("rollback target {target} is not an applied migration boundary")]
    InvalidRollbackTarget { target: i64 },
}

/// Produces a deterministic, validated forward plan without touching a database.
pub fn plan_apply(
    applied: &[MigrationRecord],
    migrations: &[SqlMigration],
) -> Result<MigrationPlan, MigrationPlanError> {
    let ordered = validated_migrations(migrations)?;
    validate_applied(applied, &ordered)?;
    let applied_versions = applied.iter().map(|item| item.version).collect::<Vec<_>>();
    Ok(MigrationPlan {
        steps: ordered
            .into_iter()
            .filter(|migration| !applied_versions.contains(&migration.version))
            .map(|migration| MigrationStep {
                version: migration.version,
                name: migration.name.clone(),
                direction: MigrationDirection::Up,
                sql: migration.up_sql.clone(),
            })
            .collect(),
    })
}

/// Produces a reverse-ordered rollback plan down to `target` (inclusive boundary).
/// A target of zero rolls back every migration.
pub fn plan_rollback(
    applied: &[MigrationRecord],
    migrations: &[SqlMigration],
    target: i64,
) -> Result<MigrationPlan, MigrationPlanError> {
    let ordered = validated_migrations(migrations)?;
    validate_applied(applied, &ordered)?;
    if target != 0 && !applied.iter().any(|item| item.version == target) {
        return Err(MigrationPlanError::InvalidRollbackTarget { target });
    }
    let mut steps = Vec::new();
    for record in applied.iter().rev().filter(|item| item.version > target) {
        let migration = ordered
            .iter()
            .find(|migration| migration.version == record.version)
            .expect("validated applied migration");
        let sql = migration
            .down_sql
            .clone()
            .ok_or_else(|| MigrationPlanError::MissingDownSql {
                version: migration.version,
                name: migration.name.clone(),
            })?;
        steps.push(MigrationStep {
            version: migration.version,
            name: migration.name.clone(),
            direction: MigrationDirection::Down,
            sql,
        });
    }
    Ok(MigrationPlan { steps })
}

fn validated_migrations(
    migrations: &[SqlMigration],
) -> Result<Vec<&SqlMigration>, MigrationPlanError> {
    let mut ordered = migrations.iter().collect::<Vec<_>>();
    ordered.sort_by_key(|migration| migration.version);
    for pair in ordered.windows(2) {
        if pair[0].version == pair[1].version {
            return Err(MigrationPlanError::DuplicateVersion(pair[0].version));
        }
    }
    Ok(ordered)
}

fn validate_applied(
    applied: &[MigrationRecord],
    migrations: &[&SqlMigration],
) -> Result<(), MigrationPlanError> {
    for record in applied {
        let migration = migrations
            .iter()
            .find(|migration| migration.version == record.version)
            .ok_or(MigrationPlanError::UnknownAppliedVersion(record.version))?;
        if migration.name != record.name {
            return Err(MigrationPlanError::NameDrift {
                version: record.version,
                applied: record.name.clone(),
                expected: migration.name.clone(),
            });
        }
    }
    Ok(())
}

macro_rules! impl_runner {
    ($fn_name:ident, $records_fn:ident, $execute_fn:ident, $pool:ty $(,)?) => {
        pub async fn $fn_name(pool: &$pool, migrations: &[SqlMigration]) -> anyhow::Result<()> {
            let applied = $records_fn(pool).await?;
            let plan = plan_apply(&applied, migrations)?;
            $execute_fn(pool, &plan).await?;
            Ok(())
        }
    };
}

macro_rules! impl_plan_executor {
    ($fn_name:ident, $ensure_fn:ident, $pool:ty, $insert_sql:expr, $delete_sql:expr) => {
        pub async fn $fn_name(pool: &$pool, plan: &MigrationPlan) -> anyhow::Result<()> {
            $ensure_fn(pool).await?;
            let mut transaction = pool.begin().await?;
            for step in &plan.steps {
                sqlx::query(AssertSqlSafe(step.sql.as_str()))
                    .execute(&mut *transaction)
                    .await?;
                match step.direction {
                    MigrationDirection::Up => {
                        sqlx::query($insert_sql)
                            .bind(step.version)
                            .bind(&step.name)
                            .execute(&mut *transaction)
                            .await?;
                    }
                    MigrationDirection::Down => {
                        sqlx::query($delete_sql)
                            .bind(step.version)
                            .execute(&mut *transaction)
                            .await?;
                    }
                }
            }
            transaction.commit().await?;
            Ok(())
        }
    };
}

impl_runner!(
    apply_sqlite_migrations,
    sqlite_migration_records,
    execute_sqlite_plan,
    SqlitePool,
);

impl_plan_executor!(
    execute_sqlite_plan,
    ensure_sqlite_ledger,
    SqlitePool,
    "INSERT INTO roze_migrations (version, name) VALUES (?, ?)",
    "DELETE FROM roze_migrations WHERE version = ?"
);
impl_plan_executor!(
    execute_postgres_plan,
    ensure_postgres_ledger,
    PgPool,
    "INSERT INTO roze_migrations (version, name) VALUES ($1, $2)",
    "DELETE FROM roze_migrations WHERE version = $1"
);
impl_plan_executor!(
    execute_mysql_plan,
    ensure_mysql_ledger,
    MySqlPool,
    "INSERT INTO roze_migrations (version, name) VALUES (?, ?)",
    "DELETE FROM roze_migrations WHERE version = ?"
);
impl_runner!(
    apply_postgres_migrations,
    postgres_migration_records,
    execute_postgres_plan,
    PgPool,
);
impl_runner!(
    apply_mysql_migrations,
    mysql_migration_records,
    execute_mysql_plan,
    MySqlPool,
);

async fn ensure_sqlite_ledger(pool: &SqlitePool) -> anyhow::Result<()> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS roze_migrations (version BIGINT PRIMARY KEY, name TEXT NOT NULL, applied_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP)",
    )
    .execute(pool)
    .await?;
    Ok(())
}

async fn ensure_postgres_ledger(pool: &PgPool) -> anyhow::Result<()> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS roze_migrations (version BIGINT PRIMARY KEY, name TEXT NOT NULL, applied_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP)",
    )
    .execute(pool)
    .await?;
    Ok(())
}

async fn ensure_mysql_ledger(pool: &MySqlPool) -> anyhow::Result<()> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS roze_migrations (version BIGINT PRIMARY KEY, name TEXT NOT NULL, applied_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP)",
    )
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn sqlite_migration_records(pool: &SqlitePool) -> anyhow::Result<Vec<MigrationRecord>> {
    ensure_sqlite_ledger(pool).await?;
    let rows = sqlx::query_as::<_, (i64, String)>(
        "SELECT version, name FROM roze_migrations ORDER BY version ASC",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(version, name)| MigrationRecord { version, name })
        .collect())
}

pub async fn postgres_migration_records(pool: &PgPool) -> anyhow::Result<Vec<MigrationRecord>> {
    ensure_postgres_ledger(pool).await?;
    let rows = sqlx::query_as::<_, (i64, String)>(
        "SELECT version, name FROM roze_migrations ORDER BY version ASC",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(version, name)| MigrationRecord { version, name })
        .collect())
}

pub async fn mysql_migration_records(pool: &MySqlPool) -> anyhow::Result<Vec<MigrationRecord>> {
    ensure_mysql_ledger(pool).await?;
    let rows = sqlx::query_as::<_, (i64, String)>(
        "SELECT version, name FROM roze_migrations ORDER BY version ASC",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(version, name)| MigrationRecord { version, name })
        .collect())
}

macro_rules! impl_lifecycle {
    ($migrate_fn:ident, $rollback_fn:ident, $records_fn:ident, $execute_fn:ident, $pool:ty) => {
        pub async fn $migrate_fn(
            pool: &$pool,
            migrations: &[SqlMigration],
        ) -> anyhow::Result<MigrationPlan> {
            let applied = $records_fn(pool).await?;
            let plan = plan_apply(&applied, migrations)?;
            $execute_fn(pool, &plan).await?;
            Ok(plan)
        }

        pub async fn $rollback_fn(
            pool: &$pool,
            migrations: &[SqlMigration],
            target: i64,
        ) -> anyhow::Result<MigrationPlan> {
            let applied = $records_fn(pool).await?;
            let plan = plan_rollback(&applied, migrations, target)?;
            $execute_fn(pool, &plan).await?;
            Ok(plan)
        }
    };
}

impl_lifecycle!(
    migrate_sqlite,
    rollback_sqlite,
    sqlite_migration_records,
    execute_sqlite_plan,
    SqlitePool
);
impl_lifecycle!(
    migrate_postgres,
    rollback_postgres,
    postgres_migration_records,
    execute_postgres_plan,
    PgPool
);
impl_lifecycle!(
    migrate_mysql,
    rollback_mysql,
    mysql_migration_records,
    execute_mysql_plan,
    MySqlPool
);

pub fn sort_migrations(migrations: &mut [SqlMigration]) {
    migrations.sort_by_key(|migration| migration.version);
}

pub fn diff_pending(applied: &[i64], migrations: &[SqlMigration]) -> Vec<SqlMigration> {
    migrations
        .iter()
        .filter(|migration| !applied.contains(&migration.version))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sorts_and_filters_migrations() {
        let mut migrations = vec![
            SqlMigration::new(2, "b", "select 2", None::<String>),
            SqlMigration::new(1, "a", "select 1", None::<String>),
        ];
        sort_migrations(&mut migrations);
        assert_eq!(migrations[0].version, 1);
        let pending = diff_pending(&[1], &migrations);
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].version, 2);
    }

    #[test]
    fn plans_apply_and_reverse_rollback_with_drift_checks() {
        let migrations = vec![
            SqlMigration::new(
                2,
                "add_email",
                "ALTER TABLE users ADD email TEXT",
                Some("ALTER TABLE users DROP COLUMN email"),
            ),
            SqlMigration::new(
                1,
                "create_users",
                "CREATE TABLE users (id BIGINT PRIMARY KEY)",
                Some("DROP TABLE users"),
            ),
        ];
        let apply = plan_apply(&[], &migrations).unwrap();
        assert_eq!(
            apply
                .steps
                .iter()
                .map(|step| step.version)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
        let applied = vec![
            MigrationRecord {
                version: 1,
                name: "create_users".into(),
            },
            MigrationRecord {
                version: 2,
                name: "add_email".into(),
            },
        ];
        let rollback = plan_rollback(&applied, &migrations, 0).unwrap();
        assert_eq!(
            rollback
                .steps
                .iter()
                .map(|step| step.version)
                .collect::<Vec<_>>(),
            vec![2, 1]
        );
        assert!(rollback
            .steps
            .iter()
            .all(|step| step.direction == MigrationDirection::Down));

        let drifted = vec![MigrationRecord {
            version: 1,
            name: "renamed".into(),
        }];
        assert!(matches!(
            plan_apply(&drifted, &migrations),
            Err(MigrationPlanError::NameDrift { version: 1, .. })
        ));
    }

    #[tokio::test]
    async fn sqlite_plan_apply_and_rollback_are_ledger_consistent() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        let migrations = vec![SqlMigration::new(
            1,
            "create_widgets",
            "CREATE TABLE widgets (id INTEGER PRIMARY KEY)",
            Some("DROP TABLE widgets"),
        )];
        let apply =
            plan_apply(&sqlite_migration_records(&pool).await.unwrap(), &migrations).unwrap();
        execute_sqlite_plan(&pool, &apply).await.unwrap();
        assert_eq!(sqlite_migration_records(&pool).await.unwrap().len(), 1);
        let rollback = plan_rollback(
            &sqlite_migration_records(&pool).await.unwrap(),
            &migrations,
            0,
        )
        .unwrap();
        execute_sqlite_plan(&pool, &rollback).await.unwrap();
        assert!(sqlite_migration_records(&pool).await.unwrap().is_empty());
        let exists: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'widgets'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(exists.0, 0);
    }

    #[tokio::test]
    async fn legacy_apply_entrypoint_uses_sorted_drift_checked_atomic_plan() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        let migrations = vec![
            SqlMigration::new(
                2,
                "add_name",
                "ALTER TABLE widgets ADD COLUMN name TEXT",
                Some("ALTER TABLE widgets DROP COLUMN name"),
            ),
            SqlMigration::new(
                1,
                "create_widgets",
                "CREATE TABLE widgets (id INTEGER PRIMARY KEY)",
                Some("DROP TABLE widgets"),
            ),
        ];
        apply_sqlite_migrations(&pool, &migrations).await.unwrap();
        assert_eq!(
            sqlite_migration_records(&pool)
                .await
                .unwrap()
                .into_iter()
                .map(|record| record.version)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );

        let renamed = vec![SqlMigration::new(
            1,
            "renamed_widgets",
            "SELECT 1",
            Some("SELECT 1"),
        )];
        assert!(apply_sqlite_migrations(&pool, &renamed)
            .await
            .unwrap_err()
            .to_string()
            .contains("name drift"));

        let failing = vec![
            migrations[1].clone(),
            migrations[0].clone(),
            SqlMigration::new(
                3,
                "create_transient",
                "CREATE TABLE transient (id INTEGER PRIMARY KEY)",
                Some("DROP TABLE transient"),
            ),
            SqlMigration::new(4, "invalid_statement", "THIS IS NOT SQL", Some("SELECT 1")),
        ];
        assert!(apply_sqlite_migrations(&pool, &failing).await.is_err());
        assert!(!sqlite_migration_records(&pool)
            .await
            .unwrap()
            .iter()
            .any(|record| record.version >= 3));
        let transient_exists: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'transient'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(transient_exists.0, 0);
    }

    #[tokio::test]
    async fn postgres_live_apply_and_rollback_evidence() {
        let Ok(url) = std::env::var("ROZECTL_TEST_POSTGRES_URL") else {
            eprintln!("skipping PostgreSQL migration evidence: ROZECTL_TEST_POSTGRES_URL not set");
            return;
        };
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect(&url)
            .await
            .unwrap();
        ensure_postgres_ledger(&pool).await.unwrap();
        sqlx::query("DROP TABLE IF EXISTS roze_parity_widgets")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM roze_migrations WHERE version = 900001")
            .execute(&pool)
            .await
            .unwrap();
        let migrations = [SqlMigration::new(
            900001,
            "parity_widgets",
            "CREATE TABLE roze_parity_widgets (id BIGINT PRIMARY KEY, score BIGINT NULL)",
            Some("DROP TABLE roze_parity_widgets"),
        )];
        let apply = migrate_postgres(&pool, &migrations).await.unwrap();
        assert_eq!(apply.steps.len(), 1);
        assert_eq!(
            postgres_migration_records(&pool)
                .await
                .unwrap()
                .last()
                .unwrap()
                .version,
            900001
        );
        let rollback = rollback_postgres(&pool, &migrations, 0).await.unwrap();
        assert_eq!(rollback.steps.len(), 1);
        assert!(!postgres_migration_records(&pool)
            .await
            .unwrap()
            .iter()
            .any(|record| record.version == 900001));
    }

    #[tokio::test]
    async fn mysql_live_apply_and_rollback_evidence() {
        let Ok(url) = std::env::var("ROZECTL_TEST_MYSQL_URL") else {
            eprintln!("skipping MySQL migration evidence: ROZECTL_TEST_MYSQL_URL not set");
            return;
        };
        let pool = sqlx::mysql::MySqlPoolOptions::new()
            .max_connections(1)
            .connect(&url)
            .await
            .unwrap();
        ensure_mysql_ledger(&pool).await.unwrap();
        sqlx::query("DROP TABLE IF EXISTS roze_parity_widgets")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM roze_migrations WHERE version = 900001")
            .execute(&pool)
            .await
            .unwrap();
        let migrations = [SqlMigration::new(
            900001,
            "parity_widgets",
            "CREATE TABLE roze_parity_widgets (id BIGINT PRIMARY KEY, score BIGINT NULL)",
            Some("DROP TABLE roze_parity_widgets"),
        )];
        let apply = migrate_mysql(&pool, &migrations).await.unwrap();
        assert_eq!(apply.steps.len(), 1);
        assert!(mysql_migration_records(&pool)
            .await
            .unwrap()
            .iter()
            .any(|record| record.version == 900001));
        let rollback = rollback_mysql(&pool, &migrations, 0).await.unwrap();
        assert_eq!(rollback.steps.len(), 1);
        assert!(!mysql_migration_records(&pool)
            .await
            .unwrap()
            .iter()
            .any(|record| record.version == 900001));
    }
}
