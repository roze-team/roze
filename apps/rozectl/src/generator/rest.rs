use std::collections::{HashMap, HashSet};

use crate::{
    generator::to_snake_case,
    parser::{ApiSpec, Field, FieldSource, HttpMethod},
};

pub fn render_rest_main(_spec: &ApiSpec) -> String {
    r#"mod config;
mod handler;
mod logic;
mod middleware;
mod openapi;
mod svc;
mod types;

use roze_http::rest::RestServer;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    roze_log::init_tracing();

    let config = config::load(config_path())?;
    let rest = config
        .rest
        .clone()
        .ok_or_else(|| anyhow::anyhow!("missing rest config"))?;
    let mut registration = if rest.register {
        let registry = roze_rpc::registry::build_service_registry(&config)?
            .ok_or_else(|| anyhow::anyhow!("missing registry config"))?;
        Some(roze_rpc::rpc::ServiceRegistrationGuard::start(
            registry,
            config.name.clone(),
            rest.addr,
        )
        .await?)
    } else {
        None
    };
    let ctx = svc::ServiceContext::new(config).await?;
    let app = roze_middleware::apply_common(handler::router(ctx));
    RestServer::new(rest.addr, app).serve().await?;
    if let Some(registration) = registration.as_mut() {
        registration.shutdown().await?;
    }

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

pub fn render_handlers(spec: &ApiSpec) -> String {
    let mut out = String::from("#![allow(unused_imports)]\n\n");
    out.push_str(
        "use poem::{handler, http::HeaderMap, web::{Data, Form, Json, Path, Query}, Endpoint, EndpointExt, Route};\nuse serde::Deserialize;\nuse roze_validation::Validate;\nuse roze_context::Context;\nuse roze_error::RozeError;\nuse roze_result::ApiResponse;\n\nuse crate::openapi;\nuse crate::svc::ServiceContext;\nuse crate::types::*;\n\n",
    );
    out.push_str("pub fn router(ctx: ServiceContext) -> impl Endpoint {\n");
    out.push_str("    Route::new()\n");
    out.push_str(&format!(
        "        .at(\"{}\", poem::get(health))\n",
        full_route_path(spec, "/healthz")
    ));
    out.push_str(&format!(
        "        .at(\"{}\", poem::get(metrics))\n",
        full_route_path(spec, "/metrics")
    ));
    out.push_str(&format!(
        "        .at(\"{}\", poem::get(openapi_doc))\n",
        full_route_path(spec, "/openapi.json")
    ));

    for route in &spec.rest_routes {
        let handler = resolved_handler_name(route);
        let routing_fn = match route.method {
            HttpMethod::Get => "poem::get",
            HttpMethod::Post => "poem::post",
            HttpMethod::Put => "poem::put",
            HttpMethod::Delete => "poem::delete",
        };
        out.push_str(&format!(
            "        .at(\"{}\", {}({}))\n",
            full_route_path(spec, &route.path),
            routing_fn,
            handler
        ));
    }

    out.push_str("        .data(ctx)\n");
    out.push_str("}\n\n");

    out.push_str(
        "#[handler]\nasync fn health() -> Result<Json<ApiResponse<&'static str>>, RozeError> {\n    Ok(Json(ApiResponse::ok(\"ok\")))\n}\n\n",
    );

    out.push_str(
        "#[handler]\nasync fn metrics() -> String {\n    roze_metrics::http_metrics()\n}\n\n",
    );

    out.push_str(
        "#[handler]\nasync fn openapi_doc() -> Json<serde_json::Value> {\n    Json(openapi::document())\n}\n\n",
    );

    if spec.rest_routes.iter().any(|route| {
        route_request_spec(spec, route).is_some_and(|spec| spec.has_header)
            || route_uses_auth(spec, route)
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
            "fn authorize(headers: &HeaderMap, ctx: &ServiceContext) -> Result<(), RozeError> {\n    let jwt = ctx.jwt_config().ok_or(RozeError::Unauthorized)?;\n    let header_value = headers\n        .get(\"authorization\")\n        .and_then(|value| value.to_str().ok())\n        .ok_or(RozeError::Unauthorized)?;\n    let token = roze_jwt::extract_bearer_token(header_value).ok_or(RozeError::Unauthorized)?;\n    roze_jwt::verify_token(token, &jwt).map_err(|_| RozeError::Unauthorized)?;\n    Ok(())\n}\n\n",
        );
    }

    for route in &spec.rest_routes {
        out.push_str(&render_route_handler(spec, route));
    }

    out
}

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
            HttpMethod::Get | HttpMethod::Delete => {
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
            HttpMethod::Post | HttpMethod::Put => {
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
            "    Ok({response}::default_response())\n",
            response = route.response
        ));
        out.push_str("}\n\n");
    }

    out.push_str(&render_default_impls(spec));
    out
}

pub fn render_openapi(spec: &ApiSpec) -> String {
    let needs_jwt = spec
        .server
        .as_ref()
        .and_then(|server| server.jwt.as_ref())
        .is_some();
    let mut out = String::from(
        "use std::collections::BTreeMap;\n\nuse roze_openapi::{OpenApiBuilder, Schema",
    );
    if !spec.rest_routes.is_empty() {
        out.push_str(", HttpMethod, Operation");
    }
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

    if spec
        .server
        .as_ref()
        .and_then(|server| server.jwt.as_ref())
        .is_some()
    {
        out.push_str(
            "    builder = builder.security_scheme(\"bearerAuth\", SecurityScheme::Http { scheme: \"bearer\".to_string(), bearer_format: Some(\"JWT\".to_string()) });\n",
        );
    }

    for ty in &spec.types {
        out.push_str("    {\n");
        out.push_str("        let mut properties = BTreeMap::new();\n");
        let mut required = Vec::new();
        for field in &ty.fields {
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
        if spec
            .server
            .as_ref()
            .and_then(|server| server.jwt.as_ref())
            .is_some()
        {
            out.push_str(".require_security(\"bearerAuth\")");
        }

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
            || matches!(route.method, HttpMethod::Post | HttpMethod::Put)
        {
            out.push_str(&format!(".request_body({:?})", route.request));
        }

        out.push_str(&format!(
            ".response(\"200\", \"OK\", {:?});\n",
            route.response
        ));
        out.push_str(&format!(
            "    builder.add_operation({:?}, HttpMethod::{}, op);\n",
            full_route_path(spec, &route.path),
            match route.method {
                HttpMethod::Get => "Get",
                HttpMethod::Post => "Post",
                HttpMethod::Put => "Put",
                HttpMethod::Delete => "Delete",
            }
        ));
    }

    out.push_str("    roze_openapi::to_json_value(&builder.finish())\n}\n");
    out
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
    let uses_auth = plan
        .builtins
        .contains(&roze_middleware::BuiltInMiddleware::Auth);
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
            let struct_name = partial_struct_name(&request_ty.name, source);
            out.push_str(&render_partial_struct(&struct_name, fields));
        }
    }

    let mut params = vec![
        "Data(ctx): Data<&ServiceContext>".to_string(),
        "Data(request_ctx): Data<&Context>".to_string(),
    ];
    if route_spec.groups.contains_key(&FieldSource::Path) {
        params.push(format!(
            "Path(path): Path<{}>",
            partial_struct_name(&request_ty.name, FieldSource::Path)
        ));
    }
    if route_spec.groups.contains_key(&FieldSource::Query) {
        params.push(format!(
            "Query(query): Query<{}>",
            partial_struct_name(&request_ty.name, FieldSource::Query)
        ));
    }
    if route_spec.groups.contains_key(&FieldSource::Form) {
        params.push(format!(
            "Form(form): Form<{}>",
            partial_struct_name(&request_ty.name, FieldSource::Form)
        ));
    }
    if route_spec.groups.contains_key(&FieldSource::Json) {
        params.push(format!(
            "Json(body): Json<{}>",
            partial_struct_name(&request_ty.name, FieldSource::Json)
        ));
    }
    if route_spec.has_header || uses_auth {
        params.push("headers: &HeaderMap".to_string());
    }

    out.push_str("#[handler]\n");
    out.push_str(&format!(
        "async fn {handler}({params}) -> Result<Json<ApiResponse<{response}>>, RozeError> {{\n",
        handler = handler,
        params = params.join(", "),
        response = route.response
    ));
    out.push_str(&format!(
        "    let (request_ctx, route_guard) = roze_middleware::begin_route(ctx.config.name.clone(), {:?}, {:?}, (*request_ctx).clone(), Some(&ctx.config.governance))?;\n",
        handler,
        http_method_name(&route.method)
    ));
    if uses_auth {
        out.push_str("    if let Err(err) = authorize(headers, ctx) {\n        roze_middleware::finish_route(route_guard, false, err.code().to_string());\n        return Err(err);\n    }\n");
    }
    for name in custom {
        out.push_str(&format!(
            "    if let Err(err) = crate::middleware::{name}(ctx, &request_ctx).await {{\n        roze_middleware::finish_route(route_guard, false, err.code().to_string());\n        return Err(err);\n    }}\n"
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
                    "    if let Err(message) = roze_validation::validate_or_message(&{var}) {{\n        let err = RozeError::BadRequest(message);\n        roze_middleware::finish_route(route_guard, false, err.code().to_string());\n        return Err(err);\n    }}\n",
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
            let value = field_value_expr(field, route, &route_spec);
            out.push_str(&format!("        {}: {},\n", rust_field_name(field), value));
        }
        out.push_str("    };\n");
    }
    out.push_str(&format!(
        "    let result = crate::logic::{handler}((*ctx).clone(), request_ctx, req).await;\n",
        handler = handler
    ));
    out.push_str("    match result {\n        Ok(resp) => {\n            roze_middleware::finish_route(route_guard, true, \"200\");\n            Ok(Json(ApiResponse::ok(resp)))\n        }\n        Err(err) => {\n            roze_middleware::finish_route(route_guard, false, err.code().to_string());\n            Err(err)\n        }\n    }\n");
    out.push_str("}\n\n");

    out
}

fn route_middlewares(spec: &ApiSpec, route: &crate::parser::RestRoute) -> Vec<String> {
    let mut names = spec
        .server
        .as_ref()
        .map(|server| server.middlewares.clone())
        .unwrap_or_default();
    names.extend(route.middlewares.clone());
    names
}

fn route_uses_auth(spec: &ApiSpec, route: &crate::parser::RestRoute) -> bool {
    route_middlewares(spec, route).iter().any(|name| {
        roze_middleware::BuiltInMiddleware::parse(name)
            == Some(roze_middleware::BuiltInMiddleware::Auth)
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

fn render_default_impls(spec: &ApiSpec) -> String {
    let mut seen = HashSet::new();
    let mut out = String::new();
    for response in spec.rest_routes.iter().map(|route| &route.response) {
        if !seen.insert(response) {
            continue;
        }
        if let Some(ty) = spec.types.iter().find(|ty| &ty.name == response) {
            out.push_str(&format!("impl {} {{\n", ty.name));
            out.push_str("    fn default_response() -> Self {\n");
            out.push_str("        Self {\n");
            for field in &ty.fields {
                out.push_str(&format!(
                    "            {}: {},\n",
                    field
                        .json_name
                        .clone()
                        .or_else(|| field.wire_name.clone())
                        .unwrap_or_else(|| to_snake_case(&field.name)),
                    default_value(&field.ty)
                ));
            }
            out.push_str("        }\n");
            out.push_str("    }\n");
            out.push_str("}\n\n");
        }
    }
    out
}

fn render_partial_struct(name: &str, fields: &[&Field]) -> String {
    let mut out = String::new();
    out.push_str("#[derive(Debug, Clone, Deserialize, Validate)]\n");
    out.push_str(&format!("struct {} {{\n", name));
    for field in fields {
        if let Some(rename) = serde_rename(field) {
            out.push_str(&format!("    #[serde(rename = \"{}\")]\n", rename));
        }
        if should_validate(field) {
            out.push_str("    #[validate(length(min = 1))]\n");
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

fn field_value_expr(
    field: &Field,
    route: &crate::parser::RestRoute,
    spec: &RouteRequestSpec<'_>,
) -> String {
    match resolve_field_source(field, route, spec) {
        FieldSource::Path => format!("path.{}", rust_field_name(field)),
        FieldSource::Query => format!("query.{}", rust_field_name(field)),
        FieldSource::Form => format!("form.{}", rust_field_name(field)),
        FieldSource::Json => format!("body.{}", rust_field_name(field)),
        FieldSource::Header => format!(
            "header_value::<{}>(headers, \"{}\")?",
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

    for field in &request_ty.fields {
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
            } else if matches!(route.method, HttpMethod::Get | HttpMethod::Delete) {
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
    to_snake_case(&field.name)
}

fn partial_struct_name(name: &str, source: FieldSource) -> String {
    let suffix = match source {
        FieldSource::Path => "Path",
        FieldSource::Query => "Query",
        FieldSource::Form => "Form",
        FieldSource::Json => "Json",
        FieldSource::Header => "Header",
        FieldSource::Auto => "Auto",
    };
    format!("{}{}", name, suffix)
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
        HttpMethod::Post => "post",
        HttpMethod::Put => "put",
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
        HttpMethod::Post => "POST",
        HttpMethod::Put => "PUT",
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

fn full_route_path(spec: &ApiSpec, path: &str) -> String {
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

pub fn full_route_path_for_openapi(spec: &ApiSpec, path: &str) -> String {
    full_route_path(spec, path)
}

fn escape_doc(doc: &str) -> String {
    doc.replace('\\', "\\\\").replace('"', "\\\"")
}

fn default_value(ty: &str) -> &'static str {
    match ty {
        "String" | "string" => "String::new()",
        "bool" => "false",
        _ => "Default::default()",
    }
}

fn map_type(ty: &str) -> &str {
    match ty {
        "string" => "String",
        "int" => "i64",
        "uint" => "u64",
        "bool" => "bool",
        other => other,
    }
}

fn openapi_schema_expr(ty: &str) -> String {
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

fn should_validate(field: &Field) -> bool {
    matches!(map_type(&field.ty), "String")
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
        assert!(handlers.contains("header_value::<String>(headers, \"X-Token\")?"));
    }
}
