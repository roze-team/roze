use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StorageProvider {
    Local,
    S3Compatible,
    QiniuKodo,
    AliyunOss,
    TencentCos,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StorageConfig {
    pub provider: StorageProvider,
    pub bucket: String,
    #[serde(default)]
    pub endpoint: Option<String>,
    #[serde(default)]
    pub region: Option<String>,
    #[serde(default)]
    pub access_key: Option<String>,
    #[serde(default)]
    pub secret_key: Option<String>,
    #[serde(default)]
    pub public_base_url: Option<String>,
    #[serde(default = "default_root")]
    pub root: PathBuf,
    #[serde(default)]
    pub tenant_prefix: Option<String>,
    #[serde(default)]
    pub validation: StorageValidation,
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            provider: StorageProvider::Local,
            bucket: "roze".to_string(),
            endpoint: None,
            region: None,
            access_key: None,
            secret_key: None,
            public_base_url: None,
            root: default_root(),
            tenant_prefix: None,
            validation: StorageValidation::default(),
        }
    }
}

fn default_root() -> PathBuf {
    PathBuf::from("storage")
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StorageValidation {
    #[serde(default = "default_max_size_bytes")]
    pub max_size_bytes: usize,
    #[serde(default = "default_allowed_image_mimes")]
    pub allowed_mime_types: Vec<String>,
    #[serde(default = "default_allowed_image_extensions")]
    pub allowed_extensions: Vec<String>,
}

impl Default for StorageValidation {
    fn default() -> Self {
        Self {
            max_size_bytes: default_max_size_bytes(),
            allowed_mime_types: default_allowed_image_mimes(),
            allowed_extensions: default_allowed_image_extensions(),
        }
    }
}

fn default_max_size_bytes() -> usize {
    10 * 1024 * 1024
}

fn default_allowed_image_mimes() -> Vec<String> {
    ["image/jpeg", "image/png", "image/webp", "image/gif"]
        .into_iter()
        .map(ToString::to_string)
        .collect()
}

fn default_allowed_image_extensions() -> Vec<String> {
    ["jpg", "jpeg", "png", "webp", "gif"]
        .into_iter()
        .map(ToString::to_string)
        .collect()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PutObjectRequest {
    pub key: String,
    pub bytes: Vec<u8>,
    #[serde(default)]
    pub content_type: Option<String>,
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
}

impl PutObjectRequest {
    pub fn image(key: impl Into<String>, bytes: Vec<u8>, content_type: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            bytes,
            content_type: Some(content_type.into()),
            metadata: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ObjectInfo {
    pub provider: StorageProvider,
    pub bucket: String,
    pub key: String,
    pub size: u64,
    pub content_type: Option<String>,
    pub etag: String,
    pub url: Option<String>,
    pub metadata: BTreeMap<String, String>,
    pub updated_at_millis: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PresignedUrl {
    pub method: String,
    pub url: String,
    pub expires_at_millis: u64,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
}

pub type FileMetadata = ObjectInfo;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UploadPolicy {
    pub max_size_bytes: usize,
    #[serde(default)]
    pub allowed_mime_types: Vec<String>,
}

impl Default for UploadPolicy {
    fn default() -> Self {
        Self {
            max_size_bytes: default_max_size_bytes(),
            allowed_mime_types: default_allowed_image_mimes(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UploadToken {
    pub key: String,
    pub expires_at_millis: u64,
    pub max_size_bytes: usize,
    pub allowed_mime_types: Vec<String>,
    pub upload: PresignedUrl,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MediaUrl {
    pub key: String,
    pub url: String,
    pub expires_at_millis: Option<u64>,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
}

#[async_trait]
pub trait ObjectStorage: std::fmt::Debug + Send + Sync + 'static {
    async fn put_object(&self, request: PutObjectRequest) -> anyhow::Result<ObjectInfo>;
    async fn get_object(&self, key: &str) -> anyhow::Result<Vec<u8>>;
    async fn delete_object(&self, key: &str) -> anyhow::Result<()>;
    async fn stat_object(&self, key: &str) -> anyhow::Result<Option<ObjectInfo>>;
    async fn presign_put(&self, key: &str, expires: Duration) -> anyhow::Result<PresignedUrl>;
    async fn presign_get(&self, key: &str, expires: Duration) -> anyhow::Result<PresignedUrl>;
}

pub async fn issue_upload_token(
    storage: &dyn ObjectStorage,
    key: &str,
    expires: Duration,
    policy: UploadPolicy,
) -> anyhow::Result<UploadToken> {
    if policy.max_size_bytes == 0 {
        anyhow::bail!("upload max_size_bytes must be greater than zero");
    }
    let key = normalize_object_key(key)?;
    let upload = storage.presign_put(&key, expires).await?;
    Ok(UploadToken {
        key,
        expires_at_millis: upload.expires_at_millis,
        max_size_bytes: policy.max_size_bytes,
        allowed_mime_types: policy.allowed_mime_types,
        upload,
    })
}

pub async fn resolve_media_url(
    storage: &dyn ObjectStorage,
    key: &str,
    expires: Duration,
) -> anyhow::Result<MediaUrl> {
    let key = normalize_object_key(key)?;
    if let Some(info) = storage.stat_object(&key).await? {
        if let Some(url) = info.url {
            return Ok(MediaUrl {
                key,
                url,
                expires_at_millis: None,
                headers: BTreeMap::new(),
            });
        }
    }
    let signed = storage.presign_get(&key, expires).await?;
    Ok(MediaUrl {
        key,
        url: signed.url,
        expires_at_millis: Some(signed.expires_at_millis),
        headers: signed.headers,
    })
}

#[derive(Debug, Clone)]
pub struct LocalObjectStorage {
    config: StorageConfig,
}

impl LocalObjectStorage {
    pub fn new(config: StorageConfig) -> Self {
        Self { config }
    }

    fn path_for(&self, key: &str) -> anyhow::Result<PathBuf> {
        let key = normalize_object_key(key)?;
        Ok(self.config.root.join(&self.config.bucket).join(key))
    }

    fn public_url(&self, key: &str) -> Option<String> {
        self.config
            .public_base_url
            .as_ref()
            .map(|base| format!("{}/{}", base.trim_end_matches('/'), key))
    }
}

#[async_trait]
impl ObjectStorage for LocalObjectStorage {
    async fn put_object(&self, request: PutObjectRequest) -> anyhow::Result<ObjectInfo> {
        validate_upload(&self.config.validation, &request)?;
        let key = normalize_object_key(with_tenant_prefix(
            self.config.tenant_prefix.as_deref(),
            &request.key,
        ))?;
        let path = self.path_for(&key)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, &request.bytes)?;
        Ok(ObjectInfo {
            provider: StorageProvider::Local,
            bucket: self.config.bucket.clone(),
            key: key.clone(),
            size: request.bytes.len() as u64,
            content_type: request.content_type,
            etag: weak_etag(&request.bytes),
            url: self.public_url(&key),
            metadata: request.metadata,
            updated_at_millis: current_millis(),
        })
    }

    async fn get_object(&self, key: &str) -> anyhow::Result<Vec<u8>> {
        let path = self.path_for(key)?;
        Ok(fs::read(path)?)
    }

    async fn delete_object(&self, key: &str) -> anyhow::Result<()> {
        let path = self.path_for(key)?;
        match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(err.into()),
        }
    }

    async fn stat_object(&self, key: &str) -> anyhow::Result<Option<ObjectInfo>> {
        let normalized = normalize_object_key(key)?;
        let path = self.path_for(&normalized)?;
        let metadata = match fs::metadata(path) {
            Ok(metadata) => metadata,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(err.into()),
        };
        Ok(Some(ObjectInfo {
            provider: StorageProvider::Local,
            bucket: self.config.bucket.clone(),
            key: normalized.clone(),
            size: metadata.len(),
            content_type: mime_from_extension(&normalized).map(ToString::to_string),
            etag: format!("size-{}", metadata.len()),
            url: self.public_url(&normalized),
            metadata: BTreeMap::new(),
            updated_at_millis: current_millis(),
        }))
    }

    async fn presign_put(&self, key: &str, expires: Duration) -> anyhow::Result<PresignedUrl> {
        local_presign("PUT", &self.config, key, expires)
    }

    async fn presign_get(&self, key: &str, expires: Duration) -> anyhow::Result<PresignedUrl> {
        local_presign("GET", &self.config, key, expires)
    }
}

#[derive(Debug, Clone)]
pub struct CloudObjectStorage {
    config: StorageConfig,
}

impl CloudObjectStorage {
    pub fn new(config: StorageConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl ObjectStorage for CloudObjectStorage {
    async fn put_object(&self, _request: PutObjectRequest) -> anyhow::Result<ObjectInfo> {
        anyhow::bail!(
            "{:?} runtime upload is not wired yet; use presign_* or provider SDK adapter",
            self.config.provider
        )
    }

    async fn get_object(&self, _key: &str) -> anyhow::Result<Vec<u8>> {
        anyhow::bail!(
            "{:?} runtime download is not wired yet; use presign_* or provider SDK adapter",
            self.config.provider
        )
    }

    async fn delete_object(&self, _key: &str) -> anyhow::Result<()> {
        anyhow::bail!(
            "{:?} runtime delete is not wired yet; use provider SDK adapter",
            self.config.provider
        )
    }

    async fn stat_object(&self, _key: &str) -> anyhow::Result<Option<ObjectInfo>> {
        anyhow::bail!(
            "{:?} runtime stat is not wired yet; use provider SDK adapter",
            self.config.provider
        )
    }

    async fn presign_put(&self, key: &str, expires: Duration) -> anyhow::Result<PresignedUrl> {
        unsigned_cloud_url("PUT", &self.config, key, expires)
    }

    async fn presign_get(&self, key: &str, expires: Duration) -> anyhow::Result<PresignedUrl> {
        unsigned_cloud_url("GET", &self.config, key, expires)
    }
}

pub fn build_storage(config: StorageConfig) -> anyhow::Result<Box<dyn ObjectStorage>> {
    match config.provider {
        StorageProvider::Local => Ok(Box::new(LocalObjectStorage::new(config))),
        StorageProvider::S3Compatible
        | StorageProvider::QiniuKodo
        | StorageProvider::AliyunOss
        | StorageProvider::TencentCos => Ok(Box::new(CloudObjectStorage::new(config))),
    }
}

pub fn normalize_object_key(key: impl AsRef<str>) -> anyhow::Result<String> {
    let raw = key.as_ref().trim().replace('\\', "/");
    if raw.is_empty() {
        anyhow::bail!("object key is empty");
    }
    if raw.starts_with('/') || raw.contains('\0') {
        anyhow::bail!("invalid object key: {raw}");
    }
    let mut parts = Vec::new();
    for part in raw.split('/') {
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." {
            anyhow::bail!("object key must not contain parent traversal");
        }
        parts.push(sanitize_path_segment(part)?);
    }
    if parts.is_empty() {
        anyhow::bail!("object key is empty");
    }
    Ok(parts.join("/"))
}

pub fn validate_upload(
    validation: &StorageValidation,
    request: &PutObjectRequest,
) -> anyhow::Result<()> {
    if request.bytes.is_empty() {
        anyhow::bail!("upload body is empty");
    }
    if request.bytes.len() > validation.max_size_bytes {
        anyhow::bail!(
            "upload body too large: {} > {}",
            request.bytes.len(),
            validation.max_size_bytes
        );
    }
    let key = normalize_object_key(&request.key)?;
    let extension = extension_of(&key).unwrap_or_default();
    if !validation.allowed_extensions.is_empty()
        && !validation
            .allowed_extensions
            .iter()
            .any(|item| item.eq_ignore_ascii_case(extension))
    {
        anyhow::bail!("unsupported file extension: {extension}");
    }
    if let Some(content_type) = request.content_type.as_deref() {
        if !validation.allowed_mime_types.is_empty()
            && !validation
                .allowed_mime_types
                .iter()
                .any(|item| item.eq_ignore_ascii_case(content_type))
        {
            anyhow::bail!("unsupported content type: {content_type}");
        }
    }
    Ok(())
}

fn with_tenant_prefix(prefix: Option<&str>, key: &str) -> String {
    match prefix {
        Some(prefix) if !prefix.trim().is_empty() => format!("{}/{}", prefix.trim(), key),
        _ => key.to_string(),
    }
}

fn sanitize_path_segment(segment: &str) -> anyhow::Result<String> {
    let sanitized: String = segment
        .chars()
        .map(|ch| match ch {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.' => ch,
            _ => '_',
        })
        .collect();
    if sanitized.is_empty() {
        anyhow::bail!("empty path segment");
    }
    Ok(sanitized)
}

fn extension_of(key: &str) -> Option<&str> {
    Path::new(key).extension().and_then(|value| value.to_str())
}

fn mime_from_extension(key: &str) -> Option<&'static str> {
    match extension_of(key)?.to_ascii_lowercase().as_str() {
        "jpg" | "jpeg" => Some("image/jpeg"),
        "png" => Some("image/png"),
        "webp" => Some("image/webp"),
        "gif" => Some("image/gif"),
        _ => None,
    }
}

fn local_presign(
    method: &str,
    config: &StorageConfig,
    key: &str,
    expires: Duration,
) -> anyhow::Result<PresignedUrl> {
    let key = normalize_object_key(key)?;
    let base = config
        .public_base_url
        .clone()
        .unwrap_or_else(|| format!("local://{}", config.bucket));
    Ok(PresignedUrl {
        method: method.to_string(),
        url: format!("{}/{}", base.trim_end_matches('/'), key),
        expires_at_millis: current_millis().saturating_add(expires.as_millis() as u64),
        headers: BTreeMap::new(),
    })
}

fn unsigned_cloud_url(
    method: &str,
    config: &StorageConfig,
    key: &str,
    expires: Duration,
) -> anyhow::Result<PresignedUrl> {
    let key = normalize_object_key(key)?;
    let base = config
        .public_base_url
        .clone()
        .or_else(|| {
            config
                .endpoint
                .as_ref()
                .map(|endpoint| format!("{}/{}", endpoint.trim_end_matches('/'), config.bucket))
        })
        .ok_or_else(|| anyhow::anyhow!("missing public_base_url or endpoint"))?;
    Ok(PresignedUrl {
        method: method.to_string(),
        url: format!("{}/{}", base.trim_end_matches('/'), key),
        expires_at_millis: current_millis().saturating_add(expires.as_millis() as u64),
        headers: provider_headers(config),
    })
}

fn provider_headers(config: &StorageConfig) -> BTreeMap<String, String> {
    let mut headers = BTreeMap::new();
    headers.insert(
        "x-roze-storage-provider".to_string(),
        format!("{:?}", config.provider),
    );
    if let Some(region) = &config.region {
        headers.insert("x-roze-storage-region".to_string(), region.clone());
    }
    headers
}

fn weak_etag(bytes: &[u8]) -> String {
    let mut hash: u64 = 14_695_981_039_346_656_037;
    for byte in bytes {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(1_099_511_628_211);
    }
    format!("{hash:x}")
}

fn current_millis() -> u64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(elapsed) => elapsed.as_millis() as u64,
        Err(_) => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_object_keys() {
        assert_eq!(
            normalize_object_key("avatars/user 1/../x.png")
                .unwrap_err()
                .to_string(),
            "object key must not contain parent traversal"
        );
        assert_eq!(
            normalize_object_key("avatars/user 1/a.png").unwrap(),
            "avatars/user_1/a.png"
        );
    }

    #[tokio::test]
    async fn local_storage_put_get_stat_delete() {
        let root = std::env::temp_dir().join(format!("roze-storage-{}", uuid::Uuid::now_v7()));
        let storage = LocalObjectStorage::new(StorageConfig {
            provider: StorageProvider::Local,
            bucket: "test".to_string(),
            root: root.clone(),
            public_base_url: Some("http://localhost/files".to_string()),
            ..Default::default()
        });

        let info = storage
            .put_object(PutObjectRequest::image(
                "avatars/a.png",
                vec![1, 2, 3],
                "image/png",
            ))
            .await
            .expect("put");
        assert_eq!(info.key, "avatars/a.png");
        assert_eq!(
            info.url.as_deref(),
            Some("http://localhost/files/avatars/a.png")
        );
        assert_eq!(
            storage.get_object("avatars/a.png").await.unwrap(),
            vec![1, 2, 3]
        );
        assert_eq!(
            storage
                .stat_object("avatars/a.png")
                .await
                .unwrap()
                .unwrap()
                .size,
            3
        );
        storage.delete_object("avatars/a.png").await.unwrap();
        assert!(storage
            .stat_object("avatars/a.png")
            .await
            .unwrap()
            .is_none());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn validates_image_upload() {
        let validation = StorageValidation::default();
        let request = PutObjectRequest::image("a.txt", vec![1], "text/plain");
        assert!(validate_upload(&validation, &request).is_err());
    }

    #[tokio::test]
    async fn cloud_provider_presign_uses_provider_metadata() {
        let storage = CloudObjectStorage::new(StorageConfig {
            provider: StorageProvider::AliyunOss,
            bucket: "bucket".to_string(),
            endpoint: Some("https://oss-cn-hangzhou.aliyuncs.com".to_string()),
            region: Some("cn-hangzhou".to_string()),
            ..Default::default()
        });
        let url = storage
            .presign_get("images/a.png", Duration::from_secs(60))
            .await
            .expect("presign");
        assert!(url.url.contains("bucket/images/a.png"));
        assert_eq!(
            url.headers
                .get("x-roze-storage-provider")
                .map(String::as_str),
            Some("AliyunOss")
        );
    }

    #[tokio::test]
    async fn upload_tokens_and_media_urls_use_storage_contract() {
        let root =
            std::env::temp_dir().join(format!("roze-storage-token-{}", uuid::Uuid::now_v7()));
        let storage = LocalObjectStorage::new(StorageConfig {
            provider: StorageProvider::Local,
            bucket: "test".to_string(),
            root: root.clone(),
            public_base_url: Some("http://localhost/files".to_string()),
            ..Default::default()
        });

        let token = issue_upload_token(
            &storage,
            "avatars/a.png",
            Duration::from_secs(60),
            UploadPolicy::default(),
        )
        .await
        .expect("upload token");
        assert_eq!(token.key, "avatars/a.png");
        assert_eq!(token.upload.method, "PUT");

        storage
            .put_object(PutObjectRequest::image(
                "avatars/a.png",
                vec![1, 2, 3],
                "image/png",
            ))
            .await
            .expect("put");
        let media = resolve_media_url(&storage, "avatars/a.png", Duration::from_secs(60))
            .await
            .expect("media url");
        assert_eq!(media.url, "http://localhost/files/avatars/a.png");
        assert!(media.expires_at_millis.is_none());
        let _ = fs::remove_dir_all(root);
    }
}
