# Config center deployment

The service starts from `ROZE_CONFIG_PATH` (or `config.yaml`) and only enables
runtime configuration watching when `ROZE_CONFIG_CENTER_KEY` is set. There are
no legacy aliases or inferred keys.

Choose at least one source:

- `ROZE_CONFIG_CENTER_ETCD_ENDPOINTS`: comma-separated etcd endpoints.
- `ROZE_CONFIG_CENTER_ENV_KEY`: environment variable containing the document.
- `ROZE_CONFIG_CENTER_FILE`: watched configuration file. When omitted, the
  resolved service configuration file is used as the local fallback.

Optional settings:

- `ROZE_CONFIG_CENTER_NAMESPACE` and `ROZE_CONFIG_CENTER_APP`: audit metadata.
- `ROZE_CONFIG_CENTER_FORMAT`: `yaml`, `json`, or `toml`; defaults to `yaml`.
- `ROZE_CONFIG_CENTER_POLL_SECS`: source polling interval; defaults to `5`.
- `ROZE_CONFIG_CENTER_DEBOUNCE_MS`: reload debounce; defaults to `400`.
- `ROZE_CONFIG_CENTER_LISTENER_TIMEOUT_MS`: listener budget; defaults to `500`.

Every snapshot is validated before publication. A rejected snapshot keeps the
last valid Kafka runtime. A valid Kafka-section change restarts the producer and
consumer pipeline under the shared service shutdown lifecycle.
