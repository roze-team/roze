use std::{path::PathBuf, sync::Arc};

use axum::{
    extract::{Path, Query, State},
    routing::{get, post},
    Json, Router,
};
use roze_dtm::{
    Branch, Dtm, DtmOptions, HttpBranchInvoker, InMemoryTransactionStore, SqliteTransactionStore,
    Transaction, TransactionKind, TransactionStatus, TransactionStore,
};
use roze_health::{HealthCheck, HealthReport, ProbeKind};
use roze_result::ApiResponse;
use serde::{Deserialize, Serialize};
use tokio::time::{self, Duration};

#[derive(Clone)]
struct AppState {
    dtm: Dtm<Arc<dyn TransactionStore>, HttpBranchInvoker>,
}

#[derive(Debug, Clone, Deserialize)]
struct AppConfig {
    #[serde(flatten)]
    service: roze_config::ServiceConfig,
    #[serde(default)]
    dtm: DtmServiceConfig,
}

#[derive(Debug, Clone, Deserialize)]
struct DtmServiceConfig {
    recover_interval_ms: u64,
    recovery_lease_ttl_ms: u64,
    worker_id: String,
    store: DtmStoreConfig,
    max_attempts: u32,
    retry_backoff_ms: u64,
    max_retry_backoff_ms: u64,
    branch_call_timeout_ms: u64,
    transaction_timeout_ms: u64,
}

impl Default for DtmServiceConfig {
    fn default() -> Self {
        Self {
            recover_interval_ms: 1_000,
            recovery_lease_ttl_ms: 5_000,
            worker_id: format!("roze-dtm-{}", std::process::id()),
            store: DtmStoreConfig::default(),
            max_attempts: 5,
            retry_backoff_ms: 1_000,
            max_retry_backoff_ms: 30_000,
            branch_call_timeout_ms: 5_000,
            transaction_timeout_ms: 60_000,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum DtmStoreConfig {
    Memory,
    Sqlite { database_url: String },
}

impl Default for DtmStoreConfig {
    fn default() -> Self {
        Self::Memory
    }
}

#[derive(Debug, Deserialize)]
struct SubmitRequest {
    gid: String,
    #[serde(default)]
    branches: Vec<Branch>,
}

#[derive(Debug, Deserialize)]
struct TransactionQuery {
    #[serde(default)]
    gid: Option<String>,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    offset: Option<usize>,
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Debug, Serialize)]
struct TransactionList {
    total: usize,
    offset: usize,
    limit: usize,
    items: Vec<Transaction>,
}

#[derive(Debug, Serialize)]
struct DtmStats {
    total: usize,
    submitted: usize,
    trying: usize,
    prepared: usize,
    succeeding: usize,
    succeeded: usize,
    aborting: usize,
    aborted: usize,
    failed: usize,
    tcc: usize,
    saga: usize,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config: AppConfig = load_config(config_path())?;
    roze_log::init_tracing_with_config(&config.service)?;
    let rest = config
        .service
        .rest
        .clone()
        .ok_or_else(|| anyhow::anyhow!("missing rest config"))?;
    let options = DtmOptions {
        max_attempts: config.dtm.max_attempts,
        retry_backoff_millis: config.dtm.retry_backoff_ms,
        max_retry_backoff_millis: config.dtm.max_retry_backoff_ms,
        branch_call_timeout_millis: config.dtm.branch_call_timeout_ms,
        transaction_timeout_millis: config.dtm.transaction_timeout_ms,
    };
    let store = build_store(&config.dtm.store).await?;
    let invoker =
        HttpBranchInvoker::with_timeout(Duration::from_millis(config.dtm.branch_call_timeout_ms))?;
    let state = AppState {
        dtm: Dtm::with_options(store, invoker, options),
    };
    spawn_recovery_worker(
        state.clone(),
        config.dtm.recover_interval_ms,
        config.dtm.recovery_lease_ttl_ms,
        config.dtm.worker_id.clone(),
    );
    let router = router(state);
    tracing::info!(addr = %rest.addr, "start roze-dtm service");
    roze_http::rest::RestServer::new(rest.addr, router)
        .serve()
        .await?;
    Ok(())
}

async fn build_store(config: &DtmStoreConfig) -> anyhow::Result<Arc<dyn TransactionStore>> {
    match config {
        DtmStoreConfig::Memory => Ok(Arc::new(InMemoryTransactionStore::new())),
        DtmStoreConfig::Sqlite { database_url } => Ok(Arc::new(
            SqliteTransactionStore::connect(database_url).await?,
        )),
    }
}

fn load_config(path: impl AsRef<std::path::Path>) -> anyhow::Result<AppConfig> {
    config::Config::builder()
        .add_source(config::File::from(path.as_ref()))
        .build()?
        .try_deserialize()
        .map_err(Into::into)
}

fn spawn_recovery_worker(state: AppState, interval_ms: u64, lease_ttl_ms: u64, worker_id: String) {
    tokio::spawn(async move {
        let mut interval = time::interval(Duration::from_millis(interval_ms.max(100)));
        loop {
            interval.tick().await;
            match state
                .dtm
                .tick_recover_once_with_lease(&worker_id, lease_ttl_ms)
                .await
            {
                Ok(changed) if !changed.is_empty() => {
                    tracing::info!(
                        event = "dtm.recovery.applied",
                        count = changed.len(),
                        "dtm recovery worker advanced transactions"
                    );
                }
                Ok(_) => {}
                Err(err) => {
                    tracing::warn!(
                        event = "dtm.recovery.failed",
                        error = %err,
                        "dtm recovery worker failed"
                    );
                }
            }
        }
    });
}

fn router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/v1/transactions", get(list_transactions))
        .route("/v1/transactions/{gid}", get(get_transaction))
        .route("/v1/transactions/{gid}/recover", post(recover_transaction))
        .route("/v1/recover", post(recover_once))
        .route("/v1/stats", get(stats))
        .route("/v1/tcc", post(submit_tcc))
        .route("/v1/saga", post(submit_saga))
        .route("/v1/tcc/{gid}/prepare", post(prepare_tcc))
        .route("/v1/tcc/{gid}/confirm", post(confirm_tcc))
        .route("/v1/tcc/{gid}/cancel", post(cancel_tcc))
        .route("/v1/saga/{gid}/start", post(start_saga))
        .route("/v1/saga/{gid}/abort", post(abort_saga))
        .with_state(state)
}

async fn healthz() -> Json<ApiResponse<roze_health::ProbeReport>> {
    let report = HealthReport::new(vec![HealthCheck::healthy("dtm")]);
    Json(ApiResponse::ok(report.probe(ProbeKind::Liveness)))
}

async fn readyz() -> Json<ApiResponse<roze_health::ProbeReport>> {
    let report = HealthReport::new(vec![HealthCheck::healthy("store")]);
    Json(ApiResponse::ok(report.probe(ProbeKind::Readiness)))
}

async fn submit_tcc(
    State(state): State<AppState>,
    Json(req): Json<SubmitRequest>,
) -> Result<Json<ApiResponse<Transaction>>, (axum::http::StatusCode, String)> {
    let tx = state
        .dtm
        .submit_default_tcc(req.gid, req.branches)
        .await
        .map_err(internal_error)?;
    Ok(Json(ApiResponse::ok(tx)))
}

async fn submit_saga(
    State(state): State<AppState>,
    Json(req): Json<SubmitRequest>,
) -> Result<Json<ApiResponse<Transaction>>, (axum::http::StatusCode, String)> {
    let tx = state
        .dtm
        .submit(Transaction::saga(req.gid, req.branches))
        .await
        .map_err(internal_error)?;
    Ok(Json(ApiResponse::ok(tx)))
}

async fn list_transactions(
    State(state): State<AppState>,
    Query(query): Query<TransactionQuery>,
) -> Result<Json<ApiResponse<TransactionList>>, (axum::http::StatusCode, String)> {
    let mut txs = state.dtm.list().await.map_err(internal_error)?;
    txs.retain(|tx| matches_query(tx, &query));
    txs.sort_by_key(|tx| tx.updated_at_millis);
    txs.reverse();

    let total = txs.len();
    let offset = query.offset.unwrap_or(0).min(total);
    let limit = query.limit.unwrap_or(50).clamp(1, 500);
    let items = txs.into_iter().skip(offset).take(limit).collect();
    Ok(Json(ApiResponse::ok(TransactionList {
        total,
        offset,
        limit,
        items,
    })))
}

async fn get_transaction(
    State(state): State<AppState>,
    Path(gid): Path<String>,
) -> Result<Json<ApiResponse<Transaction>>, (axum::http::StatusCode, String)> {
    let tx = state
        .dtm
        .store()
        .get_transaction(&gid)
        .await
        .map_err(internal_error)?
        .ok_or_else(|| not_found(format!("transaction not found: {gid}")))?;
    Ok(Json(ApiResponse::ok(tx)))
}

async fn recover_once(
    State(state): State<AppState>,
) -> Result<Json<ApiResponse<Vec<Transaction>>>, (axum::http::StatusCode, String)> {
    let txs = state
        .dtm
        .tick_recover_once()
        .await
        .map_err(internal_error)?;
    Ok(Json(ApiResponse::ok(txs)))
}

async fn recover_transaction(
    State(state): State<AppState>,
    Path(gid): Path<String>,
) -> Result<Json<ApiResponse<Transaction>>, (axum::http::StatusCode, String)> {
    let tx = state
        .dtm
        .store()
        .get_transaction(&gid)
        .await
        .map_err(internal_error)?
        .ok_or_else(|| not_found(format!("transaction not found: {gid}")))?;
    if tx.status.is_terminal() {
        return Ok(Json(ApiResponse::ok(tx)));
    }

    let recovered = match (tx.kind, tx.status) {
        (TransactionKind::Tcc, TransactionStatus::Submitted | TransactionStatus::Trying) => {
            state.dtm.prepare_tcc(&gid).await
        }
        (TransactionKind::Tcc, TransactionStatus::Prepared | TransactionStatus::Succeeding) => {
            state.dtm.confirm_tcc(&gid).await
        }
        (TransactionKind::Tcc, TransactionStatus::Aborting) => state.dtm.cancel_tcc(&gid).await,
        (TransactionKind::Saga, TransactionStatus::Submitted | TransactionStatus::Succeeding) => {
            state.dtm.start_saga(&gid).await
        }
        (TransactionKind::Saga, TransactionStatus::Trying | TransactionStatus::Prepared) => {
            state.dtm.start_saga(&gid).await
        }
        (TransactionKind::Saga, TransactionStatus::Aborting) => state.dtm.abort_saga(&gid).await,
        (
            _,
            TransactionStatus::Succeeded | TransactionStatus::Aborted | TransactionStatus::Failed,
        ) => Ok(tx),
    }
    .map_err(internal_error)?;
    Ok(Json(ApiResponse::ok(recovered)))
}

async fn stats(
    State(state): State<AppState>,
) -> Result<Json<ApiResponse<DtmStats>>, (axum::http::StatusCode, String)> {
    let txs = state.dtm.list().await.map_err(internal_error)?;
    Ok(Json(ApiResponse::ok(stats_from_transactions(&txs))))
}

async fn prepare_tcc(
    State(state): State<AppState>,
    Path(gid): Path<String>,
) -> Result<Json<ApiResponse<Transaction>>, (axum::http::StatusCode, String)> {
    Ok(Json(ApiResponse::ok(
        state.dtm.prepare_tcc(&gid).await.map_err(internal_error)?,
    )))
}

async fn confirm_tcc(
    State(state): State<AppState>,
    Path(gid): Path<String>,
) -> Result<Json<ApiResponse<Transaction>>, (axum::http::StatusCode, String)> {
    Ok(Json(ApiResponse::ok(
        state.dtm.confirm_tcc(&gid).await.map_err(internal_error)?,
    )))
}

async fn cancel_tcc(
    State(state): State<AppState>,
    Path(gid): Path<String>,
) -> Result<Json<ApiResponse<Transaction>>, (axum::http::StatusCode, String)> {
    Ok(Json(ApiResponse::ok(
        state.dtm.cancel_tcc(&gid).await.map_err(internal_error)?,
    )))
}

async fn start_saga(
    State(state): State<AppState>,
    Path(gid): Path<String>,
) -> Result<Json<ApiResponse<Transaction>>, (axum::http::StatusCode, String)> {
    Ok(Json(ApiResponse::ok(
        state.dtm.start_saga(&gid).await.map_err(internal_error)?,
    )))
}

async fn abort_saga(
    State(state): State<AppState>,
    Path(gid): Path<String>,
) -> Result<Json<ApiResponse<Transaction>>, (axum::http::StatusCode, String)> {
    Ok(Json(ApiResponse::ok(
        state.dtm.abort_saga(&gid).await.map_err(internal_error)?,
    )))
}

fn internal_error(err: anyhow::Error) -> (axum::http::StatusCode, String) {
    (
        axum::http::StatusCode::INTERNAL_SERVER_ERROR,
        err.to_string(),
    )
}

fn not_found(msg: impl Into<String>) -> (axum::http::StatusCode, String) {
    (axum::http::StatusCode::NOT_FOUND, msg.into())
}

fn matches_query(tx: &Transaction, query: &TransactionQuery) -> bool {
    if let Some(gid) = query.gid.as_deref() {
        if !tx.gid.contains(gid) {
            return false;
        }
    }
    if let Some(kind) = query.kind.as_deref() {
        if !matches_kind(tx.kind, kind) {
            return false;
        }
    }
    if let Some(status) = query.status.as_deref() {
        if !matches_status(tx.status, status) {
            return false;
        }
    }
    true
}

fn matches_kind(kind: TransactionKind, expected: &str) -> bool {
    matches!(
        (kind, expected.to_ascii_lowercase().as_str()),
        (TransactionKind::Tcc, "tcc") | (TransactionKind::Saga, "saga")
    )
}

fn matches_status(status: TransactionStatus, expected: &str) -> bool {
    matches!(
        (status, expected.to_ascii_lowercase().as_str()),
        (TransactionStatus::Submitted, "submitted")
            | (TransactionStatus::Trying, "trying")
            | (TransactionStatus::Prepared, "prepared")
            | (TransactionStatus::Succeeding, "succeeding")
            | (TransactionStatus::Succeeded, "succeeded")
            | (TransactionStatus::Aborting, "aborting")
            | (TransactionStatus::Aborted, "aborted")
            | (TransactionStatus::Failed, "failed")
    )
}

fn stats_from_transactions(txs: &[Transaction]) -> DtmStats {
    let mut stats = DtmStats {
        total: txs.len(),
        submitted: 0,
        trying: 0,
        prepared: 0,
        succeeding: 0,
        succeeded: 0,
        aborting: 0,
        aborted: 0,
        failed: 0,
        tcc: 0,
        saga: 0,
    };

    for tx in txs {
        match tx.kind {
            TransactionKind::Tcc => stats.tcc += 1,
            TransactionKind::Saga => stats.saga += 1,
        }
        match tx.status {
            TransactionStatus::Submitted => stats.submitted += 1,
            TransactionStatus::Trying => stats.trying += 1,
            TransactionStatus::Prepared => stats.prepared += 1,
            TransactionStatus::Succeeding => stats.succeeding += 1,
            TransactionStatus::Succeeded => stats.succeeded += 1,
            TransactionStatus::Aborting => stats.aborting += 1,
            TransactionStatus::Aborted => stats.aborted += 1,
            TransactionStatus::Failed => stats.failed += 1,
        }
    }
    stats
}

fn config_path() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let manifest_config = manifest_dir.join("config.yaml");
    if manifest_config.exists() {
        manifest_config
    } else {
        PathBuf::from("config.yaml")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_state() -> AppState {
        AppState {
            dtm: Dtm::with_invoker(
                Arc::new(InMemoryTransactionStore::new()),
                HttpBranchInvoker::new(),
            ),
        }
    }

    #[tokio::test]
    async fn submit_tcc_uses_tcc_by_default() {
        let state = test_state();
        let response = submit_tcc(
            State(state),
            Json(SubmitRequest {
                gid: "gid".into(),
                branches: vec![],
            }),
        )
        .await
        .expect("submit");

        assert_eq!(
            response.0.data.unwrap().kind,
            roze_dtm::TransactionKind::Tcc
        );
    }

    #[tokio::test]
    async fn control_plane_lists_filters_and_counts_transactions() {
        let state = test_state();
        state
            .dtm
            .submit_default_tcc("gid-tcc", vec![])
            .await
            .expect("submit tcc");
        state
            .dtm
            .submit(Transaction::saga("gid-saga", vec![]))
            .await
            .expect("submit saga");

        let list = list_transactions(
            State(state.clone()),
            Query(TransactionQuery {
                gid: Some("gid".into()),
                kind: Some("tcc".into()),
                status: Some("submitted".into()),
                offset: Some(0),
                limit: Some(10),
            }),
        )
        .await
        .expect("list");
        let data = list.0.data.expect("data");
        assert_eq!(data.total, 1);
        assert_eq!(data.items[0].gid, "gid-tcc");

        let stats = stats(State(state))
            .await
            .expect("stats")
            .0
            .data
            .expect("data");
        assert_eq!(stats.total, 2);
        assert_eq!(stats.tcc, 1);
        assert_eq!(stats.saga, 1);
    }

    #[tokio::test]
    async fn control_plane_recovers_single_transaction() {
        let state = test_state();
        state
            .dtm
            .submit_default_tcc("gid-recover", vec![])
            .await
            .expect("submit");

        let recovered = recover_transaction(State(state), Path("gid-recover".into()))
            .await
            .expect("recover")
            .0
            .data
            .expect("data");

        assert_eq!(recovered.status, TransactionStatus::Prepared);
    }
}
