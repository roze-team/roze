pub mod rest;
pub mod model;
pub mod rpc;
pub mod types;

use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{bail, Context};

use crate::parser::ApiSpec;

const ROZE_GIT_URL: &str = "https://github.com/roze-team/roze.git";
const REST_ROZE_CRATES: [&str; 13] = [
    "roze-config",
    "roze-error",
    "roze-http",
    "roze-log",
    "roze-metrics",
    "roze-middleware",
    "roze-jwt",
    "roze-cache",
    "roze-context",
    "roze-db",
    "roze-openapi",
    "roze-result",
    "roze-validation",
];

const RPC_ROZE_CRATES: [&str; 10] = [
    "roze-config",
    "roze-context",
    "roze-db",
    "roze-error",
    "roze-jwt",
    "roze-log",
    "roze-cache",
    "roze-result",
    "roze-rpc",
    "roze-trace",
];

const COMBINED_ROZE_CRATES: [&str; 14] = [
    "roze-config",
    "roze-error",
    "roze-http",
    "roze-log",
    "roze-metrics",
    "roze-middleware",
    "roze-jwt",
    "roze-cache",
    "roze-context",
    "roze-db",
    "roze-openapi",
    "roze-result",
    "roze-rpc",
    "roze-validation",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProjectKind {
    Rest,
    Rpc,
    Combined,
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
        }
    }
}

type GeneratorHandler = fn(GeneratorCommand) -> anyhow::Result<()>;

#[derive(Debug, Default)]
pub struct GeneratorRegistry {
    handlers: std::collections::BTreeMap<&'static str, GeneratorHandler>,
}

impl GeneratorRegistry {
    pub fn new() -> Self {
        let mut registry = Self::default();
        registry.register("api.generate", api_generate_handler);
        registry.register("api.new", api_new_handler);
        registry.register("rpc.generate", rpc_generate_handler);
        registry.register("rpc.new", rpc_new_handler);
        registry.register("model.generate", model_generate_handler);
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

fn api_generate_handler(command: GeneratorCommand) -> anyhow::Result<()> {
    match command {
        GeneratorCommand::ApiGenerate { api, out, options } => {
            let source = fs::read_to_string(&api)
                .with_context(|| format!("failed to read {}", api.display()))?;
            let spec = crate::parser::parse_api(&source)?;
            generate_project(&spec, &out, options)
                .with_context(|| format!("failed to generate project at {}", out.display()))
        }
        other => bail!("unexpected command variant for api.generate: {other:?}"),
    }
}

fn api_new_handler(command: GeneratorCommand) -> anyhow::Result<()> {
    match command {
        GeneratorCommand::ApiNew { name, out, options } => {
            create_api_project(&name, &out, options)
                .with_context(|| format!("failed to create api project at {}", out.display()))
        }
        other => bail!("unexpected command variant for api.new: {other:?}"),
    }
}

fn rpc_generate_handler(command: GeneratorCommand) -> anyhow::Result<()> {
    match command {
        GeneratorCommand::RpcGenerate { api, out, options } => {
            let source = fs::read_to_string(&api)
                .with_context(|| format!("failed to read {}", api.display()))?;
            let spec = crate::parser::parse_api(&source)?;
            generate_project(&spec, &out, options)
                .with_context(|| format!("failed to generate project at {}", out.display()))
        }
        other => bail!("unexpected command variant for rpc.generate: {other:?}"),
    }
}

fn rpc_new_handler(command: GeneratorCommand) -> anyhow::Result<()> {
    match command {
        GeneratorCommand::RpcNew { name, out, options } => {
            create_rpc_project(&name, &out, options)
                .with_context(|| format!("failed to create rpc project at {}", out.display()))
        }
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
        } => {
            let source = fs::read_to_string(&schema)
                .with_context(|| format!("failed to read {}", schema.display()))?;
            model::generate_model_project(&source, &out, options, format)
                .with_context(|| format!("failed to generate model scaffold at {}", out.display()))
        }
        other => bail!("unexpected command variant for model.generate: {other:?}"),
    }
}

pub fn generate_project(
    spec: &ApiSpec,
    out: &Path,
    options: GenerateOptions,
) -> anyhow::Result<()> {
    ensure_output(out, options.mode)?;
    let proto = render_proto(spec)?;

    fs::create_dir_all(out.join("src"))?;
    fs::create_dir_all(out.join("src/handler"))?;
    fs::create_dir_all(out.join("src/logic"))?;
    fs::create_dir_all(out.join("src/svc"))?;
    fs::create_dir_all(out.join("proto"))?;
    fs::create_dir_all(out.join(".cargo"))?;
    write_cargo_toml(spec, out, options, ProjectKind::Combined)?;
    fs::write(out.join(".cargo/config.toml"), cargo_config())?;
    fs::write(out.join("README.md"), readme(spec, ProjectKind::Combined))?;
    fs::write(out.join("build.rs"), build_rs())?;
    write_preserved(
        &out.join("config.yaml"),
        config_yaml(spec, ProjectKind::Combined),
        options.mode,
    )?;
    fs::write(out.join("src/config.rs"), config_rs())?;
    fs::write(out.join("src/pb.rs"), render_pb(spec))?;
    fs::write(out.join("src/types.rs"), types::render_types(&spec.types))?;
    fs::write(out.join("src/openapi.rs"), rest::render_openapi(spec))?;
    fs::write(out.join("src/handler/mod.rs"), rest::render_handlers(spec))?;
    write_preserved(
        &out.join("src/logic/mod.rs"),
        rest::render_logic(spec),
        options.mode,
    )?;
    fs::write(out.join("src/svc/mod.rs"), service_context_rs())?;
    fs::write(out.join("src/rpc.rs"), rpc::render_rpc(spec))?;
    fs::write(out.join("src/client.rs"), rpc::render_client(spec))?;
    fs::write(out.join("src/main.rs"), rest::render_combined_main(spec))?;
    ensure_model_module(&out)?;
    fs::write(out.join("proto/service.proto"), proto)?;

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

fn cleanup_rpc_project(out: &Path) -> anyhow::Result<()> {
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

fn generate_rest_project(
    spec: &ApiSpec,
    out: &Path,
    options: GenerateOptions,
) -> anyhow::Result<()> {
    ensure_output(out, options.mode)?;

    fs::create_dir_all(out.join("src"))?;
    fs::create_dir_all(out.join("src/handler"))?;
    fs::create_dir_all(out.join("src/logic"))?;
    fs::create_dir_all(out.join("src/svc"))?;
    fs::create_dir_all(out.join(".cargo"))?;
    write_cargo_toml(spec, out, options, ProjectKind::Rest)?;
    fs::write(out.join(".cargo/config.toml"), cargo_config())?;
    fs::write(out.join("README.md"), readme(spec, ProjectKind::Rest))?;
    write_preserved(
        &out.join("config.yaml"),
        config_yaml(spec, ProjectKind::Rest),
        options.mode,
    )?;
    fs::write(out.join("src/config.rs"), config_rs())?;
    fs::write(out.join("src/openapi.rs"), rest::render_openapi(spec))?;
    fs::write(out.join("src/handler/mod.rs"), rest::render_handlers(spec))?;
    write_preserved(
        &out.join("src/logic/mod.rs"),
        rest::render_logic(spec),
        options.mode,
    )?;
    fs::write(out.join("src/types.rs"), types::render_types(&spec.types))?;
    fs::write(out.join("src/svc/mod.rs"), service_context_rs())?;
    fs::write(out.join("src/main.rs"), rest::render_rest_main(spec))?;
    ensure_model_module(&out)?;
    Ok(())
}

fn generate_rpc_project(
    spec: &ApiSpec,
    out: &Path,
    options: GenerateOptions,
) -> anyhow::Result<()> {
    ensure_output(out, options.mode)?;

    fs::create_dir_all(out.join("src"))?;
    fs::create_dir_all(out.join("src/svc"))?;
    fs::create_dir_all(out.join("proto"))?;
    fs::create_dir_all(out.join(".cargo"))?;
    write_cargo_toml(spec, out, options, ProjectKind::Rpc)?;
    fs::write(out.join(".cargo/config.toml"), cargo_config())?;
    fs::write(out.join("README.md"), readme(spec, ProjectKind::Rpc))?;
    fs::write(out.join("build.rs"), build_rs())?;
    write_preserved(
        &out.join("config.yaml"),
        config_yaml(spec, ProjectKind::Rpc),
        options.mode,
    )?;
    fs::write(out.join("src/config.rs"), config_rs())?;
    fs::write(out.join("src/pb.rs"), render_pb(spec))?;
    fs::write(out.join("src/types.rs"), types::render_types(&spec.types))?;
    fs::write(out.join("src/svc/mod.rs"), service_context_rs())?;
    fs::write(out.join("src/rpc.rs"), rpc::render_rpc(spec))?;
    fs::write(out.join("src/client.rs"), rpc::render_client(spec))?;
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

    let content =
        fs::read_to_string(&main_path).with_context(|| format!("failed to read {}", main_path.display()))?;
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

    fs::write(&main_path, updated).with_context(|| format!("failed to write {}", main_path.display()))
}

fn write_cargo_toml(
    spec: &ApiSpec,
    out: &Path,
    options: GenerateOptions,
    kind: ProjectKind,
) -> anyhow::Result<()> {
    let path = out.join("Cargo.toml");
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

fn cargo_config() -> &'static str {
    r#"[net]
git-fetch-with-cli = true
"#
}

fn cargo_toml(
    spec: &ApiSpec,
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
        r#"edition.workspace = true
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
poem.workspace = true"#
            } else {
                r#"anyhow = "1"
config = { version = "0.14", default-features = false, features = ["json", "yaml", "toml"] }
poem = "3""#
            },
            if in_workspace {
                r#"serde.workspace = true
serde_json.workspace = true
sea-orm.workspace = true
validator.workspace = true
tokio.workspace = true
tracing.workspace = true"#
            } else {
                r#"serde = { version = "1", features = ["derive"] }
serde_json = "1"
sea-orm = { version = "1", default-features = false, features = ["macros", "runtime-tokio-rustls", "sqlx-mysql", "sqlx-postgres", "sqlx-sqlite"] }
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
config = { version = "0.14", default-features = false, features = ["json", "yaml", "toml"] }
prost = "0.12""#
            },
            if in_workspace {
                r#"serde.workspace = true
serde_json.workspace = true
sea-orm.workspace = true
tokio.workspace = true
tonic.workspace = true
tracing.workspace = true"#
            } else {
                r#"serde = { version = "1", features = ["derive"] }
serde_json = "1"
sea-orm = { version = "1", default-features = false, features = ["macros", "runtime-tokio-rustls", "sqlx-mysql", "sqlx-postgres", "sqlx-sqlite"] }
tokio = { version = "1", features = ["macros", "rt-multi-thread", "signal", "sync", "time"] }
tonic = "0.11"
tracing = "0.1""#
            },
            if in_workspace {
                r#"protoc-bin-vendored.workspace = true
tonic-build.workspace = true"#
            } else {
                r#"protoc-bin-vendored = "3"
tonic-build = "0.11""#
            },
        ),
        ProjectKind::Combined => (
            if in_workspace {
                r#"anyhow.workspace = true
config.workspace = true
poem.workspace = true
prost.workspace = true"#
            } else {
                r#"anyhow = "1"
config = { version = "0.14", default-features = false, features = ["json", "yaml", "toml"] }
poem = "3"
prost = "0.12""#
            },
            if in_workspace {
                r#"serde.workspace = true
serde_json.workspace = true
sea-orm.workspace = true
validator.workspace = true
tokio.workspace = true
tonic.workspace = true
tracing.workspace = true"#
            } else {
                r#"serde = { version = "1", features = ["derive"] }
serde_json = "1"
sea-orm = { version = "1", default-features = false, features = ["macros", "runtime-tokio-rustls", "sqlx-mysql", "sqlx-postgres", "sqlx-sqlite"] }
validator = { version = "0.20", features = ["derive"] }
tokio = { version = "1", features = ["macros", "rt-multi-thread", "signal", "sync", "time"] }
tonic = "0.11"
tracing = "0.1""#
            },
            if in_workspace {
                r#"protoc-bin-vendored.workspace = true
tonic-build.workspace = true"#
            } else {
                r#"protoc-bin-vendored = "3"
tonic-build = "0.11""#
            },
        ),
    };
    format!(
        r#"[package]
name = "{}-service"
{package}

[dependencies]
{dependencies}
{roze_dependencies}
{remaining_dependencies}

[build-dependencies]
{build_dependencies}
"#,
        spec.service,
        package = package,
        dependencies = dependencies,
        roze_dependencies = roze_dependencies,
        remaining_dependencies = remaining_dependencies,
        build_dependencies = build_dependencies,
    )
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
        ProjectKind::Combined => &COMBINED_ROZE_CRATES,
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

fn local_crates_prefix(out: &Path, workspace_root: &Path) -> anyhow::Result<String> {
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
        ProjectKind::Combined => format!(
            r#"# {name}

Generated by `rozectl`.

## Run

```bash
cargo run
```

## Endpoints

- REST: `GET /healthz`
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
    }
}

fn build_rs() -> String {
    r#"fn main() -> Result<(), Box<dyn std::error::Error>> {
    let protoc = protoc_bin_vendored::protoc_bin_path()?;
    std::env::set_var("PROTOC", protoc);

    tonic_build::configure()
        .build_server(true)
        .build_client(true)
        .compile(&["proto/service.proto"], &["proto"])?;

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
registry:
  kind: memory
  endpoints: []
  # ttl_seconds: 10
  # renew_interval_secs: 3
# database:
#   url: sqlite://data/{}.db?mode=rwc
# cache:
#   url: redis://127.0.0.1/
#   namespace: {}
# auth:
#   jwt_secret: change-me
#   jwt_issuer: {}
#   jwt_expiration_secs: 86400
"#,
            spec.service, spec.service, spec.service, spec.service
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
# database:
#   url: sqlite://data/{}.db?mode=rwc
# cache:
#   url: redis://127.0.0.1/
#   namespace: {}
# auth:
#   jwt_secret: change-me
#   jwt_issuer: {}
#   jwt_expiration_secs: 86400
"#,
            spec.service, spec.service, spec.service, spec.service
        ),
        ProjectKind::Combined => format!(
            r#"name: {}
rest:
  addr: 127.0.0.1:3000
registry:
  kind: memory
  endpoints: []
  # ttl_seconds: 10
  # renew_interval_secs: 3
# database:
#   url: sqlite://data/{}.db?mode=rwc
# cache:
#   url: redis://127.0.0.1/
#   namespace: {}
# auth:
#   jwt_secret: change-me
#   jwt_issuer: {}
#   jwt_expiration_secs: 86400
"#,
            spec.service, spec.service, spec.service, spec.service
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

fn find_workspace_root(out: &Path) -> anyhow::Result<Option<PathBuf>> {
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

fn service_context_rs() -> String {
    r#"#![allow(dead_code)]

use crate::config::Config;
use sea_orm::DatabaseConnection;

#[derive(Clone, Debug)]
pub struct ServiceContext {
    pub config: Config,
    pub db: Option<DatabaseConnection>,
    pub cache: Option<roze_cache::RedisCache>,
}

impl ServiceContext {
    pub async fn new(config: Config) -> anyhow::Result<Self> {
        let db = roze_db::connect_optional(config.database.as_ref()).await?;
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
        Ok(Self { config, db, cache })
    }

    pub fn jwt_config(&self) -> Option<roze_jwt::JwtConfig> {
        self.config.auth.as_ref().map(Into::into)
    }
}
"#
    .to_string()
}

fn render_pb(spec: &ApiSpec) -> String {
    let package = spec.service.replace('-', "_");
    format!(
        r#"pub mod {package} {{
    tonic::include_proto!("{package}");
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

fn render_proto(spec: &ApiSpec) -> anyhow::Result<String> {
    let package = spec.service.replace('-', "_");
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
        .replace('/', "_")
        .replace('-', "_")
}

fn proto_type<'a>(ty: &'a str, known_types: &HashSet<&str>) -> anyhow::Result<&'a str> {
    let proto = match ty {
        "String" | "string" => "string",
        "bool" => "bool",
        "i32" | "int32" => "int32",
        "i64" | "int" | "int64" => "int64",
        "u32" | "uint32" => "uint32",
        "u64" | "uint" | "uint64" => "uint64",
        "f32" | "float" => "float",
        "f64" | "double" => "double",
        known if known_types.contains(known) => known,
        other => anyhow::bail!("unsupported proto field type `{other}`"),
    };
    Ok(proto)
}

#[cfg(test)]
mod tests {
    use crate::parser::{parse_api, HttpMethod};

    use super::*;

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

        let cargo = cargo_toml(&spec, DependencySource::Git, None, true, ProjectKind::Rest);

        assert!(
            cargo.contains(r#"roze-config = { git = "https://github.com/roze-team/roze.git" }"#)
        );
        assert!(!cargo.contains(r#"roze-rpc = { git = "https://github.com/roze-team/roze.git" }"#));
        assert!(!cargo.contains(r#"path = "../../crates/roze-"#));
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
            DependencySource::Path,
            Some("../../crates"),
            true,
            ProjectKind::Rest,
        );

        assert!(cargo.contains(r#"roze-config = { path = "../../crates/roze-config" }"#));
        assert!(!cargo.contains(r#"roze-rpc = { path = "../../crates/roze-rpc" }"#));
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

        let cargo = cargo_toml(&spec, DependencySource::Git, None, false, ProjectKind::Combined);

        assert!(cargo.contains(r#"edition = "2021""#));
        assert!(cargo.contains(r#"version = "0.1.0""#));
        assert!(cargo.contains(r#"anyhow = "1""#));
        assert!(cargo.contains(r#"tokio = { version = "1""#));
        assert!(cargo.contains(r#"tonic-build = "0.11""#));
        assert!(!cargo.contains(".workspace = true"));
    }

    #[test]
    fn generated_cargo_config_uses_git_cli() {
        assert_eq!(cargo_config(), "[net]\ngit-fetch-with-cli = true\n");
    }

    #[test]
    fn update_preserves_business_files_and_refreshes_generated_files() {
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

        generate_project(
            &spec,
            &out,
            GenerateOptions::new(GenerateMode::Create, DependencySource::Path),
        )
        .expect("initial generation");
        fs::write(out.join("src/logic/mod.rs"), "// custom logic\n").expect("write custom logic");
        fs::write(out.join("config.yaml"), "name: custom\n").expect("write custom config");
        fs::write(out.join("src/handler/mod.rs"), "// stale handler\n")
            .expect("write stale handler");
        let cargo_path = out.join("Cargo.toml");
        let cargo = fs::read_to_string(&cargo_path)
            .expect("read initial cargo")
            .replace("name = \"user-api-service\"", "name = \"custom-service\"")
            .replace(
                "anyhow.workspace = true",
                "anyhow.workspace = true\ncustom.workspace = true",
            );
        fs::write(&cargo_path, cargo).expect("write custom cargo");

        generate_project(
            &spec,
            &out,
            GenerateOptions::new(GenerateMode::Update, DependencySource::Git),
        )
        .expect("update generation");

        assert_eq!(
            fs::read_to_string(out.join("src/logic/mod.rs")).expect("read logic"),
            "// custom logic\n"
        );
        assert_eq!(
            fs::read_to_string(out.join("config.yaml")).expect("read config"),
            "name: custom\n"
        );
        assert!(fs::read_to_string(out.join("src/handler/mod.rs"))
            .expect("read handler")
            .contains("pub fn router"));
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
            method: HttpMethod::Get,
            path: "/users/:id".to_string(),
            request: "GetUserReq".to_string(),
            response: "UserResp".to_string(),
        };
        let name = format!("get_{}", route_name_from_path(&route.path));

        assert_eq!(to_snake_case(&to_pascal_case(&name)), "get_users_id");
    }
}
