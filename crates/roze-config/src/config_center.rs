use std::{
    future::Future,
    hash::{Hash, Hasher},
    path::PathBuf,
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
use serde::{de::DeserializeOwned, Deserialize};
use serde_json::json;
use tokio::{
    sync::{mpsc, RwLock},
    time::{self, Instant},
};
use tracing::warn;

#[derive(Debug, Clone, Copy)]
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
    pub success: bool,
    pub error: Option<String>,
    pub config: Option<T>,
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
    fn success(meta: ReloadMetadata, config: T) -> Self {
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
            source: None,
            namespace: None,
            app: None,
            key: None,
        }
    }
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
    last_snapshot: String,
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

                let listeners = inner.reload_listeners.read().await.clone();
                for listener in listeners {
                    listener(&result);
                }
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

        {
            *inner.value.write().await = parsed.clone();
            last_hash = snapshot_hash.clone();
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
        );

        if result.changed {
            let listeners = inner.listeners.read().await.clone();
            for listener in listeners {
                listener(&parsed);
            }
        }

        let reload_listeners = inner.reload_listeners.read().await.clone();
        for listener in reload_listeners {
            listener(&result);
        }

        pending = None;
        tracing::info!(
            source = %source,
            version = result.version,
            old_version = result.old_version,
            hash = %result.hash,
            changed = result.changed,
            success = result.success,
            "config center reload applied"
        );
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
        .await?
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
}
