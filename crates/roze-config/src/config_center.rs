use std::{
    collections::BTreeMap,
    future::Future,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    pin::Pin,
    str::FromStr,
    string::ToString,
    sync::Arc as StdArc,
    sync::{
        atomic::{AtomicI64, AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{anyhow, Result};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use futures_util::StreamExt;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::{
    sync::{mpsc, RwLock},
    time::{self, Instant},
};
use tracing::warn;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigFormat {
    Json,
    Yaml,
    Toml,
}

impl ConfigFormat {
    fn as_file_format(self) -> config::FileFormat {
        match self {
            Self::Json => config::FileFormat::Json,
            Self::Yaml => config::FileFormat::Yaml,
            Self::Toml => config::FileFormat::Toml,
        }
    }
}

impl FromStr for ConfigFormat {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "json" => Ok(Self::Json),
            "yaml" => Ok(Self::Yaml),
            "yml" => Ok(Self::Yaml),
            "toml" => Ok(Self::Toml),
            _ => Err(anyhow!("unsupported config format: {value}")),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ReloadResult<T>
where
    T: Clone,
{
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
    pub diff: Vec<ConfigDiffEntry>,
    pub section_signatures: Vec<ConfigSectionSignature>,
    pub success: bool,
    pub error: Option<String>,
    pub config: Option<T>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigDiffEntry {
    pub path: String,
    pub kind: ConfigDiffKind,
    pub old: Option<String>,
    pub new: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigDiffKind {
    Added,
    Removed,
    Changed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigSectionSignature {
    pub section: String,
    pub hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigCenterChangeEvent {
    pub version: u64,
    pub old_version: u64,
    pub hash: String,
    pub old_hash: String,
    pub ts_millis: u64,
    pub source: String,
    pub namespace: Option<String>,
    pub app: Option<String>,
    pub key: Option<String>,
    pub section: String,
    pub changed: bool,
    pub success: bool,
    pub error: Option<String>,
    pub paths: Vec<String>,
    pub diff: Vec<ConfigDiffEntry>,
    pub section_hash: Option<String>,
}

#[derive(Debug, Clone)]
struct ReloadMetadata {
    version: u64,
    old_version: u64,
    hash: String,
    old_hash: String,
    namespace: Option<String>,
    app: Option<String>,
    key: Option<String>,
    source: String,
}

impl<T> ReloadResult<T>
where
    T: Clone,
{
    pub fn change_events(&self) -> Vec<ConfigCenterChangeEvent> {
        if !self.success {
            return vec![self.change_event("*", Vec::new())];
        }

        let mut sections = BTreeMap::<String, Vec<ConfigDiffEntry>>::new();
        for entry in &self.diff {
            sections
                .entry(config_section(&entry.path).to_string())
                .or_default()
                .push(entry.clone());
        }

        if sections.is_empty() {
            return vec![self.change_event("root", Vec::new())];
        }

        sections
            .into_iter()
            .map(|(section, diff)| self.change_event(&section, diff))
            .collect()
    }

    fn change_event(&self, section: &str, diff: Vec<ConfigDiffEntry>) -> ConfigCenterChangeEvent {
        ConfigCenterChangeEvent {
            version: self.version,
            old_version: self.old_version,
            hash: self.hash.clone(),
            old_hash: self.old_hash.clone(),
            ts_millis: self.ts_millis,
            source: self.source.clone(),
            namespace: self.namespace.clone(),
            app: self.app.clone(),
            key: self.key.clone(),
            section: section.to_string(),
            changed: self.changed,
            success: self.success,
            error: self.error.clone(),
            paths: diff.iter().map(|entry| entry.path.clone()).collect(),
            diff,
            section_hash: self
                .section_signatures
                .iter()
                .find(|signature| signature.section == section)
                .map(|signature| signature.hash.clone()),
        }
    }

    fn success(
        meta: ReloadMetadata,
        config: T,
        diff: Vec<ConfigDiffEntry>,
        section_signatures: Vec<ConfigSectionSignature>,
    ) -> Self {
        let changed = meta.old_hash != meta.hash;
        Self {
            version: meta.version,
            old_version: meta.old_version,
            hash: meta.hash,
            old_hash: meta.old_hash,
            ts_millis: current_millis(),
            source: meta.source,
            namespace: meta.namespace,
            app: meta.app,
            key: meta.key,
            changed,
            diff,
            section_signatures,
            success: true,
            error: None,
            config: Some(config),
        }
    }

    fn failed(meta: ReloadMetadata, error: impl Into<String>) -> Self {
        Self {
            version: meta.version,
            old_version: meta.old_version,
            hash: meta.hash,
            old_hash: meta.old_hash,
            ts_millis: current_millis(),
            source: meta.source,
            namespace: meta.namespace,
            app: meta.app,
            key: meta.key,
            changed: false,
            diff: Vec::new(),
            section_signatures: Vec::new(),
            success: false,
            error: Some(error.into()),
            config: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ConfigCenterConfig {
    pub format: ConfigFormat,
    pub poll_interval: Duration,
    pub debounce: Duration,
    pub listener_timeout: Duration,
    pub source: Option<String>,
    pub namespace: Option<String>,
    pub app: Option<String>,
    pub key: Option<String>,
}

impl Default for ConfigCenterConfig {
    fn default() -> Self {
        Self {
            format: ConfigFormat::Json,
            poll_interval: Duration::from_secs(5),
            debounce: Duration::from_millis(400),
            listener_timeout: Duration::from_millis(500),
            source: None,
            namespace: None,
            app: None,
            key: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigPermission {
    Read,
    Write,
    Rollback,
    Audit,
    WatchStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigPrincipal {
    pub id: String,
    #[serde(default)]
    pub permissions: Vec<ConfigPermission>,
}

impl ConfigPrincipal {
    pub fn new(
        id: impl Into<String>,
        permissions: impl IntoIterator<Item = ConfigPermission>,
    ) -> Self {
        Self {
            id: id.into(),
            permissions: permissions.into_iter().collect(),
        }
    }

    fn can(&self, permission: ConfigPermission) -> bool {
        self.permissions.contains(&permission)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigChangeRequest {
    pub actor: ConfigPrincipal,
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigVersionRecord {
    pub version: u64,
    pub hash: String,
    pub raw: String,
    pub source: String,
    pub namespace: Option<String>,
    pub app: Option<String>,
    pub key: Option<String>,
    pub author: String,
    pub reason: Option<String>,
    pub created_at_millis: u64,
    pub active: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigAuditAction {
    Read,
    Publish,
    Rollback,
    Audit,
    WatchStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigAuditResult {
    Allowed,
    Denied,
    Rejected,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigAuditRecord {
    pub id: u64,
    pub ts_millis: u64,
    pub actor: String,
    pub action: ConfigAuditAction,
    pub result: ConfigAuditResult,
    pub version: Option<u64>,
    pub target_version: Option<u64>,
    pub hash: Option<String>,
    pub reason: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigWatchMode {
    NativeWatch,
    Polling,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigWatchStatus {
    pub mode: ConfigWatchMode,
    pub source: String,
    pub last_revision: Option<i64>,
    pub last_error: Option<String>,
    pub updated_at_millis: u64,
}

#[derive(Debug, Clone)]
pub struct ConfigAdminMetadata {
    pub source: String,
    pub namespace: Option<String>,
    pub app: Option<String>,
    pub key: Option<String>,
}

impl Default for ConfigAdminMetadata {
    fn default() -> Self {
        Self {
            source: "admin".to_string(),
            namespace: None,
            app: None,
            key: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigCenterAdminStore {
    format: ConfigFormat,
    versions: Vec<ConfigVersionRecord>,
    audit: Vec<ConfigAuditRecord>,
    watch_status: ConfigWatchStatus,
    next_audit_id: u64,
}

impl ConfigCenterAdminStore {
    pub fn new(
        format: ConfigFormat,
        initial_raw: impl Into<String>,
        metadata: ConfigAdminMetadata,
        actor: impl Into<String>,
    ) -> Result<Self> {
        let initial_raw = initial_raw.into();
        parse_config_value(&initial_raw, format)?;
        let now = current_millis();
        let version = ConfigVersionRecord {
            version: 1,
            hash: snapshot_hash(&initial_raw),
            raw: initial_raw,
            source: metadata.source.clone(),
            namespace: metadata.namespace,
            app: metadata.app,
            key: metadata.key,
            author: actor.into(),
            reason: Some("initial import".to_string()),
            created_at_millis: now,
            active: true,
        };
        Ok(Self {
            format,
            versions: vec![version],
            audit: Vec::new(),
            watch_status: ConfigWatchStatus {
                mode: ConfigWatchMode::Unavailable,
                source: metadata.source,
                last_revision: None,
                last_error: None,
                updated_at_millis: now,
            },
            next_audit_id: 1,
        })
    }

    pub fn load_from_path(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let raw = std::fs::read_to_string(path).map_err(|err| {
            anyhow!(
                "read config admin snapshot {} failed: {err}",
                path.display()
            )
        })?;
        let mut store: Self = serde_json::from_str(&raw).map_err(|err| {
            anyhow!(
                "parse config admin snapshot {} failed: {err}",
                path.display()
            )
        })?;
        store.validate_snapshot()?;
        store.next_audit_id = store.next_audit_id.max(
            store
                .audit
                .iter()
                .map(|record| record.id)
                .max()
                .unwrap_or(0)
                + 1,
        );
        Ok(store)
    }

    pub fn save_to_path(&self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent).map_err(|err| {
                anyhow!(
                    "create config admin snapshot directory {} failed: {err}",
                    parent.display()
                )
            })?;
        }
        let raw = serde_json::to_string_pretty(self)?;
        std::fs::write(path, raw).map_err(|err| {
            anyhow!(
                "write config admin snapshot {} failed: {err}",
                path.display()
            )
        })
    }

    pub fn current(&mut self, actor: &ConfigPrincipal) -> Result<ConfigVersionRecord> {
        self.require(actor, ConfigPermission::Read, ConfigAuditAction::Read)?;
        let record = self
            .active_version()
            .ok_or_else(|| anyhow!("config admin store has no active version"))?
            .clone();
        self.push_audit(ConfigAuditRecordInput {
            actor,
            action: ConfigAuditAction::Read,
            result: ConfigAuditResult::Allowed,
            version: Some(record.version),
            target_version: None,
            hash: Some(record.hash.clone()),
            reason: None,
            error: None,
        });
        Ok(record)
    }

    pub fn publish(
        &mut self,
        request: ConfigChangeRequest,
        raw: impl Into<String>,
        metadata: ConfigAdminMetadata,
    ) -> Result<ConfigVersionRecord> {
        self.require(
            &request.actor,
            ConfigPermission::Write,
            ConfigAuditAction::Publish,
        )?;

        let raw = raw.into();
        if let Err(err) = parse_config_value(&raw, self.format) {
            let error = format!("config validation failed: {err}");
            self.push_audit(ConfigAuditRecordInput {
                actor: &request.actor,
                action: ConfigAuditAction::Publish,
                result: ConfigAuditResult::Rejected,
                version: None,
                target_version: None,
                hash: Some(snapshot_hash(&raw)),
                reason: request.reason.clone(),
                error: Some(error.clone()),
            });
            return Err(anyhow!(error));
        }

        let version = self.next_version();
        for record in &mut self.versions {
            record.active = false;
        }
        let record = ConfigVersionRecord {
            version,
            hash: snapshot_hash(&raw),
            raw,
            source: metadata.source,
            namespace: metadata.namespace,
            app: metadata.app,
            key: metadata.key,
            author: request.actor.id.clone(),
            reason: request.reason.clone(),
            created_at_millis: current_millis(),
            active: true,
        };
        self.versions.push(record.clone());
        self.push_audit(ConfigAuditRecordInput {
            actor: &request.actor,
            action: ConfigAuditAction::Publish,
            result: ConfigAuditResult::Allowed,
            version: Some(record.version),
            target_version: None,
            hash: Some(record.hash.clone()),
            reason: request.reason,
            error: None,
        });
        Ok(record)
    }

    pub fn rollback(
        &mut self,
        request: ConfigChangeRequest,
        target_version: u64,
    ) -> Result<ConfigVersionRecord> {
        self.require(
            &request.actor,
            ConfigPermission::Rollback,
            ConfigAuditAction::Rollback,
        )?;

        let target = match self
            .versions
            .iter()
            .find(|record| record.version == target_version)
            .cloned()
        {
            Some(target) => target,
            None => {
                let error = format!("config version {target_version} not found");
                self.push_audit(ConfigAuditRecordInput {
                    actor: &request.actor,
                    action: ConfigAuditAction::Rollback,
                    result: ConfigAuditResult::Rejected,
                    version: self.active_version().map(|record| record.version),
                    target_version: Some(target_version),
                    hash: None,
                    reason: request.reason.clone(),
                    error: Some(error.clone()),
                });
                return Err(anyhow!(error));
            }
        };

        for record in &mut self.versions {
            record.active = record.version == target_version;
        }
        self.push_audit(ConfigAuditRecordInput {
            actor: &request.actor,
            action: ConfigAuditAction::Rollback,
            result: ConfigAuditResult::Allowed,
            version: Some(target.version),
            target_version: Some(target.version),
            hash: Some(target.hash.clone()),
            reason: request.reason,
            error: None,
        });
        Ok(self
            .active_version()
            .expect("rollback just marked an active version")
            .clone())
    }

    pub fn audit_log(&mut self, actor: &ConfigPrincipal) -> Result<Vec<ConfigAuditRecord>> {
        self.require(actor, ConfigPermission::Audit, ConfigAuditAction::Audit)?;
        self.push_audit(ConfigAuditRecordInput {
            actor,
            action: ConfigAuditAction::Audit,
            result: ConfigAuditResult::Allowed,
            version: self.active_version().map(|record| record.version),
            target_version: None,
            hash: None,
            reason: None,
            error: None,
        });
        Ok(self.audit.clone())
    }

    pub fn watch_status(&mut self, actor: &ConfigPrincipal) -> Result<ConfigWatchStatus> {
        self.require(
            actor,
            ConfigPermission::WatchStatus,
            ConfigAuditAction::WatchStatus,
        )?;
        let status = self.watch_status.clone();
        self.push_audit(ConfigAuditRecordInput {
            actor,
            action: ConfigAuditAction::WatchStatus,
            result: ConfigAuditResult::Allowed,
            version: self.active_version().map(|record| record.version),
            target_version: None,
            hash: None,
            reason: None,
            error: None,
        });
        Ok(status)
    }

    pub fn set_watch_status(&mut self, status: ConfigWatchStatus) {
        self.watch_status = status;
    }

    pub fn versions(&self) -> &[ConfigVersionRecord] {
        &self.versions
    }

    fn active_version(&self) -> Option<&ConfigVersionRecord> {
        self.versions.iter().find(|record| record.active)
    }

    fn validate_snapshot(&self) -> Result<()> {
        let active_count = self.versions.iter().filter(|record| record.active).count();
        if active_count != 1 {
            return Err(anyhow!(
                "config admin snapshot must contain exactly one active version, found {active_count}"
            ));
        }
        for record in &self.versions {
            parse_config_value(&record.raw, self.format).map_err(|err| {
                anyhow!(
                    "config admin snapshot version {} is invalid: {err}",
                    record.version
                )
            })?;
            if snapshot_hash(&record.raw) != record.hash {
                return Err(anyhow!(
                    "config admin snapshot version {} hash mismatch",
                    record.version
                ));
            }
        }
        Ok(())
    }

    fn next_version(&self) -> u64 {
        self.versions
            .iter()
            .map(|record| record.version)
            .max()
            .unwrap_or(0)
            + 1
    }

    fn require(
        &mut self,
        actor: &ConfigPrincipal,
        permission: ConfigPermission,
        action: ConfigAuditAction,
    ) -> Result<()> {
        if actor.can(permission) {
            return Ok(());
        }
        let error = format!("actor `{}` lacks {:?} permission", actor.id, permission);
        self.push_audit(ConfigAuditRecordInput {
            actor,
            action,
            result: ConfigAuditResult::Denied,
            version: self.active_version().map(|record| record.version),
            target_version: None,
            hash: None,
            reason: None,
            error: Some(error.clone()),
        });
        Err(anyhow!(error))
    }

    fn push_audit(&mut self, input: ConfigAuditRecordInput<'_>) {
        let record = ConfigAuditRecord {
            id: self.next_audit_id,
            ts_millis: current_millis(),
            actor: input.actor.id.clone(),
            action: input.action,
            result: input.result,
            version: input.version,
            target_version: input.target_version,
            hash: input.hash,
            reason: input.reason,
            error: input.error,
        };
        self.next_audit_id += 1;
        self.audit.push(record);
    }
}

struct ConfigAuditRecordInput<'a> {
    actor: &'a ConfigPrincipal,
    action: ConfigAuditAction,
    result: ConfigAuditResult,
    version: Option<u64>,
    target_version: Option<u64>,
    hash: Option<String>,
    reason: Option<String>,
    error: Option<String>,
}

pub trait Subscriber: Send + Sync {
    fn value(&self) -> Pin<Box<dyn Future<Output = Result<String>> + Send + '_>>;

    fn supports_watch(&self) -> bool {
        false
    }

    fn watch(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<mpsc::UnboundedReceiver<String>>> + Send + '_>> {
        Box::pin(async { Err(anyhow!("subscriber does not support watch")) })
    }
}

#[derive(Debug, Clone)]
pub struct EnvVarSubscriber {
    key: String,
}

impl EnvVarSubscriber {
    pub fn new(key: impl Into<String>) -> Self {
        Self { key: key.into() }
    }
}

impl Subscriber for EnvVarSubscriber {
    fn value(&self) -> Pin<Box<dyn Future<Output = Result<String>> + Send + '_>> {
        let key = self.key.clone();
        Box::pin(async move {
            std::env::var(&key).map_err(|_| anyhow!("environment config key not found: {key}"))
        })
    }
}

#[derive(Debug, Clone)]
pub struct FileConfigSubscriber {
    path: PathBuf,
}

impl FileConfigSubscriber {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }
}

impl Subscriber for FileConfigSubscriber {
    fn value(&self) -> Pin<Box<dyn Future<Output = Result<String>> + Send + '_>> {
        let path = self.path.clone();
        Box::pin(async move {
            std::fs::read_to_string(&path)
                .map_err(|err| anyhow!("read config from file {} failed: {err}", path.display()))
        })
    }
}

#[derive(Debug, Clone)]
pub struct EtcdSubscriber {
    endpoints: Vec<String>,
    key: String,
}

impl EtcdSubscriber {
    pub fn new(endpoints: Vec<String>, key: impl Into<String>) -> Self {
        Self {
            endpoints,
            key: key.into(),
        }
    }
}

impl Subscriber for EtcdSubscriber {
    fn value(&self) -> Pin<Box<dyn Future<Output = Result<String>> + Send + '_>> {
        let endpoints = self.endpoints.clone();
        let key = self.key.clone();
        Box::pin(async move {
            let client = reqwest::Client::new();
            let mut last_error = None;

            for endpoint in &endpoints {
                match fetch_etcd_key(&client, endpoint, &key).await {
                    Ok(value) => return Ok(value),
                    Err(err) => {
                        let err = with_proxy_diagnostic(err, endpoint);
                        warn!(%err, endpoint, key = %key, "fetching config from etcd failed");
                        last_error = Some(err);
                    }
                }
            }

            Err(last_error
                .unwrap_or_else(|| anyhow!("no configured etcd endpoint could return config")))
        })
    }

    fn supports_watch(&self) -> bool {
        true
    }

    fn watch(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<mpsc::UnboundedReceiver<String>>> + Send + '_>> {
        let endpoints = self.endpoints.clone();
        let key = self.key.clone();
        Box::pin(async move {
            let (tx, rx) = mpsc::unbounded_channel();
            tokio::spawn(async move {
                let last_revision = Arc::new(AtomicI64::new(0));
                loop {
                    let client = reqwest::Client::new();
                    let mut connected = false;
                    for endpoint in config_center_endpoints(&endpoints, "http://127.0.0.1:2379") {
                        match stream_etcd_watch(
                            &client,
                            &endpoint,
                            &key,
                            tx.clone(),
                            last_revision.clone(),
                        )
                        .await
                        {
                            Ok(()) => {
                                connected = true;
                                break;
                            }
                            Err(err) => {
                                warn!(
                                    %err,
                                    endpoint = %endpoint,
                                    key = %key,
                                    "etcd native watch stream failed"
                                );
                            }
                        }
                    }
                    if !connected {
                        time::sleep(Duration::from_secs(1)).await;
                    }
                }
            });
            Ok(rx)
        })
    }
}

#[derive(Default)]
pub struct CascadingSubscriber {
    sources: Vec<StdArc<dyn Subscriber>>,
}

impl CascadingSubscriber {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push<S>(&mut self, source: S)
    where
        S: Subscriber + 'static,
    {
        self.sources.push(StdArc::new(source));
    }
}

impl Subscriber for CascadingSubscriber {
    fn value(&self) -> Pin<Box<dyn Future<Output = Result<String>> + Send + '_>> {
        let sources = self.sources.clone();
        Box::pin(async move {
            let mut last_error: Option<anyhow::Error> = None;
            for source in sources {
                match source.value().await {
                    Ok(raw) => return Ok(raw),
                    Err(err) => {
                        last_error = Some(anyhow!("{err}"));
                    }
                }
            }

            Err(last_error.unwrap_or_else(|| anyhow!("no config source available")))
        })
    }

    fn supports_watch(&self) -> bool {
        self.sources.iter().any(|source| source.supports_watch())
    }

    fn watch(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<mpsc::UnboundedReceiver<String>>> + Send + '_>> {
        let sources = self.sources.clone();
        Box::pin(async move {
            for source in sources {
                if source.supports_watch() {
                    return source.watch().await;
                }
            }
            Err(anyhow!("no config source supports watch"))
        })
    }
}

type Listener<T> = Arc<dyn Fn(&T) + Send + Sync + 'static>;
type ReloadListener<T> = Arc<dyn Fn(&ReloadResult<T>) + Send + Sync + 'static>;

struct ConfigCenterInner<T: Clone> {
    value: RwLock<T>,
    listeners: RwLock<Vec<Listener<T>>>,
    reload_listeners: RwLock<Vec<ReloadListener<T>>>,
    version: AtomicU64,
}

#[derive(Clone)]
pub struct ConfigCenter<T: Clone> {
    inner: Arc<ConfigCenterInner<T>>,
}

impl<T> ConfigCenter<T>
where
    T: DeserializeOwned + Clone + Send + Sync + 'static,
{
    pub async fn new<S>(subscriber: S, options: ConfigCenterConfig) -> Result<Self>
    where
        S: Subscriber + 'static,
    {
        let (initial, initial_snapshot) = load_once(&subscriber, options.format).await?;
        let inner = Arc::new(ConfigCenterInner {
            value: RwLock::new(initial),
            listeners: RwLock::new(Vec::new()),
            reload_listeners: RwLock::new(Vec::new()),
            version: AtomicU64::new(0),
        });
        let watch_inner = inner.clone();

        tokio::spawn(watch_loop(
            Arc::new(subscriber),
            options,
            initial_snapshot,
            watch_inner,
        ));

        Ok(Self { inner })
    }

    pub async fn get_config(&self) -> T {
        self.inner.value.read().await.clone()
    }

    pub async fn add_listener<F>(&self, listener: F)
    where
        F: Fn(&T) + Send + Sync + 'static,
    {
        self.inner.listeners.write().await.push(Arc::new(listener));
    }

    pub async fn add_reload_listener<F>(&self, listener: F)
    where
        F: Fn(&ReloadResult<T>) + Send + Sync + 'static,
    {
        self.inner
            .reload_listeners
            .write()
            .await
            .push(Arc::new(listener));
    }
}

async fn watch_loop<T>(
    subscriber: Arc<dyn Subscriber>,
    options: ConfigCenterConfig,
    mut last_snapshot: String,
    inner: Arc<ConfigCenterInner<T>>,
) where
    T: DeserializeOwned + Clone + Send + Sync + 'static,
{
    let source = options
        .source
        .clone()
        .unwrap_or_else(|| "config-center".to_string());
    let mut last_hash = snapshot_hash(&last_snapshot);
    let mut pending: Option<(String, Instant)> = None;
    let mut watch_rx = if subscriber.supports_watch() {
        match subscriber.watch().await {
            Ok(rx) => {
                tracing::info!(source = %source, "config center using native watch");
                Some(rx)
            }
            Err(err) => {
                warn!(%err, source = %source, "config center native watch unavailable, fallback to polling");
                None
            }
        }
    } else {
        None
    };

    loop {
        let snapshot = if let Some(rx) = &mut watch_rx {
            match rx.recv().await {
                Some(raw) => raw,
                None => {
                    warn!(source = %source, "config center native watch channel closed, fallback to polling");
                    watch_rx = None;
                    continue;
                }
            }
        } else {
            time::sleep(options.poll_interval).await;
            match subscriber.value().await {
                Ok(raw) => raw,
                Err(err) => {
                    warn!(%err, source = %source, "read config center value failed");
                    continue;
                }
            }
        };

        let snapshot_hash = snapshot_hash(&snapshot);
        if snapshot_hash == last_hash {
            pending = None;
            continue;
        }

        let now = Instant::now();
        match &pending {
            Some((cached, at)) if cached == &snapshot => {
                if now.duration_since(*at) < options.debounce {
                    continue;
                }
            }
            _ => {
                pending = Some((snapshot.clone(), now));
                continue;
            }
        }

        let parsed = match parse_config::<T>(&snapshot, options.format) {
            Ok(parsed) => parsed,
            Err(err) => {
                let old_version = inner.version.load(Ordering::SeqCst);
                let result = ReloadResult::failed(
                    ReloadMetadata {
                        version: old_version + 1,
                        old_version,
                        hash: snapshot_hash,
                        old_hash: last_hash.clone(),
                        namespace: options.namespace.clone(),
                        app: options.app.clone(),
                        key: options.key.clone(),
                        source: source.clone(),
                    },
                    err.to_string(),
                );

                notify_reload_listeners(
                    inner.reload_listeners.read().await.clone(),
                    result.clone(),
                    options.listener_timeout,
                    &source,
                )
                .await;
                warn!(
                    source = %source,
                    hash = %result.hash,
                    error = %result.error.clone().unwrap_or_default(),
                    "parse config center value failed, keep old config"
                );

                pending = None;
                continue;
            }
        };

        let next_version = inner.version.fetch_add(1, Ordering::SeqCst) + 1;
        let old_version = next_version - 1;
        let old_hash = last_hash.clone();
        let diff = config_diff(&last_snapshot, &snapshot, options.format);
        let section_signatures = config_section_signatures(&snapshot, options.format);

        {
            *inner.value.write().await = parsed.clone();
            last_hash = snapshot_hash.clone();
            last_snapshot = snapshot.clone();
        }

        let result = ReloadResult::success(
            ReloadMetadata {
                version: next_version,
                old_version,
                hash: last_hash.clone(),
                old_hash,
                namespace: options.namespace.clone(),
                app: options.app.clone(),
                key: options.key.clone(),
                source: source.clone(),
            },
            parsed.clone(),
            diff,
            section_signatures,
        );

        if result.changed {
            notify_config_listeners(
                inner.listeners.read().await.clone(),
                parsed.clone(),
                options.listener_timeout,
                &source,
            )
            .await;
        }

        notify_reload_listeners(
            inner.reload_listeners.read().await.clone(),
            result.clone(),
            options.listener_timeout,
            &source,
        )
        .await;

        pending = None;
        let changed_paths = result
            .diff
            .iter()
            .map(|entry| entry.path.as_str())
            .collect::<Vec<_>>()
            .join(",");
        tracing::info!(
            source = %source,
            version = result.version,
            old_version = result.old_version,
            hash = %result.hash,
            changed = result.changed,
            changed_paths = %changed_paths,
            success = result.success,
            "config center reload applied"
        );
    }
}

async fn notify_config_listeners<T>(
    listeners: Vec<Listener<T>>,
    config: T,
    timeout: Duration,
    source: &str,
) where
    T: Clone + Send + 'static,
{
    let mut tasks = listeners
        .into_iter()
        .enumerate()
        .map(|(index, listener)| {
            let config = config.clone();
            tokio::task::spawn_blocking(move || {
                listener(&config);
                index
            })
        })
        .collect::<Vec<_>>();

    for task in tasks.drain(..) {
        match time::timeout(timeout.max(Duration::from_millis(1)), task).await {
            Ok(Ok(_)) => {}
            Ok(Err(err)) => {
                warn!(%err, source, "config center config listener failed");
            }
            Err(_) => {
                warn!(
                    source,
                    timeout_ms = timeout.as_millis(),
                    "config center config listener timed out"
                );
            }
        }
    }
}

async fn notify_reload_listeners<T>(
    listeners: Vec<ReloadListener<T>>,
    result: ReloadResult<T>,
    timeout: Duration,
    source: &str,
) where
    T: Clone + Send + 'static,
{
    let mut tasks = listeners
        .into_iter()
        .enumerate()
        .map(|(index, listener)| {
            let result = result.clone();
            tokio::task::spawn_blocking(move || {
                listener(&result);
                index
            })
        })
        .collect::<Vec<_>>();

    for task in tasks.drain(..) {
        match time::timeout(timeout.max(Duration::from_millis(1)), task).await {
            Ok(Ok(_)) => {}
            Ok(Err(err)) => {
                warn!(%err, source, "config center reload listener failed");
            }
            Err(_) => {
                warn!(
                    source,
                    timeout_ms = timeout.as_millis(),
                    "config center reload listener timed out"
                );
            }
        }
    }
}

async fn load_once<T, S>(subscriber: &S, format: ConfigFormat) -> Result<(T, String)>
where
    T: DeserializeOwned + Send + 'static,
    S: Subscriber + ?Sized,
{
    let raw = subscriber.value().await?;
    let value = parse_config(&raw, format)?;
    Ok((value, raw))
}

fn parse_config<T>(raw: &str, format: ConfigFormat) -> Result<T>
where
    T: DeserializeOwned,
{
    config::Config::builder()
        .add_source(config::File::from_str(raw, format.as_file_format()))
        .build()?
        .try_deserialize::<T>()
        .map_err(Into::into)
}

fn snapshot_hash(raw: &str) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    raw.hash(&mut hasher);
    hasher.finish().to_string()
}

pub fn config_diff(old_raw: &str, new_raw: &str, format: ConfigFormat) -> Vec<ConfigDiffEntry> {
    let Ok(old_value) = parse_config_value(old_raw, format) else {
        return Vec::new();
    };
    let Ok(new_value) = parse_config_value(new_raw, format) else {
        return Vec::new();
    };
    let mut diff = Vec::new();
    collect_config_diff("", &old_value, &new_value, &mut diff);
    diff
}

pub fn config_section_signatures(raw: &str, format: ConfigFormat) -> Vec<ConfigSectionSignature> {
    let Ok(value) = parse_config_value(raw, format) else {
        return Vec::new();
    };
    match value {
        Value::Object(map) => {
            let mut sections = map
                .into_iter()
                .map(|(section, value)| ConfigSectionSignature {
                    section,
                    hash: stable_config_hash(&value),
                })
                .collect::<Vec<_>>();
            sections.sort_by(|left, right| left.section.cmp(&right.section));
            sections
        }
        other => vec![ConfigSectionSignature {
            section: "root".to_string(),
            hash: stable_config_hash(&other),
        }],
    }
}

fn parse_config_value(raw: &str, format: ConfigFormat) -> Result<Value> {
    config::Config::builder()
        .add_source(config::File::from_str(raw, format.as_file_format()))
        .build()?
        .try_deserialize::<Value>()
        .map_err(Into::into)
}

fn collect_config_diff(
    path: &str,
    old_value: &Value,
    new_value: &Value,
    diff: &mut Vec<ConfigDiffEntry>,
) {
    match (old_value, new_value) {
        (Value::Object(old), Value::Object(new)) => {
            let mut keys = old.keys().chain(new.keys()).collect::<Vec<_>>();
            keys.sort();
            keys.dedup();
            for key in keys {
                let child_path = join_config_path(path, key);
                match (old.get(key), new.get(key)) {
                    (Some(old_child), Some(new_child)) => {
                        collect_config_diff(&child_path, old_child, new_child, diff);
                    }
                    (None, Some(new_child)) => diff.push(ConfigDiffEntry {
                        path: child_path,
                        kind: ConfigDiffKind::Added,
                        old: None,
                        new: Some(config_value_summary(new_child)),
                    }),
                    (Some(old_child), None) => diff.push(ConfigDiffEntry {
                        path: child_path,
                        kind: ConfigDiffKind::Removed,
                        old: Some(config_value_summary(old_child)),
                        new: None,
                    }),
                    (None, None) => {}
                }
            }
        }
        _ if old_value != new_value => diff.push(ConfigDiffEntry {
            path: if path.is_empty() {
                "$".to_string()
            } else {
                path.to_string()
            },
            kind: ConfigDiffKind::Changed,
            old: Some(config_value_summary(old_value)),
            new: Some(config_value_summary(new_value)),
        }),
        _ => {}
    }
}

fn join_config_path(parent: &str, key: &str) -> String {
    if parent.is_empty() {
        key.to_string()
    } else {
        format!("{parent}.{key}")
    }
}

fn config_section(path: &str) -> &str {
    if path.is_empty() || path == "$" {
        return "root";
    }
    path.split('.').next().unwrap_or("root")
}

fn stable_config_hash(value: &Value) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    stable_config_value(value).hash(&mut hasher);
    hasher.finish().to_string()
}

fn stable_config_value(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(value) => format!("bool:{value}"),
        Value::Number(value) => format!("number:{value}"),
        Value::String(value) => format!("string:{value}"),
        Value::Array(items) => {
            let inner = items
                .iter()
                .map(stable_config_value)
                .collect::<Vec<_>>()
                .join(",");
            format!("array:[{inner}]")
        }
        Value::Object(map) => {
            let mut entries = map
                .iter()
                .map(|(key, value)| format!("{key}:{}", stable_config_value(value)))
                .collect::<Vec<_>>();
            entries.sort();
            format!("object:{{{}}}", entries.join(","))
        }
    }
}

fn config_value_summary(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::String(value) => value.clone(),
        Value::Array(value) => format!("[{} items]", value.len()),
        Value::Object(value) => format!("{{{} keys}}", value.len()),
    }
}

fn current_millis() -> u64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(elapsed) => elapsed.as_millis() as u64,
        Err(_) => 0,
    }
}

async fn fetch_etcd_key(client: &reqwest::Client, endpoint: &str, key: &str) -> Result<String> {
    let body = json!({
        "key": STANDARD.encode(key),
    });
    let endpoint = normalize_endpoint(endpoint);
    let response: EtcdRangeResponse = client
        .post(format!("{}/v3/kv/range", endpoint))
        .json(&body)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let first = response
        .kvs
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("etcd key {key} not found"))?;
    let decoded = STANDARD
        .decode(first.value)
        .map_err(|err| anyhow!("decode etcd value failed: {err}"))?;
    String::from_utf8(decoded).map_err(|err| anyhow!("config value is not utf-8: {err}"))
}

fn with_proxy_diagnostic(err: anyhow::Error, endpoint: &str) -> anyhow::Error {
    if let Some(hint) = crate::http_proxy_environment_diagnostic(endpoint) {
        anyhow!("{err}; {hint}")
    } else {
        err
    }
}

async fn stream_etcd_watch(
    client: &reqwest::Client,
    endpoint: &str,
    key: &str,
    tx: mpsc::UnboundedSender<String>,
    last_revision: Arc<AtomicI64>,
) -> Result<()> {
    let mut create_request = serde_json::Map::new();
    create_request.insert("key".to_string(), json!(STANDARD.encode(key)));
    let revision = last_revision.load(Ordering::SeqCst);
    if revision > 0 {
        create_request.insert("start_revision".to_string(), json!(revision + 1));
    }
    let body = json!({
        "create_request": create_request,
    });
    let response = client
        .post(format!("{}/v3/watch", normalize_endpoint(endpoint)))
        .json(&body)
        .send()
        .await
        .map_err(|err| with_proxy_diagnostic(err.into(), endpoint))?
        .error_for_status()?;
    let mut stream = response.bytes_stream();
    let mut buffer = String::new();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        buffer.push_str(std::str::from_utf8(&chunk)?);

        while let Some(idx) = buffer.find('\n') {
            let line = buffer[..idx].trim().to_string();
            buffer = buffer[idx + 1..].to_string();
            if line.is_empty() {
                continue;
            }
            for update in etcd_watch_updates(&line)? {
                if let Some(revision) = update.revision {
                    last_revision.fetch_max(revision, Ordering::SeqCst);
                }
                if let Some(value) = update.value {
                    let _ = tx.send(value);
                }
            }
        }

        if let Ok(updates) = etcd_watch_updates(&buffer) {
            buffer.clear();
            for update in updates {
                if let Some(revision) = update.revision {
                    last_revision.fetch_max(revision, Ordering::SeqCst);
                }
                if let Some(value) = update.value {
                    let _ = tx.send(value);
                }
            }
        }
    }

    Err(anyhow!("etcd watch stream ended"))
}

#[cfg(test)]
fn etcd_watch_values(raw: &str) -> Result<Vec<String>> {
    Ok(etcd_watch_updates(raw)?
        .into_iter()
        .filter_map(|update| update.value)
        .collect())
}

fn etcd_watch_updates(raw: &str) -> Result<Vec<EtcdWatchUpdate>> {
    if raw.trim().is_empty() {
        return Ok(Vec::new());
    }
    let response: EtcdWatchResponse = serde_json::from_str(raw)?;
    let mut updates = Vec::new();
    let Some(result) = response.result else {
        return Ok(updates);
    };
    let header_revision = result
        .header
        .as_ref()
        .and_then(|header| header.revision.as_deref())
        .and_then(|revision| revision.parse::<i64>().ok());
    for event in result.events {
        if let Some(kv) = event.kv {
            let revision = kv
                .mod_revision
                .as_deref()
                .and_then(|revision| revision.parse::<i64>().ok())
                .or(header_revision);
            let value = if kv.value.is_empty() {
                None
            } else {
                let decoded = STANDARD
                    .decode(kv.value)
                    .map_err(|err| anyhow!("decode etcd watch value failed: {err}"))?;
                Some(
                    String::from_utf8(decoded)
                        .map_err(|err| anyhow!("config watch value is not utf-8: {err}"))?,
                )
            };
            updates.push(EtcdWatchUpdate { value, revision });
        } else if let Some(revision) = header_revision {
            updates.push(EtcdWatchUpdate {
                value: None,
                revision: Some(revision),
            });
        }
    }
    Ok(updates)
}

fn normalize_endpoint(endpoint: &str) -> String {
    let endpoint = endpoint.trim().trim_end_matches('/');
    if endpoint.starts_with("http://") || endpoint.starts_with("https://") {
        endpoint.to_string()
    } else {
        format!("http://{endpoint}")
    }
}

fn config_center_endpoints(configured: &[String], default: &str) -> Vec<String> {
    if configured.is_empty() {
        vec![default.to_string()]
    } else {
        configured
            .iter()
            .map(|endpoint| normalize_endpoint(endpoint))
            .collect()
    }
}

#[derive(Debug, Deserialize)]
struct EtcdRangeResponse {
    #[serde(default)]
    kvs: Vec<EtcdKv>,
}

#[derive(Debug, Deserialize)]
struct EtcdKv {
    #[serde(default)]
    value: String,
    #[serde(default)]
    mod_revision: Option<String>,
}

#[derive(Debug, Deserialize)]
struct EtcdWatchResponse {
    #[serde(default)]
    result: Option<EtcdWatchResult>,
}

#[derive(Debug, Deserialize)]
struct EtcdWatchResult {
    #[serde(default)]
    header: Option<EtcdWatchHeader>,
    #[serde(default)]
    events: Vec<EtcdWatchEvent>,
}

#[derive(Debug, Deserialize)]
struct EtcdWatchHeader {
    #[serde(default)]
    revision: Option<String>,
}

#[derive(Debug, Deserialize)]
struct EtcdWatchEvent {
    #[serde(default)]
    kv: Option<EtcdKv>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct EtcdWatchUpdate {
    value: Option<String>,
    revision: Option<i64>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc as StdArc,
    };

    #[test]
    fn parses_config_format_names() {
        assert!(matches!(
            " json ".parse::<ConfigFormat>(),
            Ok(ConfigFormat::Json)
        ));
        assert!(matches!(
            "YML".parse::<ConfigFormat>(),
            Ok(ConfigFormat::Yaml)
        ));
        assert!(matches!(
            "toml".parse::<ConfigFormat>(),
            Ok(ConfigFormat::Toml)
        ));
        assert!("ini".parse::<ConfigFormat>().is_err());
    }

    #[test]
    fn decodes_etcd_watch_values() {
        let value = STANDARD.encode("name: demo\n");
        let raw = serde_json::json!({
            "result": {
                "events": [
                    {
                        "type": "PUT",
                        "kv": {
                            "key": STANDARD.encode("roze/demo/config"),
                            "value": value,
                        }
                    }
                ]
            }
        });

        let values = etcd_watch_values(&raw.to_string()).expect("watch values");

        assert_eq!(values, vec!["name: demo\n"]);
    }

    #[test]
    fn normalizes_config_center_endpoints() {
        assert_eq!(
            config_center_endpoints(&["127.0.0.1:2379".to_string()], "http://default"),
            vec!["http://127.0.0.1:2379"]
        );
    }

    #[test]
    fn config_diff_reports_changed_added_and_removed_paths() {
        let old = r#"
name: demo
gateway:
  timeout_ms: 1000
  services:
    - name: user
removed: true
"#;
        let new = r#"
name: demo
gateway:
  timeout_ms: 2000
  services:
    - name: user
added: yes
"#;

        let diff = config_diff(old, new, ConfigFormat::Yaml);

        assert!(diff.contains(&ConfigDiffEntry {
            path: "added".to_string(),
            kind: ConfigDiffKind::Added,
            old: None,
            new: Some("yes".to_string()),
        }));
        assert!(diff.contains(&ConfigDiffEntry {
            path: "gateway.timeout_ms".to_string(),
            kind: ConfigDiffKind::Changed,
            old: Some("1000".to_string()),
            new: Some("2000".to_string()),
        }));
        assert!(diff.contains(&ConfigDiffEntry {
            path: "removed".to_string(),
            kind: ConfigDiffKind::Removed,
            old: Some("true".to_string()),
            new: None,
        }));
    }

    #[test]
    fn reload_result_groups_change_events_by_section() {
        let result = ReloadResult::success(
            ReloadMetadata {
                version: 2,
                old_version: 1,
                hash: "new".to_string(),
                old_hash: "old".to_string(),
                namespace: Some("prod".to_string()),
                app: Some("user".to_string()),
                key: Some("roze/user/config".to_string()),
                source: "etcd".to_string(),
            },
            (),
            vec![
                ConfigDiffEntry {
                    path: "gateway.timeout_ms".to_string(),
                    kind: ConfigDiffKind::Changed,
                    old: Some("1000".to_string()),
                    new: Some("2000".to_string()),
                },
                ConfigDiffEntry {
                    path: "kafka.client_id".to_string(),
                    kind: ConfigDiffKind::Changed,
                    old: Some("old-client".to_string()),
                    new: Some("new-client".to_string()),
                },
                ConfigDiffEntry {
                    path: "kafka.group_id".to_string(),
                    kind: ConfigDiffKind::Added,
                    old: None,
                    new: Some("workers".to_string()),
                },
            ],
            vec![
                ConfigSectionSignature {
                    section: "gateway".to_string(),
                    hash: "gateway-hash".to_string(),
                },
                ConfigSectionSignature {
                    section: "kafka".to_string(),
                    hash: "kafka-hash".to_string(),
                },
            ],
        );

        let events = result.change_events();

        assert_eq!(events.len(), 2);
        assert_eq!(events[0].section, "gateway");
        assert_eq!(events[0].paths, vec!["gateway.timeout_ms"]);
        assert_eq!(events[0].section_hash.as_deref(), Some("gateway-hash"));
        assert_eq!(events[1].section, "kafka");
        assert_eq!(events[1].paths, vec!["kafka.client_id", "kafka.group_id"]);
        assert_eq!(events[1].section_hash.as_deref(), Some("kafka-hash"));
        assert!(events.iter().all(|event| event.changed && event.success));
    }

    #[test]
    fn section_signatures_are_stable_and_section_scoped() {
        let first = r#"
name: demo
kafka:
  brokers: ["127.0.0.1:9092"]
  client_id: worker
gateway:
  timeout_ms: 1000
"#;
        let reordered = r#"
gateway:
  timeout_ms: 1000
kafka:
  client_id: worker
  brokers: ["127.0.0.1:9092"]
name: demo
"#;
        let changed = r#"
name: demo
kafka:
  brokers: ["127.0.0.1:9093"]
  client_id: worker
gateway:
  timeout_ms: 1000
"#;

        let first_signatures = config_section_signatures(first, ConfigFormat::Yaml);
        let reordered_signatures = config_section_signatures(reordered, ConfigFormat::Yaml);
        let changed_signatures = config_section_signatures(changed, ConfigFormat::Yaml);

        assert_eq!(first_signatures, reordered_signatures);
        assert_ne!(
            section_hash(&first_signatures, "kafka"),
            section_hash(&changed_signatures, "kafka")
        );
        assert_eq!(
            section_hash(&first_signatures, "gateway"),
            section_hash(&changed_signatures, "gateway")
        );
    }

    #[test]
    fn failed_reload_result_emits_failure_change_event() {
        let result = ReloadResult::<()>::failed(
            ReloadMetadata {
                version: 2,
                old_version: 1,
                hash: "bad".to_string(),
                old_hash: "old".to_string(),
                namespace: None,
                app: None,
                key: None,
                source: "file".to_string(),
            },
            "parse failed",
        );

        let events = result.change_events();

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].section, "*");
        assert!(!events[0].success);
        assert_eq!(events[0].error.as_deref(), Some("parse failed"));
    }

    #[tokio::test]
    async fn config_listener_timeout_does_not_block_following_listeners() {
        let hits = StdArc::new(AtomicUsize::new(0));
        let slow: Listener<()> = Arc::new(|_| {
            std::thread::sleep(Duration::from_millis(200));
        });
        let fast_hits = hits.clone();
        let fast: Listener<()> = Arc::new(move |_| {
            fast_hits.fetch_add(1, Ordering::SeqCst);
        });

        notify_config_listeners(vec![slow, fast], (), Duration::from_millis(10), "test").await;

        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn reload_listener_panic_does_not_block_following_listeners() {
        let hits = StdArc::new(AtomicUsize::new(0));
        let panicking: ReloadListener<()> = Arc::new(|_| panic!("listener panic"));
        let fast_hits = hits.clone();
        let fast: ReloadListener<()> = Arc::new(move |_| {
            fast_hits.fetch_add(1, Ordering::SeqCst);
        });
        let result = ReloadResult::success(
            ReloadMetadata {
                version: 2,
                old_version: 1,
                hash: "new".to_string(),
                old_hash: "old".to_string(),
                namespace: None,
                app: None,
                key: None,
                source: "test".to_string(),
            },
            (),
            Vec::new(),
            Vec::new(),
        );

        notify_reload_listeners(
            vec![panicking, fast],
            result,
            Duration::from_millis(50),
            "test",
        )
        .await;

        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn config_admin_publish_validates_permissions_and_payload() {
        let mut store = ConfigCenterAdminStore::new(
            ConfigFormat::Yaml,
            "name: demo\n",
            ConfigAdminMetadata::default(),
            "system",
        )
        .expect("create store");
        let reader = ConfigPrincipal::new("reader", [ConfigPermission::Read]);
        let writer = ConfigPrincipal::new(
            "writer",
            [
                ConfigPermission::Read,
                ConfigPermission::Write,
                ConfigPermission::Audit,
            ],
        );

        let denied = store.publish(
            ConfigChangeRequest {
                actor: reader,
                reason: Some("should fail".to_string()),
            },
            "name: denied\n",
            ConfigAdminMetadata::default(),
        );
        assert!(denied
            .expect_err("reader cannot publish")
            .to_string()
            .contains("lacks Write permission"));

        let invalid = store.publish(
            ConfigChangeRequest {
                actor: writer.clone(),
                reason: Some("invalid yaml".to_string()),
            },
            "name: [broken\n",
            ConfigAdminMetadata::default(),
        );
        assert!(invalid
            .expect_err("invalid payload is rejected")
            .to_string()
            .contains("config validation failed"));
        assert_eq!(store.current(&writer).expect("current").raw, "name: demo\n");

        let published = store
            .publish(
                ConfigChangeRequest {
                    actor: writer.clone(),
                    reason: Some("roll forward".to_string()),
                },
                "name: next\n",
                ConfigAdminMetadata {
                    source: "admin".to_string(),
                    namespace: Some("prod".to_string()),
                    app: Some("user".to_string()),
                    key: Some("roze/user/config".to_string()),
                },
            )
            .expect("publish valid config");

        assert_eq!(published.version, 2);
        assert_eq!(published.namespace.as_deref(), Some("prod"));
        let audit = store.audit_log(&writer).expect("audit");
        assert!(audit
            .iter()
            .any(|record| record.action == ConfigAuditAction::Publish
                && record.result == ConfigAuditResult::Denied));
        assert!(audit
            .iter()
            .any(|record| record.action == ConfigAuditAction::Publish
                && record.result == ConfigAuditResult::Rejected));
        assert!(
            audit
                .iter()
                .any(|record| record.version == Some(2)
                    && record.result == ConfigAuditResult::Allowed)
        );
    }

    #[test]
    fn config_admin_rolls_back_and_exposes_watch_status_with_permissions() {
        let mut store = ConfigCenterAdminStore::new(
            ConfigFormat::Yaml,
            "name: v1\n",
            ConfigAdminMetadata::default(),
            "system",
        )
        .expect("create store");
        let operator = ConfigPrincipal::new(
            "operator",
            [
                ConfigPermission::Read,
                ConfigPermission::Write,
                ConfigPermission::Rollback,
                ConfigPermission::WatchStatus,
            ],
        );
        let reader = ConfigPrincipal::new("reader", [ConfigPermission::Read]);

        store
            .publish(
                ConfigChangeRequest {
                    actor: operator.clone(),
                    reason: Some("v2".to_string()),
                },
                "name: v2\n",
                ConfigAdminMetadata::default(),
            )
            .expect("publish v2");
        assert_eq!(store.current(&operator).expect("current").version, 2);

        let rolled_back = store
            .rollback(
                ConfigChangeRequest {
                    actor: operator.clone(),
                    reason: Some("bad rollout".to_string()),
                },
                1,
            )
            .expect("rollback");
        assert_eq!(rolled_back.version, 1);
        assert_eq!(rolled_back.raw, "name: v1\n");

        store.set_watch_status(ConfigWatchStatus {
            mode: ConfigWatchMode::Polling,
            source: "etcd".to_string(),
            last_revision: Some(42),
            last_error: Some("native watch unavailable".to_string()),
            updated_at_millis: 7,
        });
        assert!(store.watch_status(&reader).is_err());
        let status = store.watch_status(&operator).expect("watch status");
        assert_eq!(status.mode, ConfigWatchMode::Polling);
        assert_eq!(status.last_revision, Some(42));
    }

    #[test]
    fn config_admin_snapshot_round_trips_versions_and_audit() {
        let root = std::env::temp_dir().join(format!(
            "roze-config-admin-snapshot-{}-{}",
            std::process::id(),
            current_millis()
        ));
        let snapshot = root.join("admin.json");
        let mut store = ConfigCenterAdminStore::new(
            ConfigFormat::Yaml,
            "name: v1\n",
            ConfigAdminMetadata::default(),
            "system",
        )
        .expect("create store");
        let operator = ConfigPrincipal::new(
            "operator",
            [
                ConfigPermission::Read,
                ConfigPermission::Write,
                ConfigPermission::Audit,
            ],
        );
        store
            .publish(
                ConfigChangeRequest {
                    actor: operator.clone(),
                    reason: Some("v2".to_string()),
                },
                "name: v2\n",
                ConfigAdminMetadata::default(),
            )
            .expect("publish v2");
        store.save_to_path(&snapshot).expect("save snapshot");

        let mut loaded = ConfigCenterAdminStore::load_from_path(&snapshot).expect("load snapshot");
        assert_eq!(loaded.versions().len(), 2);
        assert_eq!(loaded.current(&operator).expect("current").version, 2);
        let audit = loaded.audit_log(&operator).expect("audit");
        assert!(
            audit
                .iter()
                .any(|record| record.action == ConfigAuditAction::Publish
                    && record.version == Some(2))
        );

        loaded
            .publish(
                ConfigChangeRequest {
                    actor: operator,
                    reason: Some("v3".to_string()),
                },
                "name: v3\n",
                ConfigAdminMetadata::default(),
            )
            .expect("publish v3");
        assert!(loaded
            .audit
            .windows(2)
            .all(|window| window[0].id < window[1].id));

        std::fs::remove_dir_all(root).expect("remove temp root");
    }

    #[test]
    #[ignore = "production-soak: set ROZE_CONFIG_CENTER_SOAK_SECONDS/ROZE_CONFIG_CENTER_SOAK_UPDATES for long runs"]
    fn production_soak_admin_store_validation_rollback_and_snapshot() {
        let seconds = std::env::var("ROZE_CONFIG_CENTER_SOAK_SECONDS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(5);
        let max_updates = std::env::var("ROZE_CONFIG_CENTER_SOAK_UPDATES")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(1_000);
        let root = std::env::temp_dir().join(format!(
            "roze-config-center-soak-{}-{}",
            std::process::id(),
            current_millis()
        ));
        let snapshot = root.join("admin.json");
        let operator = ConfigPrincipal::new(
            "operator",
            [
                ConfigPermission::Read,
                ConfigPermission::Write,
                ConfigPermission::Rollback,
                ConfigPermission::Audit,
                ConfigPermission::WatchStatus,
            ],
        );
        let mut store = ConfigCenterAdminStore::new(
            ConfigFormat::Yaml,
            "name: v0\ngateway:\n  timeout_ms: 1000\n",
            ConfigAdminMetadata::default(),
            "system",
        )
        .expect("create store");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(seconds);
        let mut accepted = 0u64;
        let mut rejected = 0u64;
        let mut rollbacks = 0u64;

        while std::time::Instant::now() < deadline && accepted < max_updates {
            let update = accepted + 1;
            store
                .publish(
                    ConfigChangeRequest {
                        actor: operator.clone(),
                        reason: Some(format!("soak update {update}")),
                    },
                    format!(
                        "name: v{update}\ngateway:\n  timeout_ms: {}\n",
                        1000 + update
                    ),
                    ConfigAdminMetadata::default(),
                )
                .expect("publish valid config");
            accepted += 1;

            if update.is_multiple_of(17) {
                let invalid = store.publish(
                    ConfigChangeRequest {
                        actor: operator.clone(),
                        reason: Some("invalid payload".to_string()),
                    },
                    "name: [broken\n",
                    ConfigAdminMetadata::default(),
                );
                assert!(invalid.is_err());
                rejected += 1;
            }

            if update.is_multiple_of(29) {
                store
                    .rollback(
                        ConfigChangeRequest {
                            actor: operator.clone(),
                            reason: Some("soak rollback".to_string()),
                        },
                        1,
                    )
                    .expect("rollback");
                rollbacks += 1;
            }

            store.set_watch_status(ConfigWatchStatus {
                mode: ConfigWatchMode::Polling,
                source: "soak".to_string(),
                last_revision: Some(update as i64),
                last_error: None,
                updated_at_millis: current_millis(),
            });
            store.save_to_path(&snapshot).expect("save snapshot");
            store = ConfigCenterAdminStore::load_from_path(&snapshot).expect("load snapshot");
            let status = store.watch_status(&operator).expect("watch status");
            assert_eq!(status.last_revision, Some(update as i64));
        }

        let audit = store.audit_log(&operator).expect("audit");
        println!(
            "roze_config_center_soak accepted={accepted} rejected={rejected} rollbacks={rollbacks} versions={} audit_records={}",
            store.versions().len(),
            audit.len()
        );
        assert!(accepted > 0, "soak must publish at least one valid update");
        assert!(audit
            .iter()
            .any(|record| record.result == ConfigAuditResult::Allowed));
        if rejected > 0 {
            assert!(audit
                .iter()
                .any(|record| record.result == ConfigAuditResult::Rejected));
        }

        std::fs::remove_dir_all(root).expect("remove temp root");
    }

    fn section_hash(signatures: &[ConfigSectionSignature], section: &str) -> Option<String> {
        signatures
            .iter()
            .find(|signature| signature.section == section)
            .map(|signature| signature.hash.clone())
    }
}
