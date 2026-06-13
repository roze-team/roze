use std::path::{Path, PathBuf};
use std::time::Duration;

pub type Config = roze_config::ServiceConfig;

pub fn load(path: impl AsRef<Path>) -> Result<Config, config::ConfigError> {
    roze_config::load(path)
}

pub async fn load_with_config_center(path: impl AsRef<Path>) -> anyhow::Result<Config> {
    Ok(load_with_config_center_with_center(path).await?.0)
}

pub async fn load_with_config_center_with_center(
    path: impl AsRef<Path>,
) -> anyhow::Result<(Config, Option<roze_config::ConfigCenter<Config>>)> {
    let path = path.as_ref();
    let center_input = match parse_config_center_from_env(path) {
        Some(center_input) => center_input,
        None => {
            return load(path)
                .map_err(anyhow::Error::from)
                .map(|config| (config, None));
        }
    };

    let mut subscriber = roze_config::CascadingSubscriber::new();
    if let Some(endpoints) = center_input.endpoints {
        subscriber.push(roze_config::EtcdSubscriber::new(
            endpoints,
            center_input.key.clone(),
        ));
    }
    if let Some(env_key) = center_input.env_key {
        subscriber.push(roze_config::EnvVarSubscriber::new(env_key));
    }
    for file_path in center_input.file_paths {
        subscriber.push(roze_config::FileConfigSubscriber::new(file_path));
    }

    let center = roze_config::ConfigCenter::new(subscriber, center_input.options).await?;
    let current = center.get_config().await;
    Ok((current, Some(center)))
}

#[derive(Debug, Clone)]
struct ConfigCenterInput {
    endpoints: Option<Vec<String>>,
    env_key: Option<String>,
    key: String,
    file_paths: Vec<PathBuf>,
    options: roze_config::ConfigCenterConfig,
}

fn parse_config_center_from_env(path: &Path) -> Option<ConfigCenterInput> {
    let namespace = std::env::var("ROZE_CONFIG_CENTER_NAMESPACE").ok();
    let app = std::env::var("ROZE_CONFIG_CENTER_APP").ok();
    let env_key = std::env::var("ROZE_CONFIG_CENTER_ENV_KEY")
        .ok()
        .filter(|value| !value.trim().is_empty());
    let key = std::env::var("ROZE_CONFIG_CENTER_KEY")
        .ok()
        .or_else(|| std::env::var("ROZE_CONFIG_CENTER_ETCD_KEY").ok())
        .or_else(|| {
            namespace
                .as_ref()
                .zip(app.as_ref())
                .map(|(namespace, app)| format!("{namespace}/{app}"))
        })
        .or_else(|| app.clone())?;

    let endpoints = std::env::var("ROZE_CONFIG_CENTER_ETCD_ENDPOINTS")
        .ok()
        .and_then(|value| split_endpoints(&value));

    let configured_file = std::env::var("ROZE_CONFIG_CENTER_FILE")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| path.to_path_buf());
    let fallback = PathBuf::from("config.yaml");
    let file_paths = dedupe_file_candidates(&[configured_file, fallback]);

    if file_paths.is_empty() && endpoints.is_none() && env_key.is_none() {
        return None;
    }

    let format = std::env::var("ROZE_CONFIG_CENTER_FORMAT")
        .ok()
        .and_then(|value| value.parse::<roze_config::ConfigFormat>().ok())
        .unwrap_or(roze_config::ConfigFormat::Yaml);

    let poll_interval = std::env::var("ROZE_CONFIG_CENTER_POLL_SECS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map_or(Duration::from_secs(5), Duration::from_secs);

    let debounce = std::env::var("ROZE_CONFIG_CENTER_DEBOUNCE_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map_or(Duration::from_millis(400), Duration::from_millis);

    let source = if endpoints.is_some() {
        "etcd".to_string()
    } else if env_key.is_some() {
        "env".to_string()
    } else {
        "file".to_string()
    };

    Some(ConfigCenterInput {
        endpoints,
        env_key,
        key: key.clone(),
        file_paths,
        options: roze_config::ConfigCenterConfig {
            format,
            poll_interval,
            debounce,
            source: Some(source),
            namespace,
            app,
            key: Some(key.clone()),
        },
    })
}

fn dedupe_file_candidates(paths: &[PathBuf]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for path in paths {
        if path.as_os_str().is_empty() {
            continue;
        }
        if path.exists() && !out.contains(path) {
            out.push(path.clone());
        }
    }
    out
}

fn split_endpoints(raw: &str) -> Option<Vec<String>> {
    let endpoints = raw
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();

    if endpoints.is_empty() {
        None
    } else {
        Some(endpoints)
    }
}
