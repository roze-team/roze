use std::time::Duration;

pub use roze_config::DatabaseConfig;
use sea_orm::{
    ConnectOptions, Database, DatabaseConnection, DbErr, TransactionError, TransactionTrait,
};

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configuration_round_trips() {
        let config = DatabaseConfig {
            url: "sqlite::memory:".to_string(),
            max_connections: 10,
            min_connections: 1,
            connect_timeout_secs: 3,
            idle_timeout_secs: 30,
            sqlx_logging: false,
        };

        assert_eq!(config.max_connections, 10);
        assert_eq!(config.min_connections, 1);
    }
}
