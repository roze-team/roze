use std::{convert::Infallible, sync::Arc, time::Duration};

use anyhow::Context as _;
use http::StatusCode;
use roze_dtm::{
    Dtm, DtmOptions, HttpBranchInvoker, InMemoryTransactionStore, SqliteTransactionStore,
    TransactionStore,
};
use roze_http::rest::{self, IncomingRequest, RestServer};
use serde::Deserialize;

#[derive(Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ApplicationConfig {
    #[serde(default)]
    dtm: DtmConfig,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct DtmConfig {
    #[serde(default)]
    store: StoreConfig,
    #[serde(default = "default_max_attempts")]
    max_attempts: u32,
    #[serde(default = "default_retry_backoff_ms")]
    retry_backoff_ms: u64,
    #[serde(default = "default_max_retry_backoff_ms")]
    max_retry_backoff_ms: u64,
    #[serde(default = "default_branch_call_timeout_ms")]
    branch_call_timeout_ms: u64,
    #[serde(default = "default_transaction_timeout_ms")]
    transaction_timeout_ms: u64,
}

impl Default for DtmConfig {
    fn default() -> Self {
        Self {
            store: StoreConfig::default(),
            max_attempts: default_max_attempts(),
            retry_backoff_ms: default_retry_backoff_ms(),
            max_retry_backoff_ms: default_max_retry_backoff_ms(),
            branch_call_timeout_ms: default_branch_call_timeout_ms(),
            transaction_timeout_ms: default_transaction_timeout_ms(),
        }
    }
}

impl DtmConfig {
    fn validate(&self, production: bool) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.max_attempts > 0,
            "application.dtm.max_attempts must be positive"
        );
        anyhow::ensure!(
            self.retry_backoff_ms > 0,
            "application.dtm.retry_backoff_ms must be positive"
        );
        anyhow::ensure!(
            self.max_retry_backoff_ms >= self.retry_backoff_ms,
            "application.dtm.max_retry_backoff_ms must be at least retry_backoff_ms"
        );
        anyhow::ensure!(
            self.branch_call_timeout_ms > 0,
            "application.dtm.branch_call_timeout_ms must be positive"
        );
        anyhow::ensure!(
            self.transaction_timeout_ms >= self.branch_call_timeout_ms,
            "application.dtm.transaction_timeout_ms must be at least branch_call_timeout_ms"
        );
        match self.store.kind {
            StoreKind::Memory => anyhow::ensure!(
                !production,
                "application.dtm.store.kind=memory is forbidden in production"
            ),
            StoreKind::Sqlite => anyhow::ensure!(
                self.store
                    .database_url
                    .as_deref()
                    .is_some_and(|url| !url.trim().is_empty()),
                "application.dtm.store.database_url is required for sqlite"
            ),
        }
        Ok(())
    }

    fn options(&self) -> DtmOptions {
        DtmOptions {
            max_attempts: self.max_attempts,
            retry_backoff_millis: self.retry_backoff_ms,
            max_retry_backoff_millis: self.max_retry_backoff_ms,
            branch_call_timeout_millis: self.branch_call_timeout_ms,
            transaction_timeout_millis: self.transaction_timeout_ms,
        }
    }
}

#[derive(Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoreConfig {
    #[serde(default)]
    kind: StoreKind,
    #[serde(default)]
    database_url: Option<String>,
}

#[derive(Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum StoreKind {
    #[default]
    Memory,
    Sqlite,
}

type DtmRuntime = Dtm<Arc<dyn TransactionStore>, HttpBranchInvoker>;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let path = roze_config::service_config_path(env!("CARGO_MANIFEST_DIR"));
    let config = roze_config::load_service_with_application::<ApplicationConfig>(&path)?;
    let production = config.profile == roze_config::ServiceProfile::Production;
    config.application.dtm.validate(production)?;
    let rest = config
        .rest
        .as_ref()
        .context("roze-dtm requires rest config")?;
    let _tracing_guard = roze_log::init_tracing_with_config(&config.service)?;

    let store: Arc<dyn TransactionStore> = match config.application.dtm.store.kind {
        StoreKind::Memory => Arc::new(InMemoryTransactionStore::new()),
        StoreKind::Sqlite => Arc::new(
            SqliteTransactionStore::connect(
                config
                    .application
                    .dtm
                    .store
                    .database_url
                    .as_deref()
                    .context("validated sqlite database URL missing")?,
            )
            .await?,
        ),
    };
    let invoker = HttpBranchInvoker::with_timeout(Duration::from_millis(
        config.application.dtm.branch_call_timeout_ms,
    ))?;
    let dtm: Arc<DtmRuntime> = Arc::new(Dtm::with_options(
        store,
        invoker,
        config.application.dtm.options(),
    ));
    let addr = rest.addr;
    tracing::info!(
        event = roze_log::events::SERVICE_CONFIG_LOADED,
        service = %config.name,
        protocol = "http",
        config_path = %path.display(),
        "DTM service configuration loaded"
    );

    let service = tower::service_fn(move |request: IncomingRequest| {
        let dtm = Arc::clone(&dtm);
        async move {
            let response = match request.uri().path() {
                "/healthz" | "/startupz" => rest::api_response(&roze_result::ApiResponse::ok("ok")),
                "/readyz" => match dtm.store().get_transaction("__roze_health__").await {
                    Ok(_) => rest::api_response(&roze_result::ApiResponse::ok("ready")),
                    Err(_) => rest::text_response(StatusCode::SERVICE_UNAVAILABLE, "not ready"),
                },
                _ => rest::text_response(
                    StatusCode::NOT_FOUND,
                    "DTM route not migrated to Roze native HTTP",
                ),
            };
            Ok::<_, Infallible>(response)
        }
    });
    tracing::info!(
        event = roze_log::events::SERVICE_STARTING,
        service = %config.name,
        protocol = "http",
        addr = %addr,
        "DTM service starting"
    );
    RestServer::new(addr, service).serve().await?;
    Ok(())
}

const fn default_max_attempts() -> u32 {
    5
}
const fn default_retry_backoff_ms() -> u64 {
    1_000
}
const fn default_max_retry_backoff_ms() -> u64 {
    30_000
}
const fn default_branch_call_timeout_ms() -> u64 {
    5_000
}
const fn default_transaction_timeout_ms() -> u64 {
    60_000
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn production_rejects_memory_store() {
        assert!(DtmConfig::default().validate(true).is_err());
    }

    #[test]
    fn sqlite_requires_database_url() {
        let mut config = DtmConfig::default();
        config.store.kind = StoreKind::Sqlite;
        assert!(config.validate(false).is_err());
        config.store.database_url = Some("sqlite://roze-dtm.db?mode=rwc".to_string());
        assert!(config.validate(true).is_ok());
    }

    #[test]
    fn checked_in_development_config_is_typed_and_valid() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("config.yaml");
        let config = roze_config::load_service_with_application::<ApplicationConfig>(&path)
            .expect("load checked-in DTM config");
        assert_eq!(config.profile, roze_config::ServiceProfile::Development);
        config
            .application
            .dtm
            .validate(false)
            .expect("validate DTM config");
    }
}
