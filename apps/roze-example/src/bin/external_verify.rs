use std::{collections::HashMap, time::Duration};

use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};
use serde_json::json;

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

fn database_config(url: String) -> roze_db::DatabaseConfig {
    roze_db::DatabaseConfig {
        url,
        replicas: Vec::new(),
        policy: roze_db::DatabaseReadPolicy::RoundRobin,
        max_connections: 5,
        min_connections: 1,
        connect_timeout_secs: 10,
        idle_timeout_secs: 30,
        sqlx_logging: false,
    }
}

async fn verify_sql(label: &str, backend: DatabaseBackend, url: String) -> anyhow::Result<()> {
    let db = roze_db::connect(&database_config(url)).await?;
    db.query_one(Statement::from_string(backend, "SELECT 1".to_string()))
        .await?;
    println!("{label} PASS");
    Ok(())
}

async fn verify_redis(url: String) -> anyhow::Result<()> {
    let cache = roze_cache::RedisCache::connect(&roze_cache::CacheConfig {
        url,
        namespace: "roze_external_verify".to_string(),
        default_ttl_secs: 60,
    })
    .await?;
    cache
        .set_json(
            "redis",
            &json!({"status": "ok"}),
            Some(Duration::from_secs(60)),
        )
        .await?;
    let value: Option<serde_json::Value> = cache.get_json("redis").await?;
    anyhow::ensure!(
        value == Some(json!({"status": "ok"})),
        "redis value mismatch"
    );
    cache.del("redis").await?;
    println!("redis PASS");
    Ok(())
}

async fn verify_kafka(broker: String) -> anyhow::Result<()> {
    let producer = roze_kafka::RdkafkaProducer::new(roze_kafka::KafkaConfig {
        brokers: vec![broker],
        client_id: Some("roze-example-external-verify".to_string()),
        topic_prefix: "roze.verify".to_string(),
        acks: "all".to_string(),
        auto_offset_reset: "earliest".to_string(),
        enable_auto_commit: false,
        enable_manual_ack: false,
        linger_ms: 10,
        batch_size: 16_384,
        session_timeout_ms: 10_000,
        heartbeat_interval_ms: 3_000,
        max_poll_interval_ms: 300_000,
        flush_timeout_ms: 10_000,
        retry_backoff_ms: 1_000,
        max_retries: 3,
        consumer_workers: 1,
        ..Default::default()
    })?;
    let result = roze_kafka::publish_json_with_result(
        &producer,
        "events",
        json!({"source": "roze-example", "status": "ok"}),
    )
    .await?;
    anyhow::ensure!(result.topic == "roze.verify.events", "kafka topic mismatch");
    producer.flush()?;
    println!("kafka PASS");
    Ok(())
}

async fn verify_nats(server: String) -> anyhow::Result<()> {
    let broker = roze_nats::NatsJetStream::connect(roze_nats::NatsConfig {
        servers: vec![server],
        client_name: Some("roze-example-external-verify".to_string()),
        subject_prefix: "roze.verify".to_string(),
        jetstream: roze_nats::JetStreamConfig {
            stream: "ROZE_VERIFY".to_string(),
            subjects: vec!["events".to_string()],
            durable: "roze-verify".to_string(),
            ..Default::default()
        },
    })
    .await?;
    roze_mq::Publisher::publish(
        &broker,
        roze_mq::Message::new("events", json!({"source": "roze-example", "status": "ok"})),
    )
    .await?;
    println!("nats PASS");
    Ok(())
}

async fn verify_mongo(url: String) -> anyhow::Result<()> {
    let mongo = roze_mongo::connect(&roze_mongo::MongoConfig {
        url,
        database: "roze".to_string(),
        max_pool_size: 5,
        min_pool_size: 0,
        app_name: Some("roze-example-external-verify".to_string()),
    })
    .await?;
    let collection = mongo.collection::<roze_mongo::bson::Document>("external_verify");
    let id = format!("roze-example-{}", std::process::id());
    collection
        .insert_one(roze_mongo::bson::doc! {
            "_id": &id,
            "status": "ok",
        })
        .await?;
    let found = collection
        .find_one(roze_mongo::bson::doc! {
            "_id": &id,
        })
        .await?;
    anyhow::ensure!(found.is_some(), "mongo document not found");
    collection
        .delete_one(roze_mongo::bson::doc! {
            "_id": &id,
        })
        .await?;
    println!("mongo PASS");
    Ok(())
}

async fn verify_search(
    label: &str,
    engine: roze_search::SearchEngine,
    url: String,
    api_key: Option<String>,
) -> anyhow::Result<()> {
    let client = roze_search::SearchClient::new(roze_search::SearchConfig {
        engine,
        url,
        api_key,
    });
    client.health().await?;
    client
        .index_document(
            "roze_external_verify",
            "1",
            &json!({"id": "1", "status": "ok"}),
        )
        .await?;
    println!("{label} PASS");
    Ok(())
}

async fn verify_registry(
    label: &str,
    kind: roze_config::RegistryKind,
    endpoint: String,
    addr: &str,
    weight: u32,
) -> anyhow::Result<()> {
    let registry = roze_rpc::registry::build_registry(&roze_config::RegistryConfig {
        kind,
        endpoints: vec![endpoint],
        prefix: "/roze/services".to_string(),
        ttl_seconds: 10,
        renew_interval_secs: 2,
        user: None,
        pass: None,
        cert_file: None,
        cert_key_file: None,
        ca_cert_file: None,
        insecure_skip_verify: false,
    })?;
    let name = format!("roze-example-{label}-{}", std::process::id());
    let mut instance = roze_rpc::registry::ServiceInstance::new(&name, addr);
    instance.weight = weight;
    instance.metadata = HashMap::from([("source".to_string(), "roze-example".to_string())]);
    registry.register(instance).await?;
    tokio::time::sleep(Duration::from_millis(250)).await;
    let instances = registry.discover(&name).await?;
    anyhow::ensure!(
        instances.iter().any(|item| item.addr == addr),
        "{label} not discovered"
    );
    registry.deregister(&name, addr).await?;
    println!("{label} PASS");
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    verify_sql(
        "postgres",
        DatabaseBackend::Postgres,
        env_or(
            "ROZE_VERIFY_POSTGRES_URL",
            "postgres://postgres:postgres@127.0.0.1:15432/roze",
        ),
    )
    .await?;
    verify_sql(
        "mysql",
        DatabaseBackend::MySql,
        env_or(
            "ROZE_VERIFY_MYSQL_URL",
            "mysql://root:root@127.0.0.1:13306/roze",
        ),
    )
    .await?;
    verify_redis(env_or("ROZE_VERIFY_REDIS_URL", "redis://127.0.0.1:16379/")).await?;
    verify_kafka(env_or("ROZE_VERIFY_KAFKA_BROKER", "127.0.0.1:19092")).await?;
    verify_nats(env_or("ROZE_VERIFY_NATS_SERVER", "127.0.0.1:14222")).await?;
    verify_mongo(env_or(
        "ROZE_VERIFY_MONGO_URL",
        "mongodb://127.0.0.1:27018/roze",
    ))
    .await?;
    verify_search(
        "elasticsearch",
        roze_search::SearchEngine::Elasticsearch,
        env_or("ROZE_VERIFY_ELASTICSEARCH_URL", "http://127.0.0.1:19200"),
        None,
    )
    .await?;
    verify_search(
        "opensearch",
        roze_search::SearchEngine::Opensearch,
        env_or("ROZE_VERIFY_OPENSEARCH_URL", "http://127.0.0.1:19201"),
        None,
    )
    .await?;
    verify_search(
        "meilisearch",
        roze_search::SearchEngine::Meilisearch,
        env_or("ROZE_VERIFY_MEILISEARCH_URL", "http://127.0.0.1:17700"),
        Some(env_or(
            "ROZE_VERIFY_MEILI_MASTER_KEY",
            "roze_meili_master_key",
        )),
    )
    .await?;
    verify_registry(
        "etcd",
        roze_config::RegistryKind::Etcd,
        env_or("ROZE_VERIFY_ETCD_URL", "http://127.0.0.1:12379"),
        "127.0.0.1:18080",
        3,
    )
    .await?;
    verify_registry(
        "consul",
        roze_config::RegistryKind::Consul,
        env_or("ROZE_VERIFY_CONSUL_URL", "http://127.0.0.1:18500"),
        "127.0.0.1:18081",
        5,
    )
    .await?;
    Ok(())
}
