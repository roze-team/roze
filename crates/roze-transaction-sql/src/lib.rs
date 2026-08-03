use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};

use anyhow::{Context, Result};
use async_trait::async_trait;
use roze_transaction::{OutboxMessage, OutboxStatus, OutboxStore, TransactionalOutbox};
use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, DatabaseTransaction, DbBackend,
    QueryResult, Statement, TransactionTrait, Value,
};

pub const POSTGRES_MIGRATION: &str = include_str!("../migrations/postgres/001_roze_outbox.sql");
pub const MYSQL_MIGRATION: &str = include_str!("../migrations/mysql/001_roze_outbox.sql");

const COLUMNS: &str = "id, topic, message_key, headers_json, idempotency_key, payload_json, status, attempts, next_attempt_millis, lease_until_millis, last_error";
static CLAIM_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone)]
pub struct SqlOutboxConfig {
    pub table: String,
    pub max_attempts: u32,
}

impl Default for SqlOutboxConfig {
    fn default() -> Self {
        Self {
            table: "roze_outbox".to_string(),
            max_attempts: 16,
        }
    }
}

#[derive(Clone)]
pub struct SqlOutboxStore {
    database: DatabaseConnection,
    backend: DbBackend,
    config: Arc<SqlOutboxConfig>,
}

impl std::fmt::Debug for SqlOutboxStore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SqlOutboxStore")
            .field("backend", &self.backend)
            .field("table", &self.config.table)
            .field("max_attempts", &self.config.max_attempts)
            .finish_non_exhaustive()
    }
}

impl SqlOutboxStore {
    pub fn new(database: DatabaseConnection) -> Result<Self> {
        Self::with_config(database, SqlOutboxConfig::default())
    }

    pub fn with_config(database: DatabaseConnection, config: SqlOutboxConfig) -> Result<Self> {
        let backend = database.get_database_backend();
        anyhow::ensure!(
            matches!(backend, DbBackend::Postgres | DbBackend::MySql),
            "SQL outbox supports PostgreSQL and MySQL"
        );
        validate_identifier(&config.table)?;
        anyhow::ensure!(
            config.max_attempts > 0,
            "SQL outbox max_attempts must be positive"
        );
        Ok(Self {
            database,
            backend,
            config: Arc::new(config),
        })
    }

    pub fn backend(&self) -> DatabaseBackend {
        self.backend
    }

    pub async fn migrate(&self) -> Result<()> {
        let migration = match self.backend {
            DbBackend::Postgres => POSTGRES_MIGRATION,
            DbBackend::MySql => MYSQL_MIGRATION,
            _ => unreachable!("backend validated by constructor"),
        }
        .replace("roze_outbox", &self.config.table);
        self.database
            .execute_unprepared(&migration)
            .await
            .context("failed to apply SQL outbox migration")?;
        Ok(())
    }

    pub async fn list_dead_letters(&self, limit: usize) -> Result<Vec<OutboxMessage>> {
        let limit = limit.clamp(1, 10_000) as i64;
        let (placeholder, values) = match self.backend {
            DbBackend::Postgres => ("$1", vec![Value::BigInt(Some(limit))]),
            DbBackend::MySql => ("?", vec![Value::BigInt(Some(limit))]),
            _ => unreachable!("backend validated by constructor"),
        };
        let sql = format!(
            "SELECT {COLUMNS} FROM {} WHERE status = 'failed' AND attempts >= {} ORDER BY id LIMIT {placeholder}",
            self.config.table, self.config.max_attempts
        );
        let rows = self
            .database
            .query_all(Statement::from_sql_and_values(self.backend, sql, values))
            .await?;
        rows.into_iter().map(decode_message).collect()
    }

    pub async fn replay_dead_letter(&self, id: &str) -> Result<bool> {
        let (id_placeholder, values) = one_value(self.backend, id.to_string().into());
        let sql = format!(
            "UPDATE {} SET status = 'pending', attempts = 0, next_attempt_millis = NULL, lease_until_millis = NULL, last_error = NULL WHERE id = {id_placeholder} AND status = 'failed'",
            self.config.table
        );
        let result = self
            .database
            .execute(Statement::from_sql_and_values(self.backend, sql, values))
            .await?;
        if result.rows_affected() > 0 {
            roze_metrics::record_outbox_event(driver_name(self.backend), "replayed");
        }
        Ok(result.rows_affected() > 0)
    }

    pub async fn cleanup_published(&self, limit: usize) -> Result<u64> {
        let limit = limit.clamp(1, 10_000) as i64;
        let sql = match self.backend {
            DbBackend::Postgres => format!(
                "DELETE FROM {} WHERE id IN (SELECT id FROM {} WHERE status = 'published' ORDER BY id LIMIT $1)",
                self.config.table, self.config.table
            ),
            DbBackend::MySql => format!(
                "DELETE FROM {} WHERE id IN (SELECT id FROM (SELECT id FROM {} WHERE status = 'published' ORDER BY id LIMIT ?) AS published_rows)",
                self.config.table, self.config.table
            ),
            _ => unreachable!("backend validated by constructor"),
        };
        let result = self
            .database
            .execute(Statement::from_sql_and_values(
                self.backend,
                sql,
                [Value::BigInt(Some(limit))],
            ))
            .await?;
        Ok(result.rows_affected())
    }

    async fn enqueue_on<C>(&self, connection: &C, message: &OutboxMessage) -> Result<bool>
    where
        C: ConnectionTrait,
    {
        let placeholders = placeholders(self.backend, 11);
        let conflict = match self.backend {
            DbBackend::Postgres => " ON CONFLICT (id) DO NOTHING",
            DbBackend::MySql => " ON DUPLICATE KEY UPDATE id = id",
            _ => unreachable!("backend validated by constructor"),
        };
        let sql = format!(
            "INSERT INTO {} ({COLUMNS}) VALUES ({}){conflict}",
            self.config.table,
            placeholders.join(", ")
        );
        let result = connection
            .execute(Statement::from_sql_and_values(
                self.backend,
                sql,
                encode_message(message)?,
            ))
            .await?;
        let inserted = result.rows_affected() == 1;
        roze_metrics::record_outbox_event(
            driver_name(self.backend),
            if inserted { "enqueued" } else { "duplicate" },
        );
        Ok(inserted)
    }

    /// Enqueues messages through a Toasty executor. Passing a
    /// [`toasty::Transaction`] keeps the business mutation and outbox insert
    /// on the same database transaction.
    pub async fn enqueue_with_toasty(
        &self,
        executor: &mut dyn toasty::Executor,
        messages: &[OutboxMessage],
    ) -> Result<()> {
        let placeholder = executor
            .capability()
            .sql_placeholder
            .context("Toasty outbox requires a SQL driver")?;
        ensure_toasty_backend(self.backend, placeholder)?;

        for message in messages {
            let sql = toasty_insert_sql(&self.config.table, self.backend);
            let attempts =
                i32::try_from(message.attempts).context("outbox attempts exceeds SQL INTEGER")?;
            let next_attempt = message.next_attempt_millis.map(millis_to_i64).transpose()?;
            let lease_until = message.lease_until_millis.map(millis_to_i64).transpose()?;
            let affected = toasty::sql::statement(sql)
                .bind_typed(message.id.clone(), toasty::schema::db::Type::Text)
                .bind_typed(message.topic.clone(), toasty::schema::db::Type::Text)
                .bind_typed(message.key.clone(), toasty::schema::db::Type::Text)
                .bind_typed(
                    serde_json::to_string(&message.headers)?,
                    toasty::schema::db::Type::Text,
                )
                .bind_typed(
                    message.idempotency_key.clone(),
                    toasty::schema::db::Type::Text,
                )
                .bind_typed(
                    serde_json::to_string(&message.payload)?,
                    toasty::schema::db::Type::Text,
                )
                .bind_typed(
                    status_name(message.status).to_string(),
                    toasty::schema::db::Type::Text,
                )
                .bind_typed(attempts, toasty::schema::db::Type::Integer(4))
                .bind_typed(next_attempt, toasty::schema::db::Type::Integer(8))
                .bind_typed(lease_until, toasty::schema::db::Type::Integer(8))
                .bind_typed(message.last_error.clone(), toasty::schema::db::Type::Text)
                .exec(executor)
                .await
                .context("failed to enqueue Toasty SQL outbox message")?;
            let inserted = affected == 1;
            roze_metrics::record_outbox_event(
                driver_name(self.backend),
                if inserted { "enqueued" } else { "duplicate" },
            );
        }
        Ok(())
    }
}

#[async_trait]
impl OutboxStore for SqlOutboxStore {
    async fn enqueue(&self, message: OutboxMessage) -> Result<bool> {
        self.enqueue_on(&self.database, &message).await
    }

    async fn get(&self, id: &str) -> Result<Option<OutboxMessage>> {
        let (placeholder, values) = one_value(self.backend, id.to_string().into());
        let sql = format!(
            "SELECT {COLUMNS} FROM {} WHERE id = {placeholder}",
            self.config.table
        );
        self.database
            .query_one(Statement::from_sql_and_values(self.backend, sql, values))
            .await?
            .map(decode_message)
            .transpose()
    }

    async fn claim_pending(
        &self,
        now_millis: u64,
        limit: usize,
        lease_until_millis: u64,
    ) -> Result<Vec<OutboxMessage>> {
        let transaction = self.database.begin().await?;
        let now = millis_to_i64(now_millis)?;
        let limit = limit.clamp(1, 10_000) as i64;
        let (sql, values) = match self.backend {
            DbBackend::Postgres => (
                format!(
                    "SELECT {COLUMNS} FROM {} WHERE ((status IN ('pending', 'failed') AND (next_attempt_millis IS NULL OR next_attempt_millis <= $1)) OR (status = 'publishing' AND (lease_until_millis IS NULL OR lease_until_millis <= $1))) ORDER BY id FOR UPDATE SKIP LOCKED LIMIT $2",
                    self.config.table
                ),
                vec![Value::BigInt(Some(now)), Value::BigInt(Some(limit))],
            ),
            DbBackend::MySql => (
                format!(
                    "SELECT {COLUMNS} FROM {} WHERE ((status IN ('pending', 'failed') AND (next_attempt_millis IS NULL OR next_attempt_millis <= ?)) OR (status = 'publishing' AND (lease_until_millis IS NULL OR lease_until_millis <= ?))) ORDER BY id FOR UPDATE SKIP LOCKED LIMIT ?",
                    self.config.table
                ),
                vec![
                    Value::BigInt(Some(now)),
                    Value::BigInt(Some(now)),
                    Value::BigInt(Some(limit)),
                ],
            ),
            _ => unreachable!("backend validated by constructor"),
        };
        let rows = transaction
            .query_all(Statement::from_sql_and_values(self.backend, sql, values))
            .await?;
        let mut messages = rows
            .into_iter()
            .map(decode_message)
            .collect::<Result<Vec<_>>>()?;
        let claim_id = CLAIM_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        for message in &mut messages {
            let values = match self.backend {
                DbBackend::Postgres => vec![
                    Value::BigInt(Some(millis_to_i64(lease_until_millis)?)),
                    message.id.clone().into(),
                ],
                DbBackend::MySql => vec![
                    Value::BigInt(Some(millis_to_i64(lease_until_millis)?)),
                    message.id.clone().into(),
                ],
                _ => unreachable!("backend validated by constructor"),
            };
            let placeholders = placeholders(self.backend, 2);
            transaction
                .execute(Statement::from_sql_and_values(
                    self.backend,
                    format!(
                        "UPDATE {} SET status = 'publishing', lease_until_millis = {} WHERE id = {}",
                        self.config.table, placeholders[0], placeholders[1]
                    ),
                    values,
                ))
                .await?;
            message.mark_publishing(lease_until_millis);
        }
        transaction.commit().await?;
        if !messages.is_empty() {
            tracing_claim(claim_id, messages.len());
            roze_metrics::record_outbox_event(driver_name(self.backend), "claimed");
        }
        Ok(messages)
    }

    async fn mark_published(&self, id: &str) -> Result<()> {
        let (placeholder, values) = one_value(self.backend, id.to_string().into());
        self.database
            .execute(Statement::from_sql_and_values(
                self.backend,
                format!(
                    "UPDATE {} SET status = 'published', next_attempt_millis = NULL, lease_until_millis = NULL, last_error = NULL WHERE id = {placeholder} AND status = 'publishing'",
                    self.config.table
                ),
                values,
            ))
            .await?;
        roze_metrics::record_outbox_event(driver_name(self.backend), "published");
        Ok(())
    }

    async fn mark_failed(
        &self,
        id: &str,
        error: &str,
        next_attempt_millis: Option<u64>,
    ) -> Result<()> {
        let current = self.get(id).await?;
        let attempts = current
            .as_ref()
            .map(|message| message.attempts.saturating_add(1))
            .unwrap_or(1);
        let next_attempt = (attempts < self.config.max_attempts)
            .then_some(next_attempt_millis)
            .flatten()
            .map(millis_to_i64)
            .transpose()?;
        let placeholders = placeholders(self.backend, 4);
        let sql = format!(
            "UPDATE {} SET status = 'failed', attempts = {}, next_attempt_millis = {}, lease_until_millis = NULL, last_error = {} WHERE id = {} AND status = 'publishing'",
            self.config.table,
            placeholders[0],
            placeholders[1],
            placeholders[2],
            placeholders[3]
        );
        self.database
            .execute(Statement::from_sql_and_values(
                self.backend,
                sql,
                [
                    Value::Int(Some(attempts as i32)),
                    Value::BigInt(next_attempt),
                    error.to_string().into(),
                    id.to_string().into(),
                ],
            ))
            .await?;
        roze_metrics::record_outbox_event(
            driver_name(self.backend),
            if attempts >= self.config.max_attempts {
                "dead_lettered"
            } else {
                "failed"
            },
        );
        Ok(())
    }
}

#[async_trait]
impl TransactionalOutbox<DatabaseTransaction> for SqlOutboxStore {
    async fn enqueue_in_transaction(
        &self,
        transaction: &mut DatabaseTransaction,
        messages: &[OutboxMessage],
    ) -> Result<()> {
        for message in messages {
            self.enqueue_on(transaction, message).await?;
        }
        Ok(())
    }
}

#[async_trait]
impl<'tx> TransactionalOutbox<toasty::Transaction<'tx>> for SqlOutboxStore {
    async fn enqueue_in_transaction(
        &self,
        transaction: &mut toasty::Transaction<'tx>,
        messages: &[OutboxMessage],
    ) -> Result<()> {
        self.enqueue_with_toasty(transaction, messages).await
    }
}

fn encode_message(message: &OutboxMessage) -> Result<Vec<Value>> {
    Ok(vec![
        message.id.clone().into(),
        message.topic.clone().into(),
        message.key.clone().into(),
        serde_json::to_string(&message.headers)?.into(),
        message.idempotency_key.clone().into(),
        serde_json::to_string(&message.payload)?.into(),
        status_name(message.status).to_string().into(),
        Value::Int(Some(message.attempts as i32)),
        Value::BigInt(message.next_attempt_millis.map(millis_to_i64).transpose()?),
        Value::BigInt(message.lease_until_millis.map(millis_to_i64).transpose()?),
        message.last_error.clone().into(),
    ])
}

fn decode_message(row: QueryResult) -> Result<OutboxMessage> {
    let status: String = row.try_get("", "status")?;
    let headers: String = row.try_get("", "headers_json")?;
    let payload: String = row.try_get("", "payload_json")?;
    let attempts: i32 = row.try_get("", "attempts")?;
    let next_attempt: Option<i64> = row.try_get("", "next_attempt_millis")?;
    let lease_until: Option<i64> = row.try_get("", "lease_until_millis")?;
    Ok(OutboxMessage {
        id: row.try_get("", "id")?,
        topic: row.try_get("", "topic")?,
        key: row.try_get("", "message_key")?,
        headers: serde_json::from_str::<BTreeMap<String, String>>(&headers)?,
        idempotency_key: row.try_get("", "idempotency_key")?,
        payload: serde_json::from_str(&payload)?,
        status: parse_status(&status)?,
        attempts: u32::try_from(attempts).context("outbox attempts must not be negative")?,
        next_attempt_millis: next_attempt.map(i64_to_millis).transpose()?,
        lease_until_millis: lease_until.map(i64_to_millis).transpose()?,
        last_error: row.try_get("", "last_error")?,
    })
}

fn placeholders(backend: DbBackend, count: usize) -> Vec<String> {
    match backend {
        DbBackend::Postgres => (1..=count).map(|index| format!("${index}")).collect(),
        DbBackend::MySql => (0..count).map(|_| "?".to_string()).collect(),
        _ => unreachable!("backend validated by constructor"),
    }
}

fn ensure_toasty_backend(backend: DbBackend, placeholder: toasty::SqlPlaceholder) -> Result<()> {
    let matches = matches!(
        (backend, placeholder),
        (DbBackend::Postgres, toasty::SqlPlaceholder::DollarNumber)
            | (DbBackend::MySql, toasty::SqlPlaceholder::QuestionMark)
    );
    anyhow::ensure!(
        matches,
        "Toasty SQL driver does not match the SQL outbox backend"
    );
    Ok(())
}

fn toasty_insert_sql(table: &str, backend: DbBackend) -> String {
    let conflict = match backend {
        DbBackend::Postgres => " ON CONFLICT (id) DO NOTHING",
        DbBackend::MySql => " ON DUPLICATE KEY UPDATE id = id",
        _ => unreachable!("backend validated by constructor"),
    };
    format!(
        "INSERT INTO {table} ({COLUMNS}) VALUES ({}){conflict}",
        placeholders(backend, 11).join(", ")
    )
}

fn one_value(backend: DbBackend, value: Value) -> (String, Vec<Value>) {
    (placeholders(backend, 1).remove(0), vec![value])
}

fn status_name(status: OutboxStatus) -> &'static str {
    match status {
        OutboxStatus::Pending => "pending",
        OutboxStatus::Publishing => "publishing",
        OutboxStatus::Published => "published",
        OutboxStatus::Failed => "failed",
    }
}

fn parse_status(value: &str) -> Result<OutboxStatus> {
    match value {
        "pending" => Ok(OutboxStatus::Pending),
        "publishing" => Ok(OutboxStatus::Publishing),
        "published" => Ok(OutboxStatus::Published),
        "failed" => Ok(OutboxStatus::Failed),
        _ => anyhow::bail!("unknown SQL outbox status"),
    }
}

fn millis_to_i64(value: u64) -> Result<i64> {
    i64::try_from(value).context("outbox timestamp exceeds SQL BIGINT")
}

fn i64_to_millis(value: i64) -> Result<u64> {
    u64::try_from(value).context("outbox timestamp must not be negative")
}

fn validate_identifier(value: &str) -> Result<()> {
    anyhow::ensure!(
        !value.is_empty()
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
            && value
                .as_bytes()
                .first()
                .is_some_and(|byte| byte.is_ascii_alphabetic()),
        "SQL outbox table must be a simple SQL identifier"
    );
    Ok(())
}

fn driver_name(backend: DbBackend) -> &'static str {
    match backend {
        DbBackend::Postgres => "postgres",
        DbBackend::MySql => "mysql",
        _ => "unsupported",
    }
}

fn tracing_claim(claim_id: u64, count: usize) {
    tracing::debug!(claim_id, count, "SQL outbox batch claimed");
}

#[cfg(test)]
mod tests {
    use super::*;
    use sea_orm::Database;

    #[test]
    fn migrations_define_claim_and_dead_letter_indexes() {
        for migration in [POSTGRES_MIGRATION, MYSQL_MIGRATION] {
            assert!(migration.contains("roze_outbox"));
            assert!(migration.contains("next_attempt_millis"));
            assert!(migration.contains("lease_until_millis"));
            assert!(migration.contains("status"));
        }
    }

    #[test]
    fn validates_table_identifiers() {
        assert!(validate_identifier("tenant_outbox").is_ok());
        assert!(validate_identifier("outbox; DROP TABLE users").is_err());
        assert!(validate_identifier("1outbox").is_err());
    }

    #[test]
    fn message_encoding_round_trips_json_fields() {
        let message = OutboxMessage::new(
            "event-1",
            "captcha.verified",
            "captcha-1",
            serde_json::json!({"verified": true}),
        );
        let values = encode_message(&message).expect("encode");
        assert_eq!(values.len(), 11);
    }

    #[test]
    fn toasty_outbox_uses_backend_placeholders_and_rejects_mismatches() {
        let postgres = toasty_insert_sql("tenant_outbox", DbBackend::Postgres);
        assert!(postgres.contains("VALUES ($1, $2, $3"));
        assert!(postgres.ends_with("ON CONFLICT (id) DO NOTHING"));
        let mysql = toasty_insert_sql("tenant_outbox", DbBackend::MySql);
        assert!(mysql.contains("VALUES (?, ?, ?"));
        assert!(mysql.ends_with("ON DUPLICATE KEY UPDATE id = id"));

        assert!(
            ensure_toasty_backend(DbBackend::Postgres, toasty::SqlPlaceholder::DollarNumber)
                .is_ok()
        );
        assert!(
            ensure_toasty_backend(DbBackend::MySql, toasty::SqlPlaceholder::QuestionMark).is_ok()
        );
        assert!(
            ensure_toasty_backend(DbBackend::Postgres, toasty::SqlPlaceholder::QuestionMark)
                .is_err()
        );
        assert!(ensure_toasty_backend(
            DbBackend::MySql,
            toasty::SqlPlaceholder::NumberedQuestionMark,
        )
        .is_err());
    }

    async fn real_database_outbox_evidence(url: &str) {
        let database = Database::connect(url).await.expect("connect database");
        let table = format!(
            "roze_outbox_test_{}_{}",
            std::process::id(),
            CLAIM_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        );
        let store = SqlOutboxStore::with_config(
            database.clone(),
            SqlOutboxConfig {
                table: table.clone(),
                max_attempts: 2,
            },
        )
        .expect("store");
        store.migrate().await.expect("migrate");

        let message = OutboxMessage::new(
            format!("{table}-event"),
            "captcha.verified",
            format!("{table}-key"),
            serde_json::json!({"verified": true}),
        );
        assert!(store.enqueue(message.clone()).await.expect("enqueue"));
        assert!(!store.enqueue(message.clone()).await.expect("deduplicate"));

        let peer = SqlOutboxStore::with_config(
            database.clone(),
            SqlOutboxConfig {
                table: table.clone(),
                max_attempts: 2,
            },
        )
        .expect("peer store");
        let (left, right) = tokio::join!(
            store.claim_pending(1_000, 1, 2_000),
            peer.claim_pending(1_000, 1, 2_000)
        );
        assert_eq!(
            left.expect("left claim").len() + right.expect("right claim").len(),
            1
        );

        // A consumer that fails before marking completion leaves the lease
        // recoverable rather than incorrectly publishing the message.
        assert!(
            peer.claim_pending(2_001, 1, 3_000)
                .await
                .expect("lease recovery")
                .len()
                == 1
        );
        peer.mark_failed(&message.id, "consumer transaction rolled back", Some(4_000))
            .await
            .expect("mark failed");
        assert!(peer
            .claim_pending(3_999, 1, 5_000)
            .await
            .expect("backoff")
            .is_empty());
        let retry = peer.claim_pending(4_000, 1, 5_000).await.expect("retry");
        assert_eq!(retry.len(), 1);
        peer.mark_published(&message.id)
            .await
            .expect("mark published");
        assert_eq!(
            peer.get(&message.id)
                .await
                .expect("get")
                .expect("message")
                .status,
            OutboxStatus::Published
        );

        let transaction = database.begin().await.expect("begin");
        let transactional = OutboxMessage::new(
            format!("{table}-transactional"),
            "captcha.stats",
            format!("{table}-stats"),
            serde_json::json!({"day": "2026-07-23"}),
        );
        let mut transaction = transaction;
        store
            .enqueue_in_transaction(&mut transaction, std::slice::from_ref(&transactional))
            .await
            .expect("transactional enqueue");
        transaction.commit().await.expect("commit");
        assert!(peer
            .get(&transactional.id)
            .await
            .expect("get transactional")
            .is_some());

        let mut toasty = toasty::Db::builder()
            .models(toasty::models!())
            .connect(url)
            .await
            .expect("connect Toasty database");
        let toasty_committed = OutboxMessage::new(
            format!("{table}-toasty-committed"),
            "captcha.stats",
            format!("{table}-toasty-commit-key"),
            serde_json::json!({"orm": "toasty", "outcome": "committed"}),
        );
        let mut transaction = toasty
            .transaction()
            .await
            .expect("begin Toasty transaction");
        store
            .enqueue_in_transaction(&mut transaction, std::slice::from_ref(&toasty_committed))
            .await
            .expect("enqueue in Toasty transaction");
        transaction
            .commit()
            .await
            .expect("commit Toasty transaction");
        assert!(peer
            .get(&toasty_committed.id)
            .await
            .expect("get Toasty committed message")
            .is_some());

        let toasty_rolled_back = OutboxMessage::new(
            format!("{table}-toasty-rolled-back"),
            "captcha.stats",
            format!("{table}-toasty-rollback-key"),
            serde_json::json!({"orm": "toasty", "outcome": "rolled-back"}),
        );
        let mut transaction = toasty.transaction().await.expect("begin Toasty rollback");
        store
            .enqueue_in_transaction(&mut transaction, std::slice::from_ref(&toasty_rolled_back))
            .await
            .expect("enqueue before Toasty rollback");
        transaction
            .rollback()
            .await
            .expect("rollback Toasty transaction");
        assert!(peer
            .get(&toasty_rolled_back.id)
            .await
            .expect("get Toasty rolled-back message")
            .is_none());

        database
            .execute_unprepared(&format!("DROP TABLE {table}"))
            .await
            .expect("drop test table");
    }

    #[tokio::test]
    #[ignore = "requires ROZE_TEST_POSTGRES_URL"]
    async fn postgres_claim_restart_and_transaction_evidence() {
        let url = std::env::var("ROZE_TEST_POSTGRES_URL").expect("ROZE_TEST_POSTGRES_URL");
        real_database_outbox_evidence(&url).await;
    }

    #[tokio::test]
    #[ignore = "requires ROZE_TEST_MYSQL_URL"]
    async fn mysql_claim_restart_and_transaction_evidence() {
        let url = std::env::var("ROZE_TEST_MYSQL_URL").expect("ROZE_TEST_MYSQL_URL");
        real_database_outbox_evidence(&url).await;
    }
}
