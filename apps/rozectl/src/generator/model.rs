use std::{
    fs,
    path::Path,
};

use anyhow::{bail, Context};

use super::{to_pascal_case, to_snake_case, GenerateMode, GenerateOptions};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelFormat {
    Auto,
    Dsl,
    Sql,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelSpec {
    pub name: String,
    pub table: String,
    pub primary: String,
    pub cache: bool,
    pub cache_ttl_secs: Option<u64>,
    pub fields: Vec<ModelField>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelField {
    pub name: String,
    pub ty: String,
}

pub fn generate_model_project(
    source: &str,
    out: &Path,
    options: GenerateOptions,
    format: ModelFormat,
) -> anyhow::Result<()> {
    let models = parse_models_with_format(source, format)?;
    ensure_model_output(out, options.mode)?;

    fs::create_dir_all(out.join("src/model"))?;
    fs::write(out.join("src/model/mod.rs"), render_model_mod(&models))?;
    for model in &models {
        let file_name = format!("{}.rs", to_snake_case(&model.name));
        fs::write(
            out.join("src/model").join(file_name),
            render_model_module(model),
        )
        .with_context(|| format!("failed to write model {}", model.name))?;
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
    fs::write(&main_path, updated).with_context(|| format!("failed to write {}", main_path.display()))
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

fn render_model_mod(models: &[ModelSpec]) -> String {
    let mut out = String::from("#![allow(dead_code, unused_imports)]\n\n");
    for model in models {
        let module = to_snake_case(&model.name);
        let pascal = to_pascal_case(&model.name);
        out.push_str(&format!("pub mod {module};\n"));
        out.push_str(&format!(
            "pub use {module}::{{{pascal}Repository, ActiveModel as {pascal}ActiveModel, Entity as {pascal}Entity, Model as {pascal}Model}};\n"
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
    let mut out = String::new();
    use std::fmt::Write as _;

    writeln!(&mut out, "#![allow(dead_code, unused_imports)]").unwrap();
    writeln!(&mut out).unwrap();
    writeln!(&mut out, "use std::time::Duration;").unwrap();
    writeln!(&mut out, "use sea_orm::entity::prelude::*;").unwrap();
    writeln!(
        &mut out,
        "use sea_orm::{{ActiveModelTrait, DatabaseConnection, DeleteResult, EntityTrait, IntoActiveModel}};"
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
    writeln!(&mut out, "#[sea_orm(table_name = \"{}\")]", table_name).unwrap();
    writeln!(&mut out, "pub struct Model {{").unwrap();
    for field in &model.fields {
        if field.name == model.primary {
            writeln!(&mut out, "    #[sea_orm(primary_key)]").unwrap();
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
        "    fn db(&self) -> anyhow::Result<&DatabaseConnection> {{"
    )
    .unwrap();
    writeln!(
        &mut out,
        "        self.ctx.db.as_ref().ok_or_else(|| anyhow::anyhow!(\"database connection is not configured\"))"
    )
    .unwrap();
    writeln!(&mut out, "    }}").unwrap();
    writeln!(&mut out).unwrap();
    writeln!(&mut out, "    pub fn table_name() -> &'static str {{").unwrap();
    writeln!(&mut out, "        \"{}\"", table_name).unwrap();
    writeln!(&mut out, "    }}").unwrap();
    writeln!(&mut out).unwrap();
    writeln!(
        &mut out,
        "    pub async fn find_by_{}(&self, {}: {}) -> anyhow::Result<Option<Model>> {{",
        primary, primary, primary_ty
    )
    .unwrap();
    writeln!(&mut out, "        let db = self.db()?;").unwrap();
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
        "    pub async fn list(&self) -> anyhow::Result<Vec<Model>> {{"
    )
    .unwrap();
    writeln!(&mut out, "        let db = self.db()?;").unwrap();
    writeln!(&mut out, "        Ok(Entity::find().all(db).await?)").unwrap();
    writeln!(&mut out, "    }}").unwrap();
    writeln!(&mut out).unwrap();
    writeln!(
        &mut out,
        "    pub async fn insert(&self, model: Model) -> anyhow::Result<Model> {{"
    )
    .unwrap();
    writeln!(&mut out, "        let db = self.db()?;").unwrap();
    writeln!(&mut out, "        let active: ActiveModel = model.into_active_model();").unwrap();
    writeln!(&mut out, "        let inserted = active.insert(db).await?;").unwrap();
    if model.cache {
        writeln!(
            &mut out,
            "        self.invalidate_cache(inserted.{primary}).await?;",
            primary = primary
        )
        .unwrap();
    }
    writeln!(&mut out, "        Ok(inserted)").unwrap();
    writeln!(&mut out, "    }}").unwrap();
    writeln!(&mut out).unwrap();
    writeln!(
        &mut out,
        "    pub async fn update(&self, model: Model) -> anyhow::Result<Model> {{"
    )
    .unwrap();
    writeln!(&mut out, "        let db = self.db()?;").unwrap();
    writeln!(&mut out, "        let active: ActiveModel = model.into_active_model();").unwrap();
    writeln!(&mut out, "        let updated = active.update(db).await?;").unwrap();
    if model.cache {
        writeln!(
            &mut out,
            "        self.invalidate_cache(updated.{primary}).await?;",
            primary = primary
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
    writeln!(&mut out, "        let db = self.db()?;").unwrap();
    writeln!(
        &mut out,
        "        let result = Entity::delete_by_id({}).exec(db).await?;",
        primary
    )
    .unwrap();
    if model.cache {
        writeln!(
            &mut out,
            "        self.invalidate_cache({primary}).await?;",
            primary = primary
        )
        .unwrap();
    }
    writeln!(&mut out, "        Ok(result)").unwrap();
    writeln!(&mut out, "    }}").unwrap();
    if model.cache {
        writeln!(&mut out).unwrap();
        writeln!(
            &mut out,
            "    fn cache_key(&self, {}: {}) -> String {{",
            primary, primary_ty
        )
        .unwrap();
        writeln!(
            &mut out,
            "        format!(\"{{}}:{{}}\", Self::table_name(), {})",
            primary
        )
        .unwrap();
        writeln!(&mut out, "    }}").unwrap();
        writeln!(&mut out).unwrap();
        writeln!(
            &mut out,
            "    async fn invalidate_cache(&self, {}: {}) -> anyhow::Result<()> {{",
            primary, primary_ty
        )
        .unwrap();
        writeln!(&mut out, "        if let Some(cache) = self.ctx.cache.as_ref() {{").unwrap();
        writeln!(&mut out, "            let key = self.cache_key({});", primary).unwrap();
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
        writeln!(&mut out, "        if let Some(cache) = self.ctx.cache.as_ref() {{").unwrap();
        writeln!(&mut out, "            let key = self.cache_key({});", primary).unwrap();
        writeln!(
            &mut out,
            "            let ttl = Duration::from_secs({});",
            cache_ttl_secs
        )
        .unwrap();
        writeln!(
            &mut out,
            "            let negative_ttl = Duration::from_secs(({} / 6).clamp(5, 60));",
            cache_ttl_secs
        )
        .unwrap();
        writeln!(&mut out, "            return cache").unwrap();
        writeln!(&mut out, "                .get_or_set_json_option(").unwrap();
        writeln!(&mut out, "                    &key,").unwrap();
        writeln!(&mut out, "                    Some(ttl),").unwrap();
        writeln!(&mut out, "                    Some(negative_ttl),").unwrap();
        writeln!(
            &mut out,
            "                    || async {{ self.find_by_{}({}).await }},",
            primary, primary
        )
        .unwrap();
        writeln!(&mut out, "                )").unwrap();
        writeln!(&mut out, "                .await;").unwrap();
        writeln!(&mut out, "        }}").unwrap();
        writeln!(&mut out).unwrap();
        writeln!(
            &mut out,
            "        self.find_by_{}({}).await",
            primary, primary
        )
        .unwrap();
        writeln!(&mut out, "    }}").unwrap();
    }
    writeln!(&mut out, "}}").unwrap();

    out
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
            if let Some(value) = inner.strip_prefix("field ") {
                let mut parts = value.split_whitespace();
                let field_name = parts
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("line {}: expected field name", inner_line_no + 1))?;
                let field_ty = parts
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("line {}: expected field type", inner_line_no + 1))?;
                fields.push(ModelField {
                    name: field_name.to_string(),
                    ty: field_ty.to_string(),
                });
                continue;
            }

            bail!(
                "line {}: expected `table:`, `primary:`, `cache:`, `cache_ttl_secs:` or `field`",
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

        models.push(ModelSpec {
            name,
            table,
            primary,
            cache,
            cache_ttl_secs,
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
    let lines = source.lines().enumerate().collect::<Vec<_>>();
    let mut i = 0usize;

    while i < lines.len() {
        let (line_no, raw) = lines[i];
        let line = raw.trim();
        i += 1;

        if line.is_empty() || is_sql_comment(line) {
            continue;
        }

        if !starts_with_ci(line, "create table") {
            bail!("line {}: expected `CREATE TABLE ...`", line_no + 1);
        }

        let mut statement = String::from(line);
        let mut depth = paren_balance(line);
        let mut finished = line.trim_end().ends_with(';') && depth <= 0;

        while !finished && i < lines.len() {
            let (_, next_raw) = lines[i];
            let next_line = next_raw.trim_end();
            statement.push('\n');
            statement.push_str(next_line);
            depth += paren_balance(next_line);
            finished = next_line.trim_end().ends_with(';') && depth <= 0;
            i += 1;
        }

        if !finished {
            bail!("line {}: unterminated CREATE TABLE statement", line_no + 1);
        }

        models.push(parse_create_table(&statement, line_no + 1)?);
    }

    if models.is_empty() {
        bail!("no model declarations found");
    }

    Ok(models)
}

fn parse_create_table(statement: &str, start_line: usize) -> anyhow::Result<ModelSpec> {
    let statement = statement.trim().trim_end_matches(';').trim();
    let open = statement
        .find('(')
        .ok_or_else(|| anyhow::anyhow!("line {}: expected `(` after table name", start_line))?;
    let close = find_matching_paren(statement, open).ok_or_else(|| {
        anyhow::anyhow!("line {}: expected closing `)` for table definition", start_line)
    })?;

    let header = statement[..open].trim();
    let body = &statement[open + 1..close];
    let table = parse_table_name(header, start_line)?;
    let mut fields = Vec::new();
    let mut primary = None;

    for entry in split_sql_items(body) {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }

        if starts_with_ci(entry, "primary key") {
            let columns = parse_key_columns(entry, start_line)?;
            if columns.len() != 1 {
                bail!("line {}: composite primary keys are not supported", start_line);
            }
            primary = columns.into_iter().next();
            continue;
        }

        if starts_with_ci(entry, "key ")
            || starts_with_ci(entry, "unique key")
            || starts_with_ci(entry, "index ")
            || starts_with_ci(entry, "constraint ")
            || starts_with_ci(entry, "foreign key")
            || starts_with_ci(entry, "unique index")
        {
            continue;
        }

        fields.push(parse_sql_field(entry, start_line)?);
    }

    if fields.is_empty() {
        bail!("line {}: CREATE TABLE must declare at least one column", start_line);
    }

    let primary = match primary {
        Some(primary) => primary,
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

    let name = model_name_from_table(&table);
    let primary_name = primary.clone();
    Ok(ModelSpec {
        name,
        table,
        primary,
        cache: true,
        cache_ttl_secs: None,
        fields: fields
            .into_iter()
            .map(|field| {
                let field_name = field.name;
                let ty = if field.nullable && field_name != primary_name {
                    format!("Option<{}>", field.ty)
                } else {
                    field.ty
                };
                ModelField {
                    name: field_name,
                    ty,
                }
            })
            .collect(),
    })
}

#[derive(Debug, Clone)]
struct ParsedSqlField {
    name: String,
    ty: String,
    nullable: bool,
    auto_increment: bool,
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
    let mut attrs = Vec::new();
    let mut i = 0usize;
    while i < tokens.len() {
        let token = tokens[i].clone();
        if is_sql_attr_keyword(token.as_str()) {
            attrs.extend(tokens[i..].iter().cloned());
            break;
        }
        type_tokens.push(token);
        i += 1;
    }
    if type_tokens.is_empty() {
        bail!("line {}: expected column type for `{}`", start_line, name);
    }

    let raw_ty = type_tokens.join(" ");
    let nullable = if contains_keyword(&attrs, "not") && contains_keyword(&attrs, "null") {
        false
    } else if contains_keyword(&attrs, "null") {
        true
    } else {
        true
    };
    let auto_increment = contains_keyword(&attrs, "auto_increment");
    let ty = map_sql_type(&raw_ty, auto_increment);
    Ok(ParsedSqlField {
        name: strip_sql_identifier(&name),
        ty,
        nullable,
        auto_increment,
    })
}

fn parse_key_columns(entry: &str, start_line: usize) -> anyhow::Result<Vec<String>> {
    let open = entry
        .find('(')
        .ok_or_else(|| anyhow::anyhow!("line {}: expected `(` in PRIMARY KEY", start_line))?;
    let close = find_matching_paren(entry, open).ok_or_else(|| {
        anyhow::anyhow!("line {}: expected `)` in PRIMARY KEY", start_line)
    })?;
    Ok(split_sql_items(&entry[open + 1..close])
        .into_iter()
        .map(|item| strip_sql_identifier(item.trim()))
        .filter(|item| !item.is_empty())
        .collect())
}

fn parse_table_name(header: &str, start_line: usize) -> anyhow::Result<String> {
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
    let table = strip_sql_identifier(table);
    if table.is_empty() {
        bail!("line {}: missing table name", start_line);
    }
    Ok(table)
}

fn split_sql_items(body: &str) -> Vec<String> {
    let mut items = Vec::new();
    let mut current = String::new();
    let mut depth = 0i32;
    let mut in_single = false;
    let mut in_double = false;
    let mut in_backtick = false;
    let mut chars = body.chars().peekable();

    while let Some(ch) = chars.next() {
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
    input
        .strip_prefix('`')
        .and_then(|value| value.strip_suffix('`'))
        .unwrap_or(input)
        .to_string()
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

fn contains_keyword(tokens: &[String], keyword: &str) -> bool {
    tokens
        .iter()
        .any(|token| token.eq_ignore_ascii_case(keyword))
}

fn map_sql_type(raw_ty: &str, auto_increment: bool) -> String {
    let normalized = raw_ty.to_ascii_lowercase();
    if normalized.contains("tinyint(1)") || normalized == "bool" || normalized == "boolean" {
        return "bool".to_string();
    }

    if normalized.contains("bigint") {
        return if normalized.contains("unsigned") || auto_increment {
            "u64".to_string()
        } else {
            "i64".to_string()
        };
    }

    if normalized.contains("int")
        || normalized.contains("mediumint")
        || normalized.contains("smallint")
        || normalized.contains("tinyint")
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
    {
        return "f64".to_string();
    }

    if normalized.contains("blob") || normalized.contains("binary") {
        return "Vec<u8>".to_string();
    }

    if normalized.contains("json")
        || normalized.contains("char")
        || normalized.contains("text")
        || normalized.contains("enum")
        || normalized.contains("set")
        || normalized.contains("date")
        || normalized.contains("time")
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

    #[test]
    fn parses_model_blocks() {
        let source = r#"
        model User {
            table: users
            primary: id
            cache: true
            cache_ttl_secs: 300
            field id i64
            field name String
        }
        "#;

        let models = parse_models(source).expect("parse");
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].name, "User");
        assert_eq!(models[0].table, "users");
        assert_eq!(models[0].primary, "id");
        assert!(models[0].cache);
    }

    #[test]
    fn parses_sql_model_blocks() {
        let source = r#"
        CREATE TABLE `users` (
            `id` bigint unsigned NOT NULL AUTO_INCREMENT,
            `name` varchar(255) NOT NULL,
            `nickname` varchar(255) NULL,
            PRIMARY KEY (`id`)
        );
        "#;

        let models = parse_models_with_format(source, ModelFormat::Sql).expect("parse");
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].name, "User");
        assert_eq!(models[0].table, "users");
        assert_eq!(models[0].primary, "id");
        assert!(models[0].cache);
        assert_eq!(models[0].fields[0].ty, "u64");
        assert_eq!(models[0].fields[2].ty, "Option<String>");
    }

    #[test]
    fn renders_repository_and_entity() {
        let model = ModelSpec {
            name: "User".to_string(),
            table: "users".to_string(),
            primary: "id".to_string(),
            cache: true,
            cache_ttl_secs: Some(300),
            fields: vec![
                ModelField {
                    name: "id".to_string(),
                    ty: "i64".to_string(),
                },
                ModelField {
                    name: "name".to_string(),
                    ty: "String".to_string(),
                },
            ],
        };

        let rendered = render_model_module(&model);
        assert!(rendered.contains("DeriveEntityModel"));
        assert!(rendered.contains("pub async fn cached_find_by_id"));
        assert!(rendered.contains("pub async fn delete_by_id"));
    }
}
