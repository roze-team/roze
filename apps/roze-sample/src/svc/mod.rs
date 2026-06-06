#![allow(dead_code)]

use crate::config::Config;
use sea_orm::DatabaseConnection;

#[derive(Clone, Debug)]
pub struct ServiceContext {
    pub config: Config,
    pub db: Option<DatabaseConnection>,
    pub cache: Option<roze_cache::RedisCache>,
}

impl ServiceContext {
    pub async fn new(config: Config) -> anyhow::Result<Self> {
        let db = roze_db::connect_optional(config.database.as_ref()).await?;
        let cache = match config.cache.as_ref() {
            Some(cache) => Some(
                roze_cache::RedisCache::connect(&roze_cache::CacheConfig {
                    url: cache.url.clone(),
                    namespace: cache.namespace.clone(),
                    default_ttl_secs: cache.default_ttl_secs,
                })
                .await?,
            ),
            None => None,
        };
        Ok(Self { config, db, cache })
    }

    pub fn jwt_config(&self) -> Option<roze_jwt::JwtConfig> {
        self.config.auth.as_ref().map(Into::into)
    }
}
