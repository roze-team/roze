use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

pub use roze_config::{DatabaseConfig, DatabaseReadPolicy};
pub use sea_orm::DatabaseConnection;
use sea_orm::{ConnectOptions, Database, DbErr, TransactionError, TransactionTrait};

pub async fn connect(config: &DatabaseConfig) -> Result<DatabaseConnection, DbErr> {
    connect_url(config, &config.url).await
}

async fn connect_url(config: &DatabaseConfig, url: &str) -> Result<DatabaseConnection, DbErr> {
    let mut options = ConnectOptions::new(url.to_string());
    options
        .max_connections(config.max_connections)
        .min_connections(config.min_connections)
        .connect_timeout(Duration::from_secs(config.connect_timeout_secs))
        .idle_timeout(Duration::from_secs(config.idle_timeout_secs))
        .sqlx_logging(config.sqlx_logging);

    Database::connect(options).await
}

#[derive(Clone, Debug)]
pub struct DatabaseConnections {
    primary: DatabaseConnection,
    replicas: Vec<DatabaseConnection>,
    policy: DatabaseReadPolicy,
    read_cursor: Arc<AtomicUsize>,
}

impl DatabaseConnections {
    pub fn primary(&self) -> &DatabaseConnection {
        &self.primary
    }

    pub fn write(&self) -> &DatabaseConnection {
        &self.primary
    }

    pub fn read(&self) -> &DatabaseConnection {
        if self.replicas.is_empty() {
            return &self.primary;
        }

        let index = match self.policy {
            DatabaseReadPolicy::RoundRobin => {
                self.read_cursor.fetch_add(1, Ordering::Relaxed) % self.replicas.len()
            }
            DatabaseReadPolicy::Random => random_index(self.replicas.len()),
        };

        &self.replicas[index]
    }
}

pub async fn connect_connections(config: &DatabaseConfig) -> Result<DatabaseConnections, DbErr> {
    let primary = connect(config).await?;
    let mut replicas = Vec::with_capacity(config.replicas.len());
    for replica in &config.replicas {
        replicas.push(connect_url(config, replica).await?);
    }

    Ok(DatabaseConnections {
        primary,
        replicas,
        policy: config.policy,
        read_cursor: Arc::new(AtomicUsize::new(0)),
    })
}

pub async fn connect_connections_optional(
    config: Option<&DatabaseConfig>,
) -> Result<Option<DatabaseConnections>, DbErr> {
    match config {
        Some(config) => connect_connections(config).await.map(Some),
        None => Ok(None),
    }
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

fn random_index(len: usize) -> usize {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as usize % len)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configuration_round_trips() {
        let config = DatabaseConfig {
            url: "sqlite::memory:".to_string(),
            replicas: Vec::new(),
            policy: DatabaseReadPolicy::RoundRobin,
            max_connections: 10,
            min_connections: 1,
            connect_timeout_secs: 3,
            idle_timeout_secs: 30,
            sqlx_logging: false,
        };

        assert_eq!(config.max_connections, 10);
        assert_eq!(config.min_connections, 1);
    }

    #[tokio::test]
    async fn read_uses_primary_when_replicas_are_empty() {
        let config = DatabaseConfig {
            url: "sqlite::memory:".to_string(),
            replicas: Vec::new(),
            policy: DatabaseReadPolicy::RoundRobin,
            max_connections: 10,
            min_connections: 1,
            connect_timeout_secs: 3,
            idle_timeout_secs: 30,
            sqlx_logging: false,
        };

        let connections = connect_connections(&config).await.expect("connect");
        assert!(std::ptr::eq(connections.primary(), connections.read()));
        assert!(std::ptr::eq(connections.primary(), connections.write()));
    }
}
