use std::{
    collections::{BTreeMap, BTreeSet},
    future::Future,
    pin::Pin,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

pub use roze_config::{
    DatabaseConfig, DatabaseMode, DatabaseReadPolicy, DatabaseRouting, DatabaseShardConfig,
    DatabaseTopologyConfig,
};
pub use sea_orm::DatabaseConnection;
use sea_orm::{
    ConnectOptions, ConnectionTrait, Database, DbErr, TransactionError, TransactionTrait,
};

pub async fn connect(config: &DatabaseConfig) -> Result<DatabaseConnection, DbErr> {
    if config.mode == DatabaseMode::Sharded {
        return Err(DbErr::Custom(
            "database.mode=sharded requires connect_sharded or connect_runtime".into(),
        ));
    }
    if config.url.trim().is_empty() {
        return Err(DbErr::Custom(
            "database.url must not be empty for direct or proxy mode".into(),
        ));
    }
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

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ShardId(String);

impl ShardId {
    pub fn new(value: impl Into<String>) -> Result<Self, DbErr> {
        let value = value.into();
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return Err(DbErr::Custom("database shard id must not be empty".into()));
        }
        if !trimmed
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
        {
            return Err(DbErr::Custom(format!(
                "database shard id `{trimmed}` contains unsupported characters"
            )));
        }
        Ok(Self(trimmed.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ShardId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

pub trait ShardKey {
    fn stable_hash(&self) -> u64;
}

impl ShardKey for str {
    fn stable_hash(&self) -> u64 {
        fnv1a64(self.as_bytes())
    }
}

impl ShardKey for String {
    fn stable_hash(&self) -> u64 {
        self.as_str().stable_hash()
    }
}

impl ShardKey for [u8] {
    fn stable_hash(&self) -> u64 {
        fnv1a64(self)
    }
}

impl ShardKey for Vec<u8> {
    fn stable_hash(&self) -> u64 {
        self.as_slice().stable_hash()
    }
}

macro_rules! impl_integer_shard_key {
    ($($ty:ty),+ $(,)?) => {
        $(
            impl ShardKey for $ty {
                fn stable_hash(&self) -> u64 {
                    fnv1a64(&self.to_le_bytes())
                }
            }
        )+
    };
}

impl_integer_shard_key!(i8, i16, i32, i64, i128, u8, u16, u32, u64, u128);

impl ShardKey for isize {
    fn stable_hash(&self) -> u64 {
        (*self as i64).stable_hash()
    }
}

impl ShardKey for usize {
    fn stable_hash(&self) -> u64 {
        (*self as u64).stable_hash()
    }
}

#[derive(Clone, Debug)]
pub struct ShardRouter {
    topology: Arc<str>,
    shard_ids: Arc<[ShardId]>,
}

impl ShardRouter {
    pub fn new(
        topology: impl Into<String>,
        shard_ids: impl IntoIterator<Item = ShardId>,
    ) -> Result<Self, DbErr> {
        let topology = topology.into();
        let topology = topology.trim();
        if topology.is_empty() {
            return Err(DbErr::Custom(
                "database topology name must not be empty".into(),
            ));
        }
        if !topology
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
        {
            return Err(DbErr::Custom(format!(
                "database topology name `{topology}` contains unsupported characters"
            )));
        }
        let mut shard_ids = shard_ids.into_iter().collect::<Vec<_>>();
        shard_ids.sort();
        if shard_ids.is_empty() {
            return Err(DbErr::Custom(format!(
                "database topology `{topology}` must contain at least one shard"
            )));
        }
        if shard_ids.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(DbErr::Custom(format!(
                "database topology `{topology}` contains duplicate shard ids"
            )));
        }
        Ok(Self {
            topology: topology.into(),
            shard_ids: shard_ids.into(),
        })
    }

    pub fn topology(&self) -> &str {
        &self.topology
    }

    pub fn shard_ids(&self) -> &[ShardId] {
        &self.shard_ids
    }

    pub fn route<K>(&self, key: &K) -> &ShardId
    where
        K: ShardKey + ?Sized,
    {
        let index = jump_consistent_hash(key.stable_hash(), self.shard_ids.len());
        &self.shard_ids[index]
    }
}

#[derive(Clone, Debug)]
pub struct ShardRoute {
    topology: Arc<str>,
    shard_id: ShardId,
    connections: DatabaseConnections,
}

impl ShardRoute {
    pub fn topology(&self) -> &str {
        &self.topology
    }

    pub fn shard_id(&self) -> &ShardId {
        &self.shard_id
    }

    pub fn read(&self) -> &DatabaseConnection {
        self.connections.read()
    }

    pub fn write(&self) -> &DatabaseConnection {
        self.connections.write()
    }

    pub fn connections(&self) -> &DatabaseConnections {
        &self.connections
    }
}

#[derive(Clone, Debug)]
pub struct ShardedDatabase {
    router: ShardRouter,
    shards: Arc<BTreeMap<ShardId, DatabaseConnections>>,
}

impl ShardedDatabase {
    pub fn topology(&self) -> &str {
        self.router.topology()
    }

    pub fn shard_ids(&self) -> &[ShardId] {
        self.router.shard_ids()
    }

    pub fn route<K>(&self, key: &K) -> ShardRoute
    where
        K: ShardKey + ?Sized,
    {
        let shard_id = self.router.route(key).clone();
        roze_metrics::record_database_shard_route(self.router.topology(), shard_id.as_str());
        let connections = self
            .shards
            .get(&shard_id)
            .expect("router and shard connection map are built together")
            .clone();
        ShardRoute {
            topology: self.router.topology.clone(),
            shard_id,
            connections,
        }
    }

    pub async fn health_check(&self) -> Result<(), DbErr> {
        for (shard_id, connections) in self.shards.iter() {
            match connections.health_check().await {
                Ok(()) => roze_metrics::record_database_shard_health(
                    self.router.topology(),
                    shard_id.as_str(),
                    "ok",
                ),
                Err(error) => {
                    roze_metrics::record_database_shard_health(
                        self.router.topology(),
                        shard_id.as_str(),
                        "error",
                    );
                    return Err(error);
                }
            }
        }
        Ok(())
    }

    pub async fn transaction_for_key<K, F, T>(&self, key: &K, operation: F) -> Result<T, DbErr>
    where
        K: ShardKey + ?Sized,
        F: for<'transaction> FnOnce(
                ShardTransaction<'transaction>,
            ) -> Pin<
                Box<dyn Future<Output = Result<T, DbErr>> + Send + 'transaction>,
            > + Send,
        T: Send,
    {
        let route = self.route(key);
        let shard_id = route.shard_id.clone();
        let router = self.router.clone();
        route
            .write()
            .transaction(move |transaction| {
                operation(ShardTransaction {
                    router,
                    shard_id,
                    transaction,
                })
            })
            .await
            .map_err(|error: TransactionError<DbErr>| match error {
                TransactionError::Connection(error) | TransactionError::Transaction(error) => error,
            })
    }
}

pub struct ShardTransaction<'transaction> {
    router: ShardRouter,
    shard_id: ShardId,
    transaction: &'transaction sea_orm::DatabaseTransaction,
}

impl<'transaction> ShardTransaction<'transaction> {
    pub fn shard_id(&self) -> &ShardId {
        &self.shard_id
    }

    pub fn connection(&self) -> &sea_orm::DatabaseTransaction {
        self.transaction
    }

    pub fn ensure_key<K>(&self, key: &K) -> Result<(), DbErr>
    where
        K: ShardKey + ?Sized,
    {
        let resolved = self.router.route(key);
        if resolved == &self.shard_id {
            return Ok(());
        }
        Err(DbErr::Custom(format!(
            "cross-shard transaction is not supported: transaction is pinned to `{}`, key resolves to `{resolved}`",
            self.shard_id
        )))
    }
}

#[derive(Clone, Debug)]
pub enum DatabaseRuntime {
    Direct(DatabaseConnections),
    Sharded(ShardedDatabase),
}

impl DatabaseRuntime {
    pub fn direct(&self) -> Option<&DatabaseConnections> {
        match self {
            Self::Direct(connections) => Some(connections),
            Self::Sharded(_) => None,
        }
    }

    pub fn sharded(&self) -> Option<&ShardedDatabase> {
        match self {
            Self::Direct(_) => None,
            Self::Sharded(database) => Some(database),
        }
    }

    pub async fn health_check(&self) -> Result<(), DbErr> {
        match self {
            Self::Direct(connections) => connections.health_check().await,
            Self::Sharded(database) => database.health_check().await,
        }
    }
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

    pub async fn health_check(&self) -> Result<(), DbErr> {
        self.primary.execute_unprepared("SELECT 1").await?;
        for replica in &self.replicas {
            replica.execute_unprepared("SELECT 1").await?;
        }
        Ok(())
    }
}

pub async fn connect_connections(config: &DatabaseConfig) -> Result<DatabaseConnections, DbErr> {
    if config.mode == DatabaseMode::Sharded {
        return Err(DbErr::Custom(
            "database.mode=sharded does not expose one implicit connection; route with ShardedDatabase"
                .into(),
        ));
    }
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

pub async fn connect_sharded(config: &DatabaseConfig) -> Result<ShardedDatabase, DbErr> {
    if config.mode != DatabaseMode::Sharded {
        return Err(DbErr::Custom(
            "connect_sharded requires database.mode=sharded".into(),
        ));
    }
    if !config.url.trim().is_empty() || !config.replicas.is_empty() {
        return Err(DbErr::Custom(
            "database.mode=sharded must configure topology.shards instead of database.url or database.replicas"
                .into(),
        ));
    }
    let topology = config
        .topology
        .as_ref()
        .ok_or_else(|| DbErr::Custom("database.mode=sharded requires database.topology".into()))?;
    validate_topology(topology)?;

    let mut shards = BTreeMap::new();
    for shard in &topology.shards {
        let shard_id = ShardId::new(&shard.id)?;
        let connections = connect_shard_connections(config, shard).await?;
        shards.insert(shard_id, connections);
    }
    let router = ShardRouter::new(topology.name.clone(), shards.keys().cloned())?;
    Ok(ShardedDatabase {
        router,
        shards: Arc::new(shards),
    })
}

pub async fn connect_runtime(config: &DatabaseConfig) -> Result<DatabaseRuntime, DbErr> {
    match config.mode {
        DatabaseMode::Direct | DatabaseMode::Proxy => connect_connections(config)
            .await
            .map(DatabaseRuntime::Direct),
        DatabaseMode::Sharded => connect_sharded(config).await.map(DatabaseRuntime::Sharded),
    }
}

pub async fn connect_runtime_optional(
    config: Option<&DatabaseConfig>,
) -> Result<Option<DatabaseRuntime>, DbErr> {
    match config {
        Some(config) => connect_runtime(config).await.map(Some),
        None => Ok(None),
    }
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

async fn connect_shard_connections(
    config: &DatabaseConfig,
    shard: &DatabaseShardConfig,
) -> Result<DatabaseConnections, DbErr> {
    let primary = connect_url(config, &shard.primary).await?;
    let mut replicas = Vec::with_capacity(shard.replicas.len());
    for replica in &shard.replicas {
        replicas.push(connect_url(config, replica).await?);
    }
    Ok(DatabaseConnections {
        primary,
        replicas,
        policy: config.policy,
        read_cursor: Arc::new(AtomicUsize::new(0)),
    })
}

fn validate_topology(topology: &DatabaseTopologyConfig) -> Result<(), DbErr> {
    if topology.name.trim().is_empty() {
        return Err(DbErr::Custom(
            "database.topology.name must not be empty".into(),
        ));
    }
    if topology.shards.is_empty() {
        return Err(DbErr::Custom(format!(
            "database topology `{}` must contain at least one shard",
            topology.name
        )));
    }
    let mut ids = BTreeSet::new();
    for shard in &topology.shards {
        let shard_id = ShardId::new(&shard.id)?;
        if !ids.insert(shard_id.clone()) {
            return Err(DbErr::Custom(format!(
                "database topology `{}` contains duplicate shard id `{shard_id}`",
                topology.name
            )));
        }
        if shard.primary.trim().is_empty() {
            return Err(DbErr::Custom(format!(
                "database shard `{shard_id}` primary URL must not be empty"
            )));
        }
        if shard.replicas.iter().any(|url| url.trim().is_empty()) {
            return Err(DbErr::Custom(format!(
                "database shard `{shard_id}` contains an empty replica URL"
            )));
        }
    }
    Ok(())
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn jump_consistent_hash(mut key: u64, buckets: usize) -> usize {
    debug_assert!(buckets > 0);
    let mut previous = -1_i64;
    let mut candidate = 0_i64;
    while candidate < buckets as i64 {
        previous = candidate;
        key = key.wrapping_mul(2_862_933_555_777_941_757).wrapping_add(1);
        candidate =
            ((previous + 1) as f64 * ((1_u64 << 31) as f64) / (((key >> 33) + 1) as f64)) as i64;
    }
    previous as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configuration_round_trips() {
        let config = DatabaseConfig {
            mode: DatabaseMode::Direct,
            url: "sqlite::memory:".to_string(),
            replicas: Vec::new(),
            topology: None,
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
            mode: DatabaseMode::Direct,
            url: "sqlite::memory:".to_string(),
            replicas: Vec::new(),
            topology: None,
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

    #[test]
    fn router_is_deterministic_and_independent_of_config_order() {
        let ordered = ShardRouter::new(
            "commerce",
            ["shard-00", "shard-01", "shard-02"]
                .into_iter()
                .map(ShardId::new)
                .collect::<Result<Vec<_>, _>>()
                .unwrap(),
        )
        .unwrap();
        let reversed = ShardRouter::new(
            "commerce",
            ["shard-02", "shard-01", "shard-00"]
                .into_iter()
                .map(ShardId::new)
                .collect::<Result<Vec<_>, _>>()
                .unwrap(),
        )
        .unwrap();

        for key in 0_u64..10_000 {
            assert_eq!(ordered.route(&key), reversed.route(&key));
        }
    }

    #[test]
    fn pointer_sized_integer_keys_have_architecture_independent_encoding() {
        assert_eq!(42_isize.stable_hash(), 42_i64.stable_hash());
        assert_eq!(42_usize.stable_hash(), 42_u64.stable_hash());
    }

    #[test]
    fn adding_one_shard_moves_only_a_bounded_fraction_of_keys() {
        let three = ShardRouter::new(
            "commerce",
            ["shard-00", "shard-01", "shard-02"]
                .into_iter()
                .map(ShardId::new)
                .collect::<Result<Vec<_>, _>>()
                .unwrap(),
        )
        .unwrap();
        let four = ShardRouter::new(
            "commerce",
            ["shard-00", "shard-01", "shard-02", "shard-03"]
                .into_iter()
                .map(ShardId::new)
                .collect::<Result<Vec<_>, _>>()
                .unwrap(),
        )
        .unwrap();

        let moved = (0_u64..100_000)
            .filter(|key| three.route(key) != four.route(key))
            .count();
        assert!((20_000..=30_000).contains(&moved), "moved={moved}");
    }

    #[test]
    fn shard_ids_reject_unsafe_metric_and_log_values() {
        assert!(ShardId::new("tenant/42").is_err());
        assert!(ShardId::new("shard-00").is_ok());
        assert!(ShardRouter::new("tenant/42", [ShardId::new("shard-00").unwrap()]).is_err());
    }

    #[tokio::test]
    async fn transaction_is_pinned_to_one_resolved_shard() {
        let config = DatabaseConfig {
            mode: DatabaseMode::Direct,
            url: "sqlite::memory:".to_string(),
            replicas: Vec::new(),
            topology: None,
            policy: DatabaseReadPolicy::RoundRobin,
            max_connections: 1,
            min_connections: 1,
            connect_timeout_secs: 3,
            idle_timeout_secs: 30,
            sqlx_logging: false,
        };
        let mut shards = BTreeMap::new();
        for shard in ["shard-00", "shard-01"] {
            shards.insert(
                ShardId::new(shard).unwrap(),
                connect_connections(&config).await.unwrap(),
            );
        }
        let database = ShardedDatabase {
            router: ShardRouter::new("commerce", shards.keys().cloned()).unwrap(),
            shards: Arc::new(shards),
        };
        let first_key = 0_u64;
        let first_shard = database.router.route(&first_key).clone();
        let second_key = (1_u64..10_000)
            .find(|key| database.router.route(key) != &first_shard)
            .expect("key for another shard");

        database
            .transaction_for_key(&first_key, |transaction| {
                Box::pin(async move {
                    transaction.ensure_key(&first_key)?;
                    let error = transaction
                        .ensure_key(&second_key)
                        .expect_err("cross-shard key must fail");
                    assert!(error.to_string().contains("cross-shard transaction"));
                    transaction
                        .connection()
                        .execute_unprepared("SELECT 1")
                        .await?;
                    Ok(())
                })
            })
            .await
            .unwrap();
    }
}
