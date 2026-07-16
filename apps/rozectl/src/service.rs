use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{bail, Context};
use serde::{Deserialize, Serialize};

const MANIFEST_NAME: &str = "roze-service.yaml";
const DEPENDENCY_CONFIG: &str = "config/roze-dependencies.yaml";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ServiceManifest {
    #[serde(default = "manifest_version")]
    pub version: u32,
    pub service: String,
    pub kind: ServiceKind,
    #[serde(default)]
    pub dependencies: BTreeMap<String, RpcDependency>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ServiceKind {
    Api,
    Rpc,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RpcDependency {
    #[serde(default = "rpc_protocol")]
    pub protocol: String,
    #[serde(rename = "crate")]
    pub crate_name: String,
    pub path: String,
    #[serde(default)]
    pub contract: Option<String>,
    #[serde(default)]
    pub target: Option<String>,
    #[serde(default)]
    pub endpoints: Vec<String>,
    #[serde(default)]
    pub etcd: Option<EtcdDependency>,
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EtcdDependency {
    pub hosts: Vec<String>,
    pub key: String,
}

#[derive(Debug, Default, Deserialize)]
struct ExistingServiceConfig {
    #[serde(default)]
    rpc_client: Option<ExistingRpcClientConfig>,
    #[serde(default)]
    rpc_clients: BTreeMap<String, ExistingRpcClientConfig>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct ExistingRpcClientConfig {
    #[serde(default)]
    target: Option<String>,
    #[serde(default)]
    endpoints: Vec<String>,
    #[serde(default)]
    etcd: Option<EtcdDependency>,
    #[serde(default = "default_timeout_ms")]
    timeout_ms: u64,
}

#[derive(Debug, Clone)]
pub struct AddDependency {
    pub name: String,
    pub crate_name: String,
    pub path: PathBuf,
    pub contract: Option<PathBuf>,
    pub target: Option<String>,
    pub endpoints: Vec<String>,
    pub etcd_hosts: Vec<String>,
    pub etcd_key: Option<String>,
    pub timeout_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncResult {
    pub changed: Vec<PathBuf>,
}

pub fn add_dependency(project: &Path, input: AddDependency) -> anyhow::Result<SyncResult> {
    validate_name(&input.name, "dependency name")?;
    validate_crate_name(&input.crate_name)?;
    let connection_modes = usize::from(input.target.is_some())
        + usize::from(!input.endpoints.is_empty())
        + usize::from(!input.etcd_hosts.is_empty() || input.etcd_key.is_some());
    if connection_modes != 1 {
        bail!(
            "dependency must select exactly one of --target, --endpoint, or --etcd-host/--etcd-key"
        );
    }
    let etcd = if !input.etcd_hosts.is_empty() || input.etcd_key.is_some() {
        let key = input
            .etcd_key
            .context("--etcd-key is required with --etcd-host")?;
        if input.etcd_hosts.is_empty() {
            bail!("at least one --etcd-host is required with --etcd-key");
        }
        Some(EtcdDependency {
            hosts: input.etcd_hosts,
            key,
        })
    } else {
        None
    };
    if input.timeout_ms == 0 {
        bail!("--timeout-ms must be greater than zero");
    }
    validate_rpc_crate(project, &input.path, &input.crate_name)?;
    if let Some(contract) = &input.contract {
        let contract_path = project_relative(project, contract);
        if !contract_path.is_file() {
            bail!("RPC contract does not exist: {}", contract_path.display());
        }
    }

    let mut manifest = load_or_create_manifest(project)?;
    manifest.dependencies.insert(
        input.name,
        RpcDependency {
            protocol: rpc_protocol(),
            crate_name: input.crate_name,
            path: normalized_path(&input.path),
            contract: input.contract.as_deref().map(normalized_path),
            target: input.target,
            endpoints: input.endpoints,
            etcd,
            timeout_ms: input.timeout_ms,
        },
    );
    validate_manifest(&manifest)?;
    write_manifest(project, &manifest)?;
    sync(project, false)
}

pub fn remove_dependency(project: &Path, name: &str) -> anyhow::Result<SyncResult> {
    let mut manifest = load_manifest(project)?;
    if manifest.dependencies.remove(name).is_none() {
        bail!("dependency `{name}` is not declared");
    }
    write_manifest(project, &manifest)?;
    sync(project, false)
}

pub fn list_dependencies(project: &Path) -> anyhow::Result<Vec<(String, RpcDependency)>> {
    Ok(load_manifest(project)?.dependencies.into_iter().collect())
}

pub fn sync(project: &Path, check: bool) -> anyhow::Result<SyncResult> {
    let manifest = load_manifest(project)?;
    validate_manifest(&manifest)?;
    validate_project_kind(project, manifest.kind)?;

    let mut changed = Vec::new();
    let cargo_path = project.join("Cargo.toml");
    let cargo = render_cargo(&cargo_path, &manifest)?;
    update_file(&cargo_path, cargo.as_bytes(), check, &mut changed)?;

    let dependency_config_path = project.join(DEPENDENCY_CONFIG);
    let dependency_config = render_dependency_config(&manifest);
    update_file(
        &dependency_config_path,
        dependency_config.as_bytes(),
        check,
        &mut changed,
    )?;

    let managed_clients = manifest
        .dependencies
        .iter()
        .map(|(name, dependency)| crate::generator::ManagedRpcClient {
            name: name.clone(),
            dependency_name: dependency.crate_name.clone(),
            path: dependency.path.clone(),
        })
        .collect::<Vec<_>>();
    if crate::generator::sync_project_rpc_clients(project, &managed_clients, check)? {
        changed.push(project.join("src/svc/mod.rs"));
    }
    Ok(SyncResult { changed })
}

pub fn load_manifest(project: &Path) -> anyhow::Result<ServiceManifest> {
    let path = project.join(MANIFEST_NAME);
    let manifest = config::Config::builder()
        .add_source(config::File::from(path.clone()))
        .build()
        .with_context(|| format!("failed to read {}", path.display()))?
        .try_deserialize::<ServiceManifest>()
        .with_context(|| format!("failed to parse {}", path.display()))?;
    validate_manifest(&manifest)?;
    Ok(manifest)
}

fn load_or_create_manifest(project: &Path) -> anyhow::Result<ServiceManifest> {
    if project.join(MANIFEST_NAME).is_file() {
        return load_manifest(project);
    }
    let cargo_path = project.join("Cargo.toml");
    let cargo = fs::read_to_string(&cargo_path)
        .with_context(|| format!("failed to read {}", cargo_path.display()))?;
    let document = cargo
        .parse::<toml_edit::DocumentMut>()
        .with_context(|| format!("failed to parse {}", cargo_path.display()))?;
    let service = document["package"]["name"]
        .as_str()
        .context("Cargo.toml package.name is required")?
        .to_string();
    let dependencies = import_existing_dependencies(project, &document)?;
    Ok(ServiceManifest {
        version: manifest_version(),
        service,
        kind: detect_project_kind(project)?,
        dependencies,
    })
}

fn import_existing_dependencies(
    project: &Path,
    document: &toml_edit::DocumentMut,
) -> anyhow::Result<BTreeMap<String, RpcDependency>> {
    let Some(dependencies) = document
        .get("dependencies")
        .and_then(toml_edit::Item::as_table)
    else {
        return Ok(BTreeMap::new());
    };
    let discovered = dependencies
        .iter()
        .filter_map(|(crate_name, item)| {
            if !crate_name.ends_with("-rpc") || crate_name == "roze-rpc" {
                return None;
            }
            cargo_dependency_path(item).map(|path| (crate_name.to_string(), path.to_string()))
        })
        .collect::<Vec<_>>();
    if discovered.is_empty() {
        return Ok(BTreeMap::new());
    }

    let config_path = project.join("config.yaml");
    let existing = config::Config::builder()
        .add_source(config::File::from(config_path.clone()))
        .build()
        .with_context(|| {
            format!(
                "failed to read {} while importing RPC dependencies",
                config_path.display()
            )
        })?
        .try_deserialize::<ExistingServiceConfig>()
        .with_context(|| {
            format!(
                "failed to parse {} while importing RPC dependencies",
                config_path.display()
            )
        })?;
    let mut imported = BTreeMap::new();
    for (crate_name, path) in discovered {
        let name = crate_name
            .strip_suffix("-rpc")
            .unwrap_or(&crate_name)
            .rsplit('-')
            .next()
            .unwrap_or(&crate_name)
            .to_string();
        let config = existing
            .rpc_clients
            .get(&name)
            .cloned()
            .or_else(|| (existing.rpc_clients.is_empty()).then(|| existing.rpc_client.clone()).flatten())
            .with_context(|| format!("existing RPC dependency `{crate_name}` has no `rpc_clients.{name}` configuration; add it before creating roze-service.yaml"))?;
        imported.insert(
            name,
            RpcDependency {
                protocol: rpc_protocol(),
                crate_name,
                path,
                contract: None,
                target: config.target,
                endpoints: config.endpoints,
                etcd: config.etcd,
                timeout_ms: config.timeout_ms,
            },
        );
    }
    Ok(imported)
}

fn cargo_dependency_path(item: &toml_edit::Item) -> Option<&str> {
    item.as_value()
        .and_then(toml_edit::Value::as_inline_table)
        .and_then(|table| table.get("path"))
        .and_then(toml_edit::Value::as_str)
        .or_else(|| {
            item.as_table()
                .and_then(|table| table.get("path"))
                .and_then(toml_edit::Item::as_str)
        })
}

fn write_manifest(project: &Path, manifest: &ServiceManifest) -> anyhow::Result<()> {
    let path = project.join(MANIFEST_NAME);
    fs::write(&path, render_manifest(manifest))
        .with_context(|| format!("failed to write {}", path.display()))
}

fn render_manifest(manifest: &ServiceManifest) -> String {
    let mut out = format!(
        "version: {}\nservice: {}\nkind: {}\n",
        manifest.version,
        yaml_string(&manifest.service),
        match manifest.kind {
            ServiceKind::Api => "api",
            ServiceKind::Rpc => "rpc",
        }
    );
    if manifest.dependencies.is_empty() {
        out.push_str("dependencies: {}\n");
        return out;
    }
    out.push_str("dependencies:\n");
    for (name, dependency) in &manifest.dependencies {
        out.push_str(&format!("  {}:\n", yaml_key(name)));
        out.push_str("    protocol: rpc\n");
        out.push_str(&format!(
            "    crate: {}\n",
            yaml_string(&dependency.crate_name)
        ));
        out.push_str(&format!("    path: {}\n", yaml_string(&dependency.path)));
        if let Some(contract) = &dependency.contract {
            out.push_str(&format!("    contract: {}\n", yaml_string(contract)));
        }
        render_connection(&mut out, dependency, 4);
        out.push_str(&format!("    timeout_ms: {}\n", dependency.timeout_ms));
    }
    out
}

fn render_dependency_config(manifest: &ServiceManifest) -> String {
    let mut out = String::from("# Generated by `rozectl service sync`; environment-specific config.yaml and ROZE__ variables override these defaults.\nrpc_clients:\n");
    if manifest.dependencies.is_empty() {
        out.push_str("  {}\n");
        return out;
    }
    for (name, dependency) in &manifest.dependencies {
        out.push_str(&format!("  {}:\n", yaml_key(name)));
        render_connection(&mut out, dependency, 4);
        out.push_str(&format!("    timeout_ms: {}\n", dependency.timeout_ms));
    }
    out
}

fn render_connection(out: &mut String, dependency: &RpcDependency, indent: usize) {
    let pad = " ".repeat(indent);
    if let Some(target) = &dependency.target {
        out.push_str(&format!("{pad}target: {}\n", yaml_string(target)));
    } else if !dependency.endpoints.is_empty() {
        out.push_str(&format!("{pad}endpoints:\n"));
        for endpoint in &dependency.endpoints {
            out.push_str(&format!("{pad}  - {}\n", yaml_string(endpoint)));
        }
    } else if let Some(etcd) = &dependency.etcd {
        out.push_str(&format!("{pad}etcd:\n{pad}  hosts:\n"));
        for host in &etcd.hosts {
            out.push_str(&format!("{pad}    - {}\n", yaml_string(host)));
        }
        out.push_str(&format!("{pad}  key: {}\n", yaml_string(&etcd.key)));
    }
}

fn render_cargo(path: &Path, manifest: &ServiceManifest) -> anyhow::Result<String> {
    let content =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    let mut document = content
        .parse::<toml_edit::DocumentMut>()
        .with_context(|| format!("failed to parse {}", path.display()))?;
    let previously_managed = managed_rpc_dependencies(&document);
    let dependencies =
        document["dependencies"].or_insert(toml_edit::Item::Table(toml_edit::Table::new()));
    let dependencies = dependencies
        .as_table_mut()
        .context("Cargo.toml dependencies must be a table")?;
    // Reinsert both adopted and previously managed entries so the first sync
    // has the same canonical ordering as every subsequent sync.
    for dependency in previously_managed.iter().chain(
        manifest
            .dependencies
            .values()
            .map(|dependency| &dependency.crate_name),
    ) {
        dependencies.remove(dependency);
    }
    for dependency in manifest.dependencies.values() {
        let mut table = toml_edit::InlineTable::new();
        table.insert("path", toml_edit::Value::from(dependency.path.clone()));
        dependencies.insert(&dependency.crate_name, toml_edit::value(table));
    }
    let mut managed = toml_edit::Array::new();
    for dependency in manifest.dependencies.values() {
        managed.push(dependency.crate_name.as_str());
    }
    document["package"]["metadata"]["roze"]["managed-rpc-dependencies"] = toml_edit::value(managed);
    Ok(document.to_string())
}

fn managed_rpc_dependencies(document: &toml_edit::DocumentMut) -> Vec<String> {
    let Some(metadata) = document
        .get("package")
        .and_then(toml_edit::Item::as_table)
        .and_then(|package| package.get("metadata"))
    else {
        return Vec::new();
    };
    let array = metadata
        .as_table()
        .and_then(|metadata| metadata.get("roze"))
        .and_then(toml_edit::Item::as_table)
        .and_then(|roze| roze.get("managed-rpc-dependencies"))
        .and_then(toml_edit::Item::as_array)
        .or_else(|| {
            metadata
                .as_value()
                .and_then(toml_edit::Value::as_inline_table)
                .and_then(|metadata| metadata.get("roze"))
                .and_then(toml_edit::Value::as_inline_table)
                .and_then(|roze| roze.get("managed-rpc-dependencies"))
                .and_then(toml_edit::Value::as_array)
        });
    array
        .into_iter()
        .flatten()
        .filter_map(toml_edit::Value::as_str)
        .map(str::to_string)
        .collect()
}

fn update_file(
    path: &Path,
    expected: &[u8],
    check: bool,
    changed: &mut Vec<PathBuf>,
) -> anyhow::Result<()> {
    if fs::read(path).ok().as_deref() == Some(expected) {
        return Ok(());
    }
    changed.push(path.to_path_buf());
    if check {
        bail!(
            "{} is not synchronized; run `rozectl service sync --project {}`",
            path.display(),
            path.parent().unwrap_or(Path::new(".")).display()
        );
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    fs::write(path, expected).with_context(|| format!("failed to write {}", path.display()))
}

fn validate_manifest(manifest: &ServiceManifest) -> anyhow::Result<()> {
    if manifest.version != manifest_version() {
        bail!(
            "unsupported roze-service.yaml version {}; expected 1",
            manifest.version
        );
    }
    if manifest.service.trim().is_empty() {
        bail!("service name cannot be empty");
    }
    let mut generated_names = BTreeMap::new();
    for (name, dependency) in &manifest.dependencies {
        validate_name(name, "dependency name")?;
        let generated_name = crate::generator::rust_identifier(name);
        if let Some(existing) = generated_names.insert(generated_name.clone(), name) {
            bail!(
                "dependency names `{existing}` and `{name}` both generate the Rust accessor `{generated_name}`"
            );
        }
        validate_crate_name(&dependency.crate_name)?;
        if dependency.protocol != "rpc" {
            bail!(
                "dependency `{name}` uses unsupported protocol `{}`; only rpc is supported",
                dependency.protocol
            );
        }
        if dependency.path.trim().is_empty() {
            bail!("dependency `{name}` path cannot be empty");
        }
        let modes = usize::from(dependency.target.is_some())
            + usize::from(!dependency.endpoints.is_empty())
            + usize::from(dependency.etcd.is_some());
        if modes != 1 {
            bail!("dependency `{name}` must select exactly one connection mode");
        }
        if dependency.timeout_ms == 0 {
            bail!("dependency `{name}` timeout_ms must be greater than zero");
        }
    }
    Ok(())
}

fn validate_name(value: &str, label: &str) -> anyhow::Result<()> {
    if value.is_empty()
        || !value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
    {
        bail!("{label} `{value}` must contain only ASCII letters, digits, `_`, or `-`");
    }
    Ok(())
}

fn validate_crate_name(value: &str) -> anyhow::Result<()> {
    validate_name(value, "crate name")?;
    if !value.ends_with("-rpc") {
        bail!("RPC dependency crate `{value}` must end with `-rpc`");
    }
    Ok(())
}

fn validate_rpc_crate(
    project: &Path,
    dependency_path: &Path,
    crate_name: &str,
) -> anyhow::Result<()> {
    let manifest_path = project_relative(project, dependency_path).join("Cargo.toml");
    let content = fs::read_to_string(&manifest_path)
        .with_context(|| format!("failed to read RPC dependency {}", manifest_path.display()))?;
    let document = content
        .parse::<toml_edit::DocumentMut>()
        .with_context(|| format!("failed to parse RPC dependency {}", manifest_path.display()))?;
    let actual = document["package"]["name"]
        .as_str()
        .context("RPC dependency Cargo.toml package.name is required")?;
    if actual != crate_name {
        bail!(
            "RPC dependency crate mismatch: declared `{crate_name}`, found `{actual}` in {}",
            manifest_path.display()
        );
    }
    Ok(())
}

fn detect_project_kind(project: &Path) -> anyhow::Result<ServiceKind> {
    let api =
        project.join("src/route/mod.rs").is_file() && project.join("src/handler/mod.rs").is_file();
    let rpc = project.join("src/server/mod.rs").is_file()
        && project.join("proto/service.proto").is_file();
    match (api, rpc) {
        (true, false) => Ok(ServiceKind::Api),
        (false, true) => Ok(ServiceKind::Rpc),
        (true, true) => bail!(
            "{} contains both API and RPC generated boundaries",
            project.display()
        ),
        (false, false) => bail!(
            "{} is not a generated Roze API or RPC service",
            project.display()
        ),
    }
}

fn validate_project_kind(project: &Path, declared: ServiceKind) -> anyhow::Result<()> {
    let actual = detect_project_kind(project)?;
    if actual != declared {
        bail!(
            "roze-service.yaml declares kind `{}` but {} is `{}`",
            service_kind_name(declared),
            project.display(),
            service_kind_name(actual)
        );
    }
    Ok(())
}

fn service_kind_name(kind: ServiceKind) -> &'static str {
    match kind {
        ServiceKind::Api => "api",
        ServiceKind::Rpc => "rpc",
    }
}

fn project_relative(project: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        project.join(path)
    }
}

fn yaml_string(value: &str) -> String {
    serde_json::to_string(value).expect("string serialization")
}
fn yaml_key(value: &str) -> String {
    if value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
    {
        value.to_string()
    } else {
        yaml_string(value)
    }
}
fn normalized_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}
fn manifest_version() -> u32 {
    1
}
fn rpc_protocol() -> String {
    "rpc".to_string()
}
fn default_timeout_ms() -> u64 {
    2_000
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(kind: ServiceKind) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "rozectl-service-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = root.join("payment");
        fs::create_dir_all(path.join("src/svc")).unwrap();
        match kind {
            ServiceKind::Api => {
                fs::create_dir_all(path.join("src/route")).unwrap();
                fs::create_dir_all(path.join("src/handler")).unwrap();
                fs::write(path.join("src/route/mod.rs"), "").unwrap();
                fs::write(path.join("src/handler/mod.rs"), "").unwrap();
            }
            ServiceKind::Rpc => {
                fs::create_dir_all(path.join("src/server")).unwrap();
                fs::create_dir_all(path.join("proto")).unwrap();
                fs::write(path.join("src/server/mod.rs"), "").unwrap();
                fs::write(path.join("proto/service.proto"), "").unwrap();
            }
        }
        fs::create_dir_all(root.join("shop-order-rpc")).unwrap();
        fs::write(
            root.join("shop-order-rpc/Cargo.toml"),
            "[package]\nname = \"shop-order-rpc\"\nversion = \"1.0.0\"\n",
        )
        .unwrap();
        fs::write(root.join("shop-order-rpc/order.api"), "service order-rpc { rpc GetOrder (GetOrderReq) returns (GetOrderResp) }\ntype GetOrderReq { id: u64 }\ntype GetOrderResp { id: u64 }\n").unwrap();
        fs::create_dir_all(root.join("shop-inventory-rpc")).unwrap();
        fs::write(
            root.join("shop-inventory-rpc/Cargo.toml"),
            "[package]\nname = \"shop-inventory-rpc\"\nversion = \"1.0.0\"\n",
        )
        .unwrap();
        fs::write(
            path.join("Cargo.toml"),
            "[package]\nname = \"payment\"\nversion = \"1.0.0\"\n\n[dependencies]\n",
        )
        .unwrap();
        fs::write(path.join("src/svc/mod.rs"), "#[derive(Clone)]\npub struct ServiceContext {\n}\n\nimpl ServiceContext {\n    pub async fn new(config: roze_config::ServiceConfig) -> anyhow::Result<Self> {\n        let health = roze_health::HealthRegistry::new();\n        health.mark_ready();\n        Ok(Self {\n        })\n    }\n}\n").unwrap();
        path
    }

    #[test]
    fn dependency_add_and_sync_are_deterministic() {
        let path = project(ServiceKind::Api);
        add_dependency(
            &path,
            AddDependency {
                name: "order".into(),
                crate_name: "shop-order-rpc".into(),
                path: PathBuf::from("../shop-order-rpc"),
                contract: Some(PathBuf::from("../shop-order-rpc/order.api")),
                target: None,
                endpoints: vec!["127.0.0.1:4002".into()],
                etcd_hosts: vec![],
                etcd_key: None,
                timeout_ms: 1500,
            },
        )
        .unwrap();
        sync(&path, true).unwrap();
        let manifest = fs::read_to_string(path.join(MANIFEST_NAME)).unwrap();
        assert!(manifest.contains("crate: \"shop-order-rpc\""));
        assert!(fs::read_to_string(path.join("Cargo.toml"))
            .unwrap()
            .contains("shop-order-rpc = { path = \"../shop-order-rpc\" }"));
        assert!(fs::read_to_string(path.join(DEPENDENCY_CONFIG))
            .unwrap()
            .contains("127.0.0.1:4002"));
        let svc = fs::read_to_string(path.join("src/svc/mod.rs")).unwrap();
        assert!(svc.contains("pub order_client: shop_order_rpc::client::RpcClient"));
        assert!(svc.contains("pub fn order(&self)"));
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn rpc_service_dependency_add_and_sync_are_deterministic() {
        let path = project(ServiceKind::Rpc);
        add_dependency(
            &path,
            AddDependency {
                name: "order".into(),
                crate_name: "shop-order-rpc".into(),
                path: PathBuf::from("../shop-order-rpc"),
                contract: None,
                target: Some("http://127.0.0.1:4002".into()),
                endpoints: vec![],
                etcd_hosts: vec![],
                etcd_key: None,
                timeout_ms: 1500,
            },
        )
        .unwrap();

        sync(&path, true).unwrap();
        let manifest = load_manifest(&path).unwrap();
        assert_eq!(manifest.kind, ServiceKind::Rpc);
        let rendered = fs::read_to_string(path.join(MANIFEST_NAME)).unwrap();
        assert!(rendered.contains("kind: rpc"));
        let svc = fs::read_to_string(path.join("src/svc/mod.rs")).unwrap();
        assert!(svc.contains("pub order_client: shop_order_rpc::client::RpcClient"));
        assert!(svc.contains("pub fn order(&self)"));
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn sync_rejects_manifest_kind_that_disagrees_with_generated_service() {
        let path = project(ServiceKind::Api);
        fs::write(
            path.join(MANIFEST_NAME),
            "version: 1\nservice: payment\nkind: rpc\ndependencies: {}\n",
        )
        .unwrap();

        let error = sync(&path, true).unwrap_err().to_string();
        assert!(error.contains("declares kind `rpc`"));
        assert!(error.contains("is `api`"));
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn dependency_remove_cleans_all_managed_surfaces() {
        let path = project(ServiceKind::Api);
        add_dependency(
            &path,
            AddDependency {
                name: "order".into(),
                crate_name: "shop-order-rpc".into(),
                path: PathBuf::from("../shop-order-rpc"),
                contract: None,
                target: Some("http://127.0.0.1:4002".into()),
                endpoints: vec![],
                etcd_hosts: vec![],
                etcd_key: None,
                timeout_ms: 2000,
            },
        )
        .unwrap();
        remove_dependency(&path, "order").unwrap();
        sync(&path, true).unwrap();
        let cargo = fs::read_to_string(path.join("Cargo.toml")).unwrap();
        assert!(!cargo.contains("shop-order-rpc ="));
        assert!(!fs::read_to_string(path.join(DEPENDENCY_CONFIG))
            .unwrap()
            .contains("order:"));
        assert!(!fs::read_to_string(path.join("src/svc/mod.rs"))
            .unwrap()
            .contains("order_client"));
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn first_manifest_imports_existing_rpc_dependencies() {
        let path = project(ServiceKind::Api);
        fs::write(
            path.join("Cargo.toml"),
            "[package]\nname = \"payment\"\nversion = \"1.0.0\"\n\n[dependencies]\nshop-inventory-rpc = { path = \"../shop-inventory-rpc\" }\nserde = \"1\"\n",
        )
        .unwrap();
        fs::write(
            path.join("config.yaml"),
            "rpc_clients:\n  inventory:\n    endpoints: [127.0.0.1:4003]\n    timeout_ms: 1800\n",
        )
        .unwrap();

        add_dependency(
            &path,
            AddDependency {
                name: "order".into(),
                crate_name: "shop-order-rpc".into(),
                path: PathBuf::from("../shop-order-rpc"),
                contract: None,
                target: Some("http://127.0.0.1:4002".into()),
                endpoints: vec![],
                etcd_hosts: vec![],
                etcd_key: None,
                timeout_ms: 2000,
            },
        )
        .unwrap();

        sync(&path, true).unwrap();

        let manifest = load_manifest(&path).unwrap();
        assert_eq!(manifest.dependencies.len(), 2);
        assert_eq!(manifest.dependencies["inventory"].timeout_ms, 1800);
        let svc = fs::read_to_string(path.join("src/svc/mod.rs")).unwrap();
        assert!(svc.contains("inventory_client"));
        assert!(svc.contains("order_client"));
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn first_add_adopts_existing_rpc_dependency_without_cargo_drift() {
        let path = project(ServiceKind::Rpc);
        fs::write(
            path.join("Cargo.toml"),
            "[package]\nname = \"payment\"\nversion = \"1.0.0\"\n\n[dependencies]\nshop-order-rpc = { path = \"../shop-order-rpc\" }\nserde = \"1\"\n",
        )
        .unwrap();
        fs::write(
            path.join("config.yaml"),
            "rpc_clients:\n  order:\n    endpoints: [127.0.0.1:4002]\n    timeout_ms: 1800\n",
        )
        .unwrap();

        add_dependency(
            &path,
            AddDependency {
                name: "order".into(),
                crate_name: "shop-order-rpc".into(),
                path: PathBuf::from("../shop-order-rpc"),
                contract: None,
                target: None,
                endpoints: vec!["127.0.0.1:4002".into()],
                etcd_hosts: vec![],
                etcd_key: None,
                timeout_ms: 1800,
            },
        )
        .unwrap();

        sync(&path, true).unwrap();
        assert_eq!(load_manifest(&path).unwrap().dependencies.len(), 1);
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
}
