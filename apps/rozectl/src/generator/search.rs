use std::{fs, path::Path};

use anyhow::{bail, Context};
use serde_json::Value;

use super::{
    find_workspace_root, local_crates_prefix, to_pascal_case, to_snake_case, DependencySource,
    GenerateMode, GenerateOptions,
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
    write_search_project(&spec, engine, out, options)
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
    write_search_project(&spec, engine, out, options)
}

fn parse_search_schema(source: &str) -> anyhow::Result<SearchIndexSpec> {
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
    let mut has_primary = false;
    for field in &mut spec.fields {
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

fn write_search_project(
    spec: &SearchIndexSpec,
    engine: SearchEngine,
    out: &Path,
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
    fs::write(search_dir.join("mod.rs"), render_search_mod(spec))?;
    fs::write(
        search_dir.join(format!("{}.rs", to_snake_case(&spec.name))),
        render_search_index(spec, engine),
    )?;
    update_search_dependencies(out, options.dependency_source)?;
    update_main_rs(out)?;
    Ok(())
}

fn render_search_mod(spec: &SearchIndexSpec) -> String {
    let module = to_snake_case(&spec.name);
    let pascal = to_pascal_case(&spec.name);
    format!(
        "#![allow(unused_imports)]\n\npub mod {module};\n\npub use {module}::{{{pascal}Document, {pascal}SearchRepository}};\n"
    )
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
        "use roze_search::{{SearchClient, SearchConfig, SearchEngine}};"
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

fn update_search_dependencies(out: &Path, source: DependencySource) -> anyhow::Result<()> {
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
        let item = match source {
            DependencySource::Git => {
                r#"{ git = "https://github.com/roze-team/roze.git" }"#.parse::<toml_edit::Item>()?
            }
            DependencySource::Path => {
                let workspace_root = find_workspace_root(out)?.ok_or_else(|| {
                    anyhow::anyhow!(
                        "--roze-source path requires output inside a Cargo workspace containing Roze crates"
                    )
                })?;
                let prefix = local_crates_prefix(out, &workspace_root)?;
                format!(r#"{{ path = "{prefix}/roze-search" }}"#).parse::<toml_edit::Item>()?
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
    fn infers_meili_sample_types() {
        let docs = vec![json!({"id": 1, "name": "Alice", "active": true})];
        assert_eq!(infer_json_field_type(&docs, "id"), SearchFieldType::I64);
        assert_eq!(
            infer_json_field_type(&docs, "active"),
            SearchFieldType::Bool
        );
    }
}
