use std::{
    collections::{BTreeMap, VecDeque},
    convert::Infallible,
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
};

use http::StatusCode;
use roze_http::rest::{self, HttpResponse, IncomingRequest};
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
}

#[derive(Clone, Default)]
pub struct AdminState {
    pub registry: Option<RegistryAdmin>,
    pub config_history: Option<ConfigReloadHistory>,
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

    pub fn with_auth(mut self, auth: AdminAuthConfig) -> Self {
        self.auth = Some(auth);
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdminAuthConfig {
    Bearer { token: String },
    ApiKey { key: String },
}

impl AdminAuthConfig {
    pub fn bearer(token: impl Into<String>) -> Self {
        Self::Bearer {
            token: token.into(),
        }
    }

    pub fn api_key(key: impl Into<String>) -> Self {
        Self::ApiKey { key: key.into() }
    }

    pub fn from_env() -> Option<Self> {
        std::env::var("ROZE_ADMIN_BEARER")
            .ok()
            .map(Self::bearer)
            .or_else(|| std::env::var("ROZE_ADMIN_API_KEY").ok().map(Self::api_key))
    }
}

#[derive(Clone)]
pub struct AdminService {
    state: AdminState,
}

pub fn admin_service(state: AdminState) -> AdminService {
    AdminService { state }
}

impl tower::Service<IncomingRequest> for AdminService {
    type Response = HttpResponse;
    type Error = Infallible;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(
        &mut self,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        std::task::Poll::Ready(Ok(()))
    }

    fn call(&mut self, request: IncomingRequest) -> Self::Future {
        let state = self.state.clone();
        Box::pin(async move { Ok(handle_admin(state, request).await) })
    }
}

async fn handle_admin(state: AdminState, request: IncomingRequest) -> HttpResponse {
    if !admin_request_authorized(&request, state.auth.as_ref()) {
        return rest::text_response(StatusCode::UNAUTHORIZED, "unauthorized");
    }
    match request.uri().path() {
        "/admin/config/reloads" => {
            let records = state
                .config_history
                .map(|history| history.list(0, 100))
                .unwrap_or_default();
            rest::json_response(StatusCode::OK, &records)
        }
        _ => rest::text_response(StatusCode::NOT_FOUND, "admin endpoint not found"),
    }
}

fn admin_request_authorized(req: &IncomingRequest, auth: Option<&AdminAuthConfig>) -> bool {
    let Some(auth) = auth else {
        return true;
    };
    match auth {
        AdminAuthConfig::Bearer { token } => req
            .headers()
            .get(http::header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value == format!("Bearer {token}")),
        AdminAuthConfig::ApiKey { key } => req
            .headers()
            .get("x-api-key")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value == key),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::Request;
    use tower::Service;

    fn audit_record(version: u64) -> ConfigReloadAuditRecord {
        ConfigReloadAuditRecord {
            version,
            old_version: version.saturating_sub(1),
            hash: format!("hash-{version}"),
            old_hash: format!("hash-{}", version.saturating_sub(1)),
            ts_millis: version,
            source: "test".to_string(),
            namespace: None,
            app: None,
            key: None,
            changed: true,
            success: true,
            error: None,
            diff: Vec::new(),
            section_signatures: Vec::new(),
        }
    }

    fn request(path: &str) -> IncomingRequest {
        Request::builder()
            .uri(path)
            .body(roze_http::body::empty())
            .expect("admin request")
    }

    #[test]
    fn config_reload_history_is_bounded_and_newest_first() {
        let history = ConfigReloadHistory::new(2);
        history.push(audit_record(1));
        history.push(audit_record(2));
        history.push(audit_record(3));

        let records = history.list(0, 100);

        assert_eq!(
            records
                .iter()
                .map(|record| record.version)
                .collect::<Vec<_>>(),
            vec![3, 2]
        );
        assert_eq!(history.list(1, 1)[0].version, 2);
    }

    #[tokio::test]
    async fn config_reload_endpoint_enforces_bearer_auth() {
        let history = ConfigReloadHistory::new(2);
        history.push(audit_record(7));
        let state = AdminState::new()
            .with_config_history(history)
            .with_auth(AdminAuthConfig::bearer("admin-secret"));
        let mut service = admin_service(state);

        let unauthorized = service
            .call(request("/admin/config/reloads"))
            .await
            .expect("admin response");
        assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

        let mut authorized_request = request("/admin/config/reloads");
        authorized_request.headers_mut().insert(
            http::header::AUTHORIZATION,
            http::HeaderValue::from_static("Bearer admin-secret"),
        );
        let authorized = service
            .call(authorized_request)
            .await
            .expect("admin response");
        assert_eq!(authorized.status(), StatusCode::OK);
        let body = roze_http::body::to_bytes(authorized.into_body(), 4096)
            .await
            .expect("admin JSON body");
        let records: Vec<ConfigReloadAuditRecord> =
            serde_json::from_slice(&body).expect("reload audit JSON");
        assert_eq!(records[0].version, 7);
    }

    #[tokio::test]
    async fn api_key_auth_and_unknown_route_are_explicit() {
        let state = AdminState::new().with_auth(AdminAuthConfig::api_key("service-secret"));
        let mut service = admin_service(state);
        let mut authorized_request = request("/admin/unknown");
        authorized_request.headers_mut().insert(
            "x-api-key",
            http::HeaderValue::from_static("service-secret"),
        );

        let response = service
            .call(authorized_request)
            .await
            .expect("admin response");

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}
