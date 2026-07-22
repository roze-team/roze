use std::time::Duration;

use redis::AsyncCommands;
use serde::{de::DeserializeOwned, Serialize};

#[derive(Debug, Clone)]
pub struct RedisClient {
    client: redis::Client,
}

#[derive(Debug, Clone)]
pub struct RedisNamespace {
    namespace: String,
}

impl RedisClient {
    pub fn open(url: impl AsRef<str>) -> anyhow::Result<Self> {
        Ok(Self {
            client: redis::Client::open(url.as_ref())?,
        })
    }

    pub fn with_namespace(self, namespace: impl Into<String>) -> NamespacedRedisClient {
        NamespacedRedisClient {
            client: self,
            namespace: RedisNamespace {
                namespace: namespace.into(),
            },
        }
    }

    pub async fn connection(&self) -> anyhow::Result<redis::aio::MultiplexedConnection> {
        Ok(self.client.get_multiplexed_async_connection().await?)
    }

    pub async fn health_check(&self) -> anyhow::Result<()> {
        let mut connection = self.connection().await?;
        let response: String = redis::cmd("PING").query_async(&mut connection).await?;
        anyhow::ensure!(response == "PONG", "unexpected Redis PING response");
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct NamespacedRedisClient {
    client: RedisClient,
    namespace: RedisNamespace,
}

impl NamespacedRedisClient {
    pub fn key(&self, key: &str) -> String {
        format!("{}:{}", self.namespace.namespace, key)
    }

    pub async fn get_json<T>(&self, key: &str) -> anyhow::Result<Option<T>>
    where
        T: DeserializeOwned,
    {
        let mut conn = self.client.connection().await?;
        let value: Option<String> = conn.get(self.key(key)).await?;
        Ok(match value {
            Some(value) => Some(serde_json::from_str(&value)?),
            None => None,
        })
    }

    pub async fn set_json<T>(&self, key: &str, value: &T, ttl: Duration) -> anyhow::Result<()>
    where
        T: Serialize,
    {
        let mut conn = self.client.connection().await?;
        let payload = serde_json::to_string(value)?;
        let _: () = conn.set_ex(self.key(key), payload, ttl.as_secs()).await?;
        Ok(())
    }

    pub async fn del(&self, key: &str) -> anyhow::Result<()> {
        let mut conn = self.client.connection().await?;
        let _: () = conn.del(self.key(key)).await?;
        Ok(())
    }

    pub async fn health_check(&self) -> anyhow::Result<()> {
        self.client.health_check().await
    }
}

pub fn namespace_key(namespace: &str, key: &str) -> String {
    format!("{}:{}", namespace, key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn namespaces_keys() {
        assert_eq!(namespace_key("roze", "user:1"), "roze:user:1");
    }

    #[tokio::test]
    #[ignore = "requires ROZE_TEST_REDIS_URL, for example redis://127.0.0.1:6379"]
    async fn redis_round_trip_against_real_service() {
        let url = std::env::var("ROZE_TEST_REDIS_URL").expect("ROZE_TEST_REDIS_URL is required");
        let namespace = format!("roze-reference-{}", std::process::id());
        let client = RedisClient::open(url)
            .expect("open Redis client")
            .with_namespace(namespace);
        let value = serde_json::json!({"status": "ready", "attempt": 1});

        client
            .set_json("dependency", &value, Duration::from_secs(30))
            .await
            .expect("write Redis value");
        assert_eq!(
            client
                .get_json::<serde_json::Value>("dependency")
                .await
                .expect("read Redis value"),
            Some(value)
        );
        client.del("dependency").await.expect("delete Redis value");
        assert_eq!(
            client
                .get_json::<serde_json::Value>("dependency")
                .await
                .expect("read deleted Redis value"),
            None
        );
    }
}
