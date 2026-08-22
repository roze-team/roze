#![allow(dead_code)]

use crate::config::Config;
use sea_orm::DatabaseConnection;

#[derive(Clone, Debug)]
pub struct ServiceContext {
    pub config: Config,
    pub db: Option<DatabaseConnection>,
    pub db_connections: Option<roze_db::DatabaseConnections>,
    pub mongo: Option<roze_mongo::MongoDatabase>,
    pub cache: Option<roze_cache::RedisCache>,
}

impl ServiceContext {
    pub async fn new(config: Config) -> anyhow::Result<Self> {
        let db_connections =
            roze_db::connect_connections_optional(config.database.as_ref()).await?;
        let db = db_connections
            .as_ref()
            .map(|connections| connections.primary().clone());
        let mongo = roze_mongo::connect_optional(config.mongo.as_ref()).await?;
        let cache = match config.cache.as_ref() {
            Some(cache) => Some(
                roze_cache::RedisCache::connect(&roze_cache::CacheConfig {
                    url: cache.url.clone(),
                    cluster_urls: cache.cluster_urls.clone(),
                    namespace: cache.namespace.clone(),
                    default_ttl_secs: cache.default_ttl_secs,
                })
                .await?,
            ),
            None => None,
        };
        Ok(Self {
            config,
            db,
            db_connections,
            mongo,
            cache,
        })
    }

    pub fn read_db(&self) -> anyhow::Result<&DatabaseConnection> {
        self.db_connections
            .as_ref()
            .map(|connections| connections.read())
            .or(self.db.as_ref())
            .ok_or_else(|| anyhow::anyhow!("database connection is not configured"))
    }

    pub fn write_db(&self) -> anyhow::Result<&DatabaseConnection> {
        self.db_connections
            .as_ref()
            .map(|connections| connections.write())
            .or(self.db.as_ref())
            .ok_or_else(|| anyhow::anyhow!("database connection is not configured"))
    }

    pub fn jwt_config(&self) -> Option<roze_jwt::JwtConfig> {
        self.config.auth.as_ref().map(Into::into)
    }
}
