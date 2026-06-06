use std::collections::{HashMap, HashSet};

use crate::{
    generator::to_snake_case,
    parser::{ApiSpec, Field, FieldSource, HttpMethod},
};

pub fn render_main(_spec: &ApiSpec) -> String {
    r#"mod config;
mod handler;
mod logic;
mod client;
mod pb;
mod rpc;
mod svc;
mod types;

use roze_core::rest::RestServer;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    roze_core::init_tracing();

    let config = config::load("config.yaml")?;
    let rest = config
        .rest
        .clone()
        .ok_or_else(|| anyhow::anyhow!("missing rest config"))?;
    let ctx = svc::ServiceContext::new(config).await?;
    let app = roze_core::middleware::apply_common(handler::router(ctx));
    RestServer::new(rest.addr, app).serve().await?;

    Ok(())
}
"#
    .to_string()
}

pub fn render_handlers(spec: &ApiSpec) -> String {
    let mut out = String::from(
        "use poem::{handler, http::HeaderMap, web::{Data, Form, Json, Path, Query}, Endpoint, EndpointExt, Route};\nuse serde::Deserialize;\nuse roze_core::rest::{ApiResponse, AppError};\n\nuse crate::svc::ServiceContext;\nuse crate::types::*;\n\n",
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

    for route in &spec.rest_routes {
        let handler = route
            .handler
            .clone()
            .unwrap_or_else(|| handler_name(&route.method, &route.path));
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
        "#[handler]\nasync fn health() -> Result<Json<ApiResponse<&'static str>>, AppError> {\n    Ok(Json(ApiResponse::ok(\"ok\")))\n}\n\n",
    );

    out.push_str(
        "#[handler]\nasync fn metrics() -> String {\n    roze_core::middleware::metrics()\n}\n\n",
    );

    if spec
        .rest_routes
        .iter()
        .any(|route| route_request_spec(spec, route).map_or(false, |spec| spec.has_header))
    {
        out.push_str(
            "fn header_value<T>(headers: &HeaderMap, name: &str) -> Result<T, AppError>\nwhere\n    T: std::str::FromStr,\n    T::Err: std::fmt::Display,\n{\n    let raw = headers\n        .get(name)\n        .ok_or_else(|| AppError::BadRequest(format!(\"missing header `{name}`\")))?;\n    let raw = raw\n        .to_str()\n        .map_err(|err| AppError::BadRequest(format!(\"invalid header `{name}`: {err}\")))?;\n    raw.parse::<T>()\n        .map_err(|err| AppError::BadRequest(format!(\"invalid header `{name}`: {err}\")))\n}\n\n",
        );
    }

    for route in &spec.rest_routes {
        out.push_str(&render_route_handler(spec, route));
    }

    out
}

pub fn render_logic(spec: &ApiSpec) -> String {
    let mut out = String::from("use roze_core::rest::AppError;\n\n");
    out.push_str("use crate::svc::ServiceContext;\n");
    out.push_str("use crate::types::*;\n\n");

    for route in &spec.rest_routes {
        let handler = route
            .handler
            .clone()
            .unwrap_or_else(|| handler_name(&route.method, &route.path));
        match route.method {
            HttpMethod::Get | HttpMethod::Delete => {
                out.push_str(&format!(
                    "pub async fn {handler}(ctx: ServiceContext, req: {request}) -> Result<{response}, AppError> {{\n",
                    handler = handler,
                    request = route.request,
                    response = route.response
                ));
                out.push_str("    let _ = ctx;\n");
                out.push_str("    let _ = req;\n");
            }
            HttpMethod::Post | HttpMethod::Put => {
                out.push_str(&format!(
                    "pub async fn {handler}(ctx: ServiceContext, req: {request}) -> Result<{response}, AppError> {{\n",
                    handler = handler,
                    request = route.request,
                    response = route.response
                ));
                out.push_str("    let _ = ctx;\n");
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

fn render_route_handler(spec: &ApiSpec, route: &crate::parser::RestRoute) -> String {
    let request_ty = spec
        .types
        .iter()
        .find(|ty| ty.name == route.request)
        .unwrap_or_else(|| panic!("missing request type `{}`", route.request));
    let route_spec = route_request_spec(spec, route).expect("request spec");
    let handler = route
        .handler
        .clone()
        .unwrap_or_else(|| handler_name(&route.method, &route.path));

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

    let mut params = vec!["Data(ctx): Data<&ServiceContext>".to_string()];
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
    if route_spec.has_header {
        params.push("headers: &HeaderMap".to_string());
    }

    out.push_str("#[handler]\n");
    out.push_str(&format!(
        "async fn {handler}({params}) -> Result<Json<ApiResponse<{response}>>, AppError> {{\n",
        handler = handler,
        params = params.join(", "),
        response = route.response
    ));
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
        "    let resp = crate::logic::{handler}((*ctx).clone(), req).await?;\n",
        handler = handler
    ));
    out.push_str("    Ok(Json(ApiResponse::ok(resp)))\n");
    out.push_str("}\n\n");

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
    out.push_str("#[derive(Debug, Clone, Deserialize)]\n");
    out.push_str(&format!("struct {} {{\n", name));
    for field in fields {
        if let Some(rename) = serde_rename(field) {
            out.push_str(&format!("    #[serde(rename = \"{}\")]\n", rename));
        }
        out.push_str(&format!("    {}: {},\n", rust_field_name(field), field.ty));
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
            field.ty,
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
                while let Some(next) = chars.next() {
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
        .replace('/', "_")
        .replace('-', "_");

    format!("{}_{}", method, path_name)
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
