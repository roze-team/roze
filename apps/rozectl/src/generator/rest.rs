use std::collections::{BTreeMap, HashMap, HashSet};

use crate::{
    generator::{rust_identifier, to_pascal_case, to_snake_case},
    parser::{ApiSpec, Field, FieldSource, HttpMethod, RestRoute, TypeDef},
};

pub fn render_rest_main(_spec: &ApiSpec) -> String {
    r#"mod config;
mod handler;
mod logic;
mod middleware;
mod openapi;
mod route;
mod svc;
mod types;

use roze_http::rest::{RestServer, RestService};
use roze_service::ServiceGroup;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = config::load(config_path())?;
    roze_log::init_tracing_with_config(&config)?;
    let rest = config
        .rest
        .clone()
        .ok_or_else(|| anyhow::anyhow!("missing rest config"))?;
    tracing::info!(
        service = %config.name,
        protocol = "rest",
        listen_addr = %rest.addr,
        register = rest.register,
        "service configuration loaded"
    );
    let mut registration = if rest.register {
        let registry = roze_rpc::registry::build_service_registry(&config)?
            .ok_or_else(|| anyhow::anyhow!("missing registry config"))?;
        let registration = roze_rpc::rpc::ServiceRegistrationGuard::start(
            registry,
            config.name.clone(),
            rest.addr,
        )
        .await?;
        tracing::info!(service = %config.name, protocol = "rest", addr = %rest.addr, "service registered");
        Some(registration)
    } else {
        None
    };
    let service_name = config.name.clone();
    let ctx = svc::ServiceContext::new(config).await?;
    tracing::info!(service = %service_name, protocol = "rest", "service context initialized");
    let health = ctx.health.clone();
    let middleware_config = roze_middleware::CommonMiddlewareConfig::from(&rest.middlewares);
    tracing::debug!(
        protocol = "rest",
        request_context = middleware_config.request_context,
        request_tracing = middleware_config.tracing,
        auth_enabled = middleware_config.auth.is_some(),
        cors_enabled = middleware_config.cors,
        timeout_ms = ?middleware_config.timeout_ms,
        body_limit_bytes = ?middleware_config.body_limit_bytes,
        "REST middleware plan resolved"
    );
    let app = route::router(ctx);
    tracing::debug!(protocol = "rest", "REST router constructed");
    let app = middleware::app::apply(app);
    tracing::debug!(protocol = "rest", "application middleware hook applied");
    let app = roze_middleware::apply_common_with_config(app, middleware_config);
    tracing::debug!(protocol = "rest", "Roze common middleware applied");
    let mut group = ServiceGroup::new();
    group.add(RestService::new(
        service_name.clone(),
        RestServer::new(rest.addr, app),
    ));
    group.add_fn("health-drain", move |shutdown| {
        let health = health.clone();
        async move {
            shutdown.wait().await;
            tracing::info!(protocol = "rest", "shutdown requested; marking service draining");
            health.mark_draining();
            Ok(())
        }
    });
    tracing::info!(service = %service_name, protocol = "rest", listen_addr = %rest.addr, "service starting");
    let result = group.start().await;
    if let Some(registration) = registration.as_mut() {
        registration.shutdown().await?;
        tracing::info!(service = %service_name, protocol = "rest", "service unregistered");
    }
    match &result {
        Ok(()) => tracing::info!(service = %service_name, protocol = "rest", "service stopped"),
        Err(error) => tracing::error!(service = %service_name, protocol = "rest", error = %error, "service failed"),
    }
    result?;

    Ok(())
}

fn config_path() -> std::path::PathBuf {
    let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let manifest_config = manifest_dir.join("config.yaml");
    if manifest_config.exists() {
        manifest_config
    } else {
        std::path::PathBuf::from("config.yaml")
    }
}
"#
    .to_string()
}

#[cfg(test)]
pub fn render_handlers(spec: &ApiSpec) -> String {
    let mut out = String::from("#![allow(unused_imports)]\n\n");
    out.push_str(
        "use roze_http::{extract::{Extension, Form, Path, Query, State}, http::HeaderMap, routing::{delete, get, head, patch, post, put}, Json, Router};\nuse serde::Deserialize;\nuse roze_validation::Validate;\nuse roze_context::Context;\nuse roze_error::RozeError;\nuse roze_result::ApiResponse;\n\nuse crate::openapi;\nuse crate::svc::ServiceContext;\nuse crate::types::*;\n\n",
    );
    out.push_str("pub fn router(ctx: ServiceContext) -> Router {\n");
    out.push_str("    Router::new()\n");
    out.push_str(&format!(
        "        .route(\"{}\", get(health))\n",
        roze_http_route_path(&full_route_path(spec, "/healthz"))
    ));
    out.push_str(&format!(
        "        .route(\"{}\", get(readiness))\n",
        roze_http_route_path(&full_route_path(spec, "/readyz"))
    ));
    out.push_str(&format!(
        "        .route(\"{}\", get(startup))\n",
        roze_http_route_path(&full_route_path(spec, "/startupz"))
    ));
    out.push_str(&format!(
        "        .route(\"{}\", get(metrics))\n",
        roze_http_route_path(&full_route_path(spec, "/metrics"))
    ));
    out.push_str(&format!(
        "        .route(\"{}\", get(openapi_doc))\n",
        roze_http_route_path(&full_route_path(spec, "/openapi.json"))
    ));

    for route in &spec.rest_routes {
        let handler = resolved_handler_name(route);
        let routing_fn = match route.method {
            HttpMethod::Get => "get",
            HttpMethod::Head => "head",
            HttpMethod::Post => "post",
            HttpMethod::Put => "put",
            HttpMethod::Patch => "patch",
            HttpMethod::Delete => "delete",
        };
        out.push_str(&format!(
            "        .route(\"{}\", {}({}))\n",
            roze_http_route_path(&full_route_path_for_route(spec, route)),
            routing_fn,
            handler
        ));
    }

    out.push_str("        .with_state(ctx)\n");
    out.push_str("}\n\n");

    out.push_str(
        "async fn health(State(ctx): State<ServiceContext>) -> Result<ApiResponse<roze_health::ProbeReport>, RozeError> {\n    Ok(ApiResponse::ok(ctx.health.liveness_report().await.probe(roze_health::ProbeKind::Liveness)))\n}\n\n",
    );

    out.push_str(
        "async fn readiness(State(ctx): State<ServiceContext>) -> Result<ApiResponse<roze_health::ProbeReport>, RozeError> {\n    Ok(ApiResponse::ok(ctx.health.readiness_report().await.probe(roze_health::ProbeKind::Readiness)))\n}\n\n",
    );

    out.push_str(
        "async fn startup(State(ctx): State<ServiceContext>) -> Result<ApiResponse<roze_health::ProbeReport>, RozeError> {\n    Ok(ApiResponse::ok(ctx.health.startup_report().await.probe(roze_health::ProbeKind::Startup)))\n}\n\n",
    );

    out.push_str("async fn metrics() -> String {\n    roze_metrics::http_metrics()\n}\n\n");

    out.push_str(
        "async fn openapi_doc() -> Json<serde_json::Value> {\n    Json(openapi::document())\n}\n\n",
    );

    if spec.rest_routes.iter().any(|route| {
        route_request_spec(spec, route).is_some_and(|spec| spec.has_header)
            || route_uses_auth(spec, route)
            || route_uses_idempotency(spec, route)
    }) {
        out.push_str(
            "fn header_value<T>(headers: &HeaderMap, name: &str) -> Result<T, RozeError>\nwhere\n    T: std::str::FromStr,\n    T::Err: std::fmt::Display,\n{\n    let raw = headers\n        .get(name)\n        .ok_or_else(|| RozeError::BadRequest(format!(\"missing header `{name}`\")))?;\n    let raw = raw\n        .to_str()\n        .map_err(|err| RozeError::BadRequest(format!(\"invalid header `{name}`: {err}\")))?;\n    raw.parse::<T>()\n        .map_err(|err| RozeError::BadRequest(format!(\"invalid header `{name}`: {err}\")))\n}\n\n",
        );
    }
    if spec
        .rest_routes
        .iter()
        .any(|route| route_uses_auth(spec, route))
    {
        out.push_str(
            "fn authorize(headers: &HeaderMap, ctx: &ServiceContext) -> Result<roze_context::AuthContext, RozeError> {\n    let jwt = ctx.jwt_config().ok_or(RozeError::Unauthorized)?;\n    let header_value = headers\n        .get(\"authorization\")\n        .and_then(|value| value.to_str().ok())\n        .ok_or(RozeError::Unauthorized)?;\n    let token = roze_jwt::extract_bearer_token(header_value).ok_or(RozeError::Unauthorized)?;\n    let claims = roze_jwt::verify_token(token, &jwt).map_err(|_| RozeError::Unauthorized)?;\n    Ok(roze_context::AuthContext {\n        subject: claims.sub,\n        roles: claims.roles,\n        tenant: claims.tenant,\n    })\n}\n\n",
        );
    }

    for route in &spec.rest_routes {
        out.push_str(&render_route_handler(spec, route));
    }

    out
}

pub fn render_handler_mod(spec: &ApiSpec) -> String {
    let mut out = String::from("#![allow(unused_imports)]\n\n");
    for group in route_groups(spec).keys() {
        out.push_str(&format!("pub mod {group};\n"));
    }
    if !spec.rest_routes.is_empty() {
        out.push('\n');
    }
    out.push_str(
        "use roze_http::{extract::{Extension, Form, Path, Query, State}, http::HeaderMap, Json};\nuse serde::Deserialize;\nuse roze_validation::Validate;\nuse roze_context::Context;\nuse roze_error::RozeError;\nuse roze_result::ApiResponse;\n\nuse crate::svc::ServiceContext;\nuse crate::types::*;\n\n",
    );
    if spec.rest_routes.iter().any(|route| {
        route_request_spec(spec, route).is_some_and(|spec| spec.has_header)
            || route_uses_auth(spec, route)
            || route_uses_idempotency(spec, route)
    }) {
        out.push_str(
            "fn header_value<T>(headers: &HeaderMap, name: &str) -> Result<T, RozeError>\nwhere\n    T: std::str::FromStr,\n    T::Err: std::fmt::Display,\n{\n    let raw = headers\n        .get(name)\n        .ok_or_else(|| RozeError::BadRequest(format!(\"missing header `{name}`\")))?;\n    let raw = raw\n        .to_str()\n        .map_err(|err| RozeError::BadRequest(format!(\"invalid header `{name}`: {err}\")))?;\n    raw.parse::<T>()\n        .map_err(|err| RozeError::BadRequest(format!(\"invalid header `{name}`: {err}\")))\n}\n\n",
        );
    }
    if spec
        .rest_routes
        .iter()
        .any(|route| route_uses_auth(spec, route))
    {
        out.push_str(
            "fn authorize(headers: &HeaderMap, ctx: &ServiceContext) -> Result<roze_context::AuthContext, RozeError> {\n    let jwt = ctx.jwt_config().ok_or(RozeError::Unauthorized)?;\n    let header_value = headers\n        .get(\"authorization\")\n        .and_then(|value| value.to_str().ok())\n        .ok_or(RozeError::Unauthorized)?;\n    let token = roze_jwt::extract_bearer_token(header_value).ok_or(RozeError::Unauthorized)?;\n    let claims = roze_jwt::verify_token(token, &jwt).map_err(|_| RozeError::Unauthorized)?;\n    Ok(roze_context::AuthContext {\n        subject: claims.sub,\n        roles: claims.roles,\n        tenant: claims.tenant,\n    })\n}\n\n",
        );
    }

    out
}

pub fn render_route_mod(spec: &ApiSpec) -> String {
    let mut out = String::from("#![allow(unused_imports)]\n\n");
    for group in route_groups(spec).keys() {
        out.push_str(&format!("mod {group};\n"));
    }
    if !spec.rest_routes.is_empty() {
        out.push('\n');
    }
    out.push_str(
        "use std::collections::BTreeMap;\n\nuse roze_http::{extract::{Query, State}, routing::get, Json, Router};\nuse roze_error::RozeError;\nuse roze_result::ApiResponse;\nuse serde::{Deserialize, Serialize};\n\nuse crate::openapi;\nuse crate::svc::ServiceContext;\n\n",
    );
    out.push_str("pub fn router(ctx: ServiceContext) -> Router {\n");
    out.push_str("    let timeout = ctx\n        .config\n        .rest\n        .as_ref()\n        .filter(|rest| rest.middlewares.timeout)\n        .and(ctx.config.governance.timeout_ms);\n    let router = Router::new()\n");
    out.push_str(&format!(
        "        .route(\"{}\", get(health))\n",
        roze_http_route_path(&full_route_path(spec, "/healthz"))
    ));
    out.push_str(&format!(
        "        .route(\"{}\", get(readiness))\n",
        roze_http_route_path(&full_route_path(spec, "/readyz"))
    ));
    out.push_str(&format!(
        "        .route(\"{}\", get(startup))\n",
        roze_http_route_path(&full_route_path(spec, "/startupz"))
    ));
    out.push_str(&format!(
        "        .route(\"{}\", get(metrics))\n",
        roze_http_route_path(&full_route_path(spec, "/metrics"))
    ));
    out.push_str(&format!(
        "        .route(\"{}\", get(report_export))\n",
        roze_http_route_path(&full_route_path(spec, "/reports/export"))
    ));
    out.push_str(&format!(
        "        .route(\"{}\", get(chart_query))\n",
        roze_http_route_path(&full_route_path(spec, "/charts/query"))
    ));
    out.push_str(&format!(
        "        .route(\"{}\", get(openapi_doc))\n",
        roze_http_route_path(&full_route_path(spec, "/openapi.json"))
    ));

    for group in route_groups(spec).keys() {
        out.push_str(&format!(
            "        .merge({group}::routes())\n",
            group = group
        ));
    }

    out.push_str(";\n");
    out.push_str("    let router = match timeout {\n        Some(timeout_ms) => roze_middleware::apply_timeout(router, timeout_ms),\n        None => router,\n    };\n    router.with_state(ctx)\n");
    out.push_str("}\n\n");

    out.push_str(
        "async fn health(State(ctx): State<ServiceContext>) -> Result<ApiResponse<roze_health::ProbeReport>, RozeError> {\n    Ok(ApiResponse::ok(ctx.health.liveness_report().await.probe(roze_health::ProbeKind::Liveness)))\n}\n\n",
    );
    out.push_str(
        "async fn readiness(State(ctx): State<ServiceContext>) -> Result<ApiResponse<roze_health::ProbeReport>, RozeError> {\n    Ok(ApiResponse::ok(ctx.health.readiness_report().await.probe(roze_health::ProbeKind::Readiness)))\n}\n\n",
    );
    out.push_str(
        "async fn startup(State(ctx): State<ServiceContext>) -> Result<ApiResponse<roze_health::ProbeReport>, RozeError> {\n    Ok(ApiResponse::ok(ctx.health.startup_report().await.probe(roze_health::ProbeKind::Startup)))\n}\n\n",
    );
    out.push_str("async fn metrics() -> String {\n    roze_metrics::http_metrics()\n}\n\n");
    out.push_str(
        "async fn openapi_doc() -> Json<serde_json::Value> {\n    Json(openapi::document())\n}\n\n",
    );
    out.push_str(report_chart_interface_code());

    out
}

fn report_chart_interface_code() -> &'static str {
    r#"#[derive(Debug, Clone, Deserialize)]
struct ReportExportQuery {
    #[serde(default)]
    report: String,
    #[serde(default = "default_report_format")]
    format: String,
    #[serde(default)]
    from: Option<String>,
    #[serde(default)]
    to: Option<String>,
    #[serde(default)]
    filters: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct ReportExportResponse {
    report: String,
    format: String,
    status: String,
    export_id: String,
    download_url: Option<String>,
    from: Option<String>,
    to: Option<String>,
    filters: Option<String>,
    columns: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct ChartQuery {
    #[serde(default)]
    chart: String,
    #[serde(default)]
    from: Option<String>,
    #[serde(default)]
    to: Option<String>,
    #[serde(default)]
    interval: Option<String>,
    #[serde(default)]
    filters: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct ChartQueryResponse {
    chart: String,
    interval: Option<String>,
    from: Option<String>,
    to: Option<String>,
    filters: Option<String>,
    series: Vec<ChartSeries>,
}

#[derive(Debug, Clone, Serialize)]
struct ChartSeries {
    name: String,
    points: Vec<ChartPoint>,
}

#[derive(Debug, Clone, Serialize)]
struct ChartPoint {
    timestamp: String,
    value: f64,
    labels: BTreeMap<String, String>,
}

fn default_report_format() -> String {
    "csv".to_string()
}

fn default_interface_name(value: String) -> String {
    if value.trim().is_empty() {
        "default".to_string()
    } else {
        value
    }
}

async fn report_export(
    Query(query): Query<ReportExportQuery>,
) -> Result<ApiResponse<ReportExportResponse>, RozeError> {
    let report = default_interface_name(query.report);
    let format = default_interface_name(query.format);
    let export_id = format!("report-{report}-{format}");
    Ok(ApiResponse::ok(ReportExportResponse {
        report,
        format,
        status: "accepted".to_string(),
        export_id,
        download_url: None,
        from: query.from,
        to: query.to,
        filters: query.filters,
        columns: Vec::new(),
    }))
}

async fn chart_query(
    Query(query): Query<ChartQuery>,
) -> Result<ApiResponse<ChartQueryResponse>, RozeError> {
    Ok(ApiResponse::ok(ChartQueryResponse {
        chart: default_interface_name(query.chart),
        interval: query.interval,
        from: query.from,
        to: query.to,
        filters: query.filters,
        series: Vec::new(),
    }))
}

"#
}

pub fn render_route_group_mods(spec: &ApiSpec) -> Vec<(String, String)> {
    route_groups(spec)
        .into_iter()
        .map(|(group, routes)| {
            let mut out = String::from("use roze_http::{routing::{delete, get, head, patch, post, put}, Router};\n\nuse crate::handler;\n\npub fn routes() -> Router {\n    Router::new()\n");
            for route in routes {
                let handler = resolved_handler_name(route);
                let routing_fn = match route.method {
                    HttpMethod::Get => "get",
                    HttpMethod::Head => "head",
                    HttpMethod::Post => "post",
                    HttpMethod::Put => "put",
                    HttpMethod::Patch => "patch",
                    HttpMethod::Delete => "delete",
                };
                out.push_str(&format!(
                    "        .route(\"{}\", {}(handler::{}::{}))\n",
                    roze_http_route_path(&full_route_path_for_route(spec, route)),
                    routing_fn,
                    group,
                    handler
                ));
            }
            out.push_str("}\n");
            (group, out)
        })
        .collect()
}

pub fn render_handler_group_mods(spec: &ApiSpec) -> Vec<(String, String)> {
    route_groups(spec)
        .into_iter()
        .map(|(group, routes)| {
            let mut out = String::new();
            for route in routes {
                let handler = resolved_handler_name(route);
                out.push_str(&format!("mod {handler};\n"));
                out.push_str(&format!("pub(crate) use {handler}::{handler};\n"));
            }
            (group, out)
        })
        .collect()
}

pub fn render_handler_files(spec: &ApiSpec) -> Vec<(String, String, String)> {
    spec.rest_routes
        .iter()
        .map(|route| {
            let group = route_group_name(route);
            let handler = resolved_handler_name(route);
            let content = format!(
                "use super::super::*;\n\n{}",
                render_route_handler(spec, route)
            );
            (group, handler, content)
        })
        .collect()
}

#[cfg(test)]
#[allow(dead_code)]
pub fn render_middleware(spec: &ApiSpec) -> String {
    let custom = custom_middlewares(spec);
    let mut out = String::from("#![allow(dead_code, unused_imports, unused_variables)]\n\n");
    out.push_str("use roze_context::Context;\nuse roze_error::RozeError;\n\nuse crate::svc::ServiceContext;\n\n");
    if custom.is_empty() {
        out.push_str(
            "// Add custom middleware hooks here when `.api` declares non-built-in middleware.\n",
        );
        return out;
    }
    for name in custom {
        out.push_str(&format!(
            "pub async fn {name}(ctx: &ServiceContext, request_ctx: &Context) -> Result<(), RozeError> {{\n    Ok(())\n}}\n\n"
        ));
    }
    out
}

pub fn render_middleware_mod(spec: &ApiSpec) -> String {
    let custom = custom_middlewares(spec);
    let mut out =
        String::from("#![allow(dead_code, unused_imports, unused_variables)]\n\npub mod app;\n");
    if custom.is_empty() {
        out.push_str(
            "\n// Route middleware hooks are added here for non-built-in `.api` middleware.\n",
        );
        return out;
    }
    for name in custom {
        out.push_str(&format!("mod {name};\n"));
        out.push_str(&format!("pub use {name}::{name};\n"));
    }
    out
}

pub fn render_application_middleware() -> String {
    r#"use roze_http::Router;

/// Stable application-owned hook for service-wide middleware.
///
/// This file is preserved by `rozectl api generate --update`. Add custom
/// Tower/Roze HTTP layers here; Roze common middleware wraps the returned
/// router so request context and CORS preflight run before application layers.
pub fn apply(router: Router) -> Router {
    router
}
"#
    .to_string()
}

pub fn render_middleware_files(spec: &ApiSpec) -> Vec<(String, String)> {
    custom_middlewares(spec)
        .into_iter()
        .map(|name| {
            let content = format!(
                "use roze_context::Context;\nuse roze_error::RozeError;\n\nuse crate::svc::ServiceContext;\n\npub async fn {name}(ctx: &ServiceContext, request_ctx: &Context) -> Result<(), RozeError> {{\n    let _ = ctx;\n    let _ = request_ctx;\n    Ok(())\n}}\n"
            );
            (name, content)
        })
        .collect()
}

#[cfg(test)]
pub fn render_logic(spec: &ApiSpec) -> String {
    if spec.rest_routes.is_empty() {
        return String::new();
    }

    let mut out = String::from("use roze_error::RozeError;\n\n");
    out.push_str("use crate::svc::ServiceContext;\n");
    out.push_str("use crate::types::*;\n\n");

    for route in &spec.rest_routes {
        let handler = resolved_handler_name(route);
        match route.method {
            HttpMethod::Get | HttpMethod::Head | HttpMethod::Delete => {
                out.push_str(&format!(
                    "pub async fn {handler}(ctx: ServiceContext, request_ctx: roze_context::Context, req: {request}) -> Result<{response}, RozeError> {{\n",
                    handler = handler,
                    request = route.request,
                    response = route.response
                ));
                out.push_str("    let _ = ctx;\n");
                out.push_str("    let _ = request_ctx;\n");
                out.push_str("    let _ = req;\n");
            }
            HttpMethod::Post | HttpMethod::Put | HttpMethod::Patch => {
                out.push_str(&format!(
                    "pub async fn {handler}(ctx: ServiceContext, request_ctx: roze_context::Context, req: {request}) -> Result<{response}, RozeError> {{\n",
                    handler = handler,
                    request = route.request,
                    response = route.response
                ));
                out.push_str("    let _ = ctx;\n");
                out.push_str("    let _ = request_ctx;\n");
                out.push_str("    let _ = req;\n");
            }
        }
        out.push_str(&format!(
            "    Ok({response}::default())\n",
            response = route.response
        ));
        out.push_str("}\n\n");
    }

    out
}

fn render_logic_fn(route: &RestRoute) -> String {
    let handler = resolved_handler_name(route);
    let mut out = String::new();
    out.push_str(&format!(
        "pub async fn {handler}(ctx: ServiceContext, request_ctx: roze_context::Context, req: {request}) -> Result<{response}, RozeError> {{\n",
        handler = handler,
        request = route.request,
        response = route.response
    ));
    out.push_str("    let _ = ctx;\n");
    out.push_str("    let _ = request_ctx;\n");
    out.push_str("    let _ = req;\n");
    out.push_str(&format!(
        "    Ok({response}::default())\n",
        response = route.response
    ));
    out.push_str("}\n");
    out
}

pub fn render_logic_mod(spec: &ApiSpec) -> String {
    let mut out = String::from("#![allow(dead_code)]\n\nuse roze_error::RozeError;\n\n");
    out.push_str("use crate::svc::ServiceContext;\n");
    out.push_str("use crate::types::*;\n\n");
    out.push_str(render_auth_context_helpers());

    for group in route_groups(spec).keys() {
        out.push_str(&format!("pub mod {group};\n"));
        out.push_str(&format!("pub use {group}::*;\n"));
    }
    if !spec.rest_routes.is_empty() {
        out.push('\n');
    }

    out
}

fn render_auth_context_helpers() -> &'static str {
    "pub fn current_subject(request_ctx: &roze_context::Context) -> Option<String> {\n    request_ctx\n        .subject()\n        .or_else(|| request_ctx.metadata_value(roze_context::USER_ID_METADATA_KEY))\n}\n\npub fn current_user_id(request_ctx: &roze_context::Context) -> Option<String> {\n    current_subject(request_ctx)\n}\n\npub fn current_admin_id(request_ctx: &roze_context::Context) -> Option<String> {\n    current_subject(request_ctx)\n}\n\npub fn current_tenant(request_ctx: &roze_context::Context) -> Option<String> {\n    request_ctx.tenant()\n}\n\npub fn current_roles(request_ctx: &roze_context::Context) -> Vec<String> {\n    request_ctx.roles()\n}\n\npub fn current_permissions(request_ctx: &roze_context::Context) -> Vec<String> {\n    request_ctx.permissions()\n}\n\npub fn current_scope(request_ctx: &roze_context::Context) -> Option<String> {\n    request_ctx.metadata_value(roze_context::SCOPE_METADATA_KEY)\n}\n\n"
}

pub fn render_logic_group_mods(spec: &ApiSpec) -> Vec<(String, String)> {
    route_groups(spec)
        .into_iter()
        .map(|(group, routes)| {
            let mut out = String::new();
            for route in routes {
                let handler = resolved_handler_name(route);
                out.push_str(&format!("mod {handler};\n"));
                out.push_str(&format!("pub use {handler}::{handler};\n"));
            }
            (group, out)
        })
        .collect()
}

pub fn render_logic_files(spec: &ApiSpec) -> Vec<(String, String, String)> {
    spec.rest_routes
        .iter()
        .map(|route| {
            let group = route_group_name(route);
            let handler = resolved_handler_name(route);
            let content = format!("use super::super::*;\n\n{}", render_logic_fn(route));
            (group, handler, content)
        })
        .collect()
}

pub fn render_openapi(spec: &ApiSpec) -> String {
    let needs_jwt = spec
        .rest_routes
        .iter()
        .any(|route| route_has_jwt(spec, route));
    let mut out = String::from(
        "use std::collections::BTreeMap;\n\nuse roze_openapi::{OpenApiBuilder, Schema, HttpMethod, Operation",
    );
    if needs_jwt {
        out.push_str(", SecurityScheme");
    }
    out.push_str("};\n\n");
    out.push_str("pub fn document() -> serde_json::Value {\n");
    out.push_str(&format!(
        "    let mut builder = OpenApiBuilder::new({:?}, \"0.1.0\").description({:?});\n",
        spec.service,
        spec.server
            .as_ref()
            .and_then(|server| server.group.as_ref())
            .map(|group| format!("service group: {}", group))
            .unwrap_or_else(|| format!("{}/api", spec.service)),
    ));
    if let Some(prefix) = spec
        .server
        .as_ref()
        .and_then(|server| server.prefix.as_deref())
    {
        out.push_str(&format!(
            "    builder = builder.server({:?}, {:?});\n",
            prefix,
            format!("service: {}", spec.service)
        ));
    } else {
        out.push_str(&format!(
            "    builder = builder.server({:?}, {:?});\n",
            "/",
            format!("service: {}", spec.service)
        ));
    }

    if needs_jwt {
        out.push_str(
            "    builder = builder.security_scheme(\"bearerAuth\", SecurityScheme::Http { scheme: \"bearer\".to_string(), bearer_format: Some(\"JWT\".to_string()) });\n",
        );
    }

    for ty in &spec.types {
        out.push_str("    {\n");
        let fields = expanded_type_fields(spec, ty);
        if fields.is_empty() {
            out.push_str("        let properties = BTreeMap::new();\n");
        } else {
            out.push_str("        let mut properties = BTreeMap::new();\n");
        }
        let mut required = Vec::new();
        for field in fields {
            out.push_str(&format!(
                "        properties.insert({:?}.to_string(), {});\n",
                field_wire_name(field),
                openapi_schema_expr(&field.ty)
            ));
            required.push(field_wire_name(field));
        }
        out.push_str(&format!(
            "        builder = builder.component_schema({:?}, Schema::object(properties, vec![{}]));\n",
            ty.name,
            required
                .iter()
                .map(|name| format!("{:?}.to_string()", name))
                .collect::<Vec<_>>()
                .join(", ")
        ));
        out.push_str("    }\n");
    }

    out.push_str(&render_report_chart_openapi(spec));

    for route in &spec.rest_routes {
        let route_spec = route_request_spec(spec, route).expect("request spec");
        let operation_id = route
            .handler
            .clone()
            .unwrap_or_else(|| handler_name(&route.method, &route.path));

        out.push_str(&format!("    let op = Operation::new({:?})", operation_id));
        if let Some(doc) = &route.doc {
            out.push_str(&format!(".summary({:?})", doc));
        }
        out.push_str(&format!(".tag({:?})", spec.service));
        if route_has_jwt(spec, route) {
            out.push_str(".require_security(\"bearerAuth\")");
        }
        if !route.permissions.is_empty() {
            out.push_str(&format!(
                ".extension(\"x-roze-permissions\", serde_json::json!({:?}))",
                route.permissions
            ));
        }
        out.push_str(
            ".parameter(\"x-roze-locale\", roze_openapi::ParameterLocation::Header, \"String\", false)",
        );

        for source in [
            FieldSource::Path,
            FieldSource::Query,
            FieldSource::Header,
            FieldSource::Form,
        ] {
            if let Some(fields) = route_spec.groups.get(&source) {
                for field in fields {
                    let location = match source {
                        FieldSource::Path => "roze_openapi::ParameterLocation::Path",
                        FieldSource::Query => "roze_openapi::ParameterLocation::Query",
                        FieldSource::Header => "roze_openapi::ParameterLocation::Header",
                        FieldSource::Form | FieldSource::Json | FieldSource::Auto => {
                            "roze_openapi::ParameterLocation::Query"
                        }
                    };
                    out.push_str(&format!(
                        ".parameter({:?}, {}, {:?}, true)",
                        field_wire_name(field),
                        location,
                        map_type(&field.ty)
                    ));
                }
            }
        }

        if route_spec.groups.contains_key(&FieldSource::Json)
            || (!route_spec.groups.is_empty()
                && matches!(
                    route.method,
                    HttpMethod::Post | HttpMethod::Put | HttpMethod::Patch
                ))
        {
            out.push_str(&format!(".request_body({:?})", route.request));
        }

        out.push_str(&format!(
            ".response(\"200\", \"OK\", {:?});\n",
            route.response
        ));
        out.push_str(&format!(
            "    builder.add_operation({:?}, HttpMethod::{}, op);\n",
            full_route_path_for_route(spec, route),
            match route.method {
                HttpMethod::Get => "Get",
                HttpMethod::Head => "Head",
                HttpMethod::Post => "Post",
                HttpMethod::Put => "Put",
                HttpMethod::Patch => "Patch",
                HttpMethod::Delete => "Delete",
            }
        ));
    }

    out.push_str("    roze_openapi::to_json_value(&builder.finish())\n}\n");
    out
}

fn render_report_chart_openapi(spec: &ApiSpec) -> String {
    let report_path = full_route_path(spec, "/reports/export");
    let chart_path = full_route_path(spec, "/charts/query");
    format!(
        r#"    {{
        let mut properties = BTreeMap::new();
        properties.insert("report".to_string(), Schema::string());
        properties.insert("format".to_string(), Schema::string());
        properties.insert("status".to_string(), Schema::string());
        properties.insert("export_id".to_string(), Schema::string());
        properties.insert("download_url".to_string(), Schema::string());
        properties.insert("from".to_string(), Schema::string());
        properties.insert("to".to_string(), Schema::string());
        properties.insert("filters".to_string(), Schema::string());
        properties.insert("columns".to_string(), Schema::array(Schema::string()));
        builder = builder.component_schema("ReportExportResponse", Schema::object(properties, vec!["report".to_string(), "format".to_string(), "status".to_string(), "export_id".to_string(), "columns".to_string()]));
    }}
    {{
        let mut properties = BTreeMap::new();
        properties.insert("timestamp".to_string(), Schema::string());
        properties.insert("value".to_string(), Schema::number("double"));
        properties.insert("labels".to_string(), Schema::object(BTreeMap::new(), Vec::new()));
        builder = builder.component_schema("ChartPoint", Schema::object(properties, vec!["timestamp".to_string(), "value".to_string(), "labels".to_string()]));
    }}
    {{
        let mut properties = BTreeMap::new();
        properties.insert("name".to_string(), Schema::string());
        properties.insert("points".to_string(), Schema::array(Schema::reference("ChartPoint")));
        builder = builder.component_schema("ChartSeries", Schema::object(properties, vec!["name".to_string(), "points".to_string()]));
    }}
    {{
        let mut properties = BTreeMap::new();
        properties.insert("chart".to_string(), Schema::string());
        properties.insert("interval".to_string(), Schema::string());
        properties.insert("from".to_string(), Schema::string());
        properties.insert("to".to_string(), Schema::string());
        properties.insert("filters".to_string(), Schema::string());
        properties.insert("series".to_string(), Schema::array(Schema::reference("ChartSeries")));
        builder = builder.component_schema("ChartQueryResponse", Schema::object(properties, vec!["chart".to_string(), "series".to_string()]));
    }}
    let op = Operation::new("reportExport")
        .summary("Export report data")
        .tag({service:?})
        .parameter("report", roze_openapi::ParameterLocation::Query, "String", false)
        .parameter("format", roze_openapi::ParameterLocation::Query, "String", false)
        .parameter("from", roze_openapi::ParameterLocation::Query, "String", false)
        .parameter("to", roze_openapi::ParameterLocation::Query, "String", false)
        .parameter("filters", roze_openapi::ParameterLocation::Query, "String", false)
        .response("200", "OK", "ReportExportResponse");
    builder.add_operation({report_path:?}, HttpMethod::Get, op);
    let op = Operation::new("chartQuery")
        .summary("Query chart series")
        .tag({service:?})
        .parameter("chart", roze_openapi::ParameterLocation::Query, "String", false)
        .parameter("from", roze_openapi::ParameterLocation::Query, "String", false)
        .parameter("to", roze_openapi::ParameterLocation::Query, "String", false)
        .parameter("interval", roze_openapi::ParameterLocation::Query, "String", false)
        .parameter("filters", roze_openapi::ParameterLocation::Query, "String", false)
        .response("200", "OK", "ChartQueryResponse");
    builder.add_operation({chart_path:?}, HttpMethod::Get, op);
"#,
        service = spec.service,
        report_path = report_path,
        chart_path = chart_path
    )
}

fn render_route_handler(spec: &ApiSpec, route: &crate::parser::RestRoute) -> String {
    let request_ty = spec
        .types
        .iter()
        .find(|ty| ty.name == route.request)
        .unwrap_or_else(|| panic!("missing request type `{}`", route.request));
    let route_spec = route_request_spec(spec, route).expect("request spec");
    validate_route_bindings(route, &route_spec);
    let handler = resolved_handler_name(route);
    let middlewares = route_middlewares(spec, route);
    let plan = roze_middleware::resolve_middleware_plan(&middlewares);
    let uses_auth = route_uses_auth(spec, route);
    let uses_idempotency = plan
        .builtins
        .contains(&roze_middleware::BuiltInMiddleware::Idempotency);
    let custom = plan
        .custom
        .into_iter()
        .map(|name| to_snake_case(&name))
        .collect::<Vec<_>>();

    let mut out = String::new();
    if let Some(doc) = &route.doc {
        out.push_str(&format!("/// {}\n", escape_doc(doc)));
    }

    for source in [
        FieldSource::Path,
        FieldSource::Query,
        FieldSource::Form,
        FieldSource::Json,
    ] {
        if let Some(fields) = route_spec.groups.get(&source) {
            let struct_name = partial_struct_name(&handler, &request_ty.name, source);
            out.push_str(&render_partial_struct(&struct_name, fields, source));
        }
    }

    let mut params = vec![
        "State(ctx): State<ServiceContext>".to_string(),
        "Extension(request_ctx): Extension<Context>".to_string(),
    ];
    if route_spec.groups.contains_key(&FieldSource::Path) {
        params.push(format!(
            "Path(path): Path<{}>",
            partial_struct_name(&handler, &request_ty.name, FieldSource::Path)
        ));
    }
    if route_spec.groups.contains_key(&FieldSource::Query) {
        params.push(format!(
            "Query(query): Query<{}>",
            partial_struct_name(&handler, &request_ty.name, FieldSource::Query)
        ));
    }
    if route_spec.groups.contains_key(&FieldSource::Form) {
        params.push(format!(
            "Form(form): Form<{}>",
            partial_struct_name(&handler, &request_ty.name, FieldSource::Form)
        ));
    }
    if route_spec.groups.contains_key(&FieldSource::Json) {
        params.push(format!(
            "Json(body): Json<{}>",
            partial_struct_name(&handler, &request_ty.name, FieldSource::Json)
        ));
    }
    if route_spec.has_header || uses_auth || uses_idempotency {
        params.push("headers: HeaderMap".to_string());
    }

    out.push_str(&format!(
        "pub(crate) async fn {handler}({params}) -> Result<ApiResponse<{response}>, RozeError> {{\n",
        handler = handler,
        params = params.join(", "),
        response = route.response
    ));
    out.push_str(&format!(
        "    let (request_ctx, route_guard) = roze_middleware::begin_route(ctx.config.name.clone(), {:?}, {:?}, request_ctx, Some(&ctx.config.governance))?;\n",
        handler,
        http_method_name(&route.method)
    ));
    if uses_auth {
        out.push_str("    let request_ctx = match authorize(&headers, &ctx) {\n        Ok(auth) => request_ctx.with_auth(auth),\n        Err(err) => {\n            roze_middleware::finish_route(route_guard, false, err.code().to_string());\n            return Err(err);\n        }\n    };\n");
    }
    if !route.permissions.is_empty() {
        out.push_str(&format!(
            "    if let Err(err) = roze_middleware::enforce_permissions(&request_ctx, &[{}]) {{\n        roze_middleware::finish_route(route_guard, false, err.code().to_string());\n        return Err(err);\n    }}\n",
            route
                .permissions
                .iter()
                .map(|permission| format!("{permission:?}"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    for name in custom {
        out.push_str(&format!(
            "    if let Err(err) = crate::middleware::{name}(&ctx, &request_ctx).await {{\n        roze_middleware::finish_route(route_guard, false, err.code().to_string());\n        return Err(err);\n    }}\n"
        ));
    }
    for source in [
        FieldSource::Path,
        FieldSource::Query,
        FieldSource::Form,
        FieldSource::Json,
    ] {
        if route_spec.groups.contains_key(&source) {
            let var = match source {
                FieldSource::Path => "path",
                FieldSource::Query => "query",
                FieldSource::Form => "form",
                FieldSource::Json => "body",
                FieldSource::Header | FieldSource::Auto => unreachable!(),
            };
            out.push_str(&format!(
                    "    if let Err(message) = roze_validation::validate_or_message_i18n(&{var}, roze_error::current_locale().as_deref()) {{\n        let err = RozeError::BadRequest(message);\n        roze_middleware::finish_route(route_guard, false, err.code().to_string());\n        return Err(err);\n    }}\n",
                var = var
            ));
        }
    }
    if request_ty.fields.is_empty() {
        out.push_str(&format!(
            "    let req = {request} {{}};\n",
            request = route.request
        ));
    } else {
        out.push_str(&format!("    let req = {} {{\n", route.request));
        for field in &request_ty.fields {
            let value = field_value_expr(spec, field, route, &route_spec);
            out.push_str(&format!("        {}: {},\n", rust_field_name(field), value));
        }
        out.push_str("    };\n");
    }
    out.push_str(&render_request_validation_checks(&request_ty.fields));
    if uses_idempotency {
        out.push_str(&format!(
            "    let idempotency_key: String = header_value(&headers, \"idempotency-key\").map_err(|_| roze_middleware::idempotency_error(400, roze_middleware::IDEMPOTENCY_MISSING_KEY, \"missing Idempotency-Key header\"))?;\n    let idempotency_fingerprint = roze_middleware::idempotency_fingerprint(&req)?;\n    match roze_middleware::begin_idempotency(ctx.idempotency.as_ref(), {handler:?}, &idempotency_key, &idempotency_fingerprint, roze_middleware::idempotency_now_millis()).await? {{\n        roze_middleware::IdempotencyDecision::Execute => {{}}\n        roze_middleware::IdempotencyDecision::Replay(value) => {{\n            let resp = serde_json::from_value(value).map_err(|err| roze_middleware::idempotency_error(500, roze_middleware::IDEMPOTENCY_REPLAY_INVALID, &format!(\"invalid idempotency replay response: {{err}}\")))?;\n            roze_middleware::finish_route(route_guard, true, \"200\");\n            return Ok(ApiResponse::ok(resp));\n        }}\n        roze_middleware::IdempotencyDecision::InFlight => {{\n            let err = roze_middleware::idempotency_error(409, roze_middleware::IDEMPOTENCY_IN_FLIGHT, \"idempotency request is in progress\");\n            roze_middleware::finish_route(route_guard, false, err.code().to_string());\n            return Err(err);\n        }}\n        roze_middleware::IdempotencyDecision::Conflict => {{\n            let err = roze_middleware::idempotency_error(409, roze_middleware::IDEMPOTENCY_KEY_REUSED, \"idempotency key was reused with a different request\");\n            roze_middleware::finish_route(route_guard, false, err.code().to_string());\n            return Err(err);\n        }}\n    }}\n",
            handler = handler
        ));
    }
    out.push_str(&format!(
        "    let timeout_enabled = ctx.config.rest.as_ref().is_none_or(|rest| rest.middlewares.timeout);\n    let timeout = timeout_enabled.then(|| request_ctx.remaining_timeout()).flatten();\n    let logic = crate::logic::{handler}(ctx.clone(), request_ctx, req);\n    let result = match timeout {{\n        Some(timeout) => match tokio::time::timeout(timeout, logic).await {{\n            Ok(result) => result,\n            Err(_) => Err(RozeError::Internal(\"request timeout\".to_string())),\n        }},\n        None => logic.await,\n    }};\n",
        handler = handler
    ));
    if uses_idempotency {
        out.push_str(&format!(
            "    match result {{\n        Ok(resp) => {{\n            let value = serde_json::to_value(&resp).map_err(|err| roze_middleware::idempotency_error(500, roze_middleware::IDEMPOTENCY_REPLAY_INVALID, &format!(\"invalid idempotency response: {{err}}\")))?;\n            roze_middleware::complete_idempotency(ctx.idempotency.as_ref(), {handler:?}, &idempotency_key, &idempotency_fingerprint, value).await?;\n            roze_middleware::finish_route(route_guard, true, \"200\");\n            Ok(ApiResponse::ok(resp))\n        }}\n        Err(mut err) => {{\n            roze_middleware::fail_idempotency(ctx.idempotency.as_ref(), {handler:?}, &idempotency_key, &idempotency_fingerprint).await;\n            err = roze_middleware::apply_fallback(\n                ctx.config.name.as_str(),\n                err,\n                roze_middleware::route_fallback(Some(&ctx.config.governance), {handler:?}),\n            );\n            roze_middleware::finish_route(route_guard, false, err.code().to_string());\n            Err(err)\n        }}\n    }}\n",
            handler = handler
        ));
    } else {
        out.push_str(&format!(
            "    match result {{\n        Ok(resp) => {{\n            roze_middleware::finish_route(route_guard, true, \"200\");\n            Ok(ApiResponse::ok(resp))\n        }}\n        Err(mut err) => {{\n            err = roze_middleware::apply_fallback(\n                ctx.config.name.as_str(),\n                err,\n                roze_middleware::route_fallback(Some(&ctx.config.governance), {handler:?}),\n            );\n            roze_middleware::finish_route(route_guard, false, err.code().to_string());\n            Err(err)\n        }}\n    }}\n",
            handler = handler
        ));
    }
    out.push_str("}\n\n");

    out
}

fn route_middlewares(spec: &ApiSpec, route: &crate::parser::RestRoute) -> Vec<String> {
    let mut names = spec
        .server
        .as_ref()
        .map(|server| server.middlewares.clone())
        .unwrap_or_default();
    if let Some(server) = &route.server {
        names.extend(server.middlewares.clone());
    }
    names.extend(route.middlewares.clone());
    names
}

fn route_groups(spec: &ApiSpec) -> BTreeMap<String, Vec<&RestRoute>> {
    let mut groups = BTreeMap::<String, Vec<&RestRoute>>::new();
    for route in &spec.rest_routes {
        groups
            .entry(route_group_name(route))
            .or_default()
            .push(route);
    }
    groups
}

fn route_group_name(route: &RestRoute) -> String {
    route_group_segments(route).join("_")
}

fn route_group_segments(route: &RestRoute) -> Vec<String> {
    let segments = route
        .server
        .as_ref()
        .and_then(|server| server.group.as_deref())
        .map(group_segments)
        .filter(|segments| !segments.is_empty())
        .unwrap_or_else(|| {
            route
                .path
                .split('/')
                .find(|segment| !segment.is_empty() && !segment.starts_with(':'))
                .map(|segment| vec![to_snake_case(segment)])
                .unwrap_or_default()
        });

    if segments.is_empty() {
        vec!["base".to_string()]
    } else {
        segments
    }
}

fn group_segments(group: &str) -> Vec<String> {
    group
        .split(['/', '\\', '.', ':'])
        .map(to_snake_case)
        .filter(|segment| !segment.is_empty())
        .collect()
}

fn route_has_jwt(spec: &ApiSpec, route: &crate::parser::RestRoute) -> bool {
    route
        .server
        .as_ref()
        .and_then(|server| server.jwt.as_ref())
        .or_else(|| spec.server.as_ref().and_then(|server| server.jwt.as_ref()))
        .is_some()
}

fn route_uses_auth(spec: &ApiSpec, route: &crate::parser::RestRoute) -> bool {
    route_has_jwt(spec, route)
        || !route.permissions.is_empty()
        || route_middlewares(spec, route).iter().any(|name| {
            roze_middleware::BuiltInMiddleware::parse(name)
                == Some(roze_middleware::BuiltInMiddleware::Auth)
        })
}

fn route_uses_idempotency(spec: &ApiSpec, route: &crate::parser::RestRoute) -> bool {
    route_middlewares(spec, route).iter().any(|name| {
        roze_middleware::BuiltInMiddleware::parse(name)
            == Some(roze_middleware::BuiltInMiddleware::Idempotency)
    })
}

fn custom_middlewares(spec: &ApiSpec) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for route in &spec.rest_routes {
        let plan = roze_middleware::resolve_middleware_plan(&route_middlewares(spec, route));
        for name in plan.custom {
            let name = to_snake_case(&name);
            if seen.insert(name.clone()) {
                out.push(name);
            }
        }
    }
    out
}

fn render_partial_struct(name: &str, fields: &[&Field], source: FieldSource) -> String {
    let mut out = String::new();
    out.push_str("#[derive(Debug, Clone, Deserialize, Validate)]\n");
    out.push_str(&format!("pub(crate) struct {} {{\n", name));
    for field in fields {
        if source == FieldSource::Query {
            out.push_str("    #[serde(default)]\n");
        }
        if field.embedded {
            out.push_str("    #[serde(flatten)]\n");
        } else if let Some(rename) = serde_rename(field) {
            out.push_str(&format!("    #[serde(rename = \"{}\")]\n", rename));
        }
        if let Some(validate) = validation_attr(field) {
            out.push_str(&format!("    #[validate({validate})]\n"));
        }
        out.push_str(&format!(
            "    {}: {},\n",
            rust_field_name(field),
            map_type(&field.ty)
        ));
    }
    out.push_str("}\n\n");
    out
}

fn render_request_validation_checks(fields: &[Field]) -> String {
    let mut out = String::new();
    for field in fields {
        out.push_str(&custom_validation_checks(
            field,
            fields,
            &format!("req.{}", rust_field_name(field)),
        ));
    }
    out
}

fn custom_validation_checks(field: &Field, fields: &[Field], expr: &str) -> String {
    let Some(rules) = field.validate.as_deref() else {
        return String::new();
    };
    if is_optional_rule(rules) {
        return String::new();
    }

    let mut out = String::new();
    let field_name = field_wire_name(field);
    let field_label = format!("field `{field_name}`");
    out.push_str(&cross_field_validation_checks(field, fields, rules, expr));
    if let Some(values) = oneof_values(rules) {
        let allowed = values
            .iter()
            .map(|value| format!("{value:?}"))
            .collect::<Vec<_>>()
            .join(", ");
        let allowed_message = values.join(", ");
        out.push_str(&format!(
            "    {{\n        let value = {expr}.to_string();\n        if ![{allowed}].contains(&value.as_str()) {{\n            let err = RozeError::BadRequest(format!(\"{field_label} must be one of: {{}}\", {allowed_message:?}));\n            roze_middleware::finish_route(route_guard, false, err.code().to_string());\n            return Err(err);\n        }}\n    }}\n"
        ));
    }

    if let Some((key_ty, value_ty)) = map_key_value_types(&field.ty) {
        out.push_str(&map_dive_validation_checks(
            field, rules, expr, &key_ty, &value_ty,
        ));
        return out;
    }

    if let Some(element_ty) = collection_element_type(&field.ty) {
        out.push_str(&dive_validation_checks(field, rules, expr, &element_ty));
        return out;
    }

    if map_type(&field.ty) != "String" {
        return out;
    }
    out.push_str(&conditional_required_checks(field, fields, rules, expr));

    if let Some(prefix) = rule_value(rules, "startswith") {
        out.push_str(&format!(
            "    if !{expr}.starts_with({prefix:?}) {{\n        let err = RozeError::BadRequest(format!(\"{field_label} must start with {{}}\", {prefix:?}));\n        roze_middleware::finish_route(route_guard, false, err.code().to_string());\n        return Err(err);\n    }}\n"
        ));
    }
    if let Some(suffix) = rule_value(rules, "endswith") {
        out.push_str(&format!(
            "    if !{expr}.ends_with({suffix:?}) {{\n        let err = RozeError::BadRequest(format!(\"{field_label} must end with {{}}\", {suffix:?}));\n        roze_middleware::finish_route(route_guard, false, err.code().to_string());\n        return Err(err);\n    }}\n"
        ));
    }
    if has_rule(rules, "alpha") {
        out.push_str(&format!(
            "    if !{expr}.chars().all(|ch| ch.is_alphabetic()) {{\n        let err = RozeError::BadRequest(\"{field_label} must contain letters only\".to_string());\n        roze_middleware::finish_route(route_guard, false, err.code().to_string());\n        return Err(err);\n    }}\n"
        ));
    }
    if has_rule(rules, "alphanum") {
        out.push_str(&format!(
            "    if !{expr}.chars().all(|ch| ch.is_alphanumeric()) {{\n        let err = RozeError::BadRequest(\"{field_label} must contain letters and numbers only\".to_string());\n        roze_middleware::finish_route(route_guard, false, err.code().to_string());\n        return Err(err);\n    }}\n"
        ));
    }
    if has_rule(rules, "ascii") {
        out.push_str(&format!(
            "    if !{expr}.is_ascii() {{\n        let err = RozeError::BadRequest(\"{field_label} must contain ASCII characters only\".to_string());\n        roze_middleware::finish_route(route_guard, false, err.code().to_string());\n        return Err(err);\n    }}\n"
        ));
    }
    if has_rule(rules, "code") {
        out.push_str(&format!(
            "    if {expr}.is_empty() || !{expr}.chars().all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.')) {{\n        let err = RozeError::BadRequest(\"{field_label} must be a valid code\".to_string());\n        roze_middleware::finish_route(route_guard, false, err.code().to_string());\n        return Err(err);\n    }}\n"
        ));
    }
    if has_rule(rules, "json") {
        out.push_str(&format!(
            "    if serde_json::from_str::<serde_json::Value>(&{expr}).is_err() {{\n        let err = RozeError::BadRequest(\"{field_label} must contain valid JSON\".to_string());\n        roze_middleware::finish_route(route_guard, false, err.code().to_string());\n        return Err(err);\n    }}\n"
        ));
    }
    if has_rule(rules, "numeric") {
        out.push_str(&format!(
            "    if {expr}.parse::<f64>().is_err() {{\n        let err = RozeError::BadRequest(\"{field_label} must be numeric\".to_string());\n        roze_middleware::finish_route(route_guard, false, err.code().to_string());\n        return Err(err);\n    }}\n"
        ));
    }
    if has_rule(rules, "lowercase") {
        out.push_str(&format!(
            "    if {expr}.chars().any(|ch| ch.is_uppercase()) {{\n        let err = RozeError::BadRequest(\"{field_label} must be lowercase\".to_string());\n        roze_middleware::finish_route(route_guard, false, err.code().to_string());\n        return Err(err);\n    }}\n"
        ));
    }
    if has_rule(rules, "uppercase") {
        out.push_str(&format!(
            "    if {expr}.chars().any(|ch| ch.is_lowercase()) {{\n        let err = RozeError::BadRequest(\"{field_label} must be uppercase\".to_string());\n        roze_middleware::finish_route(route_guard, false, err.code().to_string());\n        return Err(err);\n    }}\n"
        ));
    }

    out
}

fn dive_validation_checks(field: &Field, rules: &str, expr: &str, element_ty: &str) -> String {
    let Some(element_rules) = rules_after_dive(rules) else {
        return String::new();
    };
    if element_rules.is_empty() {
        return String::new();
    }

    let field_name = field_wire_name(field);
    let field_label = format!("field `{field_name}` item");
    let body = dive_element_body(element_rules, "item", element_ty, &field_label, "        ");

    if body.is_empty() {
        String::new()
    } else {
        format!("    for item in &{expr} {{\n{body}    }}\n")
    }
}

fn map_dive_validation_checks(
    field: &Field,
    rules: &str,
    expr: &str,
    key_ty: &str,
    value_ty: &str,
) -> String {
    let Some(element_rules) = rules_after_dive(rules) else {
        return String::new();
    };
    let (key_rules, value_rules) = split_map_dive_rules(element_rules);
    let field_name = field_wire_name(field);
    let mut body = String::new();
    if let Some(key_rules) = key_rules {
        body.push_str(&dive_element_body(
            &key_rules,
            "key",
            key_ty,
            &format!("field `{field_name}` key"),
            "        ",
        ));
    }
    if let Some(value_rules) = value_rules {
        body.push_str(&dive_element_body(
            &value_rules,
            "value",
            value_ty,
            &format!("field `{field_name}` value"),
            "        ",
        ));
    }

    if body.is_empty() {
        String::new()
    } else {
        format!("    for (key, value) in &{expr} {{\n{body}    }}\n")
    }
}

fn dive_element_body(rules: &str, var: &str, ty: &str, field_label: &str, indent: &str) -> String {
    let mut body = String::new();

    if let Some(values) = oneof_values(rules) {
        let allowed = values
            .iter()
            .map(|value| format!("{value:?}"))
            .collect::<Vec<_>>()
            .join(", ");
        let allowed_message = values.join(", ");
        body.push_str(&format!(
            "{indent}{{\n{indent}    let value = {var}.to_string();\n{indent}    if ![{allowed}].contains(&value.as_str()) {{\n{indent}        let err = RozeError::BadRequest(format!(\"{field_label} must be one of: {{}}\", {allowed_message:?}));\n{indent}        roze_middleware::finish_route(route_guard, false, err.code().to_string());\n{indent}        return Err(err);\n{indent}    }}\n{indent}}}\n"
        ));
    }

    match map_type(ty).as_str() {
        "String" => {
            let (mut min, max) = min_max_rules(rules);
            let equal = rule_value(rules, "len").and_then(parse_usize);
            if has_rule(rules, "required") {
                min.get_or_insert(1usize);
            }
            if let Some(equal) = equal {
                body.push_str(&format!(
                    "{indent}if {var}.chars().count() != {equal} {{\n{indent}    let err = RozeError::BadRequest(\"{field_label} length is invalid\".to_string());\n{indent}    roze_middleware::finish_route(route_guard, false, err.code().to_string());\n{indent}    return Err(err);\n{indent}}}\n"
                ));
            } else {
                if let Some(min) = min {
                    body.push_str(&format!(
                        "{indent}if {var}.chars().count() < {min} {{\n{indent}    let err = RozeError::BadRequest(\"{field_label} is too short\".to_string());\n{indent}    roze_middleware::finish_route(route_guard, false, err.code().to_string());\n{indent}    return Err(err);\n{indent}}}\n"
                    ));
                }
                if let Some(max) = max {
                    body.push_str(&format!(
                        "{indent}if {var}.chars().count() > {max} {{\n{indent}    let err = RozeError::BadRequest(\"{field_label} is too long\".to_string());\n{indent}    roze_middleware::finish_route(route_guard, false, err.code().to_string());\n{indent}    return Err(err);\n{indent}}}\n"
                    ));
                }
            }
            if has_rule(rules, "alpha") {
                body.push_str(&format!(
                    "{indent}if !{var}.chars().all(|ch| ch.is_alphabetic()) {{\n{indent}    let err = RozeError::BadRequest(\"{field_label} must contain letters only\".to_string());\n{indent}    roze_middleware::finish_route(route_guard, false, err.code().to_string());\n{indent}    return Err(err);\n{indent}}}\n"
                ));
            }
            if has_rule(rules, "alphanum") {
                body.push_str(&format!(
                    "{indent}if !{var}.chars().all(|ch| ch.is_alphanumeric()) {{\n{indent}    let err = RozeError::BadRequest(\"{field_label} must contain letters and numbers only\".to_string());\n{indent}    roze_middleware::finish_route(route_guard, false, err.code().to_string());\n{indent}    return Err(err);\n{indent}}}\n"
                ));
            }
            if has_rule(rules, "ascii") {
                body.push_str(&format!(
                    "{indent}if !{var}.is_ascii() {{\n{indent}    let err = RozeError::BadRequest(\"{field_label} must contain ASCII characters only\".to_string());\n{indent}    roze_middleware::finish_route(route_guard, false, err.code().to_string());\n{indent}    return Err(err);\n{indent}}}\n"
                ));
            }
            if has_rule(rules, "code") {
                body.push_str(&format!(
                    "{indent}if {var}.is_empty() || !{var}.chars().all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.')) {{\n{indent}    let err = RozeError::BadRequest(\"{field_label} must be a valid code\".to_string());\n{indent}    roze_middleware::finish_route(route_guard, false, err.code().to_string());\n{indent}    return Err(err);\n{indent}}}\n"
                ));
            }
            if has_rule(rules, "json") {
                body.push_str(&format!(
                    "{indent}if serde_json::from_str::<serde_json::Value>({var}).is_err() {{\n{indent}    let err = RozeError::BadRequest(\"{field_label} must contain valid JSON\".to_string());\n{indent}    roze_middleware::finish_route(route_guard, false, err.code().to_string());\n{indent}    return Err(err);\n{indent}}}\n"
                ));
            }
            if has_rule(rules, "numeric") {
                body.push_str(&format!(
                    "{indent}if {var}.parse::<f64>().is_err() {{\n{indent}    let err = RozeError::BadRequest(\"{field_label} must be numeric\".to_string());\n{indent}    roze_middleware::finish_route(route_guard, false, err.code().to_string());\n{indent}    return Err(err);\n{indent}}}\n"
                ));
            }
            if has_rule(rules, "lowercase") {
                body.push_str(&format!(
                    "{indent}if {var}.chars().any(|ch| ch.is_uppercase()) {{\n{indent}    let err = RozeError::BadRequest(\"{field_label} must be lowercase\".to_string());\n{indent}    roze_middleware::finish_route(route_guard, false, err.code().to_string());\n{indent}    return Err(err);\n{indent}}}\n"
                ));
            }
            if has_rule(rules, "uppercase") {
                body.push_str(&format!(
                    "{indent}if {var}.chars().any(|ch| ch.is_lowercase()) {{\n{indent}    let err = RozeError::BadRequest(\"{field_label} must be uppercase\".to_string());\n{indent}    roze_middleware::finish_route(route_guard, false, err.code().to_string());\n{indent}    return Err(err);\n{indent}}}\n"
                ));
            }
        }
        "i64" | "u64" | "i32" | "u32" => {
            body.push_str(&numeric_range_checks(rules, var, ty, field_label, indent));
        }
        _ => {}
    }

    body
}

fn numeric_range_checks(
    rules: &str,
    expr: &str,
    ty: &str,
    field_label: &str,
    indent: &str,
) -> String {
    let mut out = String::new();
    if has_rule(rules, "nonnegative") && matches!(map_type(ty).as_str(), "i64" | "i32") {
        out.push_str(&format!(
            "{indent}if {expr} < &0 {{\n{indent}    let err = RozeError::BadRequest(\"{field_label} must be non-negative\".to_string());\n{indent}    roze_middleware::finish_route(route_guard, false, err.code().to_string());\n{indent}    return Err(err);\n{indent}}}\n"
        ));
    }
    if has_rule(rules, "page") || has_rule(rules, "limit") {
        out.push_str(&format!(
            "{indent}if {expr} < &1 {{\n{indent}    let err = RozeError::BadRequest(\"{field_label} must be at least 1\".to_string());\n{indent}    roze_middleware::finish_route(route_guard, false, err.code().to_string());\n{indent}    return Err(err);\n{indent}}}\n"
        ));
    }
    if has_rule(rules, "limit") {
        out.push_str(&format!(
            "{indent}if {expr} > &1000 {{\n{indent}    let err = RozeError::BadRequest(\"{field_label} must not exceed 1000\".to_string());\n{indent}    roze_middleware::finish_route(route_guard, false, err.code().to_string());\n{indent}    return Err(err);\n{indent}}}\n"
        ));
    }
    if let Some(min) = rule_value(rules, "min")
        .or_else(|| rule_value(rules, "gte"))
        .filter(|value| is_number_literal(value))
    {
        out.push_str(&format!(
            "{indent}if {expr} < &{min} {{\n{indent}    let err = RozeError::BadRequest(\"{field_label} is too small\".to_string());\n{indent}    roze_middleware::finish_route(route_guard, false, err.code().to_string());\n{indent}    return Err(err);\n{indent}}}\n"
        ));
    }
    if let Some(max) = rule_value(rules, "max")
        .or_else(|| rule_value(rules, "lte"))
        .filter(|value| is_number_literal(value))
    {
        out.push_str(&format!(
            "{indent}if {expr} > &{max} {{\n{indent}    let err = RozeError::BadRequest(\"{field_label} is too large\".to_string());\n{indent}    roze_middleware::finish_route(route_guard, false, err.code().to_string());\n{indent}    return Err(err);\n{indent}}}\n"
        ));
    }
    if let Some(min) = rule_value(rules, "gt").filter(|value| is_number_literal(value)) {
        out.push_str(&format!(
            "{indent}if {expr} <= &{min} {{\n{indent}    let err = RozeError::BadRequest(\"{field_label} is too small\".to_string());\n{indent}    roze_middleware::finish_route(route_guard, false, err.code().to_string());\n{indent}    return Err(err);\n{indent}}}\n"
        ));
    }
    if let Some(max) = rule_value(rules, "lt").filter(|value| is_number_literal(value)) {
        out.push_str(&format!(
            "{indent}if {expr} >= &{max} {{\n{indent}    let err = RozeError::BadRequest(\"{field_label} is too large\".to_string());\n{indent}    roze_middleware::finish_route(route_guard, false, err.code().to_string());\n{indent}    return Err(err);\n{indent}}}\n"
        ));
    }
    out
}

fn cross_field_validation_checks(
    field: &Field,
    fields: &[Field],
    rules: &str,
    expr: &str,
) -> String {
    let mut out = String::new();
    let field_name = field_wire_name(field);
    for (tag, op, message) in [
        ("eqfield", "==", "must equal"),
        ("nefield", "!=", "must not equal"),
        ("gtfield", ">", "must be greater than"),
        ("gtefield", ">=", "must be greater than or equal to"),
        ("ltfield", "<", "must be less than"),
        ("ltefield", "<=", "must be less than or equal to"),
    ] {
        let Some(other_name) = rule_value(rules, tag) else {
            continue;
        };
        let Some(other) = comparable_field_ref_expr(fields, other_name, &field.ty) else {
            continue;
        };
        out.push_str(&format!(
            "    if !({expr} {op} {other}) {{\n        let err = RozeError::BadRequest(\"field `{field_name}` {message} field `{other_name}`\".to_string());\n        roze_middleware::finish_route(route_guard, false, err.code().to_string());\n        return Err(err);\n    }}\n"
        ));
    }
    out
}

fn conditional_required_checks(field: &Field, fields: &[Field], rules: &str, expr: &str) -> String {
    let mut out = String::new();
    let field_name = field_wire_name(field);

    if let Some(condition) = rule_value(rules, "required_if") {
        let conditions = condition_pairs(condition)
            .into_iter()
            .filter_map(|(other_name, expected)| {
                let other = field_ref_expr(fields, other_name)?;
                Some(format!("{other}.to_string() == {expected:?}"))
            })
            .collect::<Vec<_>>();
        if !conditions.is_empty() {
            out.push_str(&format!(
                "    if ({}) && {expr}.is_empty() {{\n        let err = RozeError::BadRequest(\"field `{field_name}` is required\".to_string());\n        roze_middleware::finish_route(route_guard, false, err.code().to_string());\n        return Err(err);\n    }}\n",
                conditions.join(" && ")
            ));
        }
    }

    if let Some(condition) = rule_value(rules, "required_unless") {
        let conditions = condition_pairs(condition)
            .into_iter()
            .filter_map(|(other_name, expected)| {
                let other = field_ref_expr(fields, other_name)?;
                Some(format!("{other}.to_string() == {expected:?}"))
            })
            .collect::<Vec<_>>();
        if !conditions.is_empty() {
            out.push_str(&format!(
                "    if !({}) && {expr}.is_empty() {{\n        let err = RozeError::BadRequest(\"field `{field_name}` is required\".to_string());\n        roze_middleware::finish_route(route_guard, false, err.code().to_string());\n        return Err(err);\n    }}\n",
                conditions.join(" || ")
            ));
        }
    }

    if let Some(names) = rule_value(rules, "required_with") {
        let conditions = condition_names(names)
            .into_iter()
            .filter_map(|other_name| {
                let other = field_ref_expr(fields, other_name)?;
                Some(format!("!{other}.to_string().is_empty()"))
            })
            .collect::<Vec<_>>();
        if !conditions.is_empty() {
            out.push_str(&format!(
                "    if ({}) && {expr}.is_empty() {{\n        let err = RozeError::BadRequest(\"field `{field_name}` is required\".to_string());\n        roze_middleware::finish_route(route_guard, false, err.code().to_string());\n        return Err(err);\n    }}\n",
                conditions.join(" || ")
            ));
        }
    }

    if let Some(names) = rule_value(rules, "required_without") {
        let conditions = condition_names(names)
            .into_iter()
            .filter_map(|other_name| {
                let other = field_ref_expr(fields, other_name)?;
                Some(format!("{other}.to_string().is_empty()"))
            })
            .collect::<Vec<_>>();
        if !conditions.is_empty() {
            out.push_str(&format!(
                "    if ({}) && {expr}.is_empty() {{\n        let err = RozeError::BadRequest(\"field `{field_name}` is required\".to_string());\n        roze_middleware::finish_route(route_guard, false, err.code().to_string());\n        return Err(err);\n    }}\n",
                conditions.join(" || ")
            ));
        }
    }

    out
}

fn field_ref_expr(fields: &[Field], name: &str) -> Option<String> {
    field_by_name(fields, name).map(|field| format!("req.{}", rust_field_name(field)))
}

fn comparable_field_ref_expr(fields: &[Field], name: &str, ty: &str) -> Option<String> {
    field_by_name(fields, name)
        .filter(|field| map_type(&field.ty) == map_type(ty))
        .map(|field| format!("req.{}", rust_field_name(field)))
}

fn field_by_name<'a>(fields: &'a [Field], name: &str) -> Option<&'a Field> {
    fields.iter().find(|field| {
        field.name == name
            || rust_field_name(field) == name
            || field.wire_name.as_deref() == Some(name)
            || field.json_name.as_deref() == Some(name)
    })
}

fn field_value_expr(
    api: &ApiSpec,
    field: &Field,
    route: &crate::parser::RestRoute,
    spec: &RouteRequestSpec<'_>,
) -> String {
    if field.embedded {
        let Some(ty) = find_type(api, &field.ty) else {
            return format!("{} {{}}", field.ty);
        };
        let fields = ty
            .fields
            .iter()
            .map(|nested| {
                format!(
                    "{}: {}",
                    rust_field_name(nested),
                    field_value_expr(api, nested, route, spec)
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        return format!("{} {{ {fields} }}", field.ty);
    }

    match resolve_field_source(field, route, spec) {
        FieldSource::Path => format!("path.{}", rust_field_name(field)),
        FieldSource::Query => format!("query.{}", rust_field_name(field)),
        FieldSource::Form => format!("form.{}", rust_field_name(field)),
        FieldSource::Json => format!("body.{}", rust_field_name(field)),
        FieldSource::Header => format!(
            "header_value::<{}>(&headers, \"{}\")?",
            map_type(&field.ty),
            field_wire_name(field)
        ),
        FieldSource::Auto => unreachable!(),
    }
}

fn serde_rename(field: &Field) -> Option<&str> {
    let rename = field.json_name.as_deref().or(field.wire_name.as_deref())?;
    if matches!(field.source, FieldSource::Header) {
        return None;
    }
    if rename == rust_field_name(field) {
        None
    } else {
        Some(rename)
    }
}

fn route_request_spec<'a>(
    spec: &'a ApiSpec,
    route: &'a crate::parser::RestRoute,
) -> Option<RouteRequestSpec<'a>> {
    let request_ty = spec.types.iter().find(|ty| ty.name == route.request)?;
    let path_params = route_path_params(&route.path);
    let mut groups: HashMap<FieldSource, Vec<&'a Field>> = HashMap::new();
    let mut has_header = false;

    for field in expanded_type_fields(spec, request_ty) {
        let source = resolve_field_source(
            field,
            route,
            &RouteRequestSpec {
                groups: HashMap::new(),
                has_header: false,
                path_params: path_params.clone(),
            },
        );
        if source == FieldSource::Header {
            has_header = true;
        } else {
            groups.entry(source).or_default().push(field);
        }
    }

    Some(RouteRequestSpec {
        groups,
        has_header,
        path_params,
    })
}

fn validate_route_bindings(route: &crate::parser::RestRoute, spec: &RouteRequestSpec<'_>) {
    let path_fields = spec
        .groups
        .get(&FieldSource::Path)
        .cloned()
        .unwrap_or_default();

    for param in &spec.path_params {
        let matched = path_fields
            .iter()
            .any(|field| normalize_ident(&field_wire_name(field)) == *param);
        assert!(
            matched,
            "route `{}` path parameter `{}` has no matching `path:` field",
            route.path, param
        );
    }

    for field in path_fields {
        let name = normalize_ident(&field_wire_name(field));
        assert!(
            spec.path_params.contains(&name),
            "route `{}` field `{}` is tagged as path but route is missing `:{}` or `{{{}}}`",
            route.path,
            field.name,
            field_wire_name(field),
            field_wire_name(field)
        );
    }
}

struct RouteRequestSpec<'a> {
    groups: HashMap<FieldSource, Vec<&'a Field>>,
    has_header: bool,
    path_params: HashSet<String>,
}

fn find_type<'a>(spec: &'a ApiSpec, name: &str) -> Option<&'a TypeDef> {
    spec.types.iter().find(|ty| ty.name == name)
}

fn expanded_type_fields<'a>(spec: &'a ApiSpec, ty: &'a TypeDef) -> Vec<&'a Field> {
    let mut fields = Vec::new();
    let mut stack = HashSet::new();
    expand_type_fields(spec, ty, &mut stack, &mut fields);
    fields
}

fn expand_type_fields<'a>(
    spec: &'a ApiSpec,
    ty: &'a TypeDef,
    stack: &mut HashSet<String>,
    fields: &mut Vec<&'a Field>,
) {
    if !stack.insert(ty.name.clone()) {
        return;
    }
    for field in &ty.fields {
        if field.embedded {
            if let Some(nested) = find_type(spec, &field.ty) {
                expand_type_fields(spec, nested, stack, fields);
                continue;
            }
        }
        fields.push(field);
    }
    stack.remove(&ty.name);
}

fn resolve_field_source(
    field: &Field,
    route: &crate::parser::RestRoute,
    spec: &RouteRequestSpec<'_>,
) -> FieldSource {
    match field.source {
        FieldSource::Auto => {
            let name = normalize_ident(&field_wire_name(field));
            if spec.path_params.contains(&name) {
                FieldSource::Path
            } else if matches!(
                route.method,
                HttpMethod::Get | HttpMethod::Head | HttpMethod::Delete
            ) {
                FieldSource::Query
            } else {
                FieldSource::Json
            }
        }
        other => other,
    }
}

fn field_wire_name(field: &Field) -> String {
    field
        .wire_name
        .as_deref()
        .or(field.json_name.as_deref())
        .map(ToString::to_string)
        .unwrap_or_else(|| rust_field_name(field))
}

fn rust_field_name(field: &Field) -> String {
    rust_identifier(&field.name)
}

fn partial_struct_name(handler: &str, name: &str, source: FieldSource) -> String {
    let suffix = match source {
        FieldSource::Path => "Path",
        FieldSource::Query => "Query",
        FieldSource::Form => "Form",
        FieldSource::Json => "Json",
        FieldSource::Header => "Header",
        FieldSource::Auto => "Auto",
    };
    format!("{}{}{}", to_pascal_case(handler), name, suffix)
}

fn route_path_params(path: &str) -> HashSet<String> {
    let mut names = HashSet::new();
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
                    names.insert(normalize_ident(&name));
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
                    names.insert(normalize_ident(&name));
                }
            }
            _ => {}
        }
    }

    names
}

fn normalize_ident(input: &str) -> String {
    input.replace('-', "_")
}

fn handler_name(method: &HttpMethod, path: &str) -> String {
    let method = match method {
        HttpMethod::Get => "get",
        HttpMethod::Head => "head",
        HttpMethod::Post => "post",
        HttpMethod::Put => "put",
        HttpMethod::Patch => "patch",
        HttpMethod::Delete => "delete",
    };
    let path_name = path
        .trim_matches('/')
        .replace(':', "")
        .replace(['{', '}'], "")
        .replace(['/', '-'], "_");

    format!("{}_{}", method, path_name)
}

pub fn handler_name_for_openapi(method: &HttpMethod, path: &str) -> String {
    handler_name(method, path)
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

fn resolved_handler_name(route: &crate::parser::RestRoute) -> String {
    route
        .handler
        .as_ref()
        .map(|handler| to_snake_case(handler))
        .unwrap_or_else(|| handler_name(&route.method, &route.path))
}

pub(crate) fn full_route_path(spec: &ApiSpec, path: &str) -> String {
    let prefix = spec
        .server
        .as_ref()
        .and_then(|server| server.prefix.as_deref())
        .unwrap_or("");

    if prefix.is_empty() {
        return path.to_string();
    }

    let prefix = prefix.trim_end_matches('/');
    let path = path.trim_start_matches('/');
    format!("{prefix}/{path}")
}

pub fn full_route_path_for_route(spec: &ApiSpec, route: &RestRoute) -> String {
    let prefix = route
        .server
        .as_ref()
        .and_then(|server| server.prefix.as_deref())
        .or_else(|| {
            spec.server
                .as_ref()
                .and_then(|server| server.prefix.as_deref())
        })
        .unwrap_or("");

    if prefix.is_empty() {
        return route.path.to_string();
    }

    let prefix = prefix.trim_end_matches('/');
    let path = route.path.trim_start_matches('/');
    format!("{prefix}/{path}")
}

pub fn full_route_path_for_openapi(spec: &ApiSpec, route: &RestRoute) -> String {
    full_route_path_for_route(spec, route)
}

fn roze_http_route_path(path: &str) -> String {
    path.split('/')
        .map(|segment| {
            segment
                .strip_prefix(':')
                .map(|name| format!("{{{name}}}"))
                .unwrap_or_else(|| segment.to_string())
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn escape_doc(doc: &str) -> String {
    doc.replace('\\', "\\\\").replace('"', "\\\"")
}

fn map_type(ty: &str) -> String {
    let ty = ty.trim();
    if let Some((key, value)) = map_key_value_types(ty) {
        return format!(
            "std::collections::HashMap<{}, {}>",
            map_type(&key),
            map_type(&value)
        );
    }
    if let Some(inner) = collection_element_type(ty) {
        return format!("Vec<{}>", map_type(&inner));
    }

    match ty {
        "string" => "String".to_string(),
        "int" | "int64" => "i64".to_string(),
        "int32" => "i32".to_string(),
        "int16" => "i16".to_string(),
        "int8" => "i8".to_string(),
        "uint" | "uint64" => "u64".to_string(),
        "uint32" => "u32".to_string(),
        "uint16" => "u16".to_string(),
        "uint8" => "u8".to_string(),
        "float" => "f32".to_string(),
        "double" => "f64".to_string(),
        "bool" => "bool".to_string(),
        other => other.to_string(),
    }
}

fn openapi_schema_expr(ty: &str) -> String {
    if map_key_value_types(ty).is_some() {
        return "Schema::object(BTreeMap::new(), Vec::new())".to_string();
    }
    if let Some(inner) = collection_element_type(ty) {
        return format!("Schema::array({})", openapi_schema_expr(&inner));
    }

    match ty {
        "String" | "string" => "Schema::string()".to_string(),
        "bool" => "Schema::boolean()".to_string(),
        "i32" | "int32" => "Schema::integer(\"int32\")".to_string(),
        "i64" | "int" | "int64" => "Schema::integer(\"int64\")".to_string(),
        "u32" | "uint32" => "Schema::integer(\"uint32\")".to_string(),
        "u64" | "uint" | "uint64" => "Schema::integer(\"uint64\")".to_string(),
        "f32" | "float" => "Schema::number(\"float\")".to_string(),
        "f64" | "double" => "Schema::number(\"double\")".to_string(),
        other => format!("Schema::reference({other:?})"),
    }
}

fn validation_attr(field: &Field) -> Option<String> {
    let rules = field.validate.as_deref()?;
    if is_optional_rule(rules) {
        return None;
    }

    if map_key_value_types(&field.ty).is_some() || collection_element_type(&field.ty).is_some() {
        return collection_validation_attr(rules_before_dive(rules));
    }

    match map_type(&field.ty).as_str() {
        "String" => string_validation_attr(rules),
        "i64" | "u64" | "i32" | "u32" => number_validation_attr(rules),
        _ => None,
    }
}

fn string_validation_attr(rules: &str) -> Option<String> {
    let mut attrs = Vec::new();
    if has_rule(rules, "email") {
        attrs.push("email".to_string());
    }
    if has_rule(rules, "url") || has_rule(rules, "uri") {
        attrs.push("url".to_string());
    }
    if has_rule(rules, "ip") {
        attrs.push("ip".to_string());
    } else if has_rule(rules, "ipv4") {
        attrs.push("ip(v4 = true)".to_string());
    } else if has_rule(rules, "ipv6") {
        attrs.push("ip(v6 = true)".to_string());
    }
    if let Some(pattern) = rule_value(rules, "contains") {
        attrs.push(format!("contains = {pattern:?}"));
    }
    if let Some(pattern) = rule_value(rules, "excludes") {
        attrs.push(format!("does_not_contain = {pattern:?}"));
    }

    let (mut min, max) = min_max_rules(rules);
    let equal = rule_value(rules, "len").and_then(parse_usize);
    for rule in rules
        .split(',')
        .map(str::trim)
        .filter(|rule| !rule.is_empty())
    {
        if rule == "required" {
            min.get_or_insert(1usize);
        }
    }

    if let Some(equal) = equal {
        attrs.push(format!("length(equal = {equal})"));
    } else {
        match (min, max) {
            (Some(min), Some(max)) => attrs.push(format!("length(min = {min}, max = {max})")),
            (Some(min), None) => attrs.push(format!("length(min = {min})")),
            (None, Some(max)) => attrs.push(format!("length(max = {max})")),
            (None, None) => {}
        }
    }

    if attrs.is_empty() {
        None
    } else {
        Some(attrs.join(", "))
    }
}

fn number_validation_attr(rules: &str) -> Option<String> {
    let mut min = rule_value(rules, "min")
        .or_else(|| rule_value(rules, "gte"))
        .filter(|value| is_number_literal(value));
    let mut max = rule_value(rules, "max")
        .or_else(|| rule_value(rules, "lte"))
        .filter(|value| is_number_literal(value));
    if min.is_none() && has_rule(rules, "nonnegative") {
        min = Some("0");
    }
    if has_rule(rules, "page") {
        min.get_or_insert("1");
    }
    if has_rule(rules, "limit") {
        min.get_or_insert("1");
        max.get_or_insert("1000");
    }
    let exclusive_min = rule_value(rules, "gt").filter(|value| is_number_literal(value));
    let exclusive_max = rule_value(rules, "lt").filter(|value| is_number_literal(value));

    let mut parts = Vec::new();
    if let Some(min) = min {
        parts.push(format!("min = {min}"));
    }
    if let Some(max) = max {
        parts.push(format!("max = {max}"));
    }
    if let Some(exclusive_min) = exclusive_min {
        parts.push(format!("exclusive_min = {exclusive_min}"));
    }
    if let Some(exclusive_max) = exclusive_max {
        parts.push(format!("exclusive_max = {exclusive_max}"));
    }

    if parts.is_empty() {
        None
    } else {
        Some(format!("range({})", parts.join(", ")))
    }
}

fn collection_validation_attr(rules: &str) -> Option<String> {
    let (mut min, max) = min_max_rules(rules);
    let equal = rule_value(rules, "len").and_then(parse_usize);
    if has_rule(rules, "required") {
        min.get_or_insert(1usize);
    }

    if let Some(equal) = equal {
        Some(format!("length(equal = {equal})"))
    } else {
        match (min, max) {
            (Some(min), Some(max)) => Some(format!("length(min = {min}, max = {max})")),
            (Some(min), None) => Some(format!("length(min = {min})")),
            (None, Some(max)) => Some(format!("length(max = {max})")),
            (None, None) => None,
        }
    }
}

fn min_max_rules(rules: &str) -> (Option<usize>, Option<usize>) {
    let min = rule_value(rules, "min")
        .or_else(|| rule_value(rules, "gte"))
        .or_else(|| rule_value(rules, "min_items"))
        .and_then(parse_usize);
    let max = rule_value(rules, "max")
        .or_else(|| rule_value(rules, "lte"))
        .or_else(|| rule_value(rules, "max_items"))
        .and_then(parse_usize);
    (min, max)
}

fn has_rule(rules: &str, name: &str) -> bool {
    for rule in rules
        .split(',')
        .map(str::trim)
        .filter(|rule| !rule.is_empty())
    {
        if rule == name {
            return true;
        }
    }
    false
}

fn is_optional_rule(rules: &str) -> bool {
    has_rule(rules, "optional") || has_rule(rules, "omitempty")
}

fn rules_before_dive(rules: &str) -> &str {
    rules
        .split_once(",dive")
        .map(|(before, _)| before)
        .unwrap_or(rules)
        .trim()
}

fn rules_after_dive(rules: &str) -> Option<&str> {
    rules
        .split_once("dive,")
        .map(|(_, after)| after.trim())
        .filter(|after| !after.is_empty())
}

fn split_map_dive_rules(rules: &str) -> (Option<String>, Option<String>) {
    let parts = rules
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    if parts.first() != Some(&"keys") {
        return (None, Some(rules.trim().to_string()));
    }

    let Some(end_idx) = parts.iter().position(|part| *part == "endkeys") else {
        return (None, Some(rules.trim().to_string()));
    };
    let key_rules = parts[1..end_idx].join(",");
    let value_rules = parts[end_idx + 1..].join(",");
    (
        (!key_rules.is_empty()).then_some(key_rules),
        (!value_rules.is_empty()).then_some(value_rules),
    )
}

fn map_key_value_types(ty: &str) -> Option<(String, String)> {
    let ty = ty.trim();
    if let Some(rest) = ty.strip_prefix("map[") {
        let (key, value) = rest.split_once(']')?;
        return Some((
            key.trim_start_matches('*').trim().to_string(),
            value.trim_start_matches('*').trim().to_string(),
        ));
    }
    if let Some(inner) = ty
        .strip_prefix("HashMap<")
        .and_then(|raw| raw.strip_suffix('>'))
    {
        let (key, value) = inner.split_once(',')?;
        return Some((
            key.trim_start_matches('*').trim().to_string(),
            value.trim_start_matches('*').trim().to_string(),
        ));
    }
    None
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

fn oneof_values(rules: &str) -> Option<Vec<&str>> {
    let values = rule_value(rules, "oneof")?
        .split_whitespace()
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    if values.is_empty() {
        None
    } else {
        Some(values)
    }
}

fn condition_pairs(value: &str) -> Vec<(&str, &str)> {
    let parts = condition_names(value);
    parts
        .chunks_exact(2)
        .map(|pair| (pair[0], pair[1]))
        .collect::<Vec<_>>()
}

fn condition_names(value: &str) -> Vec<&str> {
    value
        .split_whitespace()
        .filter(|value| !value.is_empty())
        .collect()
}

fn rule_value<'a>(rules: &'a str, name: &str) -> Option<&'a str> {
    for rule in rules
        .split(',')
        .map(str::trim)
        .filter(|rule| !rule.is_empty())
    {
        if let Some((key, value)) = rule.split_once('=') {
            if key.trim() == name {
                return Some(value.trim());
            }
        }
    }
    None
}

fn parse_usize(value: &str) -> Option<usize> {
    value.parse::<usize>().ok()
}

fn is_number_literal(value: &str) -> bool {
    let value = value.trim();
    if value.is_empty() {
        return false;
    }
    value
        .chars()
        .enumerate()
        .all(|(idx, ch)| ch.is_ascii_digit() || ch == '.' || (idx == 0 && ch == '-'))
}

#[cfg(test)]
mod tests {
    use crate::parser::parse_api;

    use super::*;

    #[test]
    fn renders_mixed_request_sources() {
        let spec = parse_api(
            r#"
            service user

            type GetUserReq {
                id u64 `path:"id"`
                q String `query:"q"`
                token String `header:"X-Token"`
                name String `form:"name"`
                password string `query:"password"`
                confirm string `query:"confirm" validate:"eqfield=password"`
                nickname String `query:"nickname" validate:"required,min=2,max=16"`
                age int `query:"age" validate:"min=1,max=120"`
                min_age int `query:"minAge"`
                parent_id int64 `query:"parent_id"`
                region_id uint64 `query:"region_id"`
                lat float `query:"lat"`
                lng double `query:"lng"`
                max_age int `query:"maxAge" validate:"gtefield=min_age"`
                score int `query:"score" validate:"gt=0,lt=100"`
                page uint `query:"page" validate:"gte=1,lte=500"`
                page_number int `query:"pageNumber" validate:"page"`
                page_limit int `query:"pageLimit" validate:"limit"`
                offset int `query:"offset" validate:"nonnegative"`
                email string `query:"email" validate:"required,email"`
                website string `query:"website" validate:"url"`
                code string `query:"code" validate:"len=6"`
                remote_ip string `query:"remoteIp" validate:"ip"`
                client_ip string `query:"clientIp" validate:"ipv4"`
                domain string `query:"domain" validate:"contains=example"`
                slug string `query:"slug" validate:"excludes=admin"`
                status string `query:"status" validate:"oneof=active disabled"`
                role_id int `query:"roleId" validate:"oneof=1 2"`
                reason string `query:"reason" validate:"required_if=status disabled"`
                comment string `query:"comment" validate:"required_unless=status active"`
                backup string `query:"backup" validate:"required_with=account"`
                fallback string `query:"fallback" validate:"required_without=account"`
                tags []string `query:"tags" validate:"min=1,dive,required,min=2,alphanum"`
                scores []int `query:"scores" validate:"len=2,dive,gte=1,lte=99"`
                labels map[string]string `json:"labels" validate:"min=1,dive,keys,min=2,endkeys,required,min=1,alphanum"`
                weights map[string]int `json:"weights" validate:"dive,keys,oneof=gold silver,endkeys,gte=1,lte=10"`
                account string `query:"account" validate:"startswith=user_"`
                resource string `query:"resource" validate:"endswith=_id"`
                alpha_name string `query:"alphaName" validate:"alpha"`
                code_name string `query:"codeName" validate:"alphanum"`
                resource_code string `query:"resourceCode" validate:"code"`
                json_config string `query:"jsonConfig" validate:"json"`
                codes []string `query:"codes" validate:"min_items=1,max_items=3,dive,code"`
                trace string `query:"trace" validate:"ascii"`
                amount string `query:"amount" validate:"numeric"`
                lower_code string `query:"lowerCode" validate:"lowercase"`
                upper_code string `query:"upperCode" validate:"uppercase"`
                note String `query:"note" validate:"optional"`
            }

            type UserResp {
                id u64
            }

            get /users/:id (GetUserReq) returns (UserResp)
            "#,
        )
        .expect("valid api");

        let handlers = render_handlers(&spec);
        assert!(handlers.contains("GetUserReqPath"));
        assert!(handlers.contains("GetUserReqQuery"));
        assert!(handlers.contains("GetUserReqForm"));
        assert!(handlers.contains("HeaderMap"));
        assert!(handlers.contains("#[serde(default)]\n    q: String"));
        assert!(!handlers.contains("#[serde(default)]\n    id: u64"));
        assert!(handlers.contains("header_value::<String>(&headers, \"X-Token\")?"));
        assert!(handlers.contains("#[validate(length(min = 2, max = 16))]"));
        assert!(handlers.contains("#[validate(range(min = 1, max = 120))]"));
        assert!(handlers.contains("#[validate(range(exclusive_min = 0, exclusive_max = 100))]"));
        assert!(handlers.contains("#[validate(range(min = 1, max = 500))]"));
        assert!(handlers.contains("#[validate(range(min = 1))]"));
        assert!(handlers.contains("#[validate(range(min = 1, max = 1000))]"));
        assert!(handlers.contains("#[validate(range(min = 0))]"));
        assert!(handlers.contains("#[validate(email, length(min = 1))]"));
        assert!(handlers.contains("#[validate(url)]"));
        assert!(handlers.contains("#[validate(length(equal = 6))]"));
        assert!(handlers.contains("#[validate(ip)]"));
        assert!(handlers.contains("#[validate(ip(v4 = true))]"));
        assert!(handlers.contains("#[validate(contains = \"example\")]"));
        assert!(handlers.contains("#[validate(does_not_contain = \"admin\")]"));
        assert!(handlers.contains("if !(req.confirm == req.password)"));
        assert!(handlers.contains("if !(req.max_age >= req.min_age)"));
        assert!(handlers.contains("parent_id: i64"));
        assert!(handlers.contains("region_id: u64"));
        assert!(handlers.contains("lat: f32"));
        assert!(handlers.contains("lng: f64"));
        assert!(!handlers.contains("parent_id: int64"));
        assert!(!handlers.contains("region_id: uint64"));
        assert!(!handlers.contains("lat: float"));
        assert!(!handlers.contains("lng: double"));
        assert!(handlers.contains("if ![\"active\", \"disabled\"].contains(&value.as_str())"));
        assert!(handlers.contains("if ![\"1\", \"2\"].contains(&value.as_str())"));
        assert!(handlers
            .contains("if (req.status.to_string() == \"disabled\") && req.reason.is_empty()"));
        assert!(handlers
            .contains("if !(req.status.to_string() == \"active\") && req.comment.is_empty()"));
        assert!(
            handlers.contains("if (!req.account.to_string().is_empty()) && req.backup.is_empty()")
        );
        assert!(
            handlers.contains("if (req.account.to_string().is_empty()) && req.fallback.is_empty()")
        );
        assert!(handlers.contains("tags: Vec<String>"));
        assert!(handlers.contains("#[validate(length(min = 1))]"));
        assert!(handlers.contains("for item in &req.tags"));
        assert!(handlers.contains("if item.chars().count() < 2"));
        assert!(handlers.contains("if !item.chars().all(|ch| ch.is_alphanumeric())"));
        assert!(handlers.contains("scores: Vec<i64>"));
        assert!(handlers.contains("#[validate(length(equal = 2))]"));
        assert!(handlers.contains("for item in &req.scores"));
        assert!(handlers.contains("if item < &1"));
        assert!(handlers.contains("if item > &99"));
        assert!(handlers.contains("labels: std::collections::HashMap<String, String>"));
        assert!(handlers.contains("for (key, value) in &req.labels"));
        assert!(handlers.contains("if key.chars().count() < 2"));
        assert!(handlers.contains("if value.chars().count() < 1"));
        assert!(handlers.contains("if !value.chars().all(|ch| ch.is_alphanumeric())"));
        assert!(handlers.contains("weights: std::collections::HashMap<String, i64>"));
        assert!(handlers.contains("for (key, value) in &req.weights"));
        assert!(handlers.contains("if ![\"gold\", \"silver\"].contains(&value.as_str())"));
        assert!(handlers.contains("if value < &1"));
        assert!(handlers.contains("if value > &10"));
        assert!(handlers.contains("if !req.account.starts_with(\"user_\")"));
        assert!(handlers.contains("if !req.resource.ends_with(\"_id\")"));
        assert!(handlers.contains("if !req.alpha_name.chars().all(|ch| ch.is_alphabetic())"));
        assert!(handlers.contains("if !req.code_name.chars().all(|ch| ch.is_alphanumeric())"));
        assert!(handlers.contains("if req.resource_code.is_empty()"));
        assert!(handlers
            .contains("serde_json::from_str::<serde_json::Value>(&req.json_config).is_err()"));
        assert!(handlers.contains("#[validate(length(min = 1, max = 3))]"));
        assert!(handlers.contains("for item in &req.codes"));
        assert!(handlers.contains("if !req.trace.is_ascii()"));
        assert!(handlers.contains("if req.amount.parse::<f64>().is_err()"));
        assert!(handlers.contains("if req.lower_code.chars().any(|ch| ch.is_uppercase())"));
        assert!(handlers.contains("if req.upper_code.chars().any(|ch| ch.is_lowercase())"));
        assert!(!handlers.contains("note:\n    #[validate"));
    }

    #[test]
    fn renders_route_without_request() {
        let spec = parse_api(
            r#"
            service health-api {
                @handler health
                get /health returns (HealthResp)
                @handler ping
                get /ping
                @handler pingHead
                head /ping-head ()
                @handler logout
                post /logout (LogoutReq)
            }

            type LogoutReq {
                token string `json:"token"`
            }

            type HealthResp {
                ok bool `json:"ok"`
            }
            "#,
        )
        .expect("valid api");

        let handlers = render_handlers(&spec);
        assert!(handlers.contains(".route(\"/health\", get(health))"));
        assert!(handlers.contains(".route(\"/ping\", get(ping))"));
        assert!(handlers.contains(".route(\"/ping-head\", head(ping_head))"));
        assert!(handlers.contains(".route(\"/logout\", post(logout))"));
        assert!(handlers.contains("let req = EmptyReq {};"));
        assert!(handlers.contains("Result<ApiResponse<EmptyResp>, RozeError>"));
        assert!(handlers.contains("roze_middleware::apply_fallback("));
        assert!(handlers.contains("ctx.config.name.as_str(),\n                err,"));
        assert!(handlers
            .contains("roze_middleware::route_fallback(Some(&ctx.config.governance), \"health\")"));

        let logic = render_logic(&spec);
        assert!(logic.contains("Ok(EmptyResp::default())"));

        let openapi = render_openapi(&spec);
        assert!(openapi.contains("builder.add_operation(\"/health\", HttpMethod::Get"));
        assert!(openapi.contains("builder.add_operation(\"/ping-head\", HttpMethod::Head"));
        assert!(openapi.contains(".response(\"200\", \"OK\", \"EmptyResp\")"));
        assert!(!openapi.contains(".request_body(\"EmptyReq\")"));
    }

    #[test]
    fn rest_main_mounts_stable_application_middleware_hook() {
        let spec = parse_api(
            r#"
            service health-api {
                get /health returns (HealthResp)
            }

            type HealthResp {
                ok bool
            }
            "#,
        )
        .expect("valid api");

        let main = render_rest_main(&spec);
        assert!(main.contains("let app = route::router(ctx);"));
        assert!(main.contains("middleware::app::apply(app)"));
        assert!(main.contains("apply_common_with_config(app, middleware_config)"));
        assert!(main.contains("\"REST middleware plan resolved\""));
        assert!(main.contains("\"REST router constructed\""));

        let middleware_mod = render_middleware_mod(&spec);
        assert!(middleware_mod.contains("pub mod app;"));
        assert!(render_application_middleware().contains("pub fn apply(router: Router) -> Router"));
    }

    #[test]
    fn rest_generation_enforces_declared_permissions_and_exposes_auth_helpers() {
        let spec = parse_api(
            r#"
            service user-api {
                @permission users:read, users:list
                get /users (ListUsersReq) returns (ListUsersResp)
            }

            type ListUsersReq {
            }
            type ListUsersResp {
            }
            "#,
        )
        .expect("valid api");

        let handlers = render_handlers(&spec);
        assert!(handlers.contains(
            "roze_middleware::enforce_permissions(&request_ctx, &[\"users:read\", \"users:list\"])"
        ));
        assert!(handlers.contains("let request_ctx = match authorize(&headers, &ctx)"));

        let openapi = render_openapi(&spec);
        assert!(openapi.contains(".extension(\"x-roze-permissions\", serde_json::json!([\"users:read\", \"users:list\"]))"));

        let logic_mod = render_logic_mod(&spec);
        assert!(logic_mod.contains("pub fn current_user_id"));
        assert!(logic_mod.contains("pub fn current_permissions"));
    }

    #[test]
    fn rest_generation_wires_idempotency_middleware() {
        let spec = parse_api(
            r#"
            service order-api {
                @middleware idempotency
                post /orders (CreateOrderReq) returns (CreateOrderResp)
            }

            type CreateOrderReq {
                sku string `json:"sku"`
            }
            type CreateOrderResp {
                id string `json:"id"`
            }
            "#,
        )
        .expect("valid api");

        let handlers = render_handlers(&spec);
        assert!(handlers.contains("headers: HeaderMap"));
        assert!(handlers.contains("IDEMPOTENCY_MISSING_KEY"));
        assert!(handlers.contains("idempotency_fingerprint(&req)"));
        assert!(handlers.contains("begin_idempotency(ctx.idempotency.as_ref()"));
        assert!(handlers.contains("roze_middleware::IdempotencyDecision::Replay(value)"));
        assert!(handlers.contains("roze_middleware::IdempotencyDecision::Conflict"));
        assert!(handlers.contains("IDEMPOTENCY_IN_FLIGHT"));
        assert!(handlers.contains("IDEMPOTENCY_KEY_REUSED"));
        assert!(handlers.contains("complete_idempotency(ctx.idempotency.as_ref()"));
        assert!(handlers.contains("fail_idempotency(ctx.idempotency.as_ref()"));
    }

    #[test]
    fn rest_generated_report_export_and_chart_query_interfaces() {
        let spec = parse_api(
            r#"
            @server (
                prefix: /api/v1
            )

            service analytics-api {
                @handler listUsers
                get /users (ListUsersReq) returns (ListUsersResp)
            }

            type (
                ListUsersReq {
                    keyword string `query:"keyword"`
                }
                ListUsersResp {
                    total u64
                }
            )
            "#,
        )
        .expect("valid api");

        let routes = render_route_mod(&spec);
        assert!(routes.contains(".route(\"/api/v1/reports/export\", get(report_export))"));
        assert!(routes.contains(".route(\"/api/v1/charts/query\", get(chart_query))"));
        assert!(routes.contains("struct ReportExportQuery"));
        assert!(routes.contains("struct ChartQueryResponse"));
        assert!(routes.contains("download_url: None"));
        assert!(routes.contains("series: Vec::new()"));

        let openapi = render_openapi(&spec);
        assert!(openapi
            .contains("builder.add_operation(\"/api/v1/reports/export\", HttpMethod::Get, op);"));
        assert!(openapi
            .contains("builder.add_operation(\"/api/v1/charts/query\", HttpMethod::Get, op);"));
        assert!(openapi.contains("builder.component_schema(\"ReportExportResponse\""));
        assert!(openapi.contains("builder.component_schema(\"ChartQueryResponse\""));
        assert!(openapi.contains(
            ".parameter(\"format\", roze_openapi::ParameterLocation::Query, \"String\", false)"
        ));
        assert!(openapi.contains(
            ".parameter(\"interval\", roze_openapi::ParameterLocation::Query, \"String\", false)"
        ));
    }

    #[test]
    fn rest_main_uses_service_group_lifecycle() {
        let spec = parse_api(
            r#"
            service user-api {
                get /users/:id (GetUserReq) returns (UserResp)
            }

            type GetUserReq {
                id u64 `path:"id"`
            }

            type UserResp {
                id u64
            }
            "#,
        )
        .expect("valid api");

        let rendered = render_rest_main(&spec);
        assert!(rendered.contains("use roze_service::ServiceGroup;"));
        assert!(rendered.contains("let health = ctx.health.clone();"));
        assert!(rendered.contains("RestService::new("));
        assert!(rendered.contains("health.mark_draining();"));
        assert!(rendered.contains("\"service configuration loaded\""));
        assert!(rendered.contains("\"service context initialized\""));
        assert!(rendered.contains("\"service starting\""));
        assert!(rendered.contains("\"service stopped\""));
        assert!(rendered.contains("\"service failed\""));
        assert!(rendered.contains("let result = group.start().await;"));
        assert!(rendered.contains("result?;"));
    }

    #[test]
    fn openapi_empty_schema_properties_are_not_mutable() {
        let spec = parse_api(
            r#"
            service upload-api {
                @handler uploadToken
                post /upload/token (UploadTokenReq) returns (UploadTokenResp)
            }

            type UploadTokenReq {
            }

            type UploadTokenResp {
                token string `json:"token"`
            }
            "#,
        )
        .expect("valid api");

        let openapi = render_openapi(&spec);
        assert!(openapi.contains("let properties = BTreeMap::new();"));
        assert!(!openapi.contains("let mut properties = BTreeMap::new();\n        builder = builder.component_schema(\"UploadTokenReq\""));
        assert!(openapi.contains(
            "let mut properties = BTreeMap::new();\n        properties.insert(\"token\""
        ));
    }

    #[test]
    fn renders_route_scoped_server_blocks() {
        let spec = parse_api(
            r#"
            @server (
                prefix: /api
                middleware: trace
            )

            service user-api {
                @server (
                    prefix: /api/v1
                    middleware: auth
                    jwt: Auth
                )
                @handler getUser
                get /users/:id (GetUserReq) returns (UserResp)

                @server (
                    prefix: /internal
                    middleware: audit
                )
                @handler getStats
                get /stats (StatsReq) returns (StatsResp)

                @handler updateUser
                patch /users/:id (UpdateUserReq) returns (UserResp)
            }

            type (
                GetUserReq {
                    id u64 `path:"id"`
                }
                UserResp {
                    id u64
                }
                StatsReq {
                    q string `query:"q"`
                }
                StatsResp {
                    ok bool
                }
                UpdateUserReq {
                    id u64 `path:"id"`
                    name string `json:"name"`
                }
            )
            "#,
        )
        .expect("valid api");

        let handlers = render_handlers(&spec);
        assert!(handlers.contains(".route(\"/api/v1/users/{id}\", get(get_user))"));
        assert!(handlers.contains(".route(\"/internal/stats\", get(get_stats))"));
        assert!(handlers.contains(".route(\"/internal/users/{id}\", patch(update_user))"));
        assert!(handlers.contains("authorize(&headers, &ctx)"));
        assert!(handlers
            .contains("ctx.config.rest.as_ref().is_none_or(|rest| rest.middlewares.timeout)"));
        assert!(handlers.contains("crate::middleware::audit(&ctx, &request_ctx).await"));

        let routes = render_route_mod(&spec);
        assert!(routes.contains("roze_middleware::apply_timeout(router, timeout_ms)"));
        assert!(!routes.contains("Duration::from_millis"));
        assert!(!routes.contains("Router<ServiceContext>"));
        assert!(routes.contains(".and(ctx.config.governance.timeout_ms)"));
        assert!(routes.contains("router.with_state(ctx)"));

        let openapi = render_openapi(&spec);
        assert!(openapi.contains("builder.security_scheme(\"bearerAuth\""));
        assert!(openapi.contains("builder.add_operation(\"/api/v1/users/:id\""));
        assert!(openapi.contains("builder.add_operation(\"/internal/stats\""));
        assert!(
            openapi.contains("builder.add_operation(\"/internal/users/:id\", HttpMethod::Patch")
        );
        assert!(openapi.contains(".require_security(\"bearerAuth\")"));
    }

    #[test]
    fn route_group_uses_multilevel_server_group_before_path_fallback() {
        let spec = parse_api(
            r#"
            service user-api {
                @server (
                    group: admin/user
                )
                @handler getProfile
                get /profiles/:id (GetProfileReq) returns (UserResp)
            }

            type (
                GetProfileReq {
                    id u64 `path:"id"`
                }
                UserResp {
                    id u64 `json:"id"`
                }
            )
            "#,
        )
        .expect("valid api");

        assert_eq!(route_group_name(&spec.rest_routes[0]), "admin_user");
        assert_eq!(
            route_group_segments(&spec.rest_routes[0]),
            vec!["admin".to_string(), "user".to_string()]
        );

        let routes = render_route_mod(&spec);
        assert!(routes.contains("mod admin_user;"));
        assert!(routes.contains(".merge(admin_user::routes())"));

        let route_groups = render_route_group_mods(&spec);
        assert_eq!(route_groups[0].0, "admin_user");
        assert!(route_groups[0]
            .1
            .contains(".route(\"/profiles/{id}\", get(handler::admin_user::get_profile))"));
    }

    #[test]
    fn reused_request_types_get_route_scoped_extractor_names() {
        let spec = crate::parser::parse_api(
            r#"
            service user-api {
                @handler getUserByIds
                post /users/batch (UserBatchReq) returns (UserBatchResp)

                @handler getAllUserState
                post /users/state (UserBatchReq) returns (UserBatchResp)
            }

            type UserBatchReq {
                ids []u64 `json:"ids"`
            }

            type UserBatchResp {
                ids []u64
            }
            "#,
        )
        .expect("valid api");

        let handlers = render_handlers(&spec);

        assert!(handlers.contains("struct GetUserByIdsUserBatchReqJson"));
        assert!(handlers.contains("Json(body): Json<GetUserByIdsUserBatchReqJson>"));
        assert!(handlers.contains("struct GetAllUserStateUserBatchReqJson"));
        assert!(handlers.contains("Json(body): Json<GetAllUserStateUserBatchReqJson>"));
        assert!(!handlers.contains("struct UserBatchReqJson"));
    }
}
