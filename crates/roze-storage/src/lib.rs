use std::{
    collections::BTreeMap,
    fmt, fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use async_trait::async_trait;
use hmac::{Hmac, KeyInit, Mac};
use percent_encoding::{percent_encode, AsciiSet, CONTROLS};
use reqwest::{header, Client, StatusCode, Url};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

type HmacSha256 = Hmac<Sha256>;
const PATH_ENCODE_SET: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'<')
    .add(b'>')
    .add(b'`')
    .add(b'?')
    .add(b'#');

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StorageProvider {
    Local,
    S3Compatible,
    QiniuKodo,
    AliyunOss,
    TencentCos,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
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

impl fmt::Debug for StorageConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StorageConfig")
            .field("provider", &self.provider)
            .field("bucket", &self.bucket)
            .field("endpoint", &self.endpoint.as_ref().map(|_| "[REDACTED]"))
            .field("region", &self.region)
            .field(
                "access_key",
                &self.access_key.as_ref().map(|_| "[REDACTED]"),
            )
            .field(
                "secret_key",
                &self.secret_key.as_ref().map(|_| "[REDACTED]"),
            )
            .field(
                "public_base_url",
                &self.public_base_url.as_ref().map(|_| "[REDACTED]"),
            )
            .field("root", &self.root)
            .field("tenant_prefix", &self.tenant_prefix)
            .field("validation", &self.validation)
            .finish()
    }
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

    fn ensure_s3(&self) -> anyhow::Result<()> {
        if self.config.provider != StorageProvider::S3Compatible {
            anyhow::bail!(
                "{:?} runtime operations are not supported; configure an SDK adapter",
                self.config.provider
            );
        }
        if self.config.endpoint.is_none() {
            anyhow::bail!("S3-compatible storage requires endpoint");
        }
        if self
            .config
            .access_key
            .as_deref()
            .unwrap_or_default()
            .is_empty()
            || self
                .config
                .secret_key
                .as_deref()
                .unwrap_or_default()
                .is_empty()
        {
            anyhow::bail!("S3-compatible storage requires access_key and secret_key");
        }
        Ok(())
    }

    fn object_key(&self, key: &str) -> anyhow::Result<String> {
        normalize_object_key(with_tenant_prefix(
            self.config.tenant_prefix.as_deref(),
            key,
        ))
    }

    fn object_url(&self, key: &str) -> anyhow::Result<Url> {
        let endpoint = self
            .config
            .endpoint
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("missing endpoint"))?
            .trim_end_matches('/');
        let path = format!("{}/{}", self.config.bucket, key);
        Url::parse(&format!("{}/{}", endpoint, path)).map_err(Into::into)
    }
}

#[async_trait]
impl ObjectStorage for CloudObjectStorage {
    async fn put_object(&self, request: PutObjectRequest) -> anyhow::Result<ObjectInfo> {
        self.ensure_s3()?;
        validate_upload(&self.config.validation, &request)?;
        let key = self.object_key(&request.key)?;
        let body_hash = hex_sha256(&request.bytes);
        let url = self.object_url(&key)?;
        let mut headers = BTreeMap::new();
        headers.insert("x-amz-content-sha256".into(), body_hash.clone());
        if let Some(content_type) = request.content_type.as_deref() {
            headers.insert("content-type".into(), content_type.to_string());
        }
        for (name, value) in &request.metadata {
            headers.insert(
                format!("x-amz-meta-{}", name.to_ascii_lowercase()),
                value.clone(),
            );
        }
        let signed = sign_headers("PUT", &url, &headers, &self.config, &body_hash, None)?;
        let client = s3_client()?;
        let mut builder = client.put(url).body(request.bytes.clone());
        for (name, value) in signed.headers {
            builder = builder.header(name, value);
        }
        let response = ensure_success(builder.send().await?, "put object").await?;
        let etag =
            response_header(&response, "etag").unwrap_or_else(|| format!("\"{}\"", body_hash));
        Ok(ObjectInfo {
            provider: self.config.provider.clone(),
            bucket: self.config.bucket.clone(),
            key: key.clone(),
            size: request.bytes.len() as u64,
            content_type: request.content_type,
            etag,
            url: self
                .config
                .public_base_url
                .as_ref()
                .map(|base| format!("{}/{}", base.trim_end_matches('/'), key)),
            metadata: request.metadata,
            updated_at_millis: current_millis(),
        })
    }

    async fn get_object(&self, key: &str) -> anyhow::Result<Vec<u8>> {
        self.ensure_s3()?;
        let key = self.object_key(key)?;
        let url = self.object_url(&key)?;
        let headers = BTreeMap::from([(
            String::from("x-amz-content-sha256"),
            String::from("UNSIGNED-PAYLOAD"),
        )]);
        let signed = sign_headers(
            "GET",
            &url,
            &headers,
            &self.config,
            "UNSIGNED-PAYLOAD",
            None,
        )?;
        let client = s3_client()?;
        let mut builder = client.get(url);
        for (name, value) in signed.headers {
            builder = builder.header(name, value);
        }
        let response = builder.send().await?;
        let response = ensure_success(response, "get object").await?;
        Ok(response.bytes().await?.to_vec())
    }

    async fn delete_object(&self, key: &str) -> anyhow::Result<()> {
        self.ensure_s3()?;
        let key = self.object_key(key)?;
        let url = self.object_url(&key)?;
        let headers = BTreeMap::from([(String::from("x-amz-content-sha256"), hex_sha256(&[]))]);
        let signed = sign_headers(
            "DELETE",
            &url,
            &headers,
            &self.config,
            &headers["x-amz-content-sha256"],
            None,
        )?;
        let client = s3_client()?;
        let mut builder = client.delete(url);
        for (name, value) in signed.headers {
            builder = builder.header(name, value);
        }
        let response = builder.send().await?;
        let _ = ensure_success(response, "delete object").await?;
        Ok(())
    }

    async fn stat_object(&self, key: &str) -> anyhow::Result<Option<ObjectInfo>> {
        self.ensure_s3()?;
        let key = self.object_key(key)?;
        let url = self.object_url(&key)?;
        let headers = BTreeMap::from([(
            String::from("x-amz-content-sha256"),
            String::from("UNSIGNED-PAYLOAD"),
        )]);
        let signed = sign_headers(
            "HEAD",
            &url,
            &headers,
            &self.config,
            "UNSIGNED-PAYLOAD",
            None,
        )?;
        let client = s3_client()?;
        let mut builder = client.head(url);
        for (name, value) in signed.headers {
            builder = builder.header(name, value);
        }
        let response = builder.send().await?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let response = ensure_success(response, "stat object").await?;
        let size = response
            .headers()
            .get(header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        let content_type = response_header(&response, "content-type");
        let etag = response_header(&response, "etag").unwrap_or_default();
        Ok(Some(ObjectInfo {
            provider: self.config.provider.clone(),
            bucket: self.config.bucket.clone(),
            key,
            size,
            content_type,
            etag,
            url: None,
            metadata: BTreeMap::new(),
            updated_at_millis: current_millis(),
        }))
    }

    async fn presign_put(&self, key: &str, expires: Duration) -> anyhow::Result<PresignedUrl> {
        if self.config.provider != StorageProvider::S3Compatible {
            return unsigned_cloud_url("PUT", &self.config, key, expires);
        }
        self.ensure_s3()?;
        let key = self.object_key(key)?;
        presign_s3("PUT", &self.object_url(&key)?, &self.config, expires)
    }

    async fn presign_get(&self, key: &str, expires: Duration) -> anyhow::Result<PresignedUrl> {
        if self.config.provider != StorageProvider::S3Compatible {
            return unsigned_cloud_url("GET", &self.config, key, expires);
        }
        self.ensure_s3()?;
        let key = self.object_key(key)?;
        presign_s3("GET", &self.object_url(&key)?, &self.config, expires)
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

#[derive(Debug)]
struct SignedRequest {
    headers: BTreeMap<String, String>,
}

fn hex_sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

fn aws_timestamp() -> (String, String) {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let days = seconds / 86_400;
    let rem = seconds % 86_400;
    let (year, month, day) = civil_from_days(days as i64);
    (
        format!("{year:04}{month:02}{day:02}"),
        format!(
            "{year:04}{month:02}{day:02}T{:02}{:02}{:02}Z",
            rem / 3600,
            (rem % 3600) / 60,
            rem % 60
        ),
    )
}

// Howard Hinnant's civil-from-days conversion, valid for the Gregorian calendar.
fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    (y as i32 + if m <= 2 { 1 } else { 0 }, m as u32, d as u32)
}

fn canonical_uri(url: &Url) -> String {
    let path = url.path();
    percent_encode(path.as_bytes(), PATH_ENCODE_SET)
        .to_string()
        .replace("%2F", "/")
}

fn sign_headers(
    method: &str,
    url: &Url,
    input: &BTreeMap<String, String>,
    config: &StorageConfig,
    payload_hash: &str,
    query: Option<&str>,
) -> anyhow::Result<SignedRequest> {
    let access = config
        .access_key
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("missing access_key"))?;
    let secret = config
        .secret_key
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("missing secret_key"))?;
    let region = config.region.as_deref().unwrap_or("us-east-1");
    let (date, timestamp) = aws_timestamp();
    let mut headers = input.clone();
    headers.insert("host".into(), host_header(url));
    headers.insert("x-amz-date".into(), timestamp.clone());
    let signed_headers = headers
        .keys()
        .map(|k| k.to_ascii_lowercase())
        .collect::<Vec<_>>()
        .join(";");
    let canonical_headers = headers
        .iter()
        .map(|(k, v)| format!("{}:{}\n", k.to_ascii_lowercase(), v.trim()))
        .collect::<String>();
    let canonical_query = query.unwrap_or("");
    let canonical_request = format!(
        "{}\n{}\n{}\n{}\n{}\n{}",
        method,
        canonical_uri(url),
        canonical_query,
        canonical_headers,
        signed_headers,
        payload_hash
    );
    let scope = format!("{date}/{region}/s3/aws4_request");
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{timestamp}\n{scope}\n{}",
        hex_sha256(canonical_request.as_bytes())
    );
    let signing_key = derive_signing_key(secret, &date, region, "s3");
    let signature = hmac_hex(&signing_key, string_to_sign.as_bytes());
    headers.insert("authorization".into(), format!("AWS4-HMAC-SHA256 Credential={access}/{scope}, SignedHeaders={signed_headers}, Signature={signature}"));
    Ok(SignedRequest { headers })
}

fn hmac_bytes(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(key).expect("hmac key");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}
fn hmac_hex(key: &[u8], data: &[u8]) -> String {
    hmac_bytes(key, data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
fn derive_signing_key(secret: &str, date: &str, region: &str, service: &str) -> Vec<u8> {
    let k_date = hmac_bytes(format!("AWS4{secret}").as_bytes(), date.as_bytes());
    let k_region = hmac_bytes(&k_date, region.as_bytes());
    let k_service = hmac_bytes(&k_region, service.as_bytes());
    hmac_bytes(&k_service, b"aws4_request")
}

fn presign_s3(
    method: &str,
    url: &Url,
    config: &StorageConfig,
    expires: Duration,
) -> anyhow::Result<PresignedUrl> {
    let access = config
        .access_key
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("missing access_key"))?;
    let secret = config
        .secret_key
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("missing secret_key"))?;
    let region = config.region.as_deref().unwrap_or("us-east-1");
    let (date, timestamp) = aws_timestamp();
    let scope = format!("{date}/{region}/s3/aws4_request");
    let host = host_header(url);
    let signed_headers = "host";
    let expires = expires.as_secs().min(604800);
    let query = format!("X-Amz-Algorithm=AWS4-HMAC-SHA256&X-Amz-Credential={}%2F{}&X-Amz-Date={}&X-Amz-Expires={}&X-Amz-SignedHeaders={}", access, scope.replace('/', "%2F"), timestamp, expires, signed_headers);
    let canonical_request = format!(
        "{}\n{}\n{}\nhost:{}\n\nhost\nUNSIGNED-PAYLOAD",
        method,
        canonical_uri(url),
        query,
        host
    );
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{timestamp}\n{scope}\n{}",
        hex_sha256(canonical_request.as_bytes())
    );
    let signature = hmac_hex(
        &derive_signing_key(secret, &date, region, "s3"),
        string_to_sign.as_bytes(),
    );
    Ok(PresignedUrl {
        method: method.into(),
        url: format!("{}?{}&X-Amz-Signature={signature}", url, query),
        expires_at_millis: current_millis().saturating_add(expires * 1000),
        headers: BTreeMap::new(),
    })
}

fn response_header(response: &reqwest::Response, name: &str) -> Option<String> {
    response
        .headers()
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(ToString::to_string)
}
fn host_header(url: &Url) -> String {
    let host = url.host_str().unwrap_or_default();
    match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_string(),
    }
}

fn s3_client() -> anyhow::Result<Client> {
    Ok(Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(30))
        .build()?)
}

async fn ensure_success(
    response: reqwest::Response,
    operation: &str,
) -> anyhow::Result<reqwest::Response> {
    if response.status().is_success() {
        return Ok(response);
    }
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    anyhow::bail!(
        "S3 {operation} failed ({status}): {}",
        body.chars().take(512).collect::<String>()
    )
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
    async fn s3_presign_contains_sigv4_scope_and_path_style_endpoint() {
        let storage = CloudObjectStorage::new(StorageConfig {
            provider: StorageProvider::S3Compatible,
            bucket: "roze-test".to_string(),
            endpoint: Some("http://127.0.0.1:9000".to_string()),
            region: Some("us-east-1".to_string()),
            access_key: Some("minioadmin".to_string()),
            secret_key: Some("minioadmin".to_string()),
            ..Default::default()
        });
        let signed = storage
            .presign_get("images/a.png", Duration::from_secs(60))
            .await
            .unwrap();
        assert!(signed
            .url
            .starts_with("http://127.0.0.1:9000/roze-test/images/a.png?"));
        assert!(signed.url.contains("X-Amz-Algorithm=AWS4-HMAC-SHA256"));
        assert!(signed.url.contains("X-Amz-Credential=minioadmin%2F"));
        assert!(signed.url.contains("X-Amz-Signature="));
    }

    #[tokio::test]
    async fn non_s3_runtime_operations_fail_explicitly() {
        let storage = CloudObjectStorage::new(StorageConfig {
            provider: StorageProvider::AliyunOss,
            endpoint: Some("https://oss.example.test".to_string()),
            bucket: "bucket".to_string(),
            ..Default::default()
        });
        let error = storage.get_object("a.png").await.unwrap_err().to_string();
        assert!(error.contains("runtime operations are not supported"));
    }

    #[tokio::test]
    #[ignore = "requires a real S3-compatible endpoint, for example MinIO"]
    async fn s3_compatible_round_trip_against_real_service() {
        let endpoint = std::env::var("ROZE_TEST_S3_ENDPOINT")
            .expect("ROZE_TEST_S3_ENDPOINT must point to a running S3-compatible service");
        let storage = CloudObjectStorage::new(StorageConfig {
            provider: StorageProvider::S3Compatible,
            bucket: std::env::var("ROZE_TEST_S3_BUCKET").unwrap_or_else(|_| "roze".into()),
            endpoint: Some(endpoint),
            region: Some("us-east-1".into()),
            access_key: Some(
                std::env::var("ROZE_TEST_S3_ACCESS_KEY").unwrap_or_else(|_| "minioadmin".into()),
            ),
            secret_key: Some(
                std::env::var("ROZE_TEST_S3_SECRET_KEY").unwrap_or_else(|_| "minioadmin".into()),
            ),
            validation: StorageValidation {
                allowed_extensions: vec!["png".into()],
                allowed_mime_types: vec!["image/png".into()],
                ..Default::default()
            },
            ..Default::default()
        });
        let key = format!("integration/{}.png", uuid::Uuid::now_v7());
        let payload = vec![137, 80, 78, 71, 1, 2, 3];
        let info = storage
            .put_object(PutObjectRequest::image(&key, payload.clone(), "image/png"))
            .await
            .expect("put object");
        assert_eq!(info.size, payload.len() as u64);
        assert_eq!(storage.get_object(&key).await.expect("get object"), payload);
        assert!(storage
            .stat_object(&key)
            .await
            .expect("stat object")
            .is_some());
        let signed = storage
            .presign_get(&key, Duration::from_secs(60))
            .await
            .expect("presign get");
        assert!(signed.url.contains("X-Amz-Signature="));
        storage.delete_object(&key).await.expect("delete object");
        assert!(storage
            .stat_object(&key)
            .await
            .expect("stat after delete")
            .is_none());
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
