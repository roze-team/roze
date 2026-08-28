use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::config::Config;

pub async fn open(
    service_config_path: impl AsRef<Path>,
) -> anyhow::Result<Option<roze_config::ConfigCenter<Config>>> {
    let Some(input) = ConfigCenterInput::from_env(service_config_path.as_ref())? else {
        return Ok(None);
    };

    let mut subscriber = roze_config::CascadingSubscriber::new();
    if let Some(endpoints) = input.endpoints {
        subscriber.push(roze_config::EtcdSubscriber::new(endpoints, input.key));
    }
    if let Some(env_key) = input.env_key {
        subscriber.push(roze_config::EnvVarSubscriber::new(env_key));
    }
    if let Some(file_path) = input.file_path {
        subscriber.push(roze_config::FileConfigSubscriber::new(file_path));
    }

    let center = roze_config::ConfigCenter::new_with_validator(
        subscriber,
        input.options,
        |config: &Config| config.validate(),
    )
    .await?;
    Ok(Some(center))
}

struct ConfigCenterInput {
    endpoints: Option<Vec<String>>,
    env_key: Option<String>,
    key: String,
    file_path: Option<PathBuf>,
    options: roze_config::ConfigCenterConfig,
}

impl ConfigCenterInput {
    fn from_env(service_config_path: &Path) -> anyhow::Result<Option<Self>> {
        let Some(key) = non_empty_env("ROZE_CONFIG_CENTER_KEY") else {
            return Ok(None);
        };

        let endpoints = non_empty_env("ROZE_CONFIG_CENTER_ETCD_ENDPOINTS")
            .map(|raw| split_csv(&raw))
            .transpose()?;
        let env_key = non_empty_env("ROZE_CONFIG_CENTER_ENV_KEY");
        let file_path = non_empty_env("ROZE_CONFIG_CENTER_FILE")
            .map(PathBuf::from)
            .or_else(|| {
                service_config_path
                    .exists()
                    .then(|| service_config_path.to_path_buf())
            });
        if endpoints.is_none() && env_key.is_none() && file_path.is_none() {
            anyhow::bail!(
                "config center requires at least one of ROZE_CONFIG_CENTER_ETCD_ENDPOINTS, \
                 ROZE_CONFIG_CENTER_ENV_KEY, or ROZE_CONFIG_CENTER_FILE"
            );
        }

        let namespace = non_empty_env("ROZE_CONFIG_CENTER_NAMESPACE");
        let app = non_empty_env("ROZE_CONFIG_CENTER_APP");
        let format = non_empty_env("ROZE_CONFIG_CENTER_FORMAT")
            .map(|value| value.parse::<roze_config::ConfigFormat>())
            .transpose()?
            .unwrap_or(roze_config::ConfigFormat::Yaml);
        let poll_interval = duration_from_env("ROZE_CONFIG_CENTER_POLL_SECS", 5, 1_000)?;
        let debounce = duration_from_env("ROZE_CONFIG_CENTER_DEBOUNCE_MS", 400, 1)?;
        let listener_timeout = duration_from_env("ROZE_CONFIG_CENTER_LISTENER_TIMEOUT_MS", 500, 1)?;
        let source = match (&endpoints, &env_key) {
            (Some(_), _) => "etcd",
            (None, Some(_)) => "env",
            (None, None) => "file",
        };

        Ok(Some(Self {
            endpoints,
            env_key,
            key: key.clone(),
            file_path,
            options: roze_config::ConfigCenterConfig {
                format,
                poll_interval,
                debounce,
                listener_timeout,
                source: Some(source.to_string()),
                namespace,
                app,
                key: Some(key),
            },
        }))
    }
}

fn non_empty_env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn split_csv(raw: &str) -> anyhow::Result<Vec<String>> {
    let values = raw
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    if values.is_empty() {
        anyhow::bail!("ROZE_CONFIG_CENTER_ETCD_ENDPOINTS must contain an endpoint");
    }
    Ok(values)
}

fn duration_from_env(name: &str, default: u64, multiplier_ms: u64) -> anyhow::Result<Duration> {
    let value = non_empty_env(name)
        .map(|raw| {
            raw.parse::<u64>()
                .map_err(|error| anyhow::anyhow!("invalid {name}: {error}"))
        })
        .transpose()?
        .unwrap_or(default);
    Ok(Duration::from_millis(value.saturating_mul(multiplier_ms)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_csv_rejects_empty_values() {
        assert!(split_csv(" , ").is_err());
        assert_eq!(split_csv("a:1, b:2").unwrap(), ["a:1", "b:2"]);
    }
}
