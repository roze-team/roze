use std::time::Duration;

use redis::AsyncCommands;
use serde::{de::DeserializeOwned, Serialize};

pub use redis;

#[derive(Clone)]
pub struct RedisClient {
    backend: RedisBackend,
}

#[derive(Clone)]
enum RedisBackend {
    Standalone(redis::Client),
    Cluster(redis::cluster::ClusterClient),
}

impl std::fmt::Debug for RedisClient {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RedisClient")
            .field(
                "topology",
                &match &self.backend {
                    RedisBackend::Standalone(_) => "standalone",
                    RedisBackend::Cluster(_) => "cluster",
                },
            )
            .finish_non_exhaustive()
    }
}

pub enum RedisConnection {
    Standalone(redis::aio::MultiplexedConnection),
    Cluster(redis::cluster_async::ClusterConnection),
}

impl redis::aio::ConnectionLike for RedisConnection {
    fn req_packed_command<'a>(
        &'a mut self,
        cmd: &'a redis::Cmd,
    ) -> redis::RedisFuture<'a, redis::Value> {
        match self {
            Self::Standalone(connection) => connection.req_packed_command(cmd),
            Self::Cluster(connection) => connection.req_packed_command(cmd),
        }
    }

    fn req_packed_commands<'a>(
        &'a mut self,
        cmd: &'a redis::Pipeline,
        offset: usize,
        count: usize,
    ) -> redis::RedisFuture<'a, Vec<redis::Value>> {
        match self {
            Self::Standalone(connection) => connection.req_packed_commands(cmd, offset, count),
            Self::Cluster(connection) => connection.req_packed_commands(cmd, offset, count),
        }
    }

    fn get_db(&self) -> i64 {
        match self {
            Self::Standalone(connection) => connection.get_db(),
            Self::Cluster(connection) => connection.get_db(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct RedisNamespace {
    namespace: String,
}

impl RedisClient {
    pub fn open(url: impl AsRef<str>) -> anyhow::Result<Self> {
        Ok(Self {
            backend: RedisBackend::Standalone(redis::Client::open(url.as_ref())?),
        })
    }

    pub fn open_cluster(
        initial_nodes: impl IntoIterator<Item = impl AsRef<str>>,
    ) -> anyhow::Result<Self> {
        let nodes = initial_nodes
            .into_iter()
            .map(|url| url.as_ref().to_string())
            .collect::<Vec<_>>();
        anyhow::ensure!(
            !nodes.is_empty(),
            "Redis Cluster requires at least one initial node"
        );
        Ok(Self {
            backend: RedisBackend::Cluster(redis::cluster::ClusterClient::new(nodes)?),
        })
    }

    pub fn open_topology(url: &str, cluster_nodes: &[String]) -> anyhow::Result<Self> {
        if cluster_nodes.is_empty() {
            Self::open(url)
        } else {
            Self::open_cluster(cluster_nodes)
        }
    }

    pub fn with_namespace(self, namespace: impl Into<String>) -> NamespacedRedisClient {
        NamespacedRedisClient {
            client: self,
            namespace: RedisNamespace {
                namespace: namespace.into(),
            },
        }
    }

    pub async fn connection(&self) -> anyhow::Result<RedisConnection> {
        Ok(match &self.backend {
            RedisBackend::Standalone(client) => {
                RedisConnection::Standalone(client.get_multiplexed_async_connection().await?)
            }
            RedisBackend::Cluster(client) => {
                RedisConnection::Cluster(client.get_async_connection().await?)
            }
        })
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

    #[test]
    fn cluster_topology_accepts_multiple_seed_nodes_without_connecting() {
        let client = RedisClient::open_cluster([
            "redis://127.0.0.1:7000",
            "redis://127.0.0.1:7001",
            "redis://127.0.0.1:7002",
        ])
        .expect("create cluster client");
        assert!(format!("{client:?}").contains("cluster"));
        assert!(RedisClient::open_cluster(Vec::<String>::new()).is_err());
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

    #[tokio::test]
    #[ignore = "requires ROZE_TEST_REDIS_CLUSTER_URLS with comma-separated cluster seed URLs"]
    async fn redis_cluster_round_trip_against_real_service() {
        let urls = std::env::var("ROZE_TEST_REDIS_CLUSTER_URLS")
            .expect("ROZE_TEST_REDIS_CLUSTER_URLS is required")
            .split(',')
            .map(str::trim)
            .filter(|url| !url.is_empty())
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>();
        let namespace = format!("roze-cluster-reference-{}", std::process::id());
        let client = RedisClient::open_cluster(urls)
            .expect("open Redis Cluster client")
            .with_namespace(namespace);
        let value = serde_json::json!({"topology": "cluster"});

        client
            .set_json("dependency", &value, Duration::from_secs(30))
            .await
            .expect("write Redis Cluster value");
        assert_eq!(
            client
                .get_json::<serde_json::Value>("dependency")
                .await
                .expect("read Redis Cluster value"),
            Some(value)
        );
        client
            .del("dependency")
            .await
            .expect("delete Redis Cluster value");
    }
}
