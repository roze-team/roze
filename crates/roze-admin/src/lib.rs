use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Arc, Mutex},
};

use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{header, Request, StatusCode},
    middleware::{from_fn_with_state, Next},
    response::IntoResponse,
    routing::{delete, get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ServiceInstanceView {
    pub name: String,
    pub addr: String,
    pub weight: u32,
    pub metadata: BTreeMap<String, String>,
}

impl From<roze_rpc::registry::ServiceInstance> for ServiceInstanceView {
    fn from(instance: roze_rpc::registry::ServiceInstance) -> Self {
        Self {
            name: instance.name,
            addr: instance.addr,
            weight: instance.weight,
            metadata: instance.metadata.into_iter().collect(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RegistryServiceSnapshot {
    pub service: String,
    pub instances: Vec<ServiceInstanceView>,
}

#[derive(Clone)]
pub struct RegistryAdmin {
    registry: Arc<dyn roze_rpc::registry::Registry>,
}

impl RegistryAdmin {
    pub fn new(registry: Arc<dyn roze_rpc::registry::Registry>) -> Self {
        Self { registry }
    }

    pub async fn service(&self, name: &str) -> anyhow::Result<RegistryServiceSnapshot> {
        let instances = self
            .registry
            .discover(name)
            .await?
            .into_iter()
            .map(ServiceInstanceView::from)
            .collect();
        Ok(RegistryServiceSnapshot {
            service: name.to_string(),
            instances,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConfigReloadAuditRecord {
    pub version: u64,
    pub old_version: u64,
    pub hash: String,
    pub old_hash: String,
    pub ts_millis: u64,
    pub source: String,
    pub namespace: Option<String>,
    pub app: Option<String>,
    pub key: Option<String>,
    pub changed: bool,
    pub success: bool,
    pub error: Option<String>,
    pub diff: Vec<roze_config::ConfigDiffEntry>,
    pub section_signatures: Vec<roze_config::ConfigSectionSignature>,
}

impl ConfigReloadAuditRecord {
    pub fn from_reload_result<T>(result: &roze_config::ReloadResult<T>) -> Self
    where
        T: Clone,
    {
        Self {
            version: result.version,
            old_version: result.old_version,
            hash: result.hash.clone(),
            old_hash: result.old_hash.clone(),
            ts_millis: result.ts_millis,
            source: result.source.clone(),
            namespace: result.namespace.clone(),
            app: result.app.clone(),
            key: result.key.clone(),
            changed: result.changed,
            success: result.success,
            error: result.error.clone(),
            diff: result.diff.clone(),
            section_signatures: result.section_signatures.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ConfigReloadHistory {
    capacity: usize,
    records: Arc<Mutex<VecDeque<ConfigReloadAuditRecord>>>,
}

impl ConfigReloadHistory {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            records: Arc::new(Mutex::new(VecDeque::new())),
        }
    }

    pub fn record<T>(&self, result: &roze_config::ReloadResult<T>)
    where
        T: Clone,
    {
        self.push(ConfigReloadAuditRecord::from_reload_result(result));
    }

    pub fn push(&self, record: ConfigReloadAuditRecord) {
        let mut records = self.records.lock().expect("config history lock poisoned");
        records.push_back(record);
        while records.len() > self.capacity {
            records.pop_front();
        }
    }

    pub fn list(&self, offset: usize, limit: usize) -> Vec<ConfigReloadAuditRecord> {
        self.records
            .lock()
            .expect("config history lock poisoned")
            .iter()
            .rev()
            .skip(offset)
            .take(limit.clamp(1, 500))
            .cloned()
            .collect()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MqAdminSnapshot {
    pub stats: roze_mq::MqStats,
    pub dead_letters: Vec<roze_mq::DeadLetterRecord>,
}

pub struct MqAdminView<A> {
    admin: A,
}

impl<A> MqAdminView<A>
where
    A: roze_mq::MqAdmin,
{
    pub fn new(admin: A) -> Self {
        Self { admin }
    }

    pub async fn snapshot(
        &self,
        query: roze_mq::DeadLetterQuery,
    ) -> anyhow::Result<MqAdminSnapshot> {
        Ok(MqAdminSnapshot {
            stats: self.admin.stats().await?,
            dead_letters: self.admin.dead_letters_query(query).await?,
        })
    }

    pub async fn replay_dead_letter(&self, id: u64) -> anyhow::Result<Option<roze_mq::Message>> {
        self.admin.replay_dead_letter(id).await
    }

    pub async fn purge_dead_letter(
        &self,
        id: u64,
    ) -> anyhow::Result<Option<roze_mq::DeadLetterRecord>> {
        self.admin.purge_dead_letter(id).await
    }
}

#[derive(Clone, Default)]
pub struct AdminState {
    pub registry: Option<RegistryAdmin>,
    pub config_history: Option<ConfigReloadHistory>,
    pub mq: Option<Arc<dyn roze_mq::MqAdmin>>,
    pub auth: Option<AdminAuthConfig>,
}

impl AdminState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_registry(mut self, registry: RegistryAdmin) -> Self {
        self.registry = Some(registry);
        self
    }

    pub fn with_config_history(mut self, history: ConfigReloadHistory) -> Self {
        self.config_history = Some(history);
        self
    }

    pub fn with_mq(mut self, mq: Arc<dyn roze_mq::MqAdmin>) -> Self {
        self.mq = Some(mq);
        self
    }

    pub fn with_auth(mut self, auth: AdminAuthConfig) -> Self {
        self.auth = Some(auth);
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdminAuthConfig {
    #[serde(default)]
    pub bearer_token: Option<String>,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default = "default_api_key_header")]
    pub api_key_header: String,
}

impl AdminAuthConfig {
    pub fn bearer(token: impl Into<String>) -> Self {
        Self {
            bearer_token: Some(token.into()),
            api_key: None,
            api_key_header: default_api_key_header(),
        }
    }

    pub fn api_key(key: impl Into<String>) -> Self {
        Self {
            bearer_token: None,
            api_key: Some(key.into()),
            api_key_header: default_api_key_header(),
        }
    }

    pub fn from_env() -> Option<Self> {
        let bearer_token = std::env::var("ROZE_ADMIN_TOKEN")
            .ok()
            .filter(|value| !value.trim().is_empty());
        let api_key = std::env::var("ROZE_ADMIN_API_KEY")
            .ok()
            .filter(|value| !value.trim().is_empty());
        if bearer_token.is_none() && api_key.is_none() {
            return None;
        }
        let api_key_header = std::env::var("ROZE_ADMIN_API_KEY_HEADER")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(default_api_key_header);
        Some(Self {
            bearer_token,
            api_key,
            api_key_header,
        })
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct PageQuery {
    #[serde(default)]
    pub offset: usize,
    #[serde(default = "default_admin_limit")]
    pub limit: usize,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DeadLetterHttpQuery {
    #[serde(default)]
    pub topic: Option<String>,
    #[serde(default)]
    pub group: Option<String>,
    #[serde(default)]
    pub offset: usize,
    #[serde(default = "default_admin_limit")]
    pub limit: usize,
}

pub fn admin_router(state: AdminState) -> Router {
    Router::new()
        .route("/admin/registry/{service}", get(http_registry_service))
        .route("/admin/config/reloads", get(http_config_reloads))
        .route("/admin/mq/stats", get(http_mq_stats))
        .route("/admin/mq/dead-letters", get(http_mq_dead_letters))
        .route(
            "/admin/mq/dead-letters/{id}/replay",
            post(http_mq_replay_dead_letter),
        )
        .route(
            "/admin/mq/dead-letters/{id}",
            delete(http_mq_purge_dead_letter),
        )
        .route_layer(from_fn_with_state(state.clone(), admin_auth_middleware))
        .with_state(state)
}

async fn admin_auth_middleware(
    State(state): State<AdminState>,
    req: Request<Body>,
    next: Next,
) -> axum::response::Response {
    let Some(auth) = state.auth.as_ref() else {
        return next.run(req).await;
    };
    if admin_request_authorized(&req, auth) {
        next.run(req).await
    } else {
        (
            StatusCode::UNAUTHORIZED,
            Json(AdminError {
                error: "admin unauthorized".to_string(),
            }),
        )
            .into_response()
    }
}

fn admin_request_authorized(req: &Request<Body>, auth: &AdminAuthConfig) -> bool {
    if let Some(expected) = auth.bearer_token.as_deref() {
        let authorized = req
            .headers()
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
            .is_some_and(|token| token == expected);
        if authorized {
            return true;
        }
    }

    if let Some(expected) = auth.api_key.as_deref() {
        let Ok(header_name) = auth.api_key_header.parse::<header::HeaderName>() else {
            return false;
        };
        return req
            .headers()
            .get(header_name)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value == expected);
    }

    false
}

async fn http_registry_service(
    State(state): State<AdminState>,
    Path(service): Path<String>,
) -> impl IntoResponse {
    let Some(registry) = state.registry else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match registry.service(&service).await {
        Ok(snapshot) => Json(snapshot).into_response(),
        Err(err) => admin_error(StatusCode::BAD_GATEWAY, err),
    }
}

async fn http_config_reloads(
    State(state): State<AdminState>,
    Query(query): Query<PageQuery>,
) -> impl IntoResponse {
    let Some(history) = state.config_history else {
        return StatusCode::NOT_FOUND.into_response();
    };
    Json(history.list(query.offset, query.limit)).into_response()
}

async fn http_mq_stats(State(state): State<AdminState>) -> impl IntoResponse {
    let Some(mq) = state.mq else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match mq.stats().await {
        Ok(stats) => Json(stats).into_response(),
        Err(err) => admin_error(StatusCode::BAD_GATEWAY, err),
    }
}

async fn http_mq_dead_letters(
    State(state): State<AdminState>,
    Query(query): Query<DeadLetterHttpQuery>,
) -> impl IntoResponse {
    let Some(mq) = state.mq else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let query = roze_mq::DeadLetterQuery {
        topic: query.topic,
        group: query.group,
        offset: query.offset,
        limit: query.limit,
    };
    match mq.dead_letters_query(query).await {
        Ok(records) => Json(records).into_response(),
        Err(err) => admin_error(StatusCode::BAD_GATEWAY, err),
    }
}

async fn http_mq_replay_dead_letter(
    State(state): State<AdminState>,
    Path(id): Path<u64>,
) -> impl IntoResponse {
    let Some(mq) = state.mq else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match mq.replay_dead_letter(id).await {
        Ok(Some(message)) => Json(message).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(err) => admin_error(StatusCode::BAD_GATEWAY, err),
    }
}

async fn http_mq_purge_dead_letter(
    State(state): State<AdminState>,
    Path(id): Path<u64>,
) -> impl IntoResponse {
    let Some(mq) = state.mq else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match mq.purge_dead_letter(id).await {
        Ok(Some(record)) => Json(record).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(err) => admin_error(StatusCode::BAD_GATEWAY, err),
    }
}

fn admin_error(status: StatusCode, err: anyhow::Error) -> axum::response::Response {
    (
        status,
        Json(AdminError {
            error: err.to_string(),
        }),
    )
        .into_response()
}

#[derive(Debug, Serialize)]
struct AdminError {
    error: String,
}

fn default_admin_limit() -> usize {
    100
}

fn default_api_key_header() -> String {
    "x-api-key".to_string()
}

pub fn runtime_name() -> &'static str {
    "roze-admin"
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{to_bytes, Body},
        http::{Method, Request, StatusCode},
    };
    use roze_mq::{MqAdmin, Publisher, Subscriber};
    use roze_rpc::registry::{MemoryRegistry, Registry, ServiceInstance};
    use tower::ServiceExt;

    #[tokio::test]
    async fn registry_admin_lists_service_instances() {
        let registry = Arc::new(MemoryRegistry::default());
        let mut instance = ServiceInstance::new("user", "127.0.0.1:8080");
        instance.weight = 2;
        instance
            .metadata
            .insert("version".to_string(), "v2".to_string());
        registry.register(instance).await.expect("register");

        let admin = RegistryAdmin::new(registry as Arc<dyn Registry>);
        let snapshot = admin.service("user").await.expect("snapshot");

        assert_eq!(snapshot.service, "user");
        assert_eq!(snapshot.instances.len(), 1);
        assert_eq!(snapshot.instances[0].weight, 2);
        assert_eq!(
            snapshot.instances[0]
                .metadata
                .get("version")
                .map(String::as_str),
            Some("v2")
        );
    }

    #[test]
    fn config_reload_history_keeps_latest_records() {
        let history = ConfigReloadHistory::new(2);
        history.push(ConfigReloadAuditRecord {
            version: 1,
            old_version: 0,
            hash: "h1".to_string(),
            old_hash: String::new(),
            ts_millis: 1,
            source: "file".to_string(),
            namespace: None,
            app: None,
            key: None,
            changed: true,
            success: true,
            error: None,
            diff: Vec::new(),
            section_signatures: Vec::new(),
        });
        history.push(ConfigReloadAuditRecord {
            version: 2,
            old_version: 1,
            hash: "h2".to_string(),
            old_hash: "h1".to_string(),
            ts_millis: 2,
            source: "file".to_string(),
            namespace: None,
            app: None,
            key: None,
            changed: true,
            success: true,
            error: None,
            diff: Vec::new(),
            section_signatures: Vec::new(),
        });
        history.push(ConfigReloadAuditRecord {
            version: 3,
            old_version: 2,
            hash: "h3".to_string(),
            old_hash: "h2".to_string(),
            ts_millis: 3,
            source: "file".to_string(),
            namespace: None,
            app: None,
            key: None,
            changed: true,
            success: false,
            error: Some("bad config".to_string()),
            diff: Vec::new(),
            section_signatures: Vec::new(),
        });

        let records = history.list(0, 10);
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].version, 3);
        assert_eq!(records[1].version, 2);
    }

    #[tokio::test]
    async fn mq_admin_view_snapshots_and_replays_dlq() {
        let broker = roze_mq::InMemoryBroker::with_dead_letter("dead", 1);
        let mut orders = broker.subscribe("orders").await.expect("subscribe orders");
        let mut replay = broker.subscribe("orders").await.expect("subscribe replay");
        broker
            .publish(roze_mq::Message::new(
                "orders",
                serde_json::json!({"id": 1}),
            ))
            .await
            .expect("publish");
        orders
            .recv()
            .await
            .expect("delivery")
            .nack()
            .await
            .expect("nack");

        let view = MqAdminView::new(broker.clone());
        let snapshot = view
            .snapshot(roze_mq::DeadLetterQuery {
                topic: Some("orders".to_string()),
                ..Default::default()
            })
            .await
            .expect("snapshot");
        assert_eq!(snapshot.stats.dead_lettered, 1);
        assert_eq!(snapshot.dead_letters.len(), 1);

        let replayed = view
            .replay_dead_letter(snapshot.dead_letters[0].id)
            .await
            .expect("replay")
            .expect("message");
        assert_eq!(replayed.topic, "orders");
        let delivery = replay.recv().await.expect("replayed");
        assert_eq!(delivery.message().payload["id"], 1);

        let purged = view
            .purge_dead_letter(snapshot.dead_letters[0].id)
            .await
            .expect("purge")
            .expect("record");
        assert_eq!(purged.original_topic, "orders");
        assert_eq!(broker.stats().await.expect("stats").dead_letter_pending, 0);
    }

    #[tokio::test]
    async fn admin_router_serves_registry_snapshot() {
        let registry = Arc::new(MemoryRegistry::default());
        registry
            .register(ServiceInstance::new("user", "127.0.0.1:8080"))
            .await
            .expect("register");
        let router = admin_router(
            AdminState::new().with_registry(RegistryAdmin::new(registry as Arc<dyn Registry>)),
        );

        let response = router
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/admin/registry/user")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 1024 * 1024)
            .await
            .expect("body");
        let snapshot: RegistryServiceSnapshot = serde_json::from_slice(&body).expect("snapshot");
        assert_eq!(snapshot.instances.len(), 1);
    }

    #[tokio::test]
    async fn admin_router_rejects_missing_auth_when_configured() {
        let router = admin_router(
            AdminState::new()
                .with_config_history(ConfigReloadHistory::new(10))
                .with_auth(AdminAuthConfig::bearer("secret")),
        );

        let response = router
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/admin/config/reloads")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn admin_router_accepts_bearer_auth() {
        let router = admin_router(
            AdminState::new()
                .with_config_history(ConfigReloadHistory::new(10))
                .with_auth(AdminAuthConfig::bearer("secret")),
        );

        let response = router
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/admin/config/reloads")
                    .header("authorization", "Bearer secret")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn admin_router_accepts_api_key_auth() {
        let router = admin_router(
            AdminState::new()
                .with_config_history(ConfigReloadHistory::new(10))
                .with_auth(AdminAuthConfig::api_key("secret-key")),
        );

        let response = router
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/admin/config/reloads")
                    .header("x-api-key", "secret-key")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn admin_router_serves_mq_dlq_and_replay() {
        let broker = Arc::new(roze_mq::InMemoryBroker::with_dead_letter("dead", 1));
        let mut orders = broker.subscribe("orders").await.expect("subscribe orders");
        let mut replay = broker.subscribe("orders").await.expect("subscribe replay");
        broker
            .publish(roze_mq::Message::new(
                "orders",
                serde_json::json!({"id": 1}),
            ))
            .await
            .expect("publish");
        orders
            .recv()
            .await
            .expect("delivery")
            .nack()
            .await
            .expect("nack");
        let router =
            admin_router(AdminState::new().with_mq(broker.clone() as Arc<dyn roze_mq::MqAdmin>));

        let list_response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/admin/mq/dead-letters?topic=orders")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(list_response.status(), StatusCode::OK);
        let list_body = to_bytes(list_response.into_body(), 1024 * 1024)
            .await
            .expect("body");
        let records: Vec<roze_mq::DeadLetterRecord> =
            serde_json::from_slice(&list_body).expect("records");
        assert_eq!(records.len(), 1);

        let replay_response = router
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/admin/mq/dead-letters/{}/replay", records[0].id))
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(replay_response.status(), StatusCode::OK);
        let delivery = replay.recv().await.expect("replayed");
        assert_eq!(delivery.message().payload["id"], 1);
    }
}
