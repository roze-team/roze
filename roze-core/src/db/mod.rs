use std::time::Duration;

use sea_orm::{
    ConnectOptions, Database, DatabaseConnection, DbErr, TransactionError, TransactionTrait,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseConfig {
    pub url: String,
    #[serde(default = "default_max_connections")]
    pub max_connections: u32,
    #[serde(default = "default_min_connections")]
    pub min_connections: u32,
    #[serde(default = "default_connect_timeout_secs")]
    pub connect_timeout_secs: u64,
    #[serde(default = "default_idle_timeout_secs")]
    pub idle_timeout_secs: u64,
    #[serde(default = "default_sqlx_logging")]
    pub sqlx_logging: bool,
}

pub async fn connect(config: &DatabaseConfig) -> Result<DatabaseConnection, DbErr> {
    let mut options = ConnectOptions::new(config.url.clone());
    options
        .max_connections(config.max_connections)
        .min_connections(config.min_connections)
        .connect_timeout(Duration::from_secs(config.connect_timeout_secs))
        .idle_timeout(Duration::from_secs(config.idle_timeout_secs))
        .sqlx_logging(config.sqlx_logging);

    Database::connect(options).await
}

pub async fn connect_optional(
    config: Option<&DatabaseConfig>,
) -> Result<Option<DatabaseConnection>, DbErr> {
    match config {
        Some(config) => connect(config).await.map(Some),
        None => Ok(None),
    }
}

pub async fn transaction<F, Fut, T>(db: &DatabaseConnection, func: F) -> Result<T, DbErr>
where
    F: for<'c> FnOnce(&'c sea_orm::DatabaseTransaction) -> Fut + Send,
    Fut: std::future::Future<Output = Result<T, DbErr>> + Send + 'static,
    T: Send,
{
    db.transaction(move |txn| Box::pin(func(txn)))
        .await
        .map_err(|err: TransactionError<DbErr>| match err {
            TransactionError::Connection(err) | TransactionError::Transaction(err) => err,
        })
}

fn default_max_connections() -> u32 {
    100
}

fn default_min_connections() -> u32 {
    5
}

fn default_connect_timeout_secs() -> u64 {
    8
}

fn default_idle_timeout_secs() -> u64 {
    600
}

fn default_sqlx_logging() -> bool {
    true
}
