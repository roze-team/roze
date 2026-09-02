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
    canonical_roze_dependency, find_workspace_root, inherited_roze_dependency, local_crates_prefix,
    migrate_legacy_generated_validator, normalize_roze_dependency_document,
    validate_roze_dependency_sources, DependencySource, GenerateMode, GenerateOptions,
};

pub use roze_ent::{
    model_graph, normalize_model_source_to_ent, parse_models, parse_models_with_format,
    validate_model_graph, ExtensionFileOwnership, InspectDatabaseKind, ModelAnnotation,
    ModelBackend, ModelEdge, ModelExtensionFile, ModelField, ModelFieldValidation, ModelFormat,
    ModelGenerationGraph, ModelGeneratorExtension, ModelIndex, ModelOrm, ModelProjectRequirements,
    ModelSpec, ModelThroughEdge, RuntimeCapability, MODEL_GENERATOR_EXTENSION_API_VERSION,
    MODEL_PROJECT_REQUIREMENTS_API_VERSION,
};

struct RozectlHost {
    dependency: roze_ent::RozeDependency,
    logical_out: PathBuf,
    dependency_source: DependencySource,
}

impl RozectlHost {
    fn current(logical_out: &Path, dependency_source: DependencySource) -> anyhow::Result<Self> {
        Ok(Self {
            dependency: roze_ent::RozeDependency::pinned(super::ROZE_GIT_URL, super::ROZE_GIT_REV)?,
            logical_out: logical_out.to_path_buf(),
            dependency_source,
        })
    }
}

impl roze_ent::HostAdapter for RozectlHost {
    fn roze_dependency(&self) -> Option<&roze_ent::RozeDependency> {
        Some(&self.dependency)
    }

    fn sync_project(&self, staged_project: &Path) -> anyhow::Result<()> {
        super::sync_managed_service_if_present(staged_project)
    }

    fn sync_model_project(
        &self,
        staged_project: &Path,
        requirements: &ModelProjectRequirements,
    ) -> anyhow::Result<()> {
        if requirements.backend == ModelBackend::MongoDb {
            ensure_mongo_project_wiring(
                staged_project,
                &self.logical_out,
                self.dependency_source,
                requirements,
            )?;
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
    let original_manifest = normalize_legacy_host_manifest(out, generate_options.mode)?;
    let host = RozectlHost::current(out, generate_options.dependency_source)?;
    let result = roze_ent::generate_model_project_with_host(
        source,
        out,
        options(generate_options),
        format,
        orm,
        &host,
    );
    rollback_manifest_on_error(out, original_manifest, result)
}

pub fn generate_model_project_with_extensions(
    source: &str,
    out: &Path,
    generate_options: GenerateOptions,
    format: ModelFormat,
    orm: ModelOrm,
    extensions: &[&dyn ModelGeneratorExtension],
) -> anyhow::Result<()> {
    let original_manifest = normalize_legacy_host_manifest(out, generate_options.mode)?;
    let host = RozectlHost::current(out, generate_options.dependency_source)?;
    let result = roze_ent::generate_model_project_with_extensions_and_host(
        source,
        out,
        options(generate_options),
        format,
        orm,
        extensions,
        &host,
    );
    rollback_manifest_on_error(out, original_manifest, result)
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
    let original_manifest = normalize_legacy_host_manifest(out, generate_options.mode)?;
    let host = RozectlHost::current(out, generate_options.dependency_source)?;
    let result = roze_ent::inspect_model_project_with_host(
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
    .await;
    rollback_manifest_on_error(out, original_manifest, result)
}

fn normalize_legacy_host_manifest(
    out: &Path,
    mode: GenerateMode,
) -> anyhow::Result<Option<String>> {
    if mode != GenerateMode::Update {
        return Ok(None);
    }
    let manifest_path = out.join("Cargo.toml");
    if !manifest_path.is_file() {
        return Ok(None);
    }
    let original = fs::read_to_string(&manifest_path)
        .with_context(|| format!("failed to read {}", manifest_path.display()))?;
    let mut document = original
        .parse::<toml_edit::DocumentMut>()
        .with_context(|| format!("failed to parse {}", manifest_path.display()))?;
    let fallback = format!(
        r#"{{ git = "{}", rev = "{}" }}"#,
        super::ROZE_GIT_URL,
        super::ROZE_GIT_REV
    )
    .parse::<toml_edit::Item>()
    .context("failed to construct pinned Roze dependency")?;
    let canonical = canonical_roze_dependency(&document, Some(&fallback))?;
    normalize_roze_dependency_document(&mut document, canonical.as_ref());
    let validator = r#"{ version = "0.21", features = ["derive"] }"#
        .parse::<toml_edit::Item>()
        .context("failed to construct current validator dependency")?;
    migrate_legacy_generated_validator(&mut document, &validator);
    let updated = document.to_string();
    if updated == original {
        return Ok(None);
    }
    fs::write(&manifest_path, updated)
        .with_context(|| format!("failed to write {}", manifest_path.display()))?;
    Ok(Some(original))
}

fn rollback_manifest_on_error<T>(
    out: &Path,
    original_manifest: Option<String>,
    result: anyhow::Result<T>,
) -> anyhow::Result<T> {
    match result {
        Ok(value) => Ok(value),
        Err(error) => {
            if let Some(original_manifest) = original_manifest {
                let manifest_path = out.join("Cargo.toml");
                fs::write(&manifest_path, original_manifest).with_context(|| {
                    format!(
                        "model generation failed and the legacy manifest migration could not be rolled back in {}: {error:#}",
                        manifest_path.display()
                    )
                })?;
            }
            Err(error)
        }
    }
}

pub(super) fn is_mongo_model_project(out: &Path) -> bool {
    fs::read_to_string(out.join("Cargo.toml"))
        .ok()
        .and_then(|manifest| manifest.parse::<toml_edit::DocumentMut>().ok())
        .and_then(|document| {
            document
                .get("package")?
                .as_table()?
                .get("metadata")?
                .as_table()?
                .get("roze")?
                .as_table()?
                .get("model")?
                .as_table()?
                .get("backend")?
                .as_str()
                .map(str::to_owned)
        })
        .is_some_and(|backend| backend == "mongo")
}

pub(super) fn ensure_mongo_project_wiring(
    staged_out: &Path,
    logical_out: &Path,
    source: DependencySource,
    requirements: &ModelProjectRequirements,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        requirements.backend == ModelBackend::MongoDb,
        "Mongo project wiring requires the MongoDb backend"
    );
    anyhow::ensure!(
        requirements.dependency("roze-mongo").is_some(),
        "Mongo project requirements must declare the direct `roze-mongo` dependency"
    );
    for capability in [
        RuntimeCapability::MongoConnection,
        RuntimeCapability::HealthRegistration,
        RuntimeCapability::ModelContextHook,
    ] {
        anyhow::ensure!(
            requirements.requires(capability),
            "Mongo project requirements are missing runtime capability {capability:?}"
        );
    }
    if requirements.requires(RuntimeCapability::ModelContextHook) {
        update_mongo_model_context_hook(staged_out)?;
    }
    if requirements.requires(RuntimeCapability::MongoConnection) {
        update_mongo_service_context(staged_out)?;
    }
    update_mongo_dependencies(staged_out, logical_out, source, requirements)
}

pub(super) fn restore_mongo_service_wiring(out: &Path) -> anyhow::Result<()> {
    update_mongo_model_context_hook(out)?;
    update_mongo_service_context(out)
}

fn update_mongo_model_context_hook(out: &Path) -> anyhow::Result<()> {
    let module_path = out.join("src/model/mod.rs");
    if !module_path.is_file() {
        return Ok(());
    }
    let mut module = fs::read_to_string(&module_path)
        .with_context(|| format!("failed to read {}", module_path.display()))?;
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

fn update_mongo_dependencies(
    staged_out: &Path,
    logical_out: &Path,
    source: DependencySource,
    requirements: &ModelProjectRequirements,
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
    for requirement in &requirements.cargo_dependencies {
        if let Some(dependency) = dependencies.get_mut(&requirement.name) {
            merge_required_features(
                dependency,
                &requirement.features,
                &requirement.name,
                &manifest_path,
            )?;
            continue;
        }

        let mut dependency = dependency_for_requirement(
            dependencies,
            logical_out,
            source,
            &requirement.name,
            requirement.version_req.as_deref(),
        )?;
        merge_required_features(
            &mut dependency,
            &requirement.features,
            &requirement.name,
            &manifest_path,
        )?;
        dependencies.insert(&requirement.name, dependency);
    }
    let package = document
        .get_mut("package")
        .and_then(toml_edit::Item::as_table_mut)
        .ok_or_else(|| anyhow::anyhow!("{} has no [package] table", manifest_path.display()))?;
    let metadata = ensure_child_table(package, "metadata", &manifest_path)?;
    let roze = ensure_child_table(metadata, "roze", &manifest_path)?;
    let model = ensure_child_table(roze, "model", &manifest_path)?;
    model.insert(
        "backend",
        toml_edit::value(match requirements.backend {
            ModelBackend::MongoDb => "mongo",
            ModelBackend::SeaOrm => "sea-orm",
            ModelBackend::Toasty => "toasty",
        }),
    );
    fs::write(&manifest_path, document.to_string())
        .with_context(|| format!("failed to write {}", manifest_path.display()))
}

fn dependency_for_requirement(
    dependencies: &toml_edit::Table,
    logical_out: &Path,
    source: DependencySource,
    name: &str,
    version_req: Option<&str>,
) -> anyhow::Result<toml_edit::Item> {
    if name.starts_with("roze-") {
        let inherited = inherited_roze_dependency(dependencies, name)?;
        if let Some(item) = inherited {
            if !dependency_uses_workspace(&item)
                || workspace_declares_dependency(logical_out, name)?
            {
                return Ok(item);
            }
        }
        return match source {
            DependencySource::Git => format!(
                r#"{{ git = "{}", rev = "{}" }}"#,
                super::ROZE_GIT_URL,
                super::ROZE_GIT_REV
            )
            .parse::<toml_edit::Item>()
            .map_err(Into::into),
            DependencySource::Path => {
                let workspace_root = find_workspace_root(logical_out)?.ok_or_else(|| {
                    anyhow::anyhow!(
                        "--roze-source path requires output inside a Cargo workspace containing Roze crates"
                    )
                })?;
                let prefix = local_crates_prefix(logical_out, &workspace_root)?;
                format!(r#"{{ path = "{prefix}/{name}" }}"#)
                    .parse::<toml_edit::Item>()
                    .map_err(Into::into)
            }
        };
    }

    if workspace_declares_dependency(logical_out, name)? {
        return r#"{ workspace = true }"#.parse::<toml_edit::Item>().map_err(Into::into);
    }

    let version_req = version_req.ok_or_else(|| {
        anyhow::anyhow!(
            "roze-ent did not declare a compatible Cargo version for crates.io dependency `{name}`"
        )
    })?;
    Ok(toml_edit::value(version_req))
}

fn merge_required_features(
    dependency: &mut toml_edit::Item,
    required: &[String],
    name: &str,
    manifest_path: &Path,
) -> anyhow::Result<()> {
    if required.is_empty() {
        return Ok(());
    }

    if let Some(version) = dependency.as_str().map(str::to_owned) {
        let mut inline = toml_edit::InlineTable::new();
        inline.insert("version", toml_edit::Value::from(version));
        *dependency = toml_edit::Item::Value(toml_edit::Value::InlineTable(inline));
    }

    if let Some(inline) = dependency.as_inline_table_mut() {
        let mut features = inline
            .get("features")
            .and_then(toml_edit::Value::as_array)
            .map(array_strings)
            .transpose()?
            .unwrap_or_default();
        features.extend(required.iter().cloned());
        features.sort();
        features.dedup();
        inline.insert("features", toml_edit::Value::Array(string_array(&features)));
        return Ok(());
    }

    if let Some(table) = dependency.as_table_mut() {
        let mut features = table
            .get("features")
            .and_then(toml_edit::Item::as_array)
            .map(array_strings)
            .transpose()?
            .unwrap_or_default();
        features.extend(required.iter().cloned());
        features.sort();
        features.dedup();
        table.insert("features", toml_edit::value(string_array(&features)));
        return Ok(());
    }

    anyhow::bail!(
        "cannot merge required features for dependency `{name}` in {}",
        manifest_path.display()
    )
}

fn array_strings(array: &toml_edit::Array) -> anyhow::Result<Vec<String>> {
    array
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| anyhow::anyhow!("Cargo dependency features must be strings"))
        })
        .collect()
}

fn string_array(values: &[String]) -> toml_edit::Array {
    let mut array = toml_edit::Array::new();
    for value in values {
        array.push(value.as_str());
    }
    array
}

fn ensure_child_table<'a>(
    parent: &'a mut toml_edit::Table,
    name: &str,
    manifest_path: &Path,
) -> anyhow::Result<&'a mut toml_edit::Table> {
    if !parent.contains_key(name) {
        parent.insert(name, toml_edit::Item::Table(toml_edit::Table::new()));
    }
    parent
        .get_mut(name)
        .and_then(toml_edit::Item::as_table_mut)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Cargo metadata `{name}` must be a table in {}",
                manifest_path.display()
            )
        })
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
    fn crates_io_dependency_uses_roze_ent_version_requirement() {
        let dependencies = toml_edit::Table::new();
        let out = temp_model_output("rozectl-model-dependency-version");

        let dependency = dependency_for_requirement(
            &dependencies,
            &out,
            DependencySource::Git,
            "future-model-runtime",
            Some("9.7"),
        )
        .expect("use roze-ent version requirement");

        assert_eq!(dependency.as_str(), Some("9.7"));
    }

    #[test]
    fn model_update_normalizes_floating_host_dependencies_and_legacy_validator() {
        let out = temp_model_output("rozectl-model-floating-host");
        fs::create_dir_all(out.join("src")).expect("create source directory");
        fs::write(out.join("src/main.rs"), "fn main() {}\n").expect("write main");
        fs::write(
            out.join("Cargo.toml"),
            r#"[package]
name = "legacy-model-host"
version = "0.1.0"
edition = "2021"

[dependencies]
roze-config = { git = "https://github.com/roze-team/roze.git" }
validator = { version = "0.20", features = ["derive"] }

[build-dependencies]
roze-grpc = { git = "https://github.com/roze-team/roze.git" }

[target.'cfg(windows)'.dependencies]
roze-context = { git = "https://github.com/roze-team/roze.git" }
"#,
        )
        .expect("write manifest");

        generate_model_project(
            r#"
entity User {
    table "users"
    field id: u64 {
        primary
    }
    field name: string {
    }
}
"#,
            &out,
            GenerateOptions::new(GenerateMode::Update, DependencySource::Git),
            ModelFormat::Ent,
            ModelOrm::Toasty,
        )
        .expect("update model project");

        let manifest = fs::read_to_string(out.join("Cargo.toml"))
            .expect("read manifest")
            .parse::<toml_edit::DocumentMut>()
            .expect("parse manifest");
        super::super::visit_dependency_tables(&manifest, |dependencies| {
            for (name, dependency) in dependencies {
                if name.starts_with("roze-") {
                    assert_eq!(
                        dependency
                            .as_inline_table()
                            .and_then(|dependency| dependency.get("rev"))
                            .and_then(toml_edit::Value::as_str),
                        Some(super::super::ROZE_GIT_REV),
                        "{name} was not pinned"
                    );
                }
            }
        });
        assert_eq!(
            manifest["dependencies"]["validator"]
                .as_inline_table()
                .and_then(|dependency| dependency.get("version"))
                .and_then(toml_edit::Value::as_str),
            Some("0.21")
        );
        fs::remove_dir_all(out).expect("remove temporary model output");
    }

    #[test]
    fn failed_model_update_rolls_back_legacy_manifest_normalization() {
        let out = temp_model_output("rozectl-model-floating-host-rollback");
        fs::create_dir_all(out.join("src")).expect("create source directory");
        fs::write(out.join("src/main.rs"), "fn main() {}\n").expect("write main");
        let original = r#"[package]
name = "legacy-model-host"
version = "0.1.0"
edition = "2021"

[dependencies]
roze-config = { git = "https://github.com/roze-team/roze.git" }
validator = { version = "0.20", features = ["derive"] }
"#;
        fs::write(out.join("Cargo.toml"), original).expect("write manifest");

        generate_model_project(
            "this is not a model schema",
            &out,
            GenerateOptions::new(GenerateMode::Update, DependencySource::Git),
            ModelFormat::Ent,
            ModelOrm::Toasty,
        )
        .expect_err("invalid model update must fail");

        assert_eq!(
            fs::read_to_string(out.join("Cargo.toml")).expect("read rolled back manifest"),
            original
        );
        fs::remove_dir_all(out).expect("remove temporary model output");
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
        assert!(manifest.contains(r#"anyhow = "1""#));
        assert!(manifest.contains(r#"serde = { version = "1", features = ["derive"] }"#));
        assert!(manifest.contains("[package.metadata.roze.model]"));
        assert!(manifest.contains(r#"backend = "mongo""#));
        assert_eq!(MODEL_PROJECT_REQUIREMENTS_API_VERSION, 2);
        assert!(fs::read_to_string(out.join("src/model/user.rs"))
            .expect("read Mongo model")
            .contains("use roze_mongo::"));
        let model_mod = fs::read_to_string(out.join("src/model/mod.rs")).expect("read model mod");
        assert!(model_mod.contains("pub async fn configure_context("));
        assert!(is_mongo_model_project(&out));

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
        assert!(manifest.contains(r#"anyhow = "1""#));
        assert!(manifest.contains(r#"serde = { version = "1", features = ["derive"] }"#));
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
