pub mod client;
pub mod model;
pub mod native;
pub mod rest;
pub mod rpc;
pub mod search;
pub mod types;

use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{bail, Context};

use crate::parser::{ApiSpec, HttpMethod, RpcMethod};

const ROZE_GIT_URL: &str = "https://github.com/roze-team/roze.git";
const REST_ROZE_CRATES: [&str; 20] = [
    "roze-config",
    "roze-error",
    "roze-health",
    "roze-http",
    "roze-log",
    "roze-metrics",
    "roze-middleware",
    "roze-jwt",
    "roze-cache",
    "roze-context",
    "roze-mq",
    "roze-nats",
    "roze-openapi",
    "roze-query",
    "roze-result",
    "roze-service",
    "roze-storage",
    "roze-transaction",
    "roze-validation",
    "roze-rpc",
];

const RPC_ROZE_CRATES: [&str; 21] = [
    "roze-config",
    "roze-context",
    "roze-db",
    "roze-mongo",
    "roze-error",
    "roze-grpc",
    "roze-health",
    "roze-jwt",
    "roze-log",
    "roze-middleware",
    "roze-cache",
    "roze-mq",
    "roze-nats",
    "roze-result",
    "roze-query",
    "roze-rpc",
    "roze-service",
    "roze-storage",
    "roze-trace",
    "roze-transaction",
    "roze-validation",
];

const STREAM_ROZE_CRATES: [&str; 4] = [
    "roze-mq",
    "roze-service",
    "roze-shutdown",
    "roze-validation",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProjectKind {
    Rest,
    Rpc,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct RpcClientBinding {
    name: String,
    dep_name: String,
    crate_name: String,
    path: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GenerateMode {
    Create,
    Update,
    Force,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DependencySource {
    Git,
    Path,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GenerateOptions {
    pub mode: GenerateMode,
    pub dependency_source: DependencySource,
}

impl GenerateOptions {
    pub const fn new(mode: GenerateMode, dependency_source: DependencySource) -> Self {
        Self {
            mode,
            dependency_source,
        }
    }
}

#[derive(Debug, Clone)]
pub enum GeneratorCommand {
    ApiGenerate {
        api: PathBuf,
        out: PathBuf,
        options: GenerateOptions,
    },
    ApiNew {
        name: String,
        out: PathBuf,
        options: GenerateOptions,
    },
    RpcGenerate {
        api: PathBuf,
        out: PathBuf,
        options: GenerateOptions,
    },
    RpcNew {
        name: String,
        out: PathBuf,
        options: GenerateOptions,
    },
    ModelGenerate {
        schema: PathBuf,
        out: PathBuf,
        options: GenerateOptions,
        format: model::ModelFormat,
        orm: model::ModelOrm,
    },
    ModelInspect {
        table: String,
        schema: Option<String>,
        db_url: String,
        db_kind: model::InspectDatabaseKind,
        sample_size: u64,
        out: PathBuf,
        options: GenerateOptions,
        orm: model::ModelOrm,
    },
}

impl GeneratorCommand {
    fn key(&self) -> &'static str {
        match self {
            Self::ApiGenerate { .. } => "api.generate",
            Self::ApiNew { .. } => "api.new",
            Self::RpcGenerate { .. } => "rpc.generate",
            Self::RpcNew { .. } => "rpc.new",
            Self::ModelGenerate { .. } => "model.generate",
            Self::ModelInspect { .. } => "model.inspect",
        }
    }

    fn with_out(mut self, next_out: PathBuf) -> Self {
        match &mut self {
            Self::ApiGenerate { out, .. }
            | Self::ApiNew { out, .. }
            | Self::RpcGenerate { out, .. }
            | Self::RpcNew { out, .. }
            | Self::ModelGenerate { out, .. }
            | Self::ModelInspect { out, .. } => {
                *out = next_out;
            }
        }
        self
    }
}

type GeneratorHandler = fn(GeneratorCommand) -> anyhow::Result<()>;

struct GeneratorEntry {
    key: &'static str,
    handler: GeneratorHandler,
}

const GENERATOR_ENTRIES: &[GeneratorEntry] = &[
    GeneratorEntry {
        key: "api.generate",
        handler: api_generate_handler,
    },
    GeneratorEntry {
        key: "api.new",
        handler: api_new_handler,
    },
    GeneratorEntry {
        key: "rpc.generate",
        handler: rpc_generate_handler,
    },
    GeneratorEntry {
        key: "rpc.new",
        handler: rpc_new_handler,
    },
    GeneratorEntry {
        key: "model.generate",
        handler: model_generate_handler,
    },
    GeneratorEntry {
        key: "model.inspect",
        handler: model_inspect_handler,
    },
];

#[derive(Debug, Default)]
pub struct GeneratorRegistry {
    handlers: std::collections::BTreeMap<&'static str, GeneratorHandler>,
}

impl GeneratorRegistry {
    pub fn new() -> Self {
        let mut registry = Self::default();
        for entry in GENERATOR_ENTRIES {
            registry.register(entry.key, entry.handler);
        }
        registry
    }

    pub fn register(&mut self, key: &'static str, handler: GeneratorHandler) {
        self.handlers.insert(key, handler);
    }

    pub fn dispatch(&self, command: GeneratorCommand) -> anyhow::Result<()> {
        let key = command.key();
        let handler = self
            .handlers
            .get(key)
            .ok_or_else(|| anyhow::anyhow!("no generator registered for `{key}`"))?;
        handler(command)
    }
}

pub fn registry() -> GeneratorRegistry {
    GeneratorRegistry::new()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DiffStatus {
    Added,
    Modified,
    Deleted,
}

impl DiffStatus {
    fn marker(self) -> &'static str {
        match self {
            Self::Added => "A",
            Self::Modified => "M",
            Self::Deleted => "D",
        }
    }
}

#[derive(Debug)]
struct DiffWorkspace {
    root: PathBuf,
}

impl DiffWorkspace {
    fn new(target: &Path) -> anyhow::Result<Self> {
        let parent = target
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let root = parent.join(format!(".rozectl-diff-{nanos}"));
        fs::create_dir_all(&root)
            .with_context(|| format!("failed to create diff workspace {}", root.display()))?;
        Ok(Self { root })
    }

    fn output_path(&self, target: &Path) -> PathBuf {
        let name = target
            .file_name()
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| std::ffi::OsStr::new("project"));
        self.root.join(name)
    }
}

impl Drop for DiffWorkspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

pub fn diff_project(
    target: &Path,
    command: GeneratorCommand,
    registry: &GeneratorRegistry,
) -> anyhow::Result<String> {
    let workspace = DiffWorkspace::new(target)?;
    let generated = workspace.output_path(target);
    if target.exists() {
        copy_dir_recursive(target, &generated)?;
    }
    registry.dispatch(command.with_out(generated.clone()))?;
    render_project_diff(target, &generated)
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> anyhow::Result<()> {
    if !src.exists() {
        return Ok(());
    }
    if src.is_file() {
        if let Some(parent) = dst.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(src, dst)
            .with_context(|| format!("failed to copy {} to {}", src.display(), dst.display()))?;
        return Ok(());
    }

    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src).with_context(|| format!("failed to read {}", src.display()))? {
        let entry = entry?;
        let path = entry.path();
        if should_skip_diff_path(&path) {
            continue;
        }
        let next_dst = dst.join(entry.file_name());
        if path.is_dir() {
            copy_dir_recursive(&path, &next_dst)?;
        } else if path.is_file() {
            fs::copy(&path, &next_dst).with_context(|| {
                format!(
                    "failed to copy {} to {}",
                    path.display(),
                    next_dst.display()
                )
            })?;
        }
    }
    Ok(())
}

fn render_project_diff(before: &Path, after: &Path) -> anyhow::Result<String> {
    let before_files = collect_files(before)?;
    let after_files = collect_files(after)?;
    let mut paths = BTreeSet::new();
    paths.extend(before_files.keys().cloned());
    paths.extend(after_files.keys().cloned());

    let mut lines = Vec::new();
    for path in paths {
        let status = match (before_files.get(&path), after_files.get(&path)) {
            (None, Some(_)) => Some(DiffStatus::Added),
            (Some(_), None) => Some(DiffStatus::Deleted),
            (Some(before_path), Some(after_path)) => {
                if files_equal(before_path, after_path)? {
                    None
                } else {
                    Some(DiffStatus::Modified)
                }
            }
            (None, None) => None,
        };
        if let Some(status) = status {
            lines.push(format!("{} {}", status.marker(), path.display()));
        }
    }

    if lines.is_empty() {
        Ok(String::new())
    } else {
        lines.push(String::new());
        Ok(lines.join("\n"))
    }
}

fn collect_files(root: &Path) -> anyhow::Result<BTreeMap<PathBuf, PathBuf>> {
    let mut files = BTreeMap::new();
    if !root.exists() {
        return Ok(files);
    }
    collect_files_inner(root, root, &mut files)?;
    Ok(files)
}

fn collect_files_inner(
    root: &Path,
    current: &Path,
    files: &mut BTreeMap<PathBuf, PathBuf>,
) -> anyhow::Result<()> {
    if should_skip_diff_path(current) {
        return Ok(());
    }
    if current.is_file() {
        let relative = current
            .strip_prefix(root)
            .with_context(|| format!("failed to relativize {}", current.display()))?
            .to_path_buf();
        files.insert(relative, current.to_path_buf());
        return Ok(());
    }
    if !current.is_dir() {
        return Ok(());
    }
    for entry in
        fs::read_dir(current).with_context(|| format!("failed to read {}", current.display()))?
    {
        let entry = entry?;
        collect_files_inner(root, &entry.path(), files)?;
    }
    Ok(())
}

fn files_equal(left: &Path, right: &Path) -> anyhow::Result<bool> {
    let left = fs::read(left).with_context(|| format!("failed to read {}", left.display()))?;
    let right = fs::read(right).with_context(|| format!("failed to read {}", right.display()))?;
    Ok(left == right)
}

fn should_skip_diff_path(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    name == ".git" || name == "target" || name.starts_with(".rozectl-diff-")
}

pub fn template(name: &str) -> anyhow::Result<String> {
    match name {
        "api" => Ok(api_template("demo")),
        "rpc" => Ok(rpc_template("demo")),
        "model" => Ok(
            "model User {\n  table: users\n  primary: id\n  cache: true\n  cache_key: id,username\n  field id ObjectId\n  field username String\n}\n"
                .to_string(),
        ),
        other => bail!("unknown template `{other}`; expected api, rpc or model"),
    }
}

pub fn init_templates(out: &Path) -> anyhow::Result<()> {
    fs::create_dir_all(out).with_context(|| format!("failed to create {}", out.display()))?;
    fs::write(out.join("api.api"), template("api")?)?;
    fs::write(out.join("rpc.api"), template("rpc")?)?;
    fs::write(out.join("model.model"), template("model")?)?;
    Ok(())
}

pub fn write_service_markdown_doc(api: &Path, out: &Path, force: bool) -> anyhow::Result<()> {
    write_api_markdown_doc_with(api, out, force, render_service_markdown_doc)
}

pub fn write_ai_context_markdown_doc(api: &Path, out: &Path, force: bool) -> anyhow::Result<()> {
    write_api_markdown_doc_with(api, out, force, render_ai_context_markdown_doc)
}

pub fn write_mock_server_project(api: &Path, out: &Path, force: bool) -> anyhow::Result<()> {
    if out.exists() && !force {
        bail!(
            "{} already exists; pass --force to overwrite mock server files",
            out.display()
        );
    }

    let source = read_api_source(api)?;
    let spec = crate::parser::parse_api(&source)
        .with_context(|| format!("failed to parse api file {}", api.display()))?;
    validate_project_kind(&spec, ProjectKind::Rest)?;

    fs::create_dir_all(out.join("src"))
        .with_context(|| format!("failed to create {}", out.join("src").display()))?;
    fs::write(out.join("Cargo.toml"), render_mock_cargo_toml(&spec))
        .with_context(|| format!("failed to write {}", out.join("Cargo.toml").display()))?;
    fs::write(out.join("src/main.rs"), render_mock_main(&spec))
        .with_context(|| format!("failed to write {}", out.join("src/main.rs").display()))?;
    fs::write(out.join("README.md"), render_mock_readme(&spec, api))
        .with_context(|| format!("failed to write {}", out.join("README.md").display()))?;
    Ok(())
}

pub fn write_http_smoke_test_project(
    api: &Path,
    out: &Path,
    base_url: &str,
    _force: bool,
) -> anyhow::Result<()> {
    let source = read_api_source(api)?;
    let spec = crate::parser::parse_api(&source)
        .with_context(|| format!("failed to parse api file {}", api.display()))?;
    validate_project_kind(&spec, ProjectKind::Rest)?;

    fs::create_dir_all(out.join("tests"))
        .with_context(|| format!("failed to create {}", out.join("tests").display()))?;
    fs::write(
        out.join("Cargo.toml"),
        render_http_smoke_test_cargo_toml(&spec),
    )
    .with_context(|| format!("failed to write {}", out.join("Cargo.toml").display()))?;
    fs::write(
        out.join("tests/http_smoke.rs"),
        render_http_smoke_tests(&spec, base_url),
    )
    .with_context(|| {
        format!(
            "failed to write {}",
            out.join("tests/http_smoke.rs").display()
        )
    })?;
    fs::write(
        out.join("tests/multi_service_smoke.rs"),
        render_multi_service_smoke_tests(),
    )
    .with_context(|| {
        format!(
            "failed to write {}",
            out.join("tests/multi_service_smoke.rs").display()
        )
    })?;
    write_application_owned_file(
        &out.join("tests/fixtures.rs"),
        &render_http_smoke_fixtures(),
    )?;
    write_application_owned_file(
        &out.join("tests/assertions.rs"),
        &render_http_smoke_assertions(),
    )?;
    fs::write(
        out.join("README.md"),
        render_http_smoke_test_readme(&spec, api, base_url),
    )
    .with_context(|| format!("failed to write {}", out.join("README.md").display()))?;
    Ok(())
}

fn write_application_owned_file(path: &Path, content: &str) -> anyhow::Result<()> {
    if path.exists() {
        return Ok(());
    }
    fs::write(path, content).with_context(|| format!("failed to write {}", path.display()))
}

pub fn write_stream_worker_project(
    api: &Path,
    out: &Path,
    options: GenerateOptions,
) -> anyhow::Result<()> {
    let source = read_api_source(api)?;
    let spec = crate::parser::parse_api(&source)
        .with_context(|| format!("failed to parse api file {}", api.display()))?;
    if spec.rpc_methods.is_empty() {
        bail!(
            "{} has no rpc methods to map to stream topics",
            api.display()
        );
    }

    ensure_output(out, options.mode)?;
    fs::create_dir_all(out.join("src/config"))
        .with_context(|| format!("failed to create {}", out.join("src/config").display()))?;
    fs::create_dir_all(out.join("src/stream"))
        .with_context(|| format!("failed to create {}", out.join("src/stream").display()))?;
    fs::create_dir_all(out.join("src/types"))
        .with_context(|| format!("failed to create {}", out.join("src/types").display()))?;

    write_stream_cargo_toml(&spec, out, options)?;
    fs::write(out.join("README.md"), render_stream_readme(&spec, api))
        .with_context(|| format!("failed to write {}", out.join("README.md").display()))?;
    write_preserved(
        &out.join("config.yaml"),
        render_stream_config_yaml(&spec),
        options.mode,
    )?;
    fs::write(out.join("src/main.rs"), render_stream_main(&spec))
        .with_context(|| format!("failed to write {}", out.join("src/main.rs").display()))?;
    write_preserved(
        &out.join("src/config/mod.rs"),
        render_stream_config_rs(),
        options.mode,
    )?;
    fs::write(
        out.join("src/types/mod.rs"),
        types::render_types(&spec.types),
    )
    .with_context(|| format!("failed to write {}", out.join("src/types/mod.rs").display()))?;
    fs::write(out.join("src/stream/mod.rs"), render_stream_mod()).with_context(|| {
        format!(
            "failed to write {}",
            out.join("src/stream/mod.rs").display()
        )
    })?;
    fs::write(
        out.join("src/stream/envelope.rs"),
        render_stream_envelope(&spec),
    )
    .with_context(|| {
        format!(
            "failed to write {}",
            out.join("src/stream/envelope.rs").display()
        )
    })?;
    fs::write(
        out.join("src/stream/producer.rs"),
        render_stream_producer(&spec),
    )
    .with_context(|| {
        format!(
            "failed to write {}",
            out.join("src/stream/producer.rs").display()
        )
    })?;
    write_preserved(
        &out.join("src/stream/consumer.rs"),
        render_stream_consumer(&spec),
        options.mode,
    )?;
    register_workspace_member(out)?;
    Ok(())
}

fn write_api_markdown_doc_with(
    api: &Path,
    out: &Path,
    force: bool,
    render: fn(&ApiSpec, &Path) -> String,
) -> anyhow::Result<()> {
    if out.exists() && !force {
        bail!(
            "{} already exists; pass --force to overwrite it",
            out.display()
        );
    }

    let source = read_api_source(api)?;
    let spec = crate::parser::parse_api(&source)
        .with_context(|| format!("failed to parse api file {}", api.display()))?;
    if let Some(parent) = out.parent().filter(|parent| !parent.as_os_str().is_empty()) {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    fs::write(out, render(&spec, api)).with_context(|| format!("failed to write {}", out.display()))
}

fn write_stream_cargo_toml(
    spec: &ApiSpec,
    out: &Path,
    options: GenerateOptions,
) -> anyhow::Result<()> {
    let workspace_root = find_workspace_root(out)?;
    let local_crates_prefix = match options.dependency_source {
        DependencySource::Git => None,
        DependencySource::Path => Some(local_crates_prefix(
            out,
            workspace_root.as_deref().ok_or_else(|| {
                anyhow::anyhow!(
                    "--roze-source path requires a Cargo workspace containing the Roze crates"
                )
            })?,
        )?),
    };
    fs::write(
        out.join("Cargo.toml"),
        render_stream_cargo_toml(
            spec,
            out,
            options.dependency_source,
            local_crates_prefix.as_deref(),
            workspace_root.is_some(),
        ),
    )
    .with_context(|| format!("failed to write {}", out.join("Cargo.toml").display()))
}

fn render_stream_cargo_toml(
    spec: &ApiSpec,
    out: &Path,
    dependency_source: DependencySource,
    local_crates_prefix: Option<&str>,
    in_workspace: bool,
) -> String {
    let package_name = package_name_from_output(out, spec);
    let package = if in_workspace {
        r#"edition = "2021"
license.workspace = true
version.workspace = true"#
    } else {
        r#"edition = "2021"
license = "MIT"
version = "0.1.0""#
    };
    let common_dependencies = if in_workspace {
        r#"anyhow.workspace = true
config.workspace = true
serde.workspace = true
serde_json.workspace = true
tokio.workspace = true
tracing.workspace = true
tracing-subscriber.workspace = true
validator.workspace = true"#
    } else {
        r#"anyhow = "1"
config = { version = "0.15.24", default-features = false, features = ["json", "yaml", "toml"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tokio = { version = "1", features = ["macros", "rt-multi-thread", "signal", "sync", "time"] }
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
validator = { version = "0.20", features = ["derive"] }"#
    };
    let roze_dependencies =
        roze_dependencies(dependency_source, local_crates_prefix, &STREAM_ROZE_CRATES);

    format!(
        r#"[package]
name = "{package_name}"
{package}

[dependencies]
{common_dependencies}
{roze_dependencies}
"#
    )
}

fn render_stream_readme(spec: &ApiSpec, api: &Path) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    writeln!(&mut out, "# {} Stream Worker", spec.service).unwrap();
    writeln!(&mut out).unwrap();
    writeln!(&mut out, "Generated by `rozectl stream gen`.").unwrap();
    writeln!(&mut out).unwrap();
    writeln!(&mut out, "## Source").unwrap();
    writeln!(&mut out).unwrap();
    writeln!(&mut out, "- API: `{}`", api.display()).unwrap();
    writeln!(&mut out).unwrap();
    writeln!(&mut out, "## Topics").unwrap();
    writeln!(&mut out).unwrap();
    writeln!(&mut out, "| Method | Topic | DLQ | Request | Response |").unwrap();
    writeln!(&mut out, "| --- | --- | --- | --- | --- |").unwrap();
    for method in &spec.rpc_methods {
        writeln!(
            &mut out,
            "| `{}` | `{}` | `{}` | `{}` | `{}` |",
            method.name,
            stream_topic(spec, method),
            stream_dlq_topic(spec, method),
            method.request,
            method.response
        )
        .unwrap();
    }
    writeln!(&mut out).unwrap();
    writeln!(&mut out, "## Editable Files").unwrap();
    writeln!(&mut out).unwrap();
    writeln!(
        &mut out,
        "- `src/stream/consumer.rs` owns business handling and is preserved during `--update`."
    )
    .unwrap();
    writeln!(
        &mut out,
        "- `config.yaml` owns deploy-time stream settings and is preserved during `--update`."
    )
    .unwrap();
    out
}

fn render_stream_config_yaml(spec: &ApiSpec) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    writeln!(&mut out, "name: {}-stream", to_snake_case(&spec.service)).unwrap();
    writeln!(&mut out, "stream:").unwrap();
    writeln!(
        &mut out,
        "  consumer_group: {}-workers",
        to_snake_case(&spec.service)
    )
    .unwrap();
    writeln!(&mut out, "  topics:").unwrap();
    for method in &spec.rpc_methods {
        writeln!(&mut out, "    - method: {}", method.name).unwrap();
        writeln!(&mut out, "      topic: {}", stream_topic(spec, method)).unwrap();
        writeln!(
            &mut out,
            "      dead_letter_topic: {}",
            stream_dlq_topic(spec, method)
        )
        .unwrap();
    }
    out
}

fn render_stream_config_rs() -> String {
    r#"use std::path::Path;

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct AppConfig {
    pub name: String,
    pub stream: StreamConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StreamConfig {
    pub consumer_group: String,
    pub topics: Vec<StreamTopic>,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct StreamTopic {
    pub method: String,
    pub topic: String,
    pub dead_letter_topic: String,
}

pub fn load(path: impl AsRef<Path>) -> anyhow::Result<AppConfig> {
    let config = ::config::Config::builder()
        .add_source(::config::File::from(path.as_ref()).required(false))
        .build()?;
    Ok(config.try_deserialize()?)
}
"#
    .to_string()
}

fn render_stream_main(_spec: &ApiSpec) -> String {
    r#"mod config;
mod stream;
mod types;

use std::path::PathBuf;

use roze_mq::InMemoryBroker;
use roze_service::ServiceGroup;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let config = config::load(config_path())?;
    let broker = InMemoryBroker::new();
    tracing::info!(service = %config.name, group = %config.stream.consumer_group, "stream worker starting");
    let service_name = config.name.clone();
    let stream_config = config.stream.clone();
    let mut group = ServiceGroup::new();
    group.add_fn(service_name, move |shutdown| {
        let broker = broker.clone();
        let stream_config = stream_config.clone();
        async move {
            stream::consumer::run(&broker, &stream_config, shutdown).await
        }
    });
    group.start().await
}

fn config_path() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let manifest_config = manifest_dir.join("config.yaml");
    if manifest_config.exists() {
        manifest_config
    } else {
        PathBuf::from("config.yaml")
    }
}
"#
    .to_string()
}

fn render_stream_mod() -> String {
    r#"pub mod consumer;
pub mod envelope;
pub mod producer;
"#
    .to_string()
}

fn render_stream_envelope(spec: &ApiSpec) -> String {
    use std::fmt::Write as _;
    let mut out = String::from(
        "#![allow(dead_code)]\n\n#[derive(Debug, Clone, Copy, PartialEq, Eq)]\npub struct TopicBinding {\n    pub method: &'static str,\n    pub topic: &'static str,\n    pub dead_letter_topic: &'static str,\n    pub request: &'static str,\n    pub response: &'static str,\n}\n\n",
    );
    for method in &spec.rpc_methods {
        writeln!(
            &mut out,
            "pub const {}: &str = {:?};",
            stream_topic_const(method),
            stream_topic(spec, method)
        )
        .unwrap();
        writeln!(
            &mut out,
            "pub const {}: &str = {:?};",
            stream_dlq_const(method),
            stream_dlq_topic(spec, method)
        )
        .unwrap();
    }
    writeln!(&mut out).unwrap();
    writeln!(&mut out, "pub const BINDINGS: &[TopicBinding] = &[").unwrap();
    for method in &spec.rpc_methods {
        writeln!(
            &mut out,
            "    TopicBinding {{ method: {:?}, topic: {}, dead_letter_topic: {}, request: {:?}, response: {:?} }},",
            method.name,
            stream_topic_const(method),
            stream_dlq_const(method),
            method.request,
            method.response
        )
        .unwrap();
    }
    writeln!(&mut out, "];").unwrap();
    out
}

fn render_stream_producer(spec: &ApiSpec) -> String {
    use std::fmt::Write as _;
    let mut out = String::from(
        "#![allow(dead_code)]\n\nuse roze_mq::{Message, Publisher};\n\nuse crate::stream::envelope::*;\nuse crate::types::*;\n\n",
    );
    for method in &spec.rpc_methods {
        let fn_name = format!("publish_{}", to_snake_case(&method.name));
        writeln!(
            &mut out,
            "pub async fn {fn_name}<P>(publisher: &P, payload: {request}) -> anyhow::Result<()>\nwhere\n    P: Publisher,\n{{\n    let payload = serde_json::to_value(payload)?;\n    let idempotency_key = format!(\"{{}}:{{}}\", {topic_const}, payload);\n    let message = Message::new({topic_const}, payload)\n        .with_dead_letter_topic({dlq_const})\n        .with_idempotency_key(idempotency_key);\n    publisher.publish(message).await\n}}\n",
            request = method.request,
            topic_const = stream_topic_const(method),
            dlq_const = stream_dlq_const(method)
        )
        .unwrap();
    }
    out
}

fn render_stream_consumer(spec: &ApiSpec) -> String {
    use std::fmt::Write as _;
    let mut out = String::from(
        "use roze_mq::{Delivery, Subscriber};\nuse roze_shutdown::ShutdownListener;\n\nuse crate::stream::envelope::*;\nuse crate::types::*;\n\npub async fn run<S>(subscriber: &S, config: &crate::config::StreamConfig, shutdown: ShutdownListener) -> anyhow::Result<()>\nwhere\n    S: Subscriber,\n{\n    tracing::info!(group = %config.consumer_group, topics = config.topics.len(), \"subscribing stream topics\");\n    let mut workers = Vec::new();\n    for binding in BINDINGS {\n        let mut rx = subscriber.subscribe(binding.topic).await?;\n        let topic = binding.topic;\n        let worker_shutdown = shutdown.clone();\n        workers.push(tokio::spawn(async move {\n            loop {\n                tokio::select! {\n                    _ = worker_shutdown.clone().wait() => break,\n                    received = rx.recv() => {\n                        match received {\n                            Ok(delivery) => {\n                                if let Err(error) = dispatch(&delivery).await {\n                                    tracing::error!(topic = %topic, ?error, \"stream message failed\");\n                                    if let Err(error) = delivery.nack().await {\n                                        tracing::error!(topic = %topic, ?error, \"failed to nack stream message\");\n                                    }\n                                } else if let Err(error) = delivery.ack().await {\n                                    tracing::error!(topic = %topic, ?error, \"failed to ack stream message\");\n                                }\n                            }\n                            Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {\n                                tracing::warn!(topic = %topic, skipped, \"stream receiver lagged\");\n                            }\n                            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,\n                        }\n                    }\n                }\n            }\n        }));\n    }\n\n    shutdown.wait().await;\n    for worker in workers {\n        worker.await?;\n    }\n    Ok(())\n}\n\nasync fn dispatch(delivery: &Delivery) -> anyhow::Result<()> {\n    match delivery.message().topic.as_str() {\n",
    );
    for method in &spec.rpc_methods {
        writeln!(
            &mut out,
            "        {topic_const} => {{\n            let payload: {request} = serde_json::from_value(delivery.message().payload.clone())?;\n            handle_{method_name}(payload).await\n        }},",
            topic_const = stream_topic_const(method),
            request = method.request,
            method_name = to_snake_case(&method.name)
        )
        .unwrap();
    }
    out.push_str(
        "        topic => anyhow::bail!(\"unknown stream topic `{topic}`\"),\n    }\n}\n\n",
    );
    for method in &spec.rpc_methods {
        writeln!(
            &mut out,
            "pub async fn handle_{method_name}(_payload: {request}) -> anyhow::Result<()> {{\n    tracing::info!(method = {:?}, \"stream handler invoked\");\n    Ok(())\n}}\n",
            method.name,
            method_name = to_snake_case(&method.name),
            request = method.request
        )
        .unwrap();
    }
    out
}

fn stream_topic(spec: &ApiSpec, method: &RpcMethod) -> String {
    format!(
        "{}.{}",
        to_snake_case(&spec.service),
        to_snake_case(&method.name)
    )
}

fn stream_dlq_topic(spec: &ApiSpec, method: &RpcMethod) -> String {
    format!("{}.dlq", stream_topic(spec, method))
}

fn stream_topic_const(method: &RpcMethod) -> String {
    format!("TOPIC_{}", to_snake_case(&method.name).to_ascii_uppercase())
}

fn stream_dlq_const(method: &RpcMethod) -> String {
    format!("DLQ_{}", to_snake_case(&method.name).to_ascii_uppercase())
}

fn render_service_markdown_doc(spec: &ApiSpec, api: &Path) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let service = &spec.service;

    writeln!(&mut out, "# {service} Service").unwrap();
    writeln!(&mut out).unwrap();
    writeln!(&mut out, "Generated by `rozectl doc service`.").unwrap();
    writeln!(&mut out).unwrap();
    writeln!(&mut out, "## Purpose").unwrap();
    writeln!(&mut out).unwrap();
    writeln!(
        &mut out,
        "- Owns the `{service}` API surface declared in `{}`.",
        api.display()
    )
    .unwrap();
    writeln!(&mut out, "- Business behavior belongs in `src/logic/**`.").unwrap();
    writeln!(&mut out).unwrap();
    writeln!(&mut out, "## API Surface").unwrap();
    writeln!(&mut out).unwrap();
    if spec.rest_routes.is_empty() {
        writeln!(&mut out, "- No REST routes declared.").unwrap();
    } else {
        writeln!(&mut out, "| Method | Path | Handler | Request | Response |").unwrap();
        writeln!(&mut out, "| --- | --- | --- | --- | --- |").unwrap();
        for route in &spec.rest_routes {
            writeln!(
                &mut out,
                "| {} | `{}` | `{}` | `{}` | `{}` |",
                http_method_name(&route.method),
                route.path,
                route.handler.as_deref().unwrap_or("-"),
                route.request,
                route.response
            )
            .unwrap();
        }
    }
    writeln!(&mut out).unwrap();
    writeln!(&mut out, "## RPC Surface").unwrap();
    writeln!(&mut out).unwrap();
    if spec.rpc_methods.is_empty() {
        writeln!(&mut out, "- No RPC methods declared.").unwrap();
    } else {
        writeln!(&mut out, "| Method | Request | Response |").unwrap();
        writeln!(&mut out, "| --- | --- | --- |").unwrap();
        for method in &spec.rpc_methods {
            writeln!(
                &mut out,
                "| `{}` | `{}` | `{}` |",
                method.name, method.request, method.response
            )
            .unwrap();
        }
    }
    writeln!(&mut out).unwrap();
    writeln!(&mut out, "## Ownership").unwrap();
    writeln!(&mut out).unwrap();
    writeln!(&mut out, "| Path | Owner | Update rule |").unwrap();
    writeln!(&mut out, "| --- | --- | --- |").unwrap();
    writeln!(
        &mut out,
        "| `src/route/**` | framework | refreshed by `rozectl api generate --update` |"
    )
    .unwrap();
    writeln!(
        &mut out,
        "| `src/handler/**` | framework | refreshed by `rozectl api generate --update` |"
    )
    .unwrap();
    writeln!(
        &mut out,
        "| `src/types/**` | contract | refreshed from `.api` |"
    )
    .unwrap();
    writeln!(
        &mut out,
        "| `src/openapi/**` | contract | refreshed from `.api` |"
    )
    .unwrap();
    writeln!(
        &mut out,
        "| `src/logic/**` | application | preserved during `--update` |"
    )
    .unwrap();
    writeln!(
        &mut out,
        "| `src/svc/mod.rs` | application/dependencies | preserved during `--update` |"
    )
    .unwrap();
    writeln!(
        &mut out,
        "| `src/middleware/<custom>.rs` | application | preserved during `--update` |"
    )
    .unwrap();
    writeln!(
        &mut out,
        "| `config.yaml` | application/deploy | preserved during `--update` |"
    )
    .unwrap();
    writeln!(&mut out).unwrap();
    writeln!(&mut out, "## Common Commands").unwrap();
    writeln!(&mut out).unwrap();
    writeln!(
        &mut out,
        "- Generate/update: `rozectl api generate {} --out . --update`",
        api.display()
    )
    .unwrap();
    writeln!(
        &mut out,
        "- Preview changes: `rozectl diff api {} --out .`",
        api.display()
    )
    .unwrap();
    writeln!(
        &mut out,
        "- Format/check: `cargo fmt --check && cargo test`"
    )
    .unwrap();
    writeln!(&mut out).unwrap();
    writeln!(&mut out, "## AI Notes").unwrap();
    writeln!(&mut out).unwrap();
    writeln!(
        &mut out,
        "- Prefer editing `src/logic/**` and application-owned extension files."
    )
    .unwrap();
    writeln!(
        &mut out,
        "- Do not hand-edit generated route, handler, type, or OpenAPI files unless regenerating is impossible."
    )
    .unwrap();
    writeln!(
        &mut out,
        "- Run `rozectl diff` before applying generated updates when changing the contract."
    )
    .unwrap();

    out
}

fn render_ai_context_markdown_doc(spec: &ApiSpec, api: &Path) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let service = &spec.service;

    writeln!(&mut out, "# AI Context for {service}").unwrap();
    writeln!(&mut out).unwrap();
    writeln!(&mut out, "Generated by `rozectl doc ai-context`.").unwrap();
    writeln!(&mut out).unwrap();
    writeln!(&mut out, "## Contract").unwrap();
    writeln!(&mut out).unwrap();
    writeln!(&mut out, "- Source: `{}`", api.display()).unwrap();
    writeln!(&mut out, "- Service: `{service}`").unwrap();
    writeln!(&mut out, "- REST routes: {}", spec.rest_routes.len()).unwrap();
    writeln!(&mut out, "- RPC methods: {}", spec.rpc_methods.len()).unwrap();
    writeln!(&mut out).unwrap();
    writeln!(&mut out, "## Editable Paths").unwrap();
    writeln!(&mut out).unwrap();
    writeln!(&mut out, "- `src/logic/**`").unwrap();
    writeln!(&mut out, "- `src/middleware/<custom>.rs`").unwrap();
    writeln!(&mut out, "- `src/model/*_ext.rs`").unwrap();
    writeln!(
        &mut out,
        "- service-specific docs such as `SERVICE.md` and `AI_CONTEXT.md`"
    )
    .unwrap();
    writeln!(&mut out).unwrap();
    writeln!(&mut out, "## Generated Paths").unwrap();
    writeln!(&mut out).unwrap();
    writeln!(
        &mut out,
        "- `src/route/**`, `src/handler/**`, `src/types/**`, and `src/openapi/**` are generated from `.api`."
    )
    .unwrap();
    writeln!(
        &mut out,
        "- `src/lib.rs`, `src/server/**`, `src/client/**`, and proto/build files are generated for RPC projects."
    )
    .unwrap();
    writeln!(
        &mut out,
        "- `src/model/<model>.rs`, `src/model/<model>_fields.rs`, and `src/model/mod.rs` are schema-owned generated model files."
    )
    .unwrap();
    writeln!(
        &mut out,
        "- Prefer changing the contract or schema and regenerating instead of hand-editing generated files."
    )
    .unwrap();
    writeln!(&mut out).unwrap();
    writeln!(&mut out, "## Contract Change Flow").unwrap();
    writeln!(&mut out).unwrap();
    writeln!(
        &mut out,
        "1. Edit `{}` or the source schema/model file.",
        api.display()
    )
    .unwrap();
    writeln!(
        &mut out,
        "2. Preview with `rozectl diff api {} --out .`.",
        api.display()
    )
    .unwrap();
    writeln!(
        &mut out,
        "3. Apply with `rozectl api generate {} --out . --update`.",
        api.display()
    )
    .unwrap();
    writeln!(
        &mut out,
        "4. Keep custom behavior in application-owned files."
    )
    .unwrap();
    writeln!(&mut out, "5. Run formatting and tests before shipping.").unwrap();
    writeln!(&mut out).unwrap();
    writeln!(&mut out, "## Verification Commands").unwrap();
    writeln!(&mut out).unwrap();
    writeln!(&mut out, "- `cargo fmt --check`").unwrap();
    writeln!(&mut out, "- `cargo test`").unwrap();
    writeln!(
        &mut out,
        "- `cargo test -p rozectl -- --ignored --skip postgres --skip mysql` for generated model compile smoke"
    )
    .unwrap();
    writeln!(&mut out).unwrap();
    writeln!(&mut out, "## Interface Summary").unwrap();
    writeln!(&mut out).unwrap();
    if spec.rest_routes.is_empty() {
        writeln!(&mut out, "- REST: none").unwrap();
    } else {
        writeln!(&mut out, "- REST:").unwrap();
        for route in &spec.rest_routes {
            writeln!(
                &mut out,
                "  - {} `{}` -> `{}`",
                http_method_name(&route.method),
                route.path,
                route.response
            )
            .unwrap();
        }
    }
    if spec.rpc_methods.is_empty() {
        writeln!(&mut out, "- RPC: none").unwrap();
    } else {
        writeln!(&mut out, "- RPC:").unwrap();
        for method in &spec.rpc_methods {
            writeln!(
                &mut out,
                "  - `{}`: `{}` -> `{}`",
                method.name, method.request, method.response
            )
            .unwrap();
        }
    }

    out
}

fn render_mock_cargo_toml(spec: &ApiSpec) -> String {
    let package = sanitize_package_name(&format!("{}-mock", spec.service));
    format!(
        r#"[package]
name = "{package}"
version = "0.1.0"
edition = "2021"

[dependencies]
roze_http = {{ version = "0.8", default-features = false, features = ["http1", "json", "tokio"] }}
serde_json = "1"
tokio = {{ version = "1", features = ["macros", "net", "rt-multi-thread"] }}
"#
    )
}

fn render_mock_main(spec: &ApiSpec) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    writeln!(
        &mut out,
        "use roze_http::{{routing::{{delete, get, head, patch, post, put}}, Json, Router}};"
    )
    .unwrap();
    writeln!(&mut out).unwrap();
    writeln!(&mut out, "#[tokio::main]").unwrap();
    writeln!(&mut out, "async fn main() {{").unwrap();
    if spec.rest_routes.is_empty() {
        writeln!(&mut out, "    let app = Router::new();").unwrap();
    } else {
        writeln!(&mut out, "    let app = Router::new()").unwrap();
        for (idx, route) in spec.rest_routes.iter().enumerate() {
            let path = mock_roze_http_path(&rest::full_route_path_for_route(spec, route));
            let method = mock_roze_http_method(&route.method);
            let handler = mock_handler_ident(route, idx);
            writeln!(&mut out, "        .route({path:?}, {method}({handler}))").unwrap();
        }
        writeln!(&mut out, "        ;").unwrap();
    }
    writeln!(
        &mut out,
        "    let listener = tokio::net::TcpListener::bind(\"127.0.0.1:3000\")"
    )
    .unwrap();
    writeln!(&mut out, "        .await").unwrap();
    writeln!(&mut out, "        .expect(\"bind mock server\");").unwrap();
    writeln!(
        &mut out,
        "    println!(\"mock server listening on http://127.0.0.1:3000\");"
    )
    .unwrap();
    writeln!(
        &mut out,
        "    roze_http::serve(listener, app).await.expect(\"serve mock server\");"
    )
    .unwrap();
    writeln!(&mut out, "}}").unwrap();
    writeln!(&mut out).unwrap();

    for (idx, route) in spec.rest_routes.iter().enumerate() {
        let handler = mock_handler_ident(route, idx);
        let value = mock_json_for_type(spec, &route.response);
        let json = serde_json::to_string_pretty(&value).unwrap_or_else(|_| "{}".to_string());
        writeln!(
            &mut out,
            "async fn {handler}() -> Json<serde_json::Value> {{"
        )
        .unwrap();
        writeln!(&mut out, "    Json(serde_json::json!({json}))").unwrap();
        writeln!(&mut out, "}}").unwrap();
        writeln!(&mut out).unwrap();
    }

    out
}

fn render_mock_readme(spec: &ApiSpec, api: &Path) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    writeln!(&mut out, "# {} Mock Server", spec.service).unwrap();
    writeln!(&mut out).unwrap();
    writeln!(
        &mut out,
        "Generated from `{}` by `rozectl mock gen`.",
        api.display()
    )
    .unwrap();
    writeln!(&mut out).unwrap();
    writeln!(&mut out, "## Run").unwrap();
    writeln!(&mut out).unwrap();
    writeln!(&mut out, "```bash").unwrap();
    writeln!(&mut out, "cargo run").unwrap();
    writeln!(&mut out, "```").unwrap();
    writeln!(&mut out).unwrap();
    writeln!(&mut out, "The mock server listens on `127.0.0.1:3000`.").unwrap();
    writeln!(&mut out).unwrap();
    writeln!(&mut out, "## Routes").unwrap();
    writeln!(&mut out).unwrap();
    if spec.rest_routes.is_empty() {
        writeln!(&mut out, "- No REST routes declared.").unwrap();
    } else {
        for route in &spec.rest_routes {
            writeln!(
                &mut out,
                "- `{}` `{}` -> `{}`",
                http_method_name(&route.method),
                rest::full_route_path_for_route(spec, route),
                route.response
            )
            .unwrap();
        }
    }
    out
}

fn render_http_smoke_test_cargo_toml(spec: &ApiSpec) -> String {
    let package = sanitize_package_name(&format!("{}-contract-tests", spec.service));
    format!(
        r#"[package]
name = "{package}"
version = "0.1.0"
edition = "2021"

[dev-dependencies]
reqwest = {{ version = "0.12", default-features = false, features = ["json", "rustls-tls"] }}
serde_json = "1"
tokio = {{ version = "1", features = ["macros", "rt-multi-thread"] }}
"#
    )
}

fn render_http_smoke_fixtures() -> String {
    r#"#[derive(Debug, Clone, Default)]
pub struct RequestFixture {
    pub headers: Vec<(String, String)>,
    pub query: Vec<(String, String)>,
    pub json_body: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceFixture {
    pub name: String,
    pub base_url: String,
}

pub fn request(_route: &str) -> RequestFixture {
    RequestFixture::default()
}

pub fn services() -> Vec<ServiceFixture> {
    std::env::var("ROZE_E2E_SERVICES")
        .unwrap_or_default()
        .split(',')
        .filter_map(|entry| {
            let (name, base_url) = entry.split_once('=')?;
            Some(ServiceFixture {
                name: name.trim().to_string(),
                base_url: base_url.trim().trim_end_matches('/').to_string(),
            })
        })
        .filter(|service| !service.name.is_empty() && !service.base_url.is_empty())
        .collect()
}
"#
    .to_string()
}

fn render_http_smoke_assertions() -> String {
    r#"pub async fn assert_route(
    _route: &str,
    _status: reqwest::StatusCode,
    _body: Option<&serde_json::Value>,
) -> Result<(), Box<dyn std::error::Error>> {
    Ok(())
}

pub async fn assert_service_ready(
    _name: &str,
    status: reqwest::StatusCode,
    _body: &serde_json::Value,
) -> Result<(), Box<dyn std::error::Error>> {
    assert!(status.is_success(), "service readiness returned {status}");
    Ok(())
}
"#
    .to_string()
}

fn render_multi_service_smoke_tests() -> String {
    r#"#[path = "assertions.rs"]
mod assertions;
#[path = "fixtures.rs"]
mod fixtures;

#[tokio::test]
async fn smoke_configured_services_are_ready() -> Result<(), Box<dyn std::error::Error>> {
    let services = fixtures::services();
    if services.is_empty() {
        return Ok(());
    }
    let client = reqwest::Client::new();
    for service in services {
        let response = client
            .get(format!("{}/readyz", service.base_url))
            .send()
            .await?;
        let status = response.status();
        let body: serde_json::Value = response.json().await?;
        assertions::assert_service_ready(&service.name, status, &body).await?;
    }
    Ok(())
}
"#
    .to_string()
}

fn render_http_smoke_tests(spec: &ApiSpec, base_url: &str) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    writeln!(&mut out, "#[path = \"assertions.rs\"]").unwrap();
    writeln!(&mut out, "mod assertions;").unwrap();
    writeln!(&mut out, "#[path = \"fixtures.rs\"]").unwrap();
    writeln!(&mut out, "mod fixtures;").unwrap();
    writeln!(&mut out, "use reqwest::StatusCode;").unwrap();
    writeln!(&mut out).unwrap();
    writeln!(
        &mut out,
        "fn base_url() -> String {{ std::env::var(\"ROZE_TEST_BASE_URL\").unwrap_or_else(|_| {:?}.to_string()) }}",
        base_url.trim_end_matches('/')
    )
    .unwrap();
    writeln!(&mut out).unwrap();

    render_framework_http_smoke_tests(&mut out, spec);

    for (idx, route) in spec.rest_routes.iter().enumerate() {
        let test_name = http_smoke_test_name(route, idx);
        let method = http_method_name(&route.method).to_ascii_lowercase();
        let path = http_smoke_sample_path(spec, route);
        let headers = http_smoke_header_fields(spec, route);
        let query = http_smoke_query_fields(spec, route);
        let form = http_smoke_form_fields(spec, route);
        let body = http_smoke_json_body(spec, route);
        writeln!(&mut out, "#[tokio::test]").unwrap();
        writeln!(
            &mut out,
            "async fn {test_name}() -> Result<(), Box<dyn std::error::Error>> {{"
        )
        .unwrap();
        writeln!(&mut out, "    let client = reqwest::Client::new();").unwrap();
        writeln!(
            &mut out,
            "    let url = format!(\"{{}}{{}}\", base_url().trim_end_matches('/'), {:?});",
            path
        )
        .unwrap();
        writeln!(&mut out, "    let mut request = client.{method}(url)").unwrap();
        for (name, value) in headers {
            writeln!(&mut out, "        .header({name:?}, {value:?})").unwrap();
        }
        if !query.is_empty() {
            writeln!(
                &mut out,
                "        .query(&{:?})",
                query
                    .iter()
                    .map(|(name, value)| (name.as_str(), value.as_str()))
                    .collect::<Vec<_>>()
            )
            .unwrap();
        }
        if !form.is_empty() {
            writeln!(
                &mut out,
                "        .form(&{:?})",
                form.iter()
                    .map(|(name, value)| (name.as_str(), value.as_str()))
                    .collect::<Vec<_>>()
            )
            .unwrap();
        } else if let Some(body) = body {
            let body = serde_json::to_string_pretty(&body).unwrap_or_else(|_| "{}".to_string());
            writeln!(&mut out, "        .json(&serde_json::json!({body}))").unwrap();
        }
        writeln!(&mut out, "        ;").unwrap();
        writeln!(
            &mut out,
            "    let fixture = fixtures::request({test_name:?});"
        )
        .unwrap();
        writeln!(
            &mut out,
            "    for (name, value) in fixture.headers {{ request = request.header(name, value); }}"
        )
        .unwrap();
        writeln!(
            &mut out,
            "    if !fixture.query.is_empty() {{ request = request.query(&fixture.query); }}"
        )
        .unwrap();
        writeln!(
            &mut out,
            "    if let Some(body) = fixture.json_body {{ request = request.json(&body); }}"
        )
        .unwrap();
        writeln!(&mut out, "    let response = request.send().await?;").unwrap();
        writeln!(&mut out).unwrap();
        writeln!(
            &mut out,
            "    assert!(response.status().is_success(), \"expected success, got {{}}\", response.status());"
        )
        .unwrap();
        writeln!(&mut out, "    let status = response.status();").unwrap();
        writeln!(
            &mut out,
            "    let body = if status != StatusCode::NO_CONTENT {{"
        )
        .unwrap();
        writeln!(
            &mut out,
            "        let content_type = response.headers().get(reqwest::header::CONTENT_TYPE).and_then(|value| value.to_str().ok()).unwrap_or(\"\");"
        )
        .unwrap();
        writeln!(
            &mut out,
            "        assert!(content_type.contains(\"json\"), \"expected JSON response, got {{content_type}}\");"
        )
        .unwrap();
        writeln!(
            &mut out,
            "        Some(response.json::<serde_json::Value>().await?)"
        )
        .unwrap();
        writeln!(&mut out, "    }} else {{ None }};").unwrap();
        writeln!(
            &mut out,
            "    assertions::assert_route({test_name:?}, status, body.as_ref()).await?;"
        )
        .unwrap();
        writeln!(&mut out, "    Ok(())").unwrap();
        writeln!(&mut out, "}}").unwrap();
        writeln!(&mut out).unwrap();
    }

    out
}

fn render_framework_http_smoke_tests(out: &mut String, spec: &ApiSpec) {
    let endpoints = [
        (
            "framework_healthz",
            rest::full_route_path(spec, "/healthz"),
            Vec::<(&str, &str)>::new(),
            true,
        ),
        (
            "framework_readyz",
            rest::full_route_path(spec, "/readyz"),
            Vec::<(&str, &str)>::new(),
            true,
        ),
        (
            "framework_startupz",
            rest::full_route_path(spec, "/startupz"),
            Vec::<(&str, &str)>::new(),
            true,
        ),
        (
            "framework_metrics",
            rest::full_route_path(spec, "/metrics"),
            Vec::<(&str, &str)>::new(),
            false,
        ),
        (
            "framework_openapi",
            rest::full_route_path(spec, "/openapi.json"),
            Vec::<(&str, &str)>::new(),
            true,
        ),
        (
            "framework_report_export",
            rest::full_route_path(spec, "/reports/export"),
            vec![
                ("report", "smoke"),
                ("format", "csv"),
                ("from", "2026-01-01T00:00:00Z"),
                ("to", "2026-01-01T01:00:00Z"),
                ("filters", "env=smoke"),
            ],
            true,
        ),
        (
            "framework_chart_query",
            rest::full_route_path(spec, "/charts/query"),
            vec![
                ("chart", "smoke"),
                ("interval", "1m"),
                ("from", "2026-01-01T00:00:00Z"),
                ("to", "2026-01-01T01:00:00Z"),
                ("filters", "env=smoke"),
            ],
            true,
        ),
    ];
    for (name, path, query, expects_json) in endpoints {
        render_framework_http_smoke_test(out, name, &path, &query, expects_json);
    }
}

fn render_framework_http_smoke_test(
    out: &mut String,
    name: &str,
    path: &str,
    query: &[(&str, &str)],
    expects_json: bool,
) {
    use std::fmt::Write as _;
    writeln!(out, "#[tokio::test]").unwrap();
    writeln!(
        out,
        "async fn smoke_{name}() -> Result<(), Box<dyn std::error::Error>> {{"
    )
    .unwrap();
    writeln!(out, "    let client = reqwest::Client::new();").unwrap();
    writeln!(
        out,
        "    let url = format!(\"{{}}{{}}\", base_url().trim_end_matches('/'), {:?});",
        path
    )
    .unwrap();
    writeln!(out, "    let response = client.get(url)").unwrap();
    if !query.is_empty() {
        writeln!(out, "        .query(&{query:?})").unwrap();
    }
    writeln!(out, "        .send()").unwrap();
    writeln!(out, "        .await?;").unwrap();
    writeln!(out).unwrap();
    writeln!(
        out,
        "    assert!(response.status().is_success(), \"expected success, got {{}}\", response.status());"
    )
    .unwrap();
    if expects_json {
        writeln!(
            out,
            "    let content_type = response.headers().get(reqwest::header::CONTENT_TYPE).and_then(|value| value.to_str().ok()).unwrap_or(\"\");"
        )
        .unwrap();
        writeln!(
            out,
            "    assert!(content_type.contains(\"json\"), \"expected JSON response, got {{content_type}}\");"
        )
        .unwrap();
        writeln!(
            out,
            "    let _: serde_json::Value = response.json().await?;"
        )
        .unwrap();
    } else {
        writeln!(out, "    let _ = response.bytes().await?;").unwrap();
    }
    writeln!(out, "    Ok(())").unwrap();
    writeln!(out, "}}").unwrap();
    writeln!(out).unwrap();
}

fn render_http_smoke_test_readme(spec: &ApiSpec, api: &Path, base_url: &str) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    writeln!(&mut out, "# {} Contract Tests", spec.service).unwrap();
    writeln!(&mut out).unwrap();
    writeln!(
        &mut out,
        "Generated from `{}` by `rozectl test gen`.",
        api.display()
    )
    .unwrap();
    writeln!(&mut out).unwrap();
    writeln!(&mut out, "## Run").unwrap();
    writeln!(&mut out).unwrap();
    writeln!(&mut out, "```bash").unwrap();
    writeln!(&mut out, "ROZE_TEST_BASE_URL={} cargo test", base_url).unwrap();
    writeln!(&mut out, "```").unwrap();
    writeln!(&mut out).unwrap();
    writeln!(
        &mut out,
        "If `ROZE_TEST_BASE_URL` is not set, tests use `{}`.",
        base_url
    )
    .unwrap();
    writeln!(&mut out).unwrap();
    writeln!(&mut out, "`tests/fixtures.rs` and `tests/assertions.rs` are application-owned and preserved across regeneration.").unwrap();
    writeln!(&mut out, "Set `ROZE_E2E_SERVICES=name=http://host:port,...` to run the generated multi-service readiness flow.").unwrap();
    writeln!(&mut out).unwrap();
    writeln!(&mut out, "## Framework Smoke").unwrap();
    writeln!(&mut out).unwrap();
    for path in [
        "/healthz",
        "/readyz",
        "/startupz",
        "/metrics",
        "/openapi.json",
        "/reports/export",
        "/charts/query",
    ] {
        writeln!(&mut out, "- `GET` `{}`", rest::full_route_path(spec, path)).unwrap();
    }
    writeln!(&mut out).unwrap();
    writeln!(&mut out, "## Routes").unwrap();
    writeln!(&mut out).unwrap();
    if spec.rest_routes.is_empty() {
        writeln!(&mut out, "- No REST routes declared.").unwrap();
    } else {
        for route in &spec.rest_routes {
            writeln!(
                &mut out,
                "- `{}` `{}`",
                http_method_name(&route.method),
                rest::full_route_path_for_route(spec, route)
            )
            .unwrap();
        }
    }
    out
}

fn http_smoke_test_name(route: &crate::parser::RestRoute, idx: usize) -> String {
    let base = route.handler.as_deref().unwrap_or(route.path.as_str());
    format!("smoke_{}_{}", sanitize_rust_ident(base), idx)
}

fn http_smoke_sample_path(spec: &ApiSpec, route: &crate::parser::RestRoute) -> String {
    let request_ty = spec.types.iter().find(|ty| ty.name == route.request);
    let mut path = rest::full_route_path_for_route(spec, route);
    for param in route_path_params(&path) {
        let value = request_ty
            .and_then(|ty| {
                ty.fields
                    .iter()
                    .find(|field| normalize_ident(&field_wire_name(field)) == param)
            })
            .map(|field| http_smoke_sample_string(&field.ty))
            .unwrap_or_else(|| "1".to_string());
        path = path.replace(&format!(":{param}"), &value);
        path = path.replace(&format!("{{{param}}}"), &value);
    }
    path
}

fn http_smoke_query_fields(
    spec: &ApiSpec,
    route: &crate::parser::RestRoute,
) -> Vec<(String, String)> {
    http_smoke_fields_for_source(spec, route, crate::parser::FieldSource::Query)
}

fn http_smoke_header_fields(
    spec: &ApiSpec,
    route: &crate::parser::RestRoute,
) -> Vec<(String, String)> {
    http_smoke_fields_for_source(spec, route, crate::parser::FieldSource::Header)
}

fn http_smoke_form_fields(
    spec: &ApiSpec,
    route: &crate::parser::RestRoute,
) -> Vec<(String, String)> {
    http_smoke_fields_for_source(spec, route, crate::parser::FieldSource::Form)
}

fn http_smoke_fields_for_source(
    spec: &ApiSpec,
    route: &crate::parser::RestRoute,
    source: crate::parser::FieldSource,
) -> Vec<(String, String)> {
    let Some(request_ty) = spec.types.iter().find(|ty| ty.name == route.request) else {
        return Vec::new();
    };
    request_ty
        .fields
        .iter()
        .filter(|field| openapi_field_source(field, route) == source)
        .map(|field| (field_wire_name(field), http_smoke_sample_string(&field.ty)))
        .collect()
}

fn http_smoke_json_body(
    spec: &ApiSpec,
    route: &crate::parser::RestRoute,
) -> Option<serde_json::Value> {
    let request_ty = spec.types.iter().find(|ty| ty.name == route.request)?;
    let mut map = serde_json::Map::new();
    for field in &request_ty.fields {
        if openapi_field_source(field, route) == crate::parser::FieldSource::Json {
            map.insert(
                field_wire_name(field),
                mock_json_for_field_type(spec, &field.ty),
            );
        }
    }
    if map.is_empty() {
        None
    } else {
        Some(serde_json::Value::Object(map))
    }
}

fn http_smoke_sample_string(ty: &str) -> String {
    let normalized = ty.trim();
    if normalized.starts_with("[]") {
        return "string".to_string();
    }
    match normalized {
        "bool" | "boolean" => "true".to_string(),
        "i8" | "i16" | "i32" | "i64" | "int" | "int8" | "int16" | "int32" | "int64" | "u8"
        | "u16" | "u32" | "u64" | "uint" | "uint8" | "uint16" | "uint32" | "uint64" => {
            "1".to_string()
        }
        "f32" | "f64" | "float" | "double" => "1.0".to_string(),
        _ => "string".to_string(),
    }
}

fn mock_roze_http_method(method: &crate::parser::HttpMethod) -> &'static str {
    match method {
        crate::parser::HttpMethod::Get => "get",
        crate::parser::HttpMethod::Head => "head",
        crate::parser::HttpMethod::Post => "post",
        crate::parser::HttpMethod::Put => "put",
        crate::parser::HttpMethod::Patch => "patch",
        crate::parser::HttpMethod::Delete => "delete",
    }
}

fn mock_roze_http_path(path: &str) -> String {
    path.split('/')
        .map(|segment| {
            if let Some(param) = segment.strip_prefix(':') {
                format!("{{{param}}}")
            } else {
                segment.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn mock_handler_ident(route: &crate::parser::RestRoute, idx: usize) -> String {
    let base = route
        .handler
        .as_deref()
        .map(str::to_string)
        .unwrap_or_else(|| format!("{}_{}", mock_roze_http_method(&route.method), route.path));
    format!("{}_{}", sanitize_rust_ident(&base), idx)
}

fn mock_json_for_type(spec: &ApiSpec, ty: &str) -> serde_json::Value {
    let Some(type_def) = spec.types.iter().find(|item| item.name == ty) else {
        return serde_json::json!({});
    };
    let mut map = serde_json::Map::new();
    for field in &type_def.fields {
        map.insert(
            field_wire_name(field),
            mock_json_for_field_type(spec, &field.ty),
        );
    }
    serde_json::Value::Object(map)
}

fn mock_json_for_field_type(spec: &ApiSpec, ty: &str) -> serde_json::Value {
    let normalized = ty.trim();
    if let Some(inner) = normalized.strip_prefix("[]") {
        return serde_json::Value::Array(vec![mock_json_for_field_type(spec, inner)]);
    }
    match normalized {
        "String" | "string" => serde_json::json!("string"),
        "bool" | "boolean" => serde_json::json!(true),
        "i8" | "i16" | "i32" | "i64" | "int" | "int8" | "int16" | "int32" | "int64" | "u8"
        | "u16" | "u32" | "u64" | "uint" | "uint8" | "uint16" | "uint32" | "uint64" => {
            serde_json::json!(1)
        }
        "f32" | "f64" | "float" | "double" => serde_json::json!(1.0),
        other => mock_json_for_type(spec, other),
    }
}

fn sanitize_package_name(input: &str) -> String {
    let mut out = input
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>();
    while out.contains("--") {
        out = out.replace("--", "-");
    }
    let trimmed = out.trim_matches('-');
    if trimmed.is_empty() {
        "mock-server".to_string()
    } else {
        trimmed.to_string()
    }
}

fn sanitize_rust_ident(input: &str) -> String {
    let mut out = String::new();
    for ch in input.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push('_');
        }
    }
    while out.contains("__") {
        out = out.replace("__", "_");
    }
    let out = out.trim_matches('_');
    if out.is_empty() || out.chars().next().is_some_and(|ch| ch.is_ascii_digit()) {
        format!("mock_{out}")
    } else {
        out.to_string()
    }
}

fn http_method_name(method: &HttpMethod) -> &'static str {
    match method {
        HttpMethod::Get => "GET",
        HttpMethod::Head => "HEAD",
        HttpMethod::Post => "POST",
        HttpMethod::Put => "PUT",
        HttpMethod::Patch => "PATCH",
        HttpMethod::Delete => "DELETE",
    }
}

pub fn write_openapi_json(api: &Path, out: &Path) -> anyhow::Result<()> {
    let document = openapi_document_from_api(api)?;
    if let Some(parent) = out.parent().filter(|parent| !parent.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    fs::write(out, serde_json::to_string_pretty(&document)?)
        .with_context(|| format!("failed to write {}", out.display()))
}

pub fn write_openapi_yaml(api: &Path, out: &Path) -> anyhow::Result<()> {
    let document = openapi_document_from_api(api)?;
    if let Some(parent) = out.parent().filter(|parent| !parent.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    fs::write(out, render_yaml_document(&document))
        .with_context(|| format!("failed to write {}", out.display()))
}

fn openapi_document_from_api(api: &Path) -> anyhow::Result<serde_json::Value> {
    let source = read_api_source(api)?;
    let spec = crate::parser::parse_api(&source)?;
    validate_project_kind(&spec, ProjectKind::Rest)?;
    Ok(openapi_document(&spec))
}

fn render_yaml_document(value: &serde_json::Value) -> String {
    let mut out = String::new();
    render_yaml_value(value, 0, &mut out);
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

fn render_yaml_value(value: &serde_json::Value, indent: usize, out: &mut String) {
    match value {
        serde_json::Value::Object(map) if map.is_empty() => {
            out.push_str(&" ".repeat(indent));
            out.push_str("{}\n");
        }
        serde_json::Value::Object(map) => {
            for (key, value) in map {
                out.push_str(&" ".repeat(indent));
                out.push_str(&render_yaml_key(key));
                if yaml_scalar(value).is_some() || is_empty_collection(value) {
                    out.push_str(": ");
                    out.push_str(&render_yaml_inline(value));
                    out.push('\n');
                } else {
                    out.push_str(":\n");
                    render_yaml_value(value, indent + 2, out);
                }
            }
        }
        serde_json::Value::Array(values) if values.is_empty() => {
            out.push_str(&" ".repeat(indent));
            out.push_str("[]\n");
        }
        serde_json::Value::Array(values) => {
            for value in values {
                out.push_str(&" ".repeat(indent));
                if yaml_scalar(value).is_some() || is_empty_collection(value) {
                    out.push_str("- ");
                    out.push_str(&render_yaml_inline(value));
                    out.push('\n');
                } else {
                    out.push_str("-\n");
                    render_yaml_value(value, indent + 2, out);
                }
            }
        }
        value => {
            out.push_str(&" ".repeat(indent));
            out.push_str(&render_yaml_inline(value));
            out.push('\n');
        }
    }
}

fn render_yaml_inline(value: &serde_json::Value) -> String {
    match yaml_scalar(value) {
        Some(scalar) => scalar,
        None if matches!(value, serde_json::Value::Array(values) if values.is_empty()) => {
            "[]".to_string()
        }
        None if matches!(value, serde_json::Value::Object(map) if map.is_empty()) => {
            "{}".to_string()
        }
        None => quote_yaml_string(&value.to_string()),
    }
}

fn yaml_scalar(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::Null => Some("null".to_string()),
        serde_json::Value::Bool(value) => Some(value.to_string()),
        serde_json::Value::Number(value) => Some(value.to_string()),
        serde_json::Value::String(value) => Some(quote_yaml_string(value)),
        serde_json::Value::Array(_) | serde_json::Value::Object(_) => None,
    }
}

fn is_empty_collection(value: &serde_json::Value) -> bool {
    matches!(value, serde_json::Value::Array(values) if values.is_empty())
        || matches!(value, serde_json::Value::Object(map) if map.is_empty())
}

fn render_yaml_key(key: &str) -> String {
    if !key.is_empty()
        && key
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
    {
        key.to_string()
    } else {
        quote_yaml_string(key)
    }
}

fn quote_yaml_string(value: &str) -> String {
    let escaped = value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t");
    format!("\"{escaped}\"")
}

pub(super) fn openapi_document(spec: &ApiSpec) -> serde_json::Value {
    let mut paths = serde_json::Map::<String, serde_json::Value>::new();
    for route in &spec.rest_routes {
        let path = openapi_path(&rest::full_route_path_for_openapi(spec, route));
        let method = openapi_method_name(&route.method);
        let path_item = paths
            .entry(path)
            .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
        let serde_json::Value::Object(path_item) = path_item else {
            continue;
        };
        path_item.insert(method.to_string(), openapi_operation(spec, route));
    }

    let mut schemas = serde_json::Map::new();
    for ty in &spec.types {
        schemas.insert(ty.name.clone(), openapi_type_schema_for_spec(spec, ty));
    }

    let mut components = serde_json::Map::new();
    components.insert("schemas".to_string(), serde_json::Value::Object(schemas));
    if spec
        .rest_routes
        .iter()
        .any(|route| route_has_jwt(spec, route))
    {
        components.insert(
            "securitySchemes".to_string(),
            serde_json::json!({
                "bearerAuth": {
                    "type": "http",
                    "scheme": "bearer",
                    "bearerFormat": "JWT"
                }
            }),
        );
    }

    serde_json::json!({
        "openapi": "3.0.0",
        "info": { "title": spec.service, "version": "0.1.0" },
        "servers": openapi_servers(spec),
        "paths": paths,
        "components": components
    })
}

fn openapi_operation(spec: &ApiSpec, route: &crate::parser::RestRoute) -> serde_json::Value {
    let request_ty = spec.types.iter().find(|ty| ty.name == route.request);
    let mut operation = serde_json::Map::new();
    operation.insert(
        "operationId".to_string(),
        serde_json::json!(route
            .handler
            .clone()
            .unwrap_or_else(|| rest::handler_name_for_openapi(&route.method, &route.path))),
    );
    operation.insert("tags".to_string(), serde_json::json!([spec.service]));
    if let Some(doc) = &route.doc {
        operation.insert("summary".to_string(), serde_json::json!(doc));
    }
    if route_has_jwt(spec, route)
        || route
            .middlewares
            .iter()
            .any(|mw| mw == "auth" || mw == "jwt")
    {
        operation.insert(
            "security".to_string(),
            serde_json::json!([{ "bearerAuth": [] }]),
        );
    }
    if !route.permissions.is_empty() {
        operation.insert(
            "x-roze-permissions".to_string(),
            serde_json::json!(route.permissions),
        );
    }

    let mut parameters = Vec::new();
    let mut json_body_fields = Vec::new();
    let mut form_body_fields = Vec::new();
    if let Some(request_ty) = request_ty {
        for field in expanded_api_fields(spec, request_ty) {
            match openapi_field_source(field, route) {
                crate::parser::FieldSource::Path => {
                    parameters.push(openapi_parameter(field, "path", true))
                }
                crate::parser::FieldSource::Query => {
                    parameters.push(openapi_parameter(field, "query", false))
                }
                crate::parser::FieldSource::Header => {
                    parameters.push(openapi_parameter(field, "header", false))
                }
                crate::parser::FieldSource::Form => form_body_fields.push(field),
                crate::parser::FieldSource::Json => json_body_fields.push(field),
                crate::parser::FieldSource::Auto => {}
            }
        }
    }
    if !parameters.is_empty() {
        operation.insert(
            "parameters".to_string(),
            serde_json::Value::Array(parameters),
        );
    }

    if !form_body_fields.is_empty() {
        operation.insert(
            "requestBody".to_string(),
            openapi_request_body("application/x-www-form-urlencoded", &form_body_fields),
        );
    } else if !json_body_fields.is_empty()
        || (request_ty.is_some_and(|ty| !expanded_api_fields(spec, ty).is_empty())
            && matches!(
                route.method,
                crate::parser::HttpMethod::Post
                    | crate::parser::HttpMethod::Put
                    | crate::parser::HttpMethod::Patch
            ))
    {
        operation.insert(
            "requestBody".to_string(),
            if json_body_fields.is_empty() {
                serde_json::json!({
                    "required": true,
                    "content": {
                        "application/json": {
                            "schema": { "$ref": format!("#/components/schemas/{}", route.request) }
                        }
                    }
                })
            } else {
                openapi_request_body("application/json", &json_body_fields)
            },
        );
    }

    operation.insert(
        "responses".to_string(),
        serde_json::json!({
            "200": {
                "description": "OK",
                "content": {
                    "application/json": {
                        "schema": { "$ref": format!("#/components/schemas/{}", route.response) }
                    }
                }
            }
        }),
    );

    serde_json::Value::Object(operation)
}

fn openapi_servers(spec: &ApiSpec) -> serde_json::Value {
    let mut seen = HashSet::new();
    let mut servers = Vec::new();
    if let Some(prefix) = spec
        .server
        .as_ref()
        .and_then(|server| server.prefix.as_deref())
    {
        if seen.insert(prefix.to_string()) {
            servers.push(serde_json::json!({ "url": prefix }));
        }
    }
    for route in &spec.rest_routes {
        if let Some(prefix) = route
            .server
            .as_ref()
            .and_then(|server| server.prefix.as_deref())
        {
            if seen.insert(prefix.to_string()) {
                servers.push(serde_json::json!({ "url": prefix }));
            }
        }
    }
    if servers.is_empty() {
        servers.push(serde_json::json!({ "url": "/" }));
    }
    serde_json::Value::Array(servers)
}

fn route_has_jwt(spec: &ApiSpec, route: &crate::parser::RestRoute) -> bool {
    route
        .server
        .as_ref()
        .and_then(|server| server.jwt.as_ref())
        .or_else(|| spec.server.as_ref().and_then(|server| server.jwt.as_ref()))
        .is_some()
}

fn openapi_type_schema_for_spec(spec: &ApiSpec, ty: &crate::parser::TypeDef) -> serde_json::Value {
    let mut properties = serde_json::Map::new();
    let mut required = Vec::new();
    for field in expanded_api_fields(spec, ty) {
        properties.insert(field_wire_name(field), openapi_schema(&field.ty));
        required.push(serde_json::Value::String(field_wire_name(field)));
    }
    serde_json::json!({
        "type": "object",
        "properties": properties,
        "required": required
    })
}

fn openapi_parameter(
    field: &crate::parser::Field,
    location: &str,
    required: bool,
) -> serde_json::Value {
    serde_json::json!({
        "name": field_wire_name(field),
        "in": location,
        "required": required,
        "schema": openapi_schema(&field.ty)
    })
}

fn openapi_request_body(content_type: &str, fields: &[&crate::parser::Field]) -> serde_json::Value {
    let mut properties = serde_json::Map::new();
    let mut required = Vec::new();
    for field in fields {
        properties.insert(field_wire_name(field), openapi_schema(&field.ty));
        required.push(serde_json::Value::String(field_wire_name(field)));
    }
    serde_json::json!({
        "required": true,
        "content": {
            content_type: {
                "schema": {
                    "type": "object",
                    "properties": properties,
                    "required": required
                }
            }
        }
    })
}

fn expanded_api_fields<'a>(
    spec: &'a ApiSpec,
    ty: &'a crate::parser::TypeDef,
) -> Vec<&'a crate::parser::Field> {
    let mut fields = Vec::new();
    let mut stack = HashSet::new();
    expand_api_fields(spec, ty, &mut stack, &mut fields);
    fields
}

fn expand_api_fields<'a>(
    spec: &'a ApiSpec,
    ty: &'a crate::parser::TypeDef,
    stack: &mut HashSet<String>,
    fields: &mut Vec<&'a crate::parser::Field>,
) {
    if !stack.insert(ty.name.clone()) {
        return;
    }
    for field in &ty.fields {
        if field.embedded {
            if let Some(nested) = spec
                .types
                .iter()
                .find(|candidate| candidate.name == field.ty)
            {
                expand_api_fields(spec, nested, stack, fields);
                continue;
            }
        }
        fields.push(field);
    }
    stack.remove(&ty.name);
}

fn openapi_schema(ty: &str) -> serde_json::Value {
    if let Some((_, value)) = map_key_value_types(ty) {
        return serde_json::json!({
            "type": "object",
            "additionalProperties": openapi_schema(&value)
        });
    }
    if let Some(inner) = collection_element_type(ty) {
        return serde_json::json!({
            "type": "array",
            "items": openapi_schema(&inner)
        });
    }
    match ty {
        "String" | "string" => serde_json::json!({ "type": "string" }),
        "bool" | "boolean" => serde_json::json!({ "type": "boolean" }),
        "i32" | "int32" => serde_json::json!({ "type": "integer", "format": "int32" }),
        "i64" | "int" | "int64" => serde_json::json!({ "type": "integer", "format": "int64" }),
        "u32" | "uint32" => serde_json::json!({ "type": "integer", "format": "uint32" }),
        "u64" | "uint" | "uint64" => serde_json::json!({ "type": "integer", "format": "uint64" }),
        "f32" | "float" => serde_json::json!({ "type": "number", "format": "float" }),
        "f64" | "double" => serde_json::json!({ "type": "number", "format": "double" }),
        other => serde_json::json!({ "$ref": format!("#/components/schemas/{other}") }),
    }
}

fn collection_element_type(ty: &str) -> Option<String> {
    let ty = ty.trim();
    if let Some(inner) = ty.strip_prefix("[]") {
        return Some(inner.trim_start_matches('*').trim().to_string());
    }
    if let Some(inner) = ty
        .strip_prefix("Vec<")
        .and_then(|raw| raw.strip_suffix('>'))
    {
        return Some(inner.trim_start_matches('*').trim().to_string());
    }
    None
}

fn map_key_value_types(ty: &str) -> Option<(String, String)> {
    let ty = ty.trim();
    if let Some(rest) = ty.strip_prefix("map[") {
        let (key, value) = rest.split_once(']')?;
        return Some((key.trim().to_string(), value.trim().to_string()));
    }
    if let Some(inner) = ty
        .strip_prefix("HashMap<")
        .and_then(|raw| raw.strip_suffix('>'))
    {
        let (key, value) = inner.split_once(',')?;
        return Some((key.trim().to_string(), value.trim().to_string()));
    }
    None
}

fn openapi_field_source(
    field: &crate::parser::Field,
    route: &crate::parser::RestRoute,
) -> crate::parser::FieldSource {
    match field.source {
        crate::parser::FieldSource::Auto => {
            let name = normalize_ident(&field_wire_name(field));
            if route_path_params(&route.path).contains(&name) {
                crate::parser::FieldSource::Path
            } else if matches!(
                route.method,
                crate::parser::HttpMethod::Get
                    | crate::parser::HttpMethod::Head
                    | crate::parser::HttpMethod::Delete
            ) {
                crate::parser::FieldSource::Query
            } else {
                crate::parser::FieldSource::Json
            }
        }
        other => other,
    }
}

fn openapi_path(path: &str) -> String {
    let mut out = String::new();
    let mut chars = path.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == ':' {
            let mut name = String::new();
            while let Some(&next) = chars.peek() {
                if next == '/' {
                    break;
                }
                name.push(next);
                chars.next();
            }
            if name.is_empty() {
                out.push(':');
            } else {
                out.push('{');
                out.push_str(&name);
                out.push('}');
            }
        } else {
            out.push(ch);
        }
    }
    out
}

fn route_path_params(path: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut chars = path.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            ':' => {
                let mut name = String::new();
                while let Some(&next) = chars.peek() {
                    if next == '/' {
                        break;
                    }
                    name.push(next);
                    chars.next();
                }
                if !name.is_empty() {
                    names.push(normalize_ident(&name));
                }
            }
            '{' => {
                let mut name = String::new();
                for next in chars.by_ref() {
                    if next == '}' {
                        break;
                    }
                    name.push(next);
                }
                if !name.is_empty() {
                    names.push(normalize_ident(&name));
                }
            }
            _ => {}
        }
    }
    names
}

fn field_wire_name(field: &crate::parser::Field) -> String {
    field
        .wire_name
        .as_deref()
        .or(field.json_name.as_deref())
        .unwrap_or(&field.name)
        .to_string()
}

fn normalize_ident(input: &str) -> String {
    input.replace('-', "_")
}

fn openapi_method_name(method: &crate::parser::HttpMethod) -> &'static str {
    match method {
        crate::parser::HttpMethod::Get => "get",
        crate::parser::HttpMethod::Head => "head",
        crate::parser::HttpMethod::Post => "post",
        crate::parser::HttpMethod::Put => "put",
        crate::parser::HttpMethod::Patch => "patch",
        crate::parser::HttpMethod::Delete => "delete",
    }
}

pub fn write_ts_client(api: &Path, out: &Path) -> anyhow::Result<()> {
    let source = read_api_source(api)?;
    let spec = crate::parser::parse_api(&source)?;
    validate_project_kind(&spec, ProjectKind::Rest)?;
    if let Some(parent) = out.parent().filter(|parent| !parent.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    fs::write(out, client::render_ts_client(&spec))
        .with_context(|| format!("failed to write {}", out.display()))
}

pub fn write_js_client(api: &Path, out: &Path) -> anyhow::Result<()> {
    let source = read_api_source(api)?;
    let spec = crate::parser::parse_api(&source)?;
    validate_project_kind(&spec, ProjectKind::Rest)?;
    if let Some(parent) = out.parent().filter(|parent| !parent.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    fs::write(out, client::render_js_client(&spec))
        .with_context(|| format!("failed to write {}", out.display()))
}

pub fn write_dart_client(api: &Path, out: &Path) -> anyhow::Result<()> {
    let source = read_api_source(api)?;
    let spec = crate::parser::parse_api(&source)?;
    validate_project_kind(&spec, ProjectKind::Rest)?;
    if let Some(parent) = out.parent().filter(|parent| !parent.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    fs::write(out, client::render_dart_client(&spec))
        .with_context(|| format!("failed to write {}", out.display()))
}

pub fn write_java_client(api: &Path, out: &Path) -> anyhow::Result<()> {
    let source = read_api_source(api)?;
    let spec = crate::parser::parse_api(&source)?;
    validate_project_kind(&spec, ProjectKind::Rest)?;
    if let Some(parent) = out.parent().filter(|parent| !parent.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    fs::write(out, client::render_java_client(&spec))
        .with_context(|| format!("failed to write {}", out.display()))
}

pub fn write_kotlin_client(api: &Path, out: &Path) -> anyhow::Result<()> {
    let source = read_api_source(api)?;
    let spec = crate::parser::parse_api(&source)?;
    validate_project_kind(&spec, ProjectKind::Rest)?;
    if let Some(parent) = out.parent().filter(|parent| !parent.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    fs::write(out, client::render_kotlin_client(&spec))
        .with_context(|| format!("failed to write {}", out.display()))
}

pub fn write_swift_client(api: &Path, out: &Path) -> anyhow::Result<()> {
    let source = read_api_source(api)?;
    let spec = crate::parser::parse_api(&source)?;
    validate_project_kind(&spec, ProjectKind::Rest)?;
    if let Some(parent) = out.parent().filter(|parent| !parent.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    fs::write(out, client::render_swift_client(&spec))
        .with_context(|| format!("failed to write {}", out.display()))
}

fn api_generate_handler(command: GeneratorCommand) -> anyhow::Result<()> {
    match command {
        GeneratorCommand::ApiGenerate { api, out, options } => {
            let rpc_clients = read_api_rpc_client_bindings(&api)?;
            let source = read_api_source(&api)?;
            let spec = crate::parser::parse_api(&source)?;
            validate_project_kind(&spec, ProjectKind::Rest)?;
            if matches!(options.mode, GenerateMode::Force) {
                cleanup_rest_project(&out)?;
            }
            generate_rest_project_with_rpc_clients(&spec, &out, options, &rpc_clients)
                .with_context(|| format!("failed to generate api project at {}", out.display()))
        }
        other => bail!("unexpected command variant for api.generate: {other:?}"),
    }
}

fn api_new_handler(command: GeneratorCommand) -> anyhow::Result<()> {
    match command {
        GeneratorCommand::ApiNew { name, out, options } => create_api_project(&name, &out, options)
            .with_context(|| format!("failed to create api project at {}", out.display())),
        other => bail!("unexpected command variant for api.new: {other:?}"),
    }
}

fn rpc_generate_handler(command: GeneratorCommand) -> anyhow::Result<()> {
    match command {
        GeneratorCommand::RpcGenerate { api, out, options } => {
            let source = read_api_source(&api)?;
            let spec = crate::parser::parse_api(&source)?;
            validate_project_kind(&spec, ProjectKind::Rpc)?;
            if matches!(options.mode, GenerateMode::Force) {
                cleanup_rpc_project(&out)?;
            }
            generate_rpc_project(&spec, &out, options)
                .with_context(|| format!("failed to generate rpc project at {}", out.display()))
        }
        other => bail!("unexpected command variant for rpc.generate: {other:?}"),
    }
}

fn rpc_new_handler(command: GeneratorCommand) -> anyhow::Result<()> {
    match command {
        GeneratorCommand::RpcNew { name, out, options } => create_rpc_project(&name, &out, options)
            .with_context(|| format!("failed to create rpc project at {}", out.display())),
        other => bail!("unexpected command variant for rpc.new: {other:?}"),
    }
}

fn model_generate_handler(command: GeneratorCommand) -> anyhow::Result<()> {
    match command {
        GeneratorCommand::ModelGenerate {
            schema,
            out,
            options,
            format,
            orm,
        } => {
            let source = fs::read_to_string(&schema)
                .with_context(|| format!("failed to read {}", schema.display()))?;
            model::generate_model_project(&source, &out, options, format, orm)
                .with_context(|| format!("failed to generate model scaffold at {}", out.display()))
        }
        other => bail!("unexpected command variant for model.generate: {other:?}"),
    }
}

fn model_inspect_handler(command: GeneratorCommand) -> anyhow::Result<()> {
    match command {
        GeneratorCommand::ModelInspect {
            table,
            schema,
            db_url,
            db_kind,
            sample_size,
            out,
            options,
            orm,
        } => {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .context("failed to create async runtime for model inspection")?;
            rt.block_on(async move {
                model::inspect_model_project(
                    &table,
                    schema.as_deref(),
                    &db_url,
                    db_kind,
                    sample_size,
                    &out,
                    options,
                    orm,
                )
                .await
                .with_context(|| format!("failed to inspect model for table `{table}`"))
            })
        }
        other => bail!("unexpected command variant for model.inspect: {other:?}"),
    }
}

fn validate_project_kind(spec: &ApiSpec, kind: ProjectKind) -> anyhow::Result<()> {
    match kind {
        ProjectKind::Rest => {
            if spec.rest_routes.is_empty() {
                bail!("api projects require at least one REST route");
            }
            if !spec.rpc_methods.is_empty() {
                bail!("api projects cannot contain `rpc` methods; use `rozectl rpc generate`");
            }
        }
        ProjectKind::Rpc => {
            if spec.rpc_methods.is_empty() {
                bail!("rpc projects require at least one `rpc` method");
            }
            if !spec.rest_routes.is_empty() {
                bail!("rpc projects cannot contain REST routes; use `rozectl api generate`");
            }
        }
    }
    Ok(())
}

pub fn create_api_project(
    service: &str,
    out: &Path,
    options: GenerateOptions,
) -> anyhow::Result<()> {
    let source = api_template(service);
    create_api_project_from_source(service, out, options, source)
}

pub fn create_api_project_from_source(
    service: &str,
    out: &Path,
    options: GenerateOptions,
    source: String,
) -> anyhow::Result<()> {
    let api_path = out.join(format!("{}.api", service));
    let spec = crate::parser::parse_api(&source)?;
    fs::create_dir_all(out)?;
    if matches!(options.mode, GenerateMode::Force) {
        cleanup_rest_project(out)?;
    }
    generate_rest_project(&spec, out, options)?;
    fs::write(&api_path, &source)
        .with_context(|| format!("failed to write {}", api_path.display()))?;
    register_workspace_member(out)?;
    Ok(())
}

pub fn create_rpc_project(
    service: &str,
    out: &Path,
    options: GenerateOptions,
) -> anyhow::Result<()> {
    let source = rpc_template(service);
    create_rpc_project_from_source(service, out, options, source)
}

pub fn create_rpc_project_from_source(
    service: &str,
    out: &Path,
    options: GenerateOptions,
    source: String,
) -> anyhow::Result<()> {
    let api_path = out.join(format!("{}.api", service));
    let spec = crate::parser::parse_api(&source)?;
    fs::create_dir_all(out)?;
    if matches!(options.mode, GenerateMode::Force) {
        cleanup_rpc_project(out)?;
    }
    generate_rpc_project(&spec, out, options)?;
    fs::write(&api_path, &source)
        .with_context(|| format!("failed to write {}", api_path.display()))?;
    register_workspace_member(out)?;
    Ok(())
}

fn cleanup_rest_project(out: &Path) -> anyhow::Result<()> {
    remove_path_if_exists(&out.join("build.rs"))?;
    remove_path_if_exists(&out.join("proto"))?;
    remove_path_if_exists(&out.join("src/client.rs"))?;
    remove_path_if_exists(&out.join("src/pb.rs"))?;
    remove_path_if_exists(&out.join("src/rpc.rs"))?;
    Ok(())
}

pub(super) fn cleanup_rpc_project(out: &Path) -> anyhow::Result<()> {
    remove_path_if_exists(&out.join("src/handler"))?;
    remove_path_if_exists(&out.join("src/logic"))?;
    remove_path_if_exists(&out.join("src/openapi.rs"))?;
    Ok(())
}

fn remove_path_if_exists(path: &Path) -> anyhow::Result<()> {
    if !path.exists() {
        return Ok(());
    }
    if path.is_dir() {
        fs::remove_dir_all(path).with_context(|| format!("failed to remove {}", path.display()))
    } else {
        fs::remove_file(path).with_context(|| format!("failed to remove {}", path.display()))
    }
}

fn migrate_flat_module_file(
    out: &Path,
    old_relative: &str,
    new_relative: &str,
    mode: GenerateMode,
) -> anyhow::Result<()> {
    let old_path = out.join(old_relative);
    let new_path = out.join(new_relative);
    if !old_path.exists() {
        return Ok(());
    }
    if mode == GenerateMode::Update && !new_path.exists() {
        if let Some(parent) = new_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::rename(&old_path, &new_path).with_context(|| {
            format!(
                "failed to migrate {} to {}",
                old_path.display(),
                new_path.display()
            )
        })?;
        return Ok(());
    }
    remove_path_if_exists(&old_path)
}

fn generate_rest_project(
    spec: &ApiSpec,
    out: &Path,
    options: GenerateOptions,
) -> anyhow::Result<()> {
    generate_rest_project_with_rpc_clients(spec, out, options, &[])
}

fn generate_rest_project_with_rpc_clients(
    spec: &ApiSpec,
    out: &Path,
    options: GenerateOptions,
    rpc_clients: &[RpcClientBinding],
) -> anyhow::Result<()> {
    ensure_output(out, options.mode)?;
    remove_path_if_exists(&out.join("src/handler"))?;

    fs::create_dir_all(out.join("src"))?;
    fs::create_dir_all(out.join("src/config"))?;
    fs::create_dir_all(out.join("src/handler"))?;
    fs::create_dir_all(out.join("src/logic"))?;
    fs::create_dir_all(out.join("src/middleware"))?;
    fs::create_dir_all(out.join("src/openapi"))?;
    fs::create_dir_all(out.join("src/route"))?;
    fs::create_dir_all(out.join("src/svc"))?;
    fs::create_dir_all(out.join("src/types"))?;
    fs::create_dir_all(out.join("ops"))?;
    fs::create_dir_all(out.join(".cargo"))?;
    fs::create_dir_all(out.join(".github/workflows"))?;
    write_cargo_toml_with_rpc_clients(spec, out, options, ProjectKind::Rest, rpc_clients)?;
    fs::write(out.join(".cargo/config.toml"), cargo_config())?;
    fs::write(out.join("README.md"), readme(spec, ProjectKind::Rest))?;
    fs::write(
        out.join("ops/production-evidence.md"),
        production_evidence_runbook(spec, ProjectKind::Rest),
    )?;
    fs::write(
        out.join("ops/governance-baseline.yaml"),
        governance_baseline_yaml(spec, ProjectKind::Rest),
    )?;
    fs::write(
        out.join("ops/prometheus-rules.yaml"),
        prometheus_rules_yaml(spec, ProjectKind::Rest),
    )?;
    fs::write(
        out.join("ops/grafana-dashboard.json"),
        grafana_dashboard_json(spec, ProjectKind::Rest),
    )?;
    fs::write(out.join("ops/slo.yaml"), slo_yaml(spec, ProjectKind::Rest))?;
    fs::write(
        out.join("ops/failure-injection-plan.yaml"),
        failure_injection_plan_yaml(spec, ProjectKind::Rest),
    )?;
    fs::write(
        out.join("ops/release-rollout.yaml"),
        release_rollout_yaml(spec, ProjectKind::Rest),
    )?;
    fs::write(
        out.join("ops/incident-response.yaml"),
        incident_response_yaml(spec, ProjectKind::Rest),
    )?;
    fs::write(
        out.join("ops/capacity-plan.yaml"),
        capacity_plan_yaml(spec, ProjectKind::Rest),
    )?;
    fs::write(
        out.join("ops/security-readiness.yaml"),
        security_readiness_yaml(spec, ProjectKind::Rest),
    )?;
    fs::write(
        out.join("ops/production-gate.yaml"),
        production_gate_yaml(spec, ProjectKind::Rest),
    )?;
    fs::write(
        out.join("ops/regeneration-policy.yaml"),
        regeneration_policy_yaml(spec, ProjectKind::Rest),
    )?;
    fs::write(
        out.join("ops/client-contract.yaml"),
        client_contract_yaml(spec, ProjectKind::Rest),
    )?;
    fs::write(
        out.join("ops/config-governance.yaml"),
        config_governance_yaml(spec, ProjectKind::Rest),
    )?;
    fs::write(
        out.join("ops/reliable-events.yaml"),
        reliable_events_yaml(spec, ProjectKind::Rest),
    )?;
    fs::write(
        out.join("ops/dependency-governance.yaml"),
        dependency_governance_yaml(spec, ProjectKind::Rest),
    )?;
    fs::write(
        out.join("ops/data-consistency.yaml"),
        data_consistency_yaml(spec, ProjectKind::Rest),
    )?;
    fs::write(
        out.join("ops/observability-contract.yaml"),
        observability_contract_yaml(spec, ProjectKind::Rest),
    )?;
    fs::write(
        out.join("ops/runtime-hardening.yaml"),
        runtime_hardening_yaml(spec, ProjectKind::Rest),
    )?;
    fs::write(
        out.join("ops/error-contract.yaml"),
        error_contract_yaml(spec, ProjectKind::Rest),
    )?;
    fs::write(
        out.join("ops/deployment-topology.yaml"),
        deployment_topology_yaml(spec, ProjectKind::Rest),
    )?;
    fs::write(
        out.join("ops/service-communication.yaml"),
        service_communication_yaml(spec, ProjectKind::Rest),
    )?;
    fs::write(
        out.join("ops/cache-governance.yaml"),
        cache_governance_yaml(spec, ProjectKind::Rest),
    )?;
    fs::write(
        out.join("ops/data-access-governance.yaml"),
        data_access_governance_yaml(spec, ProjectKind::Rest),
    )?;
    fs::write(
        out.join("ops/interface-governance.yaml"),
        interface_governance_yaml(spec, ProjectKind::Rest),
    )?;
    fs::write(
        out.join("ops/production-verify.ps1"),
        production_verify_ps1(spec, ProjectKind::Rest),
    )?;
    fs::write(
        out.join("ops/production-verify.sh"),
        production_verify_sh(spec, ProjectKind::Rest),
    )?;
    fs::write(
        out.join("ops/ci-evidence-policy.yaml"),
        ci_evidence_policy_yaml(spec, ProjectKind::Rest),
    )?;
    fs::write(
        out.join("ops/evidence-manifest.yaml"),
        evidence_manifest_yaml(spec, ProjectKind::Rest),
    )?;
    fs::write(
        out.join(".github/workflows/roze-production-verify.yml"),
        production_verify_workflow_yml(spec, ProjectKind::Rest),
    )?;
    write_preserved(
        &out.join("config.yaml"),
        config_yaml(spec, ProjectKind::Rest),
        options.mode,
    )?;
    remove_path_if_exists(&out.join("src/config.rs"))?;
    remove_path_if_exists(&out.join("src/openapi.rs"))?;
    remove_path_if_exists(&out.join("src/types.rs"))?;
    write_preserved(&out.join("src/config/mod.rs"), config_rs(), options.mode)?;
    fs::write(out.join("src/openapi/mod.rs"), rest::render_openapi(spec))?;
    fs::write(
        out.join("src/handler/mod.rs"),
        rest::render_handler_mod(spec),
    )?;
    fs::write(out.join("src/route/mod.rs"), rest::render_route_mod(spec))?;
    for (group, content) in rest::render_route_group_mods(spec) {
        fs::write(out.join("src/route").join(format!("{group}.rs")), content)?;
    }
    for (group, content) in rest::render_handler_group_mods(spec) {
        let dir = out.join("src/handler").join(&group);
        fs::create_dir_all(&dir)?;
        fs::write(dir.join("mod.rs"), content)?;
    }
    for (group, handler, content) in rest::render_handler_files(spec) {
        let dir = out.join("src/handler").join(&group);
        fs::create_dir_all(&dir)?;
        fs::write(dir.join(format!("{handler}.rs")), content)?;
    }
    migrate_flat_module_file(
        out,
        "src/middleware.rs",
        "src/middleware/mod.rs",
        options.mode,
    )?;
    fs::write(
        out.join("src/middleware/mod.rs"),
        rest::render_middleware_mod(spec),
    )?;
    for (name, content) in rest::render_middleware_files(spec) {
        write_preserved(
            &out.join("src/middleware").join(format!("{name}.rs")),
            content,
            options.mode,
        )?;
    }
    fs::write(out.join("src/logic/mod.rs"), rest::render_logic_mod(spec))?;
    for (group, content) in rest::render_logic_group_mods(spec) {
        let dir = out.join("src/logic").join(&group);
        fs::create_dir_all(&dir)?;
        write_logic_group_mod(&dir.join("mod.rs"), content, options.mode)?;
    }
    for (group, handler, content) in rest::render_logic_files(spec) {
        let dir = out.join("src/logic").join(&group);
        fs::create_dir_all(&dir)?;
        write_preserved_logic(&dir.join(format!("{handler}.rs")), content, options.mode)?;
    }
    fs::write(
        out.join("src/types/mod.rs"),
        types::render_types(&spec.types),
    )?;
    write_preserved(
        &out.join("src/svc/mod.rs"),
        rest_service_context_rs(rpc_clients),
        options.mode,
    )?;
    fs::write(out.join("src/main.rs"), rest::render_rest_main(spec))?;
    ensure_model_module(out)?;
    Ok(())
}

pub(super) fn generate_rpc_project(
    spec: &ApiSpec,
    out: &Path,
    options: GenerateOptions,
) -> anyhow::Result<()> {
    ensure_output(out, options.mode)?;

    fs::create_dir_all(out.join("src"))?;
    fs::create_dir_all(out.join("src/client"))?;
    fs::create_dir_all(out.join("src/config"))?;
    fs::create_dir_all(out.join("src/logic"))?;
    fs::create_dir_all(out.join("src/pb"))?;
    fs::create_dir_all(out.join("src/server"))?;
    fs::create_dir_all(out.join("src/svc"))?;
    fs::create_dir_all(out.join("src/types"))?;
    fs::create_dir_all(out.join("ops"))?;
    fs::create_dir_all(out.join("proto"))?;
    fs::create_dir_all(out.join(".cargo"))?;
    fs::create_dir_all(out.join(".github/workflows"))?;
    remove_path_if_exists(&out.join("src/client.rs"))?;
    remove_path_if_exists(&out.join("src/config.rs"))?;
    remove_path_if_exists(&out.join("src/pb.rs"))?;
    remove_path_if_exists(&out.join("src/rpc.rs"))?;
    remove_path_if_exists(&out.join("src/types.rs"))?;
    write_cargo_toml(spec, out, options, ProjectKind::Rpc)?;
    fs::write(out.join(".cargo/config.toml"), cargo_config())?;
    fs::write(out.join("README.md"), readme(spec, ProjectKind::Rpc))?;
    fs::write(
        out.join("ops/production-evidence.md"),
        production_evidence_runbook(spec, ProjectKind::Rpc),
    )?;
    fs::write(
        out.join("ops/governance-baseline.yaml"),
        governance_baseline_yaml(spec, ProjectKind::Rpc),
    )?;
    fs::write(
        out.join("ops/prometheus-rules.yaml"),
        prometheus_rules_yaml(spec, ProjectKind::Rpc),
    )?;
    fs::write(
        out.join("ops/grafana-dashboard.json"),
        grafana_dashboard_json(spec, ProjectKind::Rpc),
    )?;
    fs::write(out.join("ops/slo.yaml"), slo_yaml(spec, ProjectKind::Rpc))?;
    fs::write(
        out.join("ops/failure-injection-plan.yaml"),
        failure_injection_plan_yaml(spec, ProjectKind::Rpc),
    )?;
    fs::write(
        out.join("ops/release-rollout.yaml"),
        release_rollout_yaml(spec, ProjectKind::Rpc),
    )?;
    fs::write(
        out.join("ops/incident-response.yaml"),
        incident_response_yaml(spec, ProjectKind::Rpc),
    )?;
    fs::write(
        out.join("ops/capacity-plan.yaml"),
        capacity_plan_yaml(spec, ProjectKind::Rpc),
    )?;
    fs::write(
        out.join("ops/security-readiness.yaml"),
        security_readiness_yaml(spec, ProjectKind::Rpc),
    )?;
    fs::write(
        out.join("ops/production-gate.yaml"),
        production_gate_yaml(spec, ProjectKind::Rpc),
    )?;
    fs::write(
        out.join("ops/regeneration-policy.yaml"),
        regeneration_policy_yaml(spec, ProjectKind::Rpc),
    )?;
    fs::write(
        out.join("ops/client-contract.yaml"),
        client_contract_yaml(spec, ProjectKind::Rpc),
    )?;
    fs::write(
        out.join("ops/config-governance.yaml"),
        config_governance_yaml(spec, ProjectKind::Rpc),
    )?;
    fs::write(
        out.join("ops/reliable-events.yaml"),
        reliable_events_yaml(spec, ProjectKind::Rpc),
    )?;
    fs::write(
        out.join("ops/dependency-governance.yaml"),
        dependency_governance_yaml(spec, ProjectKind::Rpc),
    )?;
    fs::write(
        out.join("ops/data-consistency.yaml"),
        data_consistency_yaml(spec, ProjectKind::Rpc),
    )?;
    fs::write(
        out.join("ops/observability-contract.yaml"),
        observability_contract_yaml(spec, ProjectKind::Rpc),
    )?;
    fs::write(
        out.join("ops/runtime-hardening.yaml"),
        runtime_hardening_yaml(spec, ProjectKind::Rpc),
    )?;
    fs::write(
        out.join("ops/error-contract.yaml"),
        error_contract_yaml(spec, ProjectKind::Rpc),
    )?;
    fs::write(
        out.join("ops/deployment-topology.yaml"),
        deployment_topology_yaml(spec, ProjectKind::Rpc),
    )?;
    fs::write(
        out.join("ops/service-communication.yaml"),
        service_communication_yaml(spec, ProjectKind::Rpc),
    )?;
    fs::write(
        out.join("ops/cache-governance.yaml"),
        cache_governance_yaml(spec, ProjectKind::Rpc),
    )?;
    fs::write(
        out.join("ops/data-access-governance.yaml"),
        data_access_governance_yaml(spec, ProjectKind::Rpc),
    )?;
    fs::write(
        out.join("ops/interface-governance.yaml"),
        interface_governance_yaml(spec, ProjectKind::Rpc),
    )?;
    fs::write(
        out.join("ops/production-verify.ps1"),
        production_verify_ps1(spec, ProjectKind::Rpc),
    )?;
    fs::write(
        out.join("ops/production-verify.sh"),
        production_verify_sh(spec, ProjectKind::Rpc),
    )?;
    fs::write(
        out.join("ops/ci-evidence-policy.yaml"),
        ci_evidence_policy_yaml(spec, ProjectKind::Rpc),
    )?;
    fs::write(
        out.join("ops/evidence-manifest.yaml"),
        evidence_manifest_yaml(spec, ProjectKind::Rpc),
    )?;
    fs::write(
        out.join(".github/workflows/roze-production-verify.yml"),
        production_verify_workflow_yml(spec, ProjectKind::Rpc),
    )?;
    fs::write(out.join("build.rs"), build_rs())?;
    write_preserved(
        &out.join("config.yaml"),
        config_yaml(spec, ProjectKind::Rpc),
        options.mode,
    )?;
    write_preserved(&out.join("src/config/mod.rs"), config_rs(), options.mode)?;
    fs::write(out.join("src/pb/mod.rs"), render_pb(spec))?;
    fs::write(
        out.join("src/types/mod.rs"),
        types::render_types(&spec.types),
    )?;
    write_preserved(
        &out.join("src/svc/mod.rs"),
        service_context_rs(ProjectKind::Rpc),
        options.mode,
    )?;
    fs::write(out.join("src/server/mod.rs"), rpc::render_rpc(spec))?;
    fs::write(out.join("src/client/mod.rs"), rpc::render_client(spec))?;
    write_logic_group_mod(
        &out.join("src/logic/mod.rs"),
        rpc::render_logic_mod(spec),
        options.mode,
    )?;
    for (method, content) in rpc::render_logic_files(spec) {
        write_preserved_logic(
            &out.join("src/logic").join(format!("{method}.rs")),
            content,
            options.mode,
        )?;
    }
    fs::write(out.join("src/lib.rs"), rpc::render_lib())?;
    fs::write(out.join("src/main.rs"), rpc::render_main(spec))?;
    fs::write(out.join("proto/service.proto"), render_proto(spec)?)?;
    Ok(())
}

fn ensure_output(out: &Path, mode: GenerateMode) -> anyhow::Result<()> {
    if out.exists() && mode == GenerateMode::Create && has_entries(out)? {
        bail!(
            "{} already exists and is not empty; pass --update to preserve business files or --force to overwrite all generated files",
            out.display()
        );
    }
    fs::create_dir_all(out).with_context(|| format!("failed to create {}", out.display()))
}

fn write_preserved(path: &Path, content: String, mode: GenerateMode) -> anyhow::Result<()> {
    // Business-owned files, such as generated logic and middleware stubs, must not be
    // overwritten during --update because users are expected to edit them.
    if mode == GenerateMode::Update && path.exists() {
        return Ok(());
    }
    fs::write(path, content).with_context(|| format!("failed to write {}", path.display()))
}

fn write_preserved_logic(path: &Path, content: String, mode: GenerateMode) -> anyhow::Result<()> {
    if mode == GenerateMode::Update && path.exists() {
        let existing = fs::read_to_string(path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        if !is_generated_default_logic_stub(&existing) {
            return Ok(());
        }
    }
    fs::write(path, content).with_context(|| format!("failed to write {}", path.display()))
}

fn write_logic_group_mod(path: &Path, content: String, mode: GenerateMode) -> anyhow::Result<()> {
    if mode != GenerateMode::Update || !path.exists() {
        return fs::write(path, content)
            .with_context(|| format!("failed to write {}", path.display()));
    }

    let existing =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    let merged = merge_app_owned_mod_declarations(&content, &existing);
    fs::write(path, merged).with_context(|| format!("failed to write {}", path.display()))
}

fn merge_app_owned_mod_declarations(generated: &str, existing: &str) -> String {
    let generated_modules = generated
        .lines()
        .filter_map(mod_declaration_name)
        .collect::<HashSet<_>>();
    let generated_pub_uses = generated
        .lines()
        .filter_map(pub_use_declaration)
        .collect::<Vec<_>>();
    let existing_pub_uses = existing
        .lines()
        .filter_map(|line| {
            pub_use_declaration(line).map(|pub_use| (line.trim().to_string(), pub_use))
        })
        .collect::<Vec<_>>();

    let mut replaced_pub_uses = HashSet::new();
    let mut merged = String::new();
    for line in generated.lines() {
        if let Some(generated_pub_use) = pub_use_declaration(line) {
            if let Some((existing_line, _)) =
                existing_pub_uses.iter().find(|(_, existing_pub_use)| {
                    existing_pub_use.module == generated_pub_use.module
                        && generated_pub_use
                            .symbols
                            .iter()
                            .all(|symbol| existing_pub_use.symbols.contains(symbol))
                })
            {
                merged.push_str(existing_line);
                merged.push('\n');
                replaced_pub_uses.insert(existing_line.clone());
                continue;
            }
        }

        merged.push_str(line);
        merged.push('\n');
    }

    let extra_mods = existing
        .lines()
        .filter_map(|line| {
            let name = mod_declaration_name(line)?;
            (!generated_modules.contains(&name)).then_some(line.trim().to_string())
        })
        .collect::<Vec<_>>();
    let extra_pub_uses = existing_pub_uses
        .into_iter()
        .filter_map(|(line, pub_use)| {
            let generated_has_same_module = generated_pub_uses
                .iter()
                .any(|generated_pub_use| generated_pub_use.module == pub_use.module);
            (!generated_has_same_module && !replaced_pub_uses.contains(&line)).then_some(line)
        })
        .collect::<Vec<_>>();
    if extra_mods.is_empty() && extra_pub_uses.is_empty() {
        return merged;
    }

    if !merged.ends_with('\n') {
        merged.push('\n');
    }
    for line in extra_mods {
        merged.push_str(&line);
        merged.push('\n');
    }
    for line in extra_pub_uses {
        merged.push_str(&line);
        merged.push('\n');
    }
    merged
}

fn mod_declaration_name(line: &str) -> Option<String> {
    let line = line.trim();
    let rest = line
        .strip_prefix("mod ")
        .or_else(|| line.strip_prefix("pub mod "))?;
    let name = rest.strip_suffix(';')?.trim();
    (!name.is_empty()
        && name
            .chars()
            .all(|ch| ch == '_' || ch.is_ascii_alphanumeric()))
    .then(|| name.to_string())
}

#[derive(Debug, Eq, PartialEq)]
struct PubUseDeclaration {
    module: String,
    symbols: HashSet<String>,
}

fn pub_use_declaration(line: &str) -> Option<PubUseDeclaration> {
    let line = line.trim();
    let rest = line.strip_prefix("pub use ")?;
    let rest = rest.strip_suffix(';')?.trim();
    let (module, symbols) = rest.split_once("::")?;
    let module = module.trim();
    if !is_rust_ident(module) {
        return None;
    }

    let symbols = if let Some(symbols) = symbols
        .trim()
        .strip_prefix('{')
        .and_then(|symbols| symbols.strip_suffix('}'))
    {
        symbols
            .split(',')
            .map(str::trim)
            .filter(|symbol| !symbol.is_empty())
            .map(str::to_string)
            .collect::<HashSet<_>>()
    } else {
        let symbol = symbols.trim();
        if !is_rust_ident(symbol) {
            return None;
        }
        HashSet::from([symbol.to_string()])
    };

    (!symbols.is_empty()).then(|| PubUseDeclaration {
        module: module.to_string(),
        symbols,
    })
}

fn is_rust_ident(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

fn is_generated_default_logic_stub(content: &str) -> bool {
    let lines = content
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();

    if lines.is_empty()
        || !lines[0].starts_with("use super")
        || lines
            .iter()
            .filter(|line| line.starts_with("pub async fn "))
            .count()
            != 1
        || !lines.contains(&"let _ = ctx;")
        || !lines.contains(&"let _ = request_ctx;")
        || !lines.contains(&"let _ = req;")
    {
        return false;
    }

    lines.iter().all(|line| {
        line.starts_with("use super")
            || line.starts_with("pub async fn ")
            || *line == "{"
            || *line == "}"
            || *line == "let _ = ctx;"
            || *line == "let _ = request_ctx;"
            || *line == "let _ = req;"
            || line.starts_with("Ok(")
            || line.starts_with("})")
            || is_default_stub_field_line(line)
    })
}

fn is_default_stub_field_line(line: &str) -> bool {
    let Some((field, value)) = line.trim_end_matches(',').split_once(':') else {
        return false;
    };
    let field = field.trim();
    let value = value.trim();
    !field.is_empty()
        && field
            .chars()
            .all(|ch| ch == '_' || ch == '#' || ch.is_ascii_alphanumeric())
        && matches!(
            value,
            "Default::default()"
                | "String::new()"
                | "Vec::new()"
                | "std::collections::HashMap::new()"
                | "false"
                | "true"
                | "0"
                | "0.0"
        )
}

fn ensure_model_module(out: &Path) -> anyhow::Result<()> {
    let model_dir = out.join("src/model");
    if !model_dir.exists() {
        return Ok(());
    }

    let main_path = out.join("src/main.rs");
    if !main_path.is_file() {
        return Ok(());
    }

    let content = fs::read_to_string(&main_path)
        .with_context(|| format!("failed to read {}", main_path.display()))?;
    if content.contains("mod model;") {
        return Ok(());
    }

    let updated = if let Some(idx) = content.find("mod types;\n") {
        let mut updated = String::with_capacity(content.len() + "mod model;\n".len());
        updated.push_str(&content[..idx + "mod types;\n".len()]);
        updated.push_str("mod model;\n");
        updated.push_str(&content[idx + "mod types;\n".len()..]);
        updated
    } else {
        format!("mod model;\n{content}")
    };

    fs::write(&main_path, updated)
        .with_context(|| format!("failed to write {}", main_path.display()))
}

fn write_cargo_toml(
    spec: &ApiSpec,
    out: &Path,
    options: GenerateOptions,
    kind: ProjectKind,
) -> anyhow::Result<()> {
    write_cargo_toml_with_rpc_clients(spec, out, options, kind, &[])
}

fn write_cargo_toml_with_rpc_clients(
    spec: &ApiSpec,
    out: &Path,
    options: GenerateOptions,
    kind: ProjectKind,
    rpc_clients: &[RpcClientBinding],
) -> anyhow::Result<()> {
    let path = out.join("Cargo.toml");
    let package_name = package_name_from_output(out, spec);
    let workspace_root = find_workspace_root(out)?;
    let local_crates_prefix = match options.dependency_source {
        DependencySource::Git => None,
        DependencySource::Path => Some(local_crates_prefix(
            out,
            workspace_root.as_deref().ok_or_else(|| {
                anyhow::anyhow!(
                    "--roze-source path requires a Cargo workspace containing the Roze crates"
                )
            })?,
        )?),
    };
    if options.mode != GenerateMode::Update || !path.exists() {
        return fs::write(
            &path,
            cargo_toml(
                &package_name,
                options.dependency_source,
                local_crates_prefix.as_deref(),
                workspace_root.is_some(),
                kind,
                out,
                rpc_clients,
            ),
        )
        .with_context(|| format!("failed to write {}", path.display()));
    }

    let content =
        fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?;
    let mut document = content
        .parse::<toml_edit::DocumentMut>()
        .with_context(|| format!("failed to parse {}", path.display()))?;
    let dependencies = document
        .get_mut("dependencies")
        .and_then(toml_edit::Item::as_table_mut)
        .ok_or_else(|| anyhow::anyhow!("{} has no [dependencies] table", path.display()))?;

    for name in project_roze_crates(kind) {
        dependencies.insert(
            name,
            dependency_item(
                name,
                options.dependency_source,
                local_crates_prefix.as_deref(),
            ),
        );
    }
    for client in rpc_clients {
        dependencies.insert(
            &client.dep_name,
            toml_edit::value(toml_edit::InlineTable::from_iter([(
                "path",
                toml_edit::Value::from(rpc_client_dependency_path(out, client)),
            )])),
        );
    }

    fs::write(&path, document.to_string())
        .with_context(|| format!("failed to write {}", path.display()))
}

fn has_entries(path: &Path) -> anyhow::Result<bool> {
    Ok(fs::read_dir(path)?.next().is_some())
}

pub(super) fn read_api_source(path: &Path) -> anyhow::Result<String> {
    let mut seen = HashSet::new();
    read_api_source_inner(path, &mut seen, true)
}

fn read_api_source_inner(
    path: &Path,
    seen: &mut HashSet<PathBuf>,
    include_pure_rpc: bool,
) -> anyhow::Result<String> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let absolute = absolute
        .canonicalize()
        .with_context(|| format!("failed to resolve {}", path.display()))?;
    if !seen.insert(absolute.clone()) {
        return Ok(String::new());
    }
    let source = fs::read_to_string(&absolute)
        .with_context(|| format!("failed to read {}", absolute.display()))?;
    if !include_pure_rpc && source_is_pure_rpc_contract(&source) {
        return Ok(String::new());
    }
    let base = absolute
        .parent()
        .ok_or_else(|| anyhow::anyhow!("{} has no parent directory", absolute.display()))?;
    let mut out = String::new();
    let mut lines = source.lines();
    while let Some(raw) = lines.next() {
        let line = raw.trim();
        if let Some(import) = parse_import_line(line) {
            let import_path = base.join(import);
            out.push_str(&read_api_source_inner(&import_path, seen, false)?);
            out.push('\n');
        } else if is_import_block_start(line) {
            for import_raw in lines.by_ref() {
                let import_line = strip_inline_comment(import_raw).trim();
                if import_line == ")" {
                    break;
                }
                if import_line.is_empty() {
                    continue;
                }
                if let Some(import) = parse_import_path(import_line) {
                    let import_path = base.join(import);
                    out.push_str(&read_api_source_inner(&import_path, seen, false)?);
                    out.push('\n');
                } else {
                    anyhow::bail!(
                        "invalid import entry `{}` in {}",
                        import_line,
                        absolute.display()
                    );
                }
            }
        } else {
            out.push_str(raw);
            out.push('\n');
        }
    }
    Ok(out)
}

fn read_api_rpc_client_bindings(path: &Path) -> anyhow::Result<Vec<RpcClientBinding>> {
    let mut seen = HashSet::new();
    let mut bindings = BTreeSet::new();
    collect_api_rpc_client_bindings(path, true, &mut seen, &mut bindings)?;
    Ok(bindings.into_iter().collect())
}

fn collect_api_rpc_client_bindings(
    path: &Path,
    root: bool,
    seen: &mut HashSet<PathBuf>,
    bindings: &mut BTreeSet<RpcClientBinding>,
) -> anyhow::Result<()> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let absolute = absolute
        .canonicalize()
        .with_context(|| format!("failed to resolve {}", path.display()))?;
    if !seen.insert(absolute.clone()) {
        return Ok(());
    }
    let source = fs::read_to_string(&absolute)
        .with_context(|| format!("failed to read {}", absolute.display()))?;
    if !root && source_is_pure_rpc_contract(&source) {
        bindings.insert(rpc_client_binding_for_import(path, &absolute)?);
        return Ok(());
    }

    let base = absolute
        .parent()
        .ok_or_else(|| anyhow::anyhow!("{} has no parent directory", absolute.display()))?;
    let mut lines = source.lines();
    while let Some(raw) = lines.next() {
        let line = raw.trim();
        if let Some(import) = parse_import_line(line) {
            collect_api_rpc_client_bindings(&base.join(import), false, seen, bindings)?;
        } else if is_import_block_start(line) {
            for import_raw in lines.by_ref() {
                let import_line = strip_inline_comment(import_raw).trim();
                if import_line == ")" {
                    break;
                }
                if import_line.is_empty() {
                    continue;
                }
                if let Some(import) = parse_import_path(import_line) {
                    collect_api_rpc_client_bindings(&base.join(import), false, seen, bindings)?;
                } else {
                    anyhow::bail!(
                        "invalid import entry `{}` in {}",
                        import_line,
                        absolute.display()
                    );
                }
            }
        }
    }
    Ok(())
}

fn source_is_pure_rpc_contract(source: &str) -> bool {
    crate::parser::parse_api(source)
        .map(|spec| !spec.rpc_methods.is_empty() && spec.rest_routes.is_empty())
        .unwrap_or(false)
}

fn rpc_client_binding_for_import(
    import_path: &Path,
    absolute: &Path,
) -> anyhow::Result<RpcClientBinding> {
    let package = absolute
        .parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .or_else(|| absolute.file_stem().and_then(|name| name.to_str()))
        .ok_or_else(|| {
            anyhow::anyhow!(
                "failed to infer rpc client crate for {}",
                absolute.display()
            )
        })?;
    let dep_name = normalize_crate_name(package);
    let service_name = dep_name
        .strip_suffix("-rpc")
        .unwrap_or(&dep_name)
        .rsplit('-')
        .next()
        .unwrap_or(dep_name.as_str())
        .to_string();
    let path = import_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_string_lossy()
        .replace('\\', "/");
    Ok(RpcClientBinding {
        name: service_name,
        dep_name,
        crate_name: normalize_crate_name(package).replace('-', "_"),
        path,
    })
}

fn parse_import_line(line: &str) -> Option<&str> {
    let rest = line.strip_prefix("import ")?;
    parse_import_path(rest)
}

fn is_import_block_start(line: &str) -> bool {
    let Some(rest) = line.strip_prefix("import") else {
        return false;
    };
    rest.trim_start() == "("
}

fn parse_import_path(line: &str) -> Option<&str> {
    let rest = line.trim();
    rest.strip_prefix('"')?.strip_suffix('"')
}

fn strip_inline_comment(line: &str) -> &str {
    line.split_once("//").map_or(line, |(left, _)| left)
}

fn cargo_config() -> &'static str {
    r#"[net]
git-fetch-with-cli = true
"#
}

fn cargo_toml(
    package_name: &str,
    dependency_source: DependencySource,
    local_crates_prefix: Option<&str>,
    in_workspace: bool,
    kind: ProjectKind,
    out: &Path,
    rpc_clients: &[RpcClientBinding],
) -> String {
    let roze_dependencies = roze_dependencies(
        dependency_source,
        local_crates_prefix,
        project_roze_crates(kind),
    );
    let package = if in_workspace {
        r#"edition = "2021"
license.workspace = true
version.workspace = true"#
    } else {
        r#"edition = "2021"
license = "MIT"
version = "0.1.0""#
    };
    let (dependencies, remaining_dependencies, build_dependencies) = match kind {
        ProjectKind::Rest => (
            if in_workspace {
                r#"anyhow.workspace = true
config.workspace = true"#
            } else {
                r#"anyhow = "1"
config = { version = "0.15.24", default-features = false, features = ["json", "yaml", "toml"] }"#
            },
            if in_workspace {
                r#"serde.workspace = true
serde_json.workspace = true
validator.workspace = true
tokio.workspace = true
tracing.workspace = true"#
            } else {
                r#"serde = { version = "1", features = ["derive"] }
serde_json = "1"
validator = { version = "0.20", features = ["derive"] }
tokio = { version = "1", features = ["macros", "rt-multi-thread", "signal", "sync", "time"] }
tracing = "0.1""#
            },
            "",
        ),
        ProjectKind::Rpc => (
            if in_workspace {
                r#"anyhow.workspace = true
config.workspace = true
prost.workspace = true"#
            } else {
                r#"anyhow = "1"
config = { version = "0.15.24", default-features = false, features = ["json", "yaml", "toml"] }
prost = "0.14""#
            },
            if in_workspace {
                r#"serde.workspace = true
serde_json.workspace = true
async-trait.workspace = true
tokio.workspace = true
tonic.workspace = true
tonic-prost.workspace = true
validator.workspace = true
tracing.workspace = true"#
            } else {
                r#"serde = { version = "1", features = ["derive"] }
serde_json = "1"
async-trait = "0.1"
tokio = { version = "1", features = ["macros", "rt-multi-thread", "signal", "sync", "time"] }
tonic = "0.14.6"
tonic-prost = "0.14.6"
validator = { version = "0.20", features = ["derive"] }
tracing = "0.1""#
            },
            if in_workspace {
                r#"protoc-bin-vendored.workspace = true
roze-grpc.workspace = true
tonic-prost-build.workspace = true"#
            } else {
                r#"protoc-bin-vendored = "3"
roze-grpc = { git = "https://github.com/roze-team/roze.git" }
tonic-prost-build = "0.14.6""#
            },
        ),
    };
    let build_dependencies_section = if build_dependencies.trim().is_empty() {
        String::new()
    } else {
        format!("\n[build-dependencies]\n{build_dependencies}\n")
    };
    let rpc_client_dependencies = render_rpc_client_dependencies(out, rpc_clients);

    format!(
        r#"[package]
name = "{package_name}"
{package}

[dependencies]
{dependencies}
{roze_dependencies}
{rpc_client_dependencies}
{remaining_dependencies}
{build_dependencies_section}"#,
        package_name = package_name,
        package = package,
        dependencies = dependencies,
        roze_dependencies = roze_dependencies,
        rpc_client_dependencies = rpc_client_dependencies,
        remaining_dependencies = remaining_dependencies,
        build_dependencies_section = build_dependencies_section,
    )
}

fn render_rpc_client_dependencies(out: &Path, rpc_clients: &[RpcClientBinding]) -> String {
    rpc_clients
        .iter()
        .map(|client| {
            format!(
                r#"{} = {{ path = "{}" }}"#,
                client.dep_name,
                rpc_client_dependency_path(out, client)
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn rpc_client_dependency_path(out: &Path, client: &RpcClientBinding) -> String {
    let path = Path::new(&client.path);
    if path.is_absolute() {
        if let Some(parent) = out.parent() {
            if let (Ok(parent), Ok(client_path)) = (parent.canonicalize(), path.canonicalize()) {
                if let Ok(stripped) = client_path.strip_prefix(&parent) {
                    return format!("../{}", path_to_forward_slashes(stripped));
                }
            }
        }
    }
    path_to_forward_slashes(path)
}

fn path_to_forward_slashes(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn package_name_from_output(out: &Path, spec: &ApiSpec) -> String {
    out.file_name()
        .and_then(|name| name.to_str())
        .map(normalize_crate_name)
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| normalize_crate_name(&spec.service))
}

fn normalize_crate_name(input: &str) -> String {
    let mut out = String::new();
    let mut last_was_sep = false;
    for ch in to_snake_case(input).chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_was_sep = false;
        } else if !last_was_sep {
            out.push('-');
            last_was_sep = true;
        }
    }
    out.trim_matches('-').to_string()
}

fn roze_dependencies(
    source: DependencySource,
    local_crates_prefix: Option<&str>,
    crates: &'static [&'static str],
) -> String {
    crates
        .iter()
        .map(|name| match source {
            DependencySource::Git => format!(r#"{name} = {{ git = "{ROZE_GIT_URL}" }}"#),
            DependencySource::Path => {
                let prefix = local_crates_prefix.expect("local crates prefix");
                format!(r#"{name} = {{ path = "{prefix}/{name}" }}"#)
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn project_roze_crates(kind: ProjectKind) -> &'static [&'static str] {
    match kind {
        ProjectKind::Rest => &REST_ROZE_CRATES,
        ProjectKind::Rpc => &RPC_ROZE_CRATES,
    }
}

fn dependency_item(
    name: &str,
    source: DependencySource,
    local_crates_prefix: Option<&str>,
) -> toml_edit::Item {
    let mut dependency = toml_edit::InlineTable::new();
    match source {
        DependencySource::Git => {
            dependency.insert("git", ROZE_GIT_URL.into());
        }
        DependencySource::Path => {
            let prefix = local_crates_prefix.expect("local crates prefix");
            dependency.insert("path", format!("{prefix}/{name}").into());
        }
    }
    toml_edit::Item::Value(toml_edit::Value::InlineTable(dependency))
}

pub(super) fn local_crates_prefix(out: &Path, workspace_root: &Path) -> anyhow::Result<String> {
    let absolute_out = if out.is_absolute() {
        out.to_path_buf()
    } else {
        std::env::current_dir()?.join(out)
    };
    let relative = absolute_out.strip_prefix(workspace_root).with_context(|| {
        format!(
            "{} is not inside workspace {}",
            out.display(),
            workspace_root.display()
        )
    })?;
    let depth = relative.components().count();
    if depth == 0 {
        bail!("project output cannot be the workspace root");
    }

    Ok(format!("{}crates", "../".repeat(depth)))
}

fn readme(spec: &ApiSpec, kind: ProjectKind) -> String {
    match kind {
        ProjectKind::Rest => format!(
            r#"# {name}

Generated by `rozectl`.

## Run

```bash
cargo run
```

## Endpoints

- REST: `GET /healthz`
- REST: `GET /readyz`
- REST: `GET /startupz`
- REST: `GET /metrics`
- REST: `GET /openapi.json`
{rest_routes}

## Config

`config.yaml` is loaded from the crate directory first, then falls back to the current working directory.

## Production Evidence

Use `ops/production-evidence.md` before promoting this service beyond a controlled production path.
Run `powershell -ExecutionPolicy Bypass -File ops\production-verify.ps1` or `bash ops/production-verify.sh` in CI to fail fast on missing generated ops assets, format drift, compile errors, or test failures. GitHub Actions wiring is generated at `.github/workflows/roze-production-verify.yml`; CI evidence policy is generated at `ops/ci-evidence-policy.yaml`; the evidence index is generated at `ops/evidence-manifest.yaml`.
"#,
            name = spec.service,
            rest_routes = spec
                .rest_routes
                .iter()
                .map(|route| format!("- `{}` `{}`", method_name(&route.method), route.path))
                .collect::<Vec<_>>()
                .join("\n"),
        ),
        ProjectKind::Rpc => format!(
            r#"# {name}

Generated by `rozectl`.

## Run

```bash
cargo run
```

## RPC Methods

{rpc_methods}

## Config

`config.yaml` is loaded from the crate directory first, then falls back to the current working directory.

## Production Evidence

Use `ops/production-evidence.md` before promoting this service beyond a controlled production path.
Run `powershell -ExecutionPolicy Bypass -File ops\production-verify.ps1` or `bash ops/production-verify.sh` in CI to fail fast on missing generated ops assets, format drift, compile errors, or test failures. GitHub Actions wiring is generated at `.github/workflows/roze-production-verify.yml`; CI evidence policy is generated at `ops/ci-evidence-policy.yaml`; the evidence index is generated at `ops/evidence-manifest.yaml`.
"#,
            name = spec.service,
            rpc_methods = spec
                .rpc_methods
                .iter()
                .map(|method| format!("- `{}` -> `{}`", method.name, method.request))
                .collect::<Vec<_>>()
                .join("\n"),
        ),
    }
}

fn production_evidence_runbook(spec: &ApiSpec, kind: ProjectKind) -> String {
    let boundary = match kind {
        ProjectKind::Rest => "REST",
        ProjectKind::Rpc => "RPC",
    };
    let workload = match kind {
        ProjectKind::Rest => {
            if spec.rest_routes.is_empty() {
                "health/readiness/startup probes, metrics scrape, and representative REST traffic"
                    .to_string()
            } else {
                format!(
                    "health/readiness/startup probes, metrics scrape, and representative REST traffic for {} route(s)",
                    spec.rest_routes.len()
                )
            }
        }
        ProjectKind::Rpc => {
            if spec.rpc_methods.is_empty() {
                "startup, client RPC calls, graceful shutdown, and dependency failure drills"
                    .to_string()
            } else {
                format!(
                    "startup, client RPC calls for {} method(s), graceful shutdown, and dependency failure drills",
                    spec.rpc_methods.len()
                )
            }
        }
    };
    let failure_injection = match kind {
        ProjectKind::Rest => {
            "shutdown signal, dependency timeout, invalid config reload, slow handler, readiness failure"
        }
        ProjectKind::Rpc => {
            "shutdown signal, dependency timeout, upstream deadline exceeded, invalid config reload"
        }
    };

    format!(
        r#"# Production Evidence Runbook: {name}

Generated by `rozectl` for the {boundary} boundary.

Roze services are not production-stable by declaration alone. Keep this file
with the service and attach completed reports under `docs/evidence/` or your
team evidence store before broad rollout.

## Required Gates

- `cargo fmt --all -- --check`
- `cargo test`
- Generated project compile check in the Roze workspace
- Runtime smoke against `/healthz`, `/readyz`, `/startupz`, `/metrics`, `/openapi.json`, `/reports/export`, and `/charts/query` for REST services
- Graceful shutdown and lifecycle snapshot evidence
- Resource trend capture: CPU, memory, file descriptors, connections, and restart count
- Failure timeline with recovery outcome

## Architecture Borrowed And Extended

This generated service follows the strong parts of go-zero style architecture:

- Simple IDL-first boundaries and one generated ownership layout
- Failure-oriented resilience: timeout, rate limit, circuit breaker, load shedding, retry budget, and deadline propagation must be configured before broad rollout
- Service governance: discovery, load balancing, tracing, metrics, health checks, and structured logs must be observable in one dashboard
- Developer-friendly extension points: keep business logic in `logic`, dependencies in `svc`, transport glue in generated modules
- Easy extension without forking generated ownership: add middleware, dependencies, and background tasks through Roze extension points

Roze extends that baseline with generated evidence gates. Production readiness
requires reproducible reports, lifecycle snapshots, failure timelines, and
resource trends instead of relying on maturity claims alone.

The machine-readable governance baseline is generated at
`ops/governance-baseline.yaml`; wire CI or platform checks to that file before
promotion.
Prometheus alert templates are generated at `ops/prometheus-rules.yaml`; review
metric names against your registry, then attach the enabled rules to the
evidence report.
Grafana dashboard templates are generated at `ops/grafana-dashboard.json`; use
them to attach latency, throughput, error, and resilience panels to the report.
SLO and error-budget defaults are generated at `ops/slo.yaml`; tune them with
the service owner before launch and attach burn-rate evidence to the report.
Failure-injection plans are generated at `ops/failure-injection-plan.yaml`; run
them in staging before promotion and copy observed recovery into the report.
Release rollout gates are generated at `ops/release-rollout.yaml`; use them for
canary, blue-green, rollback, and post-release evidence before broad rollout.
Incident response playbooks are generated at `ops/incident-response.yaml`; use
them to connect alerts to triage, mitigation, rollback, and postmortem evidence.
Capacity plans are generated at `ops/capacity-plan.yaml`; use them to prove
load, soak, burst, resource trend, and scaling behavior before broad rollout.
Security readiness plans are generated at `ops/security-readiness.yaml`; use
them to prove auth, authorization, tenant isolation, key rotation, mTLS, and
audit evidence before broad rollout.
Production gates are generated at `ops/production-gate.yaml`; use that file as
the CI or platform entrypoint that validates all generated production assets.
Regeneration policies are generated at `ops/regeneration-policy.yaml`; use them
to block unsafe IDL drift, generated ownership edits, and missing evidence reruns.
Client contracts are generated at `ops/client-contract.yaml`; use them to
validate generated SDK/OpenAPI/proto projections, typed errors, auth injection,
timeouts, retry budget, and trace propagation.
Config governance plans are generated at `ops/config-governance.yaml`; use them
to validate config schema, diff, audit, canary rollout, hot reload isolation,
rollback, and snapshot recovery evidence.
Reliable event plans are generated at `ops/reliable-events.yaml`; use them to
validate event envelopes, idempotency, outbox/inbox, DLQ, replay, lag, retry
budget, and retry storm protection before enabling asynchronous workflows.
Dependency governance plans are generated at `ops/dependency-governance.yaml`;
use them to validate service discovery, load balancing, connection pools,
deadline propagation, circuit breakers, bulkheads, outlier behavior, and
fallback evidence for every downstream.
Data consistency plans are generated at `ops/data-consistency.yaml`; use them to
validate transactions, migrations, idempotent writes, outbox/DTM/Saga, read-write
consistency, backup restore, and data rollback evidence.
Observability contracts are generated at `ops/observability-contract.yaml`; use
them to validate metrics, logs, traces, profiles, sampling, label cardinality,
debug queries, and evidence retention.
Runtime hardening contracts are generated at `ops/runtime-hardening.yaml`; use
them to validate timeout, rate limit, circuit breaker, load shedding, retry
budget, deadline propagation, graceful shutdown, backpressure, and resource
guard evidence.
Error contracts are generated at `ops/error-contract.yaml`; use them to validate
typed errors, transport status mapping, retryability, trace correlation, client
behavior, redaction, and failure metrics.
Deployment topology contracts are generated at `ops/deployment-topology.yaml`;
use them to validate probes, resources, scaling, disruption budgets,
configuration, secrets, registry, network policy, and rollback wiring.
Service communication contracts are generated at `ops/service-communication.yaml`;
use them to validate discovery, load balancing, client deadlines, retries,
circuit breakers, fallback, outlier handling, and trace propagation for service
calls.
Cache governance contracts are generated at `ops/cache-governance.yaml`; use
them to validate TTL, key ownership, local/remote cache policy, singleflight,
penetration/breakdown/avalanche protection, invalidation, consistency, and cache
metrics.
Data access governance contracts are generated at
`ops/data-access-governance.yaml`; use them to validate query deadlines,
connection pools, slow-query budgets, pagination, index review, read/write
splitting, N+1 prevention, and data-access metrics.
Interface governance contracts are generated at
`ops/interface-governance.yaml`; use them to validate generated framework
interfaces, business IDL interfaces, OpenAPI/proto projection, framework smoke
coverage, typed errors, auth boundaries, and bounded observability labels.
Executable production verification is generated at `ops/production-verify.ps1`
and `ops/production-verify.sh`; run one in CI to fail fast on missing generated
ops assets, format drift, compile errors, and test failures before collecting
long-run evidence.
Generated GitHub Actions wiring is available at
`.github/workflows/roze-production-verify.yml`; it runs the verification scripts
on Linux and Windows so generated services prove the same production gates on
both runner families.
CI evidence policy is generated at `ops/ci-evidence-policy.yaml`; use it to
validate artifact naming, retention, uploaded evidence paths, blocking gates,
and the promotion rule that CI success is only a precondition for long-run
production evidence.
Evidence manifests are generated at `ops/evidence-manifest.yaml`; use them as
the machine-readable index for every generated ops contract, verification
script, workflow, smoke surface, and promotion evidence requirement uploaded in
the CI artifact.

## Evidence Scaffold

From the Roze workspace, generate a report scaffold with:

```bash
bash scripts/production-evidence.sh \
  --area generated-services \
  --duration 24h \
  --workload "{workload}" \
  --failure-injection "{failure_injection}" \
  --command "cargo run"
```

For lifecycle soak evidence, copy the complete `roze_lifecycle_soak` summary
line into:

```bash
bash scripts/production-evidence.sh \
  --area lifecycle \
  --duration 24h \
  --workload "start, drain, shutdown, failed task, timeout hooks" \
  --failure-injection "stuck task, signal shutdown, hook timeout" \
  --command "ROZE_LIFECYCLE_SOAK_SECONDS=86400 ROZE_LIFECYCLE_SOAK_CYCLES=100000000 bash scripts/production-soak-lifecycle.sh" \
  --lifecycle-summary "roze_lifecycle_soak cycles=... worker_exits=... stop_hooks=... running_snapshots=... stopped_snapshots=... max_service_count=..."
```

The lifecycle summary is rejected unless all fields are numeric and internally
consistent.

## Promotion Rule

Do not mark this service broadly production-stable until the generated-services
report, lifecycle report, logs, metrics, traces, and rollback notes are complete.
"#,
        name = spec.service,
        boundary = boundary,
        workload = workload,
        failure_injection = failure_injection,
    )
}

fn production_verify_ps1(spec: &ApiSpec, kind: ProjectKind) -> String {
    use std::fmt::Write as _;

    let boundary = match kind {
        ProjectKind::Rest => "REST",
        ProjectKind::Rpc => "RPC",
    };
    let service = ps_single_quoted(&spec.service);
    let mut out = String::new();

    writeln!(&mut out, "# Generated by rozectl.").unwrap();
    writeln!(&mut out, "# service: {service}").unwrap();
    writeln!(&mut out, "# boundary: {boundary}").unwrap();
    match kind {
        ProjectKind::Rest => {
            writeln!(&mut out, "# rest_routes: {}", spec.rest_routes.len()).unwrap();
        }
        ProjectKind::Rpc => {
            writeln!(&mut out, "# rpc_methods: {}", spec.rpc_methods.len()).unwrap();
        }
    }
    writeln!(&mut out, "param(").unwrap();
    writeln!(&mut out, "    [switch]$SkipTests,").unwrap();
    writeln!(&mut out, "    [switch]$SkipEvidenceInventory,").unwrap();
    writeln!(
        &mut out,
        "    [string]$ManifestPath = (Join-Path (Split-Path -Parent $PSScriptRoot) 'Cargo.toml')"
    )
    .unwrap();
    writeln!(&mut out, ")").unwrap();
    writeln!(&mut out).unwrap();
    writeln!(&mut out, "Set-StrictMode -Version Latest").unwrap();
    writeln!(&mut out, "$ErrorActionPreference = 'Stop'").unwrap();
    writeln!(&mut out, "$ProjectRoot = Split-Path -Parent $PSScriptRoot").unwrap();
    writeln!(&mut out, "$ServiceName = '{service}'").unwrap();
    writeln!(&mut out, "$Boundary = '{boundary}'").unwrap();
    writeln!(&mut out).unwrap();
    writeln!(&mut out, "function Invoke-Step {{").unwrap();
    writeln!(&mut out, "    param(").unwrap();
    writeln!(
        &mut out,
        "        [Parameter(Mandatory = $true)][string]$Name,"
    )
    .unwrap();
    writeln!(
        &mut out,
        "        [Parameter(Mandatory = $true)][scriptblock]$Command"
    )
    .unwrap();
    writeln!(&mut out, "    )").unwrap();
    writeln!(&mut out, "    Write-Host \"==> $Name\"").unwrap();
    writeln!(&mut out, "    $global:LASTEXITCODE = 0").unwrap();
    writeln!(&mut out, "    & $Command").unwrap();
    writeln!(&mut out, "    if ($LASTEXITCODE -ne 0) {{").unwrap();
    writeln!(
        &mut out,
        "        throw \"$Name failed with exit code $LASTEXITCODE\""
    )
    .unwrap();
    writeln!(&mut out, "    }}").unwrap();
    writeln!(&mut out, "}}").unwrap();
    writeln!(&mut out).unwrap();
    writeln!(&mut out, "$requiredOpsFiles = @(").unwrap();
    for path in [
        "ops/production-evidence.md",
        "ops/governance-baseline.yaml",
        "ops/prometheus-rules.yaml",
        "ops/grafana-dashboard.json",
        "ops/slo.yaml",
        "ops/failure-injection-plan.yaml",
        "ops/release-rollout.yaml",
        "ops/incident-response.yaml",
        "ops/capacity-plan.yaml",
        "ops/security-readiness.yaml",
        "ops/production-gate.yaml",
        "ops/regeneration-policy.yaml",
        "ops/client-contract.yaml",
        "ops/config-governance.yaml",
        "ops/reliable-events.yaml",
        "ops/dependency-governance.yaml",
        "ops/data-consistency.yaml",
        "ops/observability-contract.yaml",
        "ops/runtime-hardening.yaml",
        "ops/error-contract.yaml",
        "ops/deployment-topology.yaml",
        "ops/service-communication.yaml",
        "ops/cache-governance.yaml",
        "ops/data-access-governance.yaml",
        "ops/interface-governance.yaml",
        "ops/production-verify.ps1",
        "ops/production-verify.sh",
        "ops/ci-evidence-policy.yaml",
        "ops/evidence-manifest.yaml",
        ".github/workflows/roze-production-verify.yml",
    ] {
        writeln!(&mut out, "    '{path}'").unwrap();
    }
    writeln!(&mut out, ")").unwrap();
    writeln!(&mut out).unwrap();
    writeln!(
        &mut out,
        "$EvidenceManifestPath = Join-Path $ProjectRoot 'ops/evidence-manifest.yaml'"
    )
    .unwrap();
    writeln!(
        &mut out,
        "$CiEvidencePolicyPath = Join-Path $ProjectRoot 'ops/ci-evidence-policy.yaml'"
    )
    .unwrap();
    writeln!(
        &mut out,
        "$VerifyReportPath = Join-Path $ProjectRoot 'ops/production-verify-report.json'"
    )
    .unwrap();
    writeln!(&mut out).unwrap();
    writeln!(&mut out, "if (-not $SkipEvidenceInventory) {{").unwrap();
    writeln!(
        &mut out,
        "    Invoke-Step 'generated ops asset inventory' {{"
    )
    .unwrap();
    writeln!(
        &mut out,
        "        foreach ($relativePath in $requiredOpsFiles) {{"
    )
    .unwrap();
    writeln!(
        &mut out,
        "            $candidate = Join-Path $ProjectRoot $relativePath"
    )
    .unwrap();
    writeln!(
        &mut out,
        "            if (-not (Test-Path -LiteralPath $candidate)) {{"
    )
    .unwrap();
    writeln!(
        &mut out,
        "                throw \"Missing generated ops asset: $relativePath\""
    )
    .unwrap();
    writeln!(&mut out, "            }}").unwrap();
    writeln!(&mut out, "        }}").unwrap();
    writeln!(&mut out, "    }}").unwrap();
    writeln!(&mut out, "    Invoke-Step 'evidence manifest coverage' {{").unwrap();
    writeln!(
        &mut out,
        "        $manifest = Get-Content -LiteralPath $EvidenceManifestPath -Raw"
    )
    .unwrap();
    writeln!(
        &mut out,
        "        foreach ($relativePath in $requiredOpsFiles) {{"
    )
    .unwrap();
    writeln!(
        &mut out,
        "            $manifestEntry = \"path: $relativePath\""
    )
    .unwrap();
    writeln!(
        &mut out,
        "            if (-not $manifest.Contains($manifestEntry)) {{"
    )
    .unwrap();
    writeln!(
        &mut out,
        "                throw \"Evidence manifest does not index generated asset entry: $manifestEntry\""
    )
    .unwrap();
    writeln!(&mut out, "            }}").unwrap();
    writeln!(&mut out, "        }}").unwrap();
    writeln!(&mut out, "    }}").unwrap();
    writeln!(&mut out, "    Invoke-Step 'ci evidence policy coverage' {{").unwrap();
    writeln!(
        &mut out,
        "        $policy = Get-Content -LiteralPath $CiEvidencePolicyPath -Raw"
    )
    .unwrap();
    writeln!(
        &mut out,
        "        foreach ($relativePath in $requiredOpsFiles) {{"
    )
    .unwrap();
    writeln!(
        &mut out,
        "            $policyEntry = \"    - $relativePath\""
    )
    .unwrap();
    writeln!(
        &mut out,
        "            if (-not $policy.Contains($policyEntry)) {{"
    )
    .unwrap();
    writeln!(
        &mut out,
        "                throw \"CI evidence policy does not require generated asset path: $policyEntry\""
    )
    .unwrap();
    writeln!(&mut out, "            }}").unwrap();
    writeln!(&mut out, "        }}").unwrap();
    writeln!(&mut out, "    }}").unwrap();
    writeln!(&mut out, "}}").unwrap();
    writeln!(&mut out).unwrap();
    writeln!(
        &mut out,
        "Invoke-Step 'cargo fmt generated service' {{ cargo fmt --manifest-path $ManifestPath -- --check }}"
    )
    .unwrap();
    writeln!(
        &mut out,
        "Invoke-Step 'cargo check generated service' {{ cargo check --manifest-path $ManifestPath }}"
    )
    .unwrap();
    writeln!(&mut out, "if (-not $SkipTests) {{").unwrap();
    writeln!(
        &mut out,
        "    Invoke-Step 'cargo test generated service' {{ cargo test --manifest-path $ManifestPath }}"
    )
    .unwrap();
    writeln!(&mut out, "}}").unwrap();
    writeln!(&mut out).unwrap();
    match kind {
        ProjectKind::Rest => {
            writeln!(&mut out, "$frameworkSmoke = @(").unwrap();
            for endpoint in [
                "GET /healthz",
                "GET /readyz",
                "GET /startupz",
                "GET /metrics",
                "GET /openapi.json",
                "GET /reports/export",
                "POST /charts/query",
            ] {
                writeln!(&mut out, "    '{endpoint}'").unwrap();
            }
            for route in &spec.rest_routes {
                writeln!(
                    &mut out,
                    "    '{} {}'",
                    method_name(&route.method),
                    ps_single_quoted(&route.path)
                )
                .unwrap();
            }
            writeln!(&mut out, ")").unwrap();
            writeln!(
                &mut out,
                "Write-Host ('REST smoke endpoints required: ' + ($frameworkSmoke -join ', '))"
            )
            .unwrap();
        }
        ProjectKind::Rpc => {
            writeln!(&mut out, "$rpcSmoke = @(").unwrap();
            for method in &spec.rpc_methods {
                writeln!(
                    &mut out,
                    "    '{} -> {}'",
                    ps_single_quoted(&method.name),
                    ps_single_quoted(&method.request)
                )
                .unwrap();
            }
            writeln!(&mut out, ")").unwrap();
            writeln!(
                &mut out,
                "Write-Host ('RPC smoke methods required: ' + ($rpcSmoke -join ', '))"
            )
            .unwrap();
        }
    }
    writeln!(&mut out, "$rustcVersion = (& rustc --version) -join ''").unwrap();
    writeln!(&mut out, "$cargoVersion = (& cargo --version) -join ''").unwrap();
    writeln!(&mut out, "$rozeRevision = 'unknown'").unwrap();
    writeln!(
        &mut out,
        "if (Get-Command git -ErrorAction SilentlyContinue) {{"
    )
    .unwrap();
    writeln!(
        &mut out,
        "    $gitRevision = & git -C $ProjectRoot rev-parse HEAD 2>$null"
    )
    .unwrap();
    writeln!(
        &mut out,
        "    if ($LASTEXITCODE -eq 0) {{ $rozeRevision = ($gitRevision -join '') }}"
    )
    .unwrap();
    writeln!(&mut out, "}}").unwrap();
    writeln!(&mut out, "$verifyReport = [ordered]@{{").unwrap();
    writeln!(&mut out, "    service = $ServiceName").unwrap();
    writeln!(&mut out, "    boundary = $Boundary.ToLowerInvariant()").unwrap();
    writeln!(&mut out, "    generated_by = 'rozectl'").unwrap();
    writeln!(&mut out, "    verdict = 'pass_ci_precondition'").unwrap();
    writeln!(
        &mut out,
        "    broad_production = 'requires_long_run_evidence'"
    )
    .unwrap();
    writeln!(&mut out, "    skip_tests = [bool]$SkipTests").unwrap();
    writeln!(
        &mut out,
        "    timestamp_utc = (Get-Date).ToUniversalTime().ToString('o')"
    )
    .unwrap();
    writeln!(
        &mut out,
        "    os = [System.Runtime.InteropServices.RuntimeInformation]::OSDescription"
    )
    .unwrap();
    writeln!(&mut out, "    rustc = $rustcVersion").unwrap();
    writeln!(&mut out, "    cargo = $cargoVersion").unwrap();
    writeln!(&mut out, "    roze_revision = $rozeRevision").unwrap();
    writeln!(&mut out, "    gates = @(").unwrap();
    for gate in [
        "generated_ops_asset_inventory",
        "evidence_manifest_coverage",
        "ci_evidence_policy_coverage",
        "cargo_fmt_check",
        "cargo_check",
        "cargo_test_unless_skipped",
        "smoke_surface_declared",
    ] {
        writeln!(&mut out, "        '{gate}'").unwrap();
    }
    writeln!(&mut out, "    )").unwrap();
    writeln!(&mut out, "    required_followup_evidence = @(").unwrap();
    for evidence in [
        "24h_or_72h_soak_report",
        "failure_injection_report",
        "dashboard_and_alert_evidence",
        "rollback_or_rollforward_evidence",
        "security_readiness_signoff",
        "capacity_and_resource_trend",
    ] {
        writeln!(&mut out, "        '{evidence}'").unwrap();
    }
    writeln!(&mut out, "    )").unwrap();
    writeln!(&mut out, "}}").unwrap();
    writeln!(
        &mut out,
        "$verifyReport | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath $VerifyReportPath -Encoding UTF8"
    )
    .unwrap();
    writeln!(
        &mut out,
        "Write-Host \"Production verification report: $VerifyReportPath\""
    )
    .unwrap();
    writeln!(
        &mut out,
        "Write-Host \"Production verification passed for $ServiceName ($Boundary). Attach long-run soak, failure-injection, dashboard, alert, and rollback evidence before broad rollout.\""
    )
    .unwrap();

    out
}

fn production_verify_sh(spec: &ApiSpec, kind: ProjectKind) -> String {
    use std::fmt::Write as _;

    let boundary = match kind {
        ProjectKind::Rest => "REST",
        ProjectKind::Rpc => "RPC",
    };
    let service = sh_single_quoted(&spec.service);
    let mut out = String::new();

    writeln!(&mut out, "#!/usr/bin/env bash").unwrap();
    writeln!(&mut out, "# Generated by rozectl.").unwrap();
    writeln!(&mut out, "# service: {service}").unwrap();
    writeln!(&mut out, "# boundary: {boundary}").unwrap();
    match kind {
        ProjectKind::Rest => {
            writeln!(&mut out, "# rest_routes: {}", spec.rest_routes.len()).unwrap();
        }
        ProjectKind::Rpc => {
            writeln!(&mut out, "# rpc_methods: {}", spec.rpc_methods.len()).unwrap();
        }
    }
    writeln!(&mut out, "set -euo pipefail").unwrap();
    writeln!(&mut out).unwrap();
    writeln!(
        &mut out,
        "SCRIPT_DIR=\"$(cd \"$(dirname \"${{BASH_SOURCE[0]}}\")\" && pwd)\""
    )
    .unwrap();
    writeln!(&mut out, "PROJECT_ROOT=\"$(cd \"$SCRIPT_DIR/..\" && pwd)\"").unwrap();
    writeln!(
        &mut out,
        "MANIFEST_PATH=\"${{MANIFEST_PATH:-$PROJECT_ROOT/Cargo.toml}}\""
    )
    .unwrap();
    writeln!(
        &mut out,
        "EVIDENCE_MANIFEST_PATH=\"$PROJECT_ROOT/ops/evidence-manifest.yaml\""
    )
    .unwrap();
    writeln!(
        &mut out,
        "CI_EVIDENCE_POLICY_PATH=\"$PROJECT_ROOT/ops/ci-evidence-policy.yaml\""
    )
    .unwrap();
    writeln!(
        &mut out,
        "VERIFY_REPORT_PATH=\"$PROJECT_ROOT/ops/production-verify-report.json\""
    )
    .unwrap();
    writeln!(&mut out, "SKIP_TESTS=\"${{SKIP_TESTS:-0}}\"").unwrap();
    writeln!(
        &mut out,
        "SKIP_EVIDENCE_INVENTORY=\"${{SKIP_EVIDENCE_INVENTORY:-0}}\""
    )
    .unwrap();
    writeln!(&mut out, "SERVICE_NAME={service}").unwrap();
    writeln!(&mut out, "BOUNDARY={}", sh_single_quoted(boundary)).unwrap();
    writeln!(&mut out).unwrap();
    writeln!(&mut out, "run_step() {{").unwrap();
    writeln!(&mut out, "  local name=\"$1\"").unwrap();
    writeln!(&mut out, "  shift").unwrap();
    writeln!(&mut out, "  echo \"==> $name\"").unwrap();
    writeln!(&mut out, "  \"$@\"").unwrap();
    writeln!(&mut out, "}}").unwrap();
    writeln!(&mut out).unwrap();
    writeln!(&mut out, "json_escape() {{").unwrap();
    writeln!(&mut out, "  local value=\"$1\"").unwrap();
    writeln!(&mut out, "  value=${{value//\\\\/\\\\\\\\}}").unwrap();
    writeln!(&mut out, "  value=${{value//\\\"/\\\\\\\"}}").unwrap();
    writeln!(&mut out, "  value=${{value//$'\\n'/ }}").unwrap();
    writeln!(&mut out, "  printf '%s' \"$value\"").unwrap();
    writeln!(&mut out, "}}").unwrap();
    writeln!(&mut out).unwrap();
    writeln!(&mut out, "required_ops_files=(").unwrap();
    for path in [
        "ops/production-evidence.md",
        "ops/governance-baseline.yaml",
        "ops/prometheus-rules.yaml",
        "ops/grafana-dashboard.json",
        "ops/slo.yaml",
        "ops/failure-injection-plan.yaml",
        "ops/release-rollout.yaml",
        "ops/incident-response.yaml",
        "ops/capacity-plan.yaml",
        "ops/security-readiness.yaml",
        "ops/production-gate.yaml",
        "ops/regeneration-policy.yaml",
        "ops/client-contract.yaml",
        "ops/config-governance.yaml",
        "ops/reliable-events.yaml",
        "ops/dependency-governance.yaml",
        "ops/data-consistency.yaml",
        "ops/observability-contract.yaml",
        "ops/runtime-hardening.yaml",
        "ops/error-contract.yaml",
        "ops/deployment-topology.yaml",
        "ops/service-communication.yaml",
        "ops/cache-governance.yaml",
        "ops/data-access-governance.yaml",
        "ops/interface-governance.yaml",
        "ops/production-verify.ps1",
        "ops/production-verify.sh",
        "ops/ci-evidence-policy.yaml",
        "ops/evidence-manifest.yaml",
        ".github/workflows/roze-production-verify.yml",
    ] {
        writeln!(&mut out, "  {}", sh_single_quoted(path)).unwrap();
    }
    writeln!(&mut out, ")").unwrap();
    writeln!(&mut out).unwrap();
    writeln!(&mut out, "check_ops_inventory() {{").unwrap();
    writeln!(&mut out, "  local relative_path").unwrap();
    writeln!(&mut out, "  local candidate").unwrap();
    writeln!(
        &mut out,
        "  for relative_path in \"${{required_ops_files[@]}}\"; do"
    )
    .unwrap();
    writeln!(&mut out, "    candidate=\"$PROJECT_ROOT/$relative_path\"").unwrap();
    writeln!(&mut out, "    if [[ ! -e \"$candidate\" ]]; then").unwrap();
    writeln!(
        &mut out,
        "      echo \"Missing generated ops asset: $relative_path\" >&2"
    )
    .unwrap();
    writeln!(&mut out, "      return 1").unwrap();
    writeln!(&mut out, "    fi").unwrap();
    writeln!(&mut out, "  done").unwrap();
    writeln!(&mut out, "}}").unwrap();
    writeln!(&mut out).unwrap();
    writeln!(&mut out, "check_evidence_manifest_coverage() {{").unwrap();
    writeln!(&mut out, "  local relative_path").unwrap();
    writeln!(&mut out, "  local manifest_entry").unwrap();
    writeln!(
        &mut out,
        "  for relative_path in \"${{required_ops_files[@]}}\"; do"
    )
    .unwrap();
    writeln!(&mut out, "    manifest_entry=\"path: $relative_path\"").unwrap();
    writeln!(
        &mut out,
        "    if ! grep -Fq -- \"$manifest_entry\" \"$EVIDENCE_MANIFEST_PATH\"; then"
    )
    .unwrap();
    writeln!(
        &mut out,
        "      echo \"Evidence manifest does not index generated asset entry: $manifest_entry\" >&2"
    )
    .unwrap();
    writeln!(&mut out, "      return 1").unwrap();
    writeln!(&mut out, "    fi").unwrap();
    writeln!(&mut out, "  done").unwrap();
    writeln!(&mut out, "}}").unwrap();
    writeln!(&mut out).unwrap();
    writeln!(&mut out, "check_ci_evidence_policy_coverage() {{").unwrap();
    writeln!(&mut out, "  local relative_path").unwrap();
    writeln!(&mut out, "  local policy_entry").unwrap();
    writeln!(
        &mut out,
        "  for relative_path in \"${{required_ops_files[@]}}\"; do"
    )
    .unwrap();
    writeln!(&mut out, "    policy_entry=\"    - $relative_path\"").unwrap();
    writeln!(
        &mut out,
        "    if ! grep -Fq -- \"$policy_entry\" \"$CI_EVIDENCE_POLICY_PATH\"; then"
    )
    .unwrap();
    writeln!(
        &mut out,
        "      echo \"CI evidence policy does not require generated asset path: $policy_entry\" >&2"
    )
    .unwrap();
    writeln!(&mut out, "      return 1").unwrap();
    writeln!(&mut out, "    fi").unwrap();
    writeln!(&mut out, "  done").unwrap();
    writeln!(&mut out, "}}").unwrap();
    writeln!(&mut out).unwrap();
    writeln!(
        &mut out,
        "if [[ \"$SKIP_EVIDENCE_INVENTORY\" != \"1\" ]]; then"
    )
    .unwrap();
    writeln!(
        &mut out,
        "  run_step \"generated ops asset inventory\" check_ops_inventory"
    )
    .unwrap();
    writeln!(
        &mut out,
        "  run_step \"evidence manifest coverage\" check_evidence_manifest_coverage"
    )
    .unwrap();
    writeln!(
        &mut out,
        "  run_step \"ci evidence policy coverage\" check_ci_evidence_policy_coverage"
    )
    .unwrap();
    writeln!(&mut out, "fi").unwrap();
    writeln!(&mut out).unwrap();
    writeln!(
        &mut out,
        "run_step \"cargo fmt generated service\" cargo fmt --manifest-path \"$MANIFEST_PATH\" -- --check"
    )
    .unwrap();
    writeln!(
        &mut out,
        "run_step \"cargo check generated service\" cargo check --manifest-path \"$MANIFEST_PATH\""
    )
    .unwrap();
    writeln!(&mut out, "if [[ \"$SKIP_TESTS\" != \"1\" ]]; then").unwrap();
    writeln!(
        &mut out,
        "  run_step \"cargo test generated service\" cargo test --manifest-path \"$MANIFEST_PATH\""
    )
    .unwrap();
    writeln!(&mut out, "fi").unwrap();
    writeln!(&mut out).unwrap();
    match kind {
        ProjectKind::Rest => {
            writeln!(&mut out, "framework_smoke=(").unwrap();
            for endpoint in [
                "GET /healthz",
                "GET /readyz",
                "GET /startupz",
                "GET /metrics",
                "GET /openapi.json",
                "GET /reports/export",
                "POST /charts/query",
            ] {
                writeln!(&mut out, "  {}", sh_single_quoted(endpoint)).unwrap();
            }
            for route in &spec.rest_routes {
                let smoke = format!("{} {}", method_name(&route.method), route.path);
                writeln!(&mut out, "  {}", sh_single_quoted(&smoke)).unwrap();
            }
            writeln!(&mut out, ")").unwrap();
            writeln!(
                &mut out,
                "printf 'REST smoke endpoints required: %s\\n' \"${{framework_smoke[*]}}\""
            )
            .unwrap();
        }
        ProjectKind::Rpc => {
            writeln!(&mut out, "rpc_smoke=(").unwrap();
            for method in &spec.rpc_methods {
                let smoke = format!("{} -> {}", method.name, method.request);
                writeln!(&mut out, "  {}", sh_single_quoted(&smoke)).unwrap();
            }
            writeln!(&mut out, ")").unwrap();
            writeln!(
                &mut out,
                "printf 'RPC smoke methods required: %s\\n' \"${{rpc_smoke[*]}}\""
            )
            .unwrap();
        }
    }
    writeln!(&mut out, "rustc_version=\"$(rustc --version)\"").unwrap();
    writeln!(&mut out, "cargo_version=\"$(cargo --version)\"").unwrap();
    writeln!(&mut out, "roze_revision=\"unknown\"").unwrap();
    writeln!(
        &mut out,
        "if command -v git >/dev/null 2>&1 && git -C \"$PROJECT_ROOT\" rev-parse HEAD >/dev/null 2>&1; then"
    )
    .unwrap();
    writeln!(
        &mut out,
        "  roze_revision=\"$(git -C \"$PROJECT_ROOT\" rev-parse HEAD)\""
    )
    .unwrap();
    writeln!(&mut out, "fi").unwrap();
    writeln!(&mut out, "skip_tests_json=false").unwrap();
    writeln!(
        &mut out,
        "if [[ \"$SKIP_TESTS\" == \"1\" ]]; then skip_tests_json=true; fi"
    )
    .unwrap();
    writeln!(&mut out, "cat > \"$VERIFY_REPORT_PATH\" <<JSON").unwrap();
    writeln!(&mut out, "{{").unwrap();
    writeln!(
        &mut out,
        "  \"service\": \"$(json_escape \"$SERVICE_NAME\")\","
    )
    .unwrap();
    writeln!(
        &mut out,
        "  \"boundary\": \"$(json_escape \"${{BOUNDARY,,}}\")\","
    )
    .unwrap();
    writeln!(&mut out, "  \"generated_by\": \"rozectl\",").unwrap();
    writeln!(&mut out, "  \"verdict\": \"pass_ci_precondition\",").unwrap();
    writeln!(
        &mut out,
        "  \"broad_production\": \"requires_long_run_evidence\","
    )
    .unwrap();
    writeln!(&mut out, "  \"skip_tests\": $skip_tests_json,").unwrap();
    writeln!(
        &mut out,
        "  \"timestamp_utc\": \"$(date -u +%Y-%m-%dT%H:%M:%SZ)\","
    )
    .unwrap();
    writeln!(
        &mut out,
        "  \"os\": \"$(json_escape \"$(uname -s)-$(uname -m)\")\","
    )
    .unwrap();
    writeln!(
        &mut out,
        "  \"rustc\": \"$(json_escape \"$rustc_version\")\","
    )
    .unwrap();
    writeln!(
        &mut out,
        "  \"cargo\": \"$(json_escape \"$cargo_version\")\","
    )
    .unwrap();
    writeln!(
        &mut out,
        "  \"roze_revision\": \"$(json_escape \"$roze_revision\")\","
    )
    .unwrap();
    writeln!(
        &mut out,
        "  \"gates\": [\"generated_ops_asset_inventory\", \"evidence_manifest_coverage\", \"ci_evidence_policy_coverage\", \"cargo_fmt_check\", \"cargo_check\", \"cargo_test_unless_skipped\", \"smoke_surface_declared\"],"
    )
    .unwrap();
    writeln!(
        &mut out,
        "  \"required_followup_evidence\": [\"24h_or_72h_soak_report\", \"failure_injection_report\", \"dashboard_and_alert_evidence\", \"rollback_or_rollforward_evidence\", \"security_readiness_signoff\", \"capacity_and_resource_trend\"]"
    )
    .unwrap();
    writeln!(&mut out, "}}").unwrap();
    writeln!(&mut out, "JSON").unwrap();
    writeln!(
        &mut out,
        "printf 'Production verification report: %s\\n' \"$VERIFY_REPORT_PATH\""
    )
    .unwrap();
    writeln!(
        &mut out,
        "printf 'Production verification passed for %s (%s). Attach long-run soak, failure-injection, dashboard, alert, and rollback evidence before broad rollout.\\n' \"$SERVICE_NAME\" \"$BOUNDARY\""
    )
    .unwrap();

    out
}

fn production_verify_workflow_yml(spec: &ApiSpec, kind: ProjectKind) -> String {
    let boundary = match kind {
        ProjectKind::Rest => "rest",
        ProjectKind::Rpc => "rpc",
    };

    format!(
        r#"# Generated by rozectl. Keep this workflow aligned with ops/production-verify.*.
name: Roze Production Verify

on:
  pull_request:
  push:
    branches:
      - main
      - master
  workflow_dispatch:
    inputs:
      skip_tests:
        description: "Skip cargo test and run compile/asset gates only"
        required: false
        default: "false"

permissions:
  contents: read

env:
  CARGO_TERM_COLOR: always
  ROZE_SERVICE_NAME: {service}
  ROZE_BOUNDARY: {boundary}

jobs:
  verify:
    name: ${{{{ matrix.os }}}} production gates
    runs-on: ${{{{ matrix.os }}}}
    strategy:
      fail-fast: false
      matrix:
        os:
          - ubuntu-latest
          - windows-latest

    steps:
      - name: Checkout
        uses: actions/checkout@v4

      - name: Install Rust
        uses: dtolnay/rust-toolchain@stable

      - name: Verify generated service on Linux
        if: runner.os != 'Windows'
        shell: bash
        env:
          SKIP_TESTS: ${{{{ github.event_name == 'workflow_dispatch' && inputs.skip_tests == 'true' && '1' || '0' }}}}
        run: bash ops/production-verify.sh

      - name: Verify generated service on Windows
        if: runner.os == 'Windows'
        shell: pwsh
        run: |
          $skipTests = "${{{{ github.event_name == 'workflow_dispatch' && inputs.skip_tests == 'true' && 'true' || 'false' }}}}" -eq "true"
          powershell -ExecutionPolicy Bypass -File ops\production-verify.ps1 -SkipTests:$skipTests

      - name: Upload production evidence bundle
        if: always()
        uses: actions/upload-artifact@v4
        with:
          name: roze-production-evidence-${{{{ matrix.os }}}}
          retention-days: 30
          if-no-files-found: error
          path: |
            ops/**
            .github/workflows/roze-production-verify.yml
"#,
        service = spec.service,
        boundary = boundary,
    )
}

fn ci_evidence_policy_yaml(spec: &ApiSpec, kind: ProjectKind) -> String {
    let boundary = match kind {
        ProjectKind::Rest => "rest",
        ProjectKind::Rpc => "rpc",
    };
    let smoke_surface = match kind {
        ProjectKind::Rest => "framework_probes_report_export_chart_query_and_business_routes",
        ProjectKind::Rpc => "startup_readiness_metrics_and_representative_rpc_methods",
    };

    format!(
        r#"# Generated by rozectl. Keep this policy machine-readable for CI and release tooling.
service: {service}
boundary: {boundary}
policy: ci_evidence
workflow: .github/workflows/roze-production-verify.yml
artifact:
  name_pattern: roze-production-evidence-${{{{ matrix.os }}}}
  retention_days: 30
  upload_on: always
  if_no_files_found: error
  required_paths:
    - ops/production-evidence.md
    - ops/governance-baseline.yaml
    - ops/prometheus-rules.yaml
    - ops/grafana-dashboard.json
    - ops/slo.yaml
    - ops/failure-injection-plan.yaml
    - ops/release-rollout.yaml
    - ops/incident-response.yaml
    - ops/capacity-plan.yaml
    - ops/security-readiness.yaml
    - ops/production-gate.yaml
    - ops/regeneration-policy.yaml
    - ops/client-contract.yaml
    - ops/config-governance.yaml
    - ops/reliable-events.yaml
    - ops/dependency-governance.yaml
    - ops/data-consistency.yaml
    - ops/observability-contract.yaml
    - ops/runtime-hardening.yaml
    - ops/error-contract.yaml
    - ops/deployment-topology.yaml
    - ops/service-communication.yaml
    - ops/cache-governance.yaml
    - ops/data-access-governance.yaml
    - ops/interface-governance.yaml
    - ops/production-verify.ps1
    - ops/production-verify.sh
    - ops/ci-evidence-policy.yaml
    - ops/evidence-manifest.yaml
    - .github/workflows/roze-production-verify.yml
  produced_paths:
    - ops/production-verify-report.json
required_runner_matrix:
  - ubuntu-latest
  - windows-latest
required_gates:
  - generated_ops_asset_inventory
  - cargo_fmt_check
  - cargo_check
  - cargo_test_unless_explicitly_skipped
  - {smoke_surface}
blocking_conditions:
  - missing_generated_ops_asset
  - missing_ci_evidence_policy
  - missing_evidence_manifest
  - missing_github_actions_workflow
  - fmt_drift
  - compile_failure
  - test_failure_without_approved_skip
  - artifact_upload_missing
promotion:
  ci_success_is: precondition
  broad_production_requires:
    - 24h_or_72h_soak_report
    - failure_injection_report
    - dashboard_and_alert_evidence
    - rollback_or_rollforward_evidence
    - security_readiness_signoff
    - capacity_and_resource_trend
"#,
        service = spec.service,
        boundary = boundary,
        smoke_surface = smoke_surface,
    )
}

fn evidence_manifest_yaml(spec: &ApiSpec, kind: ProjectKind) -> String {
    use std::fmt::Write as _;

    let boundary = match kind {
        ProjectKind::Rest => "rest",
        ProjectKind::Rpc => "rpc",
    };
    let mut out = String::new();

    writeln!(
        &mut out,
        "# Generated by rozectl. Keep this manifest machine-readable for evidence indexing."
    )
    .unwrap();
    writeln!(&mut out, "service: {}", spec.service).unwrap();
    writeln!(&mut out, "boundary: {boundary}").unwrap();
    writeln!(&mut out, "manifest: production_evidence").unwrap();
    writeln!(&mut out, "version: 1").unwrap();
    writeln!(&mut out, "generated_from: idl").unwrap();
    writeln!(&mut out, "artifacts:").unwrap();
    for (path, kind, purpose, blocking) in [
        (
            "ops/production-evidence.md",
            "runbook",
            "human production evidence workflow",
            true,
        ),
        (
            "ops/governance-baseline.yaml",
            "governance",
            "go-zero inspired resilience and ownership baseline",
            true,
        ),
        (
            "ops/prometheus-rules.yaml",
            "observability",
            "alert rule scaffold",
            true,
        ),
        (
            "ops/grafana-dashboard.json",
            "observability",
            "dashboard scaffold",
            true,
        ),
        ("ops/slo.yaml", "slo", "availability and error budget", true),
        (
            "ops/failure-injection-plan.yaml",
            "resilience",
            "failure drills and recovery evidence",
            true,
        ),
        (
            "ops/release-rollout.yaml",
            "release",
            "canary blue-green rollback gates",
            true,
        ),
        (
            "ops/incident-response.yaml",
            "operations",
            "alert triage rollback and postmortem playbook",
            true,
        ),
        (
            "ops/capacity-plan.yaml",
            "capacity",
            "load soak burst and scaling evidence",
            true,
        ),
        (
            "ops/security-readiness.yaml",
            "security",
            "auth tenant isolation key rotation mtls audit evidence",
            true,
        ),
        (
            "ops/production-gate.yaml",
            "gate",
            "release promotion gate",
            true,
        ),
        (
            "ops/regeneration-policy.yaml",
            "generation",
            "IDL drift and generated ownership policy",
            true,
        ),
        (
            "ops/client-contract.yaml",
            "contract",
            "SDK OpenAPI proto typed error and auth contract",
            true,
        ),
        (
            "ops/config-governance.yaml",
            "config",
            "config diff reload rollback and audit policy",
            true,
        ),
        (
            "ops/reliable-events.yaml",
            "events",
            "event envelope idempotency DLQ and retry storm policy",
            true,
        ),
        (
            "ops/dependency-governance.yaml",
            "dependency",
            "discovery load balancing deadline breaker and fallback evidence",
            true,
        ),
        (
            "ops/data-consistency.yaml",
            "data",
            "transaction migration outbox DTM and backup restore evidence",
            true,
        ),
        (
            "ops/observability-contract.yaml",
            "observability",
            "metric log trace profile and cardinality contract",
            true,
        ),
        (
            "ops/runtime-hardening.yaml",
            "runtime",
            "timeout rate limit breaker load shedding retry and shutdown evidence",
            true,
        ),
        (
            "ops/error-contract.yaml",
            "contract",
            "typed error mapping retryability and redaction evidence",
            true,
        ),
        (
            "ops/deployment-topology.yaml",
            "deployment",
            "probes resources scaling registry network and rollback topology",
            true,
        ),
        (
            "ops/service-communication.yaml",
            "dependency",
            "service call discovery balancing retry fallback and tracing policy",
            true,
        ),
        (
            "ops/cache-governance.yaml",
            "cache",
            "TTL ownership singleflight invalidation and cache metrics",
            true,
        ),
        (
            "ops/data-access-governance.yaml",
            "data",
            "query deadline pool slow query pagination and index review",
            true,
        ),
        (
            "ops/interface-governance.yaml",
            "interface",
            "framework and business interface smoke contract",
            true,
        ),
        (
            "ops/production-verify.ps1",
            "verification",
            "Windows fail-fast production verification",
            true,
        ),
        (
            "ops/production-verify.sh",
            "verification",
            "Linux fail-fast production verification",
            true,
        ),
        (
            "ops/ci-evidence-policy.yaml",
            "ci",
            "CI artifact and promotion evidence policy",
            true,
        ),
        (
            "ops/evidence-manifest.yaml",
            "manifest",
            "machine-readable evidence index",
            true,
        ),
        (
            ".github/workflows/roze-production-verify.yml",
            "ci",
            "cross-platform production verification workflow",
            true,
        ),
    ] {
        writeln!(&mut out, "  - path: {path}").unwrap();
        writeln!(&mut out, "    kind: {kind}").unwrap();
        writeln!(&mut out, "    purpose: {purpose}").unwrap();
        writeln!(&mut out, "    blocking: {blocking}").unwrap();
    }
    writeln!(&mut out, "runtime_artifacts:").unwrap();
    writeln!(&mut out, "  - path: ops/production-verify-report.json").unwrap();
    writeln!(&mut out, "    kind: verification_report").unwrap();
    writeln!(
        &mut out,
        "    purpose: machine-readable CI gate verdict and follow-up evidence checklist"
    )
    .unwrap();
    writeln!(&mut out, "    producer: ops/production-verify.*").unwrap();
    writeln!(&mut out, "    blocking: false").unwrap();
    writeln!(&mut out, "smoke_surface:").unwrap();
    match kind {
        ProjectKind::Rest => {
            writeln!(&mut out, "  framework:").unwrap();
            for endpoint in [
                "GET /healthz",
                "GET /readyz",
                "GET /startupz",
                "GET /metrics",
                "GET /openapi.json",
                "GET /reports/export",
                "POST /charts/query",
            ] {
                writeln!(&mut out, "    - {}", yaml_double_quoted(endpoint)).unwrap();
            }
            writeln!(&mut out, "  business:").unwrap();
            if spec.rest_routes.is_empty() {
                writeln!(&mut out, "    - none_declared").unwrap();
            } else {
                for route in &spec.rest_routes {
                    writeln!(&mut out, "    - method: {}", method_name(&route.method)).unwrap();
                    writeln!(&mut out, "      path: {}", yaml_double_quoted(&route.path)).unwrap();
                    writeln!(
                        &mut out,
                        "      request: {}",
                        yaml_double_quoted(&route.request)
                    )
                    .unwrap();
                    writeln!(
                        &mut out,
                        "      response: {}",
                        yaml_double_quoted(&route.response)
                    )
                    .unwrap();
                }
            }
        }
        ProjectKind::Rpc => {
            writeln!(&mut out, "  framework:").unwrap();
            writeln!(&mut out, "    - startup_readiness_metrics").unwrap();
            writeln!(&mut out, "    - client_deadline_and_cancel").unwrap();
            writeln!(&mut out, "  methods:").unwrap();
            if spec.rpc_methods.is_empty() {
                writeln!(&mut out, "    - none_declared").unwrap();
            } else {
                for method in &spec.rpc_methods {
                    writeln!(&mut out, "    - name: {}", yaml_double_quoted(&method.name)).unwrap();
                    writeln!(
                        &mut out,
                        "      request: {}",
                        yaml_double_quoted(&method.request)
                    )
                    .unwrap();
                    writeln!(
                        &mut out,
                        "      response: {}",
                        yaml_double_quoted(&method.response)
                    )
                    .unwrap();
                }
            }
        }
    }
    writeln!(&mut out, "promotion_evidence:").unwrap();
    for item in [
        "ci_evidence_bundle",
        "24h_or_72h_soak_report",
        "failure_injection_report",
        "dashboard_and_alert_evidence",
        "rollback_or_rollforward_evidence",
        "security_readiness_signoff",
        "capacity_and_resource_trend",
    ] {
        writeln!(&mut out, "  - {item}").unwrap();
    }
    writeln!(&mut out, "policy:").unwrap();
    writeln!(&mut out, "  ci_success_is_precondition: true").unwrap();
    writeln!(
        &mut out,
        "  broad_production_requires_long_run_evidence: true"
    )
    .unwrap();
    writeln!(&mut out, "  missing_manifest_blocks_promotion: true").unwrap();

    out
}

fn ps_single_quoted(value: &str) -> String {
    value.replace('\'', "''")
}

fn sh_single_quoted(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn yaml_double_quoted(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

fn governance_baseline_yaml(spec: &ApiSpec, kind: ProjectKind) -> String {
    let boundary = match kind {
        ProjectKind::Rest => "rest",
        ProjectKind::Rpc => "rpc",
    };
    let endpoint_count = match kind {
        ProjectKind::Rest => spec.rest_routes.len(),
        ProjectKind::Rpc => spec.rpc_methods.len(),
    };

    format!(
        r#"# Generated by rozectl. Keep this file machine-readable for CI and platform checks.
service: {name}
boundary: {boundary}
endpoint_count: {endpoint_count}
architecture:
  borrowed_from_go_zero:
    - simple_idl_first_boundaries
    - high_availability_under_concurrency
    - failure_oriented_resilience
    - developer_friendly_generated_ownership
    - easy_extension_points
  roze_extensions:
    - generated_production_evidence_runbook
    - generated_prometheus_alert_rules
    - generated_grafana_dashboard
    - generated_slo_error_budget
    - generated_failure_injection_plan
    - generated_release_rollout_gates
    - generated_incident_response_playbook
    - generated_capacity_plan
    - generated_security_readiness_plan
    - generated_production_gate
    - generated_regeneration_policy
    - generated_client_contract
    - generated_config_governance
    - generated_reliable_events_plan
    - generated_dependency_governance
    - generated_data_consistency_plan
    - generated_observability_contract
    - generated_runtime_hardening_contract
    - generated_error_contract
    - generated_deployment_topology_contract
    - generated_service_communication_contract
    - generated_cache_governance_contract
    - generated_data_access_governance_contract
    - lifecycle_snapshot_evidence
    - failure_timeline_required
    - resource_trend_required
governance_required:
  timeout:
    required: true
    evidence: p95_p99_latency_and_deadline_propagation
  rate_limit:
    required: true
    evidence: allowed_rejected_request_counters
  circuit_breaker:
    required: true
    evidence: open_half_open_closed_transition_metrics
  load_shedding:
    required: true
    evidence: shed_request_rate_and_recovery_window
  retry_budget:
    required: true
    evidence: retry_attempts_capped_by_budget
  deadline_propagation:
    required: true
    evidence: cancellation_reaches_logic_and_dependencies
  service_discovery:
    required: true
    evidence: registry_health_and_endpoint_change_events
  load_balancing:
    required: true
    evidence: upstream_distribution_and_outlier_behavior
  tracing:
    required: true
    evidence: end_to_end_trace_query
  metrics:
    required: true
    evidence: prometheus_dashboard_and_alerts
    generated_rules: ops/prometheus-rules.yaml
    generated_dashboard: ops/grafana-dashboard.json
  structured_logs:
    required: true
    evidence: correlation_id_query
evidence_required:
  generated_services_report: true
  lifecycle_report: true
  slo_error_budget_report: true
  failure_injection_plan: true
  release_rollout_plan: true
  incident_response_playbook: true
  capacity_plan: true
  security_readiness_plan: true
  production_gate: true
  regeneration_policy: true
  client_contract: true
  config_governance: true
  reliable_events_plan: true
  dependency_governance: true
  data_consistency_plan: true
  observability_contract: true
  runtime_hardening_contract: true
  error_contract: true
  deployment_topology_contract: true
  service_communication_contract: true
  cache_governance_contract: true
  data_access_governance_contract: true
  lifecycle_summary_consistency: true
  failure_injection_timeline: true
  rollback_notes: true
"#,
        name = spec.service,
        boundary = boundary,
        endpoint_count = endpoint_count,
    )
}

fn prometheus_rules_yaml(spec: &ApiSpec, kind: ProjectKind) -> String {
    let boundary = match kind {
        ProjectKind::Rest => "rest",
        ProjectKind::Rpc => "rpc",
    };
    let latency_metric = match kind {
        ProjectKind::Rest => "roze_http_request_duration_seconds_bucket",
        ProjectKind::Rpc => "roze_rpc_method_duration_seconds_bucket",
    };
    let request_metric = match kind {
        ProjectKind::Rest => "roze_http_requests_total",
        ProjectKind::Rpc => "roze_rpc_requests_total",
    };

    format!(
        r#"# Generated by rozectl. Review metric names against your Prometheus registry before enabling.
groups:
  - name: {name}-{boundary}-production
    rules:
      - alert: RozeGeneratedServiceDown
        expr: up{{service="{name}"}} == 0
        for: 2m
        labels:
          severity: critical
          service: {name}
          boundary: {boundary}
        annotations:
          summary: "{name} is down"
          description: "No scrape target is up for generated service {name}."

      - alert: RozeGeneratedServiceHighErrorRate
        expr: |
          sum(rate({request_metric}{{service="{name}",status=~"5.."}}[5m]))
          /
          clamp_min(sum(rate({request_metric}{{service="{name}"}}[5m])), 1)
          > 0.01
        for: 10m
        labels:
          severity: warning
          service: {name}
          boundary: {boundary}
        annotations:
          summary: "{name} error rate is above 1%"
          description: "Generated service error budget is burning; inspect traces, logs, and failure timeline."

      - alert: RozeGeneratedServiceHighP99Latency
        expr: |
          histogram_quantile(
            0.99,
            sum(rate({latency_metric}{{service="{name}"}}[5m])) by (le)
          ) > 1
        for: 10m
        labels:
          severity: warning
          service: {name}
          boundary: {boundary}
        annotations:
          summary: "{name} p99 latency is above 1s"
          description: "Check timeout, load shedding, downstream latency, and retry budget evidence."

      - alert: RozeGeneratedServiceRateLimitRejecting
        expr: sum(rate(roze_resilience_decisions_total{{service="{name}",kind="rate_limit",decision="rejected"}}[5m])) > 0
        for: 5m
        labels:
          severity: info
          service: {name}
          boundary: {boundary}
        annotations:
          summary: "{name} is rejecting requests by rate limit"
          description: "Verify client behavior, configured limits, and allowed/rejected counter evidence."

      - alert: RozeGeneratedServiceCircuitBreakerOpen
        expr: sum(rate(roze_resilience_decisions_total{{service="{name}",kind="breaker",decision="open"}}[5m])) > 0
        for: 2m
        labels:
          severity: warning
          service: {name}
          boundary: {boundary}
        annotations:
          summary: "{name} circuit breaker is open"
          description: "Inspect downstream health, breaker transition metrics, and recovery objective."

      - alert: RozeGeneratedServiceLoadShedding
        expr: sum(rate(roze_resilience_decisions_total{{service="{name}",kind="load_shedding",decision="shed"}}[5m])) > 0
        for: 2m
        labels:
          severity: warning
          service: {name}
          boundary: {boundary}
        annotations:
          summary: "{name} is shedding load"
          description: "Compare resource trends, p99 latency, and configured shedding thresholds."

      - alert: RozeGeneratedServiceRestarting
        expr: increase(process_start_time_seconds{{service="{name}"}}[15m]) > 0
        for: 1m
        labels:
          severity: warning
          service: {name}
          boundary: {boundary}
        annotations:
          summary: "{name} restarted"
          description: "Record restart count and attach logs to the production evidence report."
"#,
        name = spec.service,
        boundary = boundary,
        latency_metric = latency_metric,
        request_metric = request_metric,
    )
}

fn grafana_dashboard_json(spec: &ApiSpec, kind: ProjectKind) -> String {
    let boundary = match kind {
        ProjectKind::Rest => "rest",
        ProjectKind::Rpc => "rpc",
    };
    let latency_metric = match kind {
        ProjectKind::Rest => "roze_http_request_duration_seconds_bucket",
        ProjectKind::Rpc => "roze_rpc_method_duration_seconds_bucket",
    };
    let request_metric = match kind {
        ProjectKind::Rest => "roze_http_requests_total",
        ProjectKind::Rpc => "roze_rpc_requests_total",
    };

    format!(
        r#"{{
  "title": "Roze Generated Service - {name}",
  "tags": ["roze", "generated-service", "{boundary}", "production-evidence"],
  "timezone": "browser",
  "schemaVersion": 39,
  "version": 1,
  "refresh": "30s",
  "templating": {{
    "list": [
      {{
        "name": "service",
        "type": "constant",
        "query": "{name}",
        "current": {{
          "text": "{name}",
          "value": "{name}"
        }}
      }}
    ]
  }},
  "panels": [
    {{
      "id": 1,
      "title": "Request Rate",
      "type": "timeseries",
      "gridPos": {{"x": 0, "y": 0, "w": 12, "h": 8}},
      "targets": [
        {{"expr": "sum(rate({request_metric}{{service=\"$service\"}}[5m]))", "legendFormat": "rps"}}
      ]
    }},
    {{
      "id": 2,
      "title": "Error Rate",
      "type": "timeseries",
      "gridPos": {{"x": 12, "y": 0, "w": 12, "h": 8}},
      "targets": [
        {{"expr": "sum(rate({request_metric}{{service=\"$service\",status=~\"5..\"}}[5m])) / clamp_min(sum(rate({request_metric}{{service=\"$service\"}}[5m])), 1)", "legendFormat": "5xx ratio"}}
      ]
    }},
    {{
      "id": 3,
      "title": "P99 Latency",
      "type": "timeseries",
      "gridPos": {{"x": 0, "y": 8, "w": 12, "h": 8}},
      "targets": [
        {{"expr": "histogram_quantile(0.99, sum(rate({latency_metric}{{service=\"$service\"}}[5m])) by (le))", "legendFormat": "p99"}}
      ]
    }},
    {{
      "id": 4,
      "title": "Resilience Decisions",
      "type": "timeseries",
      "gridPos": {{"x": 12, "y": 8, "w": 12, "h": 8}},
      "targets": [
        {{"expr": "sum(rate(roze_resilience_decisions_total{{service=\"$service\"}}[5m])) by (kind, decision)", "legendFormat": "{{{{kind}}}}/{{{{decision}}}}"}}
      ]
    }},
    {{
      "id": 5,
      "title": "Restarts",
      "type": "timeseries",
      "gridPos": {{"x": 0, "y": 16, "w": 12, "h": 8}},
      "targets": [
        {{"expr": "increase(process_start_time_seconds{{service=\"$service\"}}[15m])", "legendFormat": "restart"}}
      ]
    }}
  ]
}}
"#,
        name = spec.service,
        boundary = boundary,
        latency_metric = latency_metric,
        request_metric = request_metric,
    )
}

fn slo_yaml(spec: &ApiSpec, kind: ProjectKind) -> String {
    let boundary = match kind {
        ProjectKind::Rest => "rest",
        ProjectKind::Rpc => "rpc",
    };
    let latency_metric = match kind {
        ProjectKind::Rest => "roze_http_request_duration_seconds_bucket",
        ProjectKind::Rpc => "roze_rpc_method_duration_seconds_bucket",
    };
    let request_metric = match kind {
        ProjectKind::Rest => "roze_http_requests_total",
        ProjectKind::Rpc => "roze_rpc_requests_total",
    };

    format!(
        r#"# Generated by rozectl. Tune targets with the service owner before launch.
service: {name}
boundary: {boundary}
window: 30d
objectives:
  availability:
    target: 99.9
    unit: percent
    sli: up{{service="{name}"}} == 1
    evidence: uptime_ratio_and_restart_count
  success_rate:
    target: 99.0
    unit: percent
    sli: 1 - (5xx_requests / total_requests)
    prometheus: sum(rate({request_metric}{{service="{name}",status=~"5.."}}[5m])) / clamp_min(sum(rate({request_metric}{{service="{name}"}}[5m])), 1)
    evidence: error_rate_and_error_budget_burn
  latency_p99:
    target: 1
    unit: second
    sli: p99_request_latency
    prometheus: histogram_quantile(0.99, sum(rate({latency_metric}{{service="{name}"}}[5m])) by (le))
    evidence: p99_latency_trend_and_timeout_budget
  resilience_rejection_rate:
    target: 0.5
    unit: percent
    sli: rate_limit_breaker_and_shedding_rejections
    prometheus: sum(rate(roze_resilience_decisions_total{{service="{name}",decision=~"rejected|open|shed"}}[5m])) / clamp_min(sum(rate({request_metric}{{service="{name}"}}[5m])), 1)
    evidence: governance_rejection_rate_and_recovery_window
burn_rate_alerts:
  fast:
    window: 5m
    threshold: 14.4
  slow:
    window: 1h
    threshold: 6
promotion_required:
  generated_services_report: true
  lifecycle_report: true
  alert_rules_attached: true
  dashboard_attached: true
  rollback_notes: true
"#,
        name = spec.service,
        boundary = boundary,
        latency_metric = latency_metric,
        request_metric = request_metric,
    )
}

fn failure_injection_plan_yaml(spec: &ApiSpec, kind: ProjectKind) -> String {
    let boundary = match kind {
        ProjectKind::Rest => "rest",
        ProjectKind::Rpc => "rpc",
    };
    let endpoint_count = match kind {
        ProjectKind::Rest => spec.rest_routes.len(),
        ProjectKind::Rpc => spec.rpc_methods.len(),
    };
    let traffic_driver = match kind {
        ProjectKind::Rest => "representative_http_requests_and_probe_scrapes",
        ProjectKind::Rpc => "representative_rpc_calls_and_client_deadline_tests",
    };
    let shutdown_signal = match kind {
        ProjectKind::Rest => {
            "send_sigterm_or_ctrl_c_while_requests_and_probe_scrapes_are_in_flight"
        }
        ProjectKind::Rpc => "send_sigterm_or_ctrl_c_while_rpc_calls_are_in_flight",
    };
    let slow_dependency = match kind {
        ProjectKind::Rest => "delay_downstream_or_handler_path_beyond_configured_http_timeout",
        ProjectKind::Rpc => "delay_downstream_or_method_path_beyond_client_and_server_deadlines",
    };
    let dependency_5xx = match kind {
        ProjectKind::Rest => {
            "return_5xx_from_dependency_or_handler_path_until_error_rate_alert_fires"
        }
        ProjectKind::Rpc => {
            "return_unavailable_or_internal_from_dependency_until_error_rate_alert_fires"
        }
    };

    format!(
        r#"# Generated by rozectl. Run these drills in staging before broad production rollout.
service: {name}
boundary: {boundary}
endpoint_count: {endpoint_count}
traffic_driver: {traffic_driver}
objective:
  verify:
    - timeout
    - rate_limit
    - circuit_breaker
    - load_shedding
    - retry_budget
    - deadline_propagation
    - graceful_shutdown
    - readiness_draining
    - restart_recovery
scenarios:
  - scenario: shutdown_signal
    inject: {shutdown_signal}
    expected:
      - readiness_false_before_process_exit
      - in_flight_work_drained_or_cancelled_by_deadline
      - lifecycle_snapshot_reaches_stopped
    evidence:
      metrics_query: up{{service="{name}"}}
      trace_query: shutdown_and_cancelled_request_traces
      log_query: service_group_shutdown_and_draining_logs
      recovery_time: required
      rollback_notes: required

  - scenario: slow_dependency
    inject: {slow_dependency}
    expected:
      - configured_timeout_observed
      - deadline_propagation_reaches_logic_and_dependencies
      - p99_latency_alert_or_budget_entry_created
    evidence:
      metrics_query: roze_resilience_decisions_total{{service="{name}",kind=~"timeout|deadline"}}
      trace_query: slow_dependency_trace_with_deadline
      log_query: timeout_or_deadline_exceeded_logs
      recovery_time: required
      rollback_notes: required

  - scenario: dependency_5xx
    inject: {dependency_5xx}
    expected:
      - circuit_breaker_transitions_open_half_open_closed
      - error_rate_alert_fires
      - retry_budget_caps_amplification
    evidence:
      metrics_query: roze_resilience_decisions_total{{service="{name}",kind="breaker"}}
      trace_query: failing_dependency_trace_with_breaker_state
      log_query: breaker_transition_logs
      recovery_time: required
      rollback_notes: required

  - scenario: rate_limit_pressure
    inject: drive_request_rate_above_configured_limit
    expected:
      - rate_limit_rejects_excess_traffic
      - allowed_and_rejected_counters_are_visible
      - accepted_requests_remain_within_latency_budget
    evidence:
      metrics_query: roze_resilience_decisions_total{{service="{name}",kind="rate_limit"}}
      trace_query: rejected_request_trace_or_sampling_record
      log_query: rate_limit_decision_logs
      recovery_time: required
      rollback_notes: required

  - scenario: load_shedding_pressure
    inject: raise_concurrency_or_latency_until_shedding_threshold_is_crossed
    expected:
      - load_shedding_decisions_are_recorded
      - process_resource_trend_remains_bounded
      - service_recovers_without_restart_after_pressure_drops
    evidence:
      metrics_query: roze_resilience_decisions_total{{service="{name}",kind="load_shedding"}}
      trace_query: shed_request_trace_or_sampling_record
      log_query: load_shedding_decision_logs
      recovery_time: required
      rollback_notes: required

  - scenario: invalid_config_reload
    inject: push_invalid_governance_or_dependency_config
    expected:
      - invalid_config_is_rejected
      - last_known_good_config_remains_active
      - operator_action_is_auditable
    evidence:
      metrics_query: roze_config_reload_total{{service="{name}",result=~"rejected|rollback"}}
      trace_query: config_reload_trace_if_available
      log_query: invalid_config_reload_logs
      recovery_time: required
      rollback_notes: required

  - scenario: restart_recovery
    inject: restart_one_service_instance_under_representative_traffic
    expected:
      - startup_probe_returns_ready_after_dependencies_pass
      - readiness_returns_after_recovery
      - restart_count_is_recorded_in_evidence_report
    evidence:
      metrics_query: increase(process_start_time_seconds{{service="{name}"}}[15m])
      trace_query: startup_and_first_successful_request_trace
      log_query: startup_readiness_and_dependency_check_logs
      recovery_time: required
      rollback_notes: required
promotion_required:
  all_scenarios_executed: true
  recovery_time_recorded: true
  metrics_traces_logs_attached: true
  rollback_notes_attached: true
"#,
        name = spec.service,
        boundary = boundary,
        endpoint_count = endpoint_count,
        traffic_driver = traffic_driver,
        shutdown_signal = shutdown_signal,
        slow_dependency = slow_dependency,
        dependency_5xx = dependency_5xx,
    )
}

fn release_rollout_yaml(spec: &ApiSpec, kind: ProjectKind) -> String {
    let boundary = match kind {
        ProjectKind::Rest => "rest",
        ProjectKind::Rpc => "rpc",
    };
    let endpoint_count = match kind {
        ProjectKind::Rest => spec.rest_routes.len(),
        ProjectKind::Rpc => spec.rpc_methods.len(),
    };
    let request_metric = match kind {
        ProjectKind::Rest => "roze_http_requests_total",
        ProjectKind::Rpc => "roze_rpc_requests_total",
    };
    let latency_metric = match kind {
        ProjectKind::Rest => "roze_http_request_duration_seconds_bucket",
        ProjectKind::Rpc => "roze_rpc_method_duration_seconds_bucket",
    };
    let smoke_probe = match kind {
        ProjectKind::Rest => "healthz_readyz_startupz_metrics_and_representative_http_request",
        ProjectKind::Rpc => "startup_readiness_metric_scrape_and_representative_rpc_call",
    };

    format!(
        r#"# Generated by rozectl. Keep release evidence with the production report.
service: {name}
boundary: {boundary}
endpoint_count: {endpoint_count}
strategy:
  default: canary_then_progressive
  alternatives:
    - blue_green
    - manual_rollback
preflight_required:
  generated_services_report: true
  lifecycle_report: true
  slo_error_budget_report: true
  failure_injection_plan_completed: true
  alert_rules_attached: true
  dashboard_attached: true
  rollback_owner_named: true
  config_diff_reviewed: true
  migration_plan_reviewed: true
gates:
  - gate: preflight
    action: run_compile_tests_smoke_and_config_diff
    smoke_probe: {smoke_probe}
    pass:
      - cargo_fmt_check_passed
      - cargo_test_passed
      - smoke_probe_passed
      - no_unreviewed_config_or_schema_diff
    evidence:
      metrics_query: up{{service="{name}"}}
      trace_query: representative_success_trace
      log_query: startup_and_config_load_logs

  - gate: canary_1_percent
    action: route_1_percent_or_single_instance_to_new_version
    hold: 15m
    pass:
      - availability_slo_not_burning_fast
      - error_rate_below_1_percent
      - p99_latency_within_slo
      - no_new_panic_or_unhandled_error_logs
    rollback:
      automatic_when:
        - error_rate_above_1_percent_for_5m
        - p99_latency_above_slo_for_5m
        - circuit_breaker_open_for_2m
      manual_when:
        - business_metric_regression_observed
        - operator_detects_unexpected_dependency_pressure
    evidence:
      metrics_query: sum(rate({request_metric}{{service="{name}"}}[5m]))
      latency_query: histogram_quantile(0.99, sum(rate({latency_metric}{{service="{name}"}}[5m])) by (le))
      trace_query: canary_success_and_error_trace_samples
      log_query: canary_version_logs_with_correlation_id

  - gate: canary_10_percent
    action: increase_new_version_traffic_to_10_percent
    hold: 30m
    pass:
      - burn_rate_within_slo_yaml_thresholds
      - resilience_rejection_rate_within_budget
      - resource_trends_stable
      - dependency_error_rate_not_amplified
    rollback:
      automatic_when:
        - slo_fast_burn_threshold_exceeded
        - load_shedding_sustained_for_5m
        - retry_budget_exhausted
    evidence:
      metrics_query: roze_resilience_decisions_total{{service="{name}"}}
      trace_query: dependency_and_retry_trace_samples
      log_query: rate_limit_breaker_shedding_logs

  - gate: progressive_50_percent
    action: increase_new_version_traffic_to_50_percent
    hold: 1h
    pass:
      - p95_and_p99_latency_trends_stable
      - process_memory_and_connection_trends_stable
      - no_restart_loop
      - readiness_remains_true
    rollback:
      automatic_when:
        - process_restarts_within_15m
        - readiness_false_for_2m
        - resource_trend_unbounded
    evidence:
      metrics_query: process_resident_memory_bytes{{service="{name}"}}
      trace_query: high_latency_trace_samples
      log_query: readiness_and_restart_logs

  - gate: full_rollout
    action: shift_100_percent_traffic_to_new_version
    hold: 2h
    pass:
      - availability_success_latency_and_resilience_slos_stable
      - alerts_clear_or_acknowledged
      - rollback_window_still_available
      - owner_signoff_recorded
    rollback:
      automatic_when:
        - critical_alert_fires
        - error_budget_fast_burn_detected
      manual_when:
        - product_or_customer_signal_regression
    evidence:
      metrics_query: up{{service="{name}"}}
      trace_query: full_rollout_trace_samples
      log_query: full_rollout_version_logs

  - gate: post_release_observation
    action: observe_new_version_after_full_rollout
    hold: 24h
    pass:
      - slow_burn_rate_within_budget
      - no_delayed_dependency_or_resource_regression
      - incident_and_rollback_notes_attached
    evidence:
      metrics_query: sum(rate({request_metric}{{service="{name}"}}[1h]))
      latency_query: histogram_quantile(0.99, sum(rate({latency_metric}{{service="{name}"}}[1h])) by (le))
      trace_query: post_release_trace_samples
      log_query: post_release_error_and_warning_logs
blue_green:
  required:
    - old_and_new_versions_registered_separately
    - readiness_checked_before_switch
    - instant_switch_command_recorded
    - rollback_switch_command_recorded
rollback_required:
  owner: required
  max_decision_time: 5m
  evidence:
    - rollback_reason
    - rollback_command_or_platform_event
    - recovery_time
    - customer_impact
    - followup_issue
"#,
        name = spec.service,
        boundary = boundary,
        endpoint_count = endpoint_count,
        request_metric = request_metric,
        latency_metric = latency_metric,
        smoke_probe = smoke_probe,
    )
}

fn incident_response_yaml(spec: &ApiSpec, kind: ProjectKind) -> String {
    let boundary = match kind {
        ProjectKind::Rest => "rest",
        ProjectKind::Rpc => "rpc",
    };
    let endpoint_count = match kind {
        ProjectKind::Rest => spec.rest_routes.len(),
        ProjectKind::Rpc => spec.rpc_methods.len(),
    };
    let request_metric = match kind {
        ProjectKind::Rest => "roze_http_requests_total",
        ProjectKind::Rpc => "roze_rpc_requests_total",
    };
    let latency_metric = match kind {
        ProjectKind::Rest => "roze_http_request_duration_seconds_bucket",
        ProjectKind::Rpc => "roze_rpc_method_duration_seconds_bucket",
    };
    let user_probe = match kind {
        ProjectKind::Rest => "representative_http_request_and_health_probes",
        ProjectKind::Rpc => "representative_rpc_call_and_client_deadline_probe",
    };

    format!(
        r#"# Generated by rozectl. Fill owner and channel before broad production rollout.
service: {name}
boundary: {boundary}
endpoint_count: {endpoint_count}
owner:
  primary: required
  secondary: required
  escalation_channel: required
  customer_comms_channel: required
linked_assets:
  evidence_runbook: ops/production-evidence.md
  governance_baseline: ops/governance-baseline.yaml
  alert_rules: ops/prometheus-rules.yaml
  dashboard: ops/grafana-dashboard.json
  slo: ops/slo.yaml
  failure_injection: ops/failure-injection-plan.yaml
  release_rollout: ops/release-rollout.yaml
severity:
  sev1:
    page: immediate
    examples:
      - service_down
      - broad_5xx_regression
      - failed_rollout_without_automatic_recovery
    decision_window: 5m
  sev2:
    page: immediate
    examples:
      - sustained_p99_latency_regression
      - circuit_breaker_open
      - load_shedding_sustained
    decision_window: 15m
  sev3:
    page: business_hours
    examples:
      - slow_error_budget_burn
      - isolated_dependency_degradation
      - config_reload_rejected
    decision_window: 1h
response_matrix:
  - alert: RozeGeneratedServiceDown
    severity: sev1
    confirm:
      metrics_query: up{{service="{name}"}}
      user_probe: {user_probe}
      log_query: startup_readiness_shutdown_and_panic_logs
      trace_query: first_failing_request_or_rpc_trace
    mitigate:
      - stop_current_rollout
      - check_readiness_and_registry_membership
      - rollback_to_last_known_good_version_if_new_release
      - fail_traffic_to_healthy_instances_or_region
    rollback_when:
      - outage_started_after_release_or_config_change
      - readiness_does_not_recover_within_5m
    evidence_required:
      - incident_start_time
      - detection_source
      - rollback_or_failover_action
      - recovery_time
      - affected_endpoints_or_methods

  - alert: RozeGeneratedServiceHighErrorRate
    severity: sev1
    confirm:
      metrics_query: sum(rate({request_metric}{{service="{name}",status=~"5.."}}[5m])) / clamp_min(sum(rate({request_metric}{{service="{name}"}}[5m])), 1)
      log_query: error_logs_grouped_by_code_and_dependency
      trace_query: top_error_traces_with_correlation_id
    mitigate:
      - identify_new_release_config_or_dependency_change
      - open_or_tighten_circuit_breaker_for_failing_dependency
      - reduce_canary_or_rollback_if_release_related
      - disable_optional_feature_or_route_if_available
    rollback_when:
      - error_rate_above_1_percent_for_5m
      - error_budget_fast_burn_detected
    evidence_required:
      - top_error_classes
      - dependency_status
      - breaker_state
      - retry_budget_usage
      - recovery_time

  - alert: RozeGeneratedServiceHighP99Latency
    severity: sev2
    confirm:
      metrics_query: histogram_quantile(0.99, sum(rate({latency_metric}{{service="{name}"}}[5m])) by (le))
      log_query: slow_request_or_rpc_logs
      trace_query: p99_trace_samples
    mitigate:
      - check_downstream_latency_and_connection_pools
      - verify_timeout_budget_and_deadline_propagation
      - enable_or_tighten_load_shedding_if_saturation_detected
      - rollback_if_latency_regression_started_after_release
    rollback_when:
      - p99_latency_above_slo_for_10m_after_release
      - saturation_continues_after_shedding
    evidence_required:
      - p95_p99_before_during_after
      - slowest_dependencies
      - timeout_configuration
      - resource_trend

  - alert: RozeGeneratedServiceCircuitBreakerOpen
    severity: sev2
    confirm:
      metrics_query: roze_resilience_decisions_total{{service="{name}",kind="breaker"}}
      log_query: breaker_transition_logs
      trace_query: downstream_failure_trace_samples
    mitigate:
      - verify_downstream_health
      - keep_breaker_open_if_dependency_is_failing
      - reduce_retry_pressure
      - route_to_healthy_dependency_pool_if_available
    rollback_when:
      - breaker_open_started_after_release
      - fallback_path_is_missing_or_failing
    evidence_required:
      - breaker_open_half_open_closed_timeline
      - downstream_owner_contacted
      - fallback_result
      - recovery_time

  - alert: RozeGeneratedServiceLoadShedding
    severity: sev2
    confirm:
      metrics_query: roze_resilience_decisions_total{{service="{name}",kind="load_shedding"}}
      log_query: load_shedding_decision_logs
      trace_query: shed_and_accepted_request_trace_samples
    mitigate:
      - inspect_cpu_memory_connections_and_queue_depth
      - reduce_traffic_or_canary_weight
      - shed_optional_work_before_core_paths
      - rollback_if_new_version_increased_resource_cost
    rollback_when:
      - shedding_sustained_for_5m_after_release
      - accepted_requests_miss_latency_slo
    evidence_required:
      - shed_rate
      - accepted_latency
      - resource_trend
      - traffic_level

  - alert: RozeGeneratedServiceRestarting
    severity: sev1
    confirm:
      metrics_query: increase(process_start_time_seconds{{service="{name}"}}[15m])
      log_query: panic_oom_startup_and_shutdown_logs
      trace_query: last_successful_trace_before_restart
    mitigate:
      - freeze_rollout
      - inspect_crash_or_oom_reason
      - rollback_release_or_config_if_restart_started_after_change
      - reduce_traffic_until_stable
    rollback_when:
      - more_than_one_restart_in_15m_after_release
      - startup_probe_fails_after_restart
    evidence_required:
      - restart_count
      - crash_reason
      - memory_trend
      - rollback_action

  - alert: ConfigReloadRejectedOrRolledBack
    severity: sev3
    confirm:
      metrics_query: roze_config_reload_total{{service="{name}",result=~"rejected|rollback"}}
      log_query: config_reload_validation_and_audit_logs
      trace_query: config_reload_trace_if_available
    mitigate:
      - keep_last_known_good_config
      - identify_invalid_field_and_operator
      - rerun_config_validation_before_retry
      - attach_config_diff_to_evidence
    rollback_when:
      - invalid_config_was_applied_to_any_instance
      - service_behavior_changed_after_config_push
    evidence_required:
      - config_version
      - rejected_fields
      - operator
      - last_known_good_version
postmortem_required:
  timeline: true
  customer_impact: true
  metrics_traces_logs: true
  rollback_or_mitigation_result: true
  followup_owner: true
  prevention_change: true
"#,
        name = spec.service,
        boundary = boundary,
        endpoint_count = endpoint_count,
        request_metric = request_metric,
        latency_metric = latency_metric,
        user_probe = user_probe,
    )
}

fn capacity_plan_yaml(spec: &ApiSpec, kind: ProjectKind) -> String {
    let boundary = match kind {
        ProjectKind::Rest => "rest",
        ProjectKind::Rpc => "rpc",
    };
    let endpoint_count = match kind {
        ProjectKind::Rest => spec.rest_routes.len(),
        ProjectKind::Rpc => spec.rpc_methods.len(),
    };
    let request_metric = match kind {
        ProjectKind::Rest => "roze_http_requests_total",
        ProjectKind::Rpc => "roze_rpc_requests_total",
    };
    let latency_metric = match kind {
        ProjectKind::Rest => "roze_http_request_duration_seconds_bucket",
        ProjectKind::Rpc => "roze_rpc_method_duration_seconds_bucket",
    };
    let traffic_unit = match kind {
        ProjectKind::Rest => "requests_per_second",
        ProjectKind::Rpc => "rpc_calls_per_second",
    };
    let driver = match kind {
        ProjectKind::Rest => "representative_http_workload_with_health_and_metrics_scrapes",
        ProjectKind::Rpc => "representative_rpc_workload_with_client_deadlines",
    };

    format!(
        r#"# Generated by rozectl. Replace placeholder targets with service-owner approved values.
service: {name}
boundary: {boundary}
endpoint_count: {endpoint_count}
traffic_unit: {traffic_unit}
traffic_driver: {driver}
objectives:
  prove_sustained_capacity: true
  prove_burst_tolerance: true
  prove_resource_trend_stability: true
  prove_scaling_behavior: true
  prove_governance_under_pressure: true
targets:
  baseline_rps: required
  peak_rps: required
  burst_multiplier: 2
  soak_duration_minimum: 24h
  extended_soak_duration: 72h
  max_error_rate_percent: 1
  max_p99_latency_seconds: 1
  max_resilience_rejection_rate_percent: 0.5
  cpu_saturation_threshold_percent: 75
  memory_growth_threshold_percent_per_hour: 2
  restart_tolerance: 0
measurements:
  request_rate: sum(rate({request_metric}{{service="{name}"}}[5m]))
  error_rate: sum(rate({request_metric}{{service="{name}",status=~"5.."}}[5m])) / clamp_min(sum(rate({request_metric}{{service="{name}"}}[5m])), 1)
  p99_latency: histogram_quantile(0.99, sum(rate({latency_metric}{{service="{name}"}}[5m])) by (le))
  resilience_decisions: sum(rate(roze_resilience_decisions_total{{service="{name}"}}[5m])) by (kind, decision)
  restarts: increase(process_start_time_seconds{{service="{name}"}}[15m])
  memory: process_resident_memory_bytes{{service="{name}"}}
  cpu: rate(process_cpu_seconds_total{{service="{name}"}}[5m])
plans:
  - phase: baseline_characterization
    duration: 30m
    load: baseline_rps
    pass:
      - p99_latency_within_target
      - error_rate_within_target
      - no_restarts
      - no_unexpected_governance_rejections
    evidence:
      - request_rate_graph
      - p95_p99_latency_graph
      - error_rate_graph
      - representative_traces

  - phase: step_load
    duration: 2h
    load_steps:
      - 25_percent_peak
      - 50_percent_peak
      - 75_percent_peak
      - 100_percent_peak
    hold_each: 30m
    pass:
      - latency_curve_has_no_cliff
      - resource_curve_is_bounded
      - retry_budget_not_exhausted
      - breaker_and_shedding_decisions_match_thresholds
    evidence:
      - per_step_capacity_table
      - resource_trend_graph
      - resilience_decision_graph
      - dependency_latency_graph

  - phase: burst
    duration: 30m
    load: peak_rps_times_burst_multiplier
    pass:
      - load_shedding_or_rate_limit_protects_core_paths
      - accepted_traffic_stays_within_latency_target
      - recovery_after_burst_within_5m
    evidence:
      - burst_start_and_end_time
      - shed_or_rejected_rate
      - recovery_time
      - accepted_request_latency

  - phase: soak_24h
    duration: 24h
    load: approved_sustained_peak
    pass:
      - no_memory_leak_trend
      - no_connection_or_file_descriptor_growth
      - no_restart
      - slow_burn_rate_within_slo
    evidence:
      - memory_growth_percent_per_hour
      - cpu_trend
      - connection_trend
      - error_budget_burn
      - lifecycle_summary

  - phase: soak_72h
    duration: 72h
    load: approved_sustained_peak
    required_for: broad_production_stable_claim
    pass:
      - all_24h_soak_gates_remain_true
      - resource_trends_stay_bounded_across_day_boundaries
      - dependency_error_rates_do_not_amplify
      - operational_alert_noise_is_acceptable
    evidence:
      - hourly_resource_table
      - daily_latency_summary
      - dependency_health_summary
      - alert_noise_summary

  - phase: scale_out
    duration: 1h
    action: add_instances_or_increase_worker_capacity_under_load
    pass:
      - traffic_distribution_rebalances
      - p99_latency_improves_or_stabilizes
      - no_registry_or_readiness_flapping
    evidence:
      - instance_count_timeline
      - per_instance_request_rate
      - registry_change_events

  - phase: scale_in
    duration: 1h
    action: remove_instances_or_reduce_worker_capacity_under_load
    pass:
      - draining_completes_before_capacity_removal
      - no_request_loss_beyond_slo
      - readiness_turns_false_before_instance_exit
    evidence:
      - drain_timeline
      - readiness_timeline
      - error_rate_during_scale_in
promotion_required:
  baseline_characterization_passed: true
  step_load_passed: true
  burst_passed: true
  soak_24h_passed: true
  scale_out_passed: true
  scale_in_passed: true
  soak_72h_passed_for_broad_stability_claim: true
  owner_signoff: required
"#,
        name = spec.service,
        boundary = boundary,
        endpoint_count = endpoint_count,
        traffic_unit = traffic_unit,
        driver = driver,
        request_metric = request_metric,
        latency_metric = latency_metric,
    )
}

fn security_readiness_yaml(spec: &ApiSpec, kind: ProjectKind) -> String {
    let boundary = match kind {
        ProjectKind::Rest => "rest",
        ProjectKind::Rpc => "rpc",
    };
    let endpoint_count = match kind {
        ProjectKind::Rest => spec.rest_routes.len(),
        ProjectKind::Rpc => spec.rpc_methods.len(),
    };
    let auth_probe = match kind {
        ProjectKind::Rest => {
            "representative_http_request_with_valid_expired_missing_and_malformed_tokens"
        }
        ProjectKind::Rpc => {
            "representative_rpc_call_with_valid_expired_missing_and_malformed_credentials"
        }
    };
    let boundary_policy = match kind {
        ProjectKind::Rest => "route_method_path_and_openapi_security_projection",
        ProjectKind::Rpc => "rpc_method_and_proto_service_security_projection",
    };

    format!(
        r#"# Generated by rozectl. Fill concrete providers and owners before broad production rollout.
service: {name}
boundary: {boundary}
endpoint_count: {endpoint_count}
security_model:
  identity_provider: required
  token_format: jwt_or_oidc_required
  transport_security: tls_required
  service_to_service_security: mtls_required_for_internal_production
  tenant_isolation: required_when_multi_tenant
  audit_log: required_for_authz_and_operator_actions
  secret_storage: external_secret_manager_required
  key_rotation: required
policy_projection:
  boundary_policy: {boundary_policy}
  generated_contracts_must_include:
    - authenticated_operations
    - public_operations
    - required_scopes_or_roles
    - tenant_context_requirement
    - error_response_shape
readiness_checks:
  - check: authentication
    probe: {auth_probe}
    pass:
      - valid_credential_is_accepted
      - missing_credential_is_rejected
      - expired_credential_is_rejected
      - malformed_credential_is_rejected
      - auth_failure_uses_stable_error_model
    evidence:
      metrics_query: roze_auth_decisions_total{{service="{name}"}}
      trace_query: auth_success_and_failure_trace_samples
      log_query: auth_decision_logs_without_sensitive_token_material

  - check: authorization
    probe: allowed_denied_scope_role_and_abac_cases
    pass:
      - allowed_principal_can_access_expected_operation
      - denied_principal_receives_forbidden
      - role_scope_and_abac_denials_are_distinguishable_in_audit
      - generated_sdk_or_openapi_metadata_reflects_required_policy
    evidence:
      metrics_query: roze_authz_decisions_total{{service="{name}"}}
      trace_query: authorization_denial_trace_samples
      log_query: authorization_audit_logs

  - check: tenant_isolation
    probe: cross_tenant_read_write_and_list_attempts
    pass:
      - tenant_context_is_required
      - cross_tenant_read_is_denied
      - cross_tenant_write_is_denied
      - list_queries_are_tenant_scoped
      - cache_keys_include_tenant_scope_when_needed
    evidence:
      metrics_query: roze_tenant_isolation_decisions_total{{service="{name}"}}
      trace_query: cross_tenant_denial_trace_samples
      log_query: tenant_isolation_audit_logs

  - check: key_rotation
    probe: old_new_and_revoked_signing_keys_during_rotation_window
    pass:
      - new_key_is_accepted
      - old_key_is_accepted_only_inside_grace_window
      - revoked_key_is_rejected
      - jwks_or_key_cache_refresh_is_observed
    evidence:
      metrics_query: roze_key_rotation_total{{service="{name}"}}
      trace_query: key_rotation_auth_trace_samples
      log_query: key_rotation_and_cache_refresh_logs

  - check: mtls
    probe: valid_client_certificate_missing_certificate_and_wrong_spiffe_identity
    pass:
      - valid_service_identity_is_accepted
      - missing_client_certificate_is_rejected
      - wrong_service_identity_is_rejected
      - certificate_expiry_alert_is_configured
    evidence:
      metrics_query: roze_mtls_handshake_total{{service="{name}"}}
      trace_query: mtls_denial_trace_samples
      log_query: mtls_identity_logs_without_private_key_material

  - check: audit_log
    probe: authz_denial_sensitive_read_sensitive_write_and_operator_action
    pass:
      - actor_subject_action_resource_tenant_and_result_are_recorded
      - correlation_id_is_present
      - sensitive_values_are_redacted
      - audit_log_sink_failure_is_reported
    evidence:
      metrics_query: roze_audit_events_total{{service="{name}"}}
      trace_query: audit_event_trace_correlation_samples
      log_query: audit_log_query_by_correlation_id

  - check: sensitive_data
    probe: validation_error_debug_log_trace_and_metric_label_paths
    pass:
      - secrets_tokens_and_passwords_never_appear_in_logs
      - sensitive_fields_are_redacted_in_debug_output
      - metric_labels_do_not_contain_pii_or_secret_material
      - error_responses_do_not_leak_internal_dependency_details
    evidence:
      metrics_query: roze_sensitive_data_scan_total{{service="{name}",result="pass"}}
      trace_query: redaction_validation_trace_samples
      log_query: redaction_scan_report

  - check: dependency_security
    probe: outbound_dependency_identity_timeout_and_scope_review
    pass:
      - dependency_credentials_are_not_in_config_yaml_plaintext
      - outbound_calls_have_deadline_and_identity
      - least_privilege_scope_is_documented
      - dependency_rotation_plan_exists
    evidence:
      metrics_query: roze_dependency_security_checks_total{{service="{name}"}}
      trace_query: outbound_dependency_trace_samples
      log_query: dependency_security_review_logs
promotion_required:
  authentication_passed: true
  authorization_passed: true
  tenant_isolation_passed: true
  key_rotation_passed: true
  mtls_plan_attached: true
  audit_log_passed: true
  sensitive_data_scan_passed: true
  dependency_security_review_passed: true
  security_owner_signoff: required
blocking_findings:
  - plaintext_secret_in_repo_or_config
  - missing_auth_on_non_public_operation
  - cross_tenant_access_allowed
  - revoked_key_accepted
  - sensitive_token_or_secret_in_logs
  - missing_audit_for_privileged_action
"#,
        name = spec.service,
        boundary = boundary,
        endpoint_count = endpoint_count,
        auth_probe = auth_probe,
        boundary_policy = boundary_policy,
    )
}

fn production_gate_yaml(spec: &ApiSpec, kind: ProjectKind) -> String {
    let boundary = match kind {
        ProjectKind::Rest => "rest",
        ProjectKind::Rpc => "rpc",
    };
    let endpoint_count = match kind {
        ProjectKind::Rest => spec.rest_routes.len(),
        ProjectKind::Rpc => spec.rpc_methods.len(),
    };
    let compile_probe = match kind {
        ProjectKind::Rest => "cargo_check_generated_rest_service",
        ProjectKind::Rpc => "cargo_check_generated_rpc_service",
    };
    let runtime_probe = match kind {
        ProjectKind::Rest => "healthz_readyz_startupz_metrics_and_representative_http_request",
        ProjectKind::Rpc => "startup_readiness_metrics_and_representative_rpc_call",
    };

    format!(
        r#"# Generated by rozectl. Wire this file into CI or your deployment platform.
service: {name}
boundary: {boundary}
endpoint_count: {endpoint_count}
mode: production_promotion_gate
assets:
  production_evidence: ops/production-evidence.md
  governance_baseline: ops/governance-baseline.yaml
  prometheus_rules: ops/prometheus-rules.yaml
  grafana_dashboard: ops/grafana-dashboard.json
  slo: ops/slo.yaml
  failure_injection: ops/failure-injection-plan.yaml
  release_rollout: ops/release-rollout.yaml
  incident_response: ops/incident-response.yaml
  capacity_plan: ops/capacity-plan.yaml
  security_readiness: ops/security-readiness.yaml
  regeneration_policy: ops/regeneration-policy.yaml
  client_contract: ops/client-contract.yaml
  config_governance: ops/config-governance.yaml
  reliable_events: ops/reliable-events.yaml
  dependency_governance: ops/dependency-governance.yaml
  data_consistency: ops/data-consistency.yaml
  observability_contract: ops/observability-contract.yaml
  runtime_hardening: ops/runtime-hardening.yaml
  error_contract: ops/error-contract.yaml
  deployment_topology: ops/deployment-topology.yaml
  service_communication: ops/service-communication.yaml
  cache_governance: ops/cache-governance.yaml
  data_access_governance: ops/data-access-governance.yaml
  interface_governance: ops/interface-governance.yaml
required_stages:
  - stage: generated_code_integrity
    required: true
    source: ops/regeneration-policy.yaml
    checks:
      - cargo_fmt_check
      - cargo_test
      - {compile_probe}
      - generated_owned_files_are_regenerated_from_idl
      - application_owned_files_are_preserved_on_update
      - idl_drift_classified
      - breaking_changes_have_owner_signoff
    evidence:
      - ci_run_url
      - generated_diff_summary
      - idl_commit

  - stage: runtime_smoke
    required: true
    checks:
      - {runtime_probe}
      - config_loads_from_expected_location
      - metrics_are_scrapeable
      - tracing_correlation_id_present
      - structured_logs_have_service_boundary_and_version
    evidence:
      - smoke_command
      - smoke_output
      - metrics_sample
      - trace_sample

  - stage: observability_contract
    required: true
    source: ops/observability-contract.yaml
    checks:
      - required_metrics_exported
      - structured_logs_have_required_fields
      - trace_propagation_verified
      - profile_or_runtime_diagnostics_defined
      - label_cardinality_budget_defined
      - debug_queries_documented
    evidence:
      - metrics_sample
      - log_query_sample
      - trace_query_sample
      - dashboard_link
      - profile_or_diagnostics_sample

  - stage: runtime_hardening
    required: true
    source: ops/runtime-hardening.yaml
    checks:
      - timeout_deadline_and_cancellation_verified
      - rate_limit_and_backpressure_verified
      - circuit_breaker_state_transitions_verified
      - load_shedding_threshold_verified
      - retry_budget_caps_amplification
      - graceful_shutdown_and_drain_verified
      - resource_guardrails_have_alerts
    evidence:
      - resilience_metric_sample
      - deadline_trace_sample
      - breaker_transition_sample
      - shedding_decision_sample
      - shutdown_timeline
      - resource_trend_sample

  - stage: error_contract
    required: true
    source: ops/error-contract.yaml
    checks:
      - typed_error_catalog_defined
      - transport_status_mapping_verified
      - retryability_and_idempotency_flags_defined
      - trace_id_and_request_id_returned_or_logged
      - sensitive_error_details_redacted
      - client_behavior_documented
      - failure_metrics_are_bounded
    evidence:
      - error_catalog_review
      - representative_error_response
      - grpc_status_or_http_status_sample
      - client_retry_behavior_report
      - redaction_scan_result
      - error_metric_sample

  - stage: deployment_topology
    required: true
    source: ops/deployment-topology.yaml
    checks:
      - startup_readiness_liveness_probes_defined
      - resource_requests_limits_and_alerts_defined
      - hpa_or_scale_policy_defined
      - disruption_budget_defined
      - config_and_secret_mounts_reviewed
      - registry_and_network_policy_defined
      - rollback_and_image_pinning_defined
    evidence:
      - rendered_manifest_or_platform_spec
      - probe_output
      - resource_limit_review
      - scaling_policy_review
      - disruption_test_result
      - rollback_result

  - stage: service_communication
    required: true
    source: ops/service-communication.yaml
    checks:
      - discovery_endpoint_change_verified
      - load_balancing_distribution_verified
      - client_deadline_and_cancellation_verified
      - retry_budget_and_backoff_verified
      - circuit_breaker_and_outlier_policy_verified
      - fallback_or_no_fallback_declared
      - trace_context_propagated_across_service_call
    evidence:
      - registry_change_test
      - load_balancing_sample
      - deadline_trace_sample
      - retry_budget_metric_sample
      - breaker_transition_sample
      - fallback_test_result

  - stage: cache_governance
    required: true
    source: ops/cache-governance.yaml
    checks:
      - cache_boundary_declared_or_no_cache_declared
      - cache_keys_ttl_and_ownership_reviewed
      - penetration_breakdown_avalanche_protection_defined
      - invalidation_and_consistency_policy_defined
      - singleflight_or_request_collapse_verified
      - cache_metrics_and_alerts_defined
    evidence:
      - cache_policy_review
      - key_ttl_table
      - miss_storm_test_result
      - invalidation_test_result
      - cache_metric_sample

  - stage: data_access_governance
    required: true
    source: ops/data-access-governance.yaml
    checks:
      - persistence_boundary_declared_or_no_persistence_declared
      - query_deadlines_and_pool_limits_defined
      - slow_query_budget_and_index_review_defined
      - pagination_and_result_size_limits_defined
      - n_plus_one_and_unbounded_scan_protection_defined
      - read_write_split_or_single_primary_policy_defined
      - data_access_metrics_and_alerts_defined
    evidence:
      - data_access_policy_review
      - query_budget_table
      - slow_query_report
      - pool_saturation_sample
      - index_review
      - data_access_metric_sample

  - stage: interface_governance
    required: true
    source: ops/interface-governance.yaml
    checks:
      - generated_framework_interfaces_documented
      - business_interfaces_projected_from_idl
      - openapi_or_proto_contains_declared_interfaces
      - framework_smoke_tests_cover_generated_interfaces
      - typed_error_and_auth_boundaries_reviewed
      - observability_labels_are_bounded
    evidence:
      - interface_governance_review
      - openapi_or_proto_artifact
      - contract_diff
      - framework_smoke_output
      - auth_and_error_mapping_review

  - stage: client_contract
    required: true
    source: ops/client-contract.yaml
    checks:
      - contract_projection_generated
      - typed_errors_documented
      - timeout_and_retry_budget_exposed_to_clients
      - auth_injection_defined
      - trace_and_correlation_propagation_defined
      - sdk_smoke_tests_passed
    evidence:
      - openapi_or_proto_artifact
      - sdk_generation_output
      - typed_error_table
      - client_smoke_report

  - stage: governance
    required: true
    source: ops/governance-baseline.yaml
    checks:
      - timeout_configured
      - rate_limit_configured
      - circuit_breaker_configured
      - load_shedding_configured
      - retry_budget_configured
      - deadline_propagation_verified
      - discovery_load_balancing_tracing_metrics_verified
    evidence:
      - governance_review
      - resilience_metric_sample
      - dashboard_link

  - stage: config_governance
    required: true
    source: ops/config-governance.yaml
    checks:
      - config_schema_validated
      - config_diff_reviewed
      - config_audit_recorded
      - canary_config_rollout_defined
      - invalid_config_rejected_or_rolled_back
      - config_snapshot_restore_tested
    evidence:
      - config_diff
      - config_version
      - audit_record
      - rollback_result
      - snapshot_restore_result

  - stage: reliable_events
    required: true
    source: ops/reliable-events.yaml
    checks:
      - event_envelope_defined_or_declared_not_used
      - idempotency_key_policy_defined
      - outbox_inbox_policy_defined
      - dlq_replay_and_purge_defined
      - retry_budget_and_storm_protection_defined
      - lag_metrics_and_alerts_defined
    evidence:
      - event_contract_review
      - idempotency_test_report
      - dlq_replay_test_result
      - retry_budget_metric_sample
      - lag_metric_sample

  - stage: dependency_governance
    required: true
    source: ops/dependency-governance.yaml
    checks:
      - downstream_inventory_declared
      - service_discovery_or_static_endpoint_policy_defined
      - load_balancing_policy_defined
      - timeout_deadline_and_cancellation_verified
      - circuit_breaker_and_bulkhead_verified
      - fallback_or_degradation_path_defined
    evidence:
      - dependency_inventory
      - registry_change_test
      - load_balancing_distribution_sample
      - breaker_transition_sample
      - fallback_test_result

  - stage: data_consistency
    required: true
    source: ops/data-consistency.yaml
    checks:
      - persistence_boundary_declared
      - migrations_have_forward_and_rollback_plan
      - idempotent_write_policy_defined
      - transaction_outbox_or_dtm_policy_defined
      - backup_restore_tested
      - data_reconciliation_defined
    evidence:
      - migration_plan
      - idempotency_test_report
      - consistency_failure_drill
      - backup_restore_result
      - reconciliation_report

  - stage: reliability_evidence
    required: true
    checks:
      - generated_services_report_complete
      - lifecycle_report_complete
      - failure_injection_plan_complete
      - incident_response_playbook_reviewed
      - rollback_notes_present
    evidence:
      - production_evidence_report
      - lifecycle_summary
      - failure_timeline
      - incident_tabletop_result

  - stage: release_control
    required: true
    source: ops/release-rollout.yaml
    checks:
      - preflight_passed
      - canary_gate_defined
      - progressive_rollout_gate_defined
      - blue_green_or_manual_rollback_defined
      - rollback_owner_named
    evidence:
      - rollout_plan
      - rollback_command_or_platform_action
      - owner_signoff

  - stage: capacity_and_soak
    required: true
    source: ops/capacity-plan.yaml
    checks:
      - baseline_characterization_passed
      - step_load_passed
      - burst_passed
      - soak_24h_passed
      - scale_out_passed
      - scale_in_passed
    broad_stability_claim_requires:
      - soak_72h_passed
    evidence:
      - load_test_report
      - resource_trend_report
      - capacity_table
      - scaling_timeline

  - stage: security_readiness
    required: true
    source: ops/security-readiness.yaml
    checks:
      - authentication_passed
      - authorization_passed
      - tenant_isolation_passed
      - key_rotation_passed
      - mtls_plan_attached
      - audit_log_passed
      - sensitive_data_scan_passed
      - dependency_security_review_passed
    evidence:
      - security_review
      - authz_test_report
      - tenant_isolation_report
      - audit_log_sample
blocking_rules:
  - missing_required_asset
  - missing_owner_signoff
  - missing_rollback_path
  - failing_or_unrun_required_stage
  - unresolved_sev1_or_sev2_incident_from_release
  - slo_fast_burn_detected
  - capacity_resource_trend_unbounded
  - security_blocking_finding_present
promotion_levels:
  controlled_production:
    requires:
      - generated_code_integrity
      - runtime_smoke
      - interface_governance
      - governance
      - reliability_evidence
      - release_control
      - security_readiness
  broad_production_stable:
    requires:
      - controlled_production
      - capacity_and_soak
      - soak_72h_passed
      - post_release_observation_complete
reporting:
  required_output:
    - promotion_level
    - passed_stages
    - failed_stages
    - blocking_rules_triggered
    - evidence_links
    - owner_signoff
"#,
        name = spec.service,
        boundary = boundary,
        endpoint_count = endpoint_count,
        compile_probe = compile_probe,
        runtime_probe = runtime_probe,
    )
}

fn regeneration_policy_yaml(spec: &ApiSpec, kind: ProjectKind) -> String {
    let boundary = match kind {
        ProjectKind::Rest => "rest",
        ProjectKind::Rpc => "rpc",
    };
    let endpoint_count = match kind {
        ProjectKind::Rest => spec.rest_routes.len(),
        ProjectKind::Rpc => spec.rpc_methods.len(),
    };
    let generated_owned = match kind {
        ProjectKind::Rest => {
            r#"    - src/main.rs
    - src/openapi/mod.rs
    - src/route/**
    - src/handler/mod.rs
    - src/types/mod.rs
    - README.md
    - ops/**"#
        }
        ProjectKind::Rpc => {
            r#"    - src/main.rs
    - src/pb/mod.rs
    - src/server/mod.rs
    - src/client/mod.rs
    - src/types/mod.rs
    - proto/service.proto
    - build.rs
    - README.md
    - ops/**"#
        }
    };
    let preserved = match kind {
        ProjectKind::Rest => {
            r#"    - config.yaml
    - src/config/mod.rs
    - src/svc/mod.rs
    - src/logic/**
    - src/middleware/**
    - src/handler/*/*.rs
    - src/model/**"#
        }
        ProjectKind::Rpc => {
            r#"    - config.yaml
    - src/config/mod.rs
    - src/svc/mod.rs
    - src/logic/**
    - src/model/**"#
        }
    };
    let contract_surface = match kind {
        ProjectKind::Rest => {
            "route_method_path_request_response_validation_openapi_security_projection"
        }
        ProjectKind::Rpc => "rpc_method_request_response_proto_service_security_projection",
    };

    format!(
        r#"# Generated by rozectl. Use this file to review update/regeneration safety.
service: {name}
boundary: {boundary}
endpoint_count: {endpoint_count}
contract_surface: {contract_surface}
regeneration_modes:
  create:
    allowed_when: output_directory_is_empty_or_missing
    blocks_when:
      - output_directory_contains_untracked_service_files
  update:
    allowed_when: preserving_application_owned_extension_points
    blocks_when:
      - generated_owned_file_was_hand_edited_without_regeneration
      - idl_breaking_change_missing_owner_signoff
      - production_gate_evidence_not_refreshed
  force:
    allowed_when: explicit_full_rebuild_approved
    blocks_when:
      - app_owned_files_have_uncommitted_changes
      - rollback_plan_missing
ownership:
  generator_owned:
{generated_owned}
  application_owned_or_preserved:
{preserved}
  never_put_business_logic_in:
    - generated transport glue
    - generated DTO or protobuf modules
    - generated route or RPC registration
    - generated OpenAPI or SDK projection
drift_classification:
  non_breaking:
    - adding_optional_request_field_with_default
    - adding_response_field_that_clients_can_ignore
    - adding_new_rest_route_or_rpc_method
    - adding_new_error_variant_with_documented_mapping
  risky:
    - changing_timeout_rate_limit_breaker_or_shedding_defaults
    - changing_auth_or_tenant_policy
    - changing_dependency_or_config_schema
    - changing_generated_metrics_labels
  breaking:
    - removing_route_or_rpc_method
    - changing_path_method_or_rpc_name
    - removing_or_renaming_required_field
    - changing_field_type_or_semantics
    - changing_error_response_shape
    - weakening_auth_authorization_or_tenant_isolation
required_on_change:
  any_idl_change:
    - regenerate_service
    - cargo_fmt_check
    - cargo_test
    - runtime_smoke
    - update_openapi_or_proto_artifacts
  risky_change:
    - rerun_production_gate
    - refresh_governance_baseline_review
    - refresh_slo_and_alert_review
    - refresh_security_readiness_if_policy_related
    - refresh_capacity_plan_if_resource_related
  breaking_change:
    - owner_signoff
    - migration_or_client_rollout_plan
    - rollback_plan
    - release_rollout_refresh
    - incident_response_refresh
    - production_evidence_report_refresh
ci_checks:
  required_files_exist:
    - ops/production-evidence.md
    - ops/governance-baseline.yaml
    - ops/prometheus-rules.yaml
    - ops/grafana-dashboard.json
    - ops/slo.yaml
    - ops/failure-injection-plan.yaml
    - ops/release-rollout.yaml
    - ops/incident-response.yaml
    - ops/capacity-plan.yaml
    - ops/security-readiness.yaml
    - ops/production-gate.yaml
    - ops/regeneration-policy.yaml
  block_if:
    - generated_owned_file_changed_without_idl_or_generator_change
    - preserved_file_deleted_by_regeneration
    - breaking_change_without_migration_plan
    - risky_change_without_refreshed_evidence
evidence_required:
  regeneration_diff_summary: true
  ownership_boundary_review: true
  idl_drift_classification: true
  production_gate_result: true
  rollback_plan_for_breaking_or_force: true
"#,
        name = spec.service,
        boundary = boundary,
        endpoint_count = endpoint_count,
        contract_surface = contract_surface,
        generated_owned = generated_owned,
        preserved = preserved,
    )
}

fn client_contract_yaml(spec: &ApiSpec, kind: ProjectKind) -> String {
    let boundary = match kind {
        ProjectKind::Rest => "rest",
        ProjectKind::Rpc => "rpc",
    };
    let endpoint_count = match kind {
        ProjectKind::Rest => spec.rest_routes.len(),
        ProjectKind::Rpc => spec.rpc_methods.len(),
    };
    let contract_artifact = match kind {
        ProjectKind::Rest => "src/openapi/mod.rs_and_generated_openapi_json",
        ProjectKind::Rpc => "proto/service.proto_and_generated_tonic_client",
    };
    let sdk_targets = match kind {
        ProjectKind::Rest => "typescript_javascript_dart_and_openapi_consumers",
        ProjectKind::Rpc => "rust_tonic_clients_and_proto_consumers",
    };
    let smoke_probe = match kind {
        ProjectKind::Rest => "generated_client_calls_representative_http_route",
        ProjectKind::Rpc => "generated_client_calls_representative_rpc_method",
    };

    format!(
        r#"# Generated by rozectl. This is a production contract for generated clients and consumers.
service: {name}
boundary: {boundary}
endpoint_count: {endpoint_count}
contract_artifact: {contract_artifact}
sdk_targets: {sdk_targets}
principles:
  idl_first: true
  no_silent_contract_drift: true
  typed_errors_required: true
  timeout_required: true
  retry_budget_required: true
  auth_injection_required: true
  trace_propagation_required: true
projection_required:
  request_types: true
  response_types: true
  error_model: true
  validation_constraints: true
  auth_policy: true
  timeout_policy: true
  retry_policy: true
  trace_headers_or_metadata: true
client_runtime_contract:
  timeout:
    required: true
    client_can_override_within_service_budget: true
    evidence: timeout_smoke_with_deadline_exceeded_result
  retry_budget:
    required: true
    must_not_retry_non_idempotent_operations_without_policy: true
    evidence: retry_attempts_capped_by_budget
  auth_injection:
    required: true
    supports_token_or_credential_provider: true
    evidence: valid_missing_expired_credentials_smoke
  trace_propagation:
    required: true
    correlation_id_required: true
    evidence: client_to_service_trace_sample
  typed_errors:
    required: true
    stable_fields:
      - code
      - message
      - trace_id
      - details
    evidence: typed_error_mapping_table
  cancellation:
    required: true
    evidence: client_cancel_reaches_service_deadline_or_cancel_path
smoke_tests:
  - test: contract_projection_generated
    probe: {contract_artifact}
    pass:
      - request_response_types_exist
      - validation_constraints_projected
      - auth_policy_projected
      - error_model_projected
    evidence:
      - generated_contract_artifact
      - projection_diff

  - test: generated_client_success_path
    probe: {smoke_probe}
    pass:
      - client_compiles
      - request_serialization_matches_contract
      - response_deserialization_matches_contract
      - trace_or_correlation_id_observed
    evidence:
      - client_smoke_output
      - trace_sample

  - test: generated_client_failure_path
    probe: invalid_request_auth_failure_timeout_and_server_error
    pass:
      - validation_error_maps_to_typed_error
      - auth_error_maps_to_typed_error
      - timeout_maps_to_typed_error
      - server_error_preserves_trace_id
    evidence:
      - typed_error_table
      - failure_smoke_output

  - test: generated_client_governance_path
    probe: retry_budget_timeout_cancellation_and_idempotency_policy
    pass:
      - retry_budget_caps_attempts
      - timeout_stops_waiting
      - cancellation_stops_inflight_call
      - non_idempotent_retry_requires_explicit_policy
    evidence:
      - retry_attempt_log
      - cancellation_trace
      - timeout_trace
promotion_required:
  contract_projection_generated: true
  generated_client_success_path_passed: true
  generated_client_failure_path_passed: true
  generated_client_governance_path_passed: true
  typed_error_table_attached: true
  client_owner_signoff: required
blocking_findings:
  - generated_client_does_not_compile
  - contract_projection_missing_error_model
  - auth_policy_not_projected
  - timeout_or_retry_budget_not_projected
  - trace_or_correlation_id_not_propagated
  - typed_error_missing_trace_id
"#,
        name = spec.service,
        boundary = boundary,
        endpoint_count = endpoint_count,
        contract_artifact = contract_artifact,
        sdk_targets = sdk_targets,
        smoke_probe = smoke_probe,
    )
}

fn config_governance_yaml(spec: &ApiSpec, kind: ProjectKind) -> String {
    let boundary = match kind {
        ProjectKind::Rest => "rest",
        ProjectKind::Rpc => "rpc",
    };
    let endpoint_count = match kind {
        ProjectKind::Rest => spec.rest_routes.len(),
        ProjectKind::Rpc => spec.rpc_methods.len(),
    };
    let reload_probe = match kind {
        ProjectKind::Rest => "reload_timeout_rate_limit_cors_registry_and_dependency_config",
        ProjectKind::Rpc => "reload_timeout_registry_client_deadline_and_dependency_config",
    };

    format!(
        r#"# Generated by rozectl. Use this as the production contract for config changes.
service: {name}
boundary: {boundary}
endpoint_count: {endpoint_count}
config_sources:
  local_file: config.yaml
  external_config_center: optional_required_for_broad_production
  secret_manager: required_for_secrets
principles:
  schema_validated: true
  no_plaintext_secrets: true
  versioned_changes: true
  audited_operator_actions: true
  canary_before_global: true
  rollback_ready: true
  listener_failure_isolated: true
schema:
  required:
    - service_name
    - bind_address_or_endpoint
    - timeout_budget
    - resilience_settings
    - tracing_metrics_settings
    - registry_or_discovery_settings
  validation:
    - reject_unknown_critical_fields
    - reject_negative_timeouts_or_limits
    - reject_plaintext_secret_values
    - reject_incomplete_dependency_endpoints
change_control:
  required_metadata:
    - config_version
    - operator
    - change_reason
    - linked_ticket
    - rollout_scope
    - rollback_version
  diff_required: true
  signature_or_approval_required: true
  audit_log_required: true
rollout:
  - phase: validate
    probe: parse_schema_validate_and_secret_scan
    pass:
      - schema_valid
      - no_plaintext_secret
      - rollback_version_exists
      - owner_approved
    evidence:
      - config_diff
      - validation_output
      - approval_record

  - phase: canary_reload
    probe: {reload_probe}
    pass:
      - only_canary_instance_receives_change
      - readiness_stays_true_or_drains_cleanly
      - invalid_config_is_rejected
      - listener_timeout_does_not_block_runtime
      - metrics_report_reload_result
    evidence:
      metrics_query: roze_config_reload_total{{service="{name}"}}
      log_query: config_reload_audit_logs
      trace_query: config_reload_trace_if_available

  - phase: global_rollout
    probe: roll_config_to_all_instances_after_canary_passes
    pass:
      - all_instances_report_same_config_version
      - no_restart_loop
      - no_slo_fast_burn
      - no_unexpected_resilience_decision_spike
    evidence:
      - config_version_per_instance
      - metrics_snapshot
      - alert_snapshot

  - phase: rollback
    probe: rollback_to_last_known_good_config
    pass:
      - previous_config_version_restored
      - service_recovers_within_rollback_window
      - rollback_action_is_audited
      - rejected_config_is_quarantined
    evidence:
      - rollback_command_or_event
      - recovery_time
      - audit_record

  - phase: snapshot_restore
    probe: restore_config_snapshot_in_staging
    pass:
      - snapshot_checksum_verified
      - restored_config_matches_expected_version
      - service_starts_and_reports_ready
    evidence:
      - snapshot_id
      - checksum
      - startup_smoke_output
failure_isolation:
  listener_timeout:
    max_duration: 5s
    required: true
  listener_panic:
    must_not_crash_service: true
  invalid_remote_event:
    must_keep_last_known_good: true
  config_center_unavailable:
    service_uses_cached_or_local_config: true
promotion_required:
  schema_validation_passed: true
  config_diff_reviewed: true
  audit_record_attached: true
  canary_reload_passed: true
  invalid_config_rejection_tested: true
  rollback_tested: true
  snapshot_restore_tested: true
  owner_signoff: required
blocking_findings:
  - plaintext_secret_in_config
  - missing_rollback_version
  - invalid_config_applied_globally
  - listener_failure_blocks_runtime
  - config_change_without_audit_record
  - config_center_unavailable_breaks_startup
"#,
        name = spec.service,
        boundary = boundary,
        endpoint_count = endpoint_count,
        reload_probe = reload_probe,
    )
}

fn reliable_events_yaml(spec: &ApiSpec, kind: ProjectKind) -> String {
    let boundary = match kind {
        ProjectKind::Rest => "rest",
        ProjectKind::Rpc => "rpc",
    };
    let endpoint_count = match kind {
        ProjectKind::Rest => spec.rest_routes.len(),
        ProjectKind::Rpc => spec.rpc_methods.len(),
    };
    let producer_probe = match kind {
        ProjectKind::Rest => "representative_http_mutation_publishes_or_declares_no_event",
        ProjectKind::Rpc => "representative_rpc_mutation_publishes_or_declares_no_event",
    };

    format!(
        r#"# Generated by rozectl. Required before enabling asynchronous side effects.
service: {name}
boundary: {boundary}
endpoint_count: {endpoint_count}
mode: reliable_event_readiness
async_boundary:
  enabled: service_owner_decision_required
  if_disabled:
    required_evidence: no_async_side_effects_or_external_event_contracts
  if_enabled:
    broker: kafka_nats_or_platform_supported_broker_required
    delivery_semantics: at_least_once
    exactly_once_claims_forbidden_without_evidence: true
event_contract:
  envelope_required: true
  required_fields:
    - event_id
    - event_type
    - schema_version
    - occurred_at
    - producer
    - trace_id
    - tenant_id_when_multi_tenant
    - idempotency_key
  schema_registry_or_contract_store_required: true
  compatibility_policy: no_implicit_compatibility_claims
producer:
  probe: {producer_probe}
  required:
    - idempotency_key_generated_before_publish
    - outbox_used_for_db_plus_event_atomicity
    - publish_timeout_defined
    - retry_budget_defined
    - trace_context_attached
  evidence:
    - outbox_insert_and_publish_trace
    - duplicate_publish_idempotency_test
    - publish_timeout_test
consumer:
  required:
    - inbox_or_dedup_store_used
    - handler_is_idempotent
    - poison_message_policy_defined
    - consumer_lag_metric_exported
    - graceful_shutdown_drains_or_commits_safely
  evidence:
    - duplicate_delivery_test
    - poison_message_test
    - lag_metric_sample
    - shutdown_commit_or_ack_trace
dlq:
  required: true
  policy:
    - max_attempts_defined
    - backoff_defined
    - dlq_topic_or_subject_defined
    - replay_requires_operator_and_reason
    - purge_requires_owner_signoff
  evidence:
    - dlq_routing_test
    - dlq_replay_test
    - dlq_purge_audit_record
retry_storm_protection:
  required: true
  controls:
    - retry_budget
    - exponential_backoff_with_jitter
    - max_inflight_limit
    - circuit_breaker_for_failing_dependency
    - load_shedding_when_lag_or_error_rate_grows
  metrics:
    - roze_event_retry_attempts_total
    - roze_event_dlq_total
    - roze_event_consumer_lag
    - roze_event_dedup_total
    - roze_event_publish_duration_seconds
tests:
  - test: outbox_inbox_idempotency
    pass:
      - duplicate_event_processed_once
      - duplicate_publish_does_not_duplicate_side_effect
      - trace_id_preserved_across_event
    evidence:
      - idempotency_key_samples
      - dedup_metric_sample
      - event_trace_sample

  - test: broker_failure
    pass:
      - publish_failure_does_not_lose_outbox_record
      - retry_budget_caps_attempts
      - service_recovers_after_broker_returns
    evidence:
      - outbox_backlog_timeline
      - retry_budget_metric_sample
      - recovery_time

  - test: dlq_replay
    pass:
      - poison_message_moves_to_dlq
      - replay_is_audited
      - replay_is_idempotent
      - purge_requires_signoff
    evidence:
      - dlq_message_id
      - replay_audit_log
      - replay_result

  - test: lag_and_shutdown
    pass:
      - lag_alert_fires_when_consumer_is_slow
      - graceful_shutdown_does_not_ack_unprocessed_work
      - restart_resumes_from_committed_position
    evidence:
      - lag_metric_sample
      - shutdown_trace
      - restart_resume_trace
promotion_required:
  async_boundary_decision_recorded: true
  envelope_contract_defined_or_disabled_evidence_attached: true
  idempotency_test_passed: true
  outbox_inbox_policy_passed: true
  dlq_replay_test_passed: true
  retry_storm_protection_passed: true
  lag_metric_attached: true
  owner_signoff: required
blocking_findings:
  - event_without_idempotency_key
  - db_write_and_event_publish_without_outbox_or_transactional_evidence
  - consumer_side_effect_not_idempotent
  - dlq_replay_without_audit
  - unbounded_retry_without_budget
  - lag_metric_missing_for_enabled_consumer
"#,
        name = spec.service,
        boundary = boundary,
        endpoint_count = endpoint_count,
        producer_probe = producer_probe,
    )
}

fn dependency_governance_yaml(spec: &ApiSpec, kind: ProjectKind) -> String {
    let boundary = match kind {
        ProjectKind::Rest => "rest",
        ProjectKind::Rpc => "rpc",
    };
    let endpoint_count = match kind {
        ProjectKind::Rest => spec.rest_routes.len(),
        ProjectKind::Rpc => spec.rpc_methods.len(),
    };
    let representative_call = match kind {
        ProjectKind::Rest => {
            "representative_http_handler_calls_declared_downstream_or_declares_none"
        }
        ProjectKind::Rpc => "representative_rpc_method_calls_declared_downstream_or_declares_none",
    };

    format!(
        r#"# Generated by rozectl. Required for every downstream before broad production rollout.
service: {name}
boundary: {boundary}
endpoint_count: {endpoint_count}
mode: dependency_governance_readiness
inventory:
  required: true
  if_no_downstream:
    evidence: no_runtime_dependency_clients_or_external_calls
  downstream_fields:
    - name
    - type
    - owner
    - endpoint_or_discovery_key
    - timeout_budget
    - retry_policy
    - breaker_policy
    - fallback_policy
    - data_classification
principles:
  deadline_propagation_required: true
  no_unbounded_retry: true
  no_downstream_without_owner: true
  no_dependency_without_metrics: true
  fallback_or_explicit_fail_closed_required: true
discovery:
  required_for_dynamic_downstreams: true
  checks:
    - registry_add_remove_event_observed
    - stale_endpoint_evicted
    - readiness_or_health_used_for_endpoint_selection
    - config_center_or_registry_failure_has_cached_behavior
  evidence:
    - registry_event_log
    - endpoint_set_before_after
    - stale_endpoint_request_sample
load_balancing:
  required: true
  policy: round_robin_weighted_or_platform_policy
  checks:
    - traffic_distribution_is_visible
    - unhealthy_endpoint_gets_less_or_no_traffic
    - outlier_endpoint_is_detected
    - per_endpoint_latency_is_visible
  metrics:
    - roze_dependency_requests_total
    - roze_dependency_latency_seconds
    - roze_dependency_endpoint_health
    - roze_dependency_outlier_total
timeouts_and_cancellation:
  probe: {representative_call}
  required:
    - client_timeout_less_than_request_deadline
    - deadline_propagates_to_logic_and_dependency
    - cancellation_stops_inflight_dependency_call
    - timeout_error_maps_to_typed_error
  evidence:
    - timeout_trace
    - cancellation_trace
    - typed_error_sample
resilience:
  retry_budget:
    required: true
    checks:
      - max_attempts_defined
      - backoff_with_jitter_defined
      - retry_only_idempotent_or_explicitly_allowed_operations
      - retry_budget_metric_exported
  circuit_breaker:
    required: true
    checks:
      - open_half_open_closed_transitions_observed
      - breaker_state_metric_exported
      - breaker_open_does_not_amplify_dependency_pressure
  bulkhead:
    required: true
    checks:
      - max_inflight_per_dependency_defined
      - saturation_does_not_block_unrelated_dependencies
      - queue_or_semaphore_wait_is_bounded
  fallback:
    required: explicit_policy
    allowed_modes:
      - cached_response
      - degraded_response
      - fail_closed
      - alternate_dependency
tests:
  - test: downstream_inventory
    pass:
      - every_dependency_has_owner
      - every_dependency_has_timeout_retry_breaker_policy
      - every_dependency_has_metrics_and_trace_labels
    evidence:
      - dependency_inventory_table

  - test: endpoint_change
    pass:
      - new_endpoint_receives_traffic
      - removed_endpoint_stops_receiving_traffic
      - stale_endpoint_error_does_not_cause_slo_fast_burn
    evidence:
      - registry_event_log
      - load_balancing_distribution_sample

  - test: slow_downstream
    pass:
      - timeout_fires_within_budget
      - breaker_or_shedding_protects_service
      - accepted_requests_keep_latency_budget
    evidence:
      - p99_latency_before_during_after
      - breaker_transition_metric
      - resource_trend

  - test: failing_downstream
    pass:
      - breaker_opens
      - retry_budget_caps_attempts
      - fallback_or_fail_closed_behavior_matches_policy
      - recovery_closes_breaker_after_dependency_recovers
    evidence:
      - breaker_timeline
      - retry_budget_metric
      - fallback_result

  - test: dependency_saturation
    pass:
      - bulkhead_limits_inflight_calls
      - unrelated_dependencies_remain_healthy
      - service_remains_ready_or_drains_intentionally
    evidence:
      - inflight_metric_sample
      - unrelated_dependency_latency_sample
      - readiness_timeline
promotion_required:
  downstream_inventory_passed: true
  endpoint_change_test_passed: true
  slow_downstream_test_passed: true
  failing_downstream_test_passed: true
  dependency_saturation_test_passed: true
  fallback_policy_reviewed: true
  owner_signoff: required
blocking_findings:
  - dependency_without_owner
  - dependency_without_timeout
  - dependency_without_retry_budget
  - dependency_without_circuit_breaker
  - dependency_without_metrics_or_traces
  - unbounded_connection_pool
  - fallback_claim_without_test_evidence
"#,
        name = spec.service,
        boundary = boundary,
        endpoint_count = endpoint_count,
        representative_call = representative_call,
    )
}

fn data_consistency_yaml(spec: &ApiSpec, kind: ProjectKind) -> String {
    let boundary = match kind {
        ProjectKind::Rest => "rest",
        ProjectKind::Rpc => "rpc",
    };
    let endpoint_count = match kind {
        ProjectKind::Rest => spec.rest_routes.len(),
        ProjectKind::Rpc => spec.rpc_methods.len(),
    };
    let write_probe = match kind {
        ProjectKind::Rest => "representative_http_mutation_declares_transaction_or_no_persistence",
        ProjectKind::Rpc => "representative_rpc_mutation_declares_transaction_or_no_persistence",
    };

    format!(
        r#"# Generated by rozectl. Required before enabling persistent writes or distributed transactions.
service: {name}
boundary: {boundary}
endpoint_count: {endpoint_count}
mode: data_consistency_readiness
persistence_boundary:
  required: true
  if_no_persistence:
    evidence: no_database_cache_search_index_or_external_state_mutation
  if_persistent:
    stores:
      - sql
      - mongo
      - redis_or_cache
      - search_index
      - external_stateful_dependency
principles:
  transaction_boundary_required: true
  idempotent_writes_required: true
  migration_rollback_required: true
  no_dual_write_without_outbox_or_dtm_evidence: true
  reconciliation_required_for_eventual_consistency: true
  backup_restore_required_for_broad_production: true
write_paths:
  probe: {write_probe}
  required_for_each_mutation:
    - transaction_scope
    - idempotency_key_or_natural_unique_key
    - retry_safety_classification
    - rollback_or_compensation_strategy
    - audit_or_trace_correlation
  evidence:
    - mutation_inventory
    - duplicate_write_test
    - rollback_or_compensation_test
migrations:
  required: true
  checks:
    - forward_migration_script_reviewed
    - rollback_or_compensating_migration_reviewed
    - expand_migrate_contract_plan_for_breaking_schema_changes
    - migration_lock_or_serialization_policy_defined
    - migration_runtime_and_row_count_estimated
  evidence:
    - migration_plan
    - dry_run_output
    - rollback_dry_run_output
transactions:
  local_transaction:
    required_when_single_store_write: true
    evidence: commit_and_rollback_test
  outbox:
    required_when_db_write_publishes_event: true
    evidence: db_commit_outbox_publish_recovery_test
  dtm_or_saga:
    required_when_multiple_stateful_services_mutate: true
    evidence: saga_success_failure_compensation_test
  inbox:
    required_when_consuming_events_mutates_state: true
    evidence: duplicate_event_mutates_once
read_write_consistency:
  required: true
  checks:
    - read_after_write_expectation_documented
    - cache_invalidation_or_refresh_policy_defined
    - search_index_lag_budget_defined_when_used
    - stale_read_behavior_documented
  evidence:
    - read_after_write_test
    - cache_invalidation_test
    - index_lag_metric_sample
reconciliation:
  required_for_eventual_consistency: true
  checks:
    - reconciliation_job_or_query_defined
    - drift_threshold_defined
    - repair_action_is_audited
    - reconciliation_does_not_overwrite_newer_state
  evidence:
    - reconciliation_report_sample
    - repair_audit_log
backup_restore:
  required_for_broad_production: true
  checks:
    - backup_schedule_defined
    - restore_test_passed
    - point_in_time_recovery_objective_defined
    - restore_runbook_attached
  evidence:
    - backup_snapshot_id
    - restore_duration
    - restored_row_or_document_count
    - consistency_check_after_restore
tests:
  - test: idempotent_write
    pass:
      - duplicate_request_does_not_duplicate_state
      - retry_after_timeout_is_safe_or_rejected
      - idempotency_key_conflict_is_observable
    evidence:
      - duplicate_request_trace
      - idempotency_metric_sample

  - test: transaction_failure
    pass:
      - partial_failure_rolls_back_or_compensates
      - no_orphan_outbox_or_inbox_state
      - typed_error_preserves_trace_id
    evidence:
      - rollback_trace
      - outbox_inbox_state_query

  - test: migration_rollback
    pass:
      - forward_migration_succeeds_in_staging
      - rollback_or_compensation_succeeds_in_staging
      - old_and_new_service_versions_are_accounted_for_without_compatibility_claim
    evidence:
      - forward_migration_output
      - rollback_output
      - owner_signoff

  - test: backup_restore
    pass:
      - backup_restores_to_isolated_environment
      - restored_data_passes_consistency_checks
      - restore_time_meets_recovery_objective
    evidence:
      - restore_log
      - consistency_query_output
      - recovery_time
promotion_required:
  persistence_boundary_declared: true
  idempotent_write_test_passed: true
  transaction_failure_test_passed: true
  migration_rollback_test_passed: true
  backup_restore_test_passed_for_broad_production: true
  reconciliation_plan_attached_when_eventual_consistency_used: true
  owner_signoff: required
blocking_findings:
  - persistent_write_without_transaction_boundary
  - non_idempotent_retryable_write
  - dual_write_without_outbox_or_dtm_evidence
  - migration_without_rollback_or_compensation
  - cache_or_search_index_without_staleness_policy
  - backup_restore_not_tested_for_broad_production
"#,
        name = spec.service,
        boundary = boundary,
        endpoint_count = endpoint_count,
        write_probe = write_probe,
    )
}

fn observability_contract_yaml(spec: &ApiSpec, kind: ProjectKind) -> String {
    let boundary = match kind {
        ProjectKind::Rest => "rest",
        ProjectKind::Rpc => "rpc",
    };
    let endpoint_count = match kind {
        ProjectKind::Rest => spec.rest_routes.len(),
        ProjectKind::Rpc => spec.rpc_methods.len(),
    };
    let request_metric = match kind {
        ProjectKind::Rest => "roze_http_requests_total",
        ProjectKind::Rpc => "roze_rpc_requests_total",
    };
    let latency_metric = match kind {
        ProjectKind::Rest => "roze_http_request_duration_seconds_bucket",
        ProjectKind::Rpc => "roze_rpc_method_duration_seconds_bucket",
    };
    let operation_label = match kind {
        ProjectKind::Rest => "method_route_status",
        ProjectKind::Rpc => "service_method_status",
    };

    format!(
        r#"# Generated by rozectl. Required for production debugging and evidence retention.
service: {name}
boundary: {boundary}
endpoint_count: {endpoint_count}
mode: observability_contract
signals:
  metrics:
    required: true
    request_metric: {request_metric}
    latency_metric: {latency_metric}
    resilience_metric: roze_resilience_decisions_total
    labels_required:
      - service
      - boundary
      - version
      - {operation_label}
      - result
    label_cardinality_budget:
      route_or_method: bounded_by_idl
      status: bounded
      error_code: bounded
      tenant: forbidden_unless_low_cardinality_or_sampled
      user_id: forbidden
  logs:
    required: true
    structured_fields:
      - timestamp
      - level
      - service
      - boundary
      - version
      - trace_id
      - request_id
      - operation
      - error_code
      - tenant_id_when_allowed
    forbidden_fields:
      - password
      - token
      - secret
      - private_key
      - raw_authorization_header
  traces:
    required: true
    propagation:
      - trace_id
      - span_id
      - request_id_or_correlation_id
      - deadline_or_timeout_budget
    spans_required:
      - transport_entry
      - validation
      - auth_when_enabled
      - logic
      - downstream_dependency
      - data_or_event_boundary_when_used
  profiles:
    required_for_broad_production: true
    types:
      - cpu
      - heap
      - task_or_thread_dump
      - allocation_or_contention_when_available
sampling:
  traces:
    normal: service_owner_defined
    errors: always_sample_or_high_rate
    slow_requests: always_sample
  logs:
    info_sampling_allowed: true
    warn_error_sampling_forbidden: true
  profiles:
    on_demand: true
    incident_capture: required
debug_queries:
  request_rate: sum(rate({request_metric}{{service="{name}"}}[5m]))
  error_rate: sum(rate({request_metric}{{service="{name}",status=~"5.."}}[5m])) / clamp_min(sum(rate({request_metric}{{service="{name}"}}[5m])), 1)
  p99_latency: histogram_quantile(0.99, sum(rate({latency_metric}{{service="{name}"}}[5m])) by (le))
  resilience_decisions: sum(rate(roze_resilience_decisions_total{{service="{name}"}}[5m])) by (kind, decision)
  restarts: increase(process_start_time_seconds{{service="{name}"}}[15m])
retention:
  metrics: service_owner_defined_minimum_30d
  logs: service_owner_defined_minimum_14d
  traces: service_owner_defined_minimum_7d
  incident_evidence: retain_with_production_evidence_report
tests:
  - test: metrics_export
    pass:
      - request_count_increments
      - latency_histogram_records
      - error_status_or_code_is_bounded
      - resilience_decision_metric_available
    evidence:
      - metrics_scrape_sample
      - cardinality_report

  - test: log_correlation
    pass:
      - request_id_present
      - trace_id_present
      - error_code_present_for_failures
      - forbidden_sensitive_fields_absent
    evidence:
      - log_query_sample
      - redaction_scan_result

  - test: trace_propagation
    pass:
      - entry_span_created
      - downstream_span_links_to_entry_span
      - deadline_or_timeout_budget_visible
      - error_trace_contains_typed_error_code
    evidence:
      - trace_query_sample
      - slow_trace_sample

  - test: incident_diagnostics
    pass:
      - dashboard_links_required_queries
      - profile_or_runtime_diagnostics_capture_available
      - evidence_report_can_link_metrics_logs_traces
    evidence:
      - dashboard_link
      - profile_or_diagnostics_sample
      - evidence_report_links
promotion_required:
  metrics_export_passed: true
  log_correlation_passed: true
  trace_propagation_passed: true
  incident_diagnostics_passed: true
  cardinality_budget_reviewed: true
  retention_policy_attached: true
  owner_signoff: required
blocking_findings:
  - missing_request_or_latency_metric
  - unbounded_metric_label
  - logs_missing_trace_id
  - traces_missing_downstream_span
  - sensitive_data_in_logs_or_labels
  - no_debug_query_for_primary_slo
"#,
        name = spec.service,
        boundary = boundary,
        endpoint_count = endpoint_count,
        request_metric = request_metric,
        latency_metric = latency_metric,
        operation_label = operation_label,
    )
}

fn runtime_hardening_yaml(spec: &ApiSpec, kind: ProjectKind) -> String {
    let boundary = match kind {
        ProjectKind::Rest => "rest",
        ProjectKind::Rpc => "rpc",
    };
    let endpoint_count = match kind {
        ProjectKind::Rest => spec.rest_routes.len(),
        ProjectKind::Rpc => spec.rpc_methods.len(),
    };
    let entry_probe = match kind {
        ProjectKind::Rest => "representative_http_request_with_server_timeout_and_client_cancel",
        ProjectKind::Rpc => "representative_rpc_call_with_client_deadline_and_cancel",
    };
    let request_metric = match kind {
        ProjectKind::Rest => "roze_http_requests_total",
        ProjectKind::Rpc => "roze_rpc_requests_total",
    };
    let latency_metric = match kind {
        ProjectKind::Rest => "roze_http_request_duration_seconds_bucket",
        ProjectKind::Rpc => "roze_rpc_method_duration_seconds_bucket",
    };

    format!(
        r#"# Generated by rozectl. Required for runtime governance before production promotion.
service: {name}
boundary: {boundary}
endpoint_count: {endpoint_count}
mode: runtime_hardening
entry_probe: {entry_probe}
policy:
  timeout:
    required: true
    source: generated_config_and_platform_defaults
    enforce_at:
      - transport_entry
      - logic_boundary
      - downstream_dependency
    evidence:
      - timeout_metric
      - deadline_trace
      - cancellation_log
  rate_limit:
    required: true
    source: generated_config_or_gateway_policy
    modes:
      - per_service
      - per_operation_when_needed
      - tenant_or_principal_when_low_cardinality
    evidence:
      - allowed_rejected_metric_sample
      - limit_config_snapshot
  circuit_breaker:
    required: true
    source: dependency_governance_and_runtime_policy
    states:
      - closed
      - open
      - half_open
    evidence:
      - breaker_transition_metric
      - dependency_failure_trace
  load_shedding:
    required: true
    source: generated_config_or_platform_policy
    signals:
      - concurrency
      - queue_depth
      - latency_budget
      - cpu_or_memory_pressure
    evidence:
      - shedding_decision_metric
      - protected_latency_sample
  retry_budget:
    required: true
    max_attempts: service_owner_defined
    jitter: required
    amplification_cap: required
    evidence:
      - retry_attempt_metric
      - amplification_review
  deadline_propagation:
    required: true
    propagate_to:
      - auth
      - validation
      - logic
      - database
      - cache
      - rpc_or_http_downstream
      - mq_or_outbox_when_used
    evidence:
      - deadline_trace_sample
      - cancelled_work_does_not_continue
  graceful_shutdown:
    required: true
    phases:
      - mark_not_ready
      - stop_accepting_new_work
      - drain_in_flight_until_deadline
      - cancel_remaining_work
      - flush_metrics_logs_traces
    evidence:
      - shutdown_timeline
      - readyz_or_readiness_transition
      - no_lost_in_flight_work_without_explicit_cancellation
  resource_guards:
    required: true
    guards:
      - bounded_request_body_or_message_size
      - bounded_concurrency
      - bounded_connection_or_stream_lifetime
      - bounded_queue_depth
      - memory_and_cpu_alerts
    evidence:
      - resource_trend_sample
      - guard_rejection_sample
metrics:
  request_metric: {request_metric}
  latency_metric: {latency_metric}
  resilience_metric: roze_resilience_decisions_total
  resource_metrics:
    - process_cpu_seconds_total
    - process_resident_memory_bytes
    - tokio_tasks_or_runtime_diagnostics_when_available
tests:
  - test: timeout_and_deadline
    probe: {entry_probe}
    pass:
      - request_or_call_times_out_with_typed_error
      - downstream_work_receives_deadline
      - cancellation_is_observed_before_business_side_effect
    evidence:
      - metrics_query: sum(rate(roze_resilience_decisions_total{{service="{name}",kind=~"timeout|deadline"}}[5m]))
      - trace_query: deadline_propagates_to_logic_and_dependency
      - log_query: timeout_or_cancellation_log_with_trace_id

  - test: rate_limit_and_backpressure
    pass:
      - excess_work_rejected_or_queued_with_bound
      - accepted_work_stays_inside_latency_budget
      - retry_after_or_typed_limit_error_defined
    evidence:
      - metrics_query: sum(rate(roze_resilience_decisions_total{{service="{name}",kind="rate_limit"}}[5m])) by (decision)
      - load_profile
      - rejection_sample

  - test: breaker_and_retry_budget
    pass:
      - dependency_5xx_opens_breaker
      - half_open_probe_is_bounded
      - retries_do_not_exceed_budget
      - fallback_or_degraded_response_is_documented
    evidence:
      - metrics_query: sum(rate(roze_resilience_decisions_total{{service="{name}",kind=~"breaker|retry"}}[5m])) by (decision)
      - breaker_transition_timeline
      - retry_amplification_report

  - test: load_shedding_and_resource_guards
    pass:
      - shedding_starts_before_resource_exhaustion
      - memory_and_cpu_trend_remain_bounded
      - queue_or_concurrency_limit_is_visible
    evidence:
      - metrics_query: sum(rate(roze_resilience_decisions_total{{service="{name}",kind="load_shed"}}[5m])) by (decision)
      - resource_trend_report
      - protected_latency_report

  - test: shutdown_and_drain
    pass:
      - readiness_turns_false_before_stop
      - in_flight_work_drains_or_is_cancelled_by_deadline
      - final_metrics_logs_and_traces_are_flushed
    evidence:
      - shutdown_timeline
      - readiness_probe_output
      - lifecycle_summary
promotion_required:
  timeout_deadline_test_passed: true
  rate_limit_backpressure_test_passed: true
  breaker_retry_budget_test_passed: true
  load_shedding_resource_guard_test_passed: true
  shutdown_drain_test_passed: true
  resource_alerts_attached: true
  owner_signoff: required
blocking_findings:
  - missing_timeout_or_deadline_policy
  - unbounded_retry_amplification
  - breaker_without_state_transition_evidence
  - load_shedding_without_resource_trend
  - graceful_shutdown_without_readiness_drain_timeline
  - unbounded_queue_body_message_or_concurrency
"#,
        name = spec.service,
        boundary = boundary,
        endpoint_count = endpoint_count,
        entry_probe = entry_probe,
        request_metric = request_metric,
        latency_metric = latency_metric,
    )
}

fn error_contract_yaml(spec: &ApiSpec, kind: ProjectKind) -> String {
    let boundary = match kind {
        ProjectKind::Rest => "rest",
        ProjectKind::Rpc => "rpc",
    };
    let endpoint_count = match kind {
        ProjectKind::Rest => spec.rest_routes.len(),
        ProjectKind::Rpc => spec.rpc_methods.len(),
    };
    let transport_mapping = match kind {
        ProjectKind::Rest => {
            "http_status_mapping:
      validation: 400
      unauthenticated: 401
      permission_denied: 403
      not_found: 404
      conflict: 409
      rate_limited: 429
      timeout: 504
      dependency_unavailable: 503
      internal: 500"
        }
        ProjectKind::Rpc => {
            "grpc_status_mapping:
      validation: INVALID_ARGUMENT
      unauthenticated: UNAUTHENTICATED
      permission_denied: PERMISSION_DENIED
      not_found: NOT_FOUND
      conflict: ABORTED
      rate_limited: RESOURCE_EXHAUSTED
      timeout: DEADLINE_EXCEEDED
      dependency_unavailable: UNAVAILABLE
      internal: INTERNAL"
        }
    };
    let representative_probe = match kind {
        ProjectKind::Rest => "representative_http_error_response",
        ProjectKind::Rpc => "representative_rpc_status_and_metadata",
    };
    let status_sample = match kind {
        ProjectKind::Rest => "http_status_body_headers_and_trace_id",
        ProjectKind::Rpc => "grpc_status_code_metadata_and_trace_id",
    };
    let request_metric = match kind {
        ProjectKind::Rest => "roze_http_requests_total",
        ProjectKind::Rpc => "roze_rpc_requests_total",
    };

    format!(
        r#"# Generated by rozectl. Required for stable client behavior and incident triage.
service: {name}
boundary: {boundary}
endpoint_count: {endpoint_count}
mode: error_contract
catalog:
  typed_errors_required: true
  fields:
    - code
    - message
    - retryable
    - idempotency_required
    - trace_id
    - request_id_or_correlation_id
    - details_when_safe
  categories:
    validation:
      retryable: false
      client_action: fix_request
    unauthenticated:
      retryable: false
      client_action: refresh_or_supply_credentials
    permission_denied:
      retryable: false
      client_action: request_access
    not_found:
      retryable: false
      client_action: check_identifier_or_treat_as_absent
    conflict:
      retryable: conditional
      idempotency_required: true
      client_action: reload_state_or_use_idempotency_key
    rate_limited:
      retryable: true
      client_action: honor_retry_after_and_retry_budget
    timeout:
      retryable: conditional
      idempotency_required: true
      client_action: retry_only_when_operation_is_idempotent
    dependency_unavailable:
      retryable: true
      client_action: retry_with_backoff_inside_budget
    internal:
      retryable: false
      client_action: surface_trace_id_and_stop_retry_storm
transport:
  {transport_mapping}
response_contract:
  trace_id_required: true
  request_id_or_correlation_id_required: true
  raw_internal_error_forbidden: true
  stack_trace_forbidden_in_client_response: true
  sensitive_fields_forbidden:
    - password
    - token
    - secret
    - private_key
    - authorization
  representative_probe: {representative_probe}
client_behavior:
  typed_error_projection_required: true
  retry_budget_required: true
  retry_after_supported_for_rate_limit: true
  idempotency_key_required_for_retryable_mutations: true
  cancellation_and_timeout_errors_distinguishable: true
  no_implicit_compatibility_claim: true
observability:
  metric: {request_metric}
  labels:
    - service
    - boundary
    - operation
    - status_or_code
    - error_code
  error_code_cardinality: bounded_by_catalog
  log_fields:
    - trace_id
    - request_id
    - error_code
    - retryable
    - redaction_applied
tests:
  - test: typed_error_catalog
    pass:
      - every_generated_operation_has_declared_error_projection_or_default_catalog
      - typed_error_fields_are_available_to_clients
      - error_code_cardinality_is_bounded
    evidence:
      - catalog_review
      - generated_client_error_type_sample

  - test: transport_status_mapping
    probe: {representative_probe}
    pass:
      - validation_error_maps_to_expected_transport_status
      - timeout_error_maps_to_expected_transport_status
      - dependency_error_maps_to_expected_transport_status
      - trace_id_available_for_each_failure
    evidence:
      - {status_sample}
      - trace_query_sample

  - test: retryability_and_idempotency
    pass:
      - retryable_errors_are_explicit
      - non_retryable_errors_are_explicit
      - retryable_mutation_requires_idempotency_key_or_owner_exception
      - retry_after_or_backoff_policy_is_documented
    evidence:
      - client_retry_behavior_report
      - idempotency_review

  - test: redaction
    pass:
      - internal_error_details_are_not_returned_to_clients
      - forbidden_sensitive_fields_absent_from_response_and_logs
      - public_message_is_actionable_without_leaking_secret
    evidence:
      - redaction_scan_result
      - representative_error_response

  - test: failure_metrics
    pass:
      - error_code_label_is_bounded_by_catalog
      - failure_rate_query_uses_status_or_error_code
      - retry_storm_can_be_detected
    evidence:
      - metrics_query: sum(rate({request_metric}{{service="{name}",error_code!=""}}[5m])) by (error_code)
      - retry_metric_sample
promotion_required:
  typed_error_catalog_passed: true
  transport_status_mapping_passed: true
  retryability_idempotency_passed: true
  redaction_passed: true
  failure_metrics_passed: true
  owner_signoff: required
blocking_findings:
  - raw_internal_error_returned_to_client
  - retryable_error_without_retry_budget
  - retryable_mutation_without_idempotency_policy
  - unbounded_error_code_label
  - missing_trace_id_on_error
  - transport_status_mapping_missing
"#,
        name = spec.service,
        boundary = boundary,
        endpoint_count = endpoint_count,
        transport_mapping = transport_mapping,
        representative_probe = representative_probe,
        status_sample = status_sample,
        request_metric = request_metric,
    )
}

fn deployment_topology_yaml(spec: &ApiSpec, kind: ProjectKind) -> String {
    let boundary = match kind {
        ProjectKind::Rest => "rest",
        ProjectKind::Rpc => "rpc",
    };
    let endpoint_count = match kind {
        ProjectKind::Rest => spec.rest_routes.len(),
        ProjectKind::Rpc => spec.rpc_methods.len(),
    };
    let port = match kind {
        ProjectKind::Rest => 8080,
        ProjectKind::Rpc => 50051,
    };
    let startup_probe = match kind {
        ProjectKind::Rest => "GET /startupz",
        ProjectKind::Rpc => "grpc_health_probe_startup_or_generated_startup_probe",
    };
    let readiness_probe = match kind {
        ProjectKind::Rest => "GET /readyz",
        ProjectKind::Rpc => "grpc_health_probe_readiness_or_generated_readiness_probe",
    };
    let liveness_probe = match kind {
        ProjectKind::Rest => "GET /healthz",
        ProjectKind::Rpc => "grpc_health_probe_liveness_or_generated_liveness_probe",
    };
    let workload_probe = match kind {
        ProjectKind::Rest => "representative_http_route_probe",
        ProjectKind::Rpc => "representative_rpc_method_probe",
    };

    format!(
        r#"# Generated by rozectl. Required for deployable production topology review.
service: {name}
boundary: {boundary}
endpoint_count: {endpoint_count}
mode: deployment_topology
container:
  port: {port}
  image:
    pin_by_digest: required
    mutable_tags_forbidden_in_production: true
    roze_git_revision_label_required: true
  command:
    generated_binary: true
    config_path_required: true
probes:
  startup:
    probe: {startup_probe}
    failure_threshold: platform_owner_defined
    evidence: startup_probe_output
  readiness:
    probe: {readiness_probe}
    must_turn_false_during_draining: true
    evidence: readiness_transition_output
  liveness:
    probe: {liveness_probe}
    must_not_replace_readiness: true
    evidence: liveness_probe_output
  workload:
    probe: {workload_probe}
    required_before_canary: true
resources:
  requests:
    cpu: owner_defined_after_capacity_plan
    memory: owner_defined_after_capacity_plan
  limits:
    cpu: owner_defined_or_platform_policy
    memory: required
  alerts:
    - cpu_saturation
    - memory_pressure
    - restart_loop
    - oom_killed
scaling:
  min_replicas: owner_defined_minimum_2_for_broad_production
  max_replicas: owner_defined
  hpa_or_platform_autoscaler_required: true
  signals:
    - request_or_call_rate
    - p99_latency
    - cpu
    - memory
    - queue_or_concurrency_when_available
  scale_down_stabilization_required: true
disruption_budget:
  required: true
  min_available_or_max_unavailable: owner_defined
  validates_drain_before_termination: true
termination:
  graceful_shutdown_required: true
  pre_stop_or_platform_drain_hook_required: true
  termination_grace_period: owner_defined_from_shutdown_soak
  readiness_false_before_signal: required
configuration:
  config_source:
    - generated_config_yaml
    - environment_overlay
    - config_center_when_enabled
  secrets:
    plain_text_secret_in_config_forbidden: true
    mounted_or_platform_secret_required: true
    rotation_plan_required: true
  config_reload:
    invalid_reload_rejected_or_rolled_back: true
    listener_failure_isolated: true
network:
  service_discovery:
    registry_or_platform_service_required: true
    endpoint_change_test_required: true
  ingress:
    rest_gateway_or_ingress_for_rest: required_when_public
    rpc_internal_lb_or_mesh_for_rpc: required_when_remote
  network_policy:
    default_deny_recommended: true
    declared_downstream_allowlist_required: true
  tls:
    external_tls_required: true
    mtls_required_for_internal_sensitive_paths: owner_defined
rollout:
  canary_required: true
  blue_green_or_rollback_required: true
  image_digest_pinned: true
  generated_assets_reviewed_before_rollout:
    - ops/production-gate.yaml
    - ops/release-rollout.yaml
    - ops/runtime-hardening.yaml
    - ops/observability-contract.yaml
    - ops/error-contract.yaml
tests:
  - test: probe_contract
    pass:
      - startup_probe_passes_after_boot
      - readiness_probe_fails_during_draining
      - liveness_probe_does_not_mask_dependency_failure
      - workload_probe_passes_before_canary
    evidence:
      - startup_probe_output
      - readiness_transition_output
      - workload_probe_output

  - test: resource_and_scaling
    pass:
      - resource_requests_and_limits_reviewed_against_capacity_plan
      - autoscaler_signal_is_bound_to_generated_metrics
      - scale_out_and_scale_in_have_timeline_evidence
    evidence:
      - capacity_plan_link
      - hpa_or_platform_policy
      - scaling_timeline

  - test: disruption_and_shutdown
    pass:
      - disruption_budget_prevents_total_outage
      - termination_grace_allows_drain_or_cancellation
      - final_metrics_logs_traces_are_flushed
    evidence:
      - disruption_test_result
      - shutdown_timeline
      - lifecycle_summary

  - test: config_secret_network
    pass:
      - secret_not_present_in_plain_config
      - config_reload_or_snapshot_restore_tested
      - network_policy_matches_dependency_inventory
      - registry_or_service_discovery_endpoint_change_tested
    evidence:
      - secret_scan_result
      - config_reload_result
      - network_policy_review
      - registry_change_test
promotion_required:
  probe_contract_passed: true
  resource_scaling_passed: true
  disruption_shutdown_passed: true
  config_secret_network_passed: true
  image_digest_pinned: true
  rollback_action_tested: true
  owner_signoff: required
blocking_findings:
  - production_image_not_pinned_by_digest
  - missing_readiness_or_startup_probe
  - liveness_used_as_readiness
  - no_resource_limit_or_memory_alert
  - no_disruption_budget_for_multi_replica_service
  - secret_in_plain_text_config
  - network_policy_missing_for_declared_downstream
  - rollback_action_not_tested
"#,
        name = spec.service,
        boundary = boundary,
        endpoint_count = endpoint_count,
        port = port,
        startup_probe = startup_probe,
        readiness_probe = readiness_probe,
        liveness_probe = liveness_probe,
        workload_probe = workload_probe,
    )
}

fn service_communication_yaml(spec: &ApiSpec, kind: ProjectKind) -> String {
    let boundary = match kind {
        ProjectKind::Rest => "rest",
        ProjectKind::Rpc => "rpc",
    };
    let endpoint_count = match kind {
        ProjectKind::Rest => spec.rest_routes.len(),
        ProjectKind::Rpc => spec.rpc_methods.len(),
    };
    let representative_call = match kind {
        ProjectKind::Rest => {
            "representative_http_handler_calls_declared_downstream_or_declares_none"
        }
        ProjectKind::Rpc => "representative_rpc_method_calls_declared_downstream_or_declares_none",
    };
    let client_stack = match kind {
        ProjectKind::Rest => "generated_rest_service_context_http_clients_or_gateway_clients",
        ProjectKind::Rpc => "generated_tonic_client_and_service_context_rpc_clients",
    };
    let latency_metric = match kind {
        ProjectKind::Rest => "roze_http_request_duration_seconds_bucket",
        ProjectKind::Rpc => "roze_rpc_method_duration_seconds_bucket",
    };

    format!(
        r#"# Generated by rozectl. Required for governed service-to-service calls.
service: {name}
boundary: {boundary}
endpoint_count: {endpoint_count}
mode: service_communication
client_stack: {client_stack}
downstream_inventory:
  required: true
  source:
    - service_context
    - generated_config
    - dependency_governance
  undeclared_downstream_forbidden: true
discovery:
  registry_required_when_dynamic: true
  static_endpoint_allowed_only_for_controlled_internal_or_local: true
  endpoint_change_test_required: true
  stale_endpoint_ttl_required: true
  evidence:
    - registry_change_test
    - endpoint_snapshot
load_balancing:
  required_for_multi_endpoint_dependency: true
  policy: owner_defined
  distribution_sample_required: true
  unhealthy_endpoint_excluded: true
  evidence:
    - load_balancing_distribution_sample
    - unhealthy_endpoint_ejection_result
client_deadlines:
  required: true
  configured_from_inbound_deadline_or_service_budget: true
  cancellation_propagates_to_downstream: true
  timeout_error_maps_to_error_contract: true
  evidence:
    - deadline_trace_sample
    - cancellation_log_sample
retries:
  retry_budget_required: true
  exponential_backoff_with_jitter_required: true
  retryable_errors_from_error_contract_only: true
  retryable_mutation_requires_idempotency_policy: true
  retry_storm_protection_required: true
  evidence:
    - retry_budget_metric_sample
    - retry_amplification_report
circuit_breaker:
  required_for_remote_dependency: true
  states:
    - closed
    - open
    - half_open
  half_open_probe_limit_required: true
  state_transition_metrics_required: true
  evidence:
    - breaker_transition_sample
    - dependency_failure_drill
outlier_handling:
  required_for_multi_endpoint_dependency: true
  eject_slow_or_failing_endpoint: owner_defined_policy
  reintroduce_after_probe_success: required
  evidence:
    - outlier_ejection_timeline
fallback:
  explicit_policy_required: true
  allowed:
    - fail_fast_with_typed_error
    - cached_or_stale_response_with_staleness_header
    - degraded_response_with_audit
    - queued_or_outbox_when_async_boundary
  forbidden:
    - silent_success
    - hidden_partial_write
    - fallback_without_metric
trace_context:
  propagate:
    - trace_id
    - span_id
    - request_id_or_correlation_id
    - deadline_or_timeout_budget
    - tenant_or_principal_when_allowed
  downstream_span_required: true
  logs_join_on_trace_id: true
metrics:
  service_latency_metric: {latency_metric}
  downstream_metric: roze_downstream_requests_total
  resilience_metric: roze_resilience_decisions_total
  required_labels:
    - service
    - dependency
    - operation
    - decision
    - error_code
tests:
  - test: downstream_inventory
    probe: {representative_call}
    pass:
      - every_downstream_is_declared_or_no_downstream_is_declared
      - config_contains_endpoint_or_registry_policy
      - undeclared_downstream_scan_passes
    evidence:
      - inventory_review
      - generated_config_snapshot

  - test: discovery_and_load_balancing
    pass:
      - endpoint_change_is_observed_without_restart_or_with_declared_restart
      - unhealthy_endpoint_is_not_selected
      - traffic_distribution_is_recorded
    evidence:
      - registry_change_test
      - load_balancing_distribution_sample
      - unhealthy_endpoint_test

  - test: deadline_retry_breaker
    pass:
      - downstream_timeout_observes_deadline
      - retries_stop_at_budget
      - breaker_opens_and_half_open_probe_is_bounded
      - typed_error_matches_error_contract
    evidence:
      - deadline_trace_sample
      - retry_budget_metric_sample
      - breaker_transition_sample

  - test: fallback_and_trace_context
    pass:
      - fallback_policy_is_explicit_or_declared_absent
      - degraded_response_is_visible_in_metrics
      - downstream_span_carries_trace_and_deadline
      - no_silent_success_for_failed_dependency
    evidence:
      - fallback_test_result
      - trace_query_sample
      - resilience_metric_sample
promotion_required:
  downstream_inventory_passed: true
  discovery_load_balancing_passed: true
  deadline_retry_breaker_passed: true
  fallback_trace_context_passed: true
  owner_signoff: required
blocking_findings:
  - undeclared_remote_downstream
  - dependency_without_deadline
  - retry_without_budget_or_jitter
  - breaker_without_transition_evidence
  - fallback_without_metric_or_trace
  - silent_success_on_dependency_failure
  - missing_trace_context_on_downstream_call
"#,
        name = spec.service,
        boundary = boundary,
        endpoint_count = endpoint_count,
        representative_call = representative_call,
        client_stack = client_stack,
        latency_metric = latency_metric,
    )
}

fn cache_governance_yaml(spec: &ApiSpec, kind: ProjectKind) -> String {
    let boundary = match kind {
        ProjectKind::Rest => "rest",
        ProjectKind::Rpc => "rpc",
    };
    let endpoint_count = match kind {
        ProjectKind::Rest => spec.rest_routes.len(),
        ProjectKind::Rpc => spec.rpc_methods.len(),
    };
    let read_probe = match kind {
        ProjectKind::Rest => "representative_http_query_declares_cache_policy_or_no_cache",
        ProjectKind::Rpc => "representative_rpc_query_declares_cache_policy_or_no_cache",
    };
    let mutation_probe = match kind {
        ProjectKind::Rest => "representative_http_mutation_declares_invalidation_or_no_cache",
        ProjectKind::Rpc => "representative_rpc_mutation_declares_invalidation_or_no_cache",
    };

    format!(
        r#"# Generated by rozectl. Required when generated services use local or remote caches.
service: {name}
boundary: {boundary}
endpoint_count: {endpoint_count}
mode: cache_governance
policy:
  cache_boundary_required: true
  no_cache_allowed_when_declared: true
  key_ownership:
    required: true
    owner: service_owner
    key_prefix: "{name}:{boundary}:"
    key_contains_raw_secret_or_token: forbidden
    key_contains_unbounded_user_input: forbidden
  layers:
    local_cache:
      allowed: true
      max_entries_required: true
      ttl_required: true
      stale_read_policy_required: true
    remote_cache:
      allowed: true
      timeout_required: true
      circuit_breaker_required: true
      fallback_policy_required: true
  ttl:
    required: true
    jitter_required: true
    zero_or_infinite_ttl_forbidden_without_owner_exception: true
    negative_cache_ttl_required_when_cache_miss_is_expensive: true
  consistency:
    owner_defined: true
    modes:
      - strong_read_through_after_write_when_required
      - eventual_with_staleness_bound
      - no_cache_for_mutation_sensitive_path
    stale_read_header_or_trace_attribute_required_when_serving_stale: true
  invalidation:
    mutation_invalidation_required: true
    event_driven_invalidation_allowed: true
    manual_purge_runbook_required: true
    bulk_purge_rate_limit_required: true
protection:
  penetration:
    negative_cache_required_for_repeated_absent_keys: true
    bloom_or_guard_filter_required_for_large_keyspace: owner_defined
    evidence: penetration_test_result
  breakdown:
    singleflight_or_request_collapse_required: true
    early_refresh_or_lock_required_for_hot_keys: owner_defined
    evidence: hot_key_test_result
  avalanche:
    ttl_jitter_required: true
    bulk_expiry_forbidden_without_refresh_plan: true
    evidence: expiry_spread_report
  stampede:
    concurrency_limit_required: true
    remote_cache_timeout_required: true
    fallback_must_not_hide_stale_or_partial_data: true
observability:
  metrics_required:
    - roze_cache_requests_total
    - roze_cache_hit_ratio
    - roze_cache_latency_seconds
    - roze_cache_errors_total
    - roze_singleflight_collapses_total
  labels:
    - service
    - boundary
    - cache_name
    - operation
    - result
  unbounded_key_label_forbidden: true
  alerts:
    - cache_hit_ratio_drop
    - cache_error_rate_high
    - remote_cache_latency_high
    - hot_key_collapse_spike
tests:
  - test: cache_boundary
    probes:
      - {read_probe}
      - {mutation_probe}
    pass:
      - every_cached_path_declares_key_ttl_owner_and_consistency
      - every_mutation_declares_invalidation_or_no_cache_impact
      - raw_secret_or_unbounded_input_is_not_used_as_key
    evidence:
      - cache_policy_review
      - key_ttl_table

  - test: penetration_breakdown_avalanche
    pass:
      - repeated_absent_key_does_not_hit_backend_unbounded
      - hot_key_collapse_or_singleflight_is_observed
      - ttl_jitter_spreads_expiry
      - remote_cache_timeout_does_not_exhaust_request_budget
    evidence:
      - penetration_test_result
      - hot_key_test_result
      - expiry_spread_report
      - timeout_trace_sample

  - test: invalidation_consistency
    pass:
      - write_updates_or_invalidates_affected_keys
      - stale_read_policy_is_visible_to_client_or_trace
      - manual_purge_runbook_is_tested
      - event_invalidation_is_idempotent_when_used
    evidence:
      - invalidation_test_result
      - stale_read_sample
      - purge_runbook_output

  - test: cache_observability
    pass:
      - hit_miss_error_latency_metrics_exported
      - cache_key_not_used_as_metric_label
      - singleflight_collapse_metric_available_when_used
      - alerts_attached_to_dashboard_or_runbook
    evidence:
      - cache_metric_sample
      - cardinality_report
      - alert_review
promotion_required:
  cache_boundary_passed: true
  penetration_breakdown_avalanche_passed: true
  invalidation_consistency_passed: true
  cache_observability_passed: true
  owner_signoff: required
blocking_findings:
  - cached_path_without_ttl_or_owner
  - cache_key_contains_secret_or_unbounded_input
  - mutation_without_cache_invalidation_policy
  - hot_key_without_singleflight_or_collapse
  - ttl_without_jitter_for_large_keyset
  - cache_metric_uses_raw_key_label
  - stale_data_served_without_declared_policy
"#,
        name = spec.service,
        boundary = boundary,
        endpoint_count = endpoint_count,
        read_probe = read_probe,
        mutation_probe = mutation_probe,
    )
}

fn data_access_governance_yaml(spec: &ApiSpec, kind: ProjectKind) -> String {
    let boundary = match kind {
        ProjectKind::Rest => "rest",
        ProjectKind::Rpc => "rpc",
    };
    let endpoint_count = match kind {
        ProjectKind::Rest => spec.rest_routes.len(),
        ProjectKind::Rpc => spec.rpc_methods.len(),
    };
    let read_probe = match kind {
        ProjectKind::Rest => {
            "representative_http_query_declares_data_access_policy_or_no_persistence"
        }
        ProjectKind::Rpc => {
            "representative_rpc_query_declares_data_access_policy_or_no_persistence"
        }
    };
    let mutation_probe = match kind {
        ProjectKind::Rest => "representative_http_mutation_declares_write_policy_or_no_persistence",
        ProjectKind::Rpc => "representative_rpc_mutation_declares_write_policy_or_no_persistence",
    };

    format!(
        r#"# Generated by rozectl. Required when generated services access databases or search stores.
service: {name}
boundary: {boundary}
endpoint_count: {endpoint_count}
mode: data_access_governance
persistence_boundary:
  declared_or_no_persistence_required: true
  allowed_stores:
    - sql
    - mongo
    - search
    - cache_as_read_model
  app_owned_queries_live_outside_generated_transport: true
query_policy:
  deadlines:
    required: true
    derived_from_request_or_call_deadline: true
    database_timeout_must_be_less_than_transport_timeout: true
  connection_pool:
    max_connections_required: true
    acquire_timeout_required: true
    idle_timeout_required: true
    pool_saturation_alert_required: true
  result_size:
    pagination_required_for_lists: true
    max_page_size_required: true
    unbounded_select_forbidden: true
    streaming_or_cursor_required_for_large_exports: true
  slow_query_budget:
    required: true
    p95_budget_owner_defined: true
    p99_budget_owner_defined: true
    explain_plan_required_for_hot_queries: true
  index_review:
    required_for_filters_sorts_and_joins: true
    missing_index_exception_requires_owner_signoff: true
    index_bloat_review_required_for_broad_production: true
write_policy:
  transaction_boundary_required_for_multi_statement_write: true
  idempotency_required_for_retryable_write: true
  optimistic_or_pessimistic_locking_policy_required_when_conflict_possible: true
  outbox_or_dtm_required_for_write_plus_event: true
  write_timeout_required: true
read_write_split:
  single_primary_allowed: true
  read_replica_allowed: true
  stale_read_policy_required_when_replica_used: true
  read_after_write_policy_required: true
n_plus_one_protection:
  required: true
  batching_or_join_plan_required_for_collection_expansion: true
  per_request_query_count_budget_required: true
  query_count_trace_attribute_required: true
security:
  tenant_filter_required_when_multi_tenant: true
  row_level_policy_or_repository_guard_required: owner_defined
  raw_sql_review_required: true
  pii_projection_review_required: true
observability:
  metrics_required:
    - roze_data_queries_total
    - roze_data_query_duration_seconds
    - roze_data_pool_acquire_seconds
    - roze_data_pool_in_use
    - roze_data_slow_queries_total
  labels:
    - service
    - boundary
    - store
    - operation
    - result
  raw_sql_or_bind_values_as_labels_forbidden: true
  traces:
    db_span_required: true
    query_name_required: true
    row_count_or_page_size_attribute_required: true
tests:
  - test: persistence_boundary
    probes:
      - {read_probe}
      - {mutation_probe}
    pass:
      - every_data_access_path_declares_store_or_no_persistence
      - generated_transport_does_not_hide_application_owned_query
      - tenant_or_permission_guard_is_declared_when_required
    evidence:
      - data_access_policy_review
      - repository_or_query_inventory

  - test: query_deadline_pool
    pass:
      - database_timeout_fires_before_transport_timeout
      - pool_acquire_timeout_is_observed
      - pool_saturation_alert_query_is_defined
      - cancellation_stops_expensive_query_or_marks_owner_exception
    evidence:
      - timeout_trace_sample
      - pool_saturation_sample
      - cancellation_test_result

  - test: slow_query_index_pagination
    pass:
      - hot_query_has_explain_plan_or_owner_exception
      - list_endpoint_or_method_has_page_limit
      - sort_filter_fields_have_index_review
      - unbounded_scan_is_absent_or_blocked
    evidence:
      - slow_query_report
      - explain_plan_sample
      - index_review
      - pagination_test_result

  - test: n_plus_one_and_write_policy
    pass:
      - collection_expansion_has_query_count_budget
      - multi_statement_write_has_transaction_boundary
      - retryable_write_has_idempotency_policy
      - write_plus_event_uses_outbox_or_dtm_or_declares_no_event
    evidence:
      - query_count_trace
      - transaction_test_result
      - idempotency_test_report
promotion_required:
  persistence_boundary_passed: true
  query_deadline_pool_passed: true
  slow_query_index_pagination_passed: true
  n_plus_one_write_policy_passed: true
  owner_signoff: required
blocking_findings:
  - data_access_without_timeout_or_deadline
  - unbounded_query_result
  - list_without_page_limit
  - hot_query_without_index_or_explain_review
  - n_plus_one_without_query_count_budget
  - pool_without_saturation_alert
  - multi_statement_write_without_transaction
  - raw_sql_without_review
"#,
        name = spec.service,
        boundary = boundary,
        endpoint_count = endpoint_count,
        read_probe = read_probe,
        mutation_probe = mutation_probe,
    )
}

fn interface_governance_yaml(spec: &ApiSpec, kind: ProjectKind) -> String {
    use std::fmt::Write as _;

    let boundary = match kind {
        ProjectKind::Rest => "rest",
        ProjectKind::Rpc => "rpc",
    };
    let endpoint_count = match kind {
        ProjectKind::Rest => spec.rest_routes.len(),
        ProjectKind::Rpc => spec.rpc_methods.len(),
    };

    let mut out = String::new();
    writeln!(
        &mut out,
        "# Generated by rozectl. Required when generated service interfaces are exposed."
    )
    .unwrap();
    writeln!(&mut out, "service: {}", spec.service).unwrap();
    writeln!(&mut out, "boundary: {boundary}").unwrap();
    writeln!(&mut out, "endpoint_count: {endpoint_count}").unwrap();
    writeln!(&mut out, "mode: interface_governance").unwrap();
    writeln!(&mut out, "source_contract: generated_from_idl").unwrap();

    match kind {
        ProjectKind::Rest => {
            writeln!(&mut out, "framework_endpoints:").unwrap();
            for (name, method, path, evidence) in [
                (
                    "liveness",
                    "GET",
                    rest::full_route_path(spec, "/healthz"),
                    "probe_returns_probe_report",
                ),
                (
                    "readiness",
                    "GET",
                    rest::full_route_path(spec, "/readyz"),
                    "drain_changes_readiness",
                ),
                (
                    "startup",
                    "GET",
                    rest::full_route_path(spec, "/startupz"),
                    "startup_state_reported",
                ),
                (
                    "metrics",
                    "GET",
                    rest::full_route_path(spec, "/metrics"),
                    "prometheus_scrapeable",
                ),
                (
                    "openapi",
                    "GET",
                    rest::full_route_path(spec, "/openapi.json"),
                    "openapi_contains_business_and_framework_interfaces",
                ),
                (
                    "report_export",
                    "GET",
                    rest::full_route_path(spec, "/reports/export"),
                    "smoke_export_request_returns_typed_response",
                ),
                (
                    "chart_query",
                    "GET",
                    rest::full_route_path(spec, "/charts/query"),
                    "smoke_chart_query_returns_typed_series_response",
                ),
            ] {
                writeln!(&mut out, "  - name: {name}").unwrap();
                writeln!(&mut out, "    method: {method}").unwrap();
                writeln!(&mut out, "    path: {path}").unwrap();
                writeln!(&mut out, "    owner: framework").unwrap();
                writeln!(&mut out, "    evidence: {evidence}").unwrap();
            }

            writeln!(&mut out, "business_endpoints:").unwrap();
            for route in &spec.rest_routes {
                writeln!(&mut out, "  - method: {}", http_method_name(&route.method)).unwrap();
                writeln!(
                    &mut out,
                    "    path: {}",
                    rest::full_route_path_for_route(spec, route)
                )
                .unwrap();
                writeln!(&mut out, "    request: {}", route.request).unwrap();
                writeln!(&mut out, "    response: {}", route.response).unwrap();
                writeln!(
                    &mut out,
                    "    handler: {}",
                    route
                        .handler
                        .as_deref()
                        .unwrap_or("generated_from_method_path")
                )
                .unwrap();
                writeln!(&mut out, "    owner: application_logic").unwrap();
                writeln!(&mut out, "    generated_boundary: route_handler_logic").unwrap();
            }

            writeln!(&mut out, "required_smoke:").unwrap();
            writeln!(&mut out, "  framework:").unwrap();
            for test in [
                "smoke_framework_healthz",
                "smoke_framework_readyz",
                "smoke_framework_startupz",
                "smoke_framework_metrics",
                "smoke_framework_openapi",
                "smoke_framework_report_export",
                "smoke_framework_chart_query",
            ] {
                writeln!(&mut out, "    - {test}").unwrap();
            }
            writeln!(&mut out, "  business_routes: generated_from_api_contract").unwrap();
        }
        ProjectKind::Rpc => {
            writeln!(&mut out, "rpc_methods:").unwrap();
            for method in &spec.rpc_methods {
                writeln!(&mut out, "  - name: {}", method.name).unwrap();
                writeln!(&mut out, "    request: {}", method.request).unwrap();
                writeln!(&mut out, "    response: {}", method.response).unwrap();
                writeln!(&mut out, "    owner: application_logic").unwrap();
                writeln!(
                    &mut out,
                    "    generated_boundary: tonic_server_client_adapter"
                )
                .unwrap();
            }
            writeln!(&mut out, "required_smoke:").unwrap();
            writeln!(&mut out, "  lifecycle: startup_readiness_metrics").unwrap();
            writeln!(&mut out, "  representative_rpc_call: required").unwrap();
            writeln!(&mut out, "  client_deadline_and_cancel: required").unwrap();
        }
    }

    writeln!(&mut out, "policy:").unwrap();
    writeln!(&mut out, "  compatibility_required: true").unwrap();
    writeln!(&mut out, "  versioning_required: true").unwrap();
    writeln!(
        &mut out,
        "  request_response_contract_review_required: true"
    )
    .unwrap();
    writeln!(&mut out, "  breaking_change_requires_owner_approval: true").unwrap();
    writeln!(
        &mut out,
        "  generated_openapi_or_proto_source_of_truth: true"
    )
    .unwrap();
    writeln!(&mut out, "  public_error_contract_required: true").unwrap();
    writeln!(&mut out, "  authn_authz_boundary_required: true").unwrap();
    writeln!(
        &mut out,
        "  idempotency_review_required_for_mutations: true"
    )
    .unwrap();
    writeln!(&mut out, "  pagination_required_for_list_endpoints: true").unwrap();
    writeln!(&mut out, "  deprecation_window_required: true").unwrap();
    writeln!(&mut out, "governance_requirements:").unwrap();
    writeln!(&mut out, "  timeout: required").unwrap();
    writeln!(&mut out, "  rate_limit: required").unwrap();
    writeln!(&mut out, "  circuit_breaker: required").unwrap();
    writeln!(&mut out, "  load_shedding: required").unwrap();
    writeln!(&mut out, "  retry_budget: required").unwrap();
    writeln!(&mut out, "  deadline_propagation: required").unwrap();
    writeln!(&mut out, "  auth_projection: required_or_declared_not_used").unwrap();
    writeln!(&mut out, "  typed_error_projection: required").unwrap();
    writeln!(
        &mut out,
        "  observability_labels: bounded_method_route_status"
    )
    .unwrap();
    writeln!(&mut out, "  trace_context: propagated").unwrap();
    writeln!(&mut out, "evidence_required:").unwrap();
    writeln!(&mut out, "  contract_diff: true").unwrap();
    writeln!(&mut out, "  compatibility_test: true").unwrap();
    writeln!(&mut out, "  framework_smoke: true").unwrap();
    writeln!(&mut out, "  auth_boundary_review: true").unwrap();
    writeln!(&mut out, "  error_mapping_review: true").unwrap();
    writeln!(&mut out, "  owner_signoff: required").unwrap();
    writeln!(&mut out, "blocking_findings:").unwrap();
    writeln!(&mut out, "  - undocumented_interface_change").unwrap();
    writeln!(&mut out, "  - breaking_change_without_version_or_exception").unwrap();
    writeln!(&mut out, "  - missing_auth_boundary").unwrap();
    writeln!(&mut out, "  - mutation_without_idempotency_review").unwrap();
    writeln!(&mut out, "  - list_endpoint_without_pagination_or_limit").unwrap();
    writeln!(&mut out, "  - endpoint_missing_smoke_test").unwrap();
    writeln!(&mut out, "  - openapi_or_proto_missing_interface").unwrap();
    writeln!(&mut out, "  - handler_bypasses_timeout_or_deadline").unwrap();
    writeln!(&mut out, "  - unbounded_label_cardinality").unwrap();
    writeln!(&mut out, "  - error_contract_mismatch").unwrap();
    out
}

fn build_rs() -> String {
    r#"fn main() -> Result<(), Box<dyn std::error::Error>> {
    let protoc = protoc_bin_vendored::protoc_bin_path()?;
    std::env::set_var("PROTOC", protoc);

    roze_grpc::build::compile(&["proto/service.proto"], &["proto"])?;

    println!("cargo:rerun-if-changed=proto/service.proto");
    Ok(())
}
"#
    .to_string()
}

fn config_yaml(spec: &ApiSpec, kind: ProjectKind) -> String {
    let governance_routes = governance_routes_yaml(spec, kind);
    match kind {
        ProjectKind::Rest => format!(
            r#"name: {}
rest:
  addr: 127.0.0.1:3000
  register: false
  middlewares:
    recover: true
    trace: true
    stat: true
    prometheus: true
    cors: true
    # cors_config:
    #   allow_origins: ["*"]
    #   allow_methods: ["GET", "POST", "PUT", "PATCH", "DELETE"]
    #   allow_headers: ["authorization", "content-type", "x-request-id", "x-trace-id"]
    #   expose_headers: ["x-request-id", "x-trace-id"]
    #   allow_credentials: false
    #   max_age_seconds: 3600
    timeout: true
    # max_conns: 1000
    # shedding:
    #   concurrency: 1000
    #   window_ms: 1000
    #   min_samples: 100
    #   max_avg_latency_ms: 500
    #   max_failure_ratio_per_mille: 500
    #   cool_down_ms: 1000
    # gunzip: true
    # request_body_limit_bytes: 2097152
registry:
  kind: memory
  endpoints: []
  # prefix: /roze/services
  # ttl_seconds: 10
  # renew_interval_secs: 3
governance:
  timeout_ms: 5000
  rate_limit:
    burst: 100
    refill_ms: 10
  breaker:
    failure_threshold: 5
    reset_timeout_ms: 30000
  shedding:
    concurrency: 1000
    window_ms: 1000
    min_samples: 100
    max_avg_latency_ms: 500
    max_failure_ratio_per_mille: 500
    cool_down_ms: 1000
  fallback:
    enabled: false
    status: 503
    body:
      code: 503
      message: degraded
    headers:
      x-roze-fallback: governance
  routes:
{}
# rpc_client:
#   endpoints: ["127.0.0.1:4000"]
#   # target: dns:///user.rpc
#   # app: app-name
#   # token: change-me
#   # non_block: false
#   timeout_ms: 2000
#   keepalive_time_secs: 20
#   balancer: power_of_two_choices # first_available, round_robin, weighted_round_robin, power_of_two_choices, health_aware
#   middlewares:
#     trace: true
#     recover: true
#     stat: true
#     prometheus: true
#     breaker: true
# cache:
#   url: redis://127.0.0.1/
#   namespace: {}
# nats:
#   servers: ["127.0.0.1:4222"]
#   client_name: {}
#   subject_prefix: {}
#   jetstream:
#     stream: ROZE
#     subjects: ["{}.*"]
#     durable: {}
# outbox:
#   enabled: true
#   batch_size: 100
#   interval_ms: 1000
# auth:
#   jwt_secret: change-me
#   jwt_issuer: {}
#   jwt_expiration_secs: 86400
# telemetry:
#   name: {}
#   endpoint: http://127.0.0.1:4317
#   sampler: 1.0
#   batcher: otlpgrpc # otlpgrpc or otlphttp
"#,
            spec.service,
            governance_routes,
            spec.service,
            spec.service,
            spec.service,
            spec.service,
            spec.service,
            spec.service,
            spec.service
        ),
        ProjectKind::Rpc => format!(
            r#"name: {}
rpc:
  addr: 127.0.0.1:4000
  # advertise_addr: 127.0.0.1:4000
registry:
  kind: memory
  endpoints: []
  # prefix: /roze/services
  # ttl_seconds: 10
  # renew_interval_secs: 3
governance:
  timeout_ms: 5000
  rate_limit:
    burst: 100
    refill_ms: 10
  breaker:
    failure_threshold: 5
    reset_timeout_ms: 30000
  shedding:
    concurrency: 1000
    window_ms: 1000
    min_samples: 100
    max_avg_latency_ms: 500
    max_failure_ratio_per_mille: 500
    cool_down_ms: 1000
  fallback:
    enabled: false
    status: 503
    body:
      code: 503
      message: degraded
    headers:
      x-roze-fallback: governance
  routes:
{}
# database:
#   url: postgres://postgres:postgres@127.0.0.1:5432/{}
#   # policy: round-robin # round-robin or random
#   # replicas:
#   #   - postgres://postgres:postgres@127.0.0.1:5432/{}_replica
# mongo:
#   url: mongodb://127.0.0.1:27017
#   database: {}
# rpc_client:
#   endpoints: ["127.0.0.1:4000"]
#   # target: dns:///user.rpc
#   # app: app-name
#   # token: change-me
#   # non_block: false
#   timeout_ms: 2000
#   keepalive_time_secs: 20
#   balancer: power_of_two_choices # first_available, round_robin, weighted_round_robin, power_of_two_choices, health_aware
#   middlewares:
#     trace: true
#     recover: true
#     stat: true
#     prometheus: true
#     breaker: true
# cache:
#   url: redis://127.0.0.1/
#   namespace: {}
# nats:
#   servers: ["127.0.0.1:4222"]
#   client_name: {}
#   subject_prefix: {}
#   jetstream:
#     stream: ROZE
#     subjects: ["{}.*"]
#     durable: {}
# outbox:
#   enabled: true
#   batch_size: 100
#   interval_ms: 1000
# auth:
#   jwt_secret: change-me
#   jwt_issuer: {}
#   jwt_expiration_secs: 86400
# telemetry:
#   name: {}
#   endpoint: http://127.0.0.1:4317
#   sampler: 1.0
#   batcher: otlpgrpc # otlpgrpc or otlphttp
"#,
            spec.service,
            governance_routes,
            spec.service,
            spec.service,
            spec.service,
            spec.service,
            spec.service,
            spec.service,
            spec.service,
            spec.service,
            spec.service,
            spec.service
        ),
    }
}

fn governance_routes_yaml(spec: &ApiSpec, kind: ProjectKind) -> String {
    use std::fmt::Write as _;

    let mut out = String::new();
    match kind {
        ProjectKind::Rest => {
            for route in &spec.rest_routes {
                let key = rest_governance_key(route);
                let retry_attempts = match route.method {
                    HttpMethod::Get | HttpMethod::Head => 2,
                    HttpMethod::Post | HttpMethod::Put | HttpMethod::Patch | HttpMethod::Delete => {
                        1
                    }
                };
                writeln!(&mut out, "    {key}:").unwrap();
                writeln!(&mut out, "      timeout_ms: 5000").unwrap();
                writeln!(&mut out, "      retry:").unwrap();
                writeln!(&mut out, "        max_attempts: {retry_attempts}").unwrap();
                writeln!(&mut out, "        backoff_ms: 50").unwrap();
                writeln!(&mut out, "        max_backoff_ms: 500").unwrap();
                writeln!(&mut out, "        budget_percent: 10").unwrap();
                writeln!(&mut out, "      rate_limit:").unwrap();
                writeln!(&mut out, "        burst: 100").unwrap();
                writeln!(&mut out, "        refill_ms: 10").unwrap();
                writeln!(&mut out, "      breaker:").unwrap();
                writeln!(&mut out, "        failure_threshold: 5").unwrap();
                writeln!(&mut out, "        reset_timeout_ms: 30000").unwrap();
                writeln!(&mut out, "      shedding:").unwrap();
                writeln!(&mut out, "        concurrency: 100").unwrap();
                writeln!(&mut out, "        window_ms: 1000").unwrap();
                writeln!(&mut out, "        min_samples: 100").unwrap();
                writeln!(&mut out, "        max_avg_latency_ms: 500").unwrap();
                writeln!(&mut out, "        max_failure_ratio_per_mille: 500").unwrap();
                writeln!(&mut out, "        cool_down_ms: 1000").unwrap();
                writeln!(&mut out, "      fallback:").unwrap();
                writeln!(&mut out, "        enabled: false").unwrap();
                writeln!(&mut out, "        status: 503").unwrap();
                writeln!(&mut out, "        body:").unwrap();
                writeln!(&mut out, "          code: 503").unwrap();
                writeln!(&mut out, "          message: degraded").unwrap();
                writeln!(&mut out, "        headers:").unwrap();
                writeln!(&mut out, "          x-roze-fallback: route").unwrap();
            }
        }
        ProjectKind::Rpc => {
            for method in &spec.rpc_methods {
                writeln!(&mut out, "    {}:", method.name).unwrap();
                writeln!(&mut out, "      timeout_ms: 5000").unwrap();
                writeln!(&mut out, "      retry:").unwrap();
                writeln!(&mut out, "        max_attempts: 2").unwrap();
                writeln!(&mut out, "        backoff_ms: 50").unwrap();
                writeln!(&mut out, "        max_backoff_ms: 500").unwrap();
                writeln!(&mut out, "        budget_percent: 10").unwrap();
                writeln!(&mut out, "      rate_limit:").unwrap();
                writeln!(&mut out, "        burst: 100").unwrap();
                writeln!(&mut out, "        refill_ms: 10").unwrap();
                writeln!(&mut out, "      breaker:").unwrap();
                writeln!(&mut out, "        failure_threshold: 5").unwrap();
                writeln!(&mut out, "        reset_timeout_ms: 30000").unwrap();
                writeln!(&mut out, "      shedding:").unwrap();
                writeln!(&mut out, "        concurrency: 100").unwrap();
                writeln!(&mut out, "        window_ms: 1000").unwrap();
                writeln!(&mut out, "        min_samples: 100").unwrap();
                writeln!(&mut out, "        max_avg_latency_ms: 500").unwrap();
                writeln!(&mut out, "        max_failure_ratio_per_mille: 500").unwrap();
                writeln!(&mut out, "        cool_down_ms: 1000").unwrap();
                writeln!(&mut out, "      fallback:").unwrap();
                writeln!(&mut out, "        enabled: false").unwrap();
                writeln!(&mut out, "        status: 503").unwrap();
                writeln!(&mut out, "        body:").unwrap();
                writeln!(&mut out, "          code: 503").unwrap();
                writeln!(&mut out, "          message: degraded").unwrap();
                writeln!(&mut out, "        headers:").unwrap();
                writeln!(&mut out, "          x-roze-fallback: method").unwrap();
            }
        }
    }
    if out.is_empty() {
        "    {}".to_string()
    } else {
        out.trim_end().to_string()
    }
}

fn rest_governance_key(route: &crate::parser::RestRoute) -> String {
    route
        .handler
        .as_deref()
        .map(to_snake_case)
        .unwrap_or_else(|| {
            to_snake_case(&rest::handler_name_for_openapi(&route.method, &route.path))
        })
}

fn api_template(service: &str) -> String {
    format!(
        r#"service {service} {{
    @server (
        prefix: /api
        group: {group}
    )
    post /{group}/login (LoginReq) returns (LoginResp)
}}

type LoginReq {{
    username: string `json:"username"`
    password: string `json:"password"`
}}

type LoginResp {{
    token: string
    expiresAt: u64
}}
"#,
        service = service,
        group = sanitize_group(service),
    )
}

fn rpc_template(service: &str) -> String {
    format!(
        r#"service {service} {{
    @server (
        group: {group}
    )
    rpc Get{pascal} (Get{pascal}Req) returns (Get{pascal}Resp)
}}

type Get{pascal}Req {{
    id: u64
}}

type Get{pascal}Resp {{
    id: u64
    name: string
}}
"#,
        service = service,
        group = sanitize_group(service),
        pascal = to_pascal_case(service),
    )
}

fn sanitize_group(service: &str) -> String {
    to_snake_case(service)
}

fn method_name(method: &crate::parser::HttpMethod) -> &'static str {
    match method {
        crate::parser::HttpMethod::Get => "GET",
        crate::parser::HttpMethod::Head => "HEAD",
        crate::parser::HttpMethod::Post => "POST",
        crate::parser::HttpMethod::Put => "PUT",
        crate::parser::HttpMethod::Patch => "PATCH",
        crate::parser::HttpMethod::Delete => "DELETE",
    }
}

fn register_workspace_member(out: &Path) -> anyhow::Result<()> {
    let Some(workspace_root) = find_workspace_root(out)? else {
        return Ok(());
    };
    let relative = out
        .strip_prefix(&workspace_root)
        .map(Path::to_path_buf)
        .unwrap_or_else(|_| out.to_path_buf());

    let relative = relative.to_string_lossy().replace('\\', "/");
    if relative.starts_with("../") || relative.starts_with('/') || relative.contains(":") {
        return Ok(());
    }

    let cargo_toml = workspace_root.join("Cargo.toml");
    let mut content = fs::read_to_string(&cargo_toml)
        .with_context(|| format!("failed to read {}", cargo_toml.display()))?;
    if content.contains(&format!(r#""{}""#, relative)) {
        return Ok(());
    }

    let marker = "members = [";
    let start = content
        .find(marker)
        .ok_or_else(|| anyhow::anyhow!("workspace members block not found"))?;
    let after = start + marker.len();
    let rest = &content[after..];
    let end = rest
        .find("]")
        .ok_or_else(|| anyhow::anyhow!("workspace members block not closed"))?;
    let insert_at = after + end;
    content.insert_str(insert_at, &format!("    \"{}\",\n", relative));
    fs::write(&cargo_toml, content)
        .with_context(|| format!("failed to update {}", cargo_toml.display()))?;
    Ok(())
}

pub(super) fn find_workspace_root(out: &Path) -> anyhow::Result<Option<PathBuf>> {
    let absolute_out = if out.is_absolute() {
        out.to_path_buf()
    } else {
        std::env::current_dir()?.join(out)
    };

    for directory in absolute_out.ancestors() {
        let manifest = directory.join("Cargo.toml");
        if !manifest.is_file() {
            continue;
        }
        let content = fs::read_to_string(&manifest)
            .with_context(|| format!("failed to read {}", manifest.display()))?;
        if content.lines().any(|line| line.trim() == "[workspace]") {
            return Ok(Some(directory.to_path_buf()));
        }
    }

    Ok(None)
}

fn config_rs() -> String {
    r#"pub type Config = roze_config::ServiceConfig;

pub fn load(path: impl AsRef<std::path::Path>) -> Result<Config, config::ConfigError> {
    roze_config::load(path)
}
"#
    .to_string()
}

fn service_context_rs(kind: ProjectKind) -> String {
    match kind {
        ProjectKind::Rest => rest_service_context_rs(&[]),
        ProjectKind::Rpc => rpc_service_context_rs(),
    }
}

fn rest_service_context_rs(rpc_clients: &[RpcClientBinding]) -> String {
    let mut out = r#"#![allow(dead_code)]

use std::sync::Arc;

use crate::config::Config;

#[derive(Clone, Debug)]
pub struct ServiceContext {
    pub config: Config,
    pub health: roze_health::HealthRegistry,
    pub cache: Option<roze_cache::RedisCache>,
    pub mq: Option<Arc<roze_nats::NatsJetStream>>,
    pub storage: Option<Arc<dyn roze_storage::ObjectStorage>>,
    pub outbox: Arc<dyn roze_transaction::OutboxStore>,
    pub idempotency: Arc<dyn roze_middleware::IdempotencyStore>,
}

impl ServiceContext {
    pub async fn new(config: Config) -> anyhow::Result<Self> {
        let health = roze_health::HealthRegistry::new();
        let cache = match config.cache.as_ref() {
            Some(cache) => Some(
                roze_cache::RedisCache::connect(&roze_cache::CacheConfig {
                    url: cache.url.clone(),
                    namespace: cache.namespace.clone(),
                    default_ttl_secs: cache.default_ttl_secs,
                })
                .await?,
            ),
            None => None,
        };
        let mq = match config.nats.as_ref() {
            Some(nats) => Some(Arc::new(roze_nats::NatsJetStream::connect(nats.clone()).await?)),
            None => None,
        };
        let storage = match config.storage.clone() {
            Some(storage) => Some(Arc::from(roze_storage::build_storage(storage)?)),
            None => None,
        };
        if cache.is_some() {
            health.register_static(roze_health::HealthCheck::healthy("redis"));
        }
        if mq.is_some() {
            health.register_static(roze_health::HealthCheck::healthy("nats"));
        }
        health.mark_ready();
        Ok(Self {
            config,
            health,
            cache,
            mq,
            storage,
            outbox: Arc::new(roze_transaction::InMemoryOutbox::new()),
            idempotency: Arc::new(roze_middleware::InMemoryIdempotencyStore::default()),
        })
    }

    pub fn with_outbox_store(
        mut self,
        outbox: Arc<dyn roze_transaction::OutboxStore>,
    ) -> Self {
        self.outbox = outbox;
        self
    }

    pub fn with_idempotency_store(
        mut self,
        idempotency: Arc<dyn roze_middleware::IdempotencyStore>,
    ) -> Self {
        self.idempotency = idempotency;
        self
    }

    pub fn with_storage(
        mut self,
        storage: Arc<dyn roze_storage::ObjectStorage>,
    ) -> Self {
        self.storage = Some(storage);
        self
    }

    pub fn storage(&self) -> anyhow::Result<Arc<dyn roze_storage::ObjectStorage>> {
        self.storage
            .clone()
            .ok_or_else(|| anyhow::anyhow!("object storage is not configured"))
    }

    pub async fn media_url(
        &self,
        key: &str,
        expires: std::time::Duration,
    ) -> anyhow::Result<roze_storage::MediaUrl> {
        roze_storage::resolve_media_url(self.storage()?.as_ref(), key, expires).await
    }

    pub fn jwt_config(&self) -> Option<roze_jwt::JwtConfig> {
        self.config.auth.as_ref().map(Into::into)
    }

    pub fn mq(&self) -> anyhow::Result<Arc<roze_nats::NatsJetStream>> {
        self.mq
            .clone()
            .ok_or_else(|| anyhow::anyhow!("nats jetstream is not configured"))
    }
"#
    .to_string();

    for client in rpc_clients {
        out.push_str(&format!(
            r#"

    pub async fn {name}(&self) -> anyhow::Result<{crate_name}::client::RpcClient> {{
        let config = self
            .config
            .rpc_client_config("{name}")
            .ok_or_else(|| anyhow::anyhow!("rpc client `{name}` is not configured"))?;
        {crate_name}::client::RpcClient::connect_from_config(config).await
    }}
"#,
            name = rust_identifier(&client.name),
            crate_name = client.crate_name
        ));
    }

    out.push_str(
        r#"}
"#,
    );
    out
}

fn rpc_service_context_rs() -> String {
    r#"#![allow(dead_code)]

use std::sync::Arc;

use crate::config::Config;

#[derive(Clone, Debug)]
pub struct ServiceContext {
    pub config: Config,
    pub health: roze_health::HealthRegistry,
    pub db_connections: Option<roze_db::DatabaseConnections>,
    pub mongo: Option<roze_mongo::MongoDatabase>,
    pub cache: Option<roze_cache::RedisCache>,
    pub mq: Option<Arc<roze_nats::NatsJetStream>>,
    pub storage: Option<Arc<dyn roze_storage::ObjectStorage>>,
    pub outbox: Arc<dyn roze_transaction::OutboxStore>,
    pub idempotency: Arc<dyn roze_middleware::IdempotencyStore>,
}

impl ServiceContext {
    pub async fn new(config: Config) -> anyhow::Result<Self> {
        let health = roze_health::HealthRegistry::new();
        let db_connections = roze_db::connect_connections_optional(config.database.as_ref()).await?;
        let mongo = roze_mongo::connect_optional(config.mongo.as_ref()).await?;
        let cache = match config.cache.as_ref() {
            Some(cache) => Some(
                roze_cache::RedisCache::connect(&roze_cache::CacheConfig {
                    url: cache.url.clone(),
                    namespace: cache.namespace.clone(),
                    default_ttl_secs: cache.default_ttl_secs,
                })
                .await?,
            ),
            None => None,
        };
        let mq = match config.nats.as_ref() {
            Some(nats) => Some(Arc::new(roze_nats::NatsJetStream::connect(nats.clone()).await?)),
            None => None,
        };
        let storage = match config.storage.clone() {
            Some(storage) => Some(Arc::from(roze_storage::build_storage(storage)?)),
            None => None,
        };
        if db_connections.is_some() {
            health.register_static(roze_health::HealthCheck::healthy("database"));
        }
        if mongo.is_some() {
            health.register_static(roze_health::HealthCheck::healthy("mongo"));
        }
        if cache.is_some() {
            health.register_static(roze_health::HealthCheck::healthy("redis"));
        }
        if mq.is_some() {
            health.register_static(roze_health::HealthCheck::healthy("nats"));
        }
        health.mark_ready();
        Ok(Self {
            config,
            health,
            db_connections,
            mongo,
            cache,
            mq,
            storage,
            outbox: Arc::new(roze_transaction::InMemoryOutbox::new()),
            idempotency: Arc::new(roze_middleware::InMemoryIdempotencyStore::default()),
        })
    }

    pub fn with_outbox_store(
        mut self,
        outbox: Arc<dyn roze_transaction::OutboxStore>,
    ) -> Self {
        self.outbox = outbox;
        self
    }

    pub fn with_idempotency_store(
        mut self,
        idempotency: Arc<dyn roze_middleware::IdempotencyStore>,
    ) -> Self {
        self.idempotency = idempotency;
        self
    }

    pub fn with_storage(
        mut self,
        storage: Arc<dyn roze_storage::ObjectStorage>,
    ) -> Self {
        self.storage = Some(storage);
        self
    }

    pub fn storage(&self) -> anyhow::Result<Arc<dyn roze_storage::ObjectStorage>> {
        self.storage
            .clone()
            .ok_or_else(|| anyhow::anyhow!("object storage is not configured"))
    }

    pub async fn media_url(
        &self,
        key: &str,
        expires: std::time::Duration,
    ) -> anyhow::Result<roze_storage::MediaUrl> {
        roze_storage::resolve_media_url(self.storage()?.as_ref(), key, expires).await
    }

    pub fn read_db(&self) -> anyhow::Result<&roze_db::DatabaseConnection> {
        self.db_connections
            .as_ref()
            .map(|connections| connections.read())
            .ok_or_else(|| anyhow::anyhow!("database connection is not configured"))
    }

    pub fn write_db(&self) -> anyhow::Result<&roze_db::DatabaseConnection> {
        self.db_connections
            .as_ref()
            .map(|connections| connections.write())
            .ok_or_else(|| anyhow::anyhow!("database connection is not configured"))
    }

    pub fn jwt_config(&self) -> Option<roze_jwt::JwtConfig> {
        self.config.auth.as_ref().map(Into::into)
    }

    pub fn mq(&self) -> anyhow::Result<Arc<roze_nats::NatsJetStream>> {
        self.mq
            .clone()
            .ok_or_else(|| anyhow::anyhow!("nats jetstream is not configured"))
    }
}
"#
    .to_string()
}

fn render_pb(spec: &ApiSpec) -> String {
    let package = to_snake_case(&spec.service);
    format!(
        r#"pub mod {package} {{
    roze_grpc::include_proto!("{package}");
}}
"#
    )
}

pub fn to_pascal_case(input: &str) -> String {
    input
        .split(['_', '-', ' '])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

pub fn to_snake_case(input: &str) -> String {
    let mut out = String::new();
    for (idx, ch) in input.chars().enumerate() {
        if ch.is_uppercase() && idx > 0 {
            out.push('_');
        } else if ch == '-' || ch == ' ' {
            out.push('_');
            continue;
        }
        out.push(ch.to_ascii_lowercase());
    }
    out
}

pub fn rust_identifier(input: &str) -> String {
    let ident = to_snake_case(input);
    if is_rust_keyword(&ident) {
        format!("r#{ident}")
    } else {
        ident
    }
}

fn is_rust_keyword(ident: &str) -> bool {
    matches!(
        ident,
        "as" | "break"
            | "const"
            | "continue"
            | "crate"
            | "else"
            | "enum"
            | "extern"
            | "false"
            | "fn"
            | "for"
            | "if"
            | "impl"
            | "in"
            | "let"
            | "loop"
            | "match"
            | "mod"
            | "move"
            | "mut"
            | "pub"
            | "ref"
            | "return"
            | "self"
            | "Self"
            | "static"
            | "struct"
            | "super"
            | "trait"
            | "true"
            | "type"
            | "unsafe"
            | "use"
            | "where"
            | "while"
            | "async"
            | "await"
            | "dyn"
            | "abstract"
            | "become"
            | "box"
            | "do"
            | "final"
            | "macro"
            | "override"
            | "priv"
            | "try"
            | "typeof"
            | "unsized"
            | "virtual"
            | "yield"
    )
}

fn render_proto(spec: &ApiSpec) -> anyhow::Result<String> {
    let package = to_snake_case(&spec.service);
    let known_types = spec
        .types
        .iter()
        .map(|ty| ty.name.as_str())
        .collect::<HashSet<_>>();
    let mut out = format!("syntax = \"proto3\";\n\npackage {};\n\n", package);

    out.push_str(&format!("service {} {{\n", to_pascal_case(&spec.service)));
    for route in &spec.rest_routes {
        let rpc_name = route.handler.clone().unwrap_or_else(|| {
            let method = match route.method {
                crate::parser::HttpMethod::Get => "get",
                crate::parser::HttpMethod::Head => "head",
                crate::parser::HttpMethod::Post => "post",
                crate::parser::HttpMethod::Put => "put",
                crate::parser::HttpMethod::Patch => "patch",
                crate::parser::HttpMethod::Delete => "delete",
            };
            format!("{}_{}", method, route_name_from_path(&route.path))
        });
        out.push_str(&format!(
            "  rpc {} ({}) returns ({});\n",
            to_pascal_case(&rpc_name),
            route.request,
            route.response
        ));
    }
    for method in &spec.rpc_methods {
        out.push_str(&format!(
            "  rpc {} ({}) returns ({});\n",
            to_pascal_case(&method.name),
            method.request,
            method.response
        ));
    }
    out.push_str("}\n\n");

    for ty in &spec.types {
        out.push_str(&format!("message {} {{\n", ty.name));
        for (idx, field) in ty.fields.iter().enumerate() {
            let name = field
                .json_name
                .clone()
                .unwrap_or_else(|| to_snake_case(&field.name));
            out.push_str(&format!(
                "  {} {} = {};\n",
                proto_type(&field.ty, &known_types)?,
                name,
                idx + 1
            ));
        }
        out.push_str("}\n\n");
    }

    Ok(out)
}

fn route_name_from_path(path: &str) -> String {
    path.trim_matches('/')
        .replace(':', "")
        .replace(['{', '}'], "")
        .replace(['/', '-'], "_")
}

fn proto_type(ty: &str, known_types: &HashSet<&str>) -> anyhow::Result<String> {
    if let Some(inner) = collection_element_type(ty) {
        return Ok(format!("repeated {}", proto_type(&inner, known_types)?));
    }
    if let Some((key, value)) = map_key_value_types(ty) {
        return Ok(format!(
            "map<{}, {}>",
            proto_type(&key, known_types)?,
            proto_type(&value, known_types)?
        ));
    }
    let proto = match ty {
        "String" | "string" => "string",
        "bool" => "bool",
        "bytes" => "bytes",
        "i32" | "int32" => "int32",
        "i64" | "int" | "int64" => "int64",
        "u32" | "uint32" => "uint32",
        "u64" | "uint" | "uint64" => "uint64",
        "f32" | "float" => "float",
        "f64" | "double" => "double",
        known if known_types.contains(known) => known,
        other => anyhow::bail!("unsupported proto field type `{other}`"),
    };
    Ok(proto.to_string())
}

#[cfg(test)]
mod tests {
    use crate::parser::{parse_api, HttpMethod};

    use super::*;

    #[test]
    fn generated_service_context_accepts_persistent_outbox_store() {
        for rendered in [rest_service_context_rs(&[]), rpc_service_context_rs()] {
            assert!(rendered.contains("Arc<dyn roze_transaction::OutboxStore>"));
            assert!(rendered.contains("pub fn with_outbox_store"));
            assert!(rendered.contains("Arc::new(roze_transaction::InMemoryOutbox::new())"));
            assert!(rendered.contains("Arc<dyn roze_middleware::IdempotencyStore>"));
            assert!(rendered.contains("pub fn with_idempotency_store"));
            assert!(rendered.contains("Arc<dyn roze_storage::ObjectStorage>"));
            assert!(rendered.contains("pub fn with_storage"));
            assert!(rendered.contains("pub async fn media_url"));
        }
    }

    fn temp_test_root(prefix: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "{}-{}-{}",
            prefix,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time")
                .as_nanos()
        ))
    }

    fn repo_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("apps directory")
            .parent()
            .expect("repo root")
            .to_path_buf()
    }

    fn generated_compile_workspace(prefix: &str) -> PathBuf {
        let root = temp_test_root(prefix);
        fs::create_dir_all(root.join("apps")).expect("create apps dir");
        let repo_manifest =
            fs::read_to_string(repo_root().join("Cargo.toml")).expect("read repo manifest");
        let workspace_tail = repo_manifest
            .find("[workspace.package]")
            .map(|idx| &repo_manifest[idx..])
            .expect("workspace package section");
        fs::write(
            root.join("Cargo.toml"),
            format!("[workspace]\nmembers = [\n]\nresolver = \"2\"\n\n{workspace_tail}"),
        )
        .expect("write temp workspace manifest");
        link_or_copy_crates(&repo_root().join("crates"), &root.join("crates"));
        root
    }

    #[cfg(unix)]
    fn link_or_copy_crates(src: &Path, dst: &Path) {
        std::os::unix::fs::symlink(src, dst).expect("symlink crates");
    }

    #[cfg(not(unix))]
    fn link_or_copy_crates(src: &Path, dst: &Path) {
        copy_dir_recursive(src, dst).expect("copy crates");
    }

    fn cargo_check_generated(manifest: &Path) {
        cargo_generated(manifest, "check", &["--quiet"]);
    }

    fn cargo_clippy_generated(manifest: &Path) {
        cargo_generated(
            manifest,
            "clippy",
            &["--all-targets", "--", "-D", "warnings"],
        );
    }

    fn cargo_generated(manifest: &Path, command: &str, args: &[&str]) {
        let output = std::process::Command::new("cargo")
            .arg(command)
            .arg("--manifest-path")
            .arg(manifest)
            .args(args)
            .output()
            .unwrap_or_else(|err| panic!("run cargo {command}: {err}"));
        assert!(
            output.status.success(),
            "cargo {command} failed for {}\nstdout:\n{}\nstderr:\n{}",
            manifest.display(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn read_api_source_expands_import_blocks() {
        let root = temp_test_root("roze-import-block");
        std::fs::create_dir_all(&root).expect("create temp root");
        std::fs::write(
            root.join("types.api"),
            r#"
            type GetUserReq {
                id u64 `path:"id"`
            }

            type UserResp {
                id u64 `json:"id"`
            }
            "#,
        )
        .expect("write imported api");
        std::fs::write(
            root.join("main.api"),
            r#"
            import (
                "types.api"
            )

            service user-api {
                get /users/:id (GetUserReq) returns (UserResp)
            }
            "#,
        )
        .expect("write main api");

        let source = read_api_source(&root.join("main.api")).expect("read api source");
        let spec = parse_api(&source).expect("parse expanded api");

        assert_eq!(spec.types.len(), 2);
        assert_eq!(spec.rest_routes.len(), 1);

        std::fs::remove_dir_all(root).expect("cleanup temp root");
    }

    #[test]
    fn api_generation_turns_rpc_imports_into_named_clients() {
        let root = temp_test_root("roze-api-rpc-client-imports");
        let admin = root.join("shop-admin-api");
        let order = root.join("shop-order-rpc");
        let payment = root.join("shop-payment-rpc");
        std::fs::create_dir_all(&admin).expect("create admin dir");
        std::fs::create_dir_all(&order).expect("create order dir");
        std::fs::create_dir_all(&payment).expect("create payment dir");
        std::fs::write(root.join("Cargo.toml"), "[workspace]\nmembers = []\n")
            .expect("write workspace manifest");
        std::fs::write(
            order.join("order.api"),
            r#"
            service order-rpc {
                rpc GetOrder (GetOrderReq) returns (GetOrderResp)
            }

            type GetOrderReq {
                id: u64
            }

            type GetOrderResp {
                id: u64
            }
            "#,
        )
        .expect("write order api");
        std::fs::write(
            payment.join("payment.api"),
            r#"
            service payment-rpc {
                rpc GetPayment (GetPaymentReq) returns (GetPaymentResp)
            }

            type GetPaymentReq {
                id: u64
            }

            type GetPaymentResp {
                id: u64
            }
            "#,
        )
        .expect("write payment api");
        let api = admin.join("admin.api");
        std::fs::write(
            &api,
            r#"
            import (
                "../shop-order-rpc/order.api"
                "../shop-payment-rpc/payment.api"
            )

            service shop-admin-api {
                get /health returns (HealthResp)
            }

            type HealthResp {
                ok: bool
            }
            "#,
        )
        .expect("write admin api");

        let source = read_api_source(&api).expect("read api source");
        let spec = parse_api(&source).expect("parse api");
        validate_project_kind(&spec, ProjectKind::Rest).expect("pure REST api");
        let clients = read_api_rpc_client_bindings(&api).expect("rpc client imports");

        assert_eq!(
            clients
                .iter()
                .map(|client| client.name.as_str())
                .collect::<Vec<_>>(),
            vec!["order", "payment"]
        );

        generate_rest_project_with_rpc_clients(
            &spec,
            &admin,
            GenerateOptions::new(GenerateMode::Update, DependencySource::Git),
            &clients,
        )
        .expect("generate admin api");

        let svc = std::fs::read_to_string(admin.join("src/svc/mod.rs")).expect("read svc");
        assert!(svc.contains("pub async fn order(&self)"));
        assert!(svc.contains("rpc_client_config(\"order\")"));
        assert!(svc.contains("shop_order_rpc::client::RpcClient::connect_from_config(config)"));
        assert!(svc.contains("pub async fn payment(&self)"));
        assert!(svc.contains("rpc_client_config(\"payment\")"));
        assert!(!svc.trim_end().ends_with("}\n}\n}"));

        let cargo = std::fs::read_to_string(admin.join("Cargo.toml")).expect("read cargo");
        assert!(cargo.contains(r#"shop-order-rpc = { path = "../shop-order-rpc" }"#));
        assert!(cargo.contains(r#"shop-payment-rpc = { path = "../shop-payment-rpc" }"#));

        std::fs::remove_dir_all(root).expect("cleanup temp root");
    }

    #[test]
    fn renders_valid_rpc_name_for_route_without_handler() {
        let spec = parse_api(
            r#"
            service user-api {
                get /users/:id (GetUserReq) returns (UserResp)
            }

            type GetUserReq {
                id: u64
            }

            type UserResp {
                name: string
            }
            "#,
        )
        .expect("valid api");

        let proto = render_proto(&spec).expect("valid proto");

        assert!(proto.contains("rpc GetUsersId (GetUserReq) returns (UserResp);"));
        assert!(!proto.contains('/'));
        assert!(!proto.contains(':'));
    }

    #[test]
    fn openapi_document_contains_parameters_bodies_and_schemas() {
        let spec = parse_api(
            r#"
            @server (
                prefix: /api/v1
                jwt: Auth
            )
            service user-api {
                @handler getUser
                get /users/:id (GetUserReq) returns (UserResp)
                @handler login
                post /login (LoginReq) returns (LoginResp)
            }
            type (
                GetUserReq {
                    id u64 `path:"id"`
                    token string `header:"Authorization"`
                    q string `query:"q"`
                }
                UserResp {
                    id u64 `json:"id"`
                    name string `json:"name"`
                }
                LoginReq {
                    username string `json:"username"`
                    password string `json:"password"`
                }
                LoginResp {
                    token string `json:"token"`
                }
            )
            "#,
        )
        .expect("valid api");

        let document = openapi_document(&spec);
        assert_eq!(
            document["paths"]["/api/v1/users/{id}"]["get"]["parameters"][0]["in"],
            "path"
        );
        assert_eq!(
            document["paths"]["/api/v1/users/{id}"]["get"]["parameters"][1]["in"],
            "header"
        );
        assert_eq!(
            document["paths"]["/api/v1/login"]["post"]["requestBody"]["content"]
                ["application/json"]["schema"]["properties"]["username"]["type"],
            "string"
        );
        assert_eq!(
            document["components"]["schemas"]["UserResp"]["properties"]["id"]["format"],
            "uint64"
        );
        assert!(document["components"]["securitySchemes"]["bearerAuth"].is_object());
    }

    #[test]
    fn openapi_document_flattens_embedded_request_fields() {
        let spec = parse_api(
            r#"
            service user-api {
                @handler createUser
                post /users (CreateUserReq) returns (UserResp)
            }
            type (
                BaseReq {
                    traceId string `json:"traceId"`
                }
                CreateUserReq {
                    BaseReq
                    name string `json:"name"`
                }
                UserResp {
                    id u64 `json:"id"`
                }
            )
            "#,
        )
        .expect("valid api");

        let document = openapi_document(&spec);

        assert_eq!(
            document["components"]["schemas"]["CreateUserReq"]["properties"]["traceId"]["type"],
            "string"
        );
        assert_eq!(
            document["paths"]["/users"]["post"]["requestBody"]["content"]["application/json"]
                ["schema"]["properties"]["traceId"]["type"],
            "string"
        );
        assert!(
            document["components"]["schemas"]["CreateUserReq"]["properties"]["baseReq"].is_null()
        );
    }

    #[test]
    fn writes_openapi_yaml_document() {
        let root = temp_test_root("rozectl-openapi-yaml");
        fs::create_dir_all(&root).expect("create root");
        let api = root.join("user.api");
        let out = root.join("docs/swagger.yaml");
        fs::write(
            &api,
            r#"
            service user-api {
                @handler getUser
                get /users/:id (GetUserReq) returns (UserResp)
            }
            type (
                GetUserReq {
                    id u64 `path:"id"`
                }
                UserResp {
                    name string `json:"name"`
                }
            )
            "#,
        )
        .expect("write api");

        write_openapi_yaml(&api, &out).expect("write yaml");

        let yaml = fs::read_to_string(&out).expect("read yaml");
        assert!(yaml.contains("openapi: \"3.0.0\""));
        assert!(yaml.contains("\"/users/{id}\":"));
        assert!(yaml.contains("operationId: \"getUser\""));
        assert!(yaml.contains("components:"));

        fs::remove_dir_all(root).expect("remove root");
    }

    #[test]
    fn generated_cargo_uses_git_dependencies_for_roze_crates() {
        let cargo = cargo_toml(
            "user-api",
            DependencySource::Git,
            None,
            true,
            ProjectKind::Rest,
            Path::new("user-api"),
            &[],
        );

        assert!(
            cargo.contains(r#"roze-config = { git = "https://github.com/roze-team/roze.git" }"#)
        );
        assert!(cargo.contains(r#"roze-rpc = { git = "https://github.com/roze-team/roze.git" }"#));
        assert!(!cargo.contains("roze-db"));
        assert!(!cargo.contains("roze-mongo"));
        assert!(!cargo.contains("toasty"));
        assert!(!cargo.contains(r#"path = "../../crates/roze-"#));
        assert!(!cargo.contains("[build-dependencies]"));
    }

    #[test]
    fn generated_cargo_can_use_local_roze_dependencies() {
        let cargo = cargo_toml(
            "user-api",
            DependencySource::Path,
            Some("../../crates"),
            true,
            ProjectKind::Rest,
            Path::new("user-api"),
            &[],
        );

        assert!(cargo.contains(r#"roze-config = { path = "../../crates/roze-config" }"#));
        assert!(cargo.contains(r#"roze-rpc = { path = "../../crates/roze-rpc" }"#));
        assert!(!cargo.contains("roze-db"));
        assert!(!cargo.contains("roze-mongo"));
        assert!(!cargo.contains("toasty"));
        assert!(!cargo.contains(ROZE_GIT_URL));
    }

    #[test]
    fn generated_cargo_is_standalone_outside_workspace() {
        let cargo = cargo_toml(
            "user",
            DependencySource::Git,
            None,
            false,
            ProjectKind::Rpc,
            Path::new("user"),
            &[],
        );

        assert!(cargo.contains(r#"edition = "2021""#));
        assert!(cargo.contains(r#"version = "0.1.0""#));
        assert!(cargo.contains(r#"anyhow = "1""#));
        assert!(cargo.contains(r#"tokio = { version = "1""#));
        assert!(cargo.contains(r#"roze-grpc = { git = "https://github.com/roze-team/roze.git" }"#));
        assert_eq!(
            cargo
                .matches(r#"roze-grpc = { git = "https://github.com/roze-team/roze.git" }"#)
                .count(),
            2
        );
        assert!(!cargo.contains("rev ="));
        assert!(!cargo.contains(".workspace = true"));
    }

    #[test]
    fn generated_standalone_rest_cargo_uses_only_valid_roze_http_dependency() {
        let cargo = cargo_toml(
            "user-api",
            DependencySource::Git,
            None,
            false,
            ProjectKind::Rest,
            Path::new("user-api"),
            &[],
        );
        let document = cargo
            .parse::<toml_edit::DocumentMut>()
            .expect("valid generated cargo manifest");
        let dependencies = document["dependencies"]
            .as_table()
            .expect("dependencies table");

        assert!(dependencies.contains_key("roze-http"));
        assert!(!dependencies.contains_key("roze_http"));
    }

    #[test]
    fn generated_package_name_uses_output_directory() {
        let spec = parse_api(
            r#"
            service HulaAuth {
                rpc Login (LoginReq) returns (LoginResp)
            }

            type LoginReq {
                token: string
            }

            type LoginResp {
                ok: bool
            }
            "#,
        )
        .expect("valid api");
        let root = temp_test_root("rozectl-package-name-test");
        let out = root.join("services/hula-auth");
        fs::create_dir_all(&root).expect("create test root");
        fs::write(root.join("Cargo.toml"), "[workspace]\nmembers = []\n")
            .expect("write workspace manifest");

        generate_rpc_project(
            &spec,
            &out,
            GenerateOptions::new(GenerateMode::Create, DependencySource::Git),
        )
        .expect("generate rpc project");

        let cargo = fs::read_to_string(out.join("Cargo.toml")).expect("read cargo");
        assert!(cargo.contains(r#"name = "hula-auth""#));
        assert!(!cargo.contains(r#"name = "HulaAuth-service""#));

        fs::remove_dir_all(root).expect("remove test output");
    }

    #[test]
    fn generated_cargo_config_uses_git_cli() {
        assert_eq!(cargo_config(), "[net]\ngit-fetch-with-cli = true\n");
    }

    #[test]
    fn api_generate_outputs_rest_project_only() {
        let root = temp_test_root("rozectl-api-generate-test");
        let api = root.join("user.api");
        let out = root.join("user");
        fs::create_dir_all(&root).expect("create test root");
        fs::write(
            &api,
            r#"
            service user-api {
                get /users/:id (GetUserReq) returns (UserResp)
            }

            type GetUserReq {
                id: u64
            }

            type UserResp {
                name: string
            }
            "#,
        )
        .expect("write api");

        registry()
            .dispatch(GeneratorCommand::ApiGenerate {
                api,
                out: out.clone(),
                options: GenerateOptions::new(GenerateMode::Create, DependencySource::Git),
            })
            .expect("generate api project");

        assert!(out.join("src/handler/mod.rs").is_file());
        assert!(out.join("src/handler/users/get_users_id.rs").is_file());
        assert!(out.join("src/middleware/mod.rs").is_file());
        assert!(out.join("src/openapi/mod.rs").is_file());
        let routes = fs::read_to_string(out.join("src/route/mod.rs")).expect("read routes");
        assert!(routes.contains("/healthz"));
        assert!(routes.contains("/readyz"));
        assert!(routes.contains("/startupz"));
        assert!(routes.contains("/metrics"));
        assert!(out.join("src/logic/mod.rs").is_file());
        assert!(out.join("src/logic/users/get_users_id.rs").is_file());
        assert!(out.join("src/config/mod.rs").is_file());
        assert!(out.join("src/types/mod.rs").is_file());
        assert!(!out.join("src/rpc.rs").exists());
        assert!(!out.join("src/client.rs").exists());
        assert!(!out.join("src/pb.rs").exists());
        assert!(!out.join("build.rs").exists());
        assert!(!out.join("proto").exists());
        assert!(fs::read_to_string(out.join("Cargo.toml"))
            .expect("read cargo")
            .contains("roze-rpc"));
        let cargo = fs::read_to_string(out.join("Cargo.toml")).expect("read cargo");
        assert!(!cargo.contains("roze-db"));
        assert!(!cargo.contains("roze-mongo"));
        assert!(!cargo.contains("toasty"));
        let svc = fs::read_to_string(out.join("src/svc/mod.rs")).expect("read svc");
        assert!(!svc.contains("DatabaseConnections"));
        assert!(!svc.contains("connect_connections_optional"));
        assert!(!svc.contains("read_db"));
        let config = fs::read_to_string(out.join("config.yaml")).expect("read config");
        assert!(!config.contains("database:"));
        assert!(!config.contains("mongo:"));
        assert!(!config.contains("sqlite://"));

        fs::remove_dir_all(root).expect("remove test output");
    }

    #[test]
    fn diff_project_preserves_business_logic_during_update_preview() {
        let root = temp_test_root("rozectl-diff-update-test");
        let api = root.join("user.api");
        let out = root.join("user");
        fs::create_dir_all(&root).expect("create test root");
        fs::write(
            &api,
            r#"
            service user-api {
                get /users/:id (GetUserReq) returns (UserResp)
            }

            type GetUserReq {
                id: u64
            }

            type UserResp {
                name: string
            }
            "#,
        )
        .expect("write api");

        let registry = registry();
        registry
            .dispatch(GeneratorCommand::ApiGenerate {
                api: api.clone(),
                out: out.clone(),
                options: GenerateOptions::new(GenerateMode::Create, DependencySource::Git),
            })
            .expect("generate api project");

        let logic = out.join("src/logic/users/get_users_id.rs");
        fs::write(&logic, "// custom business logic\n").expect("customize logic");
        fs::write(
            &api,
            r#"
            service user-api {
                get /users/:id (GetUserReq) returns (UserResp)
            }

            type GetUserReq {
                id: u64
            }

            type UserResp {
                name: string
                email: string
            }
            "#,
        )
        .expect("update api");

        let report = diff_project(
            &out,
            GeneratorCommand::ApiGenerate {
                api,
                out: PathBuf::new(),
                options: GenerateOptions::new(GenerateMode::Update, DependencySource::Git),
            },
            &registry,
        )
        .expect("diff project");

        let normalized_report = report.replace('\\', "/");
        assert!(normalized_report.contains("M src/types/mod.rs"), "{report}");
        assert!(
            !normalized_report.contains("src/logic/users/get_users_id.rs"),
            "{report}"
        );
        assert_eq!(
            fs::read_to_string(logic).expect("read logic"),
            "// custom business logic\n"
        );

        fs::remove_dir_all(root).expect("remove test output");
    }

    #[test]
    fn rpc_generate_outputs_rpc_project_only() {
        let root = temp_test_root("rozectl-rpc-generate-test");
        let api = root.join("user.api");
        let out = root.join("user");
        fs::create_dir_all(&root).expect("create test root");
        fs::write(
            &api,
            r#"
            service user {
                rpc GetUser (GetUserReq) returns (GetUserResp)
            }

            type GetUserReq {
                id: u64
            }

            type GetUserResp {
                id: u64
            }
            "#,
        )
        .expect("write api");

        registry()
            .dispatch(GeneratorCommand::RpcGenerate {
                api,
                out: out.clone(),
                options: GenerateOptions::new(GenerateMode::Create, DependencySource::Git),
            })
            .expect("generate rpc project");

        assert!(out.join("src/server/mod.rs").is_file());
        assert!(out.join("src/lib.rs").is_file());
        assert!(out.join("src/client/mod.rs").is_file());
        assert!(out.join("src/config/mod.rs").is_file());
        assert!(out.join("src/pb/mod.rs").is_file());
        assert!(out.join("src/types/mod.rs").is_file());
        assert!(out.join("src/logic/get_user.rs").is_file());
        assert!(out.join("build.rs").is_file());
        assert!(out.join("proto/service.proto").is_file());
        assert!(!out.join("src/handler").exists());
        assert!(!out.join("src/openapi.rs").exists());
        assert!(!out.join("src/config.rs").exists());
        assert!(!out.join("src/types.rs").exists());
        assert!(!fs::read_to_string(out.join("Cargo.toml"))
            .expect("read cargo")
            .contains("roze-http"));
        assert!(!fs::read_to_string(out.join("Cargo.toml"))
            .expect("read cargo")
            .contains("toasty"));
        let lib = fs::read_to_string(out.join("src/lib.rs")).expect("read lib");
        assert!(lib.contains("pub mod client;"));
        assert!(lib.contains("pub mod pb;"));
        let config = fs::read_to_string(out.join("config.yaml")).expect("read config");
        assert!(config.contains("postgres://postgres:postgres@127.0.0.1:5432/user"));
        assert!(!config.contains("sqlite://"));

        fs::remove_dir_all(root).expect("remove test output");
    }

    #[test]
    #[ignore = "compile-smoke: generates a REST project and runs cargo check/clippy"]
    fn generated_rest_project_compiles_with_model_and_search() {
        let root = generated_compile_workspace("rozectl-rest-compile-smoke");
        let api = root.join("user.api");
        let model = root.join("user.model");
        let search = root.join("user.search");
        let out = root.join("apps/user-api");
        fs::write(
            &api,
            r#"
            @server (
                prefix: /api
            )
            service user-api {
                @handler getUser
                get /users/:id (GetUserReq) returns (UserResp)
                @handler createUser
                post /users (CreateUserReq) returns (UserResp)
            }

            type (
                GetUserReq {
                    id u64 `path:"id"`
                }

                CreateUserReq {
                    name string `json:"name"`
                    email string `json:"email"`
                    code string `json:"code" validate:"code"`
                    config string `json:"config" validate:"json"`
                    offset int `json:"offset" validate:"nonnegative"`
                    page int `json:"page" validate:"page"`
                    limit int `json:"limit" validate:"limit"`
                    codes []string `json:"codes" validate:"min_items=1,max_items=3,dive,code"`
                }

                UserResp {
                    id u64 `json:"id"`
                    name string `json:"name"`
                    email string `json:"email"`
                }
            )
            "#,
        )
        .expect("write api");
        fs::write(
            &model,
            r#"
            model User {
                table: users
                primary: id
                cache: true
                field id u64
                field name string
                field email string
                field created_at datetime
                unique_index: email
            }
            "#,
        )
        .expect("write model");
        fs::write(
            &search,
            r#"
            index users
            primary id
            field id u64 primary filterable sortable
            field name text searchable
            field email keyword filterable
            field created_at datetime sortable
            "#,
        )
        .expect("write search");

        registry()
            .dispatch(GeneratorCommand::ApiGenerate {
                api,
                out: out.clone(),
                options: GenerateOptions::new(GenerateMode::Create, DependencySource::Path),
            })
            .expect("generate rest project");
        register_workspace_member(&out).expect("register rest smoke workspace member");
        model::generate_model_project(
            &fs::read_to_string(&model).expect("read model"),
            &out,
            GenerateOptions::new(GenerateMode::Update, DependencySource::Path),
            model::ModelFormat::Dsl,
            model::ModelOrm::Toasty,
        )
        .expect("generate model");
        search::generate_search_project(
            &search,
            search::SearchEngine::Elasticsearch,
            &out,
            GenerateOptions::new(GenerateMode::Update, DependencySource::Path),
        )
        .expect("generate search");

        cargo_check_generated(&out.join("Cargo.toml"));
        cargo_clippy_generated(&out.join("Cargo.toml"));
        fs::remove_dir_all(root).expect("remove compile workspace");
    }

    #[test]
    #[ignore = "compile-smoke: generates an RPC project and runs cargo check/clippy"]
    fn generated_rpc_project_compiles() {
        let root = generated_compile_workspace("rozectl-rpc-compile-smoke");
        let api = root.join("user-rpc.api");
        let out = root.join("apps/user-rpc");
        fs::write(
            &api,
            r#"
            service user {
                rpc GetUser (GetUserReq) returns (GetUserResp)
                rpc CreateUser (CreateUserReq) returns (CreateUserResp)
            }

            type (
                GetUserReq {
                    id: u64
                }

                GetUserResp {
                    id: u64
                    name: string
                }

                CreateUserReq {
                    name: string
                    email: string
                    code: string `validate:"code"`
                    config: string `validate:"json"`
                    offset: int `validate:"nonnegative"`
                    page: int `validate:"page"`
                    limit: int `validate:"limit"`
                    codes: []string `validate:"min_items=1,max_items=3,dive,code"`
                }

                CreateUserResp {
                    id: u64
                }
            )
            "#,
        )
        .expect("write rpc api");

        registry()
            .dispatch(GeneratorCommand::RpcGenerate {
                api,
                out: out.clone(),
                options: GenerateOptions::new(GenerateMode::Create, DependencySource::Path),
            })
            .expect("generate rpc project");
        register_workspace_member(&out).expect("register rpc smoke workspace member");

        cargo_check_generated(&out.join("Cargo.toml"));
        cargo_clippy_generated(&out.join("Cargo.toml"));
        fs::remove_dir_all(root).expect("remove compile workspace");
    }

    #[test]
    #[ignore = "compile-smoke: generates a stream worker project and runs cargo check/clippy"]
    fn generated_stream_project_compiles() {
        let root = generated_compile_workspace("rozectl-stream-compile-smoke");
        let api = root.join("user-stream.api");
        let out = root.join("apps/user-stream");
        fs::write(
            &api,
            r#"
            service user {
                rpc UserCreated (UserCreatedReq) returns (UserCreatedResp)
                rpc UserDeleted (UserDeletedReq) returns (UserDeletedResp)
            }

            type (
                UserCreatedReq {
                    id: u64
                    email: string
                }

                UserCreatedResp {
                    ok: bool
                }

                UserDeletedReq {
                    id: u64
                }

                UserDeletedResp {
                    ok: bool
                }
            )
            "#,
        )
        .expect("write stream api");

        write_stream_worker_project(
            &api,
            &out,
            GenerateOptions::new(GenerateMode::Create, DependencySource::Path),
        )
        .expect("generate stream project");

        cargo_check_generated(&out.join("Cargo.toml"));
        cargo_clippy_generated(&out.join("Cargo.toml"));
        fs::remove_dir_all(root).expect("remove compile workspace");
    }

    #[test]
    #[ignore = "compile-smoke: generates HTTP and multi-service smoke tests and runs cargo check"]
    fn generated_http_smoke_project_compiles() {
        let root = generated_compile_workspace("rozectl-http-smoke-compile");
        let api = root.join("smoke.api");
        let out = root.join("apps/smoke-tests");
        fs::write(
            &api,
            r#"
            service smoke-api {
                @handler createItem
                post /items (CreateItemReq) returns (ItemResp)
            }
            type CreateItemReq {
                name string `json:"name"`
            }
            type ItemResp {
                id u64 `json:"id"`
            }
            "#,
        )
        .expect("write smoke api");

        write_http_smoke_test_project(&api, &out, "http://127.0.0.1:3000", false)
            .expect("generate smoke tests");
        register_workspace_member(&out).expect("register smoke workspace member");
        cargo_check_generated(&out.join("Cargo.toml"));
        fs::remove_dir_all(root).expect("remove compile workspace");
    }

    #[test]
    fn writes_stream_worker_project() {
        let root = temp_test_root("rozectl-stream-gen");
        fs::create_dir_all(&root).expect("create stream root");
        let api = root.join("user.api");
        let out = root.join("stream");
        fs::write(
            &api,
            r#"
            service user {
                rpc UserCreated (UserCreatedReq) returns (UserCreatedResp)
            }

            type (
                UserCreatedReq {
                    id: u64
                    email: string
                }

                UserCreatedResp {
                    ok: bool
                }
            )
            "#,
        )
        .expect("write api");

        write_stream_worker_project(
            &api,
            &out,
            GenerateOptions::new(GenerateMode::Create, DependencySource::Git),
        )
        .expect("write stream project");

        let envelope =
            fs::read_to_string(out.join("src/stream/envelope.rs")).expect("read envelope");
        assert!(envelope.contains("TOPIC_USER_CREATED"));
        assert!(envelope.contains("\"user.user_created\""));
        let producer =
            fs::read_to_string(out.join("src/stream/producer.rs")).expect("read producer");
        assert!(producer.contains("publish_user_created"));
        let consumer =
            fs::read_to_string(out.join("src/stream/consumer.rs")).expect("read consumer");
        let main = fs::read_to_string(out.join("src/main.rs")).expect("read main");
        let manifest = fs::read_to_string(out.join("Cargo.toml")).expect("read manifest");
        assert!(main.contains("use roze_service::ServiceGroup;"));
        assert!(main.contains("stream::consumer::run(&broker, &stream_config, shutdown).await"));
        assert!(consumer.contains("ShutdownListener"));
        assert!(consumer.contains("tokio::select!"));
        assert!(consumer.contains("handle_user_created"));
        assert!(manifest.contains("roze-service"));
        assert!(manifest.contains("roze-shutdown"));
        assert!(fs::read_to_string(out.join("README.md"))
            .expect("read readme")
            .contains("`user.user_created.dlq`"));

        fs::remove_dir_all(root).expect("remove stream root");
    }

    #[test]
    fn project_kind_validation_rejects_mixed_or_wrong_idl() {
        let rest_spec = parse_api(
            r#"
            service user-api {
                get /users/:id (GetUserReq) returns (UserResp)
            }

            type GetUserReq {
                id: u64
            }

            type UserResp {
                id: u64
            }
            "#,
        )
        .expect("valid rest api");
        let rpc_spec = parse_api(
            r#"
            service user {
                rpc GetUser (GetUserReq) returns (UserResp)
            }

            type GetUserReq {
                id: u64
            }

            type UserResp {
                id: u64
            }
            "#,
        )
        .expect("valid rpc api");
        let mixed_spec = parse_api(
            r#"
            service user {
                get /users/:id (GetUserReq) returns (UserResp)
                rpc GetUser (GetUserReq) returns (UserResp)
            }

            type GetUserReq {
                id: u64
            }

            type UserResp {
                id: u64
            }
            "#,
        )
        .expect("valid mixed api");

        assert!(validate_project_kind(&rest_spec, ProjectKind::Rest).is_ok());
        assert!(validate_project_kind(&rpc_spec, ProjectKind::Rpc).is_ok());
        assert!(validate_project_kind(&rest_spec, ProjectKind::Rpc)
            .expect_err("rest spec is not rpc")
            .to_string()
            .contains("rpc projects require"));
        assert!(validate_project_kind(&rpc_spec, ProjectKind::Rest)
            .expect_err("rpc spec is not api")
            .to_string()
            .contains("api projects require"));
        assert!(validate_project_kind(&mixed_spec, ProjectKind::Rest)
            .expect_err("mixed spec is not rest-only")
            .to_string()
            .contains("cannot contain `rpc` methods"));
        assert!(validate_project_kind(&mixed_spec, ProjectKind::Rpc)
            .expect_err("mixed spec is not rpc-only")
            .to_string()
            .contains("cannot contain REST routes"));
    }

    #[test]
    fn generated_projects_include_production_evidence_runbooks() {
        let rest_spec = parse_api(
            r#"
            service user-api {
                @handler getUser
                get /users/:id (GetUserReq) returns (UserResp)
            }

            type GetUserReq {
                id: u64
            }

            type UserResp {
                name: string
            }
            "#,
        )
        .expect("valid rest api");
        let rpc_spec = parse_api(
            r#"
            service user {
                rpc GetUser (GetUserReq) returns (UserResp)
            }

            type GetUserReq {
                id: u64
            }

            type UserResp {
                name: string
            }
            "#,
        )
        .expect("valid rpc api");
        let root = temp_test_root("rozectl-production-evidence-runbooks");
        let rest_out = root.join("rest");
        let rpc_out = root.join("rpc");

        generate_rest_project(
            &rest_spec,
            &rest_out,
            GenerateOptions::new(GenerateMode::Create, DependencySource::Git),
        )
        .expect("generate rest project");
        generate_rpc_project(
            &rpc_spec,
            &rpc_out,
            GenerateOptions::new(GenerateMode::Create, DependencySource::Git),
        )
        .expect("generate rpc project");

        let rest_readme = fs::read_to_string(rest_out.join("README.md")).expect("read rest readme");
        let rest_config =
            fs::read_to_string(rest_out.join("config.yaml")).expect("read rest config");
        let rest_runbook = fs::read_to_string(rest_out.join("ops/production-evidence.md"))
            .expect("read rest runbook");
        let rest_governance = fs::read_to_string(rest_out.join("ops/governance-baseline.yaml"))
            .expect("read rest governance baseline");
        let rest_rules = fs::read_to_string(rest_out.join("ops/prometheus-rules.yaml"))
            .expect("read rest prometheus rules");
        let rest_dashboard = fs::read_to_string(rest_out.join("ops/grafana-dashboard.json"))
            .expect("read rest grafana dashboard");
        let rest_slo = fs::read_to_string(rest_out.join("ops/slo.yaml")).expect("read rest slo");
        let rest_failure_plan =
            fs::read_to_string(rest_out.join("ops/failure-injection-plan.yaml"))
                .expect("read rest failure injection plan");
        let rest_release_rollout = fs::read_to_string(rest_out.join("ops/release-rollout.yaml"))
            .expect("read rest release rollout plan");
        let rest_incident_response =
            fs::read_to_string(rest_out.join("ops/incident-response.yaml"))
                .expect("read rest incident response playbook");
        let rest_capacity_plan = fs::read_to_string(rest_out.join("ops/capacity-plan.yaml"))
            .expect("read rest capacity plan");
        let rest_security_readiness =
            fs::read_to_string(rest_out.join("ops/security-readiness.yaml"))
                .expect("read rest security readiness plan");
        let rest_production_gate = fs::read_to_string(rest_out.join("ops/production-gate.yaml"))
            .expect("read rest production gate");
        let rest_regeneration_policy =
            fs::read_to_string(rest_out.join("ops/regeneration-policy.yaml"))
                .expect("read rest regeneration policy");
        let rest_client_contract = fs::read_to_string(rest_out.join("ops/client-contract.yaml"))
            .expect("read rest client contract");
        let rest_config_governance =
            fs::read_to_string(rest_out.join("ops/config-governance.yaml"))
                .expect("read rest config governance");
        let rest_reliable_events = fs::read_to_string(rest_out.join("ops/reliable-events.yaml"))
            .expect("read rest reliable events");
        let rest_dependency_governance =
            fs::read_to_string(rest_out.join("ops/dependency-governance.yaml"))
                .expect("read rest dependency governance");
        let rest_data_consistency = fs::read_to_string(rest_out.join("ops/data-consistency.yaml"))
            .expect("read rest data consistency");
        let rest_observability =
            fs::read_to_string(rest_out.join("ops/observability-contract.yaml"))
                .expect("read rest observability contract");
        let rest_runtime_hardening =
            fs::read_to_string(rest_out.join("ops/runtime-hardening.yaml"))
                .expect("read rest runtime hardening contract");
        let rest_error_contract = fs::read_to_string(rest_out.join("ops/error-contract.yaml"))
            .expect("read rest error contract");
        let rest_deployment_topology =
            fs::read_to_string(rest_out.join("ops/deployment-topology.yaml"))
                .expect("read rest deployment topology");
        let rest_service_communication =
            fs::read_to_string(rest_out.join("ops/service-communication.yaml"))
                .expect("read rest service communication");
        let rest_cache_governance = fs::read_to_string(rest_out.join("ops/cache-governance.yaml"))
            .expect("read rest cache governance");
        let rest_data_access_governance =
            fs::read_to_string(rest_out.join("ops/data-access-governance.yaml"))
                .expect("read rest data access governance");
        let rest_interface_governance =
            fs::read_to_string(rest_out.join("ops/interface-governance.yaml"))
                .expect("read rest interface governance");
        let rest_production_verify = fs::read_to_string(rest_out.join("ops/production-verify.ps1"))
            .expect("read rest production verify script");
        let rest_production_verify_sh =
            fs::read_to_string(rest_out.join("ops/production-verify.sh"))
                .expect("read rest production verify shell script");
        let rest_ci_evidence_policy =
            fs::read_to_string(rest_out.join("ops/ci-evidence-policy.yaml"))
                .expect("read rest ci evidence policy");
        let rest_evidence_manifest =
            fs::read_to_string(rest_out.join("ops/evidence-manifest.yaml"))
                .expect("read rest evidence manifest");
        let rest_production_workflow =
            fs::read_to_string(rest_out.join(".github/workflows/roze-production-verify.yml"))
                .expect("read rest production verify workflow");
        let rpc_readme = fs::read_to_string(rpc_out.join("README.md")).expect("read rpc readme");
        let rpc_config = fs::read_to_string(rpc_out.join("config.yaml")).expect("read rpc config");
        let rpc_runbook = fs::read_to_string(rpc_out.join("ops/production-evidence.md"))
            .expect("read rpc runbook");
        let rpc_governance = fs::read_to_string(rpc_out.join("ops/governance-baseline.yaml"))
            .expect("read rpc governance baseline");
        let rpc_rules = fs::read_to_string(rpc_out.join("ops/prometheus-rules.yaml"))
            .expect("read rpc prometheus rules");
        let rpc_dashboard = fs::read_to_string(rpc_out.join("ops/grafana-dashboard.json"))
            .expect("read rpc grafana dashboard");
        let rpc_slo = fs::read_to_string(rpc_out.join("ops/slo.yaml")).expect("read rpc slo");
        let rpc_failure_plan = fs::read_to_string(rpc_out.join("ops/failure-injection-plan.yaml"))
            .expect("read rpc failure injection plan");
        let rpc_release_rollout = fs::read_to_string(rpc_out.join("ops/release-rollout.yaml"))
            .expect("read rpc release rollout plan");
        let rpc_incident_response = fs::read_to_string(rpc_out.join("ops/incident-response.yaml"))
            .expect("read rpc incident response playbook");
        let rpc_capacity_plan = fs::read_to_string(rpc_out.join("ops/capacity-plan.yaml"))
            .expect("read rpc capacity plan");
        let rpc_security_readiness =
            fs::read_to_string(rpc_out.join("ops/security-readiness.yaml"))
                .expect("read rpc security readiness plan");
        let rpc_production_gate = fs::read_to_string(rpc_out.join("ops/production-gate.yaml"))
            .expect("read rpc production gate");
        let rpc_regeneration_policy =
            fs::read_to_string(rpc_out.join("ops/regeneration-policy.yaml"))
                .expect("read rpc regeneration policy");
        let rpc_client_contract = fs::read_to_string(rpc_out.join("ops/client-contract.yaml"))
            .expect("read rpc client contract");
        let rpc_config_governance = fs::read_to_string(rpc_out.join("ops/config-governance.yaml"))
            .expect("read rpc config governance");
        let rpc_reliable_events = fs::read_to_string(rpc_out.join("ops/reliable-events.yaml"))
            .expect("read rpc reliable events");
        let rpc_dependency_governance =
            fs::read_to_string(rpc_out.join("ops/dependency-governance.yaml"))
                .expect("read rpc dependency governance");
        let rpc_data_consistency = fs::read_to_string(rpc_out.join("ops/data-consistency.yaml"))
            .expect("read rpc data consistency");
        let rpc_observability = fs::read_to_string(rpc_out.join("ops/observability-contract.yaml"))
            .expect("read rpc observability contract");
        let rpc_runtime_hardening = fs::read_to_string(rpc_out.join("ops/runtime-hardening.yaml"))
            .expect("read rpc runtime hardening contract");
        let rpc_error_contract = fs::read_to_string(rpc_out.join("ops/error-contract.yaml"))
            .expect("read rpc error contract");
        let rpc_deployment_topology =
            fs::read_to_string(rpc_out.join("ops/deployment-topology.yaml"))
                .expect("read rpc deployment topology");
        let rpc_service_communication =
            fs::read_to_string(rpc_out.join("ops/service-communication.yaml"))
                .expect("read rpc service communication");
        let rpc_cache_governance = fs::read_to_string(rpc_out.join("ops/cache-governance.yaml"))
            .expect("read rpc cache governance");
        let rpc_data_access_governance =
            fs::read_to_string(rpc_out.join("ops/data-access-governance.yaml"))
                .expect("read rpc data access governance");
        let rpc_interface_governance =
            fs::read_to_string(rpc_out.join("ops/interface-governance.yaml"))
                .expect("read rpc interface governance");
        let rpc_production_verify = fs::read_to_string(rpc_out.join("ops/production-verify.ps1"))
            .expect("read rpc production verify script");
        let rpc_production_verify_sh = fs::read_to_string(rpc_out.join("ops/production-verify.sh"))
            .expect("read rpc production verify shell script");
        let rpc_ci_evidence_policy =
            fs::read_to_string(rpc_out.join("ops/ci-evidence-policy.yaml"))
                .expect("read rpc ci evidence policy");
        let rpc_evidence_manifest = fs::read_to_string(rpc_out.join("ops/evidence-manifest.yaml"))
            .expect("read rpc evidence manifest");
        let rpc_production_workflow =
            fs::read_to_string(rpc_out.join(".github/workflows/roze-production-verify.yml"))
                .expect("read rpc production verify workflow");

        assert!(rest_readme.contains("ops/production-evidence.md"));
        assert!(rpc_readme.contains("ops/production-evidence.md"));
        assert!(rest_readme.contains("ops\\production-verify.ps1"));
        assert!(rpc_readme.contains("ops\\production-verify.ps1"));
        assert!(rest_readme.contains("ops/production-verify.sh"));
        assert!(rpc_readme.contains("ops/production-verify.sh"));
        assert!(rest_readme.contains("ops/ci-evidence-policy.yaml"));
        assert!(rpc_readme.contains("ops/ci-evidence-policy.yaml"));
        assert!(rest_readme.contains("ops/evidence-manifest.yaml"));
        assert!(rpc_readme.contains("ops/evidence-manifest.yaml"));
        assert!(rest_readme.contains(".github/workflows/roze-production-verify.yml"));
        assert!(rpc_readme.contains(".github/workflows/roze-production-verify.yml"));
        assert!(rest_config.contains("routes:\n    get_user:"));
        assert!(rest_config.contains("max_attempts: 2"));
        assert!(rest_config.contains("rate_limit:\n        burst: 100"));
        assert!(rest_config.contains("breaker:\n        failure_threshold: 5"));
        assert!(rest_config.contains("shedding:\n        concurrency: 100"));
        assert!(rest_config.contains("max_failure_ratio_per_mille: 500"));
        assert!(rpc_config.contains("routes:\n    GetUser:"));
        assert!(rpc_config.contains("timeout_ms: 5000"));
        assert!(rpc_config.contains("budget_percent: 10"));
        assert!(rpc_config.contains("shedding:\n        concurrency: 100"));
        assert!(rest_config.contains("fallback:\n    enabled: false"));
        assert!(rest_config.contains("x-roze-fallback: route"));
        assert!(rpc_config.contains("fallback:\n    enabled: false"));
        assert!(rpc_config.contains("x-roze-fallback: method"));
        assert!(rpc_config.contains("balancer: power_of_two_choices"));
        assert!(rest_runbook.contains("Generated by `rozectl` for the REST boundary."));
        assert!(rpc_runbook.contains("Generated by `rozectl` for the RPC boundary."));
        assert!(rest_runbook.contains("## Architecture Borrowed And Extended"));
        assert!(rest_runbook.contains("timeout, rate limit, circuit breaker, load shedding"));
        assert!(rest_runbook.contains("generated evidence gates"));
        assert!(rest_runbook.contains("--area generated-services"));
        assert!(rest_runbook.contains("--lifecycle-summary"));
        assert!(rest_runbook.contains("The lifecycle summary is rejected"));
        assert!(rest_runbook.contains("ops/governance-baseline.yaml"));
        assert!(rest_runbook.contains("ops/prometheus-rules.yaml"));
        assert!(rest_runbook.contains("ops/grafana-dashboard.json"));
        assert!(rest_runbook.contains("ops/slo.yaml"));
        assert!(rest_runbook.contains("ops/failure-injection-plan.yaml"));
        assert!(rest_runbook.contains("ops/release-rollout.yaml"));
        assert!(rest_runbook.contains("ops/incident-response.yaml"));
        assert!(rest_runbook.contains("ops/capacity-plan.yaml"));
        assert!(rest_runbook.contains("ops/security-readiness.yaml"));
        assert!(rest_runbook.contains("ops/production-gate.yaml"));
        assert!(rest_runbook.contains("ops/regeneration-policy.yaml"));
        assert!(rest_runbook.contains("ops/client-contract.yaml"));
        assert!(rest_runbook.contains("ops/config-governance.yaml"));
        assert!(rest_runbook.contains("ops/reliable-events.yaml"));
        assert!(rest_runbook.contains("ops/dependency-governance.yaml"));
        assert!(rest_runbook.contains("ops/data-consistency.yaml"));
        assert!(rest_runbook.contains("ops/observability-contract.yaml"));
        assert!(rest_runbook.contains("ops/runtime-hardening.yaml"));
        assert!(rest_runbook.contains("ops/error-contract.yaml"));
        assert!(rest_runbook.contains("ops/deployment-topology.yaml"));
        assert!(rest_runbook.contains("ops/service-communication.yaml"));
        assert!(rest_runbook.contains("ops/cache-governance.yaml"));
        assert!(rest_runbook.contains("ops/data-access-governance.yaml"));
        assert!(rest_runbook.contains("ops/interface-governance.yaml"));
        assert!(rest_runbook.contains("ops/production-verify.ps1"));
        assert!(rest_runbook.contains("ops/production-verify.sh"));
        assert!(rest_runbook.contains("ops/ci-evidence-policy.yaml"));
        assert!(rest_runbook.contains("ops/evidence-manifest.yaml"));
        assert!(rest_runbook.contains(".github/workflows/roze-production-verify.yml"));
        assert!(rest_production_verify.contains("function Invoke-Step"));
        assert!(rest_production_verify.contains("$requiredOpsFiles"));
        assert!(rest_production_verify.contains("$EvidenceManifestPath"));
        assert!(rest_production_verify.contains("$CiEvidencePolicyPath"));
        assert!(rest_production_verify.contains("$VerifyReportPath"));
        assert!(rest_production_verify.contains("evidence manifest coverage"));
        assert!(rest_production_verify.contains("$manifestEntry = \"path: $relativePath\""));
        assert!(rest_production_verify
            .contains("Evidence manifest does not index generated asset entry"));
        assert!(rest_production_verify.contains("ci evidence policy coverage"));
        assert!(rest_production_verify.contains("$policyEntry = \"    - $relativePath\""));
        assert!(rest_production_verify
            .contains("CI evidence policy does not require generated asset path"));
        assert!(rest_production_verify.contains("ops/production-gate.yaml"));
        assert!(rest_production_verify.contains("ops/production-verify.sh"));
        assert!(rest_production_verify.contains("ops/ci-evidence-policy.yaml"));
        assert!(rest_production_verify.contains("ops/evidence-manifest.yaml"));
        assert!(rest_production_verify.contains(".github/workflows/roze-production-verify.yml"));
        assert!(rest_production_verify.contains("cargo fmt --manifest-path"));
        assert!(rest_production_verify.contains("cargo check --manifest-path"));
        assert!(rest_production_verify.contains("cargo test --manifest-path"));
        assert!(rest_production_verify.contains("production-verify-report.json"));
        assert!(rest_production_verify.contains("pass_ci_precondition"));
        assert!(rest_production_verify.contains("requires_long_run_evidence"));
        assert!(rest_production_verify.contains("required_followup_evidence"));
        assert!(rest_production_verify.contains("Production verification report"));
        assert!(rest_production_verify.contains("GET /reports/export"));
        assert!(rest_production_verify.contains("POST /charts/query"));
        assert!(rest_production_verify.contains("GET /users/:id"));
        assert!(rest_production_verify_sh.starts_with("#!/usr/bin/env bash"));
        assert!(rest_production_verify_sh.contains("set -euo pipefail"));
        assert!(rest_production_verify_sh.contains("run_step()"));
        assert!(rest_production_verify_sh.contains("required_ops_files=("));
        assert!(rest_production_verify_sh.contains("EVIDENCE_MANIFEST_PATH"));
        assert!(rest_production_verify_sh.contains("CI_EVIDENCE_POLICY_PATH"));
        assert!(rest_production_verify_sh.contains("VERIFY_REPORT_PATH"));
        assert!(rest_production_verify_sh.contains("check_evidence_manifest_coverage()"));
        assert!(rest_production_verify_sh.contains("evidence manifest coverage"));
        assert!(rest_production_verify_sh.contains("manifest_entry=\"path: $relative_path\""));
        assert!(rest_production_verify_sh.contains("check_ci_evidence_policy_coverage()"));
        assert!(rest_production_verify_sh.contains("ci evidence policy coverage"));
        assert!(rest_production_verify_sh.contains("policy_entry=\"    - $relative_path\""));
        assert!(rest_production_verify_sh
            .contains("CI evidence policy does not require generated asset path"));
        assert!(rest_production_verify_sh.contains("ops/production-verify.ps1"));
        assert!(rest_production_verify_sh.contains("ops/production-verify.sh"));
        assert!(rest_production_verify_sh.contains("ops/ci-evidence-policy.yaml"));
        assert!(rest_production_verify_sh.contains("ops/evidence-manifest.yaml"));
        assert!(rest_production_verify_sh.contains(".github/workflows/roze-production-verify.yml"));
        assert!(rest_production_verify_sh.contains("cargo fmt --manifest-path"));
        assert!(rest_production_verify_sh.contains("cargo check --manifest-path"));
        assert!(rest_production_verify_sh.contains("cargo test --manifest-path"));
        assert!(rest_production_verify_sh.contains("production-verify-report.json"));
        assert!(rest_production_verify_sh.contains("pass_ci_precondition"));
        assert!(rest_production_verify_sh.contains("requires_long_run_evidence"));
        assert!(rest_production_verify_sh.contains("required_followup_evidence"));
        assert!(rest_production_verify_sh.contains("Production verification report"));
        assert!(rest_production_verify_sh.contains("GET /reports/export"));
        assert!(rest_production_verify_sh.contains("POST /charts/query"));
        assert!(rest_production_verify_sh.contains("GET /users/:id"));
        assert!(rest_production_workflow.contains("name: Roze Production Verify"));
        assert!(rest_production_workflow.contains("ROZE_SERVICE_NAME: user-api"));
        assert!(rest_production_workflow.contains("ROZE_BOUNDARY: rest"));
        assert!(rest_production_workflow.contains("ubuntu-latest"));
        assert!(rest_production_workflow.contains("windows-latest"));
        assert!(rest_production_workflow.contains("bash ops/production-verify.sh"));
        assert!(rest_production_workflow.contains("ops\\production-verify.ps1"));
        assert!(rest_production_workflow.contains("actions/upload-artifact@v4"));
        assert!(rest_production_workflow.contains("roze-production-evidence-${{ matrix.os }}"));
        assert!(rest_production_workflow.contains("retention-days: 30"));
        assert!(rest_production_workflow.contains("ops/**"));
        assert!(rest_ci_evidence_policy.contains("service: user-api"));
        assert!(rest_ci_evidence_policy.contains("boundary: rest"));
        assert!(rest_ci_evidence_policy.contains("policy: ci_evidence"));
        assert!(rest_ci_evidence_policy.contains("retention_days: 30"));
        assert!(rest_ci_evidence_policy.contains("artifact_upload_missing"));
        assert!(rest_ci_evidence_policy.contains("missing_ci_evidence_policy"));
        assert!(rest_ci_evidence_policy.contains("missing_evidence_manifest"));
        assert!(rest_ci_evidence_policy.contains("    - ops/production-gate.yaml"));
        assert!(rest_ci_evidence_policy.contains("    - ops/production-verify.ps1"));
        assert!(rest_ci_evidence_policy.contains("    - ops/production-verify.sh"));
        assert!(rest_ci_evidence_policy.contains("    - ops/ci-evidence-policy.yaml"));
        assert!(rest_ci_evidence_policy.contains("ops/evidence-manifest.yaml"));
        assert!(
            rest_ci_evidence_policy.contains("    - .github/workflows/roze-production-verify.yml")
        );
        assert!(rest_ci_evidence_policy.contains("produced_paths:"));
        assert!(rest_ci_evidence_policy.contains("    - ops/production-verify-report.json"));
        assert!(rest_ci_evidence_policy
            .contains("framework_probes_report_export_chart_query_and_business_routes"));
        assert!(rest_ci_evidence_policy.contains("ci_success_is: precondition"));
        assert!(rest_evidence_manifest.contains("service: user-api"));
        assert!(rest_evidence_manifest.contains("boundary: rest"));
        assert!(rest_evidence_manifest.contains("manifest: production_evidence"));
        assert!(rest_evidence_manifest.contains("path: ops/evidence-manifest.yaml"));
        assert!(rest_evidence_manifest.contains("kind: manifest"));
        assert!(rest_evidence_manifest.contains("runtime_artifacts:"));
        assert!(rest_evidence_manifest.contains("path: ops/production-verify-report.json"));
        assert!(rest_evidence_manifest.contains("kind: verification_report"));
        assert!(rest_evidence_manifest.contains("GET /reports/export"));
        assert!(rest_evidence_manifest.contains("POST /charts/query"));
        assert!(rest_evidence_manifest.contains("path: \"/users/:id\""));
        assert!(rest_evidence_manifest.contains("ci_evidence_bundle"));
        assert!(rest_evidence_manifest.contains("missing_manifest_blocks_promotion: true"));
        assert!(rest_governance.contains("boundary: rest"));
        assert!(rest_governance.contains("endpoint_count: 1"));
        assert!(rest_governance.contains("failure_oriented_resilience"));
        assert!(rest_governance.contains("generated_prometheus_alert_rules"));
        assert!(rest_governance.contains("generated_grafana_dashboard"));
        assert!(rest_governance.contains("generated_slo_error_budget"));
        assert!(rest_governance.contains("generated_failure_injection_plan"));
        assert!(rest_governance.contains("generated_release_rollout_gates"));
        assert!(rest_governance.contains("generated_incident_response_playbook"));
        assert!(rest_governance.contains("generated_capacity_plan"));
        assert!(rest_governance.contains("generated_security_readiness_plan"));
        assert!(rest_governance.contains("generated_production_gate"));
        assert!(rest_governance.contains("generated_regeneration_policy"));
        assert!(rest_governance.contains("generated_client_contract"));
        assert!(rest_governance.contains("generated_config_governance"));
        assert!(rest_governance.contains("generated_reliable_events_plan"));
        assert!(rest_governance.contains("generated_dependency_governance"));
        assert!(rest_governance.contains("generated_data_consistency_plan"));
        assert!(rest_governance.contains("generated_observability_contract"));
        assert!(rest_governance.contains("generated_runtime_hardening_contract"));
        assert!(rest_governance.contains("generated_error_contract"));
        assert!(rest_governance.contains("generated_deployment_topology_contract"));
        assert!(rest_governance.contains("generated_service_communication_contract"));
        assert!(rest_governance.contains("generated_cache_governance_contract"));
        assert!(rest_governance.contains("generated_data_access_governance_contract"));
        assert!(rest_governance.contains("circuit_breaker:"));
        assert!(rest_governance.contains("generated_rules: ops/prometheus-rules.yaml"));
        assert!(rest_governance.contains("generated_dashboard: ops/grafana-dashboard.json"));
        assert!(rest_governance.contains("lifecycle_summary_consistency: true"));
        assert!(rest_governance.contains("slo_error_budget_report: true"));
        assert!(rest_governance.contains("failure_injection_plan: true"));
        assert!(rest_governance.contains("release_rollout_plan: true"));
        assert!(rest_governance.contains("incident_response_playbook: true"));
        assert!(rest_governance.contains("capacity_plan: true"));
        assert!(rest_governance.contains("security_readiness_plan: true"));
        assert!(rest_governance.contains("production_gate: true"));
        assert!(rest_governance.contains("regeneration_policy: true"));
        assert!(rest_governance.contains("client_contract: true"));
        assert!(rest_governance.contains("config_governance: true"));
        assert!(rest_governance.contains("reliable_events_plan: true"));
        assert!(rest_governance.contains("dependency_governance: true"));
        assert!(rest_governance.contains("data_consistency_plan: true"));
        assert!(rest_governance.contains("observability_contract: true"));
        assert!(rest_governance.contains("runtime_hardening_contract: true"));
        assert!(rest_governance.contains("error_contract: true"));
        assert!(rest_governance.contains("deployment_topology_contract: true"));
        assert!(rest_governance.contains("service_communication_contract: true"));
        assert!(rest_governance.contains("cache_governance_contract: true"));
        assert!(rest_governance.contains("data_access_governance_contract: true"));
        assert!(rest_rules.contains("RozeGeneratedServiceHighErrorRate"));
        assert!(rest_rules.contains("RozeGeneratedServiceHighP99Latency"));
        assert!(rest_rules.contains("RozeGeneratedServiceCircuitBreakerOpen"));
        assert!(rest_rules.contains("RozeGeneratedServiceLoadShedding"));
        assert!(rest_rules.contains("roze_http_request_duration_seconds_bucket"));
        assert!(rest_dashboard.contains("\"title\": \"Roze Generated Service - user-api\""));
        assert!(rest_dashboard.contains("\"Request Rate\""));
        assert!(rest_dashboard.contains("\"P99 Latency\""));
        assert!(rest_dashboard.contains("roze_resilience_decisions_total"));
        assert!(rest_slo.contains("target: 99.9"));
        assert!(rest_slo.contains("latency_p99:"));
        assert!(rest_slo.contains("error_budget_burn"));
        assert!(rest_slo.contains("dashboard_attached: true"));
        assert!(rest_failure_plan.contains("boundary: rest"));
        assert!(rest_failure_plan.contains("scenario: shutdown_signal"));
        assert!(rest_failure_plan.contains("scenario: dependency_5xx"));
        assert!(rest_failure_plan.contains("circuit_breaker"));
        assert!(rest_failure_plan.contains("load_shedding"));
        assert!(rest_failure_plan.contains("recovery_time: required"));
        assert!(rest_release_rollout.contains("boundary: rest"));
        assert!(rest_release_rollout.contains("gate: canary_1_percent"));
        assert!(rest_release_rollout.contains("gate: progressive_50_percent"));
        assert!(rest_release_rollout.contains("blue_green:"));
        assert!(rest_release_rollout.contains("rollback_required:"));
        assert!(rest_release_rollout.contains("roze_http_requests_total"));
        assert!(rest_incident_response.contains("boundary: rest"));
        assert!(rest_incident_response.contains("alert: RozeGeneratedServiceDown"));
        assert!(rest_incident_response.contains("alert: RozeGeneratedServiceHighErrorRate"));
        assert!(rest_incident_response.contains("alert: RozeGeneratedServiceLoadShedding"));
        assert!(rest_incident_response.contains("rollback_when:"));
        assert!(rest_incident_response.contains("postmortem_required:"));
        assert!(rest_incident_response.contains("roze_http_requests_total"));
        assert!(rest_capacity_plan.contains("boundary: rest"));
        assert!(rest_capacity_plan.contains("phase: soak_24h"));
        assert!(rest_capacity_plan.contains("phase: soak_72h"));
        assert!(rest_capacity_plan.contains("phase: burst"));
        assert!(rest_capacity_plan.contains("phase: scale_out"));
        assert!(rest_capacity_plan.contains("phase: scale_in"));
        assert!(rest_capacity_plan.contains("roze_http_request_duration_seconds_bucket"));
        assert!(rest_security_readiness.contains("boundary: rest"));
        assert!(rest_security_readiness.contains("check: authentication"));
        assert!(rest_security_readiness.contains("check: tenant_isolation"));
        assert!(rest_security_readiness.contains("check: key_rotation"));
        assert!(rest_security_readiness.contains("check: mtls"));
        assert!(rest_security_readiness.contains("check: audit_log"));
        assert!(rest_security_readiness.contains("blocking_findings:"));
        assert!(
            rest_security_readiness.contains("route_method_path_and_openapi_security_projection")
        );
        assert!(rest_production_gate.contains("boundary: rest"));
        assert!(rest_production_gate.contains("ops/security-readiness.yaml"));
        assert!(rest_production_gate.contains("ops/regeneration-policy.yaml"));
        assert!(rest_production_gate.contains("ops/client-contract.yaml"));
        assert!(rest_production_gate.contains("ops/config-governance.yaml"));
        assert!(rest_production_gate.contains("ops/reliable-events.yaml"));
        assert!(rest_production_gate.contains("ops/dependency-governance.yaml"));
        assert!(rest_production_gate.contains("ops/data-consistency.yaml"));
        assert!(rest_production_gate.contains("ops/observability-contract.yaml"));
        assert!(rest_production_gate.contains("ops/runtime-hardening.yaml"));
        assert!(rest_production_gate.contains("ops/error-contract.yaml"));
        assert!(rest_production_gate.contains("ops/deployment-topology.yaml"));
        assert!(rest_production_gate.contains("ops/service-communication.yaml"));
        assert!(rest_production_gate.contains("ops/cache-governance.yaml"));
        assert!(rest_production_gate.contains("ops/data-access-governance.yaml"));
        assert!(rest_production_gate.contains("ops/interface-governance.yaml"));
        assert!(rest_production_gate.contains("stage: client_contract"));
        assert!(rest_production_gate.contains("stage: config_governance"));
        assert!(rest_production_gate.contains("stage: reliable_events"));
        assert!(rest_production_gate.contains("stage: dependency_governance"));
        assert!(rest_production_gate.contains("stage: data_consistency"));
        assert!(rest_production_gate.contains("stage: observability_contract"));
        assert!(rest_production_gate.contains("stage: runtime_hardening"));
        assert!(rest_production_gate.contains("stage: error_contract"));
        assert!(rest_production_gate.contains("stage: deployment_topology"));
        assert!(rest_production_gate.contains("stage: service_communication"));
        assert!(rest_production_gate.contains("stage: cache_governance"));
        assert!(rest_production_gate.contains("stage: data_access_governance"));
        assert!(rest_production_gate.contains("stage: interface_governance"));
        assert!(rest_production_gate.contains("framework_smoke_output"));
        assert!(rest_production_gate.contains("stage: capacity_and_soak"));
        assert!(rest_production_gate.contains("stage: security_readiness"));
        assert!(rest_production_gate.contains("idl_drift_classified"));
        assert!(rest_production_gate.contains("blocking_rules:"));
        assert!(rest_production_gate.contains("broad_production_stable:"));
        assert!(rest_regeneration_policy.contains("boundary: rest"));
        assert!(rest_regeneration_policy.contains("src/openapi/mod.rs"));
        assert!(rest_regeneration_policy.contains("src/logic/**"));
        assert!(rest_regeneration_policy.contains("drift_classification:"));
        assert!(rest_regeneration_policy.contains("breaking_change_without_migration_plan"));
        assert!(rest_client_contract.contains("boundary: rest"));
        assert!(rest_client_contract.contains("src/openapi/mod.rs_and_generated_openapi_json"));
        assert!(rest_client_contract.contains("typed_errors_required: true"));
        assert!(rest_client_contract.contains("auth_injection_required: true"));
        assert!(rest_client_contract.contains("generated_client_governance_path"));
        assert!(rest_config_governance.contains("boundary: rest"));
        assert!(rest_config_governance
            .contains("reload_timeout_rate_limit_cors_registry_and_dependency_config"));
        assert!(rest_config_governance.contains("phase: canary_reload"));
        assert!(rest_config_governance.contains("phase: snapshot_restore"));
        assert!(rest_config_governance.contains("listener_failure_isolated: true"));
        assert!(rest_reliable_events.contains("boundary: rest"));
        assert!(rest_reliable_events
            .contains("representative_http_mutation_publishes_or_declares_no_event"));
        assert!(rest_reliable_events.contains("outbox_inbox_idempotency"));
        assert!(rest_reliable_events.contains("dlq_replay"));
        assert!(rest_reliable_events.contains("retry_storm_protection"));
        assert!(rest_dependency_governance.contains("boundary: rest"));
        assert!(rest_dependency_governance
            .contains("representative_http_handler_calls_declared_downstream_or_declares_none"));
        assert!(rest_dependency_governance.contains("load_balancing:"));
        assert!(rest_dependency_governance.contains("circuit_breaker:"));
        assert!(rest_dependency_governance.contains("dependency_without_timeout"));
        assert!(rest_data_consistency.contains("boundary: rest"));
        assert!(rest_data_consistency
            .contains("representative_http_mutation_declares_transaction_or_no_persistence"));
        assert!(rest_data_consistency.contains("outbox:"));
        assert!(rest_data_consistency.contains("test: migration_rollback"));
        assert!(rest_data_consistency.contains("dual_write_without_outbox_or_dtm_evidence"));
        assert!(rest_observability.contains("boundary: rest"));
        assert!(rest_observability.contains("roze_http_requests_total"));
        assert!(rest_observability.contains("method_route_status"));
        assert!(rest_observability.contains("label_cardinality_budget:"));
        assert!(rest_observability.contains("sensitive_data_in_logs_or_labels"));
        assert!(rest_runtime_hardening.contains("boundary: rest"));
        assert!(rest_runtime_hardening
            .contains("representative_http_request_with_server_timeout_and_client_cancel"));
        assert!(rest_runtime_hardening.contains("load_shedding:"));
        assert!(rest_runtime_hardening.contains("retry_budget:"));
        assert!(
            rest_runtime_hardening.contains("graceful_shutdown_without_readiness_drain_timeline")
        );
        assert!(rest_error_contract.contains("boundary: rest"));
        assert!(rest_error_contract.contains("http_status_mapping:"));
        assert!(rest_error_contract.contains("typed_errors_required: true"));
        assert!(rest_error_contract.contains("no_implicit_compatibility_claim: true"));
        assert!(rest_error_contract.contains("raw_internal_error_returned_to_client"));
        assert!(rest_deployment_topology.contains("boundary: rest"));
        assert!(rest_deployment_topology.contains("port: 8080"));
        assert!(rest_deployment_topology.contains("GET /readyz"));
        assert!(rest_deployment_topology.contains("image_digest_pinned: true"));
        assert!(rest_deployment_topology.contains("secret_in_plain_text_config"));
        assert!(rest_service_communication.contains("boundary: rest"));
        assert!(rest_service_communication
            .contains("generated_rest_service_context_http_clients_or_gateway_clients"));
        assert!(rest_service_communication
            .contains("representative_http_handler_calls_declared_downstream_or_declares_none"));
        assert!(rest_service_communication.contains("discovery_and_load_balancing"));
        assert!(rest_service_communication.contains("silent_success_on_dependency_failure"));
        assert!(rest_cache_governance.contains("boundary: rest"));
        assert!(rest_cache_governance
            .contains("representative_http_query_declares_cache_policy_or_no_cache"));
        assert!(rest_cache_governance.contains("singleflight_or_request_collapse_required"));
        assert!(rest_cache_governance.contains("ttl_without_jitter_for_large_keyset"));
        assert!(rest_cache_governance.contains("cache_metric_uses_raw_key_label"));
        assert!(rest_data_access_governance.contains("boundary: rest"));
        assert!(rest_data_access_governance
            .contains("representative_http_query_declares_data_access_policy_or_no_persistence"));
        assert!(rest_data_access_governance.contains("slow_query_index_pagination"));
        assert!(rest_data_access_governance.contains("n_plus_one_without_query_count_budget"));
        assert!(rest_data_access_governance.contains("raw_sql_without_review"));
        assert!(rest_interface_governance.contains("boundary: rest"));
        assert!(rest_interface_governance.contains("framework_endpoints:"));
        assert!(rest_interface_governance.contains("path: /reports/export"));
        assert!(rest_interface_governance.contains("path: /charts/query"));
        assert!(rest_interface_governance.contains("business_endpoints:"));
        assert!(rest_interface_governance.contains("path: /users/:id"));
        assert!(rest_interface_governance.contains("smoke_framework_report_export"));
        assert!(rest_interface_governance.contains("endpoint_missing_smoke_test"));
        assert!(rpc_runbook.contains("client RPC calls for 1 method(s)"));
        assert!(rpc_governance.contains("boundary: rpc"));
        assert!(rpc_governance.contains("endpoint_count: 1"));
        assert!(rpc_governance.contains("deadline_propagation:"));
        assert!(rpc_rules.contains("roze_rpc_method_duration_seconds_bucket"));
        assert!(rpc_dashboard.contains("roze_rpc_method_duration_seconds_bucket"));
        assert!(rpc_slo.contains("boundary: rpc"));
        assert!(rpc_slo.contains("roze_rpc_method_duration_seconds_bucket"));
        assert!(rpc_failure_plan.contains("boundary: rpc"));
        assert!(rpc_failure_plan.contains("representative_rpc_calls_and_client_deadline_tests"));
        assert!(rpc_failure_plan.contains("scenario: slow_dependency"));
        assert!(rpc_failure_plan.contains("deadline_propagation"));
        assert!(rpc_failure_plan.contains("rollback_notes: required"));
        assert!(rpc_release_rollout.contains("boundary: rpc"));
        assert!(rpc_release_rollout.contains("representative_rpc_call"));
        assert!(rpc_release_rollout.contains("gate: full_rollout"));
        assert!(rpc_release_rollout.contains("roze_rpc_requests_total"));
        assert!(rpc_release_rollout.contains("max_decision_time: 5m"));
        assert!(rpc_incident_response.contains("boundary: rpc"));
        assert!(rpc_incident_response.contains("representative_rpc_call_and_client_deadline_probe"));
        assert!(rpc_incident_response.contains("alert: RozeGeneratedServiceHighP99Latency"));
        assert!(rpc_incident_response.contains("ConfigReloadRejectedOrRolledBack"));
        assert!(rpc_incident_response.contains("roze_rpc_requests_total"));
        assert!(rpc_capacity_plan.contains("boundary: rpc"));
        assert!(rpc_capacity_plan.contains("rpc_calls_per_second"));
        assert!(rpc_capacity_plan.contains("representative_rpc_workload_with_client_deadlines"));
        assert!(rpc_capacity_plan.contains("soak_72h_passed_for_broad_stability_claim"));
        assert!(rpc_capacity_plan.contains("roze_rpc_method_duration_seconds_bucket"));
        assert!(rpc_security_readiness.contains("boundary: rpc"));
        assert!(rpc_security_readiness.contains(
            "representative_rpc_call_with_valid_expired_missing_and_malformed_credentials"
        ));
        assert!(rpc_security_readiness.contains("rpc_method_and_proto_service_security_projection"));
        assert!(rpc_security_readiness.contains("security_owner_signoff: required"));
        assert!(rpc_security_readiness.contains("revoked_key_accepted"));
        assert!(rpc_production_gate.contains("boundary: rpc"));
        assert!(rpc_production_gate.contains("ops/interface-governance.yaml"));
        assert!(rpc_production_gate.contains("cargo_check_generated_rpc_service"));
        assert!(rpc_production_gate.contains("stage: interface_governance"));
        assert!(rpc_production_verify.contains("function Invoke-Step"));
        assert!(rpc_production_verify.contains("$requiredOpsFiles"));
        assert!(rpc_production_verify.contains("$EvidenceManifestPath"));
        assert!(rpc_production_verify.contains("$CiEvidencePolicyPath"));
        assert!(rpc_production_verify.contains("$VerifyReportPath"));
        assert!(rpc_production_verify.contains("evidence manifest coverage"));
        assert!(rpc_production_verify.contains("$manifestEntry = \"path: $relativePath\""));
        assert!(rpc_production_verify.contains("ci evidence policy coverage"));
        assert!(rpc_production_verify.contains("$policyEntry = \"    - $relativePath\""));
        assert!(rpc_production_verify
            .contains("CI evidence policy does not require generated asset path"));
        assert!(rpc_production_verify.contains("ops/production-verify.ps1"));
        assert!(rpc_production_verify.contains("ops/production-verify.sh"));
        assert!(rpc_production_verify.contains("ops/ci-evidence-policy.yaml"));
        assert!(rpc_production_verify.contains("ops/evidence-manifest.yaml"));
        assert!(rpc_production_verify.contains(".github/workflows/roze-production-verify.yml"));
        assert!(rpc_production_verify.contains("cargo check --manifest-path"));
        assert!(rpc_production_verify.contains("production-verify-report.json"));
        assert!(rpc_production_verify.contains("pass_ci_precondition"));
        assert!(rpc_production_verify.contains("requires_long_run_evidence"));
        assert!(rpc_production_verify.contains("required_followup_evidence"));
        assert!(rpc_production_verify.contains("Production verification report"));
        assert!(rpc_production_verify.contains("RPC smoke methods required"));
        assert!(rpc_production_verify.contains("GetUser -> GetUserReq"));
        assert!(rpc_production_verify_sh.starts_with("#!/usr/bin/env bash"));
        assert!(rpc_production_verify_sh.contains("set -euo pipefail"));
        assert!(rpc_production_verify_sh.contains("run_step()"));
        assert!(rpc_production_verify_sh.contains("CI_EVIDENCE_POLICY_PATH"));
        assert!(rpc_production_verify_sh.contains("VERIFY_REPORT_PATH"));
        assert!(rpc_production_verify_sh.contains("check_evidence_manifest_coverage()"));
        assert!(rpc_production_verify_sh.contains("manifest_entry=\"path: $relative_path\""));
        assert!(rpc_production_verify_sh.contains("check_ci_evidence_policy_coverage()"));
        assert!(rpc_production_verify_sh.contains("ci evidence policy coverage"));
        assert!(rpc_production_verify_sh.contains("policy_entry=\"    - $relative_path\""));
        assert!(rpc_production_verify_sh
            .contains("CI evidence policy does not require generated asset path"));
        assert!(rpc_production_verify_sh.contains("ops/production-verify.ps1"));
        assert!(rpc_production_verify_sh.contains("ops/production-verify.sh"));
        assert!(rpc_production_verify_sh.contains("ops/ci-evidence-policy.yaml"));
        assert!(rpc_production_verify_sh.contains("ops/evidence-manifest.yaml"));
        assert!(rpc_production_verify_sh.contains(".github/workflows/roze-production-verify.yml"));
        assert!(rpc_production_verify_sh.contains("cargo check --manifest-path"));
        assert!(rpc_production_verify_sh.contains("production-verify-report.json"));
        assert!(rpc_production_verify_sh.contains("pass_ci_precondition"));
        assert!(rpc_production_verify_sh.contains("requires_long_run_evidence"));
        assert!(rpc_production_verify_sh.contains("required_followup_evidence"));
        assert!(rpc_production_verify_sh.contains("Production verification report"));
        assert!(rpc_production_verify_sh.contains("RPC smoke methods required"));
        assert!(rpc_production_verify_sh.contains("GetUser -> GetUserReq"));
        assert!(rpc_production_workflow.contains("ROZE_SERVICE_NAME: user"));
        assert!(rpc_production_workflow.contains("ROZE_BOUNDARY: rpc"));
        assert!(rpc_production_workflow.contains("ubuntu-latest"));
        assert!(rpc_production_workflow.contains("windows-latest"));
        assert!(rpc_production_workflow.contains("bash ops/production-verify.sh"));
        assert!(rpc_production_workflow.contains("ops\\production-verify.ps1"));
        assert!(rpc_production_workflow.contains("actions/upload-artifact@v4"));
        assert!(rpc_production_workflow.contains("roze-production-evidence-${{ matrix.os }}"));
        assert!(rpc_production_workflow.contains("ops/**"));
        assert!(rpc_ci_evidence_policy.contains("service: user"));
        assert!(rpc_ci_evidence_policy.contains("boundary: rpc"));
        assert!(rpc_ci_evidence_policy
            .contains("startup_readiness_metrics_and_representative_rpc_methods"));
        assert!(rpc_ci_evidence_policy.contains("test_failure_without_approved_skip"));
        assert!(rpc_ci_evidence_policy.contains("failure_injection_report"));
        assert!(rpc_ci_evidence_policy.contains("    - ops/production-gate.yaml"));
        assert!(rpc_ci_evidence_policy.contains("    - ops/production-verify.ps1"));
        assert!(rpc_ci_evidence_policy.contains("    - ops/production-verify.sh"));
        assert!(rpc_ci_evidence_policy.contains("    - ops/ci-evidence-policy.yaml"));
        assert!(rpc_ci_evidence_policy.contains("ops/evidence-manifest.yaml"));
        assert!(
            rpc_ci_evidence_policy.contains("    - .github/workflows/roze-production-verify.yml")
        );
        assert!(rpc_ci_evidence_policy.contains("produced_paths:"));
        assert!(rpc_ci_evidence_policy.contains("    - ops/production-verify-report.json"));
        assert!(rpc_evidence_manifest.contains("service: user"));
        assert!(rpc_evidence_manifest.contains("boundary: rpc"));
        assert!(rpc_evidence_manifest.contains("path: ops/production-verify.ps1"));
        assert!(rpc_evidence_manifest.contains("path: ops/evidence-manifest.yaml"));
        assert!(rpc_evidence_manifest.contains("runtime_artifacts:"));
        assert!(rpc_evidence_manifest.contains("path: ops/production-verify-report.json"));
        assert!(rpc_evidence_manifest.contains("kind: verification_report"));
        assert!(rpc_evidence_manifest.contains("startup_readiness_metrics"));
        assert!(rpc_evidence_manifest.contains("client_deadline_and_cancel"));
        assert!(rpc_evidence_manifest.contains("name: \"GetUser\""));
        assert!(rpc_evidence_manifest.contains("request: \"GetUserReq\""));
        assert!(rpc_evidence_manifest.contains("broad_production_requires_long_run_evidence: true"));
        assert!(
            rpc_production_gate.contains("startup_readiness_metrics_and_representative_rpc_call")
        );
        assert!(rpc_production_gate.contains("controlled_production:"));
        assert!(rpc_production_gate.contains("security_blocking_finding_present"));
        assert!(rpc_regeneration_policy.contains("boundary: rpc"));
        assert!(rpc_regeneration_policy.contains("proto/service.proto"));
        assert!(rpc_regeneration_policy
            .contains("rpc_method_request_response_proto_service_security_projection"));
        assert!(rpc_regeneration_policy
            .contains("generated_owned_file_changed_without_idl_or_generator_change"));
        assert!(rpc_client_contract.contains("boundary: rpc"));
        assert!(rpc_client_contract.contains("proto/service.proto_and_generated_tonic_client"));
        assert!(rpc_client_contract.contains("rust_tonic_clients_and_proto_consumers"));
        assert!(rpc_client_contract.contains("generated_client_calls_representative_rpc_method"));
        assert!(rpc_client_contract.contains("typed_error_missing_trace_id"));
        assert!(rpc_config_governance.contains("boundary: rpc"));
        assert!(rpc_config_governance
            .contains("reload_timeout_registry_client_deadline_and_dependency_config"));
        assert!(rpc_config_governance.contains("config_center_unavailable_breaks_startup"));
        assert!(rpc_config_governance.contains("snapshot_restore_tested: true"));
        assert!(rpc_reliable_events.contains("boundary: rpc"));
        assert!(rpc_reliable_events
            .contains("representative_rpc_mutation_publishes_or_declares_no_event"));
        assert!(rpc_reliable_events.contains("roze_event_consumer_lag"));
        assert!(rpc_reliable_events.contains("event_without_idempotency_key"));
        assert!(rpc_dependency_governance.contains("boundary: rpc"));
        assert!(rpc_dependency_governance
            .contains("representative_rpc_method_calls_declared_downstream_or_declares_none"));
        assert!(rpc_dependency_governance.contains("endpoint_change_test_passed: true"));
        assert!(rpc_dependency_governance.contains("fallback_claim_without_test_evidence"));
        assert!(rpc_data_consistency.contains("boundary: rpc"));
        assert!(rpc_data_consistency
            .contains("representative_rpc_mutation_declares_transaction_or_no_persistence"));
        assert!(rpc_data_consistency.contains("dtm_or_saga:"));
        assert!(rpc_data_consistency.contains("backup_restore_test_passed_for_broad_production"));
        assert!(rpc_observability.contains("boundary: rpc"));
        assert!(rpc_observability.contains("roze_rpc_requests_total"));
        assert!(rpc_observability.contains("service_method_status"));
        assert!(rpc_observability.contains("trace_propagation"));
        assert!(rpc_observability.contains("no_debug_query_for_primary_slo"));
        assert!(rpc_runtime_hardening.contains("boundary: rpc"));
        assert!(rpc_runtime_hardening
            .contains("representative_rpc_call_with_client_deadline_and_cancel"));
        assert!(rpc_runtime_hardening.contains("timeout_and_deadline"));
        assert!(rpc_runtime_hardening.contains("breaker_and_retry_budget"));
        assert!(rpc_runtime_hardening.contains("unbounded_retry_amplification"));
        assert!(rpc_error_contract.contains("boundary: rpc"));
        assert!(rpc_error_contract.contains("grpc_status_mapping:"));
        assert!(rpc_error_contract.contains("representative_rpc_status_and_metadata"));
        assert!(rpc_error_contract.contains("retryable_mutation_without_idempotency_policy"));
        assert!(rpc_error_contract.contains("transport_status_mapping_missing"));
        assert!(rpc_deployment_topology.contains("boundary: rpc"));
        assert!(rpc_deployment_topology.contains("port: 50051"));
        assert!(rpc_deployment_topology.contains("grpc_health_probe_readiness"));
        assert!(rpc_deployment_topology.contains("representative_rpc_method_probe"));
        assert!(rpc_deployment_topology.contains("rollback_action_not_tested"));
        assert!(rpc_service_communication.contains("boundary: rpc"));
        assert!(rpc_service_communication
            .contains("generated_tonic_client_and_service_context_rpc_clients"));
        assert!(rpc_service_communication
            .contains("representative_rpc_method_calls_declared_downstream_or_declares_none"));
        assert!(rpc_service_communication.contains("retry_without_budget_or_jitter"));
        assert!(rpc_service_communication.contains("missing_trace_context_on_downstream_call"));
        assert!(rpc_cache_governance.contains("boundary: rpc"));
        assert!(rpc_cache_governance
            .contains("representative_rpc_query_declares_cache_policy_or_no_cache"));
        assert!(rpc_cache_governance.contains("roze_singleflight_collapses_total"));
        assert!(rpc_cache_governance.contains("mutation_without_cache_invalidation_policy"));
        assert!(rpc_cache_governance.contains("stale_data_served_without_declared_policy"));
        assert!(rpc_data_access_governance.contains("boundary: rpc"));
        assert!(rpc_data_access_governance
            .contains("representative_rpc_query_declares_data_access_policy_or_no_persistence"));
        assert!(rpc_data_access_governance.contains("roze_data_pool_acquire_seconds"));
        assert!(rpc_data_access_governance.contains("data_access_without_timeout_or_deadline"));
        assert!(rpc_data_access_governance.contains("multi_statement_write_without_transaction"));
        assert!(rpc_interface_governance.contains("boundary: rpc"));
        assert!(rpc_interface_governance.contains("rpc_methods:"));
        assert!(rpc_interface_governance.contains("name: GetUser"));
        assert!(rpc_interface_governance.contains("representative_rpc_call: required"));
        assert!(rpc_interface_governance.contains("client_deadline_and_cancel: required"));
        assert!(rpc_interface_governance.contains("openapi_or_proto_missing_interface"));

        fs::remove_dir_all(root).expect("remove runbook test root");
    }

    #[test]
    fn update_preserves_business_files_and_refreshes_generated_files() {
        let spec = parse_api(
            r#"
            service user-api {
                @server (
                    middleware: tenantGuard
                )
                @handler getUser
                get /users/:id (GetUserReq) returns (UserResp)
            }

            type GetUserReq {
                id: u64
            }

            type UserResp {
                name: string
            }
            "#,
        )
        .expect("valid api");
        let root = std::env::temp_dir().join(format!(
            "rozectl-update-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time")
                .as_nanos()
        ));
        let out = root.join("user");
        fs::create_dir_all(&root).expect("create test workspace");
        fs::write(root.join("Cargo.toml"), "[workspace]\nmembers = []\n")
            .expect("write workspace manifest");

        generate_rest_project(
            &spec,
            &out,
            GenerateOptions::new(GenerateMode::Create, DependencySource::Path),
        )
        .expect("initial generation");
        fs::write(out.join("src/logic/users/get_user.rs"), "// custom logic\n")
            .expect("write custom logic");
        fs::write(
            out.join("src/middleware/tenant_guard.rs"),
            "// custom middleware\n",
        )
        .expect("write custom middleware");
        fs::write(
            out.join("src/logic/users/mod.rs"),
            "mod get_user;\npub use get_user::{get_user, AdminTokenReq};\nmod catalog_map;\n",
        )
        .expect("write custom logic group mod");
        let svc_path = out.join("src/svc/mod.rs");
        let svc = fs::read_to_string(&svc_path).expect("read svc");
        fs::write(
            &svc_path,
            format!(
                "{svc}\nimpl ServiceContext {{\n    pub fn catalog(&self) -> anyhow::Result<()> {{\n        Ok(())\n    }}\n}}\n"
            ),
        )
        .expect("write custom svc extension");
        fs::write(out.join("config.yaml"), "name: custom\n").expect("write custom config");
        fs::write(out.join("src/handler/mod.rs"), "// stale handler\n")
            .expect("write stale handler");
        let cargo_path = out.join("Cargo.toml");
        let cargo = fs::read_to_string(&cargo_path)
            .expect("read initial cargo")
            .replace("name = \"user\"", "name = \"custom-service\"")
            .replace(
                "anyhow.workspace = true",
                "anyhow.workspace = true\ncustom.workspace = true",
            );
        fs::write(&cargo_path, cargo).expect("write custom cargo");

        generate_rest_project(
            &spec,
            &out,
            GenerateOptions::new(GenerateMode::Update, DependencySource::Git),
        )
        .expect("update generation");

        assert_eq!(
            fs::read_to_string(out.join("src/logic/users/get_user.rs")).expect("read logic"),
            "// custom logic\n"
        );
        assert_eq!(
            fs::read_to_string(out.join("src/middleware/tenant_guard.rs"))
                .expect("read middleware"),
            "// custom middleware\n"
        );
        assert_eq!(
            fs::read_to_string(out.join("config.yaml")).expect("read config"),
            "name: custom\n"
        );
        assert!(fs::read_to_string(out.join("src/route/mod.rs"))
            .expect("read route")
            .contains("pub fn router"));
        assert!(fs::read_to_string(out.join("src/route/users.rs"))
            .expect("read user routes")
            .contains("handler::users::get_user"));
        assert!(fs::read_to_string(out.join("src/middleware/mod.rs"))
            .expect("read middleware mod")
            .contains("pub use tenant_guard::tenant_guard;"));
        assert!(fs::read_to_string(out.join("src/logic/users/mod.rs"))
            .expect("read logic group mod")
            .contains("mod catalog_map;"));
        assert!(fs::read_to_string(out.join("src/logic/users/mod.rs"))
            .expect("read logic group mod")
            .contains("pub use get_user::{get_user, AdminTokenReq};"));
        assert!(fs::read_to_string(out.join("src/svc/mod.rs"))
            .expect("read svc")
            .contains("pub fn catalog(&self)"));
        let cargo = fs::read_to_string(out.join("Cargo.toml")).expect("read cargo");
        assert!(cargo.contains(ROZE_GIT_URL));
        assert!(cargo.contains(r#"name = "custom-service""#));
        assert!(cargo.contains("custom.workspace = true"));

        fs::remove_dir_all(root).expect("remove test output");
    }

    #[test]
    fn api_update_preserves_config_module_and_refreshes_handler_adapters() {
        let spec = parse_api(
            r#"
            service admin-api {
                @handler authMe
                get /admin/auth/me returns (AdminInfoResp)
            }

            type AdminInfoResp {
                id: u64
            }
            "#,
        )
        .expect("valid api");
        let root = temp_test_root("rozectl-api-update-integration-hooks-test");
        let out = root.join("admin");
        fs::create_dir_all(&root).expect("create test workspace");
        fs::write(root.join("Cargo.toml"), "[workspace]\nmembers = []\n")
            .expect("write workspace manifest");

        generate_rest_project(
            &spec,
            &out,
            GenerateOptions::new(GenerateMode::Create, DependencySource::Path),
        )
        .expect("initial generation");

        fs::write(
            out.join("src/config/mod.rs"),
            "pub struct ServiceConfig {\n    pub rpc_clients: std::collections::BTreeMap<String, String>,\n}\n",
        )
        .expect("write custom config module");
        fs::write(
            out.join("src/handler/admin/auth_me.rs"),
            "use roze_http::http::HeaderMap;\n\npub async fn auth_me(headers: HeaderMap) -> roze_result::Result<()> {\n    let _ = headers;\n    Ok(())\n}\n",
        )
        .expect("write custom handler adapter");
        fs::write(
            out.join("src/handler/admin/mod.rs"),
            "// stale admin handler mod\n",
        )
        .expect("write stale handler group mod");

        generate_rest_project(
            &spec,
            &out,
            GenerateOptions::new(GenerateMode::Update, DependencySource::Git),
        )
        .expect("update generation");

        assert!(fs::read_to_string(out.join("src/config/mod.rs"))
            .expect("read config module")
            .contains("rpc_clients"));
        let handler = fs::read_to_string(out.join("src/handler/admin/auth_me.rs"))
            .expect("read handler adapter");
        assert!(!handler.contains("pub async fn auth_me(headers: HeaderMap)"));
        assert!(handler.contains("let req = EmptyReq {};"));
        assert!(fs::read_to_string(out.join("src/handler/admin/mod.rs"))
            .expect("read handler group mod")
            .contains("pub(crate) use auth_me::auth_me;"));

        fs::remove_dir_all(root).expect("remove test output");
    }

    #[test]
    fn api_update_refreshes_handler_when_route_request_changes() {
        let initial = parse_api(
            r#"
            service user-api {
                @handler getUser
                get /users/current returns (UserResp)
            }

            type UserResp {
                id: u64
            }
            "#,
        )
        .expect("valid initial api");
        let updated = parse_api(
            r#"
            service user-api {
                @handler getUser
                get /users/current (GetUserReq) returns (UserResp)
            }

            type GetUserReq {
                include_profile: bool `query:"includeProfile"`
            }

            type UserResp {
                id: u64
            }
            "#,
        )
        .expect("valid updated api");
        let root = temp_test_root("rozectl-api-update-handler-request-test");
        let out = root.join("user");
        fs::create_dir_all(&root).expect("create test workspace");
        fs::write(root.join("Cargo.toml"), "[workspace]\nmembers = []\n")
            .expect("write workspace manifest");

        generate_rest_project(
            &initial,
            &out,
            GenerateOptions::new(GenerateMode::Create, DependencySource::Path),
        )
        .expect("initial generation");
        fs::write(
            out.join("src/logic/users/get_user.rs"),
            "// application-owned logic\n",
        )
        .expect("write application logic");

        generate_rest_project(
            &updated,
            &out,
            GenerateOptions::new(GenerateMode::Update, DependencySource::Git),
        )
        .expect("update generation");

        let handler = fs::read_to_string(out.join("src/handler/users/get_user.rs"))
            .expect("read refreshed handler");
        assert!(handler.contains("Query(query): Query<GetUserGetUserReqQuery>"));
        assert!(handler.contains("let req = GetUserReq"));
        assert!(!handler.contains("let req = EmptyReq {};"));
        assert_eq!(
            fs::read_to_string(out.join("src/logic/users/get_user.rs"))
                .expect("read application logic"),
            "// application-owned logic\n"
        );

        fs::remove_dir_all(root).expect("remove test output");
    }

    #[test]
    fn rpc_update_preserves_config_module_extensions() {
        let spec = parse_api(
            r#"
            service catalog-rpc {
                rpc ListProducts (ListProductsReq) returns (ListProductsResp)
            }

            type ListProductsReq {
                page: u64
            }

            type ListProductsResp {
                total: u64
            }
            "#,
        )
        .expect("valid api");
        let root = temp_test_root("rozectl-rpc-update-config-hooks-test");
        let out = root.join("catalog");
        fs::create_dir_all(&root).expect("create test workspace");
        fs::write(root.join("Cargo.toml"), "[workspace]\nmembers = []\n")
            .expect("write workspace manifest");

        generate_rpc_project(
            &spec,
            &out,
            GenerateOptions::new(GenerateMode::Create, DependencySource::Path),
        )
        .expect("initial generation");
        fs::write(
            out.join("src/config/mod.rs"),
            "pub struct ServiceConfig {\n    pub registry_namespace: String,\n}\n",
        )
        .expect("write custom config module");

        generate_rpc_project(
            &spec,
            &out,
            GenerateOptions::new(GenerateMode::Update, DependencySource::Git),
        )
        .expect("update generation");

        assert!(fs::read_to_string(out.join("src/config/mod.rs"))
            .expect("read config module")
            .contains("registry_namespace"));

        fs::remove_dir_all(root).expect("remove test output");
    }

    #[test]
    fn rpc_update_preserves_custom_logic_module_declarations() {
        let spec = parse_api(
            r#"
            service promotion-rpc {
                rpc ListCoupons (ListCouponsReq) returns (ListCouponsResp)
            }

            type ListCouponsReq {
                page: u64
            }

            type ListCouponsResp {
                total: u64
            }
            "#,
        )
        .expect("valid api");
        let root = temp_test_root("rozectl-rpc-update-logic-mod-test");
        let out = root.join("promotion");
        fs::create_dir_all(&root).expect("create test workspace");
        fs::write(root.join("Cargo.toml"), "[workspace]\nmembers = []\n")
            .expect("write workspace manifest");

        generate_rpc_project(
            &spec,
            &out,
            GenerateOptions::new(GenerateMode::Create, DependencySource::Path),
        )
        .expect("initial generation");
        fs::write(out.join("src/logic/coupon_map.rs"), "// custom helper\n")
            .expect("write custom helper");
        fs::write(
            out.join("src/logic/mod.rs"),
            "mod list_coupons;\npub use list_coupons::list_coupons;\nmod coupon_map;\n",
        )
        .expect("write custom logic mod");

        generate_rpc_project(
            &spec,
            &out,
            GenerateOptions::new(GenerateMode::Update, DependencySource::Git),
        )
        .expect("update generation");

        let logic_mod = fs::read_to_string(out.join("src/logic/mod.rs")).expect("read logic mod");
        assert!(logic_mod.contains("mod list_coupons;"));
        assert!(logic_mod.contains("pub use list_coupons::list_coupons;"));
        assert!(logic_mod.contains("mod coupon_map;"));
        assert_eq!(
            fs::read_to_string(out.join("src/logic/coupon_map.rs")).expect("read helper"),
            "// custom helper\n"
        );

        fs::remove_dir_all(root).expect("remove test output");
    }

    #[test]
    fn rpc_update_refreshes_legacy_default_logic_stub() {
        let spec = parse_api(
            r#"
            service fulfillment {
                rpc CreateAftersales (CreateAftersalesReq) returns (AftersalesOrder)
            }

            type CreateAftersalesReq {
                order_id: string
            }

            type AftersalesOrder {
                id: i64
                aftersales_no: string
                order_id: string
                status: string
                type: string
                refund_amount: string
                created_at: i64
            }
            "#,
        )
        .expect("valid api");
        let root = temp_test_root("rozectl-rpc-update-legacy-logic-test");
        let out = root.join("fulfillment");
        fs::create_dir_all(&root).expect("create test workspace");
        fs::write(root.join("Cargo.toml"), "[workspace]\nmembers = []\n")
            .expect("write workspace manifest");

        generate_rpc_project(
            &spec,
            &out,
            GenerateOptions::new(GenerateMode::Create, DependencySource::Path),
        )
        .expect("initial generation");

        let logic_path = out.join("src/logic/create_aftersales.rs");
        fs::write(
            &logic_path,
            r#"use super::*;

pub async fn create_aftersales(ctx: ServiceContext, request_ctx: roze_context::Context, req: CreateAftersalesReq) -> Result<AftersalesOrder, RozeError> {
    let _ = ctx;
    let _ = request_ctx;
    let _ = req;
    Ok(AftersalesOrder {
        id: Default::default(),
        aftersales_no: String::new(),
        order_id: String::new(),
        status: String::new(),
    })
}
"#,
        )
        .expect("write legacy logic");

        generate_rpc_project(
            &spec,
            &out,
            GenerateOptions::new(GenerateMode::Update, DependencySource::Path),
        )
        .expect("update generation");

        let logic = fs::read_to_string(logic_path).expect("read logic");
        assert!(logic.contains("Ok(AftersalesOrder::default())"));
        assert!(!logic.contains("Ok(AftersalesOrder {"));

        fs::remove_dir_all(root).expect("remove test output");
    }

    #[test]
    fn registers_new_project_in_nearest_workspace() {
        let root = std::env::temp_dir().join(format!(
            "rozectl-workspace-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time")
                .as_nanos()
        ));
        let out = root.join("apps/demo");
        fs::create_dir_all(&out).expect("create project output");
        fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nmembers = [\n]\nresolver = \"2\"\n",
        )
        .expect("write workspace manifest");

        register_workspace_member(&out).expect("register workspace member");

        assert_eq!(
            fs::read_to_string(root.join("Cargo.toml")).expect("read workspace manifest"),
            "[workspace]\nmembers = [\n    \"apps/demo\",\n]\nresolver = \"2\"\n"
        );

        fs::remove_dir_all(root).expect("remove test workspace");
    }

    #[test]
    fn local_dependency_path_matches_project_depth() {
        let root = std::env::temp_dir().join(format!(
            "rozectl-path-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time")
                .as_nanos()
        ));
        fs::create_dir_all(root.join("apps/user")).expect("create nested project");
        fs::create_dir_all(root.join("user")).expect("create root project");
        fs::write(root.join("Cargo.toml"), "[workspace]\nmembers = []\n")
            .expect("write workspace manifest");

        assert_eq!(
            local_crates_prefix(&root.join("user"), &root).expect("root project prefix"),
            "../crates"
        );
        assert_eq!(
            local_crates_prefix(&root.join("apps/user"), &root).expect("nested project prefix"),
            "../../crates"
        );

        fs::remove_dir_all(root).expect("remove test workspace");
    }

    #[test]
    fn creates_rpc_project_from_builtin_template() {
        let root = std::env::temp_dir().join(format!(
            "rozectl-rpc-new-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time")
                .as_nanos()
        ));
        let out = root.join("apps/user");
        fs::create_dir_all(root.join("apps")).expect("create apps directory");
        fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nmembers = [\n]\nresolver = \"2\"\n\n[workspace.package]\nedition = \"2021\"\nlicense = \"MIT\"\nversion = \"0.1.0\"\n",
        )
        .expect("write workspace manifest");

        create_rpc_project(
            "user",
            &out,
            GenerateOptions::new(GenerateMode::Create, DependencySource::Git),
        )
        .expect("create RPC project");

        assert!(out.join("user.api").is_file());
        assert!(fs::read_to_string(out.join("proto/service.proto"))
            .expect("read proto")
            .contains("rpc GetUser (GetUserReq) returns (GetUserResp);"));
        assert!(fs::read_to_string(out.join("Cargo.toml"))
            .expect("read project manifest")
            .contains(r#"edition = "2021""#));
        assert!(!fs::read_to_string(out.join("Cargo.toml"))
            .expect("read project manifest")
            .contains("edition.workspace = true"));
        assert!(fs::read_to_string(out.join("Cargo.toml"))
            .expect("read project manifest")
            .contains(ROZE_GIT_URL));
        assert!(fs::read_to_string(root.join("Cargo.toml"))
            .expect("read workspace manifest")
            .contains(r#""apps/user""#));

        fs::remove_dir_all(root).expect("remove test workspace");
    }

    #[test]
    fn creates_standalone_rpc_project_without_workspace() {
        let root = std::env::temp_dir().join(format!(
            "rozectl-standalone-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time")
                .as_nanos()
        ));
        let out = root.join("demo");

        create_rpc_project(
            "demo",
            &out,
            GenerateOptions::new(GenerateMode::Create, DependencySource::Git),
        )
        .expect("create standalone RPC project");

        let cargo = fs::read_to_string(out.join("Cargo.toml")).expect("read project manifest");
        assert!(cargo.contains(r#"edition = "2021""#));
        assert!(cargo.contains(r#"anyhow = "1""#));
        assert!(!cargo.contains(".workspace = true"));
        assert!(out.join("demo.api").is_file());
        assert_eq!(
            fs::read_to_string(out.join(".cargo/config.toml")).expect("read cargo config"),
            "[net]\ngit-fetch-with-cli = true\n"
        );

        fs::remove_dir_all(root).expect("remove standalone project");
    }

    #[test]
    fn writes_service_markdown_doc_from_api_contract() {
        let root = std::env::temp_dir().join(format!(
            "rozectl-service-doc-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time")
                .as_nanos()
        ));
        fs::create_dir_all(&root).expect("create root");
        let api = root.join("user.api");
        let out = root.join("SERVICE.md");
        fs::write(
            &api,
            r#"
            service user-api {
                @handler getUser
                get /users/:id (GetUserReq) returns (UserResp)
                rpc Ping (PingReq) returns (PingResp)
            }

            type (
                GetUserReq {
                    id: i64
                }

                UserResp {
                    id: i64
                }

                PingReq {
                    traceId: string
                }

                PingResp {
                    ok: bool
                }
            )
            "#,
        )
        .expect("write api");

        write_service_markdown_doc(&api, &out, false).expect("write service doc");

        let content = fs::read_to_string(&out).expect("read service doc");
        assert!(content.contains("# user-api Service"));
        assert!(content.contains("| GET | `/users/:id` | `getUser` | `GetUserReq` | `UserResp` |"));
        assert!(content.contains("| `Ping` | `PingReq` | `PingResp` |"));
        assert!(content.contains("`src/logic/**` | application | preserved during `--update`"));
        assert!(content
            .contains("`src/svc/mod.rs` | application/dependencies | preserved during `--update`"));
        assert!(content.contains("rozectl diff api"));
        assert!(write_service_markdown_doc(&api, &out, false).is_err());
        write_service_markdown_doc(&api, &out, true).expect("force service doc");

        fs::remove_dir_all(root).expect("remove service doc project");
    }

    #[test]
    fn renders_mock_server_from_api_contract() {
        let spec = parse_api(
            r#"
            @server(
                prefix: /api
            )
            service user-api {
                @handler getUser
                get /users/:id (GetUserReq) returns (UserResp)
            }

            type GetUserReq {
                id string `path:"id"`
            }

            type UserResp {
                id string `json:"id"`
                active bool `json:"active"`
                score i64 `json:"score"`
                tags []string `json:"tags"`
            }
            "#,
        )
        .expect("valid api");

        let main = render_mock_main(&spec);

        assert!(main.contains(r#".route("/api/users/{id}", get(getuser_0))"#));
        assert!(main.contains(r#""id": "string""#));
        assert!(main.contains(r#""active": true"#));
        assert!(main.contains(r#""score": 1"#));
        assert!(main.contains(r#""tags": ["#));
    }

    #[test]
    fn writes_http_smoke_test_project_from_api_contract() {
        let root = temp_test_root("rozectl-contract-test-gen");
        fs::create_dir_all(&root).expect("create contract test root");
        let api = root.join("user.api");
        let out = root.join("contract-tests");
        fs::write(
            &api,
            r#"
            @server(
                prefix: /api
            )
            service user-api {
                @handler getUser
                get /users/:id (GetUserReq) returns (UserResp)

                @handler createUser
                post /users (CreateUserReq) returns (UserResp)
            }

            type GetUserReq {
                id string `path:"id"`
                traceId string `header:"x-trace-id"`
                verbose bool `query:"verbose,optional"`
            }

            type CreateUserReq {
                name string `json:"name"`
            }

            type UserResp {
                id string `json:"id"`
            }
            "#,
        )
        .expect("write api");

        write_http_smoke_test_project(&api, &out, "http://127.0.0.1:3000", false)
            .expect("write contract tests");

        let cargo = fs::read_to_string(out.join("Cargo.toml")).expect("read cargo");
        let tests = fs::read_to_string(out.join("tests/http_smoke.rs")).expect("read tests");
        assert!(cargo.contains(r#"name = "user-api-contract-tests""#));
        assert!(tests.contains(r#"std::env::var("ROZE_TEST_BASE_URL")"#));
        assert!(tests.contains("let mut request = client.get(url)"));
        assert!(tests.contains("let fixture = fixtures::request("));
        assert!(tests.contains("assertions::assert_route"));
        assert!(tests.contains("async fn smoke_framework_healthz()"));
        assert!(tests.contains("async fn smoke_framework_report_export()"));
        assert!(tests.contains("async fn smoke_framework_chart_query()"));
        assert!(tests.contains(r#""/api/reports/export""#));
        assert!(tests.contains(r#".query(&[("report", "smoke"), ("format", "csv")"#));
        assert!(tests.contains(r#""/api/charts/query""#));
        assert!(tests.contains(r#""/api/users/string""#));
        assert!(tests.contains(r#".header("x-trace-id", "string")"#));
        assert!(tests.contains(r#".query(&[("verbose", "true")])"#));
        assert!(tests.contains(r#""name": "string""#));
        let readme = fs::read_to_string(out.join("README.md")).expect("read readme");
        assert!(readme.contains("## Framework Smoke"));
        assert!(readme.contains("`GET` `/api/reports/export`"));
        assert!(readme.contains("`GET` `/api/charts/query`"));
        let fixtures_path = out.join("tests/fixtures.rs");
        let assertions_path = out.join("tests/assertions.rs");
        let generated_fixtures = fs::read_to_string(&fixtures_path).expect("read fixtures");
        let multi_service = fs::read_to_string(out.join("tests/multi_service_smoke.rs"))
            .expect("read multi-service smoke");
        assert!(generated_fixtures.contains("ROZE_E2E_SERVICES"));
        assert!(multi_service.contains("fixtures::services()"));
        assert!(multi_service.contains("assert_service_ready"));
        fs::write(&fixtures_path, "// application fixtures\n").expect("customize fixtures");
        fs::write(&assertions_path, "// application assertions\n").expect("customize assertions");
        write_http_smoke_test_project(&api, &out, "http://127.0.0.1:3000", false)
            .expect("update contract tests");
        assert_eq!(
            fs::read_to_string(&fixtures_path).expect("read fixtures"),
            "// application fixtures\n"
        );
        assert_eq!(
            fs::read_to_string(&assertions_path).expect("read assertions"),
            "// application assertions\n"
        );
        write_http_smoke_test_project(&api, &out, "http://127.0.0.1:3000", true)
            .expect("force contract tests");
        assert_eq!(
            fs::read_to_string(&fixtures_path).expect("read fixtures after force"),
            "// application fixtures\n"
        );

        fs::remove_dir_all(root).expect("remove contract test project");
    }

    #[test]
    fn rejects_unsupported_proto_field_type() {
        let spec = parse_api(
            r#"
            service user-api {
                post /users (CreateUserReq) returns (UserResp)
            }

            type CreateUserReq {
                profile: Profile
            }

            type UserResp {
                id: u64
            }
            "#,
        )
        .expect("valid api");

        let err = render_proto(&spec).expect_err("unsupported type should fail");

        assert!(err
            .to_string()
            .contains("unsupported proto field type `Profile`"));
    }

    #[test]
    fn allows_known_message_field_type() {
        let spec = parse_api(
            r#"
            service user-api {
                post /users (CreateUserReq) returns (UserResp)
            }

            type CreateUserReq {
                profile: Profile
            }

            type Profile {
                displayName: string
            }

            type UserResp {
                id: u64
            }
            "#,
        )
        .expect("valid api");

        let proto = render_proto(&spec).expect("known message type should render");

        assert!(proto.contains("Profile profile = 1;"));
    }

    #[test]
    fn route_name_from_path_strips_path_syntax() {
        assert_eq!(
            route_name_from_path("/teams/{team-id}/users/:id"),
            "teams_team_id_users_id"
        );
    }

    #[test]
    fn implicit_route_rpc_name_stays_aligned_with_handler_name() {
        let route = crate::parser::RestRoute {
            handler: None,
            doc: None,
            middlewares: Vec::new(),
            permissions: Vec::new(),
            server: None,
            method: HttpMethod::Get,
            path: "/users/:id".to_string(),
            request: "GetUserReq".to_string(),
            response: "UserResp".to_string(),
        };
        let name = format!("get_{}", route_name_from_path(&route.path));

        assert_eq!(to_snake_case(&to_pascal_case(&name)), "get_users_id");
    }
}
