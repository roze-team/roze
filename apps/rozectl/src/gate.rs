use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::Context;
use config::{Config, File};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    generator::{self, model::ModelFormat},
    parser::{self, ApiSpec, Field, HttpMethod},
};

pub const GATE_MANIFEST_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateSeverity {
    Additive,
    Safe,
    Behavioral,
    OnlineRisk,
    Breaking,
    Destructive,
}

impl GateSeverity {
    pub fn blocks_release(self) -> bool {
        matches!(self, Self::OnlineRisk | Self::Breaking | Self::Destructive)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateIssue {
    pub code: String,
    pub domain: String,
    pub severity: GateSeverity,
    pub path: String,
    pub before: Option<String>,
    pub after: Option<String>,
    pub remediation: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub acknowledged_by: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateCheckReport {
    pub domain: GateDomain,
    pub old: String,
    pub new: String,
    pub old_digest: String,
    pub new_digest: String,
    pub issues: Vec<GateIssue>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateReport {
    pub version: u32,
    pub passed: bool,
    pub blocking_issues: usize,
    pub checks: Vec<GateCheckReport>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateDomain {
    Api,
    Search,
    Sql,
}

impl GateDomain {
    fn label(self) -> &'static str {
        match self {
            Self::Api => "api",
            Self::Search => "search",
            Self::Sql => "sql",
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GateManifest {
    pub version: u32,
    #[serde(default)]
    pub checks: Vec<GateCheck>,
    #[serde(default)]
    pub acknowledgements: Vec<PathBuf>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GateCheck {
    pub domain: GateDomain,
    pub old: PathBuf,
    pub new: PathBuf,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GateAcknowledgement {
    pub version: u32,
    pub id: String,
    pub scope: GateDomain,
    pub old_digest: String,
    pub new_digest: String,
    pub owner: String,
    pub reason: String,
    pub migration_plan: String,
    pub rollback_plan: String,
    pub expires_at: String,
}

#[derive(Debug, thiserror::Error)]
#[error("gate input error: {0}")]
pub struct GateInputError(pub String);

#[derive(Debug, thiserror::Error)]
#[error("gate blocked release with {0} unacknowledged issue(s)")]
pub struct GateBlockedError(pub usize);

pub fn run_manifest(path: &Path) -> anyhow::Result<GateReport> {
    let manifest: GateManifest = load_yaml(path)?;
    if manifest.version != GATE_MANIFEST_VERSION {
        return Err(GateInputError(format!(
            "unsupported manifest version {}; expected {GATE_MANIFEST_VERSION}",
            manifest.version
        ))
        .into());
    }
    if manifest.checks.is_empty() {
        return Err(GateInputError("manifest must contain at least one check".into()).into());
    }

    let base = path.parent().unwrap_or_else(|| Path::new("."));
    let acknowledgements = manifest
        .acknowledgements
        .iter()
        .map(|ack| {
            let path = resolve(base, ack);
            let ack: GateAcknowledgement = load_yaml(&path)?;
            validate_acknowledgement(&ack)
                .with_context(|| format!("invalid acknowledgement {}", path.display()))?;
            Ok(ack)
        })
        .collect::<anyhow::Result<Vec<_>>>()?;

    let mut reports = Vec::with_capacity(manifest.checks.len());
    for check in manifest.checks {
        let old = resolve(base, &check.old);
        let new = resolve(base, &check.new);
        let old_source = read_input(&old)?;
        let new_source = read_input(&new)?;
        let old_digest = content_digest(old_source.as_bytes());
        let new_digest = content_digest(new_source.as_bytes());
        let mut issues = match check.domain {
            GateDomain::Api => diff_api_sources(&old_source, &new_source)?,
            GateDomain::Search => diff_search_sources(&old_source, &new_source)?,
            GateDomain::Sql => diff_sql_sources(&old_source, &new_source)?,
        };
        for issue in &mut issues {
            if issue.severity.blocks_release() {
                issue.acknowledged_by = acknowledgements
                    .iter()
                    .find(|ack| {
                        ack.scope == check.domain
                            && ack.old_digest == old_digest
                            && ack.new_digest == new_digest
                    })
                    .map(|ack| ack.id.clone());
            }
        }
        issues.sort_by(|left, right| {
            (&left.domain, &left.path, &left.code).cmp(&(&right.domain, &right.path, &right.code))
        });
        reports.push(GateCheckReport {
            domain: check.domain,
            old: check.old.to_string_lossy().replace('\\', "/"),
            new: check.new.to_string_lossy().replace('\\', "/"),
            old_digest,
            new_digest,
            issues,
        });
    }

    reports.sort_by_key(|report| (report.domain, report.old.clone(), report.new.clone()));
    let blocking_issues = reports
        .iter()
        .flat_map(|report| &report.issues)
        .filter(|issue| issue.severity.blocks_release() && issue.acknowledged_by.is_none())
        .count();
    Ok(GateReport {
        version: GATE_MANIFEST_VERSION,
        passed: blocking_issues == 0,
        blocking_issues,
        checks: reports,
    })
}

pub fn write_json_report(report: &GateReport, path: &Path) -> anyhow::Result<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let mut json = serde_json::to_string_pretty(report)?;
    json.push('\n');
    fs::write(path, json).with_context(|| format!("failed to write {}", path.display()))
}

pub fn render_markdown(report: &GateReport) -> String {
    use std::fmt::Write as _;

    let mut out = String::from("# Roze Release Gate Report\n\n");
    writeln!(
        &mut out,
        "Status: **{}**",
        if report.passed { "pass" } else { "blocked" }
    )
    .unwrap();
    writeln!(&mut out, "\nBlocking issues: {}\n", report.blocking_issues).unwrap();
    for check in &report.checks {
        writeln!(
            &mut out,
            "## {}: `{}` -> `{}`\n",
            check.domain.label(),
            check.old,
            check.new
        )
        .unwrap();
        writeln!(&mut out, "- Old digest: `{}`", check.old_digest).unwrap();
        writeln!(&mut out, "- New digest: `{}`", check.new_digest).unwrap();
        if check.issues.is_empty() {
            writeln!(&mut out, "- No semantic changes\n").unwrap();
            continue;
        }
        for issue in &check.issues {
            let ack = issue
                .acknowledged_by
                .as_deref()
                .map(|id| format!("; acknowledged by `{id}`"))
                .unwrap_or_default();
            writeln!(
                &mut out,
                "- `{:?}` `{}` `{}`{}: {} -> {}",
                issue.severity,
                issue.code,
                issue.path,
                ack,
                issue.before.as_deref().unwrap_or("<missing>"),
                issue.after.as_deref().unwrap_or("<missing>")
            )
            .unwrap();
        }
        writeln!(&mut out).unwrap();
    }
    out
}

pub fn content_digest(content: &[u8]) -> String {
    let digest = Sha256::digest(content);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        write!(&mut out, "{byte:02x}").unwrap();
    }
    out
}

fn load_yaml<T: for<'de> Deserialize<'de>>(path: &Path) -> anyhow::Result<T> {
    if !path.is_file() {
        return Err(GateInputError(format!("{} does not exist", path.display())).into());
    }
    Config::builder()
        .add_source(File::from(path).format(config::FileFormat::Yaml))
        .build()
        .and_then(Config::try_deserialize)
        .map_err(|err| GateInputError(format!("failed to parse {}: {err}", path.display())).into())
}

fn resolve(base: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    }
}

fn read_input(path: &Path) -> anyhow::Result<String> {
    fs::read_to_string(path)
        .map_err(|err| GateInputError(format!("failed to read {}: {err}", path.display())).into())
}

fn validate_acknowledgement(ack: &GateAcknowledgement) -> anyhow::Result<()> {
    if ack.version != GATE_MANIFEST_VERSION {
        anyhow::bail!(GateInputError(format!(
            "acknowledgement {} has unsupported version {}",
            ack.id, ack.version
        )));
    }
    for (name, value) in [
        ("id", ack.id.as_str()),
        ("owner", ack.owner.as_str()),
        ("reason", ack.reason.as_str()),
        ("migration_plan", ack.migration_plan.as_str()),
        ("rollback_plan", ack.rollback_plan.as_str()),
    ] {
        if value.trim().is_empty() {
            anyhow::bail!(GateInputError(format!(
                "acknowledgement {} requires non-empty {name}",
                ack.id
            )));
        }
    }
    if !is_sha256(&ack.old_digest) || !is_sha256(&ack.new_digest) {
        anyhow::bail!(GateInputError(format!(
            "acknowledgement {} requires lowercase SHA-256 digests",
            ack.id
        )));
    }
    let expiry = parse_expiry_date(&ack.expires_at).ok_or_else(|| {
        GateInputError(format!(
            "acknowledgement {} expires_at must start with YYYY-MM-DD",
            ack.id
        ))
    })?;
    if expiry < current_unix_day() {
        anyhow::bail!(GateInputError(format!(
            "acknowledgement {} expired at {}",
            ack.id, ack.expires_at
        )));
    }
    Ok(())
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn parse_expiry_date(value: &str) -> Option<i64> {
    let date = value.get(..10)?;
    let mut parts = date.split('-');
    let year = parts.next()?.parse::<i64>().ok()?;
    let month = parts.next()?.parse::<i64>().ok()?;
    let day = parts.next()?.parse::<i64>().ok()?;
    if parts.next().is_some() || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    Some(days_from_civil(year, month, day))
}

fn days_from_civil(mut year: i64, month: i64, day: i64) -> i64 {
    year -= i64::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let adjusted_month = month + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * adjusted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

fn current_unix_day() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .div_euclid(86_400) as i64
}

pub fn diff_api_sources(old: &str, new: &str) -> anyhow::Result<Vec<GateIssue>> {
    let old =
        parser::parse_api(old).map_err(|err| GateInputError(format!("invalid old API: {err}")))?;
    let new =
        parser::parse_api(new).map_err(|err| GateInputError(format!("invalid new API: {err}")))?;
    let mut issues = diff_api(&old, &new);
    diff_openapi(&old, &new, &mut issues);
    Ok(issues)
}

fn diff_api(old: &ApiSpec, new: &ApiSpec) -> Vec<GateIssue> {
    let mut issues = Vec::new();
    let old_routes = api_routes(old);
    let new_routes = api_routes(new);
    diff_named_maps("api", "route", &old_routes, &new_routes, &mut issues);

    let old_types = old
        .types
        .iter()
        .map(|ty| (ty.name.as_str(), ty))
        .collect::<BTreeMap<_, _>>();
    let new_types = new
        .types
        .iter()
        .map(|ty| (ty.name.as_str(), ty))
        .collect::<BTreeMap<_, _>>();
    for (name, old_ty) in &old_types {
        let Some(new_ty) = new_types.get(name) else {
            issues.push(issue(
                "api.type.removed",
                "api",
                GateSeverity::Breaking,
                format!("types.{name}"),
                Some("present"),
                None::<String>,
                "restore the type or stage a versioned contract",
            ));
            continue;
        };
        let old_fields = old_ty
            .fields
            .iter()
            .map(|field| (field_key(field), field))
            .collect::<BTreeMap<_, _>>();
        let new_fields = new_ty
            .fields
            .iter()
            .map(|field| (field_key(field), field))
            .collect::<BTreeMap<_, _>>();
        for (field_name, old_field) in &old_fields {
            let path = format!("types.{name}.fields.{field_name}");
            let Some(new_field) = new_fields.get(field_name) else {
                issues.push(issue(
                    "api.field.removed",
                    "api",
                    GateSeverity::Breaking,
                    path,
                    Some(field_signature(old_field)),
                    None::<String>,
                    "restore the field or stage a versioned contract",
                ));
                continue;
            };
            if old_field.ty != new_field.ty || old_field.source != new_field.source {
                issues.push(issue(
                    "api.field.shape_changed",
                    "api",
                    GateSeverity::Breaking,
                    path.clone(),
                    Some(field_signature(old_field)),
                    Some(field_signature(new_field)),
                    "add a new field and migrate clients before removing the old field",
                ));
            }
            if old_field.validate != new_field.validate {
                let severity = if new_field.validate.is_some() {
                    GateSeverity::Breaking
                } else {
                    GateSeverity::Behavioral
                };
                issues.push(issue(
                    "api.field.validation_changed",
                    "api",
                    severity,
                    path,
                    old_field.validate.clone(),
                    new_field.validate.clone(),
                    "stage validation changes and document rejected inputs",
                ));
            }
        }
        for (field_name, new_field) in &new_fields {
            if !old_fields.contains_key(field_name) {
                let required = !is_optional_api_field(new_field);
                issues.push(issue(
                    if required {
                        "api.field.required_added"
                    } else {
                        "api.field.optional_added"
                    },
                    "api",
                    if required {
                        GateSeverity::Breaking
                    } else {
                        GateSeverity::Additive
                    },
                    format!("types.{name}.fields.{field_name}"),
                    None::<String>,
                    Some(field_signature(new_field)),
                    if required {
                        "make the field optional for a staged rollout"
                    } else {
                        "no migration required"
                    },
                ));
            }
        }
    }
    for name in new_types
        .keys()
        .filter(|name| !old_types.contains_key(*name))
    {
        issues.push(issue(
            "api.type.added",
            "api",
            GateSeverity::Additive,
            format!("types.{name}"),
            None::<String>,
            Some("present"),
            "no migration required",
        ));
    }
    issues
}

fn api_routes(spec: &ApiSpec) -> BTreeMap<String, String> {
    let mut routes = spec
        .rest_routes
        .iter()
        .map(|route| {
            let key = format!(
                "{} {}",
                method_name(&route.method),
                generator::rest::full_route_path_for_route(spec, route)
            );
            let value = format!(
                "{} -> {}; permissions={:?}; middleware={:?}",
                route.request, route.response, route.permissions, route.middlewares
            );
            (key, value)
        })
        .collect::<BTreeMap<_, _>>();
    routes.extend(spec.rpc_methods.iter().map(|method| {
        (
            format!("RPC {}", method.name),
            format!(
                "{} -> {}; permissions={:?}; middleware={:?}",
                method.request, method.response, method.permissions, method.middlewares
            ),
        )
    }));
    routes
}

fn diff_openapi(old: &ApiSpec, new: &ApiSpec, issues: &mut Vec<GateIssue>) {
    let old = openapi_surfaces(old);
    let new = openapi_surfaces(new);
    for (path, old_value) in &old {
        match new.get(path) {
            None => issues.push(issue(
                "openapi.surface.removed",
                "openapi",
                GateSeverity::Breaking,
                path,
                Some(old_value.clone()),
                None::<String>,
                "restore the OpenAPI surface or version the endpoint",
            )),
            Some(new_value) if new_value != old_value => issues.push(issue(
                "openapi.surface.changed",
                "openapi",
                GateSeverity::Behavioral,
                path,
                Some(old_value.clone()),
                Some(new_value.clone()),
                "review generated OpenAPI consumer impact",
            )),
            Some(_) => {}
        }
    }
    for (path, value) in new.iter().filter(|(path, _)| !old.contains_key(*path)) {
        issues.push(issue(
            "openapi.surface.added",
            "openapi",
            GateSeverity::Additive,
            path,
            None::<String>,
            Some(value.clone()),
            "no migration required",
        ));
    }
}

fn openapi_surfaces(spec: &ApiSpec) -> BTreeMap<String, String> {
    let document = generator::openapi_document(spec);
    let mut surfaces = BTreeMap::new();
    for root in ["paths", "components"] {
        if let Some(values) = document.get(root).and_then(serde_json::Value::as_object) {
            for (name, value) in values {
                if root == "components" {
                    if let Some(schemas) = value.as_object() {
                        for (schema, value) in schemas {
                            surfaces.insert(
                                format!("components.{name}.{schema}"),
                                canonical_json(value),
                            );
                        }
                    }
                } else {
                    surfaces.insert(format!("paths.{name}"), canonical_json(value));
                }
            }
        }
    }
    surfaces
}

pub fn diff_search_sources(old: &str, new: &str) -> anyhow::Result<Vec<GateIssue>> {
    let old = generator::search::parse_search_schema(old)
        .map_err(|err| GateInputError(format!("invalid old search schema: {err}")))?;
    let new = generator::search::parse_search_schema(new)
        .map_err(|err| GateInputError(format!("invalid new search schema: {err}")))?;
    let mut issues = Vec::new();
    if old.name != new.name {
        issues.push(issue(
            "search.index.changed",
            "search",
            GateSeverity::Breaking,
            "index",
            Some(old.name.clone()),
            Some(new.name.clone()),
            "create and backfill a versioned index before switching aliases",
        ));
    }
    if old.primary != new.primary {
        issues.push(issue(
            "search.primary.changed",
            "search",
            GateSeverity::Breaking,
            "primary",
            Some(old.primary.clone()),
            Some(new.primary.clone()),
            "reindex into a new index with the new primary field",
        ));
    }
    let old_fields = old
        .fields
        .iter()
        .map(|field| (field.name.as_str(), field))
        .collect::<BTreeMap<_, _>>();
    let new_fields = new
        .fields
        .iter()
        .map(|field| (field.name.as_str(), field))
        .collect::<BTreeMap<_, _>>();
    for (name, old_field) in &old_fields {
        let path = format!("fields.{name}");
        let Some(new_field) = new_fields.get(name) else {
            issues.push(issue(
                "search.field.removed",
                "search",
                GateSeverity::Breaking,
                path,
                Some(search_field_signature(old_field)),
                None::<String>,
                "create and backfill a versioned index",
            ));
            continue;
        };
        if old_field.ty != new_field.ty {
            issues.push(issue(
                "search.field.type_changed",
                "search",
                GateSeverity::Breaking,
                path.clone(),
                Some(search_field_signature(old_field)),
                Some(search_field_signature(new_field)),
                "create and backfill a versioned index",
            ));
        }
        for (capability, before, after) in [
            ("searchable", old_field.searchable, new_field.searchable),
            ("filterable", old_field.filterable, new_field.filterable),
            ("sortable", old_field.sortable, new_field.sortable),
        ] {
            if before != after {
                issues.push(issue(
                    "search.field.capability_changed",
                    "search",
                    if before {
                        GateSeverity::Breaking
                    } else {
                        GateSeverity::Behavioral
                    },
                    format!("{path}.{capability}"),
                    Some(before.to_string()),
                    Some(after.to_string()),
                    "reindex when required by the target search engine and update query clients",
                ));
            }
        }
    }
    for (name, field) in new_fields
        .iter()
        .filter(|(name, _)| !old_fields.contains_key(*name))
    {
        issues.push(issue(
            "search.field.added",
            "search",
            GateSeverity::Additive,
            format!("fields.{name}"),
            None::<String>,
            Some(search_field_signature(field)),
            "backfill the field when existing documents require a value",
        ));
    }
    Ok(issues)
}

pub fn diff_sql_sources(old: &str, new: &str) -> anyhow::Result<Vec<GateIssue>> {
    let old = generator::model::parse_models_with_format(old, ModelFormat::Sql)
        .map_err(|err| GateInputError(format!("invalid old SQL schema: {err}")))?;
    let new = generator::model::parse_models_with_format(new, ModelFormat::Sql)
        .map_err(|err| GateInputError(format!("invalid new SQL schema: {err}")))?;
    let old_models = old
        .iter()
        .map(|model| (model.table.as_str(), model))
        .collect::<BTreeMap<_, _>>();
    let new_models = new
        .iter()
        .map(|model| (model.table.as_str(), model))
        .collect::<BTreeMap<_, _>>();
    let mut issues = Vec::new();
    for (table, old_model) in &old_models {
        let Some(new_model) = new_models.get(table) else {
            issues.push(issue(
                "sql.table.removed",
                "sql",
                GateSeverity::Destructive,
                format!("tables.{table}"),
                Some("present"),
                None::<String>,
                "use an expand/migrate/contract sequence with a verified backup",
            ));
            continue;
        };
        let old_fields = old_model
            .fields
            .iter()
            .map(|field| (field.source_name.as_deref().unwrap_or(&field.name), field))
            .collect::<BTreeMap<_, _>>();
        let new_fields = new_model
            .fields
            .iter()
            .map(|field| (field.source_name.as_deref().unwrap_or(&field.name), field))
            .collect::<BTreeMap<_, _>>();
        for (column, old_field) in &old_fields {
            let path = format!("tables.{table}.columns.{column}");
            let Some(new_field) = new_fields.get(column) else {
                issues.push(issue("sql.column.removed", "sql", GateSeverity::Destructive, path, Some(model_field_signature(old_field)), None::<String>, "stop reads and writes, back up data, then remove the column in a later release"));
                continue;
            };
            if old_field.ty != new_field.ty {
                let severity = classify_sql_type_change(&old_field.ty, &new_field.ty);
                issues.push(issue(
                    "sql.column.type_changed",
                    "sql",
                    severity,
                    path.clone(),
                    Some(old_field.ty.clone()),
                    Some(new_field.ty.clone()),
                    "use an online expand/backfill/contract migration",
                ));
            }
            let old_optional = is_optional_model_type(&old_field.ty);
            let new_optional = is_optional_model_type(&new_field.ty);
            if old_optional && !new_optional {
                issues.push(issue(
                    "sql.column.nullability_narrowed",
                    "sql",
                    GateSeverity::Destructive,
                    path.clone(),
                    Some("nullable"),
                    Some("not_null"),
                    "backfill NULL values and validate before adding NOT NULL",
                ));
            }
            if old_field.default_value != new_field.default_value {
                issues.push(issue(
                    "sql.column.default_changed",
                    "sql",
                    GateSeverity::Behavioral,
                    path,
                    old_field.default_value.clone(),
                    new_field.default_value.clone(),
                    "verify write behavior for old and new service versions",
                ));
            }
        }
        for (column, field) in new_fields
            .iter()
            .filter(|(column, _)| !old_fields.contains_key(*column))
        {
            let safe = is_optional_model_type(&field.ty) || field.default_value.is_some();
            issues.push(issue(
                "sql.column.added",
                "sql",
                if safe {
                    GateSeverity::Safe
                } else {
                    GateSeverity::Destructive
                },
                format!("tables.{table}.columns.{column}"),
                None::<String>,
                Some(model_field_signature(field)),
                if safe {
                    "verify the online DDL behavior of the target database"
                } else {
                    "add the column as nullable, backfill it, then enforce NOT NULL"
                },
            ));
        }
        diff_indexes(table, &old_model.indexes, &new_model.indexes, &mut issues);
        diff_edges(table, &old_model.edges, &new_model.edges, &mut issues);
    }
    for table in new_models
        .keys()
        .filter(|table| !old_models.contains_key(*table))
    {
        issues.push(issue(
            "sql.table.added",
            "sql",
            GateSeverity::Safe,
            format!("tables.{table}"),
            None::<String>,
            Some("present"),
            "verify creation privileges and storage policy",
        ));
    }
    Ok(issues)
}

fn diff_indexes(
    table: &str,
    old: &[generator::model::ModelIndex],
    new: &[generator::model::ModelIndex],
    issues: &mut Vec<GateIssue>,
) {
    let old = old
        .iter()
        .map(|index| (index.name.as_str(), index))
        .collect::<BTreeMap<_, _>>();
    let new = new
        .iter()
        .map(|index| (index.name.as_str(), index))
        .collect::<BTreeMap<_, _>>();
    for (name, old_index) in &old {
        let path = format!("tables.{table}.indexes.{name}");
        match new.get(name) {
            None => issues.push(issue(
                "sql.index.removed",
                "sql",
                GateSeverity::OnlineRisk,
                path,
                Some(format!(
                    "{:?}; unique={}",
                    old_index.fields, old_index.unique
                )),
                None::<String>,
                "verify query plans and remove the index with an online operation",
            )),
            Some(new_index)
                if old_index.fields != new_index.fields || old_index.unique != new_index.unique =>
            {
                issues.push(issue(
                    "sql.index.changed",
                    "sql",
                    GateSeverity::OnlineRisk,
                    path,
                    Some(format!(
                        "{:?}; unique={}",
                        old_index.fields, old_index.unique
                    )),
                    Some(format!(
                        "{:?}; unique={}",
                        new_index.fields, new_index.unique
                    )),
                    "create the replacement index online before dropping the old index",
                ))
            }
            Some(_) => {}
        }
    }
    for (name, index) in new.iter().filter(|(name, _)| !old.contains_key(*name)) {
        issues.push(issue(
            "sql.index.added",
            "sql",
            GateSeverity::OnlineRisk,
            format!("tables.{table}.indexes.{name}"),
            None::<String>,
            Some(format!("{:?}; unique={}", index.fields, index.unique)),
            "use concurrent/online index creation and estimate lock duration",
        ));
    }
}

fn diff_edges(
    table: &str,
    old: &[generator::model::ModelEdge],
    new: &[generator::model::ModelEdge],
    issues: &mut Vec<GateIssue>,
) {
    let old = old
        .iter()
        .map(|edge| (edge.name.as_str(), edge))
        .collect::<BTreeMap<_, _>>();
    let new = new
        .iter()
        .map(|edge| (edge.name.as_str(), edge))
        .collect::<BTreeMap<_, _>>();
    for (name, old_edge) in &old {
        let path = format!("tables.{table}.constraints.{name}");
        match new.get(name) {
            None => issues.push(issue("sql.foreign_key.removed", "sql", GateSeverity::OnlineRisk, path, Some(format!("{}:{}", old_edge.target, old_edge.field)), None::<String>, "verify orphan behavior before removing the constraint")),
            Some(new_edge) if old_edge.target != new_edge.target || old_edge.field != new_edge.field || old_edge.required != new_edge.required || old_edge.unique != new_edge.unique => issues.push(issue("sql.foreign_key.changed", "sql", GateSeverity::Destructive, path, Some(format!("{}:{} required={} unique={}", old_edge.target, old_edge.field, old_edge.required, old_edge.unique)), Some(format!("{}:{} required={} unique={}", new_edge.target, new_edge.field, new_edge.required, new_edge.unique)), "stage and validate the replacement constraint before removing the old constraint")),
            Some(_) => {}
        }
    }
    for (name, edge) in new.iter().filter(|(name, _)| !old.contains_key(*name)) {
        issues.push(issue(
            "sql.foreign_key.added",
            "sql",
            GateSeverity::OnlineRisk,
            format!("tables.{table}.constraints.{name}"),
            None::<String>,
            Some(format!("{}:{}", edge.target, edge.field)),
            "validate existing rows before adding the constraint",
        ));
    }
}

fn diff_named_maps(
    domain: &str,
    kind: &str,
    old: &BTreeMap<String, String>,
    new: &BTreeMap<String, String>,
    issues: &mut Vec<GateIssue>,
) {
    for (name, old_value) in old {
        match new.get(name) {
            None => issues.push(issue(
                format!("{domain}.{kind}.removed"),
                domain,
                GateSeverity::Breaking,
                format!("{kind}s.{name}"),
                Some(old_value.clone()),
                None::<String>,
                "restore or version the removed surface",
            )),
            Some(new_value) if new_value != old_value => issues.push(issue(
                format!("{domain}.{kind}.changed"),
                domain,
                GateSeverity::Breaking,
                format!("{kind}s.{name}"),
                Some(old_value.clone()),
                Some(new_value.clone()),
                "stage a new surface and migrate callers",
            )),
            Some(_) => {}
        }
    }
    for (name, value) in new.iter().filter(|(name, _)| !old.contains_key(*name)) {
        issues.push(issue(
            format!("{domain}.{kind}.added"),
            domain,
            GateSeverity::Additive,
            format!("{kind}s.{name}"),
            None::<String>,
            Some(value.clone()),
            "no migration required",
        ));
    }
}

fn issue(
    code: impl Into<String>,
    domain: impl Into<String>,
    severity: GateSeverity,
    path: impl Into<String>,
    before: Option<impl Into<String>>,
    after: Option<impl Into<String>>,
    remediation: impl Into<String>,
) -> GateIssue {
    GateIssue {
        code: code.into(),
        domain: domain.into(),
        severity,
        path: path.into(),
        before: before.map(Into::into),
        after: after.map(Into::into),
        remediation: remediation.into(),
        acknowledged_by: None,
    }
}

fn canonical_json(value: &serde_json::Value) -> String {
    serde_json::to_string(value).expect("JSON value is serializable")
}

fn method_name(method: &HttpMethod) -> &'static str {
    match method {
        HttpMethod::Get => "GET",
        HttpMethod::Head => "HEAD",
        HttpMethod::Post => "POST",
        HttpMethod::Put => "PUT",
        HttpMethod::Patch => "PATCH",
        HttpMethod::Delete => "DELETE",
    }
}

fn field_key(field: &Field) -> String {
    field
        .wire_name
        .as_deref()
        .or(field.json_name.as_deref())
        .unwrap_or(&field.name)
        .to_string()
}

fn field_signature(field: &Field) -> String {
    format!(
        "{} {:?} validate={:?}",
        field.ty, field.source, field.validate
    )
}

fn is_optional_api_field(field: &Field) -> bool {
    field.validate.as_deref().is_some_and(|rules| {
        rules
            .split(',')
            .map(str::trim)
            .any(|rule| rule == "optional" || rule == "omitempty")
    })
}

fn search_field_signature(field: &generator::search::SearchFieldSpec) -> String {
    format!(
        "{:?}; searchable={}; filterable={}; sortable={}",
        field.ty, field.searchable, field.filterable, field.sortable
    )
}

fn model_field_signature(field: &generator::model::ModelField) -> String {
    format!("{}; default={:?}", field.ty, field.default_value)
}

fn is_optional_model_type(ty: &str) -> bool {
    ty.trim_start().starts_with("Option<")
}

fn classify_sql_type_change(old: &str, new: &str) -> GateSeverity {
    let old = normalized_scalar_type(old);
    let new = normalized_scalar_type(new);
    let widths = BTreeMap::from([
        ("i8", 8),
        ("u8", 8),
        ("i16", 16),
        ("u16", 16),
        ("i32", 32),
        ("u32", 32),
        ("i64", 64),
        ("u64", 64),
        ("f32", 32),
        ("f64", 64),
    ]);
    match (widths.get(old.as_str()), widths.get(new.as_str())) {
        (Some(old), Some(new)) if new >= old => GateSeverity::OnlineRisk,
        _ => GateSeverity::Destructive,
    }
}

fn normalized_scalar_type(ty: &str) -> String {
    ty.trim()
        .strip_prefix("Option<")
        .and_then(|value| value.strip_suffix('>'))
        .unwrap_or(ty.trim())
        .to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_api_search_and_sql_changes() {
        let api = diff_api_sources(
            "service demo {\n  @handler get\n  get /items (Req) returns (Resp)\n}\ntype Req {\n  id string `json:\"id,optional\" validate:\"optional\"`\n}\ntype Resp {\n  id string `json:\"id\"`\n}\n",
            "service demo {\n  @handler get\n  get /items (Req) returns (Resp)\n}\ntype Req {\n  id string `json:\"id,optional\" validate:\"optional\"`\n  tenant string `json:\"tenant\"`\n}\ntype Resp {\n  id string `json:\"id\"`\n}\n",
        ).unwrap();
        assert!(api
            .iter()
            .any(|issue| issue.code == "api.field.required_added"
                && issue.severity == GateSeverity::Breaking));

        let search = diff_search_sources(
            "index products\nprimary id\nfield id keyword primary filterable\nfield title text searchable\n",
            "index products\nprimary id\nfield id keyword primary filterable\nfield title keyword\n",
        ).unwrap();
        assert!(search
            .iter()
            .any(|issue| issue.code == "search.field.type_changed"
                && issue.severity == GateSeverity::Breaking));

        let sql = diff_sql_sources(
            "CREATE TABLE users (id BIGINT PRIMARY KEY, email TEXT NULL);",
            "CREATE TABLE users (id BIGINT PRIMARY KEY, email TEXT NOT NULL);",
        )
        .unwrap();
        assert!(sql
            .iter()
            .any(|issue| issue.code == "sql.column.nullability_narrowed"
                && issue.severity == GateSeverity::Destructive));
    }

    #[test]
    fn digest_is_stable_sha256() {
        assert_eq!(
            content_digest(b"roze"),
            "0ce32571f69b5766b0c48147eaa1cc4d1a56fc66550fdd9007dad95eb7544990"
        );
    }

    #[test]
    fn expiry_parser_accepts_date_and_rfc3339_prefix() {
        assert_eq!(
            parse_expiry_date("2026-07-16"),
            parse_expiry_date("2026-07-16T23:59:59Z")
        );
        assert!(parse_expiry_date("16-07-2026").is_none());
    }

    #[test]
    fn manifest_requires_matching_unexpired_hash_acknowledgement() {
        let root = std::env::temp_dir().join(format!(
            "roze-gate-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let old = "service demo {\n  get /items (Req) returns (Resp)\n}\ntype Req {\n  id string `json:\"id\"`\n}\ntype Resp {\n  id string `json:\"id\"`\n}\n";
        let new = "service demo {\n  get /items (Req) returns (Resp)\n}\ntype Req {\n  id string `json:\"id\"`\n}\ntype Resp {\n}\n";
        fs::write(root.join("old.api"), old).unwrap();
        fs::write(root.join("new.api"), new).unwrap();
        fs::write(
            root.join("roze-gate.yaml"),
            "version: 1\nchecks:\n  - domain: api\n    old: old.api\n    new: new.api\nacknowledgements: []\n",
        )
        .unwrap();

        let blocked = run_manifest(&root.join("roze-gate.yaml")).unwrap();
        assert!(!blocked.passed);
        assert!(blocked.blocking_issues > 0);

        fs::write(
            root.join("ack.yaml"),
            format!(
                "version: 1\nid: remove-response-id\nscope: api\nold_digest: {}\nnew_digest: {}\nowner: platform\nreason: replace response contract\nmigration_plan: regenerate callers before deployment\nrollback_plan: restore the old response field\nexpires_at: 2999-12-31\n",
                content_digest(old.as_bytes()),
                content_digest(new.as_bytes())
            ),
        )
        .unwrap();
        fs::write(
            root.join("roze-gate.yaml"),
            "version: 1\nchecks:\n  - domain: api\n    old: old.api\n    new: new.api\nacknowledgements:\n  - ack.yaml\n",
        )
        .unwrap();
        let acknowledged = run_manifest(&root.join("roze-gate.yaml")).unwrap();
        assert!(acknowledged.passed);
        assert_eq!(acknowledged.blocking_issues, 0);
        assert!(acknowledged.checks[0]
            .issues
            .iter()
            .filter(|issue| issue.severity.blocks_release())
            .all(|issue| issue.acknowledged_by.as_deref() == Some("remove-response-id")));

        fs::remove_dir_all(root).unwrap();
    }
}
