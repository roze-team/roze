use serde::{Deserialize, Serialize};
use sqlx::{
    mysql::{MySqlPool, MySqlPoolOptions},
    postgres::{PgPool, PgPoolOptions},
    sqlite::{SqlitePool, SqlitePoolOptions},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SqlxDatabaseKind {
    Sqlite,
    Postgres,
    MySql,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SqlxConfig {
    pub kind: SqlxDatabaseKind,
    pub url: String,
    #[serde(default = "default_max_connections")]
    pub max_connections: u32,
}

#[derive(Debug, Clone)]
pub enum SqlxPool {
    Sqlite(SqlitePool),
    Postgres(PgPool),
    MySql(MySqlPool),
}

impl SqlxPool {
    pub fn kind(&self) -> SqlxDatabaseKind {
        match self {
            SqlxPool::Sqlite(_) => SqlxDatabaseKind::Sqlite,
            SqlxPool::Postgres(_) => SqlxDatabaseKind::Postgres,
            SqlxPool::MySql(_) => SqlxDatabaseKind::MySql,
        }
    }
}

pub async fn connect(config: &SqlxConfig) -> anyhow::Result<SqlxPool> {
    match config.kind {
        SqlxDatabaseKind::Sqlite => Ok(SqlxPool::Sqlite(
            SqlitePoolOptions::new()
                .max_connections(config.max_connections)
                .connect(&config.url)
                .await?,
        )),
        SqlxDatabaseKind::Postgres => Ok(SqlxPool::Postgres(
            PgPoolOptions::new()
                .max_connections(config.max_connections)
                .connect(&config.url)
                .await?,
        )),
        SqlxDatabaseKind::MySql => Ok(SqlxPool::MySql(
            MySqlPoolOptions::new()
                .max_connections(config.max_connections)
                .connect(&config.url)
                .await?,
        )),
    }
}

pub async fn connect_sqlite(url: impl AsRef<str>, max_connections: u32) -> anyhow::Result<SqlitePool> {
    Ok(SqlitePoolOptions::new()
        .max_connections(max_connections)
        .connect(url.as_ref())
        .await?)
}

pub async fn connect_postgres(url: impl AsRef<str>, max_connections: u32) -> anyhow::Result<PgPool> {
    Ok(PgPoolOptions::new()
        .max_connections(max_connections)
        .connect(url.as_ref())
        .await?)
}

pub async fn connect_mysql(url: impl AsRef<str>, max_connections: u32) -> anyhow::Result<MySqlPool> {
    Ok(MySqlPoolOptions::new()
        .max_connections(max_connections)
        .connect(url.as_ref())
        .await?)
}

fn default_max_connections() -> u32 {
    10
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn config_defaults_make_sense() {
        let cfg = SqlxConfig {
            kind: SqlxDatabaseKind::Sqlite,
            url: "sqlite::memory:".into(),
            max_connections: default_max_connections(),
        };
        assert_eq!(cfg.max_connections, 10);
        let pool = sqlx::SqlitePool::connect_lazy("sqlite::memory:")
            .expect("pool");
        assert!(matches!(
            SqlxPool::Sqlite(pool).kind(),
            SqlxDatabaseKind::Sqlite
        ));
    }
}
