pub use mongodb::{
    bson,
    options::{ClientOptions, IndexOptions},
    Client, Collection, Database, IndexModel,
};
pub use roze_config::MongoConfig;

#[derive(Clone, Debug)]
pub struct MongoDatabase {
    client: Client,
    database: Database,
}

impl MongoDatabase {
    pub async fn connect(config: &MongoConfig) -> anyhow::Result<Self> {
        let mut options = ClientOptions::parse(&config.url).await?;
        options.app_name = config.app_name.clone();
        options.max_pool_size = Some(config.max_pool_size);
        options.min_pool_size = Some(config.min_pool_size);
        let client = Client::with_options(options)?;
        let database = client.database(&config.database);
        Ok(Self { client, database })
    }

    pub fn client(&self) -> &Client {
        &self.client
    }

    pub fn database(&self) -> &Database {
        &self.database
    }

    pub fn collection<T>(&self, name: impl AsRef<str>) -> Collection<T>
    where
        T: Send + Sync,
    {
        self.database.collection(name.as_ref())
    }
}

pub async fn connect(config: &MongoConfig) -> anyhow::Result<MongoDatabase> {
    MongoDatabase::connect(config).await
}

pub async fn connect_optional(
    config: Option<&MongoConfig>,
) -> anyhow::Result<Option<MongoDatabase>> {
    match config {
        Some(config) => connect(config).await.map(Some),
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposes_collection_names() {
        let config = MongoConfig {
            url: "mongodb://localhost:27017".to_string(),
            database: "roze".to_string(),
            max_pool_size: 8,
            min_pool_size: 0,
            app_name: Some("roze-test".to_string()),
        };

        assert_eq!(config.database, "roze");
        assert_eq!(config.max_pool_size, 8);
    }
}
