use std::{
    future::Future,
    hash::{Hash, Hasher},
    path::PathBuf,
    pin::Pin,
    str::FromStr,
    string::ToString,
    sync::Arc as StdArc,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{anyhow, Result};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde::{de::DeserializeOwned, Deserialize};
use serde_json::json;
use tokio::{
    sync::RwLock,
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
        match value.to_ascii_lowercase().as_str() {
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

impl<T> ReloadResult<T>
where
    T: Clone,
{
    fn success(
        version: u64,
        old_version: u64,
        hash: String,
        old_hash: String,
        namespace: Option<String>,
        app: Option<String>,
        key: Option<String>,
        source: String,
        config: T,
    ) -> Self {
        Self {
            version,
            old_version,
            hash: hash.clone(),
            old_hash: old_hash.clone(),
            ts_millis: current_millis(),
            source,
            namespace,
            app,
            key,
            changed: old_hash != hash,
            success: true,
            error: None,
            config: Some(config),
        }
    }

    fn failed(
        version: u64,
        old_version: u64,
        hash: String,
        old_hash: String,
        namespace: Option<String>,
        app: Option<String>,
        key: Option<String>,
        source: String,
        error: impl Into<String>,
    ) -> Self {
        Self {
            version,
            old_version,
            hash,
            old_hash,
            ts_millis: current_millis(),
            source,
            namespace,
            app,
            key,
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

    loop {
        time::sleep(options.poll_interval).await;

        let snapshot = match subscriber.value().await {
            Ok(raw) => raw,
            Err(err) => {
                warn!(%err, source = %source, "read config center value failed");
                continue;
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
                    old_version + 1,
                    old_version,
                    snapshot_hash,
                    last_hash.clone(),
                    options.namespace.clone(),
                    options.app.clone(),
                    options.key.clone(),
                    source.clone(),
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
            next_version,
            old_version,
            last_hash.clone(),
            old_hash,
            options.namespace.clone(),
            options.app.clone(),
            options.key.clone(),
            source.clone(),
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

fn normalize_endpoint(endpoint: &str) -> String {
    let endpoint = endpoint.trim().trim_end_matches('/');
    if endpoint.starts_with("http://") || endpoint.starts_with("https://") {
        endpoint.to_string()
    } else {
        format!("http://{endpoint}")
    }
}

#[derive(Debug, Deserialize)]
struct EtcdRangeResponse {
    #[serde(default)]
    kvs: Vec<EtcdKv>,
}

#[derive(Debug, Deserialize)]
struct EtcdKv {
    value: String,
}
