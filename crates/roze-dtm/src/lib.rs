use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};
use tokio::sync::RwLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TransactionKind {
    Saga,
    Tcc,
}

impl Default for TransactionKind {
    fn default() -> Self {
        Self::Tcc
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TransactionStatus {
    Submitted,
    Trying,
    Prepared,
    Succeeding,
    Succeeded,
    Aborting,
    Aborted,
    Failed,
}

impl TransactionStatus {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Succeeded | Self::Aborted | Self::Failed)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BranchKind {
    SagaAction,
    SagaCompensate,
    TccTry,
    TccConfirm,
    TccCancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BranchStatus {
    Pending,
    Running,
    Succeeded,
    Failed,
    Skipped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Branch {
    pub id: String,
    pub kind: BranchKind,
    pub action: String,
    pub compensate: Option<String>,
    #[serde(default)]
    pub confirm: Option<String>,
    #[serde(default)]
    pub cancel: Option<String>,
    pub payload: serde_json::Value,
    pub status: BranchStatus,
    pub attempts: u32,
    pub last_error: Option<String>,
    #[serde(default)]
    pub next_retry_millis: Option<u64>,
}

impl Branch {
    pub fn saga(
        id: impl Into<String>,
        action: impl Into<String>,
        compensate: impl Into<String>,
        payload: serde_json::Value,
    ) -> Self {
        Self {
            id: id.into(),
            kind: BranchKind::SagaAction,
            action: action.into(),
            compensate: Some(compensate.into()),
            confirm: None,
            cancel: None,
            payload,
            status: BranchStatus::Pending,
            attempts: 0,
            last_error: None,
            next_retry_millis: None,
        }
    }

    pub fn tcc_try(
        id: impl Into<String>,
        try_action: impl Into<String>,
        confirm_action: impl Into<String>,
        cancel_action: impl Into<String>,
        payload: serde_json::Value,
    ) -> Self {
        Self {
            id: id.into(),
            kind: BranchKind::TccTry,
            action: try_action.into(),
            compensate: None,
            confirm: Some(confirm_action.into()),
            cancel: Some(cancel_action.into()),
            payload,
            status: BranchStatus::Pending,
            attempts: 0,
            last_error: None,
            next_retry_millis: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transaction {
    pub gid: String,
    pub kind: TransactionKind,
    pub status: TransactionStatus,
    pub branches: Vec<Branch>,
    pub created_at_millis: u64,
    pub updated_at_millis: u64,
    pub timeout_millis: Option<u64>,
    pub metadata: BTreeMap<String, String>,
}

impl Transaction {
    pub fn default_tcc(gid: impl Into<String>, branches: Vec<Branch>) -> Self {
        Self::tcc(gid, branches)
    }

    pub fn saga(gid: impl Into<String>, branches: Vec<Branch>) -> Self {
        Self::new(gid, TransactionKind::Saga, branches)
    }

    pub fn tcc(gid: impl Into<String>, branches: Vec<Branch>) -> Self {
        Self::new(gid, TransactionKind::Tcc, branches)
    }

    pub fn new(gid: impl Into<String>, kind: TransactionKind, branches: Vec<Branch>) -> Self {
        let now = current_millis();
        Self {
            gid: gid.into(),
            kind,
            status: TransactionStatus::Submitted,
            branches,
            created_at_millis: now,
            updated_at_millis: now,
            timeout_millis: None,
            metadata: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DtmOptions {
    pub max_attempts: u32,
    pub retry_backoff_millis: u64,
    pub max_retry_backoff_millis: u64,
    pub branch_call_timeout_millis: u64,
    pub transaction_timeout_millis: u64,
}

impl Default for DtmOptions {
    fn default() -> Self {
        Self {
            max_attempts: 5,
            retry_backoff_millis: 1_000,
            max_retry_backoff_millis: 30_000,
            branch_call_timeout_millis: 5_000,
            transaction_timeout_millis: 60_000,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BranchBarrier {
    pub gid: String,
    pub branch_id: String,
    pub op: String,
}

impl BranchBarrier {
    pub fn new(
        gid: impl Into<String>,
        branch_id: impl Into<String>,
        op: impl Into<String>,
    ) -> Self {
        Self {
            gid: gid.into(),
            branch_id: branch_id.into(),
            op: op.into(),
        }
    }

    fn key(&self) -> String {
        format!("{}:{}:{}", self.gid, self.branch_id, self.op)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BarrierDecision {
    Execute,
    SkipDuplicate,
    SkipNullCompensation,
}

#[async_trait]
pub trait TransactionStore: Send + Sync + 'static {
    async fn insert_transaction(&self, tx: Transaction) -> anyhow::Result<()>;
    async fn get_transaction(&self, gid: &str) -> anyhow::Result<Option<Transaction>>;
    async fn update_transaction(&self, tx: Transaction) -> anyhow::Result<()>;
    async fn list_transactions(&self) -> anyhow::Result<Vec<Transaction>>;
    async fn barrier(&self, barrier: BranchBarrier) -> anyhow::Result<BarrierDecision>;
    async fn try_acquire_recovery_lease(
        &self,
        _name: &str,
        _owner: &str,
        _ttl_millis: u64,
    ) -> anyhow::Result<bool> {
        Ok(true)
    }
}

#[async_trait]
impl<T> TransactionStore for Arc<T>
where
    T: TransactionStore + ?Sized,
{
    async fn insert_transaction(&self, tx: Transaction) -> anyhow::Result<()> {
        (**self).insert_transaction(tx).await
    }

    async fn get_transaction(&self, gid: &str) -> anyhow::Result<Option<Transaction>> {
        (**self).get_transaction(gid).await
    }

    async fn update_transaction(&self, tx: Transaction) -> anyhow::Result<()> {
        (**self).update_transaction(tx).await
    }

    async fn list_transactions(&self) -> anyhow::Result<Vec<Transaction>> {
        (**self).list_transactions().await
    }

    async fn barrier(&self, barrier: BranchBarrier) -> anyhow::Result<BarrierDecision> {
        (**self).barrier(barrier).await
    }

    async fn try_acquire_recovery_lease(
        &self,
        name: &str,
        owner: &str,
        ttl_millis: u64,
    ) -> anyhow::Result<bool> {
        (**self)
            .try_acquire_recovery_lease(name, owner, ttl_millis)
            .await
    }
}

#[async_trait]
pub trait BranchInvoker: Clone + Send + Sync + 'static {
    async fn invoke(&self, url: &str, payload: &serde_json::Value) -> anyhow::Result<()>;
}

#[derive(Debug, Clone, Default)]
pub struct NoopBranchInvoker;

#[async_trait]
impl BranchInvoker for NoopBranchInvoker {
    async fn invoke(&self, _url: &str, _payload: &serde_json::Value) -> anyhow::Result<()> {
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct HttpBranchInvoker {
    client: reqwest::Client,
}

impl HttpBranchInvoker {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
        }
    }

    pub fn with_timeout(timeout: Duration) -> anyhow::Result<Self> {
        Ok(Self {
            client: reqwest::Client::builder().timeout(timeout).build()?,
        })
    }
}

impl Default for HttpBranchInvoker {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl BranchInvoker for HttpBranchInvoker {
    async fn invoke(&self, url: &str, payload: &serde_json::Value) -> anyhow::Result<()> {
        let response = self.client.post(url).json(payload).send().await?;
        if response.status().is_success() {
            Ok(())
        } else {
            anyhow::bail!("branch call {url} failed with status {}", response.status())
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct InMemoryTransactionStore {
    txs: Arc<RwLock<BTreeMap<String, Transaction>>>,
    barriers: Arc<RwLock<BTreeSet<String>>>,
    leases: Arc<RwLock<BTreeMap<String, RecoveryLease>>>,
}

impl InMemoryTransactionStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[derive(Debug, Clone)]
struct RecoveryLease {
    owner: String,
    expires_at_millis: u64,
}

#[async_trait]
impl TransactionStore for InMemoryTransactionStore {
    async fn insert_transaction(&self, tx: Transaction) -> anyhow::Result<()> {
        let mut txs = self.txs.write().await;
        if txs.contains_key(&tx.gid) {
            anyhow::bail!("transaction already exists: {}", tx.gid);
        }
        txs.insert(tx.gid.clone(), tx);
        Ok(())
    }

    async fn get_transaction(&self, gid: &str) -> anyhow::Result<Option<Transaction>> {
        Ok(self.txs.read().await.get(gid).cloned())
    }

    async fn update_transaction(&self, mut tx: Transaction) -> anyhow::Result<()> {
        tx.updated_at_millis = current_millis();
        self.txs.write().await.insert(tx.gid.clone(), tx);
        Ok(())
    }

    async fn list_transactions(&self) -> anyhow::Result<Vec<Transaction>> {
        Ok(self.txs.read().await.values().cloned().collect())
    }

    async fn barrier(&self, barrier: BranchBarrier) -> anyhow::Result<BarrierDecision> {
        let mut barriers = self.barriers.write().await;
        let key = barrier.key();
        if barriers.contains(&key) {
            return Ok(BarrierDecision::SkipDuplicate);
        }

        let cancel_key = format!("{}:{}:cancel", barrier.gid, barrier.branch_id);
        let try_key = format!("{}:{}:try", barrier.gid, barrier.branch_id);
        if barrier.op == "cancel" && !barriers.contains(&try_key) {
            barriers.insert(cancel_key);
            return Ok(BarrierDecision::SkipNullCompensation);
        }

        barriers.insert(key);
        Ok(BarrierDecision::Execute)
    }

    async fn try_acquire_recovery_lease(
        &self,
        name: &str,
        owner: &str,
        ttl_millis: u64,
    ) -> anyhow::Result<bool> {
        let now = current_millis();
        let mut leases = self.leases.write().await;
        if let Some(lease) = leases.get(name) {
            if lease.owner != owner && lease.expires_at_millis > now {
                return Ok(false);
            }
        }

        leases.insert(
            name.to_owned(),
            RecoveryLease {
                owner: owner.to_owned(),
                expires_at_millis: now.saturating_add(ttl_millis),
            },
        );
        Ok(true)
    }
}

#[derive(Debug, Clone)]
pub struct SqliteTransactionStore {
    pool: SqlitePool,
}

impl SqliteTransactionStore {
    pub async fn connect(database_url: &str) -> anyhow::Result<Self> {
        let pool = SqlitePool::connect(database_url).await?;
        let store = Self { pool };
        store.migrate().await?;
        Ok(store)
    }

    pub fn from_pool(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn migrate(&self) -> anyhow::Result<()> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS roze_dtm_transactions (
                gid TEXT PRIMARY KEY NOT NULL,
                payload TEXT NOT NULL,
                updated_at_millis INTEGER NOT NULL
            )
            "#,
        )
        .execute(&self.pool)
        .await?;
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS roze_dtm_barriers (
                barrier_key TEXT PRIMARY KEY NOT NULL,
                gid TEXT NOT NULL,
                branch_id TEXT NOT NULL,
                op TEXT NOT NULL,
                created_at_millis INTEGER NOT NULL
            )
            "#,
        )
        .execute(&self.pool)
        .await?;
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS roze_dtm_recovery_leases (
                name TEXT PRIMARY KEY NOT NULL,
                owner TEXT NOT NULL,
                expires_at_millis INTEGER NOT NULL
            )
            "#,
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

#[async_trait]
impl TransactionStore for SqliteTransactionStore {
    async fn insert_transaction(&self, tx: Transaction) -> anyhow::Result<()> {
        let payload = serde_json::to_string(&tx)?;
        let changed = sqlx::query(
            r#"
            INSERT OR IGNORE INTO roze_dtm_transactions (gid, payload, updated_at_millis)
            VALUES (?, ?, ?)
            "#,
        )
        .bind(&tx.gid)
        .bind(payload)
        .bind(tx.updated_at_millis as i64)
        .execute(&self.pool)
        .await?
        .rows_affected();

        if changed == 0 {
            anyhow::bail!("transaction already exists: {}", tx.gid);
        }
        Ok(())
    }

    async fn get_transaction(&self, gid: &str) -> anyhow::Result<Option<Transaction>> {
        let row = sqlx::query("SELECT payload FROM roze_dtm_transactions WHERE gid = ?")
            .bind(gid)
            .fetch_optional(&self.pool)
            .await?;
        row.map(|row| serde_json::from_str(row.get::<&str, _>("payload")).map_err(Into::into))
            .transpose()
    }

    async fn update_transaction(&self, mut tx: Transaction) -> anyhow::Result<()> {
        tx.updated_at_millis = current_millis();
        let payload = serde_json::to_string(&tx)?;
        sqlx::query(
            r#"
            INSERT INTO roze_dtm_transactions (gid, payload, updated_at_millis)
            VALUES (?, ?, ?)
            ON CONFLICT(gid) DO UPDATE SET
                payload = excluded.payload,
                updated_at_millis = excluded.updated_at_millis
            "#,
        )
        .bind(&tx.gid)
        .bind(payload)
        .bind(tx.updated_at_millis as i64)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn list_transactions(&self) -> anyhow::Result<Vec<Transaction>> {
        let rows =
            sqlx::query("SELECT payload FROM roze_dtm_transactions ORDER BY updated_at_millis ASC")
                .fetch_all(&self.pool)
                .await?;
        rows.into_iter()
            .map(|row| serde_json::from_str(row.get::<&str, _>("payload")).map_err(Into::into))
            .collect()
    }

    async fn barrier(&self, barrier: BranchBarrier) -> anyhow::Result<BarrierDecision> {
        let key = barrier.key();
        let existing: Option<(String,)> =
            sqlx::query_as("SELECT barrier_key FROM roze_dtm_barriers WHERE barrier_key = ?")
                .bind(&key)
                .fetch_optional(&self.pool)
                .await?;
        if existing.is_some() {
            return Ok(BarrierDecision::SkipDuplicate);
        }

        if barrier.op == "cancel" {
            let try_key = format!("{}:{}:try", barrier.gid, barrier.branch_id);
            let tried: Option<(String,)> =
                sqlx::query_as("SELECT barrier_key FROM roze_dtm_barriers WHERE barrier_key = ?")
                    .bind(&try_key)
                    .fetch_optional(&self.pool)
                    .await?;
            if tried.is_none() {
                insert_barrier(&self.pool, &key, &barrier).await?;
                return Ok(BarrierDecision::SkipNullCompensation);
            }
        }

        insert_barrier(&self.pool, &key, &barrier).await?;
        Ok(BarrierDecision::Execute)
    }

    async fn try_acquire_recovery_lease(
        &self,
        name: &str,
        owner: &str,
        ttl_millis: u64,
    ) -> anyhow::Result<bool> {
        let now = current_millis();
        let expires_at = now.saturating_add(ttl_millis);
        let mut tx = self.pool.begin().await?;
        let current: Option<(String, i64)> = sqlx::query_as(
            "SELECT owner, expires_at_millis FROM roze_dtm_recovery_leases WHERE name = ?",
        )
        .bind(name)
        .fetch_optional(&mut *tx)
        .await?;
        if let Some((current_owner, current_expires_at)) = current {
            if current_owner != owner && current_expires_at as u64 > now {
                tx.commit().await?;
                return Ok(false);
            }
        }
        sqlx::query(
            r#"
            INSERT INTO roze_dtm_recovery_leases (name, owner, expires_at_millis)
            VALUES (?, ?, ?)
            ON CONFLICT(name) DO UPDATE SET
                owner = excluded.owner,
                expires_at_millis = excluded.expires_at_millis
            "#,
        )
        .bind(name)
        .bind(owner)
        .bind(expires_at as i64)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(true)
    }
}

#[derive(Debug, Clone)]
pub struct Dtm<S, I = NoopBranchInvoker> {
    store: S,
    invoker: I,
    options: DtmOptions,
}

impl<S> Dtm<S, NoopBranchInvoker>
where
    S: TransactionStore,
{
    pub fn new(store: S) -> Self {
        Self {
            store,
            invoker: NoopBranchInvoker,
            options: DtmOptions::default(),
        }
    }
}

impl<S, I> Dtm<S, I>
where
    S: TransactionStore,
    I: BranchInvoker,
{
    pub fn with_invoker(store: S, invoker: I) -> Self {
        Self {
            store,
            invoker,
            options: DtmOptions::default(),
        }
    }

    pub fn with_options(store: S, invoker: I, options: DtmOptions) -> Self {
        Self {
            store,
            invoker,
            options,
        }
    }

    pub fn store(&self) -> &S {
        &self.store
    }

    pub async fn submit(&self, tx: Transaction) -> anyhow::Result<Transaction> {
        let mut tx = tx;
        tx.timeout_millis
            .get_or_insert(self.options.transaction_timeout_millis);
        self.store.insert_transaction(tx.clone()).await?;
        Ok(tx)
    }

    pub async fn submit_default_tcc(
        &self,
        gid: impl Into<String>,
        branches: Vec<Branch>,
    ) -> anyhow::Result<Transaction> {
        self.submit(Transaction::default_tcc(gid, branches)).await
    }

    pub async fn start_saga(&self, gid: &str) -> anyhow::Result<Transaction> {
        let mut tx = self
            .store
            .get_transaction(gid)
            .await?
            .ok_or_else(|| anyhow::anyhow!("transaction not found: {gid}"))?;
        ensure_kind(&tx, TransactionKind::Saga)?;
        tx.status = TransactionStatus::Succeeding;
        let mut applied = Vec::new();
        for (idx, branch) in tx.branches.iter_mut().enumerate() {
            branch.status = BranchStatus::Running;
            branch.attempts = branch.attempts.saturating_add(1);
            let action = branch.action.clone();
            match self.invoke_branch(branch, &action).await {
                Ok(()) => {
                    branch.status = BranchStatus::Succeeded;
                    branch.next_retry_millis = None;
                    applied.push(idx);
                }
                Err(err) => {
                    branch.status = BranchStatus::Failed;
                    branch.last_error = Some(err.to_string());
                    tx.status = TransactionStatus::Aborting;
                    for idx in applied.into_iter().rev() {
                        let previous = &mut tx.branches[idx];
                        if let Some(compensate) = previous.compensate.as_deref() {
                            let _ = self.invoker.invoke(compensate, &previous.payload).await;
                            previous.status = BranchStatus::Skipped;
                        }
                    }
                    tx.status = TransactionStatus::Aborted;
                    self.store.update_transaction(tx.clone()).await?;
                    return Ok(tx);
                }
            }
        }
        tx.status = TransactionStatus::Succeeded;
        self.store.update_transaction(tx.clone()).await?;
        Ok(tx)
    }

    pub async fn abort_saga(&self, gid: &str) -> anyhow::Result<Transaction> {
        let mut tx = self
            .store
            .get_transaction(gid)
            .await?
            .ok_or_else(|| anyhow::anyhow!("transaction not found: {gid}"))?;
        ensure_kind(&tx, TransactionKind::Saga)?;
        tx.status = TransactionStatus::Aborting;
        for branch in tx.branches.iter_mut().rev() {
            let barrier = BranchBarrier::new(&tx.gid, &branch.id, "compensate");
            match self.store.barrier(barrier).await? {
                BarrierDecision::Execute => {
                    if let Some(compensate) = branch.compensate.clone() {
                        self.invoke_url(branch, &compensate).await?;
                    }
                    branch.status = BranchStatus::Skipped;
                    branch.attempts = branch.attempts.saturating_add(1);
                }
                BarrierDecision::SkipDuplicate | BarrierDecision::SkipNullCompensation => {}
            }
        }
        tx.status = TransactionStatus::Aborted;
        self.store.update_transaction(tx.clone()).await?;
        Ok(tx)
    }

    pub async fn prepare_tcc(&self, gid: &str) -> anyhow::Result<Transaction> {
        let mut tx = self
            .store
            .get_transaction(gid)
            .await?
            .ok_or_else(|| anyhow::anyhow!("transaction not found: {gid}"))?;
        ensure_kind(&tx, TransactionKind::Tcc)?;
        tx.status = TransactionStatus::Trying;
        for branch in &mut tx.branches {
            let decision = self
                .store
                .barrier(BranchBarrier::new(&tx.gid, &branch.id, "try"))
                .await?;
            if decision == BarrierDecision::Execute {
                branch.status = BranchStatus::Running;
                branch.attempts = branch.attempts.saturating_add(1);
                let action = branch.action.clone();
                match self.invoke_branch(branch, &action).await {
                    Ok(()) => {
                        branch.status = BranchStatus::Succeeded;
                        branch.next_retry_millis = None;
                    }
                    Err(err) => {
                        branch.status = BranchStatus::Failed;
                        branch.last_error = Some(err.to_string());
                        if branch.attempts >= self.options.max_attempts {
                            tx.status = TransactionStatus::Aborting;
                        }
                        self.store.update_transaction(tx.clone()).await?;
                        return Ok(tx);
                    }
                }
            }
        }
        tx.status = TransactionStatus::Prepared;
        self.store.update_transaction(tx.clone()).await?;
        Ok(tx)
    }

    pub async fn confirm_tcc(&self, gid: &str) -> anyhow::Result<Transaction> {
        let mut tx = self
            .store
            .get_transaction(gid)
            .await?
            .ok_or_else(|| anyhow::anyhow!("transaction not found: {gid}"))?;
        ensure_kind(&tx, TransactionKind::Tcc)?;
        tx.status = TransactionStatus::Succeeding;
        for branch in &mut tx.branches {
            let decision = self
                .store
                .barrier(BranchBarrier::new(&tx.gid, &branch.id, "confirm"))
                .await?;
            if decision == BarrierDecision::Execute {
                let confirm = branch.confirm.clone().ok_or_else(|| {
                    anyhow::anyhow!("missing confirm action for branch {}", branch.id)
                })?;
                self.invoke_url(branch, &confirm).await?;
                branch.status = BranchStatus::Succeeded;
                branch.attempts = branch.attempts.saturating_add(1);
            }
        }
        tx.status = TransactionStatus::Succeeded;
        self.store.update_transaction(tx.clone()).await?;
        Ok(tx)
    }

    pub async fn cancel_tcc(&self, gid: &str) -> anyhow::Result<Transaction> {
        let mut tx = self
            .store
            .get_transaction(gid)
            .await?
            .ok_or_else(|| anyhow::anyhow!("transaction not found: {gid}"))?;
        ensure_kind(&tx, TransactionKind::Tcc)?;
        tx.status = TransactionStatus::Aborting;
        for branch in &mut tx.branches {
            match self
                .store
                .barrier(BranchBarrier::new(&tx.gid, &branch.id, "cancel"))
                .await?
            {
                BarrierDecision::Execute => {
                    let cancel = branch
                        .cancel
                        .clone()
                        .or(branch.compensate.clone())
                        .ok_or_else(|| {
                            anyhow::anyhow!("missing cancel action for branch {}", branch.id)
                        })?;
                    self.invoke_url(branch, &cancel).await?;
                    branch.status = BranchStatus::Skipped;
                    branch.attempts = branch.attempts.saturating_add(1);
                }
                BarrierDecision::SkipNullCompensation => {
                    branch.status = BranchStatus::Skipped;
                }
                BarrierDecision::SkipDuplicate => {}
            }
        }
        tx.status = TransactionStatus::Aborted;
        self.store.update_transaction(tx.clone()).await?;
        Ok(tx)
    }

    pub async fn list(&self) -> anyhow::Result<Vec<Transaction>> {
        self.store.list_transactions().await
    }

    pub async fn tick_recover_once(&self) -> anyhow::Result<Vec<Transaction>> {
        let mut changed = Vec::new();
        let now = current_millis();
        for tx in self.store.list_transactions().await? {
            if tx.status.is_terminal() {
                continue;
            }
            if is_expired(&tx, now) {
                let next = match tx.kind {
                    TransactionKind::Tcc => self.cancel_tcc(&tx.gid).await?,
                    TransactionKind::Saga => self.abort_saga(&tx.gid).await?,
                };
                changed.push(next);
                continue;
            }
            if !transaction_due(&tx, now) {
                continue;
            }
            let next = match (tx.kind, tx.status) {
                (TransactionKind::Tcc, TransactionStatus::Submitted) => {
                    self.prepare_tcc(&tx.gid).await?
                }
                (TransactionKind::Tcc, TransactionStatus::Prepared) => {
                    self.confirm_tcc(&tx.gid).await?
                }
                (TransactionKind::Tcc, TransactionStatus::Aborting) => {
                    self.cancel_tcc(&tx.gid).await?
                }
                (TransactionKind::Saga, TransactionStatus::Submitted) => {
                    self.start_saga(&tx.gid).await?
                }
                (TransactionKind::Saga, TransactionStatus::Aborting) => {
                    self.abort_saga(&tx.gid).await?
                }
                _ => continue,
            };
            changed.push(next);
        }
        Ok(changed)
    }

    pub async fn tick_recover_once_with_lease(
        &self,
        owner: &str,
        ttl_millis: u64,
    ) -> anyhow::Result<Vec<Transaction>> {
        if !self
            .store
            .try_acquire_recovery_lease("roze-dtm-recovery", owner, ttl_millis)
            .await?
        {
            return Ok(Vec::new());
        }
        self.tick_recover_once().await
    }

    async fn invoke_branch(&self, branch: &mut Branch, url: &str) -> anyhow::Result<()> {
        match self.invoker.invoke(url, &branch.payload).await {
            Ok(()) => Ok(()),
            Err(err) => {
                record_branch_failure(
                    branch,
                    err.to_string(),
                    self.options.retry_backoff_millis,
                    self.options.max_retry_backoff_millis,
                );
                Err(anyhow::anyhow!(
                    "{}",
                    branch.last_error.clone().unwrap_or_default()
                ))
            }
        }
    }

    async fn invoke_url(&self, branch: &mut Branch, url: &str) -> anyhow::Result<()> {
        self.invoke_branch(branch, url).await
    }
}

fn ensure_kind(tx: &Transaction, expected: TransactionKind) -> anyhow::Result<()> {
    if tx.kind != expected {
        anyhow::bail!(
            "transaction {} is {:?}, expected {:?}",
            tx.gid,
            tx.kind,
            expected
        );
    }
    Ok(())
}

fn current_millis() -> u64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(elapsed) => elapsed.as_millis() as u64,
        Err(_) => 0,
    }
}

async fn insert_barrier(
    pool: &SqlitePool,
    key: &str,
    barrier: &BranchBarrier,
) -> anyhow::Result<()> {
    sqlx::query(
        r#"
        INSERT OR IGNORE INTO roze_dtm_barriers
            (barrier_key, gid, branch_id, op, created_at_millis)
        VALUES (?, ?, ?, ?, ?)
        "#,
    )
    .bind(key)
    .bind(&barrier.gid)
    .bind(&barrier.branch_id)
    .bind(&barrier.op)
    .bind(current_millis() as i64)
    .execute(pool)
    .await?;
    Ok(())
}

fn record_branch_failure(
    branch: &mut Branch,
    error: String,
    backoff_millis: u64,
    max_backoff_millis: u64,
) {
    let shift = branch.attempts.saturating_sub(1).min(16);
    let factor = 1_u64 << shift;
    let backoff = backoff_millis
        .saturating_mul(factor)
        .min(max_backoff_millis.max(backoff_millis));
    branch.last_error = Some(error);
    branch.next_retry_millis = Some(current_millis().saturating_add(backoff));
}

fn transaction_due(tx: &Transaction, now: u64) -> bool {
    tx.branches
        .iter()
        .filter(|branch| matches!(branch.status, BranchStatus::Failed | BranchStatus::Running))
        .all(|branch| branch.next_retry_millis.is_none_or(|next| next <= now))
}

fn is_expired(tx: &Transaction, now: u64) -> bool {
    let Some(timeout) = tx.timeout_millis else {
        return false;
    };
    now.saturating_sub(tx.created_at_millis) >= timeout
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        collections::BTreeMap,
        sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        },
    };

    #[derive(Clone)]
    struct FailingOnceInvoker {
        calls: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl BranchInvoker for FailingOnceInvoker {
        async fn invoke(&self, _url: &str, _payload: &serde_json::Value) -> anyhow::Result<()> {
            let calls = self.calls.fetch_add(1, Ordering::SeqCst);
            if calls == 0 {
                anyhow::bail!("temporary failure");
            }
            Ok(())
        }
    }

    #[tokio::test]
    async fn saga_can_submit_and_abort_with_compensation_barriers() {
        let dtm = Dtm::new(InMemoryTransactionStore::new());
        let tx = Transaction::saga(
            "gid-1",
            vec![Branch::saga(
                "b1",
                "http://inventory/reserve",
                "http://inventory/release",
                serde_json::json!({"sku": "A"}),
            )],
        );
        dtm.submit(tx).await.expect("submit");

        let aborted = dtm.abort_saga("gid-1").await.expect("abort");

        assert_eq!(aborted.status, TransactionStatus::Aborted);
        assert_eq!(aborted.branches[0].status, BranchStatus::Skipped);
    }

    #[tokio::test]
    async fn tcc_prepares_and_confirms() {
        let dtm = Dtm::new(InMemoryTransactionStore::new());
        let tx = Transaction::tcc(
            "gid-2",
            vec![Branch::tcc_try(
                "b1",
                "http://account/try",
                "http://account/confirm",
                "http://account/cancel",
                serde_json::json!({"amount": 100}),
            )],
        );
        dtm.submit(tx).await.expect("submit");

        let prepared = dtm.prepare_tcc("gid-2").await.expect("prepare");
        assert_eq!(prepared.status, TransactionStatus::Prepared);
        let confirmed = dtm.confirm_tcc("gid-2").await.expect("confirm");
        assert_eq!(confirmed.status, TransactionStatus::Succeeded);
    }

    #[tokio::test]
    async fn default_transaction_kind_is_tcc() {
        assert_eq!(TransactionKind::default(), TransactionKind::Tcc);

        let dtm = Dtm::new(InMemoryTransactionStore::new());
        let tx = dtm
            .submit_default_tcc(
                "gid-default",
                vec![Branch::tcc_try(
                    "b1",
                    "http://try",
                    "http://confirm",
                    "http://cancel",
                    serde_json::json!({}),
                )],
            )
            .await
            .expect("submit");

        assert_eq!(tx.kind, TransactionKind::Tcc);
    }

    #[tokio::test]
    async fn barrier_skips_null_compensation() {
        let store = InMemoryTransactionStore::new();
        let decision = store
            .barrier(BranchBarrier::new("gid", "branch", "cancel"))
            .await
            .expect("barrier");

        assert_eq!(decision, BarrierDecision::SkipNullCompensation);
    }

    #[tokio::test]
    async fn failed_branch_gets_retry_schedule() {
        let dtm = Dtm::with_options(
            InMemoryTransactionStore::new(),
            FailingOnceInvoker {
                calls: Arc::new(AtomicUsize::new(0)),
            },
            DtmOptions {
                max_attempts: 5,
                retry_backoff_millis: 1,
                max_retry_backoff_millis: 10,
                branch_call_timeout_millis: 5_000,
                transaction_timeout_millis: 60_000,
            },
        );
        let tx = Transaction::tcc(
            "gid-retry",
            vec![Branch::tcc_try(
                "b1",
                "try",
                "confirm",
                "cancel",
                serde_json::json!({}),
            )],
        );
        dtm.submit(tx).await.expect("submit");

        let prepared = dtm.prepare_tcc("gid-retry").await.expect("prepare");

        assert_eq!(prepared.status, TransactionStatus::Trying);
        assert_eq!(prepared.branches[0].status, BranchStatus::Failed);
        assert!(prepared.branches[0].next_retry_millis.is_some());
    }

    #[tokio::test]
    async fn recovery_cancels_expired_tcc() {
        let store = InMemoryTransactionStore::new();
        let dtm = Dtm::with_options(
            store,
            NoopBranchInvoker,
            DtmOptions {
                max_attempts: 5,
                retry_backoff_millis: 1,
                max_retry_backoff_millis: 10,
                branch_call_timeout_millis: 5_000,
                transaction_timeout_millis: 0,
            },
        );
        let mut tx = Transaction::tcc(
            "gid-timeout",
            vec![Branch::tcc_try(
                "b1",
                "try",
                "confirm",
                "cancel",
                serde_json::json!({}),
            )],
        );
        tx.metadata = BTreeMap::new();
        dtm.submit(tx).await.expect("submit");

        let recovered = dtm.tick_recover_once().await.expect("recover");

        assert_eq!(recovered[0].status, TransactionStatus::Aborted);
    }

    #[tokio::test]
    async fn recovery_lease_allows_one_owner_until_expired() {
        let store = InMemoryTransactionStore::new();

        assert!(store
            .try_acquire_recovery_lease("recovery", "worker-a", 10_000)
            .await
            .expect("lease"));
        assert!(!store
            .try_acquire_recovery_lease("recovery", "worker-b", 10_000)
            .await
            .expect("lease"));
        assert!(store
            .try_acquire_recovery_lease("recovery", "worker-a", 10_000)
            .await
            .expect("renew"));
    }

    #[tokio::test]
    async fn sqlite_store_persists_transactions_and_barriers() {
        let store = SqliteTransactionStore::connect("sqlite::memory:")
            .await
            .expect("connect");
        let tx = Transaction::tcc(
            "gid-sqlite",
            vec![Branch::tcc_try(
                "b1",
                "try",
                "confirm",
                "cancel",
                serde_json::json!({}),
            )],
        );

        store.insert_transaction(tx.clone()).await.expect("insert");
        assert_eq!(
            store
                .get_transaction("gid-sqlite")
                .await
                .expect("get")
                .unwrap()
                .gid,
            tx.gid
        );
        assert_eq!(
            store
                .barrier(BranchBarrier::new("gid-sqlite", "b1", "try"))
                .await
                .expect("barrier"),
            BarrierDecision::Execute
        );
        assert_eq!(
            store
                .barrier(BranchBarrier::new("gid-sqlite", "b1", "try"))
                .await
                .expect("barrier"),
            BarrierDecision::SkipDuplicate
        );
    }
}
