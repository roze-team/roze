//! `rozectl model` compatibility adapter.
//!
//! Keep this module intentionally thin: parsing, inspection, rendering, update
//! semantics, cleanup, and extensions are owned by the `roze-ent` crate.

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::Context;

use crate::generator::{
    find_workspace_root, inherited_roze_dependency, local_crates_prefix,
    validate_roze_dependency_sources, DependencySource, GenerateMode, GenerateOptions,
};

const MONGO_MODEL_MARKER: &str = "// @roze-model-backend: mongo";

pub use roze_ent::{
    model_graph, normalize_model_source_to_ent, parse_models, parse_models_with_format,
    validate_model_graph, ExtensionFileOwnership, InspectDatabaseKind, ModelAnnotation, ModelEdge,
    ModelExtensionFile, ModelField, ModelFieldValidation, ModelFormat, ModelGenerationGraph,
    ModelGeneratorExtension, ModelIndex, ModelOrm, ModelSpec, ModelThroughEdge,
    MODEL_GENERATOR_EXTENSION_API_VERSION,
};

struct RozectlHost {
    dependency: roze_ent::RozeDependency,
    logical_out: PathBuf,
    dependency_source: DependencySource,
    mongo: bool,
}

impl RozectlHost {
    fn current(
        logical_out: &Path,
        dependency_source: DependencySource,
        mongo: bool,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            dependency: roze_ent::RozeDependency::pinned(super::ROZE_GIT_URL, super::ROZE_GIT_REV)?,
            logical_out: logical_out.to_path_buf(),
            dependency_source,
            mongo,
        })
    }
}

impl roze_ent::HostAdapter for RozectlHost {
    fn roze_dependency(&self) -> Option<&roze_ent::RozeDependency> {
        Some(&self.dependency)
    }

    fn sync_project(&self, staged_project: &Path) -> anyhow::Result<()> {
        if self.mongo {
            ensure_mongo_project_wiring(staged_project, &self.logical_out, self.dependency_source)?;
        }
        super::sync_managed_service_if_present(staged_project)
    }

    fn format_generated_rust(&self, project: &Path, rust_files: &[PathBuf]) -> anyhow::Result<()> {
        super::format_generated_rust_files(project, rust_files)
    }
}

fn options(options: GenerateOptions) -> roze_ent::GenerateOptions {
    roze_ent::GenerateOptions::new(
        match options.mode {
            GenerateMode::Create => roze_ent::GenerateMode::Create,
            GenerateMode::Update => roze_ent::GenerateMode::Update,
            GenerateMode::Force => roze_ent::GenerateMode::Force,
        },
        match options.dependency_source {
            DependencySource::Git => roze_ent::DependencySource::Git,
            DependencySource::Path => roze_ent::DependencySource::Path,
        },
    )
}

fn mode(mode: GenerateMode) -> roze_ent::GenerateMode {
    match mode {
        GenerateMode::Create => roze_ent::GenerateMode::Create,
        GenerateMode::Update => roze_ent::GenerateMode::Update,
        GenerateMode::Force => roze_ent::GenerateMode::Force,
    }
}

pub fn resolve_model_orm(
    out: &Path,
    generate_mode: GenerateMode,
    requested: Option<ModelOrm>,
    switch_orm: bool,
) -> anyhow::Result<ModelOrm> {
    roze_ent::resolve_model_orm(out, mode(generate_mode), requested, switch_orm)
}

pub fn generate_model_project(
    source: &str,
    out: &Path,
    generate_options: GenerateOptions,
    format: ModelFormat,
    orm: ModelOrm,
) -> anyhow::Result<()> {
    let host = RozectlHost::current(
        out,
        generate_options.dependency_source,
        matches!(format, ModelFormat::Mongo),
    )?;
    roze_ent::generate_model_project_with_host(
        source,
        out,
        options(generate_options),
        format,
        orm,
        &host,
    )
}

pub fn generate_model_project_with_extensions(
    source: &str,
    out: &Path,
    generate_options: GenerateOptions,
    format: ModelFormat,
    orm: ModelOrm,
    extensions: &[&dyn ModelGeneratorExtension],
) -> anyhow::Result<()> {
    let host = RozectlHost::current(
        out,
        generate_options.dependency_source,
        matches!(format, ModelFormat::Mongo),
    )?;
    roze_ent::generate_model_project_with_extensions_and_host(
        source,
        out,
        options(generate_options),
        format,
        orm,
        extensions,
        &host,
    )
}

#[allow(clippy::too_many_arguments)]
pub async fn inspect_model_project(
    table: &str,
    schema_name: Option<&str>,
    db_url: &str,
    db_kind: InspectDatabaseKind,
    sample_size: u64,
    out: &Path,
    generate_options: GenerateOptions,
    orm: ModelOrm,
) -> anyhow::Result<()> {
    let host = RozectlHost::current(
        out,
        generate_options.dependency_source,
        matches!(db_kind, InspectDatabaseKind::Mongo),
    )?;
    roze_ent::inspect_model_project_with_host(
        table,
        schema_name,
        db_url,
        db_kind,
        sample_size,
        out,
        options(generate_options),
        orm,
        &host,
    )
    .await
}

pub(super) fn is_mongo_model_project(out: &Path) -> bool {
    let model_dir = out.join("src/model");
    let model_mod = model_dir.join("mod.rs");
    if fs::read_to_string(model_mod).is_ok_and(|source| source.contains(MONGO_MODEL_MARKER)) {
        return true;
    }
    fs::read_dir(model_dir).is_ok_and(|entries| {
        entries.filter_map(Result::ok).any(|entry| {
            entry
                .path()
                .extension()
                .is_some_and(|extension| extension == "rs")
                && fs::read_to_string(entry.path())
                    .is_ok_and(|source| source.contains("use roze_mongo::"))
        })
    })
}

pub(super) fn ensure_mongo_project_wiring(
    staged_out: &Path,
    logical_out: &Path,
    source: DependencySource,
) -> anyhow::Result<()> {
    update_mongo_model_context_hook(staged_out)?;
    update_mongo_service_context(staged_out)?;
    update_mongo_dependency(staged_out, logical_out, source)
}

fn update_mongo_model_context_hook(out: &Path) -> anyhow::Result<()> {
    let module_path = out.join("src/model/mod.rs");
    if !module_path.is_file() {
        return Ok(());
    }
    let mut module = fs::read_to_string(&module_path)
        .with_context(|| format!("failed to read {}", module_path.display()))?;
    if !module.contains(MONGO_MODEL_MARKER) {
        module.push_str(&format!("\n{MONGO_MODEL_MARKER}\n"));
    }
    if !module.contains("pub async fn configure_context(") {
        module.push_str(
            "\nuse crate::svc::ServiceContext;\n\npub async fn configure_context(\n    ctx: ServiceContext,\n) -> anyhow::Result<ServiceContext> {\n    Ok(ctx)\n}\n",
        );
    }
    fs::write(&module_path, module)
        .with_context(|| format!("failed to write {}", module_path.display()))
}

fn update_mongo_service_context(out: &Path) -> anyhow::Result<()> {
    let service_context_path = out.join("src/svc/mod.rs");
    if !service_context_path.is_file() {
        return Ok(());
    }
    let mut source = fs::read_to_string(&service_context_path)
        .with_context(|| format!("failed to read {}", service_context_path.display()))?;
    if source.contains("pub mongo: Option<roze_mongo::MongoDatabase>") {
        return Ok(());
    }

    source = replace_required(
        source,
        "    pub db_shards: Option<roze_db::ShardedDatabase>,\n    pub cache:",
        "    pub db_shards: Option<roze_db::ShardedDatabase>,\n    pub mongo: Option<roze_mongo::MongoDatabase>,\n    pub cache:",
        &service_context_path,
    )?;
    source = replace_required(
        source,
        "            .and_then(roze_db::DatabaseRuntime::sharded)\n            .cloned();\n        let cache =",
        "            .and_then(roze_db::DatabaseRuntime::sharded)\n            .cloned();\n        let mongo = roze_mongo::connect_optional(config.mongo.as_ref()).await?;\n        let cache =",
        &service_context_path,
    )?;
    source = replace_required(
        source,
        "        if let Some(cache) = cache.clone() {",
        "        if let Some(mongo) = mongo.clone() {\n            health.register_dependency(\"mongo\", move || {\n                let mongo = mongo.clone();\n                async move { mongo.health_check().await }\n            });\n        }\n        if let Some(cache) = cache.clone() {",
        &service_context_path,
    )?;
    source = replace_required(
        source,
        "            db_connections,\n            db_shards,\n            cache,",
        "            db_connections,\n            db_shards,\n            mongo,\n            cache,",
        &service_context_path,
    )?;
    fs::write(&service_context_path, source)
        .with_context(|| format!("failed to write {}", service_context_path.display()))
}

fn replace_required(source: String, from: &str, to: &str, path: &Path) -> anyhow::Result<String> {
    anyhow::ensure!(
        source.contains(from),
        "cannot add Mongo wiring because the generated service context anchor is missing in {}",
        path.display()
    );
    Ok(source.replacen(from, to, 1))
}

fn update_mongo_dependency(
    staged_out: &Path,
    logical_out: &Path,
    source: DependencySource,
) -> anyhow::Result<()> {
    let manifest_path = staged_out.join("Cargo.toml");
    if !manifest_path.is_file() {
        return Ok(());
    }

    let content = fs::read_to_string(&manifest_path)
        .with_context(|| format!("failed to read {}", manifest_path.display()))?;
    let mut document = content
        .parse::<toml_edit::DocumentMut>()
        .with_context(|| format!("failed to parse {}", manifest_path.display()))?;
    let dependencies = document
        .get_mut("dependencies")
        .and_then(toml_edit::Item::as_table_mut)
        .ok_or_else(|| {
            anyhow::anyhow!("{} has no [dependencies] table", manifest_path.display())
        })?;
    validate_roze_dependency_sources(dependencies)?;
    if dependencies.contains_key("roze-mongo") {
        return Ok(());
    }

    let inherited = inherited_roze_dependency(dependencies, "roze-mongo")?;
    let inherited = match inherited {
        Some(item)
            if !dependency_uses_workspace(&item)
                || workspace_declares_dependency(logical_out, "roze-mongo")? =>
        {
            Some(item)
        }
        _ => None,
    };
    let dependency = if let Some(inherited) = inherited {
        inherited
    } else {
        match source {
            DependencySource::Git => format!(
                r#"{{ git = "{}", rev = "{}" }}"#,
                super::ROZE_GIT_URL,
                super::ROZE_GIT_REV
            )
            .parse::<toml_edit::Item>()?,
            DependencySource::Path => {
                let workspace_root = find_workspace_root(logical_out)?.ok_or_else(|| {
                    anyhow::anyhow!(
                        "--roze-source path requires output inside a Cargo workspace containing Roze crates"
                    )
                })?;
                let prefix = local_crates_prefix(logical_out, &workspace_root)?;
                format!(r#"{{ path = "{prefix}/roze-mongo" }}"#).parse::<toml_edit::Item>()?
            }
        }
    };
    dependencies.insert("roze-mongo", dependency);
    fs::write(&manifest_path, document.to_string())
        .with_context(|| format!("failed to write {}", manifest_path.display()))
}

fn dependency_uses_workspace(item: &toml_edit::Item) -> bool {
    item.as_inline_table()
        .and_then(|dependency| dependency.get("workspace"))
        .and_then(toml_edit::Value::as_bool)
        == Some(true)
}

fn workspace_declares_dependency(logical_out: &Path, name: &str) -> anyhow::Result<bool> {
    let Some(workspace_root) = find_workspace_root(logical_out)? else {
        return Ok(false);
    };
    let manifest_path = workspace_root.join("Cargo.toml");
    let manifest = fs::read_to_string(&manifest_path)
        .with_context(|| format!("failed to read {}", manifest_path.display()))?;
    let document = manifest
        .parse::<toml_edit::DocumentMut>()
        .with_context(|| format!("failed to parse {}", manifest_path.display()))?;
    Ok(document
        .get("workspace")
        .and_then(toml_edit::Item::as_table)
        .and_then(|workspace| workspace.get("dependencies"))
        .and_then(toml_edit::Item::as_table)
        .is_some_and(|dependencies| dependencies.contains_key(name)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_model_output(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "{name}-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ))
    }

    #[test]
    fn mongo_generation_adds_pinned_roze_dependency_and_updates_stably() {
        let out = temp_model_output("rozectl-mongo-dependency");
        fs::create_dir_all(out.join("src")).expect("create source directory");
        fs::write(out.join("src/main.rs"), "fn main() {}\n").expect("write main");
        fs::write(
            out.join("Cargo.toml"),
            r#"[package]
name = "mongo-service"
version = "0.1.0"
edition = "2021"

[dependencies]
"#,
        )
        .expect("write manifest");
        let source = r#"
model User {
    table: users
    primary: id
    field id object_id
    field username String
}
"#;

        generate_model_project(
            source,
            &out,
            GenerateOptions::new(GenerateMode::Create, DependencySource::Git),
            ModelFormat::Mongo,
            ModelOrm::SeaOrm,
        )
        .expect("generate Mongo model");

        let manifest = fs::read_to_string(out.join("Cargo.toml")).expect("read manifest");
        let expected = format!(
            r#"roze-mongo = {{ git = "{}", rev = "{}" }}"#,
            super::super::ROZE_GIT_URL,
            super::super::ROZE_GIT_REV
        );
        assert!(manifest.contains(&expected));
        assert!(fs::read_to_string(out.join("src/model/user.rs"))
            .expect("read Mongo model")
            .contains("use roze_mongo::"));
        let model_mod = fs::read_to_string(out.join("src/model/mod.rs")).expect("read model mod");
        assert!(model_mod.contains(MONGO_MODEL_MARKER));
        assert!(model_mod.contains("pub async fn configure_context("));

        generate_model_project(
            source,
            &out,
            GenerateOptions::new(GenerateMode::Update, DependencySource::Git),
            ModelFormat::Mongo,
            ModelOrm::SeaOrm,
        )
        .expect("update Mongo model");
        assert_eq!(
            fs::read_to_string(out.join("Cargo.toml")).expect("read updated manifest"),
            manifest
        );

        fs::remove_dir_all(out).expect("remove temporary model output");
    }

    #[test]
    fn mongo_generation_inherits_local_roze_dependency_source() {
        let root = temp_model_output("rozectl-mongo-path-dependency");
        let out = root.join("apps/mongo-service");
        fs::create_dir_all(out.join("src")).expect("create source directory");
        fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nmembers = []\nresolver = \"2\"\n\n[workspace.dependencies]\nroze-http = { path = \"crates/roze-http\" }\n",
        )
        .expect("write workspace manifest");
        fs::write(out.join("src/main.rs"), "fn main() {}\n").expect("write main");
        fs::create_dir_all(out.join("src/svc")).expect("create service context directory");
        fs::write(
            out.join("src/svc/mod.rs"),
            r#"pub struct ServiceContext {
    pub db_connections: Option<roze_db::DatabaseConnections>,
    pub db_shards: Option<roze_db::ShardedDatabase>,
    pub cache: Option<roze_cache::RedisCache>,
}

async fn build(config: Config, health: Health) -> anyhow::Result<ServiceContext> {
    let database_runtime = roze_db::connect_runtime_optional(config.database.as_ref()).await?;
    let db_connections = database_runtime.as_ref().and_then(roze_db::DatabaseRuntime::direct).cloned();
    let db_shards = database_runtime
            .as_ref()
            .and_then(roze_db::DatabaseRuntime::sharded)
            .cloned();
        let cache = None;
        if let Some(cache) = cache.clone() {
            let _ = cache;
        }
        Ok(ServiceContext {
            db_connections,
            db_shards,
            cache,
        })
}
"#,
        )
        .expect("write service context");
        fs::write(
            out.join("Cargo.toml"),
            r#"[package]
name = "mongo-service"
version = "0.1.0"
edition = "2021"

[dependencies]
roze-http = { workspace = true }
"#,
        )
        .expect("write manifest");

        generate_model_project(
            r#"
model User {
    table: users
    primary: id
    field id object_id
}
"#,
            &out,
            GenerateOptions::new(GenerateMode::Create, DependencySource::Path),
            ModelFormat::Mongo,
            ModelOrm::SeaOrm,
        )
        .expect("generate Mongo model with local dependencies");

        let manifest = fs::read_to_string(out.join("Cargo.toml")).expect("read manifest");
        assert!(manifest.contains(r#"roze-mongo = { path = "../../crates/roze-mongo" }"#));
        let service_context =
            fs::read_to_string(out.join("src/svc/mod.rs")).expect("read service context");
        assert!(service_context.contains("pub mongo: Option<roze_mongo::MongoDatabase>"));
        assert!(
            service_context.contains("roze_mongo::connect_optional(config.mongo.as_ref()).await?")
        );
        assert!(service_context.contains("health.register_dependency(\"mongo\""));
        assert!(service_context.contains("            mongo,"));

        fs::remove_dir_all(root).expect("remove temporary workspace");
    }
}
