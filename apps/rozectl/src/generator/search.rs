use std::{collections::BTreeSet, fs, path::Path};

use anyhow::{bail, Context};
use serde_json::Value;

use super::{
    find_workspace_root, inherited_roze_dependency, local_crates_prefix, plan::GenerationPlan,
    rust_identifier, sync_managed_service_if_present, to_pascal_case, to_snake_case,
    validate_roze_dependency_sources, DependencySource, GenerateMode, GenerateOptions,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchEngine {
    Elasticsearch,
    Opensearch,
    Meilisearch,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchIndexSpec {
    pub name: String,
    pub primary: String,
    pub fields: Vec<SearchFieldSpec>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchFieldSpec {
    pub name: String,
    pub source_name: Option<String>,
    pub ty: SearchFieldType,
    pub searchable: bool,
    pub filterable: bool,
    pub sortable: bool,
    pub primary: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchFieldType {
    Keyword,
    Text,
    I32,
    I64,
    U64,
    F64,
    Bool,
    DateTime,
    Json,
}

impl SearchFieldType {
    fn rust_type(self) -> &'static str {
        match self {
            Self::Keyword | Self::Text | Self::DateTime => "String",
            Self::I32 => "i32",
            Self::I64 => "i64",
            Self::U64 => "u64",
            Self::F64 => "f64",
            Self::Bool => "bool",
            Self::Json => "serde_json::Value",
        }
    }
}

pub fn generate_search_project(
    schema: &Path,
    engine: SearchEngine,
    out: &Path,
    options: GenerateOptions,
) -> anyhow::Result<()> {
    let source = fs::read_to_string(schema)
        .with_context(|| format!("failed to read {}", schema.display()))?;
    let spec = parse_search_schema(&source)?;
    commit_search_project(&spec, engine, out, options)
}

pub async fn inspect_search_index(
    index: &str,
    engine: SearchEngine,
    url: &str,
    api_key: Option<&str>,
    sample_size: u64,
    out: &Path,
    options: GenerateOptions,
) -> anyhow::Result<()> {
    let spec = match engine {
        SearchEngine::Elasticsearch | SearchEngine::Opensearch => {
            inspect_elastic_mapping(index, url, api_key).await?
        }
        SearchEngine::Meilisearch => {
            inspect_meilisearch_index(index, url, api_key, sample_size).await?
        }
    };
    commit_search_project(&spec, engine, out, options)
}

pub fn parse_search_schema(source: &str) -> anyhow::Result<SearchIndexSpec> {
    let trimmed = source.trim();
    if trimmed.starts_with('{') {
        return parse_json_search_schema(trimmed);
    }
    parse_dsl_search_schema(trimmed)
}

fn parse_json_search_schema(source: &str) -> anyhow::Result<SearchIndexSpec> {
    let value: Value = serde_json::from_str(source)?;
    let name = value
        .get("index")
        .or_else(|| value.get("name"))
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("search schema JSON requires `index`"))?
        .to_string();
    let primary = value
        .get("primary")
        .and_then(Value::as_str)
        .unwrap_or("id")
        .to_string();
    let primary_rust = to_snake_case(&primary);
    let fields = value
        .get("fields")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("search schema JSON requires `fields` array"))?
        .iter()
        .map(|field| {
            let name = field
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("search field requires `name`"))?;
            let ty = field
                .get("type")
                .or_else(|| field.get("kind"))
                .and_then(Value::as_str)
                .map(parse_search_type)
                .transpose()?
                .unwrap_or(SearchFieldType::Json);
            Ok(SearchFieldSpec {
                name: to_snake_case(name),
                source_name: search_source_name(name),
                ty,
                searchable: field
                    .get("searchable")
                    .and_then(Value::as_bool)
                    .unwrap_or(matches!(ty, SearchFieldType::Text)),
                filterable: field
                    .get("filterable")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                sortable: field
                    .get("sortable")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                primary: field
                    .get("primary")
                    .and_then(Value::as_bool)
                    .unwrap_or(to_snake_case(name) == primary_rust),
            })
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    normalize_search_spec(SearchIndexSpec {
        name,
        primary: primary_rust,
        fields,
    })
}

fn parse_dsl_search_schema(source: &str) -> anyhow::Result<SearchIndexSpec> {
    let mut name = None;
    let mut primary = None;
    let mut fields = Vec::new();
    for (line_no, raw) in source.lines().enumerate() {
        let line = raw.split_once('#').map_or(raw, |(left, _)| left).trim();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.split_whitespace();
        match parts.next() {
            Some("index") => {
                name = parts.next().map(str::to_string);
                if name.is_none() {
                    bail!("line {}: index requires a name", line_no + 1);
                }
            }
            Some("primary") => {
                primary = parts.next().map(to_snake_case);
                if primary.is_none() {
                    bail!("line {}: primary requires a field name", line_no + 1);
                }
            }
            Some("field") => {
                let field_name = parts.next().ok_or_else(|| {
                    anyhow::anyhow!("line {}: field requires a name", line_no + 1)
                })?;
                let field_type = parts.next().ok_or_else(|| {
                    anyhow::anyhow!("line {}: field requires a type", line_no + 1)
                })?;
                let ty = parse_search_type(field_type)?;
                let flags = parts.collect::<Vec<_>>();
                fields.push(SearchFieldSpec {
                    name: to_snake_case(field_name),
                    source_name: search_source_name(field_name),
                    ty,
                    searchable: flags.contains(&"searchable")
                        || matches!(ty, SearchFieldType::Text),
                    filterable: flags.contains(&"filterable"),
                    sortable: flags.contains(&"sortable"),
                    primary: flags.contains(&"primary"),
                });
            }
            Some(other) => bail!(
                "line {}: unknown search schema directive `{other}`",
                line_no + 1
            ),
            None => {}
        }
    }
    normalize_search_spec(SearchIndexSpec {
        name: name.ok_or_else(|| anyhow::anyhow!("search schema requires `index <name>`"))?,
        primary: primary.unwrap_or_else(|| "id".to_string()),
        fields,
    })
}

fn normalize_search_spec(mut spec: SearchIndexSpec) -> anyhow::Result<SearchIndexSpec> {
    if spec.fields.is_empty() {
        bail!("search schema requires at least one field");
    }
    let module = to_snake_case(&spec.name);
    if !is_valid_search_rust_ident(&module) {
        bail!(
            "search index `{}` generates invalid Rust module `{module}`",
            spec.name
        );
    }
    let pascal = to_pascal_case(&spec.name);
    if !is_valid_search_rust_type(&pascal) {
        bail!(
            "search index `{}` generates invalid Rust type `{pascal}`",
            spec.name
        );
    }

    let mut generated_fields = std::collections::BTreeMap::<String, String>::new();
    let mut has_primary = false;
    for field in &mut spec.fields {
        if !is_valid_search_rust_ident(&field.name) {
            bail!(
                "search field `{}` generates invalid Rust field `{}`",
                field.source_name.as_deref().unwrap_or(&field.name),
                field.name
            );
        }
        if let Some(previous) = generated_fields.insert(
            field.name.clone(),
            field
                .source_name
                .as_deref()
                .unwrap_or(&field.name)
                .to_string(),
        ) {
            let current = field.source_name.as_deref().unwrap_or(&field.name);
            bail!(
                "duplicate generated search field `{}`: {} and {}",
                field.name,
                previous,
                current
            );
        }
        if field.name == spec.primary {
            field.primary = true;
        }
        has_primary |= field.primary;
    }
    if !has_primary {
        bail!("search schema primary field `{}` not found", spec.primary);
    }
    Ok(spec)
}

fn is_valid_search_rust_type(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if name == "_" {
        return false;
    }
    (first == '_' || first.is_ascii_uppercase())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

fn is_valid_search_rust_ident(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if name == "_" {
        return false;
    }
    (first == '_' || first.is_ascii_lowercase())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
        && rust_identifier(name) == name
}

fn parse_search_type(value: &str) -> anyhow::Result<SearchFieldType> {
    match value {
        "keyword" | "string" => Ok(SearchFieldType::Keyword),
        "text" => Ok(SearchFieldType::Text),
        "i32" | "int" | "integer" => Ok(SearchFieldType::I32),
        "i64" | "long" => Ok(SearchFieldType::I64),
        "u64" | "uint64" | "unsigned_long" => Ok(SearchFieldType::U64),
        "f64" | "float" | "double" | "number" => Ok(SearchFieldType::F64),
        "bool" | "boolean" => Ok(SearchFieldType::Bool),
        "datetime" | "date" => Ok(SearchFieldType::DateTime),
        "json" | "object" => Ok(SearchFieldType::Json),
        other => bail!("unsupported search field type `{other}`"),
    }
}

async fn inspect_elastic_mapping(
    index: &str,
    url: &str,
    api_key: Option<&str>,
) -> anyhow::Result<SearchIndexSpec> {
    let value = http_json(
        reqwest::Method::GET,
        &format!("{}/{}/_mapping", url.trim_end_matches('/'), index),
        api_key,
        None,
    )
    .await?;
    let properties = value
        .get(index)
        .and_then(|index| index.get("mappings"))
        .and_then(|mappings| mappings.get("properties"))
        .or_else(|| {
            value
                .get("mappings")
                .and_then(|mappings| mappings.get("properties"))
        })
        .and_then(Value::as_object)
        .ok_or_else(|| anyhow::anyhow!("mapping for index `{index}` has no properties"))?;
    let fields = properties
        .iter()
        .map(|(name, field)| {
            let ty = field
                .get("type")
                .and_then(Value::as_str)
                .map(elastic_type)
                .unwrap_or(SearchFieldType::Json);
            Ok(SearchFieldSpec {
                name: to_snake_case(name),
                source_name: search_source_name(name),
                ty,
                searchable: matches!(ty, SearchFieldType::Text),
                filterable: !matches!(ty, SearchFieldType::Text | SearchFieldType::Json),
                sortable: !matches!(ty, SearchFieldType::Text | SearchFieldType::Json),
                primary: name == "id",
            })
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    normalize_search_spec(SearchIndexSpec {
        name: index.to_string(),
        primary: "id".to_string(),
        fields,
    })
}

fn elastic_type(value: &str) -> SearchFieldType {
    match value {
        "keyword" => SearchFieldType::Keyword,
        "text" => SearchFieldType::Text,
        "byte" | "short" | "integer" => SearchFieldType::I32,
        "long" => SearchFieldType::I64,
        "unsigned_long" => SearchFieldType::U64,
        "float" | "half_float" | "scaled_float" | "double" => SearchFieldType::F64,
        "boolean" => SearchFieldType::Bool,
        "date" | "date_nanos" => SearchFieldType::DateTime,
        _ => SearchFieldType::Json,
    }
}

async fn inspect_meilisearch_index(
    index: &str,
    url: &str,
    api_key: Option<&str>,
    sample_size: u64,
) -> anyhow::Result<SearchIndexSpec> {
    let base = url.trim_end_matches('/');
    let settings = http_json(
        reqwest::Method::GET,
        &format!("{base}/indexes/{index}/settings"),
        api_key,
        None,
    )
    .await?;
    let documents = http_json(
        reqwest::Method::GET,
        &format!(
            "{base}/indexes/{index}/documents?limit={}",
            sample_size.max(1)
        ),
        api_key,
        None,
    )
    .await?;
    let primary = http_json(
        reqwest::Method::GET,
        &format!("{base}/indexes/{index}"),
        api_key,
        None,
    )
    .await
    .ok()
    .and_then(|value| {
        value
            .get("primaryKey")
            .and_then(Value::as_str)
            .map(to_snake_case)
    })
    .unwrap_or_else(|| "id".to_string());
    let filterable = string_array(&settings, "filterableAttributes");
    let sortable = string_array(&settings, "sortableAttributes");
    let searchable = string_array(&settings, "searchableAttributes");
    let results = documents
        .get("results")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut names = results
        .iter()
        .filter_map(Value::as_object)
        .flat_map(|document| document.keys().cloned())
        .collect::<Vec<_>>();
    names.extend(filterable.iter().cloned());
    names.extend(sortable.iter().cloned());
    names.extend(searchable.iter().filter(|name| *name != "*").cloned());
    names.push(primary.clone());
    names.sort();
    names.dedup();
    let fields = names
        .into_iter()
        .map(|name| {
            let rust_name = to_snake_case(&name);
            let ty = infer_json_field_type(&results, &name);
            SearchFieldSpec {
                name: rust_name.clone(),
                source_name: Some(name.clone()).filter(|source| source != &rust_name),
                ty,
                searchable: searchable.contains(&name) || searchable.contains(&"*".to_string()),
                filterable: filterable.contains(&name),
                sortable: sortable.contains(&name),
                primary: rust_name == primary,
            }
        })
        .collect::<Vec<_>>();
    normalize_search_spec(SearchIndexSpec {
        name: index.to_string(),
        primary,
        fields,
    })
}

fn string_array(value: &Value, key: &str) -> Vec<String> {
    value
        .get(key)
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn infer_json_field_type(documents: &[Value], field: &str) -> SearchFieldType {
    let mut out = None;
    for value in documents
        .iter()
        .filter_map(Value::as_object)
        .filter_map(|document| document.get(field))
    {
        let next = match value {
            Value::Bool(_) => SearchFieldType::Bool,
            Value::Number(number) if number.as_i64().is_some() => SearchFieldType::I64,
            Value::Number(number) if number.as_u64().is_some() => SearchFieldType::U64,
            Value::Number(_) => SearchFieldType::F64,
            Value::String(_) => SearchFieldType::Text,
            _ => SearchFieldType::Json,
        };
        out = Some(match (out, next) {
            (None, next) => next,
            (Some(left), right) if left == right => left,
            (Some(SearchFieldType::I32), SearchFieldType::I64)
            | (Some(SearchFieldType::I64), SearchFieldType::I32) => SearchFieldType::I64,
            (Some(SearchFieldType::I32), SearchFieldType::U64)
            | (Some(SearchFieldType::U64), SearchFieldType::I32)
            | (Some(SearchFieldType::I64), SearchFieldType::U64)
            | (Some(SearchFieldType::U64), SearchFieldType::I64) => SearchFieldType::Json,
            (Some(SearchFieldType::I64), SearchFieldType::F64)
            | (Some(SearchFieldType::F64), SearchFieldType::I64)
            | (Some(SearchFieldType::I32), SearchFieldType::F64)
            | (Some(SearchFieldType::F64), SearchFieldType::I32)
            | (Some(SearchFieldType::U64), SearchFieldType::F64)
            | (Some(SearchFieldType::F64), SearchFieldType::U64) => SearchFieldType::F64,
            _ => SearchFieldType::Json,
        });
    }
    out.unwrap_or(SearchFieldType::Json)
}

async fn http_json(
    method: reqwest::Method,
    url: &str,
    api_key: Option<&str>,
    body: Option<Value>,
) -> anyhow::Result<Value> {
    let client = reqwest::Client::new();
    let mut request = client.request(method, url);
    if let Some(api_key) = api_key {
        request = request
            .bearer_auth(api_key)
            .header("X-Meili-API-Key", api_key);
    }
    if let Some(body) = body {
        request = request.json(&body);
    }
    Ok(request.send().await?.error_for_status()?.json().await?)
}

fn commit_search_project(
    spec: &SearchIndexSpec,
    engine: SearchEngine,
    out: &Path,
    options: GenerateOptions,
) -> anyhow::Result<()> {
    let plan = GenerationPlan::prepare_component(out)?;
    write_search_project(spec, engine, plan.staged(), out, options)?;
    sync_managed_service_if_present(plan.staged())?;
    plan.commit()
}

fn write_search_project(
    spec: &SearchIndexSpec,
    engine: SearchEngine,
    out: &Path,
    logical_out: &Path,
    options: GenerateOptions,
) -> anyhow::Result<()> {
    let search_dir = out.join("src/search");
    if options.mode == GenerateMode::Create && search_dir.exists() && has_entries(&search_dir)? {
        bail!(
            "{} already contains generated search files; pass --update to refresh them",
            search_dir.display()
        );
    }
    fs::create_dir_all(&search_dir)?;
    update_search_mod(&search_dir, spec)?;
    fs::write(
        search_dir.join(format!("{}.rs", to_snake_case(&spec.name))),
        render_search_index(spec, engine),
    )?;
    update_search_dependencies(out, logical_out, options.dependency_source)?;
    update_main_rs(out)?;
    Ok(())
}

fn update_search_mod(search_dir: &Path, spec: &SearchIndexSpec) -> anyhow::Result<()> {
    let path = search_dir.join("mod.rs");
    let mut modules = if path.is_file() {
        fs::read_to_string(&path)?
            .lines()
            .filter_map(|line| line.trim().strip_prefix("pub mod "))
            .filter_map(|line| line.strip_suffix(';'))
            .map(str::to_string)
            .collect::<BTreeSet<_>>()
    } else {
        BTreeSet::new()
    };
    modules.insert(to_snake_case(&spec.name));
    let mut out = String::from("#![allow(unused_imports)]\n\n");
    use std::fmt::Write as _;
    for module in &modules {
        writeln!(&mut out, "pub mod {module};").unwrap();
    }
    writeln!(&mut out).unwrap();
    for module in &modules {
        let pascal = to_pascal_case(module);
        writeln!(
            &mut out,
            "pub use {module}::{{{pascal}Document, {pascal}SearchRepository}};"
        )
        .unwrap();
    }
    fs::write(path, out)?;
    Ok(())
}

fn render_search_index(spec: &SearchIndexSpec, engine: SearchEngine) -> String {
    let pascal = to_pascal_case(&spec.name);
    let primary = spec
        .fields
        .iter()
        .find(|field| field.primary)
        .expect("primary field normalized");
    let mut out = String::new();
    use std::fmt::Write as _;
    writeln!(&mut out, "#![allow(dead_code)]").unwrap();
    writeln!(&mut out).unwrap();
    writeln!(&mut out, "use serde::{{Deserialize, Serialize}};").unwrap();
    writeln!(&mut out, "use serde_json::json;").unwrap();
    writeln!(
        &mut out,
        "use roze_search::{{SearchClient, SearchConfig, SearchEngine, SearchFilter, SearchIndexSettings, SearchPage, SearchRequest, SearchTask}};"
    )
    .unwrap();
    writeln!(&mut out).unwrap();
    writeln!(&mut out, "#[derive(Clone, Debug, Serialize, Deserialize)]").unwrap();
    writeln!(&mut out, "pub struct {pascal}Document {{").unwrap();
    for field in &spec.fields {
        if let Some(source_name) = &field.source_name {
            writeln!(&mut out, "    #[serde(rename = \"{source_name}\")]").unwrap();
        }
        writeln!(
            &mut out,
            "    pub {}: {},",
            field.name,
            field.ty.rust_type()
        )
        .unwrap();
    }
    writeln!(&mut out, "}}").unwrap();
    writeln!(&mut out).unwrap();
    writeln!(&mut out, "pub struct {pascal}SearchRepository {{").unwrap();
    writeln!(&mut out, "    client: SearchClient,").unwrap();
    writeln!(&mut out, "}}").unwrap();
    writeln!(&mut out).unwrap();
    writeln!(&mut out, "impl {pascal}SearchRepository {{").unwrap();
    writeln!(
        &mut out,
        "    pub const INDEX: &'static str = \"{}\";",
        spec.name
    )
    .unwrap();
    let searchable = spec
        .fields
        .iter()
        .filter(|field| field.searchable)
        .map(search_field_name)
        .collect::<Vec<_>>();
    let filterable = spec
        .fields
        .iter()
        .filter(|field| field.filterable)
        .map(search_field_name)
        .collect::<Vec<_>>();
    let sortable = spec
        .fields
        .iter()
        .filter(|field| field.sortable)
        .map(search_field_name)
        .collect::<Vec<_>>();
    let attributes = spec
        .fields
        .iter()
        .map(search_field_name)
        .collect::<Vec<_>>();
    writeln!(
        &mut out,
        "    pub const SEARCHABLE_FIELDS: &'static [&'static str] = &{searchable:?};"
    )
    .unwrap();
    writeln!(
        &mut out,
        "    pub const FILTERABLE_FIELDS: &'static [&'static str] = &{filterable:?};"
    )
    .unwrap();
    writeln!(
        &mut out,
        "    pub const SORTABLE_FIELDS: &'static [&'static str] = &{sortable:?};"
    )
    .unwrap();
    writeln!(
        &mut out,
        "    pub const ATTRIBUTES: &'static [&'static str] = &{attributes:?};"
    )
    .unwrap();
    writeln!(&mut out).unwrap();
    writeln!(
        &mut out,
        "    pub fn new(url: impl Into<String>, api_key: Option<String>) -> Self {{"
    )
    .unwrap();
    writeln!(&mut out, "        Self {{").unwrap();
    writeln!(
        &mut out,
        "            client: SearchClient::new(SearchConfig {{"
    )
    .unwrap();
    writeln!(
        &mut out,
        "                engine: SearchEngine::{},",
        match engine {
            SearchEngine::Elasticsearch => "Elasticsearch",
            SearchEngine::Opensearch => "Opensearch",
            SearchEngine::Meilisearch => "Meilisearch",
        }
    )
    .unwrap();
    writeln!(&mut out, "                url: url.into(),").unwrap();
    writeln!(&mut out, "                api_key,").unwrap();
    writeln!(&mut out, "            }}),").unwrap();
    writeln!(&mut out, "        }}").unwrap();
    writeln!(&mut out, "    }}").unwrap();
    writeln!(&mut out).unwrap();
    writeln!(&mut out, "    pub fn client(&self) -> &SearchClient {{").unwrap();
    writeln!(&mut out, "        &self.client").unwrap();
    writeln!(&mut out, "    }}").unwrap();
    writeln!(&mut out).unwrap();
    writeln!(
        &mut out,
        "    pub async fn initialize(&self, timeout: std::time::Duration) -> anyhow::Result<()> {{"
    )
    .unwrap();
    writeln!(
        &mut out,
        "        self.client.ensure_index(Self::INDEX, {:?}).await?.wait(timeout).await?;",
        search_field_name(primary)
    )
    .unwrap();
    writeln!(
        &mut out,
        "        self.client.apply_settings(Self::INDEX, &SearchIndexSettings {{"
    )
    .unwrap();
    writeln!(&mut out, "            searchable_attributes: Self::SEARCHABLE_FIELDS.iter().map(|field| (*field).to_string()).collect(),").unwrap();
    writeln!(&mut out, "            filterable_attributes: Self::FILTERABLE_FIELDS.iter().map(|field| (*field).to_string()).collect(),").unwrap();
    writeln!(&mut out, "            sortable_attributes: Self::SORTABLE_FIELDS.iter().map(|field| (*field).to_string()).collect(),").unwrap();
    writeln!(&mut out, "        }}).await?.wait(timeout).await?;").unwrap();
    writeln!(&mut out, "        Ok(())").unwrap();
    writeln!(&mut out, "    }}").unwrap();
    writeln!(&mut out).unwrap();
    writeln!(
        &mut out,
        "    pub async fn health(&self) -> anyhow::Result<serde_json::Value> {{"
    )
    .unwrap();
    writeln!(&mut out, "        self.client.health().await").unwrap();
    writeln!(&mut out, "    }}").unwrap();
    writeln!(&mut out).unwrap();
    writeln!(
        &mut out,
        "    pub async fn index(&self, document: &{pascal}Document) -> anyhow::Result<serde_json::Value> {{"
    )
    .unwrap();
    writeln!(&mut out, "        self.client").unwrap();
    writeln!(
        &mut out,
        "            .index_document(Self::INDEX, &document.{}.to_string(), document)",
        primary.name
    )
    .unwrap();
    writeln!(&mut out, "            .await").unwrap();
    writeln!(&mut out, "    }}").unwrap();
    writeln!(&mut out).unwrap();
    writeln!(
        &mut out,
        "    pub async fn index_task(&self, document: &{pascal}Document) -> anyhow::Result<SearchTask> {{"
    )
    .unwrap();
    writeln!(
        &mut out,
        "        self.client.index_document_task(Self::INDEX, &document.{}.to_string(), document).await",
        primary.name
    )
    .unwrap();
    writeln!(&mut out, "    }}").unwrap();
    writeln!(&mut out).unwrap();
    writeln!(
        &mut out,
        "    pub async fn delete(&self, id: impl ToString) -> anyhow::Result<serde_json::Value> {{"
    )
    .unwrap();
    writeln!(
        &mut out,
        "        self.client.delete_document(Self::INDEX, &id.to_string()).await"
    )
    .unwrap();
    writeln!(&mut out, "    }}").unwrap();
    writeln!(&mut out).unwrap();
    writeln!(
        &mut out,
        "    pub async fn delete_task(&self, id: impl ToString) -> anyhow::Result<SearchTask> {{"
    )
    .unwrap();
    writeln!(
        &mut out,
        "        self.client.delete_document_task(Self::INDEX, &id.to_string()).await"
    )
    .unwrap();
    writeln!(&mut out, "    }}").unwrap();
    writeln!(&mut out).unwrap();
    writeln!(
        &mut out,
        "    pub async fn delete_all(&self) -> anyhow::Result<SearchTask> {{"
    )
    .unwrap();
    writeln!(
        &mut out,
        "        self.client.delete_all(Self::INDEX).await"
    )
    .unwrap();
    writeln!(&mut out, "    }}").unwrap();
    writeln!(&mut out).unwrap();
    writeln!(&mut out, "    pub async fn delete_by_filter(&self, filters: &[SearchFilter]) -> anyhow::Result<SearchTask> {{").unwrap();
    writeln!(&mut out, "        Self::validate_filters(filters)?;").unwrap();
    writeln!(
        &mut out,
        "        self.client.delete_by_filter(Self::INDEX, filters).await"
    )
    .unwrap();
    writeln!(&mut out, "    }}").unwrap();
    writeln!(&mut out).unwrap();
    writeln!(&mut out, "    pub async fn search(&self, request: SearchRequest) -> anyhow::Result<SearchPage<{pascal}Document>> {{").unwrap();
    writeln!(
        &mut out,
        "        Self::validate_filters(&request.filters)?;"
    )
    .unwrap();
    writeln!(&mut out, "        for sort in &request.sort {{ if !Self::SORTABLE_FIELDS.contains(&sort.field.as_str()) {{ anyhow::bail!(\"search field `{{}}` is not sortable\", sort.field); }} }}").unwrap();
    writeln!(&mut out, "        for attribute in &request.attributes {{ if !Self::ATTRIBUTES.contains(&attribute.as_str()) {{ anyhow::bail!(\"search attribute `{{attribute}}` is not declared\"); }} }}").unwrap();
    writeln!(
        &mut out,
        "        self.client.search_page(Self::INDEX, &request).await"
    )
    .unwrap();
    writeln!(&mut out, "    }}").unwrap();
    writeln!(&mut out).unwrap();
    writeln!(
        &mut out,
        "    fn validate_filters(filters: &[SearchFilter]) -> anyhow::Result<()> {{"
    )
    .unwrap();
    writeln!(&mut out, "        for filter in filters {{ if !Self::FILTERABLE_FIELDS.contains(&filter.field.as_str()) {{ anyhow::bail!(\"search field `{{}}` is not filterable\", filter.field); }} }}").unwrap();
    writeln!(&mut out, "        Ok(())").unwrap();
    writeln!(&mut out, "    }}").unwrap();
    writeln!(&mut out).unwrap();
    writeln!(
        &mut out,
        "    pub async fn search_text(&self, query: &str) -> anyhow::Result<serde_json::Value> {{"
    )
    .unwrap();
    match engine {
        SearchEngine::Elasticsearch | SearchEngine::Opensearch => {
            let fields = spec
                .fields
                .iter()
                .filter(|field| field.searchable)
                .map(search_field_name)
                .collect::<Vec<_>>();
            writeln!(&mut out, "        self.client").unwrap();
            writeln!(&mut out, "            .search(").unwrap();
            writeln!(&mut out, "                Self::INDEX,").unwrap();
            writeln!(
                &mut out,
                "                json!({{ \"query\": {{ \"multi_match\": {{ \"query\": query, \"fields\": {:?} }} }} }}),",
                fields
            )
            .unwrap();
            writeln!(&mut out, "            )").unwrap();
            writeln!(&mut out, "            .await").unwrap();
        }
        SearchEngine::Meilisearch => {
            writeln!(&mut out, "        self.client").unwrap();
            writeln!(
                &mut out,
                "            .search(Self::INDEX, json!({{ \"q\": query }}))"
            )
            .unwrap();
            writeln!(&mut out, "            .await").unwrap();
        }
    }
    writeln!(&mut out, "    }}").unwrap();
    writeln!(&mut out, "}}").unwrap();
    out
}

fn search_source_name(name: &str) -> Option<String> {
    let rust_name = to_snake_case(name);
    Some(name.to_string()).filter(|source| source != &rust_name)
}

fn search_field_name(field: &SearchFieldSpec) -> &str {
    field.source_name.as_deref().unwrap_or(&field.name)
}

fn update_search_dependencies(
    out: &Path,
    logical_out: &Path,
    source: DependencySource,
) -> anyhow::Result<()> {
    let manifest_path = out.join("Cargo.toml");
    if !manifest_path.is_file() {
        return Ok(());
    }
    let content = fs::read_to_string(&manifest_path)
        .with_context(|| format!("failed to read {}", manifest_path.display()))?;
    let mut document = content.parse::<toml_edit::DocumentMut>()?;
    let dependencies = document
        .get_mut("dependencies")
        .and_then(toml_edit::Item::as_table_mut)
        .ok_or_else(|| {
            anyhow::anyhow!("{} has no [dependencies] table", manifest_path.display())
        })?;
    validate_roze_dependency_sources(dependencies)?;
    let uses_workspace = content.contains("edition.workspace = true");
    insert_dependency(dependencies, "anyhow", uses_workspace, None);
    insert_dependency(
        dependencies,
        "serde",
        uses_workspace,
        Some(r#"{ version = "1", features = ["derive"] }"#),
    );
    insert_dependency(dependencies, "serde_json", uses_workspace, Some(r#""1""#));
    if !dependencies.contains_key("roze-search") {
        let inherited = inherited_roze_dependency(dependencies, "roze-search")?;
        let item = if let Some(item) = inherited {
            item
        } else {
            match source {
                DependencySource::Git => r#"{ git = "https://github.com/roze-team/roze.git" }"#
                    .parse::<toml_edit::Item>()?,
                DependencySource::Path => {
                    let workspace_root = find_workspace_root(logical_out)?.ok_or_else(|| {
                    anyhow::anyhow!(
                        "--roze-source path requires output inside a Cargo workspace containing Roze crates"
                    )
                })?;
                    let prefix = local_crates_prefix(logical_out, &workspace_root)?;
                    format!(r#"{{ path = "{prefix}/roze-search" }}"#).parse::<toml_edit::Item>()?
                }
            }
        };
        dependencies.insert("roze-search", item);
    }
    fs::write(&manifest_path, document.to_string())?;
    Ok(())
}

fn insert_dependency(
    dependencies: &mut toml_edit::Table,
    name: &str,
    uses_workspace: bool,
    standalone: Option<&str>,
) {
    if dependencies.contains_key(name) {
        return;
    }
    let item = if uses_workspace {
        let mut table = toml_edit::InlineTable::new();
        table.insert("workspace", true.into());
        toml_edit::Item::Value(toml_edit::Value::InlineTable(table))
    } else {
        standalone
            .unwrap_or(r#""1""#)
            .parse()
            .expect("valid dependency")
    };
    dependencies.insert(name, item);
}

fn update_main_rs(out: &Path) -> anyhow::Result<()> {
    let main_path = out.join("src/main.rs");
    if !main_path.is_file() {
        return Ok(());
    }
    let content = fs::read_to_string(&main_path)
        .with_context(|| format!("failed to read {}", main_path.display()))?;
    if content.contains("mod search;") {
        return Ok(());
    }
    let updated = if let Some(index) = content.find("mod model;\n") {
        let insert_at = index + "mod model;\n".len();
        format!(
            "{}mod search;\n{}",
            &content[..insert_at],
            &content[insert_at..]
        )
    } else if let Some(index) = content.find("mod types;\n") {
        let insert_at = index + "mod types;\n".len();
        format!(
            "{}mod search;\n{}",
            &content[..insert_at],
            &content[insert_at..]
        )
    } else {
        format!("mod search;\n{content}")
    };
    fs::write(&main_path, updated)
        .with_context(|| format!("failed to write {}", main_path.display()))
}

fn has_entries(path: &Path) -> anyhow::Result<bool> {
    Ok(fs::read_dir(path)?.next().is_some())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_search_dsl() {
        let spec = parse_search_schema(
            r#"
            index users
            primary id
            field id keyword primary filterable sortable
            field name text searchable
            field age i64 filterable sortable
            "#,
        )
        .expect("parse");
        assert_eq!(spec.name, "users");
        assert_eq!(spec.primary, "id");
        assert_eq!(spec.fields.len(), 3);
        assert!(spec.fields[1].searchable);
    }

    #[test]
    fn renders_elasticsearch_repository() {
        let spec = parse_search_schema(
            r#"
            index users
            primary id
            field id keyword primary filterable sortable
            field display-name text searchable
            "#,
        )
        .expect("parse");
        let rendered = render_search_index(&spec, SearchEngine::Elasticsearch);
        assert!(rendered.contains("SearchEngine::Elasticsearch"));
        assert!(rendered.contains("pub struct UsersDocument"));
        assert!(rendered.contains("#[serde(rename = \"display-name\")]"));
        assert!(rendered.contains("multi_match"));
        assert!(rendered.contains("\"display-name\""));
        assert!(rendered.contains("pub async fn health"));
        assert!(rendered.contains("pub async fn initialize"));
        assert!(rendered.contains("pub async fn search(&self, request: SearchRequest)"));
        assert!(rendered.contains("FILTERABLE_FIELDS"));
    }

    #[test]
    fn search_module_index_merges_multiple_schemas_deterministically() {
        let dir = tempfile::tempdir().expect("tempdir");
        let search_dir = dir.path().join("search");
        fs::create_dir_all(&search_dir).expect("search dir");
        let doctors = parse_search_schema("index doctors\nprimary id\nfield id keyword primary")
            .expect("doctors");
        let hospitals =
            parse_search_schema("index hospitals\nprimary id\nfield id keyword primary")
                .expect("hospitals");

        update_search_mod(&search_dir, &hospitals).expect("first");
        update_search_mod(&search_dir, &doctors).expect("second");
        let merged = fs::read_to_string(search_dir.join("mod.rs")).expect("mod");
        assert!(merged.contains("pub mod doctors;"));
        assert!(merged.contains("pub mod hospitals;"));
        assert!(merged.find("doctors").unwrap() < merged.find("hospitals").unwrap());
    }

    #[test]
    fn new_roze_dependency_inherits_existing_git_revision() {
        let document = r#"
            [dependencies]
            roze-rpc = { git = "https://github.com/roze-team/roze.git", rev = "12c63307" }
        "#
        .parse::<toml_edit::DocumentMut>()
        .expect("manifest");
        let dependencies = document["dependencies"].as_table().expect("dependencies");
        let inherited = inherited_roze_dependency(dependencies, "roze-search")
            .expect("inherit")
            .expect("source");
        assert!(inherited.to_string().contains("rev = \"12c63307\""));
    }

    #[test]
    fn roze_dependency_inheritance_preserves_tag_branch_workspace_and_path() {
        for (dependency, expected) in [
            (
                r#"roze-rpc = { git = "https://github.com/roze-team/roze.git", tag = "v1.0.0" }"#,
                "tag = \"v1.0.0\"",
            ),
            (
                r#"roze-rpc = { git = "https://github.com/roze-team/roze.git", branch = "release" }"#,
                "branch = \"release\"",
            ),
            (r#"roze-rpc = { workspace = true }"#, "workspace = true"),
            (
                r#"roze-rpc = { path = "../../crates/roze-rpc" }"#,
                "../../crates/roze-search",
            ),
        ] {
            let document = format!("[dependencies]\n{dependency}\n")
                .parse::<toml_edit::DocumentMut>()
                .expect("manifest");
            let dependencies = document["dependencies"].as_table().expect("dependencies");
            let inherited = inherited_roze_dependency(dependencies, "roze-search")
                .expect("inherit")
                .expect("source");
            assert!(inherited.to_string().contains(expected), "{inherited}");
        }
    }

    #[test]
    fn conflicting_roze_revisions_fail_instead_of_floating() {
        let document = r#"
            [dependencies]
            roze-rpc = { git = "https://github.com/roze-team/roze.git", rev = "aaaa" }
            roze-http = { git = "https://github.com/roze-team/roze.git", rev = "bbbb" }
        "#
        .parse::<toml_edit::DocumentMut>()
        .expect("manifest");
        let dependencies = document["dependencies"].as_table().expect("dependencies");
        let error = inherited_roze_dependency(dependencies, "roze-search")
            .expect_err("mixed revisions must fail");
        assert!(error
            .to_string()
            .contains("conflicting Roze dependency sources"));
    }

    #[test]
    fn parses_json_search_schema_with_kind() {
        let spec = parse_search_schema(
            r#"
            {
              "index": "users",
              "primary": "display-id",
              "fields": [
                { "name": "display-id", "kind": "keyword", "primary": true, "filterable": true, "sortable": true },
                { "name": "view_count", "kind": "u64", "sortable": true },
                { "name": "name", "kind": "text", "searchable": true }
              ]
            }
            "#,
        )
        .expect("parse");
        assert_eq!(spec.primary, "display_id");
        assert_eq!(spec.fields[0].name, "display_id");
        assert_eq!(spec.fields[0].source_name.as_deref(), Some("display-id"));
        assert_eq!(spec.fields[1].ty, SearchFieldType::U64);
    }

    #[test]
    fn rejects_invalid_generated_search_rust_names() {
        let invalid_index = parse_search_schema(
            r#"
            index 123-users
            primary id
            field id keyword primary
            "#,
        )
        .expect_err("invalid index");
        assert!(invalid_index
            .to_string()
            .contains("search index `123-users` generates invalid Rust module `123_users`"));

        let underscore_index = parse_search_schema(
            r#"
            index _
            primary id
            field id keyword primary
            "#,
        )
        .expect_err("invalid underscore index");
        assert!(underscore_index
            .to_string()
            .contains("search index `_` generates invalid Rust module `_`"));

        let invalid_field = parse_search_schema(
            r#"
            index users
            primary id
            field id keyword primary
            field type keyword
            "#,
        )
        .expect_err("invalid field");
        assert!(invalid_field
            .to_string()
            .contains("search field `type` generates invalid Rust field `type`"));

        let underscore_field = parse_search_schema(
            r#"
            index users
            primary id
            field id keyword primary
            field _ keyword
            "#,
        )
        .expect_err("invalid underscore field");
        assert!(underscore_field
            .to_string()
            .contains("search field `_` generates invalid Rust field `_`"));

        let duplicate_field = parse_search_schema(
            r#"
            {
              "index": "users",
              "primary": "id",
              "fields": [
                { "name": "id", "kind": "keyword", "primary": true },
                { "name": "display-name", "kind": "text" },
                { "name": "display_name", "kind": "text" }
              ]
            }
            "#,
        )
        .expect_err("duplicate field");
        assert!(duplicate_field.to_string().contains(
            "duplicate generated search field `display_name`: display-name and display_name"
        ));
    }

    #[test]
    fn infers_meili_sample_types() {
        let docs = vec![json!({"id": 1, "name": "Alice", "active": true})];
        assert_eq!(infer_json_field_type(&docs, "id"), SearchFieldType::I64);
        assert_eq!(
            infer_json_field_type(&docs, "active"),
            SearchFieldType::Bool
        );
    }
}
