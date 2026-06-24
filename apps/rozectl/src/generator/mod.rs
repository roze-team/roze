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

use crate::parser::{ApiSpec, HttpMethod};

const ROZE_GIT_URL: &str = "https://github.com/roze-team/roze.git";
const REST_ROZE_CRATES: [&str; 16] = [
    "roze-config",
    "roze-error",
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
    "roze-result",
    "roze-transaction",
    "roze-validation",
    "roze-rpc",
];

const RPC_ROZE_CRATES: [&str; 16] = [
    "roze-config",
    "roze-context",
    "roze-db",
    "roze-mongo",
    "roze-error",
    "roze-grpc",
    "roze-jwt",
    "roze-log",
    "roze-cache",
    "roze-mq",
    "roze-nats",
    "roze-result",
    "roze-rpc",
    "roze-trace",
    "roze-transaction",
    "roze-validation",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProjectKind {
    Rest,
    Rpc,
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
    force: bool,
) -> anyhow::Result<()> {
    if out.exists() && !force {
        bail!(
            "{} already exists; pass --force to overwrite contract test files",
            out.display()
        );
    }

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
        out.join("README.md"),
        render_http_smoke_test_readme(&spec, api, base_url),
    )
    .with_context(|| format!("failed to write {}", out.join("README.md").display()))?;
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
        "- `src/server/**`, `src/client/**`, and proto/build files are generated for RPC projects."
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
axum = {{ version = "0.8", default-features = false, features = ["http1", "json", "tokio"] }}
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
        "use axum::{{routing::{{delete, get, patch, post, put}}, Json, Router}};"
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
            let path = mock_axum_path(&rest::full_route_path_for_route(spec, route));
            let method = mock_axum_method(&route.method);
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
        "    axum::serve(listener, app).await.expect(\"serve mock server\");"
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

fn render_http_smoke_tests(spec: &ApiSpec, base_url: &str) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    writeln!(&mut out, "use reqwest::StatusCode;").unwrap();
    writeln!(&mut out).unwrap();
    writeln!(
        &mut out,
        "fn base_url() -> String {{ std::env::var(\"ROZE_TEST_BASE_URL\").unwrap_or_else(|_| {:?}.to_string()) }}",
        base_url.trim_end_matches('/')
    )
    .unwrap();
    writeln!(&mut out).unwrap();

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
        writeln!(&mut out, "    let response = client.{method}(url)").unwrap();
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
        writeln!(&mut out, "        .send()").unwrap();
        writeln!(&mut out, "        .await?;").unwrap();
        writeln!(&mut out).unwrap();
        writeln!(
            &mut out,
            "    assert!(response.status().is_success(), \"expected success, got {{}}\", response.status());"
        )
        .unwrap();
        writeln!(
            &mut out,
            "    if response.status() != StatusCode::NO_CONTENT {{"
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
            "        let _: serde_json::Value = response.json().await?;"
        )
        .unwrap();
        writeln!(&mut out, "    }}").unwrap();
        writeln!(&mut out, "    Ok(())").unwrap();
        writeln!(&mut out, "}}").unwrap();
        writeln!(&mut out).unwrap();
    }

    if spec.rest_routes.is_empty() {
        writeln!(&mut out, "#[test]").unwrap();
        writeln!(&mut out, "fn no_rest_routes_declared() {{}}").unwrap();
    }

    out
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

fn mock_axum_method(method: &crate::parser::HttpMethod) -> &'static str {
    match method {
        crate::parser::HttpMethod::Get => "get",
        crate::parser::HttpMethod::Post => "post",
        crate::parser::HttpMethod::Put => "put",
        crate::parser::HttpMethod::Patch => "patch",
        crate::parser::HttpMethod::Delete => "delete",
    }
}

fn mock_axum_path(path: &str) -> String {
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
        .unwrap_or_else(|| format!("{}_{}", mock_axum_method(&route.method), route.path));
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
        HttpMethod::Post => "POST",
        HttpMethod::Put => "PUT",
        HttpMethod::Patch => "PATCH",
        HttpMethod::Delete => "DELETE",
    }
}

pub fn write_openapi_json(api: &Path, out: &Path) -> anyhow::Result<()> {
    let source = read_api_source(api)?;
    let spec = crate::parser::parse_api(&source)?;
    validate_project_kind(&spec, ProjectKind::Rest)?;
    let document = openapi_document(&spec);
    if let Some(parent) = out.parent().filter(|parent| !parent.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    fs::write(out, serde_json::to_string_pretty(&document)?)
        .with_context(|| format!("failed to write {}", out.display()))
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
        schemas.insert(ty.name.clone(), openapi_type_schema(ty));
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

    let mut parameters = Vec::new();
    let mut json_body_fields = Vec::new();
    let mut form_body_fields = Vec::new();
    if let Some(request_ty) = request_ty {
        for field in &request_ty.fields {
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
        || (request_ty.is_some_and(|ty| !ty.fields.is_empty())
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

fn openapi_type_schema(ty: &crate::parser::TypeDef) -> serde_json::Value {
    let mut properties = serde_json::Map::new();
    let mut required = Vec::new();
    for field in &ty.fields {
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
                crate::parser::HttpMethod::Get | crate::parser::HttpMethod::Delete
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

fn api_generate_handler(command: GeneratorCommand) -> anyhow::Result<()> {
    match command {
        GeneratorCommand::ApiGenerate { api, out, options } => {
            let source = read_api_source(&api)?;
            let spec = crate::parser::parse_api(&source)?;
            validate_project_kind(&spec, ProjectKind::Rest)?;
            if matches!(options.mode, GenerateMode::Force) {
                cleanup_rest_project(&out)?;
            }
            generate_rest_project(&spec, &out, options)
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
    ensure_output(out, options.mode)?;

    fs::create_dir_all(out.join("src"))?;
    fs::create_dir_all(out.join("src/config"))?;
    fs::create_dir_all(out.join("src/handler"))?;
    fs::create_dir_all(out.join("src/logic"))?;
    fs::create_dir_all(out.join("src/middleware"))?;
    fs::create_dir_all(out.join("src/openapi"))?;
    fs::create_dir_all(out.join("src/route"))?;
    fs::create_dir_all(out.join("src/svc"))?;
    fs::create_dir_all(out.join("src/types"))?;
    fs::create_dir_all(out.join(".cargo"))?;
    write_cargo_toml(spec, out, options, ProjectKind::Rest)?;
    fs::write(out.join(".cargo/config.toml"), cargo_config())?;
    fs::write(out.join("README.md"), readme(spec, ProjectKind::Rest))?;
    write_preserved(
        &out.join("config.yaml"),
        config_yaml(spec, ProjectKind::Rest),
        options.mode,
    )?;
    remove_path_if_exists(&out.join("src/config.rs"))?;
    remove_path_if_exists(&out.join("src/openapi.rs"))?;
    remove_path_if_exists(&out.join("src/types.rs"))?;
    fs::write(out.join("src/config/mod.rs"), config_rs())?;
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
        fs::write(dir.join("mod.rs"), content)?;
    }
    for (group, handler, content) in rest::render_logic_files(spec) {
        let dir = out.join("src/logic").join(&group);
        fs::create_dir_all(&dir)?;
        write_preserved(&dir.join(format!("{handler}.rs")), content, options.mode)?;
    }
    fs::write(
        out.join("src/types/mod.rs"),
        types::render_types(&spec.types),
    )?;
    fs::write(
        out.join("src/svc/mod.rs"),
        service_context_rs(ProjectKind::Rest),
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
    fs::create_dir_all(out.join("proto"))?;
    fs::create_dir_all(out.join(".cargo"))?;
    remove_path_if_exists(&out.join("src/client.rs"))?;
    remove_path_if_exists(&out.join("src/config.rs"))?;
    remove_path_if_exists(&out.join("src/pb.rs"))?;
    remove_path_if_exists(&out.join("src/rpc.rs"))?;
    remove_path_if_exists(&out.join("src/types.rs"))?;
    write_cargo_toml(spec, out, options, ProjectKind::Rpc)?;
    fs::write(out.join(".cargo/config.toml"), cargo_config())?;
    fs::write(out.join("README.md"), readme(spec, ProjectKind::Rpc))?;
    fs::write(out.join("build.rs"), build_rs())?;
    write_preserved(
        &out.join("config.yaml"),
        config_yaml(spec, ProjectKind::Rpc),
        options.mode,
    )?;
    fs::write(out.join("src/config/mod.rs"), config_rs())?;
    fs::write(out.join("src/pb/mod.rs"), render_pb(spec))?;
    fs::write(
        out.join("src/types/mod.rs"),
        types::render_types(&spec.types),
    )?;
    fs::write(
        out.join("src/svc/mod.rs"),
        service_context_rs(ProjectKind::Rpc),
    )?;
    fs::write(out.join("src/server/mod.rs"), rpc::render_rpc(spec))?;
    fs::write(out.join("src/client/mod.rs"), rpc::render_client(spec))?;
    fs::write(out.join("src/logic/mod.rs"), rpc::render_logic_mod(spec))?;
    for (method, content) in rpc::render_logic_files(spec) {
        write_preserved(
            &out.join("src/logic").join(format!("{method}.rs")),
            content,
            options.mode,
        )?;
    }
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
                spec,
                &package_name,
                options.dependency_source,
                local_crates_prefix.as_deref(),
                workspace_root.is_some(),
                kind,
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

    fs::write(&path, document.to_string())
        .with_context(|| format!("failed to write {}", path.display()))
}

fn has_entries(path: &Path) -> anyhow::Result<bool> {
    Ok(fs::read_dir(path)?.next().is_some())
}

pub(super) fn read_api_source(path: &Path) -> anyhow::Result<String> {
    let mut seen = HashSet::new();
    read_api_source_inner(path, &mut seen)
}

fn read_api_source_inner(path: &Path, seen: &mut HashSet<PathBuf>) -> anyhow::Result<String> {
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
    let base = absolute
        .parent()
        .ok_or_else(|| anyhow::anyhow!("{} has no parent directory", absolute.display()))?;
    let mut out = String::new();
    let mut lines = source.lines();
    while let Some(raw) = lines.next() {
        let line = raw.trim();
        if let Some(import) = parse_import_line(line) {
            let import_path = base.join(import);
            out.push_str(&read_api_source_inner(&import_path, seen)?);
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
                    out.push_str(&read_api_source_inner(&import_path, seen)?);
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
    _spec: &ApiSpec,
    package_name: &str,
    dependency_source: DependencySource,
    local_crates_prefix: Option<&str>,
    in_workspace: bool,
    kind: ProjectKind,
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
config.workspace = true
axum.workspace = true"#
            } else {
                r#"anyhow = "1"
config = { version = "0.15.24", default-features = false, features = ["json", "yaml", "toml"] }
axum = { version = "0.8", default-features = false, features = ["form", "http1", "http2", "json", "query", "tokio", "tracing"] }"#
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

    format!(
        r#"[package]
name = "{package_name}"
{package}

[dependencies]
{dependencies}
{roze_dependencies}
{remaining_dependencies}
{build_dependencies_section}"#,
        package_name = package_name,
        package = package,
        dependencies = dependencies,
        roze_dependencies = roze_dependencies,
        remaining_dependencies = remaining_dependencies,
        build_dependencies_section = build_dependencies_section,
    )
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
  routes: {{}}
# rpc_client:
#   endpoints: ["127.0.0.1:4000"]
#   # target: dns:///user.rpc
#   # app: app-name
#   # token: change-me
#   # non_block: false
#   timeout_ms: 2000
#   keepalive_time_secs: 20
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
registry:
  kind: memory
  endpoints: []
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
  routes: {{}}
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
        ProjectKind::Rest => rest_service_context_rs(),
        ProjectKind::Rpc => rpc_service_context_rs(),
    }
}

fn rest_service_context_rs() -> String {
    r#"#![allow(dead_code)]

use std::sync::Arc;

use crate::config::Config;

#[derive(Clone, Debug)]
pub struct ServiceContext {
    pub config: Config,
    pub cache: Option<roze_cache::RedisCache>,
    pub mq: Option<Arc<roze_nats::NatsJetStream>>,
    pub outbox: roze_transaction::InMemoryOutbox,
}

impl ServiceContext {
    pub async fn new(config: Config) -> anyhow::Result<Self> {
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
        Ok(Self {
            config,
            cache,
            mq,
            outbox: roze_transaction::InMemoryOutbox::new(),
        })
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

fn rpc_service_context_rs() -> String {
    r#"#![allow(dead_code)]

use std::sync::Arc;

use crate::config::Config;

#[derive(Clone, Debug)]
pub struct ServiceContext {
    pub config: Config,
    pub db_connections: Option<roze_db::DatabaseConnections>,
    pub mongo: Option<roze_mongo::MongoDatabase>,
    pub cache: Option<roze_cache::RedisCache>,
    pub mq: Option<Arc<roze_nats::NatsJetStream>>,
    pub outbox: roze_transaction::InMemoryOutbox,
}

impl ServiceContext {
    pub async fn new(config: Config) -> anyhow::Result<Self> {
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
        Ok(Self {
            config,
            db_connections,
            mongo,
            cache,
            mq,
            outbox: roze_transaction::InMemoryOutbox::new(),
        })
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
        let output = std::process::Command::new("cargo")
            .arg("check")
            .arg("--manifest-path")
            .arg(manifest)
            .arg("--quiet")
            .output()
            .expect("run cargo check");
        assert!(
            output.status.success(),
            "cargo check failed for {}\nstdout:\n{}\nstderr:\n{}",
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
    fn generated_cargo_uses_git_dependencies_for_roze_crates() {
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

        let cargo = cargo_toml(
            &spec,
            "user-api",
            DependencySource::Git,
            None,
            true,
            ProjectKind::Rest,
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

        let cargo = cargo_toml(
            &spec,
            "user-api",
            DependencySource::Path,
            Some("../../crates"),
            true,
            ProjectKind::Rest,
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
        let spec = parse_api(
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
        .expect("valid api");

        let cargo = cargo_toml(
            &spec,
            "user",
            DependencySource::Git,
            None,
            false,
            ProjectKind::Rpc,
        );

        assert!(cargo.contains(r#"edition = "2021""#));
        assert!(cargo.contains(r#"version = "0.1.0""#));
        assert!(cargo.contains(r#"anyhow = "1""#));
        assert!(cargo.contains(r#"tokio = { version = "1""#));
        assert!(cargo.contains(r#"roze-grpc = { git = "https://github.com/roze-team/roze.git" }"#));
        assert!(!cargo.contains(".workspace = true"));
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

        assert!(report.contains("M src/types/mod.rs"), "{report}");
        assert!(
            !report.contains("src/logic/users/get_users_id.rs"),
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
        let config = fs::read_to_string(out.join("config.yaml")).expect("read config");
        assert!(config.contains("postgres://postgres:postgres@127.0.0.1:5432/user"));
        assert!(!config.contains("sqlite://"));

        fs::remove_dir_all(root).expect("remove test output");
    }

    #[test]
    #[ignore = "compile-smoke: generates a REST project and runs cargo check"]
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
        fs::remove_dir_all(root).expect("remove compile workspace");
    }

    #[test]
    #[ignore = "compile-smoke: generates an RPC project and runs cargo check"]
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
        fs::remove_dir_all(root).expect("remove compile workspace");
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
        let cargo = fs::read_to_string(out.join("Cargo.toml")).expect("read cargo");
        assert!(cargo.contains(ROZE_GIT_URL));
        assert!(cargo.contains(r#"name = "custom-service""#));
        assert!(cargo.contains("custom.workspace = true"));

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
        assert!(tests.contains("let response = client.get(url)"));
        assert!(tests.contains(r#""/api/users/string""#));
        assert!(tests.contains(r#".header("x-trace-id", "string")"#));
        assert!(tests.contains(r#".query(&[("verbose", "true")])"#));
        assert!(tests.contains(r#""name": "string""#));
        assert!(write_http_smoke_test_project(&api, &out, "http://127.0.0.1:3000", false).is_err());
        write_http_smoke_test_project(&api, &out, "http://127.0.0.1:3000", true)
            .expect("force contract tests");

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
