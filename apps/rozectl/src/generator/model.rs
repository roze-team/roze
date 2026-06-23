use std::{
    collections::{HashMap, HashSet},
    fs,
    path::Path,
};

use anyhow::{bail, Context};
use roze_sqlx::{SqlxConfig, SqlxDatabaseKind, SqlxPool};
use sqlx::{AssertSqlSafe, Row};

use super::{to_pascal_case, to_snake_case, GenerateMode, GenerateOptions};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelFormat {
    Auto,
    Dsl,
    Sql,
    Mongo,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelOrm {
    SeaOrm,
    Toasty,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelSpec {
    pub name: String,
    pub schema_name: Option<String>,
    pub table: String,
    pub primary: String,
    pub soft_delete: Option<String>,
    pub tenant: Option<String>,
    pub cache: bool,
    pub cache_ttl_secs: Option<u64>,
    pub negative_cache_ttl_secs: Option<u64>,
    pub cache_keys: Vec<String>,
    pub cache_prefix: Option<String>,
    pub fields: Vec<ModelField>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelField {
    pub name: String,
    pub ty: String,
    pub auto_increment: bool,
    pub default_value: Option<String>,
    pub comment: Option<String>,
}

pub fn generate_model_project(
    source: &str,
    out: &Path,
    options: GenerateOptions,
    format: ModelFormat,
    orm: ModelOrm,
) -> anyhow::Result<()> {
    let models = parse_models_with_format(source, format)?;
    match format {
        ModelFormat::Mongo => write_mongo_model_project(&models, out, options),
        _ => write_model_project(&models, out, options, orm),
    }
}

pub async fn inspect_model_project(
    table: &str,
    schema_name: Option<&str>,
    db_url: &str,
    db_kind: SqlxDatabaseKind,
    out: &Path,
    options: GenerateOptions,
    orm: ModelOrm,
) -> anyhow::Result<()> {
    let pool = roze_sqlx::connect(&SqlxConfig {
        kind: db_kind,
        url: db_url.to_string(),
        max_connections: 10,
    })
    .await?;

    let model = match pool {
        SqlxPool::Sqlite(pool) => inspect_sqlite_table(&pool, schema_name, table).await?,
        SqlxPool::Postgres(pool) => inspect_postgres_table(&pool, schema_name, table).await?,
        SqlxPool::MySql(pool) => inspect_mysql_table(&pool, schema_name, table).await?,
    };

    write_model_project(&[model], out, options, orm)
}

fn write_model_project(
    models: &[ModelSpec],
    out: &Path,
    options: GenerateOptions,
    orm: ModelOrm,
) -> anyhow::Result<()> {
    ensure_model_output(out, options.mode)?;
    let model_dir = out.join("src/model");
    fs::create_dir_all(&model_dir)?;

    // Model files are schema-owned generated artifacts. Refresh them on --update
    // so database/schema changes are reflected deterministically.
    fs::write(model_dir.join("mod.rs"), render_model_mod(models, orm))?;
    for model in models {
        let module_path = model_dir.join(format!("{}.rs", to_snake_case(&model.name)));
        let rendered = match orm {
            ModelOrm::SeaOrm => render_model_module(model),
            ModelOrm::Toasty => render_toasty_model_module(model),
        };
        fs::write(module_path, rendered)?;
    }

    update_model_dependencies(out, orm)?;
    update_main_rs(out)?;
    Ok(())
}

fn write_mongo_model_project(
    models: &[ModelSpec],
    out: &Path,
    options: GenerateOptions,
) -> anyhow::Result<()> {
    ensure_model_output(out, options.mode)?;
    let model_dir = out.join("src/model");
    fs::create_dir_all(&model_dir)?;

    // Model files are schema-owned generated artifacts. Refresh them on --update
    // so database/schema changes are reflected deterministically.
    fs::write(model_dir.join("mod.rs"), render_mongo_model_mod(models))?;
    for model in models {
        let module_path = model_dir.join(format!("{}.rs", to_snake_case(&model.name)));
        fs::write(module_path, render_mongo_model_module(model))?;
    }

    update_main_rs(out)?;
    Ok(())
}

fn ensure_model_output(out: &Path, mode: GenerateMode) -> anyhow::Result<()> {
    let model_dir = out.join("src/model");
    if mode == GenerateMode::Create && model_dir.exists() && has_entries(&model_dir)? {
        bail!(
            "{} already contains generated model files; pass --update to refresh them",
            model_dir.display()
        );
    }
    Ok(())
}

fn update_main_rs(out: &Path) -> anyhow::Result<()> {
    let main_path = out.join("src/main.rs");
    if !main_path.is_file() {
        return Ok(());
    }

    let content = fs::read_to_string(&main_path)
        .with_context(|| format!("failed to read {}", main_path.display()))?;
    if content.contains("mod model;") {
        return Ok(());
    }

    let updated = insert_after_module(&content, "mod types;\n", "mod model;\n")
        .unwrap_or_else(|| format!("mod model;\n{content}"));
    fs::write(&main_path, updated)
        .with_context(|| format!("failed to write {}", main_path.display()))
}

fn update_model_dependencies(out: &Path, orm: ModelOrm) -> anyhow::Result<()> {
    let manifest_path = out.join("Cargo.toml");
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

    let dependency_name = match orm {
        ModelOrm::SeaOrm => "sea-orm",
        ModelOrm::Toasty => "toasty",
    };
    if dependencies.contains_key(dependency_name) {
        return Ok(());
    }

    let uses_workspace = content.contains("edition.workspace = true")
        || content.contains("toasty.workspace = true")
        || content.contains("sea-orm.workspace = true");

    let item = if uses_workspace {
        let mut table = toml_edit::InlineTable::new();
        table.insert("workspace", true.into());
        toml_edit::Item::Value(toml_edit::Value::InlineTable(table))
    } else {
        match orm {
            ModelOrm::SeaOrm => {
                r#"{ version = "1", default-features = false, features = ["macros", "runtime-tokio-rustls", "sqlx-mysql", "sqlx-postgres", "sqlx-sqlite"] }"#
                    .parse::<toml_edit::Item>()
                    .expect("valid toml dependency value")
            }
            ModelOrm::Toasty => {
                r#"{ version = "0.7", default-features = false, features = ["postgresql", "mysql", "serde"] }"#
                    .parse::<toml_edit::Item>()
                    .expect("valid toml dependency value")
            }
        }
    };
    dependencies.insert(dependency_name, item);
    fs::write(&manifest_path, document.to_string())
        .with_context(|| format!("failed to write {}", manifest_path.display()))
}

fn insert_after_module(content: &str, needle: &str, insert: &str) -> Option<String> {
    let idx = content.find(needle)?;
    let mut updated = String::with_capacity(content.len() + insert.len());
    updated.push_str(&content[..idx + needle.len()]);
    updated.push_str(insert);
    updated.push_str(&content[idx + needle.len()..]);
    Some(updated)
}

fn has_entries(path: &Path) -> anyhow::Result<bool> {
    Ok(fs::read_dir(path)?.next().is_some())
}

fn render_model_mod(models: &[ModelSpec], orm: ModelOrm) -> String {
    let mut out = String::from("#![allow(dead_code, unused_imports)]\n\n");
    for model in models {
        let module = to_snake_case(&model.name);
        let pascal = to_pascal_case(&model.name);
        out.push_str(&format!("pub mod {module};\n"));
        match orm {
            ModelOrm::SeaOrm => out.push_str(&format!(
                "pub use {module}::{{{pascal}Repository, ActiveModel as {pascal}ActiveModel, Entity as {pascal}Entity, Model as {pascal}Model}};\n"
            )),
            ModelOrm::Toasty => out.push_str(&format!(
                "pub use {module}::{{{pascal}, {pascal}Repository}};\n"
            )),
        }
    }
    out
}

fn render_mongo_model_mod(models: &[ModelSpec]) -> String {
    let mut out = String::from("#![allow(dead_code, unused_imports)]\n\n");
    for model in models {
        let module = to_snake_case(&model.name);
        let pascal = to_pascal_case(&model.name);
        out.push_str(&format!("pub mod {module};\n"));
        out.push_str(&format!(
            "pub use {module}::{{{pascal}Repository, Model as {pascal}Model}};\n"
        ));
    }
    out
}

fn render_model_module(model: &ModelSpec) -> String {
    let pascal = to_pascal_case(&model.name);
    let primary = &model.primary;
    let primary_ty = model
        .fields
        .iter()
        .find(|field| field.name == *primary)
        .map(|field| field.ty.clone())
        .expect("primary field present");
    let table_name = &model.table;
    let cache_ttl_secs = model.cache_ttl_secs.unwrap_or(300);
    let negative_cache_ttl_secs = model
        .negative_cache_ttl_secs
        .unwrap_or_else(|| (cache_ttl_secs / 6).clamp(5, 60));
    let cache_prefix = model.cache_prefix.as_deref().unwrap_or(table_name);
    let cache_fields = cache_lookup_fields(model);
    let mut out = String::new();
    use std::fmt::Write as _;

    writeln!(&mut out, "#![allow(dead_code, unused_imports)]").unwrap();
    writeln!(&mut out).unwrap();
    writeln!(&mut out, "use std::time::Duration;").unwrap();
    writeln!(&mut out, "use sea_orm::entity::prelude::*;").unwrap();
    writeln!(
        &mut out,
        "use sea_orm::{{sea_query::Expr, ActiveModelTrait, ColumnTrait, DatabaseConnection, DeleteResult, EntityTrait, IntoActiveModel, PaginatorTrait, QueryFilter, QueryOrder, Select, UpdateResult}};"
    )
    .unwrap();
    writeln!(&mut out, "use serde::{{Deserialize, Serialize}};").unwrap();
    writeln!(&mut out).unwrap();
    writeln!(&mut out, "use crate::svc::ServiceContext;").unwrap();
    writeln!(&mut out).unwrap();
    writeln!(
        &mut out,
        "#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, DeriveEntityModel)]"
    )
    .unwrap();
    match &model.schema_name {
        Some(schema_name) => {
            writeln!(
                &mut out,
                "#[sea_orm(schema_name = \"{}\", table_name = \"{}\")]",
                schema_name, table_name
            )
            .unwrap();
        }
        None => {
            writeln!(&mut out, "#[sea_orm(table_name = \"{}\")]", table_name).unwrap();
        }
    }
    writeln!(&mut out, "pub struct Model {{").unwrap();
    for field in &model.fields {
        if field.name == model.primary {
            writeln!(&mut out, "    #[sea_orm(primary_key)]").unwrap();
        }
        if let Some(comment) = &field.comment {
            writeln!(&mut out, "    /// {}", comment.replace('\n', " ")).unwrap();
        }
        if let Some(default_value) = &field.default_value {
            writeln!(
                &mut out,
                "    /// default: {}",
                default_value.replace('\n', " ")
            )
            .unwrap();
        }
        writeln!(&mut out, "    pub {}: {},", field.name, field.ty).unwrap();
    }
    writeln!(&mut out, "}}").unwrap();
    writeln!(&mut out).unwrap();
    writeln!(
        &mut out,
        "#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]"
    )
    .unwrap();
    writeln!(&mut out, "pub enum Relation {{}}").unwrap();
    writeln!(&mut out).unwrap();
    writeln!(&mut out, "impl ActiveModelBehavior for ActiveModel {{}}").unwrap();
    writeln!(&mut out).unwrap();
    render_query_types(&mut out, model, &pascal, "Model");
    writeln!(&mut out, "pub struct {}Repository<'a> {{", pascal).unwrap();
    writeln!(&mut out, "    ctx: &'a ServiceContext,").unwrap();
    writeln!(&mut out, "}}").unwrap();
    writeln!(&mut out).unwrap();
    writeln!(&mut out, "impl<'a> {}Repository<'a> {{", pascal).unwrap();
    writeln!(
        &mut out,
        "    pub fn new(ctx: &'a ServiceContext) -> Self {{"
    )
    .unwrap();
    writeln!(&mut out, "        Self {{ ctx }}").unwrap();
    writeln!(&mut out, "    }}").unwrap();
    writeln!(&mut out).unwrap();
    writeln!(
        &mut out,
        "    fn read_db(&self) -> anyhow::Result<&DatabaseConnection> {{"
    )
    .unwrap();
    writeln!(&mut out, "        self.ctx.read_db()").unwrap();
    writeln!(&mut out, "    }}").unwrap();
    writeln!(&mut out).unwrap();
    writeln!(
        &mut out,
        "    fn write_db(&self) -> anyhow::Result<&DatabaseConnection> {{"
    )
    .unwrap();
    writeln!(&mut out, "        self.ctx.write_db()").unwrap();
    writeln!(&mut out, "    }}").unwrap();
    writeln!(&mut out).unwrap();
    writeln!(&mut out, "    pub fn table_name() -> &'static str {{").unwrap();
    writeln!(&mut out, "        \"{}\"", table_name).unwrap();
    writeln!(&mut out, "    }}").unwrap();
    writeln!(&mut out).unwrap();
    render_sea_orm_scope_methods(&mut out, model);
    writeln!(
        &mut out,
        "    pub async fn count(&self) -> anyhow::Result<u64> {{"
    )
    .unwrap();
    writeln!(&mut out, "        let db = self.read_db()?;").unwrap();
    writeln!(&mut out, "        let mut select = Entity::find();").unwrap();
    if model.soft_delete.is_some() {
        writeln!(&mut out, "        select = self.apply_live_scope(select);").unwrap();
    }
    writeln!(&mut out, "        Ok(select.count(db).await?)").unwrap();
    writeln!(&mut out, "    }}").unwrap();
    writeln!(&mut out).unwrap();
    render_sea_orm_query_methods(&mut out, model, &pascal, primary, &primary_ty);
    writeln!(
        &mut out,
        "    pub async fn find_by_{}(&self, {}: {}) -> anyhow::Result<Option<Model>> {{",
        primary, primary, primary_ty
    )
    .unwrap();
    if model.cache {
        writeln!(
            &mut out,
            "        self.cached_find_by_{}({}).await",
            primary, primary
        )
        .unwrap();
        writeln!(&mut out, "    }}").unwrap();
        writeln!(&mut out).unwrap();
        writeln!(
            &mut out,
            "    pub async fn find_by_{}_uncached(&self, {}: {}) -> anyhow::Result<Option<Model>> {{",
            primary, primary, primary_ty
        )
        .unwrap();
    }
    writeln!(&mut out, "        let db = self.read_db()?;").unwrap();
    writeln!(
        &mut out,
        "        Ok(Entity::find_by_id({}).one(db).await?)",
        primary
    )
    .unwrap();
    writeln!(&mut out, "    }}").unwrap();
    writeln!(&mut out).unwrap();
    writeln!(
        &mut out,
        "    pub async fn find_by_{}_primary(&self, {}: {}) -> anyhow::Result<Option<Model>> {{",
        primary, primary, primary_ty
    )
    .unwrap();
    writeln!(&mut out, "        let db = self.write_db()?;").unwrap();
    writeln!(
        &mut out,
        "        Ok(Entity::find_by_id({}).one(db).await?)",
        primary
    )
    .unwrap();
    writeln!(&mut out, "    }}").unwrap();
    writeln!(&mut out).unwrap();
    for field in cache_fields.iter().filter(|field| field.name != *primary) {
        let field_name = &field.name;
        let field_ty = &field.ty;
        let column = to_pascal_case(field_name);
        writeln!(
            &mut out,
            "    pub async fn find_by_{}(&self, {}: {}) -> anyhow::Result<Option<Model>> {{",
            field_name, field_name, field_ty
        )
        .unwrap();
        if model.cache {
            writeln!(
                &mut out,
                "        self.cached_find_by_{}({}).await",
                field_name, field_name
            )
            .unwrap();
            writeln!(&mut out, "    }}").unwrap();
            writeln!(&mut out).unwrap();
            writeln!(
                &mut out,
                "    pub async fn find_by_{}_uncached(&self, {}: {}) -> anyhow::Result<Option<Model>> {{",
                field_name, field_name, field_ty
            )
            .unwrap();
        }
        writeln!(&mut out, "        let db = self.read_db()?;").unwrap();
        writeln!(
            &mut out,
            "        Ok(Entity::find().filter(Column::{}.eq({})).one(db).await?)",
            column, field_name
        )
        .unwrap();
        writeln!(&mut out, "    }}").unwrap();
        writeln!(&mut out).unwrap();
        writeln!(
            &mut out,
            "    pub async fn find_by_{}_primary(&self, {}: {}) -> anyhow::Result<Option<Model>> {{",
            field_name, field_name, field_ty
        )
        .unwrap();
        writeln!(&mut out, "        let db = self.write_db()?;").unwrap();
        writeln!(
            &mut out,
            "        Ok(Entity::find().filter(Column::{}.eq({})).one(db).await?)",
            column, field_name
        )
        .unwrap();
        writeln!(&mut out, "    }}").unwrap();
        writeln!(&mut out).unwrap();
    }
    writeln!(
        &mut out,
        "    pub async fn list(&self) -> anyhow::Result<Vec<Model>> {{"
    )
    .unwrap();
    writeln!(&mut out, "        let db = self.read_db()?;").unwrap();
    writeln!(&mut out, "        let mut select = Entity::find();").unwrap();
    if model.soft_delete.is_some() {
        writeln!(&mut out, "        select = self.apply_live_scope(select);").unwrap();
    }
    writeln!(&mut out, "        Ok(select.all(db).await?)").unwrap();
    writeln!(&mut out, "    }}").unwrap();
    writeln!(&mut out).unwrap();
    writeln!(
        &mut out,
        "    pub async fn insert(&self, model: Model) -> anyhow::Result<Model> {{"
    )
    .unwrap();
    writeln!(&mut out, "        let db = self.write_db()?;").unwrap();
    writeln!(
        &mut out,
        "        let active: ActiveModel = model.into_active_model();"
    )
    .unwrap();
    writeln!(&mut out, "        let inserted = active.insert(db).await?;").unwrap();
    if model.cache {
        writeln!(
            &mut out,
            "        self.invalidate_model_cache(&inserted).await?;"
        )
        .unwrap();
    }
    writeln!(&mut out, "        Ok(inserted)").unwrap();
    writeln!(&mut out, "    }}").unwrap();
    writeln!(&mut out).unwrap();
    render_sea_orm_batch_methods(&mut out, model, primary, &primary_ty);
    writeln!(
        &mut out,
        "    pub async fn update(&self, model: Model) -> anyhow::Result<Model> {{"
    )
    .unwrap();
    writeln!(&mut out, "        let db = self.write_db()?;").unwrap();
    writeln!(
        &mut out,
        "        let active: ActiveModel = model.into_active_model();"
    )
    .unwrap();
    writeln!(&mut out, "        let updated = active.update(db).await?;").unwrap();
    if model.cache {
        writeln!(
            &mut out,
            "        self.invalidate_model_cache(&updated).await?;"
        )
        .unwrap();
    }
    writeln!(&mut out, "        Ok(updated)").unwrap();
    writeln!(&mut out, "    }}").unwrap();
    writeln!(&mut out).unwrap();
    writeln!(
        &mut out,
        "    pub async fn delete_by_{}(&self, {}: {}) -> anyhow::Result<DeleteResult> {{",
        primary, primary, primary_ty
    )
    .unwrap();
    writeln!(&mut out, "        let db = self.write_db()?;").unwrap();
    if model.cache {
        writeln!(
            &mut out,
            "        let existing = self.find_by_{}_uncached({}.clone()).await?;",
            primary, primary
        )
        .unwrap();
    }
    writeln!(&mut out, "        let delete_key = {}.clone();", primary).unwrap();
    writeln!(
        &mut out,
        "        let result = Entity::delete_by_id(delete_key).exec(db).await?;"
    )
    .unwrap();
    if model.cache {
        writeln!(
            &mut out,
            "        if let Some(model) = existing.as_ref() {{"
        )
        .unwrap();
        writeln!(
            &mut out,
            "            self.invalidate_model_cache(model).await?;"
        )
        .unwrap();
        writeln!(&mut out, "        }} else {{").unwrap();
        writeln!(
            &mut out,
            "            self.invalidate_cache_field(\"{}\", &{}).await?;",
            primary, primary
        )
        .unwrap();
        writeln!(&mut out, "        }}").unwrap();
    }
    writeln!(&mut out, "        Ok(result)").unwrap();
    writeln!(&mut out, "    }}").unwrap();
    render_sea_orm_soft_delete_methods(&mut out, model, primary, &primary_ty);
    if model.cache {
        writeln!(&mut out).unwrap();
        writeln!(
            &mut out,
            "    fn cache_key(&self, field: &str, value: impl std::fmt::Display) -> String {{"
        )
        .unwrap();
        writeln!(
            &mut out,
            "        format!(\"{}:{{}}:{{}}\", field, value)",
            cache_prefix
        )
        .unwrap();
        writeln!(&mut out, "    }}").unwrap();
        writeln!(&mut out).unwrap();
        writeln!(
            &mut out,
            "    async fn invalidate_model_cache(&self, model: &Model) -> anyhow::Result<()> {{"
        )
        .unwrap();
        writeln!(
            &mut out,
            "        if let Some(cache) = self.ctx.cache.as_ref() {{"
        )
        .unwrap();
        for field in &cache_fields {
            writeln!(
                &mut out,
                "            cache.del(&self.cache_key(\"{}\", &model.{})).await?;",
                field.name, field.name
            )
            .unwrap();
        }
        writeln!(&mut out, "        }}").unwrap();
        writeln!(&mut out, "        Ok(())").unwrap();
        writeln!(&mut out, "    }}").unwrap();
        writeln!(&mut out).unwrap();
        writeln!(
            &mut out,
            "    async fn invalidate_cache_field(&self, field: &str, value: impl std::fmt::Display) -> anyhow::Result<()> {{"
        )
        .unwrap();
        writeln!(
            &mut out,
            "        if let Some(cache) = self.ctx.cache.as_ref() {{"
        )
        .unwrap();
        writeln!(
            &mut out,
            "            let key = self.cache_key(field, value);"
        )
        .unwrap();
        writeln!(&mut out, "            cache.del(&key).await?;").unwrap();
        writeln!(&mut out, "        }}").unwrap();
        writeln!(&mut out, "        Ok(())").unwrap();
        writeln!(&mut out, "    }}").unwrap();
        writeln!(&mut out).unwrap();
        writeln!(
            &mut out,
            "    pub async fn cached_find_by_{}(&self, {}: {}) -> anyhow::Result<Option<Model>> {{",
            primary, primary, primary_ty
        )
        .unwrap();
        writeln!(
            &mut out,
            "        if let Some(cache) = self.ctx.cache.as_ref() {{"
        )
        .unwrap();
        writeln!(
            &mut out,
            "            let key = self.cache_key(\"{}\", &{});",
            primary, primary
        )
        .unwrap();
        writeln!(
            &mut out,
            "            let ttl = Duration::from_secs({});",
            cache_ttl_secs
        )
        .unwrap();
        writeln!(
            &mut out,
            "            let negative_ttl = Duration::from_secs({});",
            negative_cache_ttl_secs
        )
        .unwrap();
        writeln!(&mut out, "            let lookup = {}.clone();", primary).unwrap();
        writeln!(&mut out, "            return cache").unwrap();
        writeln!(&mut out, "                .get_or_set_json_option(").unwrap();
        writeln!(&mut out, "                    &key,").unwrap();
        writeln!(&mut out, "                    Some(ttl),").unwrap();
        writeln!(&mut out, "                    Some(negative_ttl),").unwrap();
        writeln!(
            &mut out,
            "                    || async move {{ self.find_by_{}_uncached(lookup).await }},",
            primary
        )
        .unwrap();
        writeln!(&mut out, "                )").unwrap();
        writeln!(&mut out, "                .await;").unwrap();
        writeln!(&mut out, "        }}").unwrap();
        writeln!(&mut out).unwrap();
        writeln!(
            &mut out,
            "        self.find_by_{}_uncached({}).await",
            primary, primary
        )
        .unwrap();
        writeln!(&mut out, "    }}").unwrap();
        for field in cache_fields.iter().filter(|field| field.name != *primary) {
            let field_name = &field.name;
            let field_ty = &field.ty;
            writeln!(&mut out).unwrap();
            writeln!(
                &mut out,
                "    pub async fn cached_find_by_{}(&self, {}: {}) -> anyhow::Result<Option<Model>> {{",
                field_name, field_name, field_ty
            )
            .unwrap();
            writeln!(
                &mut out,
                "        if let Some(cache) = self.ctx.cache.as_ref() {{"
            )
            .unwrap();
            writeln!(
                &mut out,
                "            let key = self.cache_key(\"{}\", &{});",
                field_name, field_name
            )
            .unwrap();
            writeln!(
                &mut out,
                "            let ttl = Duration::from_secs({});",
                cache_ttl_secs
            )
            .unwrap();
            writeln!(
                &mut out,
                "            let negative_ttl = Duration::from_secs({});",
                negative_cache_ttl_secs
            )
            .unwrap();
            writeln!(&mut out, "            let lookup = {}.clone();", field_name).unwrap();
            writeln!(&mut out, "            return cache").unwrap();
            writeln!(&mut out, "                .get_or_set_json_option(").unwrap();
            writeln!(&mut out, "                    &key,").unwrap();
            writeln!(&mut out, "                    Some(ttl),").unwrap();
            writeln!(&mut out, "                    Some(negative_ttl),").unwrap();
            writeln!(
                &mut out,
                "                    || async move {{ self.find_by_{}_uncached(lookup).await }},",
                field_name
            )
            .unwrap();
            writeln!(&mut out, "                )").unwrap();
            writeln!(&mut out, "                .await;").unwrap();
            writeln!(&mut out, "        }}").unwrap();
            writeln!(&mut out).unwrap();
            writeln!(
                &mut out,
                "        self.find_by_{}_uncached({}).await",
                field_name, field_name
            )
            .unwrap();
            writeln!(&mut out, "    }}").unwrap();
        }
    }
    writeln!(&mut out, "}}").unwrap();

    out
}

fn render_query_types(out: &mut String, model: &ModelSpec, pascal: &str, item_ty: &str) {
    use std::fmt::Write as _;
    writeln!(
        out,
        "#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]"
    )
    .unwrap();
    writeln!(out, "pub enum {pascal}SortField {{").unwrap();
    for field in sea_orm_filter_fields(model) {
        writeln!(out, "    {},", to_pascal_case(&field.name)).unwrap();
    }
    writeln!(out, "}}").unwrap();
    writeln!(out).unwrap();
    writeln!(
        out,
        "#[derive(Clone, Debug, Default, Serialize, Deserialize)]"
    )
    .unwrap();
    writeln!(out, "pub struct {pascal}Query {{").unwrap();
    writeln!(out, "    pub page: u64,").unwrap();
    writeln!(out, "    pub page_size: u64,").unwrap();
    writeln!(out, "    pub sort_by: Option<{pascal}SortField>,").unwrap();
    writeln!(out, "    pub sort_desc: bool,").unwrap();
    if model.soft_delete.is_some() {
        writeln!(out, "    pub include_deleted: bool,").unwrap();
    }
    for field in sea_orm_filter_fields(model) {
        writeln!(out, "    pub {}: Option<{}>,", field.name, field.ty).unwrap();
        writeln!(out, "    pub {}_in: Vec<{}>,", field.name, field.ty).unwrap();
        if is_numeric_type(&field.ty) {
            writeln!(out, "    pub {}_min: Option<{}>,", field.name, field.ty).unwrap();
            writeln!(out, "    pub {}_max: Option<{}>,", field.name, field.ty).unwrap();
        }
    }
    writeln!(out, "}}").unwrap();
    writeln!(out).unwrap();
    writeln!(out, "#[derive(Clone, Debug, Serialize, Deserialize)]").unwrap();
    writeln!(out, "pub struct {pascal}Page {{").unwrap();
    writeln!(out, "    pub items: Vec<{item_ty}>,").unwrap();
    writeln!(out, "    pub total: u64,").unwrap();
    writeln!(out, "    pub page: u64,").unwrap();
    writeln!(out, "    pub page_size: u64,").unwrap();
    writeln!(out, "}}").unwrap();
    writeln!(out).unwrap();
}

fn render_sea_orm_scope_methods(out: &mut String, model: &ModelSpec) {
    use std::fmt::Write as _;
    let Some(field) = model
        .soft_delete
        .as_deref()
        .and_then(|name| model.fields.iter().find(|field| field.name == name))
    else {
        return;
    };
    let column = to_pascal_case(&field.name);
    writeln!(
        out,
        "    fn apply_live_scope(&self, select: Select<Entity>) -> Select<Entity> {{"
    )
    .unwrap();
    if field.ty == "bool" {
        writeln!(out, "        select.filter(Column::{column}.eq(false))").unwrap();
    } else if is_optional_type(&field.ty) {
        writeln!(out, "        select.filter(Column::{column}.is_null())").unwrap();
    } else {
        writeln!(out, "        select").unwrap();
    }
    writeln!(out, "    }}").unwrap();
    writeln!(out).unwrap();
}

fn render_sea_orm_query_methods(
    out: &mut String,
    model: &ModelSpec,
    pascal: &str,
    primary: &str,
    primary_ty: &str,
) {
    use std::fmt::Write as _;
    writeln!(
        out,
        "    pub async fn query(&self, req: {pascal}Query) -> anyhow::Result<{pascal}Page> {{"
    )
    .unwrap();
    writeln!(out, "        let db = self.read_db()?;").unwrap();
    writeln!(
        out,
        "        let page = if req.page == 0 {{ 1 }} else {{ req.page }};"
    )
    .unwrap();
    writeln!(
        out,
        "        let page_size = if req.page_size == 0 {{ 20 }} else {{ req.page_size.min(500) }};"
    )
    .unwrap();
    writeln!(out, "        let mut select = Entity::find();").unwrap();
    if model.soft_delete.is_some() {
        writeln!(
            out,
            "        if !req.include_deleted {{ select = self.apply_live_scope(select); }}"
        )
        .unwrap();
    }
    for field in sea_orm_filter_fields(model) {
        let column = to_pascal_case(&field.name);
        writeln!(
            out,
            "        if let Some(value) = req.{} {{ select = select.filter(Column::{column}.eq(value)); }}",
            field.name
        )
        .unwrap();
        writeln!(
            out,
            "        if !req.{}_in.is_empty() {{ select = select.filter(Column::{column}.is_in(req.{}_in)); }}",
            field.name, field.name
        )
        .unwrap();
        if is_numeric_type(&field.ty) {
            writeln!(
                out,
                "        if let Some(value) = req.{}_min {{ select = select.filter(Column::{column}.gte(value)); }}",
                field.name
            )
            .unwrap();
            writeln!(
                out,
                "        if let Some(value) = req.{}_max {{ select = select.filter(Column::{column}.lte(value)); }}",
                field.name
            )
            .unwrap();
        }
    }
    render_sea_orm_sort(out, model, pascal);
    writeln!(
        out,
        "        let paginator = select.paginate(db, page_size);"
    )
    .unwrap();
    writeln!(out, "        let total = paginator.num_items().await?;").unwrap();
    writeln!(
        out,
        "        let items = paginator.fetch_page(page.saturating_sub(1)).await?;"
    )
    .unwrap();
    writeln!(
        out,
        "        Ok({pascal}Page {{ items, total, page, page_size }})"
    )
    .unwrap();
    writeln!(out, "    }}").unwrap();
    writeln!(out).unwrap();
    writeln!(
        out,
        "    pub async fn list_page(&self, page: u64, page_size: u64) -> anyhow::Result<{pascal}Page> {{"
    )
    .unwrap();
    if model.soft_delete.is_some() {
        writeln!(
            out,
            "        self.query({pascal}Query {{ page, page_size, include_deleted: false, ..Default::default() }}).await"
        )
        .unwrap();
    } else {
        writeln!(
            out,
            "        self.query({pascal}Query {{ page, page_size, ..Default::default() }}).await"
        )
        .unwrap();
    }
    writeln!(out, "    }}").unwrap();
    if let Some(tenant_field) = tenant_field(model) {
        writeln!(out).unwrap();
        let tenant_column = to_pascal_case(&tenant_field.name);
        let primary_column = to_pascal_case(primary);
        writeln!(
            out,
            "    pub async fn find_by_{primary}_for_{}(&self, {primary}: {primary_ty}, {}: {}) -> anyhow::Result<Option<Model>> {{",
            tenant_field.name, tenant_field.name, tenant_field.ty
        )
        .unwrap();
        writeln!(out, "        let db = self.read_db()?;").unwrap();
        writeln!(
            out,
            "        let mut select = Entity::find().filter(Column::{primary_column}.eq({primary})).filter(Column::{tenant_column}.eq({}));",
            tenant_field.name
        )
        .unwrap();
        if model.soft_delete.is_some() {
            writeln!(out, "        select = self.apply_live_scope(select);").unwrap();
        }
        writeln!(out, "        Ok(select.one(db).await?)").unwrap();
        writeln!(out, "    }}").unwrap();
    }
    writeln!(out).unwrap();
}

fn render_sea_orm_sort(out: &mut String, model: &ModelSpec, pascal: &str) {
    use std::fmt::Write as _;
    let fields = sea_orm_filter_fields(model);
    if fields.is_empty() {
        return;
    }
    writeln!(out, "        match req.sort_by {{").unwrap();
    for field in fields {
        let variant = to_pascal_case(&field.name);
        writeln!(
            out,
            "            Some({pascal}SortField::{variant}) if req.sort_desc => select = select.order_by_desc(Column::{variant}),"
        )
        .unwrap();
        writeln!(
            out,
            "            Some({pascal}SortField::{variant}) => select = select.order_by_asc(Column::{variant}),"
        )
        .unwrap();
    }
    writeln!(out, "            None => {{}}").unwrap();
    writeln!(out, "        }}").unwrap();
}

fn render_sea_orm_batch_methods(
    out: &mut String,
    model: &ModelSpec,
    primary: &str,
    primary_ty: &str,
) {
    use std::fmt::Write as _;
    let primary_column = to_pascal_case(primary);
    writeln!(
        out,
        "    pub async fn insert_many(&self, models: Vec<Model>) -> anyhow::Result<()> {{"
    )
    .unwrap();
    writeln!(out, "        if models.is_empty() {{ return Ok(()); }}").unwrap();
    writeln!(out, "        let db = self.write_db()?;").unwrap();
    writeln!(
        out,
        "        let active_models = models.into_iter().map(IntoActiveModel::into_active_model).collect::<Vec<_>>();"
    )
    .unwrap();
    writeln!(
        out,
        "        Entity::insert_many(active_models).exec(db).await?;"
    )
    .unwrap();
    writeln!(out, "        Ok(())").unwrap();
    writeln!(out, "    }}").unwrap();
    writeln!(out).unwrap();
    writeln!(
        out,
        "    pub async fn delete_many_by_ids(&self, ids: Vec<{primary_ty}>) -> anyhow::Result<DeleteResult> {{"
    )
    .unwrap();
    writeln!(
        out,
        "        if ids.is_empty() {{ return Ok(DeleteResult {{ rows_affected: 0 }}); }}"
    )
    .unwrap();
    writeln!(out, "        let db = self.write_db()?;").unwrap();
    writeln!(
        out,
        "        let result = Entity::delete_many().filter(Column::{primary_column}.is_in(ids.clone())).exec(db).await?;"
    )
    .unwrap();
    if model.cache {
        writeln!(out, "        for id in ids {{").unwrap();
        writeln!(
            out,
            "            self.invalidate_cache_field(\"{primary}\", &id).await?;"
        )
        .unwrap();
        writeln!(out, "        }}").unwrap();
    }
    writeln!(out, "        Ok(result)").unwrap();
    writeln!(out, "    }}").unwrap();
    writeln!(out).unwrap();
}

fn render_sea_orm_soft_delete_methods(
    out: &mut String,
    model: &ModelSpec,
    primary: &str,
    primary_ty: &str,
) {
    use std::fmt::Write as _;
    let Some(field) = model
        .soft_delete
        .as_deref()
        .and_then(|name| model.fields.iter().find(|field| field.name == name))
    else {
        return;
    };
    let primary_column = to_pascal_case(primary);
    let soft_column = to_pascal_case(&field.name);
    writeln!(out).unwrap();
    writeln!(
        out,
        "    pub async fn soft_delete_by_{primary}(&self, {primary}: {primary_ty}) -> anyhow::Result<UpdateResult> {{"
    )
    .unwrap();
    writeln!(out, "        let db = self.write_db()?;").unwrap();
    writeln!(
        out,
        "        let mut update = Entity::update_many().filter(Column::{primary_column}.eq({primary}.clone()));"
    )
    .unwrap();
    render_soft_delete_assignment(out, field, &soft_column);
    writeln!(out, "        let result = update.exec(db).await?;").unwrap();
    if model.cache {
        writeln!(
            out,
            "        self.invalidate_cache_field(\"{primary}\", &{primary}).await?;"
        )
        .unwrap();
    }
    writeln!(out, "        Ok(result)").unwrap();
    writeln!(out, "    }}").unwrap();
    writeln!(out).unwrap();
    writeln!(
        out,
        "    pub async fn soft_delete_many_by_ids(&self, ids: Vec<{primary_ty}>) -> anyhow::Result<UpdateResult> {{"
    )
    .unwrap();
    writeln!(
        out,
        "        if ids.is_empty() {{ return Ok(UpdateResult {{ rows_affected: 0 }}); }}"
    )
    .unwrap();
    writeln!(out, "        let db = self.write_db()?;").unwrap();
    writeln!(
        out,
        "        let mut update = Entity::update_many().filter(Column::{primary_column}.is_in(ids.clone()));"
    )
    .unwrap();
    render_soft_delete_assignment(out, field, &soft_column);
    writeln!(out, "        let result = update.exec(db).await?;").unwrap();
    if model.cache {
        writeln!(out, "        for id in ids {{").unwrap();
        writeln!(
            out,
            "            self.invalidate_cache_field(\"{primary}\", &id).await?;"
        )
        .unwrap();
        writeln!(out, "        }}").unwrap();
    }
    writeln!(out, "        Ok(result)").unwrap();
    writeln!(out, "    }}").unwrap();
}

fn render_soft_delete_assignment(out: &mut String, field: &ModelField, column: &str) {
    use std::fmt::Write as _;
    if field.ty == "bool" {
        writeln!(
            out,
            "        update = update.col_expr(Column::{column}, Expr::value(true));"
        )
        .unwrap();
    } else if field.ty == "Option<String>" {
        writeln!(
            out,
            "        update = update.col_expr(Column::{column}, Expr::value(Some(format!(\"{{:?}}\", std::time::SystemTime::now()))));"
        )
        .unwrap();
    } else if field.ty == "Option<i64>" || field.ty == "Option<u64>" {
        writeln!(
            out,
            "        update = update.col_expr(Column::{column}, Expr::value(Some(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|duration| duration.as_millis() as {}).unwrap_or_default())));",
            field.ty.trim_start_matches("Option<").trim_end_matches('>')
        )
        .unwrap();
    } else {
        writeln!(
            out,
            "        update = update.col_expr(Column::{column}, Expr::value(Default::default()));"
        )
        .unwrap();
    }
}

fn sea_orm_filter_fields(model: &ModelSpec) -> Vec<&ModelField> {
    model
        .fields
        .iter()
        .filter(|field| Some(field.name.as_str()) != model.soft_delete.as_deref())
        .filter(|field| !is_optional_type(&field.ty))
        .collect()
}

fn tenant_field(model: &ModelSpec) -> Option<&ModelField> {
    model
        .tenant
        .as_deref()
        .and_then(|name| model.fields.iter().find(|field| field.name == name))
}

fn render_toasty_model_module(model: &ModelSpec) -> String {
    let pascal = to_pascal_case(&model.name);
    let primary = &model.primary;
    let primary_ty = model
        .fields
        .iter()
        .find(|field| field.name == *primary)
        .map(|field| field.ty.clone())
        .expect("primary field present");
    let table_name = &model.table;
    let cache_fields = cache_lookup_fields(model);
    let mut out = String::new();
    use std::fmt::Write as _;

    writeln!(&mut out, "#![allow(dead_code, unused_imports)]").unwrap();
    writeln!(&mut out).unwrap();
    writeln!(&mut out, "use serde::{{Deserialize, Serialize}};").unwrap();
    writeln!(&mut out).unwrap();
    writeln!(
        &mut out,
        "#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, toasty::Model)]"
    )
    .unwrap();
    if model.schema_name.is_some() {
        writeln!(
            &mut out,
            "/// Toasty maps models by table name; configure schema/database selection in the Toasty driver."
        )
        .unwrap();
    }
    writeln!(&mut out, "#[table = \"{}\"]", table_name).unwrap();
    writeln!(&mut out, "pub struct {} {{", pascal).unwrap();
    for field in &model.fields {
        if field.name == *primary {
            writeln!(&mut out, "    #[key]").unwrap();
            if field.auto_increment {
                writeln!(&mut out, "    #[auto]").unwrap();
            }
        } else if cache_fields
            .iter()
            .any(|cache_field| cache_field.name == field.name)
        {
            writeln!(&mut out, "    #[unique]").unwrap();
        }
        if let Some(comment) = &field.comment {
            writeln!(&mut out, "    /// {}", comment.replace('\n', " ")).unwrap();
        }
        if let Some(default_value) = &field.default_value {
            writeln!(
                &mut out,
                "    /// default: {}",
                default_value.replace('\n', " ")
            )
            .unwrap();
        }
        writeln!(&mut out, "    pub {}: {},", field.name, field.ty).unwrap();
    }
    writeln!(&mut out, "}}").unwrap();
    writeln!(&mut out).unwrap();
    render_query_types(&mut out, model, &pascal, &pascal);
    writeln!(&mut out, "pub struct {}Repository;", pascal).unwrap();
    writeln!(&mut out).unwrap();
    writeln!(&mut out, "impl {}Repository {{", pascal).unwrap();
    writeln!(&mut out, "    pub fn table_name() -> &'static str {{").unwrap();
    writeln!(&mut out, "        \"{}\"", table_name).unwrap();
    writeln!(&mut out, "    }}").unwrap();
    writeln!(&mut out).unwrap();
    render_toasty_query_methods(&mut out, model, &pascal, primary, &primary_ty);
    writeln!(
        &mut out,
        "    pub async fn find_by_{}(db: &mut toasty::Db, {}: &{}) -> toasty::Result<{}> {{",
        primary, primary, primary_ty, pascal
    )
    .unwrap();
    writeln!(
        &mut out,
        "        {}::get_by_{}(db, {}).await",
        pascal, primary, primary
    )
    .unwrap();
    writeln!(&mut out, "    }}").unwrap();
    writeln!(&mut out).unwrap();
    for field in cache_fields.iter().filter(|field| field.name != *primary) {
        writeln!(
            &mut out,
            "    pub async fn find_by_{}(db: &mut toasty::Db, {}: &{}) -> toasty::Result<{}> {{",
            field.name, field.name, field.ty, pascal
        )
        .unwrap();
        writeln!(
            &mut out,
            "        {}::get_by_{}(db, {}).await",
            pascal, field.name, field.name
        )
        .unwrap();
        writeln!(&mut out, "    }}").unwrap();
        writeln!(&mut out).unwrap();
    }
    writeln!(
        &mut out,
        "    pub async fn list(db: &mut toasty::Db) -> toasty::Result<Vec<{}>> {{",
        pascal
    )
    .unwrap();
    writeln!(
        &mut out,
        "        {}::all().collect::<Vec<_>>(db).await",
        pascal
    )
    .unwrap();
    writeln!(&mut out, "    }}").unwrap();
    writeln!(&mut out).unwrap();
    writeln!(
        &mut out,
        "    pub async fn insert(db: &mut toasty::Db, model: {}) -> toasty::Result<{}> {{",
        pascal, pascal
    )
    .unwrap();
    write!(&mut out, "        {}::create()", pascal).unwrap();
    for field in &model.fields {
        if field.auto_increment {
            continue;
        }
        write!(
            &mut out,
            "\n            .{}(model.{})",
            field.name, field.name
        )
        .unwrap();
    }
    writeln!(&mut out, "\n            .exec(db)\n            .await").unwrap();
    writeln!(&mut out, "    }}").unwrap();
    writeln!(&mut out).unwrap();
    writeln!(
        &mut out,
        "    pub async fn update(db: &mut toasty::Db, mut model: {}) -> toasty::Result<{}> {{",
        pascal, pascal
    )
    .unwrap();
    write!(&mut out, "        model.update()").unwrap();
    for field in &model.fields {
        if field.name == *primary {
            continue;
        }
        write!(
            &mut out,
            "\n            .{}(model.{}.clone())",
            field.name, field.name
        )
        .unwrap();
    }
    writeln!(&mut out, "\n            .exec(db)\n            .await?;").unwrap();
    writeln!(&mut out, "        Ok(model)").unwrap();
    writeln!(&mut out, "    }}").unwrap();
    writeln!(&mut out).unwrap();
    render_toasty_batch_methods(&mut out, model, &pascal, primary, &primary_ty);
    writeln!(
        &mut out,
        "    pub async fn delete_by_{}(db: &mut toasty::Db, {}: &{}) -> toasty::Result<()> {{",
        primary, primary, primary_ty
    )
    .unwrap();
    writeln!(
        &mut out,
        "        let model = {}::get_by_{}(db, {}).await?;",
        pascal, primary, primary
    )
    .unwrap();
    writeln!(&mut out, "        model.delete().exec(db).await").unwrap();
    writeln!(&mut out, "    }}").unwrap();
    render_toasty_soft_delete_methods(&mut out, model, &pascal, primary, &primary_ty);
    writeln!(&mut out, "}}").unwrap();

    out
}

fn render_toasty_query_methods(
    out: &mut String,
    model: &ModelSpec,
    pascal: &str,
    primary: &str,
    primary_ty: &str,
) {
    use std::fmt::Write as _;
    writeln!(
        out,
        "    fn build_query(req: &{pascal}Query) -> toasty::stmt::Query<toasty::stmt::List<{pascal}>> {{"
    )
    .unwrap();
    writeln!(
        out,
        "        let mut query = toasty::stmt::Query::<toasty::stmt::List<{pascal}>>::all();"
    )
    .unwrap();
    if let Some(field) = soft_delete_field(model) {
        if field.ty == "bool" {
            writeln!(
                out,
                "        if !req.include_deleted {{ query = query.and({pascal}::fields().{}().eq(false)); }}",
                field.name
            )
            .unwrap();
        } else if let Some(inner_ty) = optional_inner_type(&field.ty) {
            writeln!(
                out,
                "        if !req.include_deleted {{ query = query.and({pascal}::fields().{}().eq(None::<{inner_ty}>)); }}",
                field.name
            )
            .unwrap();
        }
    }
    for field in sea_orm_filter_fields(model) {
        writeln!(
            out,
            "        if let Some(value) = req.{}.clone() {{ query = query.and({pascal}::fields().{}().eq(value)); }}",
            field.name, field.name
        )
        .unwrap();
        writeln!(
            out,
            "        if !req.{}_in.is_empty() {{ query = query.and({pascal}::fields().{}().in_list(req.{}_in.clone())); }}",
            field.name, field.name, field.name
        )
        .unwrap();
        if is_numeric_type(&field.ty) {
            writeln!(
                out,
                "        if let Some(value) = req.{}_min.clone() {{ query = query.and({pascal}::fields().{}().ge(value)); }}",
                field.name, field.name
            )
            .unwrap();
            writeln!(
                out,
                "        if let Some(value) = req.{}_max.clone() {{ query = query.and({pascal}::fields().{}().le(value)); }}",
                field.name, field.name
            )
            .unwrap();
        }
    }
    render_toasty_sort(out, model, pascal);
    writeln!(out, "        query").unwrap();
    writeln!(out, "    }}").unwrap();
    writeln!(out).unwrap();
    writeln!(
        out,
        "    pub async fn count(db: &mut toasty::Db) -> toasty::Result<u64> {{"
    )
    .unwrap();
    if model.soft_delete.is_some() {
        writeln!(
            out,
            "        Self::build_query(&{pascal}Query {{ include_deleted: false, ..Default::default() }}).count().exec(db).await"
        )
        .unwrap();
    } else {
        writeln!(
            out,
            "        Self::build_query(&{pascal}Query::default()).count().exec(db).await"
        )
        .unwrap();
    }
    writeln!(out, "    }}").unwrap();
    writeln!(out).unwrap();
    writeln!(
        out,
        "    pub async fn query(db: &mut toasty::Db, req: {pascal}Query) -> toasty::Result<{pascal}Page> {{"
    )
    .unwrap();
    writeln!(
        out,
        "        let page = if req.page == 0 {{ 1 }} else {{ req.page }};"
    )
    .unwrap();
    writeln!(
        out,
        "        let page_size = if req.page_size == 0 {{ 20 }} else {{ req.page_size.min(500) }};"
    )
    .unwrap();
    writeln!(out, "        let mut query = Self::build_query(&req);").unwrap();
    writeln!(
        out,
        "        let total = query.clone().count().exec(db).await?;"
    )
    .unwrap();
    writeln!(out, "        query.limit(page_size as usize);").unwrap();
    writeln!(
        out,
        "        query.offset(page.saturating_sub(1).saturating_mul(page_size) as usize);"
    )
    .unwrap();
    writeln!(out, "        let items = query.exec(db).await?;").unwrap();
    writeln!(
        out,
        "        Ok({pascal}Page {{ items, total, page, page_size }})"
    )
    .unwrap();
    writeln!(out, "    }}").unwrap();
    writeln!(out).unwrap();
    writeln!(
        out,
        "    pub async fn list_page(db: &mut toasty::Db, page: u64, page_size: u64) -> toasty::Result<{pascal}Page> {{"
    )
    .unwrap();
    if model.soft_delete.is_some() {
        writeln!(
            out,
            "        Self::query(db, {pascal}Query {{ page, page_size, include_deleted: false, ..Default::default() }}).await"
        )
        .unwrap();
    } else {
        writeln!(
            out,
            "        Self::query(db, {pascal}Query {{ page, page_size, ..Default::default() }}).await"
        )
        .unwrap();
    }
    writeln!(out, "    }}").unwrap();
    if let Some(tenant_field) = tenant_field(model) {
        writeln!(out).unwrap();
        writeln!(
            out,
            "    pub async fn find_by_{primary}_for_{}(db: &mut toasty::Db, {primary}: &{primary_ty}, {}: &{}) -> toasty::Result<Option<{pascal}>> {{",
            tenant_field.name, tenant_field.name, tenant_field.ty
        )
        .unwrap();
        writeln!(
            out,
            "        let page = Self::query(db, {pascal}Query {{ {primary}: Some({primary}.clone()), {}: Some({}.clone()), page: 1, page_size: 1, {}..Default::default() }}).await?;",
            tenant_field.name,
            tenant_field.name,
            if model.soft_delete.is_some() {
                "include_deleted: false, "
            } else {
                ""
            }
        )
        .unwrap();
        writeln!(out, "        Ok(page.items.into_iter().next())").unwrap();
        writeln!(out, "    }}").unwrap();
    }
    writeln!(out).unwrap();
}

fn render_toasty_sort(out: &mut String, model: &ModelSpec, pascal: &str) {
    use std::fmt::Write as _;
    let fields = sea_orm_filter_fields(model);
    if fields.is_empty() {
        return;
    }
    writeln!(out, "        match req.sort_by {{").unwrap();
    for field in fields {
        let variant = to_pascal_case(&field.name);
        writeln!(
            out,
            "            Some({pascal}SortField::{variant}) if req.sort_desc => {{ query.order_by({pascal}::fields().{}().desc()); }}",
            field.name
        )
        .unwrap();
        writeln!(
            out,
            "            Some({pascal}SortField::{variant}) => {{ query.order_by({pascal}::fields().{}().asc()); }}",
            field.name
        )
        .unwrap();
    }
    writeln!(out, "            None => {{}}").unwrap();
    writeln!(out, "        }}").unwrap();
}

fn render_toasty_batch_methods(
    out: &mut String,
    _model: &ModelSpec,
    pascal: &str,
    primary: &str,
    primary_ty: &str,
) {
    use std::fmt::Write as _;
    writeln!(
        out,
        "    pub async fn insert_many(db: &mut toasty::Db, models: Vec<{pascal}>) -> toasty::Result<Vec<{pascal}>> {{"
    )
    .unwrap();
    writeln!(
        out,
        "        let mut inserted = Vec::with_capacity(models.len());"
    )
    .unwrap();
    writeln!(out, "        for model in models {{").unwrap();
    writeln!(
        out,
        "            inserted.push(Self::insert(db, model).await?);"
    )
    .unwrap();
    writeln!(out, "        }}").unwrap();
    writeln!(out, "        Ok(inserted)").unwrap();
    writeln!(out, "    }}").unwrap();
    writeln!(out).unwrap();
    writeln!(
        out,
        "    pub async fn delete_many_by_ids(db: &mut toasty::Db, ids: Vec<{primary_ty}>) -> toasty::Result<u64> {{"
    )
    .unwrap();
    writeln!(out, "        let mut rows_affected = 0;").unwrap();
    writeln!(out, "        for id in ids {{").unwrap();
    writeln!(
        out,
        "            Self::delete_by_{primary}(db, &id).await?;"
    )
    .unwrap();
    writeln!(out, "            rows_affected += 1;").unwrap();
    writeln!(out, "        }}").unwrap();
    writeln!(out, "        Ok(rows_affected)").unwrap();
    writeln!(out, "    }}").unwrap();
    writeln!(out).unwrap();
}

fn render_toasty_soft_delete_methods(
    out: &mut String,
    model: &ModelSpec,
    pascal: &str,
    primary: &str,
    primary_ty: &str,
) {
    use std::fmt::Write as _;
    let Some(field) = soft_delete_field(model) else {
        return;
    };
    writeln!(out).unwrap();
    writeln!(
        out,
        "    pub async fn soft_delete_by_{primary}(db: &mut toasty::Db, {primary}: &{primary_ty}) -> toasty::Result<{pascal}> {{"
    )
    .unwrap();
    writeln!(
        out,
        "        let mut model = Self::find_by_{primary}(db, {primary}).await?;"
    )
    .unwrap();
    write!(out, "        model.update()\n            .{}", field.name).unwrap();
    render_toasty_soft_delete_value(out, field);
    writeln!(out, "\n            .exec(db)\n            .await?;").unwrap();
    writeln!(out, "        Ok(model)").unwrap();
    writeln!(out, "    }}").unwrap();
    writeln!(out).unwrap();
    writeln!(
        out,
        "    pub async fn soft_delete_many_by_ids(db: &mut toasty::Db, ids: Vec<{primary_ty}>) -> toasty::Result<u64> {{"
    )
    .unwrap();
    writeln!(out, "        let mut rows_affected = 0;").unwrap();
    writeln!(out, "        for id in ids {{").unwrap();
    writeln!(
        out,
        "            Self::soft_delete_by_{primary}(db, &id).await?;"
    )
    .unwrap();
    writeln!(out, "            rows_affected += 1;").unwrap();
    writeln!(out, "        }}").unwrap();
    writeln!(out, "        Ok(rows_affected)").unwrap();
    writeln!(out, "    }}").unwrap();
}

fn render_toasty_soft_delete_value(out: &mut String, field: &ModelField) {
    use std::fmt::Write as _;
    if field.ty == "bool" {
        write!(out, "(true)").unwrap();
    } else if field.ty == "Option<String>" {
        write!(
            out,
            "(Some(format!(\"{{:?}}\", std::time::SystemTime::now())))"
        )
        .unwrap();
    } else if field.ty == "Option<i64>" || field.ty == "Option<u64>" {
        let inner = field.ty.trim_start_matches("Option<").trim_end_matches('>');
        write!(
            out,
            "(Some(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|duration| duration.as_millis() as {inner}).unwrap_or_default()))"
        )
        .unwrap();
    } else {
        write!(out, "(Default::default())").unwrap();
    }
}

fn soft_delete_field(model: &ModelSpec) -> Option<&ModelField> {
    model
        .soft_delete
        .as_deref()
        .and_then(|name| model.fields.iter().find(|field| field.name == name))
}

fn optional_inner_type(ty: &str) -> Option<&str> {
    ty.trim()
        .strip_prefix("Option<")
        .and_then(|ty| ty.strip_suffix('>'))
}

fn render_mongo_model_module(model: &ModelSpec) -> String {
    let pascal = to_pascal_case(&model.name);
    let primary = &model.primary;
    let primary_ty = model
        .fields
        .iter()
        .find(|field| field.name == *primary)
        .map(|field| field.ty.clone())
        .expect("primary field present");
    let collection_name = &model.table;
    let cache_prefix = model.cache_prefix.as_deref().unwrap_or(collection_name);
    let cache_fields = cache_lookup_fields(model);
    let mut out = String::new();
    use std::fmt::Write as _;

    writeln!(&mut out, "#![allow(dead_code, unused_imports)]").unwrap();
    writeln!(&mut out).unwrap();
    writeln!(&mut out, "use std::time::Duration;").unwrap();
    writeln!(
        &mut out,
        "use roze_mongo::bson::{{self, doc, oid::ObjectId, DateTime, Document}};"
    )
    .unwrap();
    writeln!(&mut out, "use roze_mongo::Collection;").unwrap();
    writeln!(&mut out, "use serde::{{Deserialize, Serialize}};").unwrap();
    writeln!(&mut out).unwrap();
    writeln!(&mut out, "use crate::svc::ServiceContext;").unwrap();
    writeln!(&mut out).unwrap();
    writeln!(
        &mut out,
        "#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]"
    )
    .unwrap();
    writeln!(&mut out, "pub struct Model {{").unwrap();
    for field in &model.fields {
        if field.name == "id" && model.primary == "id" {
            writeln!(&mut out, "    #[serde(rename = \"_id\")]").unwrap();
        }
        if let Some(comment) = &field.comment {
            writeln!(&mut out, "    /// {}", comment.replace('\n', " ")).unwrap();
        }
        writeln!(&mut out, "    pub {}: {},", field.name, field.ty).unwrap();
    }
    writeln!(&mut out, "}}").unwrap();
    writeln!(&mut out).unwrap();
    writeln!(&mut out, "pub struct {}Repository<'a> {{", pascal).unwrap();
    writeln!(&mut out, "    ctx: &'a ServiceContext,").unwrap();
    writeln!(&mut out, "}}").unwrap();
    writeln!(&mut out).unwrap();
    writeln!(&mut out, "impl<'a> {}Repository<'a> {{", pascal).unwrap();
    writeln!(
        &mut out,
        "    pub fn new(ctx: &'a ServiceContext) -> Self {{"
    )
    .unwrap();
    writeln!(&mut out, "        Self {{ ctx }}").unwrap();
    writeln!(&mut out, "    }}").unwrap();
    writeln!(&mut out).unwrap();
    writeln!(
        &mut out,
        "    fn collection(&self) -> anyhow::Result<Collection<Model>> {{"
    )
    .unwrap();
    writeln!(
        &mut out,
        "        let mongo = self.ctx.mongo.as_ref().ok_or_else(|| anyhow::anyhow!(\"mongo connection is not configured\"))?;"
    )
    .unwrap();
    writeln!(
        &mut out,
        "        Ok(mongo.collection(\"{}\"))",
        collection_name
    )
    .unwrap();
    writeln!(&mut out, "    }}").unwrap();
    writeln!(&mut out).unwrap();
    writeln!(
        &mut out,
        "    fn filter_by<T: Serialize + ?Sized>(field: &str, value: &T) -> anyhow::Result<Document> {{"
    )
    .unwrap();
    writeln!(&mut out, "        let mut filter = Document::new();").unwrap();
    writeln!(
        &mut out,
        "        filter.insert(field, bson::to_bson(value)?);"
    )
    .unwrap();
    writeln!(&mut out, "        Ok(filter)").unwrap();
    writeln!(&mut out, "    }}").unwrap();
    writeln!(&mut out).unwrap();
    writeln!(&mut out, "    pub fn collection_name() -> &'static str {{").unwrap();
    writeln!(&mut out, "        \"{}\"", collection_name).unwrap();
    writeln!(&mut out, "    }}").unwrap();
    writeln!(&mut out).unwrap();
    render_mongo_find_methods(&mut out, model, primary, &primary_ty, true);
    for field in cache_fields.iter().filter(|field| field.name != *primary) {
        render_mongo_find_methods(&mut out, model, &field.name, &field.ty, false);
    }
    writeln!(
        &mut out,
        "    pub async fn list(&self) -> anyhow::Result<Vec<Model>> {{"
    )
    .unwrap();
    writeln!(
        &mut out,
        "        let mut cursor = self.collection()?.find(doc! {{}}).await?;"
    )
    .unwrap();
    writeln!(&mut out, "        let mut items = Vec::new();").unwrap();
    writeln!(&mut out, "        while cursor.advance().await? {{").unwrap();
    writeln!(
        &mut out,
        "            items.push(cursor.deserialize_current()?);"
    )
    .unwrap();
    writeln!(&mut out, "        }}").unwrap();
    writeln!(&mut out, "        Ok(items)").unwrap();
    writeln!(&mut out, "    }}").unwrap();
    writeln!(&mut out).unwrap();
    writeln!(
        &mut out,
        "    pub async fn insert(&self, model: Model) -> anyhow::Result<Model> {{"
    )
    .unwrap();
    writeln!(
        &mut out,
        "        self.collection()?.insert_one(&model).await?;"
    )
    .unwrap();
    if model.cache {
        writeln!(
            &mut out,
            "        self.invalidate_model_cache(&model).await?;"
        )
        .unwrap();
    }
    writeln!(&mut out, "        Ok(model)").unwrap();
    writeln!(&mut out, "    }}").unwrap();
    writeln!(&mut out).unwrap();
    writeln!(
        &mut out,
        "    pub async fn update(&self, model: Model) -> anyhow::Result<Model> {{"
    )
    .unwrap();
    writeln!(
        &mut out,
        "        let filter = Self::filter_by(\"{}\", &model.{})?;",
        mongo_field_name(primary),
        primary
    )
    .unwrap();
    writeln!(
        &mut out,
        "        self.collection()?.replace_one(filter, &model).await?;"
    )
    .unwrap();
    if model.cache {
        writeln!(
            &mut out,
            "        self.invalidate_model_cache(&model).await?;"
        )
        .unwrap();
    }
    writeln!(&mut out, "        Ok(model)").unwrap();
    writeln!(&mut out, "    }}").unwrap();
    writeln!(&mut out).unwrap();
    writeln!(
        &mut out,
        "    pub async fn delete_by_{}(&self, {}: {}) -> anyhow::Result<u64> {{",
        primary, primary, primary_ty
    )
    .unwrap();
    if model.cache {
        writeln!(
            &mut out,
            "        let existing = self.find_by_{}_uncached({}.clone()).await?;",
            primary, primary
        )
        .unwrap();
    }
    writeln!(
        &mut out,
        "        let filter = Self::filter_by(\"{}\", &{})?;",
        mongo_field_name(primary),
        primary
    )
    .unwrap();
    writeln!(
        &mut out,
        "        let result = self.collection()?.delete_one(filter).await?;"
    )
    .unwrap();
    if model.cache {
        writeln!(
            &mut out,
            "        if let Some(model) = existing.as_ref() {{"
        )
        .unwrap();
        writeln!(
            &mut out,
            "            self.invalidate_model_cache(model).await?;"
        )
        .unwrap();
        writeln!(&mut out, "        }} else {{").unwrap();
        writeln!(
            &mut out,
            "            self.invalidate_cache_field(\"{}\", &{}).await?;",
            primary, primary
        )
        .unwrap();
        writeln!(&mut out, "        }}").unwrap();
    }
    writeln!(&mut out, "        Ok(result.deleted_count)").unwrap();
    writeln!(&mut out, "    }}").unwrap();
    if model.cache {
        writeln!(&mut out).unwrap();
        writeln!(
            &mut out,
            "    fn cache_key(&self, field: &str, value: impl std::fmt::Display) -> String {{"
        )
        .unwrap();
        writeln!(
            &mut out,
            "        format!(\"{}:{{}}:{{}}\", field, value)",
            cache_prefix
        )
        .unwrap();
        writeln!(&mut out, "    }}").unwrap();
        writeln!(&mut out).unwrap();
        writeln!(
            &mut out,
            "    async fn invalidate_model_cache(&self, model: &Model) -> anyhow::Result<()> {{"
        )
        .unwrap();
        writeln!(
            &mut out,
            "        if let Some(cache) = self.ctx.cache.as_ref() {{"
        )
        .unwrap();
        for field in &cache_fields {
            writeln!(
                &mut out,
                "            cache.del(&self.cache_key(\"{}\", &model.{})).await?;",
                field.name, field.name
            )
            .unwrap();
        }
        writeln!(&mut out, "        }}").unwrap();
        writeln!(&mut out, "        Ok(())").unwrap();
        writeln!(&mut out, "    }}").unwrap();
        writeln!(&mut out).unwrap();
        writeln!(
            &mut out,
            "    async fn invalidate_cache_field(&self, field: &str, value: impl std::fmt::Display) -> anyhow::Result<()> {{"
        )
        .unwrap();
        writeln!(
            &mut out,
            "        if let Some(cache) = self.ctx.cache.as_ref() {{"
        )
        .unwrap();
        writeln!(
            &mut out,
            "            let key = self.cache_key(field, value);"
        )
        .unwrap();
        writeln!(&mut out, "            cache.del(&key).await?;").unwrap();
        writeln!(&mut out, "        }}").unwrap();
        writeln!(&mut out, "        Ok(())").unwrap();
        writeln!(&mut out, "    }}").unwrap();
    }
    writeln!(&mut out, "}}").unwrap();

    out
}

fn render_mongo_find_methods(
    out: &mut String,
    model: &ModelSpec,
    field_name: &str,
    field_ty: &str,
    is_primary: bool,
) {
    use std::fmt::Write as _;
    let mongo_name = mongo_field_name(field_name);
    writeln!(
        out,
        "    pub async fn find_by_{}(&self, {}: {}) -> anyhow::Result<Option<Model>> {{",
        field_name, field_name, field_ty
    )
    .unwrap();
    if model.cache {
        writeln!(
            out,
            "        self.cached_find_by_{}({}).await",
            field_name, field_name
        )
        .unwrap();
        writeln!(out, "    }}").unwrap();
        writeln!(out).unwrap();
        writeln!(
            out,
            "    pub async fn find_by_{}_uncached(&self, {}: {}) -> anyhow::Result<Option<Model>> {{",
            field_name, field_name, field_ty
        )
        .unwrap();
    }
    writeln!(
        out,
        "        let filter = Self::filter_by(\"{}\", &{})?;",
        mongo_name, field_name
    )
    .unwrap();
    writeln!(
        out,
        "        Ok(self.collection()?.find_one(filter).await?)"
    )
    .unwrap();
    writeln!(out, "    }}").unwrap();
    writeln!(out).unwrap();
    if model.cache {
        writeln!(
            out,
            "    pub async fn cached_find_by_{}(&self, {}: {}) -> anyhow::Result<Option<Model>> {{",
            field_name, field_name, field_ty
        )
        .unwrap();
        writeln!(
            out,
            "        if let Some(cache) = self.ctx.cache.as_ref() {{"
        )
        .unwrap();
        writeln!(
            out,
            "            let key = self.cache_key(\"{}\", &{});",
            field_name, field_name
        )
        .unwrap();
        writeln!(
            out,
            "            let ttl = Duration::from_secs({});",
            model.cache_ttl_secs.unwrap_or(300)
        )
        .unwrap();
        let negative_cache_ttl_secs = model
            .negative_cache_ttl_secs
            .unwrap_or_else(|| (model.cache_ttl_secs.unwrap_or(300) / 6).clamp(5, 60));
        writeln!(
            out,
            "            let negative_ttl = Duration::from_secs({});",
            negative_cache_ttl_secs
        )
        .unwrap();
        writeln!(out, "            let lookup = {}.clone();", field_name).unwrap();
        writeln!(out, "            return cache").unwrap();
        writeln!(out, "                .get_or_set_json_option(").unwrap();
        writeln!(out, "                    &key,").unwrap();
        writeln!(out, "                    Some(ttl),").unwrap();
        writeln!(out, "                    Some(negative_ttl),").unwrap();
        writeln!(
            out,
            "                    || async move {{ self.find_by_{}_uncached(lookup).await }},",
            field_name
        )
        .unwrap();
        writeln!(out, "                )").unwrap();
        writeln!(out, "                .await;").unwrap();
        writeln!(out, "        }}").unwrap();
        writeln!(out).unwrap();
        writeln!(
            out,
            "        self.find_by_{}_uncached({}).await",
            field_name, field_name
        )
        .unwrap();
        writeln!(out, "    }}").unwrap();
        if is_primary {
            writeln!(out).unwrap();
        }
    }
}

fn mongo_field_name(field_name: &str) -> &str {
    if field_name == "id" {
        "_id"
    } else {
        field_name
    }
}

fn cache_lookup_fields(model: &ModelSpec) -> Vec<&ModelField> {
    let mut seen = HashSet::new();
    model
        .cache_keys
        .iter()
        .filter_map(|key| {
            let field = model
                .fields
                .iter()
                .find(|field| field.name == *key && !is_optional_type(&field.ty))?;
            if seen.insert(field.name.as_str()) {
                Some(field)
            } else {
                None
            }
        })
        .collect()
}

#[derive(Debug, Clone)]
struct InspectedColumn {
    name: String,
    ty: String,
    nullable: bool,
    auto_increment: bool,
    default_value: Option<String>,
    comment: Option<String>,
}

async fn inspect_sqlite_table(
    pool: &sqlx::SqlitePool,
    schema_name: Option<&str>,
    table: &str,
) -> anyhow::Result<ModelSpec> {
    let table_name = strip_sql_identifier(table);
    let pragma_table = table_name.clone();
    let rows = sqlx::query(AssertSqlSafe(format!(
        "PRAGMA table_info({})",
        sqlite_identifier(&pragma_table)
    )))
    .fetch_all(pool)
    .await?;

    let mut columns = Vec::new();
    let mut primary = None;
    for row in rows {
        let name: String = row.try_get("name")?;
        let ty: String = row
            .try_get::<Option<String>, _>("type")?
            .unwrap_or_default();
        let notnull: i64 = row.try_get("notnull")?;
        let default_value: Option<String> = row.try_get("dflt_value")?;
        let pk: i64 = row.try_get("pk")?;
        if pk > 0 {
            if primary.is_some() {
                bail!(
                    "table `{}` has composite primary keys which are not supported",
                    table_name
                );
            }
            primary = Some(name.clone());
        }
        columns.push(InspectedColumn {
            name,
            ty: map_sql_type(&ty, pk > 0),
            nullable: notnull == 0,
            auto_increment: pk > 0 && ty.to_ascii_lowercase().contains("int"),
            default_value,
            comment: None,
        });
    }

    let unique_cache_keys = inspect_sqlite_unique_cache_keys(pool, &pragma_table).await?;
    build_inspected_model(
        schema_name.map(strip_sql_identifier),
        &table_name,
        columns,
        primary,
        unique_cache_keys,
    )
}

async fn inspect_sqlite_unique_cache_keys(
    pool: &sqlx::SqlitePool,
    table: &str,
) -> anyhow::Result<Vec<String>> {
    let rows = sqlx::query(AssertSqlSafe(format!(
        "PRAGMA index_list({})",
        sqlite_identifier(table)
    )))
    .fetch_all(pool)
    .await?;
    let mut keys = Vec::new();
    for row in rows {
        let unique: i64 = row.try_get("unique")?;
        let origin: Option<String> = row.try_get("origin").ok();
        let partial: i64 = row.try_get("partial").unwrap_or(0);
        if unique == 0 || partial != 0 || origin.as_deref() == Some("pk") {
            continue;
        }
        let index_name: String = row.try_get("name")?;
        let columns = sqlx::query(AssertSqlSafe(format!(
            "PRAGMA index_info({})",
            sqlite_identifier(&index_name)
        )))
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(|row| row.try_get::<String, _>("name"))
        .collect::<Result<Vec<_>, _>>()?;
        if columns.len() == 1 {
            keys.push(columns[0].clone());
        }
    }
    Ok(keys)
}

async fn inspect_postgres_table(
    pool: &sqlx::PgPool,
    schema_name: Option<&str>,
    table: &str,
) -> anyhow::Result<ModelSpec> {
    let (schema, table_name) = normalize_table_reference(schema_name, table)?;
    let schema = schema.unwrap_or_else(|| "public".to_string());

    let columns = sqlx::query(
        r#"
        SELECT
            column_name,
            data_type,
            udt_name,
            is_nullable,
            column_default
        FROM information_schema.columns
        WHERE table_schema = $1 AND table_name = $2
        ORDER BY ordinal_position
        "#,
    )
    .bind(&schema)
    .bind(&table_name)
    .fetch_all(pool)
    .await?;

    if columns.is_empty() {
        bail!("table `{schema}.{table_name}` not found");
    }

    let primary_rows = sqlx::query(
        r#"
        SELECT kcu.column_name
        FROM information_schema.table_constraints tc
        JOIN information_schema.key_column_usage kcu
          ON tc.constraint_name = kcu.constraint_name
         AND tc.table_schema = kcu.table_schema
         AND tc.table_name = kcu.table_name
        WHERE tc.constraint_type = 'PRIMARY KEY'
          AND tc.table_schema = $1
          AND tc.table_name = $2
        ORDER BY kcu.ordinal_position
        "#,
    )
    .bind(&schema)
    .bind(&table_name)
    .fetch_all(pool)
    .await?;

    let primary_columns = primary_rows
        .into_iter()
        .map(|row| row.try_get::<String, _>("column_name"))
        .collect::<Result<Vec<_>, _>>()?;
    if primary_columns.len() > 1 {
        bail!("table `{schema}.{table_name}` has composite primary keys which are not supported");
    }
    let primary = primary_columns.first().cloned();
    let unique_cache_keys = inspect_postgres_unique_cache_keys(pool, &schema, &table_name).await?;

    let mut inspected = Vec::new();
    for row in columns {
        let name: String = row.try_get("column_name")?;
        let data_type: String = row.try_get("data_type")?;
        let udt_name: String = row.try_get("udt_name")?;
        let nullable: String = row.try_get("is_nullable")?;
        let default_value: Option<String> = row.try_get("column_default")?;
        let auto_increment = default_value
            .as_deref()
            .map(|value| value.contains("nextval("))
            .unwrap_or(false);
        let ty = map_sql_type(&format!("{data_type} {udt_name}"), auto_increment);
        inspected.push(InspectedColumn {
            name,
            ty,
            nullable: nullable.eq_ignore_ascii_case("yes"),
            auto_increment,
            default_value,
            comment: None,
        });
    }

    build_inspected_model(
        Some(schema),
        &table_name,
        inspected,
        primary,
        unique_cache_keys,
    )
}

async fn inspect_postgres_unique_cache_keys(
    pool: &sqlx::PgPool,
    schema: &str,
    table: &str,
) -> anyhow::Result<Vec<String>> {
    let rows = sqlx::query(
        r#"
        SELECT max(a.attname) AS column_name
        FROM pg_index ix
        JOIN pg_class t ON t.oid = ix.indrelid
        JOIN pg_namespace n ON n.oid = t.relnamespace
        JOIN pg_class i ON i.oid = ix.indexrelid
        JOIN unnest(ix.indkey) WITH ORDINALITY AS keys(attnum, ord) ON true
        JOIN pg_attribute a ON a.attrelid = t.oid AND a.attnum = keys.attnum
        WHERE n.nspname = $1
          AND t.relname = $2
          AND ix.indisunique
          AND NOT ix.indisprimary
          AND ix.indpred IS NULL
        GROUP BY i.relname
        HAVING count(*) = 1
        ORDER BY i.relname
        "#,
    )
    .bind(schema)
    .bind(table)
    .fetch_all(pool)
    .await?;

    rows.into_iter()
        .map(|row| row.try_get::<String, _>("column_name"))
        .collect::<Result<Vec<_>, _>>()
        .map_err(Into::into)
}

async fn inspect_mysql_table(
    pool: &sqlx::MySqlPool,
    schema_name: Option<&str>,
    table: &str,
) -> anyhow::Result<ModelSpec> {
    let (schema, table_name) = normalize_table_reference(schema_name, table)?;
    let schema = match schema {
        Some(schema) => schema,
        None => sqlx::query("SELECT DATABASE() AS db")
            .fetch_one(pool)
            .await?
            .try_get::<Option<String>, _>("db")?
            .ok_or_else(|| anyhow::anyhow!("mysql connection has no default database"))?,
    };

    let columns = sqlx::query(
        r#"
        SELECT
            column_name,
            data_type,
            column_type,
            is_nullable,
            column_default,
            extra,
            column_comment
        FROM information_schema.columns
        WHERE table_schema = ? AND table_name = ?
        ORDER BY ordinal_position
        "#,
    )
    .bind(&schema)
    .bind(&table_name)
    .fetch_all(pool)
    .await?;

    if columns.is_empty() {
        bail!("table `{schema}.{table_name}` not found");
    }

    let primary_rows = sqlx::query(
        r#"
        SELECT column_name
        FROM information_schema.key_column_usage
        WHERE table_schema = ? AND table_name = ? AND constraint_name = 'PRIMARY'
        ORDER BY ordinal_position
        "#,
    )
    .bind(&schema)
    .bind(&table_name)
    .fetch_all(pool)
    .await?;

    let primary_columns = primary_rows
        .into_iter()
        .map(|row| row.try_get::<String, _>("column_name"))
        .collect::<Result<Vec<_>, _>>()?;
    if primary_columns.len() > 1 {
        bail!("table `{schema}.{table_name}` has composite primary keys which are not supported");
    }
    let primary = primary_columns.first().cloned();
    let unique_cache_keys = inspect_mysql_unique_cache_keys(pool, &schema, &table_name).await?;

    let mut inspected = Vec::new();
    for row in columns {
        let name: String = row.try_get("column_name")?;
        let data_type: String = row.try_get("data_type")?;
        let column_type: String = row.try_get("column_type")?;
        let nullable: String = row.try_get("is_nullable")?;
        let default_value: Option<String> = row.try_get("column_default")?;
        let extra: String = row.try_get("extra")?;
        let comment: Option<String> = row.try_get("column_comment")?;
        let auto_increment = extra.to_ascii_lowercase().contains("auto_increment");
        let ty = map_sql_type(&format!("{data_type} {column_type}"), auto_increment);
        inspected.push(InspectedColumn {
            name,
            ty,
            nullable: nullable.eq_ignore_ascii_case("yes"),
            auto_increment,
            default_value,
            comment,
        });
    }

    build_inspected_model(
        Some(schema),
        &table_name,
        inspected,
        primary,
        unique_cache_keys,
    )
}

async fn inspect_mysql_unique_cache_keys(
    pool: &sqlx::MySqlPool,
    schema: &str,
    table: &str,
) -> anyhow::Result<Vec<String>> {
    let rows = sqlx::query(
        r#"
        SELECT index_name, MIN(column_name) AS column_name, COUNT(*) AS column_count
        FROM information_schema.statistics
        WHERE table_schema = ?
          AND table_name = ?
          AND non_unique = 0
          AND index_name <> 'PRIMARY'
        GROUP BY index_name
        HAVING COUNT(*) = 1
        ORDER BY index_name
        "#,
    )
    .bind(schema)
    .bind(table)
    .fetch_all(pool)
    .await?;

    rows.into_iter()
        .map(|row| row.try_get::<String, _>("column_name"))
        .collect::<Result<Vec<_>, _>>()
        .map_err(Into::into)
}

fn build_inspected_model(
    schema_name: Option<String>,
    table: &str,
    columns: Vec<InspectedColumn>,
    primary: Option<String>,
    unique_cache_keys: Vec<String>,
) -> anyhow::Result<ModelSpec> {
    if columns.is_empty() {
        bail!("table `{table}` has no columns");
    }

    let primary = match primary {
        Some(primary) => primary,
        None => columns
            .iter()
            .find(|column| column.auto_increment)
            .map(|column| column.name.clone())
            .ok_or_else(|| anyhow::anyhow!("table `{table}` has no primary key"))?,
    };

    let mut fields = Vec::with_capacity(columns.len());
    for column in columns {
        let ty = if column.nullable && column.name != primary {
            format!("Option<{}>", column.ty)
        } else {
            column.ty
        };
        fields.push(ModelField {
            name: column.name,
            ty,
            auto_increment: column.auto_increment,
            default_value: column.default_value,
            comment: column.comment,
        });
    }

    if !fields.iter().any(|field| field.name == primary) {
        bail!("table `{table}` primary field `{primary}` not found");
    }

    let cache_keys = normalize_cache_keys(&primary, unique_cache_keys, &fields);

    Ok(ModelSpec {
        name: model_name_from_table(table),
        schema_name,
        table: table.to_string(),
        primary: primary.clone(),
        cache: true,
        cache_ttl_secs: None,
        negative_cache_ttl_secs: None,
        cache_keys,
        cache_prefix: None,
        soft_delete: infer_soft_delete_field(&fields),
        tenant: infer_tenant_field(&fields),
        fields,
    })
}

fn normalize_cache_keys(
    primary: &str,
    candidates: Vec<String>,
    fields: &[ModelField],
) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut keys = Vec::new();
    for key in std::iter::once(primary.to_string()).chain(candidates) {
        if !seen.insert(key.clone()) {
            continue;
        }
        let Some(field) = fields.iter().find(|field| field.name == key) else {
            continue;
        };
        if is_optional_type(&field.ty) {
            continue;
        }
        keys.push(key);
    }
    keys
}

fn validate_model_field(
    model_name: &str,
    directive: &str,
    field_name: &str,
    fields: &[ModelField],
) -> anyhow::Result<()> {
    if fields.iter().any(|field| field.name == field_name) {
        Ok(())
    } else {
        bail!("model `{model_name}` {directive} field `{field_name}` not found in fields")
    }
}

fn validate_soft_delete_field(
    model_name: &str,
    field_name: &str,
    fields: &[ModelField],
) -> anyhow::Result<()> {
    let Some(field) = fields.iter().find(|field| field.name == field_name) else {
        bail!("model `{model_name}` soft_delete field `{field_name}` not found in fields")
    };
    if matches!(
        field.ty.as_str(),
        "bool" | "Option<String>" | "Option<i64>" | "Option<u64>"
    ) {
        Ok(())
    } else {
        bail!(
            "model `{model_name}` soft_delete field `{field_name}` must be bool, Option<String>, Option<i64>, or Option<u64>"
        )
    }
}

fn validate_tenant_field(
    model_name: &str,
    field_name: &str,
    fields: &[ModelField],
) -> anyhow::Result<()> {
    let Some(field) = fields.iter().find(|field| field.name == field_name) else {
        bail!("model `{model_name}` tenant field `{field_name}` not found in fields")
    };
    if is_optional_type(&field.ty) {
        bail!("model `{model_name}` tenant field `{field_name}` cannot be optional")
    }
    Ok(())
}

fn infer_soft_delete_field(fields: &[ModelField]) -> Option<String> {
    for candidate in ["deleted", "is_deleted"] {
        if fields
            .iter()
            .any(|field| field.name == candidate && field.ty == "bool")
        {
            return Some(candidate.to_string());
        }
    }

    for candidate in ["deleted_at", "delete_time", "deleted_at_millis"] {
        if fields.iter().any(|field| {
            field.name == candidate
                && matches!(
                    field.ty.as_str(),
                    "Option<String>" | "Option<i64>" | "Option<u64>"
                )
        }) {
            return Some(candidate.to_string());
        }
    }

    None
}

fn infer_tenant_field(fields: &[ModelField]) -> Option<String> {
    ["tenant_id", "org_id", "account_id"]
        .into_iter()
        .find(|candidate| {
            fields
                .iter()
                .any(|field| field.name == *candidate && !is_optional_type(&field.ty))
        })
        .map(ToOwned::to_owned)
}

fn is_optional_type(ty: &str) -> bool {
    ty.trim_start().starts_with("Option<")
}

fn is_numeric_type(ty: &str) -> bool {
    matches!(
        ty,
        "i8" | "i16"
            | "i32"
            | "i64"
            | "i128"
            | "isize"
            | "u8"
            | "u16"
            | "u32"
            | "u64"
            | "u128"
            | "usize"
            | "f32"
            | "f64"
    )
}

fn normalize_table_reference(
    schema_name: Option<&str>,
    table: &str,
) -> anyhow::Result<(Option<String>, String)> {
    let trimmed = strip_sql_identifier(table);
    let parsed = split_table_reference(&trimmed);
    match (schema_name.map(strip_sql_identifier), parsed.0, parsed.1) {
        (Some(expected), Some(actual), table) => {
            if !expected.eq_ignore_ascii_case(&actual) {
                bail!(
                    "schema mismatch: `--schema {}` does not match table `{}`",
                    expected,
                    actual
                );
            }
            Ok((Some(actual), table))
        }
        (Some(schema), None, table) => Ok((Some(schema), table)),
        (None, Some(schema), table) => Ok((Some(schema), table)),
        (None, None, table) => Ok((None, table)),
    }
}

fn split_table_reference(input: &str) -> (Option<String>, String) {
    let trimmed = strip_sql_identifier(input);
    let mut parts = trimmed
        .split('.')
        .map(strip_sql_identifier)
        .filter(|part| !part.is_empty());
    let first = parts.next();
    let second = parts.next();
    match (first, second, parts.next()) {
        (Some(table), None, None) => (None, table),
        (Some(schema), Some(table), None) => (Some(schema), table),
        _ => (None, trimmed),
    }
}

fn table_key(schema_name: Option<&str>, table: &str) -> String {
    match schema_name {
        Some(schema) if !schema.is_empty() => format!(
            "{}.{}",
            strip_sql_identifier(schema),
            strip_sql_identifier(table)
        ),
        _ => strip_sql_identifier(table),
    }
}

fn sqlite_identifier(input: &str) -> String {
    let cleaned = strip_sql_identifier(input);
    format!("\"{}\"", cleaned.replace('\"', "\"\""))
}

#[allow(dead_code)]
pub fn parse_models(source: &str) -> anyhow::Result<Vec<ModelSpec>> {
    parse_models_with_format(source, ModelFormat::Auto)
}

pub fn parse_models_with_format(
    source: &str,
    format: ModelFormat,
) -> anyhow::Result<Vec<ModelSpec>> {
    match format {
        ModelFormat::Auto => {
            if looks_like_sql(source) {
                parse_sql_models(source)
            } else {
                parse_dsl_models(source)
            }
        }
        ModelFormat::Dsl => parse_dsl_models(source),
        ModelFormat::Sql => parse_sql_models(source),
        ModelFormat::Mongo => parse_dsl_models(source),
    }
}

fn parse_dsl_models(source: &str) -> anyhow::Result<Vec<ModelSpec>> {
    let mut models = Vec::new();
    let lines = source.lines().enumerate().collect::<Vec<_>>();
    let mut i = 0usize;

    while i < lines.len() {
        let (line_no, raw) = lines[i];
        i += 1;
        let line = strip_comment(raw).trim();
        if line.is_empty() {
            continue;
        }
        if !line.starts_with("model ") {
            bail!("line {}: expected `model Name {{`", line_no + 1);
        }
        if !line.ends_with('{') {
            bail!("line {}: expected `model Name {{`", line_no + 1);
        }

        let name = line
            .trim_start_matches("model ")
            .trim_end_matches('{')
            .trim()
            .to_string();
        if name.is_empty() {
            bail!("line {}: model name is required", line_no + 1);
        }

        let mut table = to_snake_case(&name);
        let mut primary = None;
        let mut cache = false;
        let mut cache_ttl_secs = None;
        let mut negative_cache_ttl_secs = None;
        let mut cache_keys = Vec::new();
        let mut cache_prefix = None;
        let mut soft_delete = None;
        let mut tenant = None;
        let mut fields = Vec::new();

        while i < lines.len() {
            let (inner_line_no, raw) = lines[i];
            i += 1;
            let inner = strip_comment(raw).trim();
            if inner.is_empty() {
                continue;
            }
            if inner == "}" {
                break;
            }

            if let Some(value) = inner.strip_prefix("table:") {
                table = value.trim().to_string();
                continue;
            }
            if let Some(value) = inner.strip_prefix("primary:") {
                primary = Some(value.trim().to_string());
                continue;
            }
            if let Some(value) = inner.strip_prefix("cache:") {
                cache = parse_bool(value.trim(), inner_line_no + 1)?;
                continue;
            }
            if let Some(value) = inner.strip_prefix("cache_ttl_secs:") {
                cache_ttl_secs = Some(parse_u64(value.trim(), inner_line_no + 1)?);
                continue;
            }
            if let Some(value) = inner.strip_prefix("negative_cache_ttl_secs:") {
                negative_cache_ttl_secs = Some(parse_u64(value.trim(), inner_line_no + 1)?);
                continue;
            }
            if let Some(value) = inner.strip_prefix("cache_key:") {
                cache_keys = value
                    .split(',')
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(ToOwned::to_owned)
                    .collect();
                continue;
            }
            if let Some(value) = inner.strip_prefix("cache_prefix:") {
                cache_prefix = Some(value.trim().trim_matches('"').to_string());
                continue;
            }
            if let Some(value) = inner.strip_prefix("soft_delete:") {
                soft_delete = Some(value.trim().to_string());
                continue;
            }
            if let Some(value) = inner.strip_prefix("tenant:") {
                tenant = Some(value.trim().to_string());
                continue;
            }
            if let Some(value) = inner.strip_prefix("field ") {
                let mut parts = value.split_whitespace();
                let field_name = parts.next().ok_or_else(|| {
                    anyhow::anyhow!("line {}: expected field name", inner_line_no + 1)
                })?;
                let field_ty = parts.next().ok_or_else(|| {
                    anyhow::anyhow!("line {}: expected field type", inner_line_no + 1)
                })?;
                fields.push(ModelField {
                    name: field_name.to_string(),
                    ty: field_ty.to_string(),
                    auto_increment: false,
                    default_value: None,
                    comment: None,
                });
                continue;
            }

            bail!(
                "line {}: expected `table:`, `primary:`, `cache:`, `cache_key:`, `cache_prefix:`, `cache_ttl_secs:`, `negative_cache_ttl_secs:`, `soft_delete:`, `tenant:` or `field`",
                inner_line_no + 1
            );
        }

        if fields.is_empty() {
            bail!("model `{name}` must declare at least one field");
        }

        let primary = primary.unwrap_or_else(|| fields[0].name.clone());
        if !fields.iter().any(|field| field.name == primary) {
            bail!("model `{name}` primary field `{primary}` not found in fields");
        }
        if cache_keys.is_empty() {
            cache_keys.push(primary.clone());
        }
        for key in &cache_keys {
            let Some(field) = fields.iter().find(|field| &field.name == key) else {
                bail!("model `{name}` cache key field `{key}` not found in fields");
            };
            if is_optional_type(&field.ty) {
                bail!("model `{name}` cache key field `{key}` cannot be optional");
            }
        }
        let soft_delete = match soft_delete {
            Some(field) => {
                validate_model_field(&name, "soft_delete", &field, &fields)?;
                validate_soft_delete_field(&name, &field, &fields)?;
                Some(field)
            }
            None => infer_soft_delete_field(&fields),
        };
        let tenant = match tenant {
            Some(field) => {
                validate_model_field(&name, "tenant", &field, &fields)?;
                validate_tenant_field(&name, &field, &fields)?;
                Some(field)
            }
            None => infer_tenant_field(&fields),
        };

        models.push(ModelSpec {
            name,
            schema_name: None,
            table,
            primary,
            soft_delete,
            tenant,
            cache,
            cache_ttl_secs,
            negative_cache_ttl_secs,
            cache_keys,
            cache_prefix,
            fields,
        });
    }

    if models.is_empty() {
        bail!("no model declarations found");
    }

    Ok(models)
}

fn parse_sql_models(source: &str) -> anyhow::Result<Vec<ModelSpec>> {
    let mut models = Vec::new();
    let mut model_indexes = HashMap::<String, usize>::new();
    let mut pending_comments = Vec::<(String, String, String)>::new();
    let lines = source.lines().enumerate().collect::<Vec<_>>();
    let mut i = 0usize;

    while i < lines.len() {
        let (line_no, raw) = lines[i];
        let line = raw.trim();
        i += 1;

        if line.is_empty() || is_sql_comment(line) {
            continue;
        }

        let statement_kind = if starts_with_ci(line, "create table") {
            Some(StatementKind::CreateTable)
        } else if starts_with_ci(line, "comment on column") {
            Some(StatementKind::CommentOnColumn)
        } else {
            None
        }
        .ok_or_else(|| {
            anyhow::anyhow!(
                "line {}: expected `CREATE TABLE` or `COMMENT ON COLUMN`",
                line_no + 1
            )
        })?;

        let (statement, next_i) = collect_sql_statement(&lines, i - 1, line_no + 1)?;
        i = next_i;

        match statement_kind {
            StatementKind::CreateTable => {
                let model = parse_create_table(&statement, line_no + 1)?;
                let key = table_key(model.schema_name.as_deref(), &model.table);
                model_indexes.insert(key, models.len());
                models.push(model);
            }
            StatementKind::CommentOnColumn => {
                let (table, column, comment) = parse_comment_on_column(&statement, line_no + 1)?;
                pending_comments.push((table, column, comment));
            }
        }
    }

    if models.is_empty() {
        bail!("no model declarations found");
    }

    for (table, column, comment) in pending_comments {
        let index = model_indexes
            .get(&table)
            .copied()
            .ok_or_else(|| anyhow::anyhow!("unknown table `{table}` for column comment"))?;
        let model = models.get_mut(index).expect("model index valid");
        let field = model
            .fields
            .iter_mut()
            .find(|field| field.name == column)
            .ok_or_else(|| anyhow::anyhow!("unknown column `{column}` for table `{table}`"))?;
        field.comment = Some(comment);
    }

    Ok(models)
}

#[derive(Debug, Clone, Copy)]
enum StatementKind {
    CreateTable,
    CommentOnColumn,
}

fn collect_sql_statement(
    lines: &[(usize, &str)],
    start_idx: usize,
    start_line: usize,
) -> anyhow::Result<(String, usize)> {
    let mut statement = String::new();
    let mut depth = 0i32;
    let mut finished = false;
    let mut i = start_idx;

    while i < lines.len() {
        let (_, raw) = lines[i];
        let trimmed = raw.trim_end();
        if !statement.is_empty() {
            statement.push('\n');
        }
        statement.push_str(trimmed);
        depth += paren_balance(trimmed);
        finished = trimmed.trim_end().ends_with(';') && depth <= 0;
        i += 1;
        if finished {
            break;
        }
    }

    if !finished {
        bail!("line {}: unterminated SQL statement", start_line);
    }

    Ok((statement, i))
}

fn parse_create_table(statement: &str, start_line: usize) -> anyhow::Result<ModelSpec> {
    let statement = statement.trim().trim_end_matches(';').trim();
    let open = statement
        .find('(')
        .ok_or_else(|| anyhow::anyhow!("line {}: expected `(` after table name", start_line))?;
    let close = find_matching_paren(statement, open).ok_or_else(|| {
        anyhow::anyhow!(
            "line {}: expected closing `)` for table definition",
            start_line
        )
    })?;

    let header = statement[..open].trim();
    let body = &statement[open + 1..close];
    let (schema_name, table) = parse_table_reference(header, start_line)?;
    let mut fields = Vec::new();
    let mut primary = None;
    let mut inline_primary_fields = Vec::new();
    let mut unique_cache_keys = Vec::new();

    for entry in split_sql_items(body) {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }

        if starts_with_ci(entry, "primary key") {
            let columns = parse_key_columns(entry, start_line)?;
            if columns.len() != 1 {
                bail!(
                    "line {}: composite primary keys are not supported",
                    start_line
                );
            }
            primary = columns.into_iter().next();
            continue;
        }

        if is_unique_key_entry(entry) {
            let columns = parse_key_columns(entry, start_line)?;
            if columns.len() == 1 {
                unique_cache_keys.push(columns[0].clone());
            }
            continue;
        }

        if starts_with_ci(entry, "key ") || starts_with_ci(entry, "index ") {
            continue;
        }

        if starts_with_ci(entry, "constraint ")
            && (entry.to_ascii_lowercase().contains("foreign key")
                || entry.to_ascii_lowercase().contains("references"))
        {
            bail!("line {}: foreign keys are not supported", start_line);
        }

        if starts_with_ci(entry, "foreign key") || entry.to_ascii_lowercase().contains("references")
        {
            bail!("line {}: foreign keys are not supported", start_line);
        }

        fields.push(parse_sql_field(entry, start_line)?);
        let last = fields.last().expect("field pushed");
        if last.inline_primary_key {
            inline_primary_fields.push(last.name.clone());
        }
    }

    if fields.is_empty() {
        bail!(
            "line {}: CREATE TABLE must declare at least one column",
            start_line
        );
    }

    let primary = match primary {
        Some(primary) => primary,
        None if inline_primary_fields.len() == 1 => inline_primary_fields[0].clone(),
        None if inline_primary_fields.len() > 1 => {
            bail!(
                "line {}: composite primary keys are not supported",
                start_line
            )
        }
        None => fields
            .iter()
            .find(|field| field.auto_increment)
            .map(|field| field.name.clone())
            .ok_or_else(|| anyhow::anyhow!("line {}: missing PRIMARY KEY", start_line))?,
    };

    if !fields.iter().any(|field| field.name == primary) {
        bail!(
            "line {}: primary field `{}` not found in columns",
            start_line,
            primary
        );
    }

    let primary_name = primary.clone();
    let fields = fields
        .into_iter()
        .map(|field| {
            let ty = if field.nullable && field.name != primary_name {
                format!("Option<{}>", field.ty)
            } else {
                field.ty
            };
            ModelField {
                name: field.name,
                ty,
                auto_increment: field.auto_increment,
                default_value: field.default_value,
                comment: field.comment,
            }
        })
        .collect::<Vec<_>>();
    let cache_keys = normalize_cache_keys(&primary, unique_cache_keys, &fields);
    let name = model_name_from_table(&table);
    Ok(ModelSpec {
        name,
        schema_name,
        table,
        primary: primary.clone(),
        soft_delete: infer_soft_delete_field(&fields),
        tenant: infer_tenant_field(&fields),
        cache: true,
        cache_ttl_secs: None,
        negative_cache_ttl_secs: None,
        cache_keys,
        cache_prefix: None,
        fields,
    })
}

fn parse_comment_on_column(
    statement: &str,
    start_line: usize,
) -> anyhow::Result<(String, String, String)> {
    let statement = statement.trim().trim_end_matches(';').trim();
    let lower = statement.to_ascii_lowercase();
    let prefix = "comment on column ";
    if !lower.starts_with(prefix) {
        bail!(
            "line {}: expected `COMMENT ON COLUMN ... IS ...`",
            start_line
        );
    }

    let rest = statement[prefix.len()..].trim();
    let (target, value) = split_once_ci(rest, " is ").ok_or_else(|| {
        anyhow::anyhow!("line {}: expected `IS` in COMMENT ON COLUMN", start_line)
    })?;
    let (schema_name, table_name, column) = parse_column_reference(target.trim(), start_line)?;
    let table = table_key(schema_name.as_deref(), &table_name);
    if table.is_empty() {
        bail!(
            "line {}: missing table name in COMMENT ON COLUMN",
            start_line
        );
    }

    Ok((table, column, unquote_sql_string(value.trim())))
}

#[derive(Debug, Clone)]
struct ParsedSqlField {
    name: String,
    ty: String,
    nullable: bool,
    auto_increment: bool,
    inline_primary_key: bool,
    default_value: Option<String>,
    comment: Option<String>,
}

fn parse_sql_field(entry: &str, start_line: usize) -> anyhow::Result<ParsedSqlField> {
    let entry = entry.trim().trim_end_matches(',');
    let (name, rest) = split_identifier(entry)
        .ok_or_else(|| anyhow::anyhow!("line {}: expected column definition", start_line))?;
    let tokens = tokenize_sql(&rest);
    if tokens.is_empty() {
        bail!("line {}: expected column type for `{}`", start_line, name);
    }

    let mut type_tokens = Vec::new();
    let mut i = 0usize;
    while i < tokens.len() {
        let token = tokens[i].clone();
        if is_sql_attr_keyword(token.as_str()) {
            break;
        }
        type_tokens.push(token);
        i += 1;
    }
    if type_tokens.is_empty() {
        bail!("line {}: expected column type for `{}`", start_line, name);
    }

    let raw_ty = type_tokens.join(" ");
    let attrs = tokens[i..].to_vec();
    let sql_attrs = parse_sql_attrs(&attrs);
    let auto_increment = sql_attrs.auto_increment || raw_ty.to_ascii_lowercase().contains("serial");
    let ty = map_sql_type(&raw_ty, auto_increment);
    let nullable = if auto_increment {
        false
    } else {
        sql_attrs.nullable
    };
    Ok(ParsedSqlField {
        name: strip_sql_identifier(&name),
        ty,
        nullable,
        auto_increment,
        inline_primary_key: sql_attrs.inline_primary_key,
        default_value: sql_attrs.default_value,
        comment: sql_attrs.comment,
    })
}

#[derive(Debug, Clone, Default)]
struct SqlAttrs {
    nullable: bool,
    auto_increment: bool,
    inline_primary_key: bool,
    default_value: Option<String>,
    comment: Option<String>,
}

fn parse_sql_attrs(tokens: &[String]) -> SqlAttrs {
    let mut attrs = SqlAttrs {
        nullable: true,
        ..SqlAttrs::default()
    };
    let mut i = 0usize;

    while i < tokens.len() {
        let token = tokens[i].as_str();
        match token.to_ascii_lowercase().as_str() {
            "not"
                if tokens
                    .get(i + 1)
                    .is_some_and(|next| next.eq_ignore_ascii_case("null")) =>
            {
                attrs.nullable = false;
                i += 2;
            }
            "null" => {
                attrs.nullable = true;
                i += 1;
            }
            "auto_increment" | "identity" => {
                attrs.auto_increment = true;
                i += 1;
            }
            "primary"
                if tokens
                    .get(i + 1)
                    .is_some_and(|next| next.eq_ignore_ascii_case("key")) =>
            {
                attrs.inline_primary_key = true;
                i += 2;
            }
            "default" => {
                let (value, consumed) = collect_sql_clause(tokens, i + 1);
                if !value.is_empty() {
                    attrs.default_value = Some(value);
                }
                i += 1 + consumed;
            }
            "comment" => {
                let (value, consumed) = collect_sql_clause(tokens, i + 1);
                if !value.is_empty() {
                    attrs.comment = Some(unquote_sql_string(&value));
                }
                i += 1 + consumed;
            }
            "=" => {
                i += 1;
            }
            "serial" | "bigserial" | "smallserial" => {
                attrs.auto_increment = true;
                i += 1;
            }
            _ => {
                i += 1;
            }
        }
    }

    attrs
}

fn collect_sql_clause(tokens: &[String], start: usize) -> (String, usize) {
    let mut parts = Vec::new();
    let mut consumed = 0usize;
    let mut depth = 0i32;

    for token in tokens.iter().skip(start) {
        let lower = token.to_ascii_lowercase();
        if !parts.is_empty() && depth == 0 && is_sql_attr_keyword(&lower) {
            break;
        }
        depth += token.chars().filter(|ch| *ch == '(').count() as i32;
        depth -= token.chars().filter(|ch| *ch == ')').count() as i32;
        parts.push(token.clone());
        consumed += 1;
    }

    (parts.join(" ").trim().to_string(), consumed)
}

fn unquote_sql_string(input: &str) -> String {
    let trimmed = input.trim();
    if let Some(value) = trimmed
        .strip_prefix('\'')
        .and_then(|value| value.strip_suffix('\''))
    {
        return value.replace("''", "'");
    }
    if let Some(value) = trimmed
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
    {
        return value.replace("\"\"", "\"");
    }
    trimmed.to_string()
}

fn parse_key_columns(entry: &str, start_line: usize) -> anyhow::Result<Vec<String>> {
    let open = entry
        .find('(')
        .ok_or_else(|| anyhow::anyhow!("line {}: expected `(` in PRIMARY KEY", start_line))?;
    let close = find_matching_paren(entry, open)
        .ok_or_else(|| anyhow::anyhow!("line {}: expected `)` in PRIMARY KEY", start_line))?;
    Ok(split_sql_items(&entry[open + 1..close])
        .into_iter()
        .map(|item| strip_sql_identifier(item.trim()))
        .filter(|item| !item.is_empty())
        .collect())
}

fn parse_table_reference(
    header: &str,
    start_line: usize,
) -> anyhow::Result<(Option<String>, String)> {
    let header = header.trim().trim_end_matches('(').trim();
    let lower = header.to_ascii_lowercase();
    if !lower.starts_with("create table") {
        bail!("line {}: expected `CREATE TABLE`", start_line);
    }

    let rest = header["create table".len()..].trim();
    let rest = if starts_with_ci(rest, "if not exists") {
        rest["if not exists".len()..].trim()
    } else {
        rest
    };
    if rest.is_empty() {
        bail!("line {}: missing table name", start_line);
    }
    let table = rest.split_whitespace().next().unwrap_or_default();
    let (schema_name, table) = split_schema_table_name(table);
    if table.is_empty() {
        bail!("line {}: missing table name", start_line);
    }
    Ok((schema_name, table))
}

fn parse_column_reference(
    reference: &str,
    start_line: usize,
) -> anyhow::Result<(Option<String>, String, String)> {
    let mut parts = reference
        .split('.')
        .map(strip_sql_identifier)
        .filter(|part| !part.is_empty());
    let column = parts.next_back().ok_or_else(|| {
        anyhow::anyhow!(
            "line {}: missing column name in COMMENT ON COLUMN",
            start_line
        )
    })?;
    let table = parts.next_back().ok_or_else(|| {
        anyhow::anyhow!(
            "line {}: missing table name in COMMENT ON COLUMN",
            start_line
        )
    })?;
    let schema_name = parts.next_back();
    Ok((schema_name, table, column))
}

fn split_sql_items(body: &str) -> Vec<String> {
    let mut items = Vec::new();
    let mut current = String::new();
    let mut depth = 0i32;
    let mut in_single = false;
    let mut in_double = false;
    let mut in_backtick = false;
    let chars = body.chars().peekable();

    for ch in chars {
        match ch {
            '\'' if !in_double && !in_backtick => {
                in_single = !in_single;
                current.push(ch);
            }
            '"' if !in_single && !in_backtick => {
                in_double = !in_double;
                current.push(ch);
            }
            '`' if !in_single && !in_double => {
                in_backtick = !in_backtick;
                current.push(ch);
            }
            '(' if !in_single && !in_double && !in_backtick => {
                depth += 1;
                current.push(ch);
            }
            ')' if !in_single && !in_double && !in_backtick => {
                depth -= 1;
                current.push(ch);
            }
            ',' if depth == 0 && !in_single && !in_double && !in_backtick => {
                let item = current.trim();
                if !item.is_empty() {
                    items.push(item.to_string());
                }
                current.clear();
            }
            _ => current.push(ch),
        }
    }

    let item = current.trim();
    if !item.is_empty() {
        items.push(item.to_string());
    }

    items
}

fn find_matching_paren(input: &str, open: usize) -> Option<usize> {
    let mut depth = 0i32;
    let mut in_single = false;
    let mut in_double = false;
    let mut in_backtick = false;
    for (idx, ch) in input.char_indices().skip(open) {
        match ch {
            '\'' if !in_double && !in_backtick => in_single = !in_single,
            '"' if !in_single && !in_backtick => in_double = !in_double,
            '`' if !in_single && !in_double => in_backtick = !in_backtick,
            '(' if !in_single && !in_double && !in_backtick => depth += 1,
            ')' if !in_single && !in_double && !in_backtick => {
                depth -= 1;
                if depth == 0 {
                    return Some(idx);
                }
            }
            _ => {}
        }
    }
    None
}

fn tokenize_sql(input: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_single = false;
    let mut in_double = false;
    let mut in_backtick = false;

    for ch in input.chars() {
        match ch {
            '\'' if !in_double && !in_backtick => {
                in_single = !in_single;
                current.push(ch);
            }
            '"' if !in_single && !in_backtick => {
                in_double = !in_double;
                current.push(ch);
            }
            '`' if !in_single && !in_double => {
                in_backtick = !in_backtick;
                current.push(ch);
            }
            c if c.is_whitespace() && !in_single && !in_double && !in_backtick => {
                if !current.is_empty() {
                    tokens.push(current.clone());
                    current.clear();
                }
            }
            _ => current.push(ch),
        }
    }

    if !current.is_empty() {
        tokens.push(current);
    }

    tokens
}

fn split_identifier(input: &str) -> Option<(String, String)> {
    let trimmed = input.trim_start();
    if trimmed.is_empty() {
        return None;
    }

    if let Some(rest) = trimmed.strip_prefix('`') {
        let end = rest.find('`')?;
        let name = rest[..end].to_string();
        let remainder = rest[end + 1..].trim_start().to_string();
        return Some((name, remainder));
    }

    let mut split = trimmed.splitn(2, char::is_whitespace);
    let name = split.next()?.to_string();
    let remainder = split.next().unwrap_or_default().trim_start().to_string();
    Some((name, remainder))
}

fn strip_sql_identifier(input: &str) -> String {
    let input = input.trim().trim_end_matches(',').trim();
    for (open, close) in [('`', '`'), ('"', '"'), ('[', ']')] {
        if input.starts_with(open) && input.ends_with(close) && input.len() >= 2 {
            return input[1..input.len() - 1].to_string();
        }
    }
    input.to_string()
}

fn split_schema_table_name(input: &str) -> (Option<String>, String) {
    let cleaned = strip_sql_identifier(input);
    let mut parts = cleaned
        .split('.')
        .map(strip_sql_identifier)
        .filter(|part| !part.is_empty());
    let first = parts.next();
    let second = parts.next();
    match (first, second, parts.next()) {
        (Some(table), None, None) => (None, table),
        (Some(schema), Some(table), None) => (Some(schema), table),
        _ => (None, cleaned),
    }
}

fn model_name_from_table(table: &str) -> String {
    let table = strip_sql_identifier(table);
    let table = table.rsplit('.').next().unwrap_or(&table);
    to_pascal_case(&singularize_identifier(table))
}

fn singularize_identifier(input: &str) -> String {
    let lower = input.to_ascii_lowercase();
    if lower.ends_with("ies") && input.len() > 3 {
        let mut stem = input[..input.len() - 3].to_string();
        stem.push('y');
        return stem;
    }
    if lower.ends_with("ches")
        || lower.ends_with("shes")
        || lower.ends_with("xes")
        || lower.ends_with("zes")
        || lower.ends_with("sses")
    {
        return input[..input.len() - 2].to_string();
    }
    if lower.ends_with('s') && !lower.ends_with("ss") && input.len() > 1 {
        return input[..input.len() - 1].to_string();
    }
    input.to_string()
}

fn starts_with_ci(input: &str, prefix: &str) -> bool {
    input
        .get(..prefix.len())
        .map(|head| head.eq_ignore_ascii_case(prefix))
        .unwrap_or(false)
}

fn is_unique_key_entry(entry: &str) -> bool {
    starts_with_ci(entry, "unique key")
        || starts_with_ci(entry, "unique index")
        || (starts_with_ci(entry, "constraint ") && entry.to_ascii_lowercase().contains(" unique"))
}

fn split_once_ci<'a>(input: &'a str, needle: &str) -> Option<(&'a str, &'a str)> {
    let lower = input.to_ascii_lowercase();
    let idx = lower.find(&needle.to_ascii_lowercase())?;
    Some((&input[..idx], &input[idx + needle.len()..]))
}

fn is_sql_comment(input: &str) -> bool {
    let trimmed = input.trim_start();
    trimmed.starts_with("--") || trimmed.starts_with('#')
}

fn paren_balance(input: &str) -> i32 {
    let mut depth = 0i32;
    let mut in_single = false;
    let mut in_double = false;
    let mut in_backtick = false;
    for ch in input.chars() {
        match ch {
            '\'' if !in_double && !in_backtick => in_single = !in_single,
            '"' if !in_single && !in_backtick => in_double = !in_double,
            '`' if !in_single && !in_double => in_backtick = !in_backtick,
            '(' if !in_single && !in_double && !in_backtick => depth += 1,
            ')' if !in_single && !in_double && !in_backtick => depth -= 1,
            _ => {}
        }
    }
    depth
}

fn looks_like_sql(source: &str) -> bool {
    source
        .lines()
        .map(str::trim_start)
        .find(|line| !line.is_empty() && !is_sql_comment(line))
        .map(|line| starts_with_ci(line, "create table"))
        .unwrap_or(false)
}

fn is_sql_attr_keyword(token: &str) -> bool {
    matches!(
        token.to_ascii_lowercase().as_str(),
        "not"
            | "null"
            | "default"
            | "comment"
            | "auto_increment"
            | "primary"
            | "key"
            | "unique"
            | "index"
            | "constraint"
            | "references"
            | "on"
            | "unsigned"
            | "collate"
            | "character"
            | "charset"
            | "check"
    )
}

fn map_sql_type(raw_ty: &str, auto_increment: bool) -> String {
    let normalized = raw_ty.to_ascii_lowercase();
    if normalized.contains("tinyint(1)") || normalized == "bool" || normalized == "boolean" {
        return "bool".to_string();
    }

    if normalized.contains("bigserial") || normalized.contains("bigint") {
        return if normalized.contains("unsigned") || auto_increment {
            "u64".to_string()
        } else {
            "i64".to_string()
        };
    }

    if normalized.contains("serial") {
        return "i64".to_string();
    }

    if normalized.contains("int")
        || normalized.contains("mediumint")
        || normalized.contains("smallint")
        || normalized.contains("tinyint")
        || normalized.contains("integer")
    {
        return if normalized.contains("unsigned") {
            "u64".to_string()
        } else {
            "i64".to_string()
        };
    }

    if normalized.contains("double")
        || normalized.contains("decimal")
        || normalized.contains("numeric")
        || normalized.contains("float")
        || normalized.contains("real")
        || normalized.contains("money")
    {
        return "f64".to_string();
    }

    if normalized.contains("blob") || normalized.contains("binary") || normalized.contains("bytea")
    {
        return "Vec<u8>".to_string();
    }

    if normalized.contains("json")
        || normalized.contains("jsonb")
        || normalized.contains("char")
        || normalized.contains("text")
        || normalized.contains("enum")
        || normalized.contains("set")
        || normalized.contains("date")
        || normalized.contains("time")
        || normalized.contains("timestamp")
        || normalized.contains("timestamptz")
        || normalized.contains("uuid")
        || normalized.contains("cidr")
        || normalized.contains("inet")
        || normalized.contains("macaddr")
    {
        return "String".to_string();
    }

    if normalized.contains("bit") {
        return "bool".to_string();
    }

    "String".to_string()
}

fn strip_comment(line: &str) -> &str {
    let hash = line.find('#');
    let slash = line.find("//");
    match (hash, slash) {
        (Some(a), Some(b)) => line[..a.min(b)].trim_end(),
        (Some(a), None) => line[..a].trim_end(),
        (None, Some(b)) => line[..b].trim_end(),
        (None, None) => line,
    }
}

fn parse_bool(input: &str, line_no: usize) -> anyhow::Result<bool> {
    match input {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => bail!("line {}: expected `true` or `false`", line_no),
    }
}

fn parse_u64(input: &str, line_no: usize) -> anyhow::Result<u64> {
    input
        .parse::<u64>()
        .with_context(|| format!("line {}: expected a positive integer", line_no))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generator::DependencySource;
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    #[test]
    fn parses_model_blocks() {
        let source = r#"
        model User {
            table: users
            primary: id
            cache: true
            cache_key: id
            cache_prefix: account
            cache_ttl_secs: 300
            negative_cache_ttl_secs: 30
            soft_delete: deleted
            tenant: tenant_id
            field id i64
            field tenant_id String
            field name String
            field deleted bool
        }
        "#;

        let models = parse_models(source).expect("parse");
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].name, "User");
        assert_eq!(models[0].table, "users");
        assert_eq!(models[0].primary, "id");
        assert!(models[0].cache);
        assert_eq!(models[0].cache_keys, vec!["id"]);
        assert_eq!(models[0].cache_prefix.as_deref(), Some("account"));
        assert_eq!(models[0].negative_cache_ttl_secs, Some(30));
        assert_eq!(models[0].soft_delete.as_deref(), Some("deleted"));
        assert_eq!(models[0].tenant.as_deref(), Some("tenant_id"));
    }

    #[test]
    fn parses_sql_model_blocks() {
        let source = r#"
        CREATE TABLE `users` (
            `id` bigint unsigned NOT NULL AUTO_INCREMENT PRIMARY KEY,
            `tenant_id` varchar(64) NOT NULL,
            `name` varchar(255) NOT NULL,
            `nickname` varchar(255) NULL DEFAULT 'guest' COMMENT 'Nickname',
            `deleted` tinyint(1) NOT NULL DEFAULT 0,
            `created_at` timestamp NOT NULL DEFAULT CURRENT_TIMESTAMP,
            UNIQUE KEY `uniq_users_name` (`name`),
            UNIQUE KEY `uniq_users_name_created_at` (`name`, `created_at`)
        );
        COMMENT ON COLUMN users.nickname IS 'nickname from profile';
        "#;

        let models = parse_models_with_format(source, ModelFormat::Sql).expect("parse");
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].name, "User");
        assert_eq!(models[0].table, "users");
        assert_eq!(models[0].primary, "id");
        assert!(models[0].cache);
        assert_eq!(models[0].cache_keys, vec!["id", "name"]);
        assert_eq!(models[0].soft_delete.as_deref(), Some("deleted"));
        assert_eq!(models[0].tenant.as_deref(), Some("tenant_id"));
        assert_eq!(models[0].fields[0].ty, "u64");
        assert_eq!(models[0].fields[3].ty, "Option<String>");
        assert_eq!(
            models[0].fields[3].default_value.as_deref(),
            Some("'guest'")
        );
        assert_eq!(
            models[0].fields[3].comment.as_deref(),
            Some("nickname from profile")
        );
        assert_eq!(
            models[0].fields[5].default_value.as_deref(),
            Some("CURRENT_TIMESTAMP")
        );
    }

    #[test]
    fn rejects_foreign_keys() {
        let source = r#"
        CREATE TABLE users (
            id bigint NOT NULL AUTO_INCREMENT,
            org_id bigint NOT NULL,
            PRIMARY KEY (id),
            CONSTRAINT fk_users_org FOREIGN KEY (org_id) REFERENCES orgs(id)
        );
        "#;

        let err = parse_models_with_format(source, ModelFormat::Sql).expect_err("parse error");
        assert!(err.to_string().contains("foreign keys are not supported"));
    }

    #[test]
    fn renders_repository_and_entity() {
        let model = ModelSpec {
            name: "User".to_string(),
            schema_name: None,
            table: "users".to_string(),
            primary: "id".to_string(),
            soft_delete: Some("deleted".to_string()),
            tenant: Some("tenant_id".to_string()),
            cache: true,
            cache_ttl_secs: Some(300),
            negative_cache_ttl_secs: None,
            cache_keys: vec!["id".to_string()],
            cache_prefix: None,
            fields: vec![
                ModelField {
                    name: "id".to_string(),
                    ty: "i64".to_string(),
                    auto_increment: true,
                    default_value: None,
                    comment: None,
                },
                ModelField {
                    name: "name".to_string(),
                    ty: "String".to_string(),
                    auto_increment: false,
                    default_value: None,
                    comment: None,
                },
                ModelField {
                    name: "tenant_id".to_string(),
                    ty: "String".to_string(),
                    auto_increment: false,
                    default_value: None,
                    comment: None,
                },
                ModelField {
                    name: "deleted".to_string(),
                    ty: "bool".to_string(),
                    auto_increment: false,
                    default_value: None,
                    comment: None,
                },
            ],
        };

        let rendered = render_model_module(&model);
        assert!(rendered.contains("DeriveEntityModel"));
        assert!(rendered.contains("PaginatorTrait"));
        assert!(rendered.contains("pub async fn count(&self) -> anyhow::Result<u64>"));
        assert!(rendered.contains("pub struct UserQuery"));
        assert!(rendered.contains("pub enum UserSortField"));
        assert!(rendered.contains("pub async fn query(&self, req: UserQuery)"));
        assert!(rendered.contains("pub id_in: Vec<i64>"));
        assert!(rendered.contains("pub id_min: Option<i64>"));
        assert!(rendered.contains("Column::Id.is_in(req.id_in)"));
        assert!(rendered.contains("Column::Id.gte(value)"));
        assert!(rendered.contains("order_by_desc(Column::Id)"));
        assert!(rendered.contains("pub async fn insert_many"));
        assert!(rendered.contains("pub async fn soft_delete_by_id"));
        assert!(rendered.contains("pub async fn find_by_id_for_tenant_id"));
        assert!(rendered.contains("pub async fn cached_find_by_id"));
        assert!(rendered.contains("pub async fn delete_by_id"));
    }

    #[test]
    fn renders_toasty_model_and_repository() {
        let source = r#"
        CREATE TABLE users (
            id BIGINT UNSIGNED NOT NULL AUTO_INCREMENT PRIMARY KEY,
            tenant_id VARCHAR(64) NOT NULL,
            email VARCHAR(255) NOT NULL,
            name VARCHAR(255) NOT NULL,
            nickname VARCHAR(255) NULL,
            deleted TINYINT(1) NOT NULL DEFAULT 0,
            UNIQUE KEY uniq_users_email (email)
        );
        "#;
        let models = parse_models_with_format(source, ModelFormat::Sql).expect("parse");
        let rendered = render_toasty_model_module(&models[0]);

        assert!(rendered
            .contains("#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, toasty::Model)]"));
        assert!(rendered.contains("#[table = \"users\"]"));
        assert!(rendered.contains("#[key]\n    #[auto]\n    pub id: u64"));
        assert!(rendered.contains("#[unique]\n    pub email: String"));
        assert!(rendered.contains("pub async fn find_by_id(db: &mut toasty::Db, id: &u64)"));
        assert!(
            rendered.contains("pub async fn find_by_email(db: &mut toasty::Db, email: &String)")
        );
        assert!(rendered.contains("pub struct UserQuery"));
        assert!(rendered.contains("pub enum UserSortField"));
        assert!(rendered.contains("pub async fn query(db: &mut toasty::Db, req: UserQuery)"));
        assert!(rendered.contains("pub id_in: Vec<u64>"));
        assert!(rendered.contains("pub id_min: Option<u64>"));
        assert!(rendered.contains("User::fields().id().in_list(req.id_in.clone())"));
        assert!(rendered.contains("User::fields().id().ge(value)"));
        assert!(rendered.contains("query.order_by(User::fields().id().desc())"));
        assert!(rendered.contains("pub async fn insert_many"));
        assert!(rendered.contains("pub async fn soft_delete_by_id"));
        assert!(rendered.contains("pub async fn find_by_id_for_tenant_id"));
        assert!(rendered.contains("User::all().collect::<Vec<_>>(db).await"));
        assert!(rendered.contains(".email(model.email)"));
        assert!(!rendered.contains(".id(model.id)"));
    }

    #[test]
    fn toasty_generation_updates_manifest_dependency() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let out = std::env::temp_dir().join(format!("rozectl-toasty-out-{unique}"));
        write_minimal_main(&out);
        fs::write(
            out.join("Cargo.toml"),
            r#"[package]
name = "user-service"
edition.workspace = true
license.workspace = true
version.workspace = true

[dependencies]
serde.workspace = true
"#,
        )
        .expect("manifest");

        generate_model_project(
            r#"
            CREATE TABLE users (
                id BIGINT NOT NULL AUTO_INCREMENT PRIMARY KEY,
                name VARCHAR(255) NOT NULL
            );
            "#,
            &out,
            GenerateOptions::new(GenerateMode::Create, DependencySource::Path),
            ModelFormat::Sql,
            ModelOrm::Toasty,
        )
        .expect("generate");

        let manifest = fs::read_to_string(out.join("Cargo.toml")).expect("manifest read");
        assert!(manifest.contains("toasty = { workspace = true }"));
        assert!(!manifest.contains("sea-orm.workspace = true"));
        let module = fs::read_to_string(out.join("src/model/user.rs")).expect("module read");
        assert!(module.contains("toasty::Model"));
        let mod_rs = fs::read_to_string(out.join("src/model/mod.rs")).expect("mod read");
        assert!(mod_rs.contains("pub use user::{User, UserRepository};"));
    }

    #[test]
    fn sea_orm_generation_updates_manifest_dependency_when_selected() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let out = std::env::temp_dir().join(format!("rozectl-sea-orm-out-{unique}"));
        write_minimal_main(&out);
        fs::write(
            out.join("Cargo.toml"),
            r#"[package]
name = "user-service"
edition.workspace = true
license.workspace = true
version.workspace = true

[dependencies]
serde.workspace = true
toasty.workspace = true
"#,
        )
        .expect("manifest");

        generate_model_project(
            r#"
            CREATE TABLE users (
                id BIGINT NOT NULL AUTO_INCREMENT PRIMARY KEY,
                name VARCHAR(255) NOT NULL
            );
            "#,
            &out,
            GenerateOptions::new(GenerateMode::Create, DependencySource::Path),
            ModelFormat::Sql,
            ModelOrm::SeaOrm,
        )
        .expect("generate");

        let manifest = fs::read_to_string(out.join("Cargo.toml")).expect("manifest read");
        assert!(manifest.contains("sea-orm = { workspace = true }"));
        let module = fs::read_to_string(out.join("src/model/user.rs")).expect("module read");
        assert!(module.contains("sea_orm::"));
    }

    #[test]
    fn renders_mongo_repository_with_cache_keys() {
        let source = r#"
        model User {
            table: users
            primary: id
            cache: true
            cache_key: id,username
            cache_prefix: account
            cache_ttl_secs: 120
            negative_cache_ttl_secs: 10
            field id ObjectId
            field username String
            field display_name Option<String>
        }
        "#;
        let models = parse_models_with_format(source, ModelFormat::Mongo).expect("parse");
        assert_eq!(models[0].cache_keys, vec!["id", "username"]);

        let rendered = render_mongo_model_module(&models[0]);
        assert!(rendered.contains("roze_mongo::bson"));
        assert!(rendered.contains("#[serde(rename = \"_id\")]"));
        assert!(rendered.contains("pub async fn find_by_username"));
        assert!(rendered.contains("pub async fn cached_find_by_username"));
        assert!(rendered.contains("Self::filter_by(\"_id\", &id)?"));
        assert!(rendered.contains("format!(\"account:{}:{}\", field, value)"));
        assert!(rendered.contains("cache.del(&self.cache_key(\"username\", &model.username))"));
    }

    #[tokio::test]
    async fn inspects_sqlite_and_writes_model_project() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let db_path = std::env::temp_dir().join(format!("rozectl-sqlite-{unique}.db"));
        let db_url = format!("sqlite://{}?mode=rwc", db_path.display());
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect(&db_url)
            .await
            .expect("connect");
        sqlx::query(
            r#"
            CREATE TABLE users (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL,
                nickname TEXT DEFAULT 'guest'
            );
            "#,
        )
        .execute(&pool)
        .await
        .expect("create table");
        sqlx::query("CREATE UNIQUE INDEX uniq_users_name ON users(name)")
            .execute(&pool)
            .await
            .expect("create unique index");

        let out = std::env::temp_dir().join(format!("rozectl-inspect-out-{unique}"));
        fs::create_dir_all(out.join("src")).expect("out src");
        fs::write(
            out.join("src/main.rs"),
            r#"mod config;
mod svc;
mod types;
"#,
        )
        .expect("main");

        inspect_model_project(
            "users",
            Some("audit"),
            &db_url,
            SqlxDatabaseKind::Sqlite,
            &out,
            GenerateOptions::new(GenerateMode::Create, DependencySource::Path),
            ModelOrm::SeaOrm,
        )
        .await
        .expect("inspect");

        assert!(out.join("src/model/mod.rs").is_file());
        assert!(out.join("src/model/user.rs").is_file());
        let rendered = fs::read_to_string(out.join("src/model/user.rs")).expect("rendered");
        assert!(rendered.contains("schema_name = \"audit\""));
        assert!(rendered.contains("pub async fn find_by_name"));
        let main_rs = fs::read_to_string(out.join("src/main.rs")).expect("main read");
        assert!(main_rs.contains("mod model;"));
    }

    #[tokio::test]
    async fn inspects_postgres_schema_namespace_if_available() {
        let Some(db_url) = std::env::var("ROZECTL_TEST_POSTGRES_URL").ok() else {
            eprintln!("skipping postgres inspect test: ROZECTL_TEST_POSTGRES_URL not set");
            return;
        };

        let pool = sqlx::PgPool::connect(&db_url).await.expect("connect");
        let schema = format!(
            "rozectl_{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        );
        sqlx::query(AssertSqlSafe(format!(
            r#"CREATE SCHEMA IF NOT EXISTS "{schema}""#
        )))
        .execute(&pool)
        .await
        .expect("create schema");
        sqlx::query(AssertSqlSafe(format!(
            r#"
            CREATE TABLE IF NOT EXISTS "{schema}".rozectl_users (
                id BIGSERIAL PRIMARY KEY,
                name TEXT NOT NULL,
                nickname TEXT DEFAULT 'guest'
            )
            "#
        )))
        .execute(&pool)
        .await
        .expect("create table");

        let out = temp_model_output("postgres");
        write_minimal_main(&out);

        inspect_model_project(
            &format!("{schema}.rozectl_users"),
            Some(&schema),
            &db_url,
            SqlxDatabaseKind::Postgres,
            &out,
            GenerateOptions::new(GenerateMode::Create, DependencySource::Path),
            ModelOrm::SeaOrm,
        )
        .await
        .expect("inspect");

        let spec = inspect_postgres_table(&pool, Some(&schema), "rozectl_users")
            .await
            .expect("inspect");
        assert_eq!(spec.schema_name.as_deref(), Some(schema.as_str()));
        assert_eq!(spec.table, "rozectl_users");
        assert_eq!(spec.primary, "id");

        let rendered = render_model_module(&spec);
        assert!(rendered.contains(&format!("schema_name = \"{schema}\"")));
        assert!(rendered.contains("table_name = \"rozectl_users\""));
        assert!(out.join("src/model/mod.rs").is_file());
        assert!(out.join("src/model/rozectl_user.rs").is_file());
        let main_rs = fs::read_to_string(out.join("src/main.rs")).expect("main read");
        assert!(main_rs.contains("mod model;"));
    }

    #[tokio::test]
    async fn inspects_mysql_schema_namespace_if_available() {
        let Some(db_url) = std::env::var("ROZECTL_TEST_MYSQL_URL").ok() else {
            eprintln!("skipping mysql inspect test: ROZECTL_TEST_MYSQL_URL not set");
            return;
        };

        let pool = sqlx::MySqlPool::connect(&db_url).await.expect("connect");
        let db_name: Option<String> = sqlx::query_scalar("SELECT DATABASE()")
            .fetch_one(&pool)
            .await
            .expect("database");
        let Some(db_name) = db_name else {
            eprintln!("skipping mysql inspect test: connection has no default database");
            return;
        };

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS rozectl_users (
                id BIGINT UNSIGNED NOT NULL AUTO_INCREMENT PRIMARY KEY,
                name VARCHAR(255) NOT NULL,
                nickname VARCHAR(255) DEFAULT 'guest'
            );
            "#,
        )
        .execute(&pool)
        .await
        .expect("create table");

        let out = temp_model_output("mysql");
        write_minimal_main(&out);

        inspect_model_project(
            &format!("{db_name}.rozectl_users"),
            Some(&db_name),
            &db_url,
            SqlxDatabaseKind::MySql,
            &out,
            GenerateOptions::new(GenerateMode::Create, DependencySource::Path),
            ModelOrm::SeaOrm,
        )
        .await
        .expect("inspect");

        let qualified = format!("{}.rozectl_users", db_name);
        let spec = inspect_mysql_table(&pool, Some(&db_name), &qualified)
            .await
            .expect("inspect");
        assert_eq!(spec.schema_name.as_deref(), Some(db_name.as_str()));
        assert_eq!(spec.table, "rozectl_users");
        assert_eq!(spec.primary, "id");

        let rendered = render_model_module(&spec);
        assert!(rendered.contains(&format!("schema_name = \"{}\"", db_name)));
        assert!(rendered.contains("table_name = \"rozectl_users\""));
        assert!(out.join("src/model/mod.rs").is_file());
        assert!(out.join("src/model/rozectl_user.rs").is_file());
        let main_rs = fs::read_to_string(out.join("src/main.rs")).expect("main read");
        assert!(main_rs.contains("mod model;"));
    }

    #[test]
    fn schema_mismatch_is_rejected() {
        let err = normalize_table_reference(Some("db"), "public.users").expect_err("mismatch");
        assert!(
            err.to_string().contains("schema mismatch"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn inspect_matches_generate_sql_for_sqlite_schema() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let db_path = std::env::temp_dir().join(format!("rozectl-compare-{unique}.db"));
        let db_url = format!("sqlite://{}?mode=rwc", db_path.display());
        let ddl = r#"
        CREATE TABLE users (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL,
            nickname TEXT NULL
        );
        "#;

        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect(&db_url)
            .await
            .expect("connect");
        sqlx::query(ddl).execute(&pool).await.expect("create table");

        let inspect_out = std::env::temp_dir().join(format!("rozectl-inspect-compare-{unique}"));
        let generate_out = std::env::temp_dir().join(format!("rozectl-generate-compare-{unique}"));
        write_minimal_main(&inspect_out);
        write_minimal_main(&generate_out);

        inspect_model_project(
            "users",
            None,
            &db_url,
            SqlxDatabaseKind::Sqlite,
            &inspect_out,
            GenerateOptions::new(GenerateMode::Create, DependencySource::Path),
            ModelOrm::SeaOrm,
        )
        .await
        .expect("inspect");

        generate_model_project(
            ddl,
            &generate_out,
            GenerateOptions::new(GenerateMode::Create, DependencySource::Path),
            ModelFormat::Sql,
            ModelOrm::SeaOrm,
        )
        .expect("generate");

        let inspect_module =
            fs::read_to_string(inspect_out.join("src/model/user.rs")).expect("inspect module");
        let generate_module =
            fs::read_to_string(generate_out.join("src/model/user.rs")).expect("generate module");
        let inspect_mod =
            fs::read_to_string(inspect_out.join("src/model/mod.rs")).expect("inspect mod");
        let generate_mod =
            fs::read_to_string(generate_out.join("src/model/mod.rs")).expect("generate mod");
        assert_eq!(inspect_module, generate_module);
        assert_eq!(inspect_mod, generate_mod);
    }

    #[tokio::test]
    async fn inspect_with_schema_compares_against_generate_sql_for_postgres_if_available() {
        let Some(db_url) = std::env::var("ROZECTL_TEST_POSTGRES_URL").ok() else {
            eprintln!("skipping postgres compare test: ROZECTL_TEST_POSTGRES_URL not set");
            return;
        };

        let pool = sqlx::PgPool::connect(&db_url).await.expect("connect");
        let schema = format!(
            "rozectl_{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        );
        sqlx::query(AssertSqlSafe(format!(
            r#"CREATE SCHEMA IF NOT EXISTS "{schema}""#
        )))
        .execute(&pool)
        .await
        .expect("create schema");
        sqlx::query(AssertSqlSafe(format!(
            r#"
            CREATE TABLE IF NOT EXISTS "{schema}".rozectl_users (
                id BIGSERIAL PRIMARY KEY,
                name TEXT NOT NULL,
                nickname TEXT NULL
            )
            "#
        )))
        .execute(&pool)
        .await
        .expect("create table");

        let ddl = format!(
            r#"
            CREATE TABLE "{schema}".rozectl_users (
                id BIGSERIAL PRIMARY KEY,
                name TEXT NOT NULL,
                nickname TEXT NULL
            );
            "#
        );

        let inspect_out = temp_model_output("postgres-compare");
        let generate_out = temp_model_output("postgres-generate");
        write_minimal_main(&inspect_out);
        write_minimal_main(&generate_out);

        inspect_model_project(
            "rozectl_users",
            Some(&schema),
            &db_url,
            SqlxDatabaseKind::Postgres,
            &inspect_out,
            GenerateOptions::new(GenerateMode::Create, DependencySource::Path),
            ModelOrm::SeaOrm,
        )
        .await
        .expect("inspect");

        generate_model_project(
            &ddl,
            &generate_out,
            GenerateOptions::new(GenerateMode::Create, DependencySource::Path),
            ModelFormat::Sql,
            ModelOrm::SeaOrm,
        )
        .expect("generate");

        let inspect_module = fs::read_to_string(inspect_out.join("src/model/rozectl_user.rs"))
            .expect("inspect module");
        let generate_module = fs::read_to_string(generate_out.join("src/model/rozectl_user.rs"))
            .expect("generate module");
        assert_eq!(inspect_module, generate_module);
    }

    #[tokio::test]
    async fn inspect_with_schema_compares_against_generate_sql_for_mysql_if_available() {
        let Some(db_url) = std::env::var("ROZECTL_TEST_MYSQL_URL").ok() else {
            eprintln!("skipping mysql compare test: ROZECTL_TEST_MYSQL_URL not set");
            return;
        };

        let pool = sqlx::MySqlPool::connect(&db_url).await.expect("connect");
        let db_name: Option<String> = sqlx::query_scalar("SELECT DATABASE()")
            .fetch_one(&pool)
            .await
            .expect("database");
        let Some(db_name) = db_name else {
            eprintln!("skipping mysql compare test: connection has no default database");
            return;
        };

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS rozectl_users (
                id BIGINT UNSIGNED NOT NULL AUTO_INCREMENT PRIMARY KEY,
                name VARCHAR(255) NOT NULL,
                nickname VARCHAR(255) NULL
            );
            "#,
        )
        .execute(&pool)
        .await
        .expect("create table");

        let ddl = format!(
            r#"
            CREATE TABLE `{db_name}`.`rozectl_users` (
                id BIGINT UNSIGNED NOT NULL AUTO_INCREMENT PRIMARY KEY,
                name VARCHAR(255) NOT NULL,
                nickname VARCHAR(255) NULL
            );
            "#
        );

        let inspect_out = temp_model_output("mysql-compare");
        let generate_out = temp_model_output("mysql-generate");
        write_minimal_main(&inspect_out);
        write_minimal_main(&generate_out);

        inspect_model_project(
            "rozectl_users",
            Some(&db_name),
            &db_url,
            SqlxDatabaseKind::MySql,
            &inspect_out,
            GenerateOptions::new(GenerateMode::Create, DependencySource::Path),
            ModelOrm::SeaOrm,
        )
        .await
        .expect("inspect");

        generate_model_project(
            &ddl,
            &generate_out,
            GenerateOptions::new(GenerateMode::Create, DependencySource::Path),
            ModelFormat::Sql,
            ModelOrm::SeaOrm,
        )
        .expect("generate");

        let inspect_module = fs::read_to_string(inspect_out.join("src/model/rozectl_user.rs"))
            .expect("inspect module");
        let generate_module = fs::read_to_string(generate_out.join("src/model/rozectl_user.rs"))
            .expect("generate module");
        assert_eq!(inspect_module, generate_module);
    }

    #[test]
    fn build_inspected_model_preserves_nullable_defaults_and_comments() {
        let model = build_inspected_model(
            Some("public".to_string()),
            "users",
            vec![
                InspectedColumn {
                    name: "id".to_string(),
                    ty: "i64".to_string(),
                    nullable: false,
                    auto_increment: true,
                    default_value: None,
                    comment: None,
                },
                InspectedColumn {
                    name: "name".to_string(),
                    ty: "String".to_string(),
                    nullable: false,
                    auto_increment: false,
                    default_value: None,
                    comment: None,
                },
                InspectedColumn {
                    name: "nickname".to_string(),
                    ty: "String".to_string(),
                    nullable: true,
                    auto_increment: false,
                    default_value: Some("'guest'".to_string()),
                    comment: Some("Nickname".to_string()),
                },
                InspectedColumn {
                    name: "active".to_string(),
                    ty: "bool".to_string(),
                    nullable: true,
                    auto_increment: false,
                    default_value: Some("true".to_string()),
                    comment: Some("Active flag".to_string()),
                },
            ],
            Some("id".to_string()),
            vec!["name".to_string(), "nickname".to_string()],
        )
        .expect("build");

        assert_eq!(model.schema_name.as_deref(), Some("public"));
        assert_eq!(model.table, "users");
        assert_eq!(model.primary, "id");
        assert_eq!(model.fields[0].ty, "i64");
        assert_eq!(model.fields[2].ty, "Option<String>");
        assert_eq!(model.fields[2].default_value.as_deref(), Some("'guest'"));
        assert_eq!(model.fields[2].comment.as_deref(), Some("Nickname"));
        assert_eq!(model.fields[3].ty, "Option<bool>");
        assert_eq!(model.fields[3].default_value.as_deref(), Some("true"));
        assert_eq!(model.fields[3].comment.as_deref(), Some("Active flag"));
        assert_eq!(model.cache_keys, vec!["id", "name"]);

        let rendered = render_model_module(&model);
        assert!(rendered.contains("schema_name = \"public\""));
        assert!(rendered.contains("/// Nickname"));
        assert!(rendered.contains("/// default: 'guest'"));
        assert!(rendered.contains("/// Active flag"));
        assert!(rendered.contains("/// default: true"));
        assert!(rendered.contains("pub nickname: Option<String>"));
        assert!(rendered.contains("pub active: Option<bool>"));
        assert!(rendered.contains("pub async fn find_by_name"));
        assert!(!rendered.contains("pub async fn find_by_nickname"));
    }

    fn temp_model_output(label: &str) -> std::path::PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        std::env::temp_dir().join(format!("rozectl-{label}-{unique}"))
    }

    fn write_minimal_main(out: &std::path::Path) {
        fs::create_dir_all(out.join("src")).expect("src dir");
        fs::write(
            out.join("src/main.rs"),
            r#"mod config;
mod svc;
mod types;
"#,
        )
        .expect("main");
    }
}
