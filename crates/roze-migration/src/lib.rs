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

macro_rules! impl_runner {
    ($fn_name:ident, $ensure_fn:ident, $applied_fn:ident, $pool:ty, $insert_sql:expr) => {
        pub async fn $fn_name(pool: &$pool, migrations: &[SqlMigration]) -> anyhow::Result<()> {
            $ensure_fn(pool).await?;
            let applied = $applied_fn(pool).await?;

            for migration in migrations {
                if applied.contains(&migration.version) {
                    continue;
                }

                sqlx::query(AssertSqlSafe(migration.up_sql.as_str()))
                    .execute(pool)
                    .await?;
                sqlx::query($insert_sql)
                    .bind(migration.version)
                    .bind(&migration.name)
                    .execute(pool)
                    .await?;
            }

            Ok(())
        }
    };
}

impl_runner!(
    apply_sqlite_migrations,
    ensure_sqlite_ledger,
    applied_sqlite_versions,
    SqlitePool,
    "INSERT INTO roze_migrations (version, name) VALUES (?, ?)"
);
impl_runner!(
    apply_postgres_migrations,
    ensure_postgres_ledger,
    applied_postgres_versions,
    PgPool,
    "INSERT INTO roze_migrations (version, name) VALUES ($1, $2)"
);
impl_runner!(
    apply_mysql_migrations,
    ensure_mysql_ledger,
    applied_mysql_versions,
    MySqlPool,
    "INSERT INTO roze_migrations (version, name) VALUES (?, ?)"
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

async fn applied_sqlite_versions(pool: &SqlitePool) -> anyhow::Result<Vec<i64>> {
    let rows =
        sqlx::query_as::<_, (i64,)>("SELECT version FROM roze_migrations ORDER BY version ASC")
            .fetch_all(pool)
            .await?;
    Ok(rows.into_iter().map(|row| row.0).collect())
}

async fn applied_postgres_versions(pool: &PgPool) -> anyhow::Result<Vec<i64>> {
    let rows =
        sqlx::query_as::<_, (i64,)>("SELECT version FROM roze_migrations ORDER BY version ASC")
            .fetch_all(pool)
            .await?;
    Ok(rows.into_iter().map(|row| row.0).collect())
}

async fn applied_mysql_versions(pool: &MySqlPool) -> anyhow::Result<Vec<i64>> {
    let rows =
        sqlx::query_as::<_, (i64,)>("SELECT version FROM roze_migrations ORDER BY version ASC")
            .fetch_all(pool)
            .await?;
    Ok(rows.into_iter().map(|row| row.0).collect())
}

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
}
