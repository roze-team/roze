use crate::{
    generator::{to_pascal_case, to_snake_case},
    parser::{ApiSpec, Field, HttpMethod, RestRoute, RpcMethod, TypeDef},
};

pub fn render_main(spec: &ApiSpec) -> String {
    let package = spec.service.replace('-', "_");
    let service = to_pascal_case(&spec.service);
    let server_mod = format!("{}_server", to_snake_case(&service));

    format!(
        r#"mod config;
mod client;
mod pb;
mod rpc;
mod svc;
mod types;

use std::path::PathBuf;

use crate::pb::{package}::{{{server_mod}::{service}Server}};
use roze_rpc::rpc::RpcServer;

#[tokio::main]
async fn main() -> anyhow::Result<()> {{
    let config = config::load(config_path())?;
    roze_log::init_tracing_with_config(&config)?;
    let rpc = config
        .rpc
        .clone()
        .ok_or_else(|| anyhow::anyhow!("missing rpc config"))?;
    let registry = roze_rpc::registry::build_service_registry(&config)?
        .ok_or_else(|| anyhow::anyhow!("missing registry config"))?;
    let mut registration = roze_rpc::rpc::ServiceRegistrationGuard::start(
        registry,
        config.name.clone(),
        rpc.addr,
    )
    .await?;
    let ctx = svc::ServiceContext::new(config).await?;
    RpcServer::new(rpc.addr)
        .builder()
        .add_service({service}Server::new(rpc::RpcService::new(ctx)))
        .serve(rpc.addr)
        .await?;
    registration.shutdown().await?;

    Ok(())
}}

fn config_path() -> PathBuf {{
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let manifest_config = manifest_dir.join("config.yaml");
    if manifest_config.exists() {{
        manifest_config
    }} else {{
        PathBuf::from("config.yaml")
    }}
}}
"#
    )
}

pub fn render_rpc(spec: &ApiSpec) -> String {
    let package = spec.service.replace('-', "_");
    let service = to_pascal_case(&spec.service);
    let server_mod = format!("{}_server", to_snake_case(&service));

    let mut out = String::from("#![allow(dead_code, unused_imports)]\n\n");
    out.push_str(&format!(
        "use crate::pb::{package}::{{self as proto, {server_mod}::{service}}};\n",
        package = package,
        server_mod = server_mod,
        service = service
    ));
    out.push_str("use roze_grpc::transport::{Request, Response, Status};\n");
    out.push_str("use crate::svc::ServiceContext;\n");
    out.push_str("use crate::types::*;\n\n");

    out.push_str("#[derive(Debug, Clone)]\n");
    out.push_str("pub struct RpcService {\n");
    out.push_str("    ctx: ServiceContext,\n");
    out.push_str("}\n\n");
    out.push_str("impl RpcService {\n");
    out.push_str("    pub fn new(ctx: ServiceContext) -> Self {\n");
    out.push_str("        Self { ctx }\n");
    out.push_str("    }\n");
    out.push_str("}\n\n");

    out.push_str("#[async_trait::async_trait]\n");
    out.push_str(&format!(
        "impl {service} for RpcService {{\n",
        service = service
    ));
    for route in &spec.rest_routes {
        out.push_str(&render_route_method(spec, route));
    }
    for method in &spec.rpc_methods {
        out.push_str(&render_rpc_method(spec, method));
    }
    out.push_str("}\n");

    out
}

pub fn render_client(spec: &ApiSpec) -> String {
    let package = spec.service.replace('-', "_");
    let service = to_pascal_case(&spec.service);
    let client_mod = format!("{}_client", to_snake_case(&service));

    let mut out = String::from("#![allow(dead_code, unused_imports)]\n\n");
    out.push_str(&format!(
        "use crate::pb::{package}::{{self as proto, {client_mod}::{service}Client as ProtoClient}};\n",
        package = package,
        client_mod = client_mod,
        service = service
    ));
    out.push_str("use roze_rpc::balance::Balancer;\n");
    out.push_str("use roze_rpc::registry::{CachedRegistryResolver, Registry};\n");
    out.push_str("use roze_grpc::transport::{Channel, Endpoint, Request, Status};\n\n");

    out.push_str("#[derive(Debug, Clone)]\n");
    out.push_str("pub struct RpcClient {\n");
    out.push_str("    inner: ProtoClient<Channel>,\n");
    out.push_str("    options: roze_rpc::rpc::RpcClientOptions,\n");
    out.push_str("    client_config: Option<roze_config::RpcClientConfig>,\n");
    out.push_str("}\n\n");
    out.push_str("impl RpcClient {\n");
    out.push_str("    pub fn new(channel: Channel) -> Self {\n");
    out.push_str("        Self {\n");
    out.push_str("            inner: ProtoClient::new(channel),\n");
    out.push_str("            options: roze_rpc::rpc::RpcClientOptions::default(),\n");
    out.push_str("            client_config: None,\n");
    out.push_str("        }\n");
    out.push_str("    }\n\n");
    out.push_str(
        "    pub fn with_options(channel: Channel, options: roze_rpc::rpc::RpcClientOptions) -> Self {\n",
    );
    out.push_str("        Self {\n");
    out.push_str("            inner: ProtoClient::new(channel),\n");
    out.push_str("            options,\n");
    out.push_str("            client_config: None,\n");
    out.push_str("        }\n");
    out.push_str("    }\n\n");
    out.push_str(
        "    pub fn with_config(channel: Channel, config: roze_config::RpcClientConfig) -> Self {\n",
    );
    out.push_str("        let options = roze_rpc::rpc::RpcClientOptions::from_config(&config);\n");
    out.push_str("        Self {\n");
    out.push_str("            inner: ProtoClient::new(channel),\n");
    out.push_str("            options,\n");
    out.push_str("            client_config: Some(config),\n");
    out.push_str("        }\n");
    out.push_str("    }\n\n");
    out.push_str("    pub fn inner_mut(&mut self) -> &mut ProtoClient<Channel> {\n");
    out.push_str("        &mut self.inner\n");
    out.push_str("    }\n\n");
    out.push_str("    pub async fn connect(addr: impl AsRef<str>) -> anyhow::Result<Self> {\n");
    out.push_str("        let url = roze_rpc::rpc::normalize_endpoint(addr.as_ref())?;\n");
    out.push_str("        let options = roze_rpc::rpc::RpcClientOptions::default();\n");
    out.push_str(
        "        let channel = Endpoint::from_shared(url)?.connect_timeout(options.connect_timeout).timeout(options.request_timeout).connect().await?;\n",
    );
    out.push_str("        Ok(Self::with_options(channel, options))\n");
    out.push_str("    }\n\n");
    out.push_str(
        "    pub async fn connect_from_config(config: roze_config::RpcClientConfig) -> anyhow::Result<Self> {\n",
    );
    out.push_str(
        "        let channel = roze_rpc::rpc::connect_channel_from_config(&config).await?;\n",
    );
    out.push_str("        Ok(Self::with_config(channel, config))\n");
    out.push_str("    }\n\n");
    out.push_str(
        "    pub async fn connect_via_registry<R, B>(service: &str, registry: &R, balancer: &B) -> anyhow::Result<Self>\n",
    );
    out.push_str("    where\n");
    out.push_str("        R: Registry,\n");
    out.push_str("        B: Balancer,\n");
    out.push_str("    {\n");
    out.push_str("        let channel = roze_rpc::rpc::connect_via_registry_with_options(service, registry, balancer, roze_rpc::rpc::RpcClientOptions::default()).await?;\n");
    out.push_str(
        "        Ok(Self::with_options(channel, roze_rpc::rpc::RpcClientOptions::default()))\n",
    );
    out.push_str("    }\n");
    out.push_str(
        "\n    pub async fn connect_via_cached_registry<R, B>(service: &str, resolver: &CachedRegistryResolver<R, B>) -> anyhow::Result<Self>\n",
    );
    out.push_str("    where\n");
    out.push_str("        R: Registry,\n");
    out.push_str("        B: Balancer,\n");
    out.push_str("    {\n");
    out.push_str("        let channel = roze_rpc::rpc::connect_via_cached_registry_with_options(service, resolver, roze_rpc::rpc::RpcClientOptions::default()).await?;\n");
    out.push_str(
        "        Ok(Self::with_options(channel, roze_rpc::rpc::RpcClientOptions::default()))\n",
    );
    out.push_str("    }\n");
    out.push_str("}\n");

    for route in &spec.rest_routes {
        let handler = resolved_handler_name(route);
        out.push_str(&format!(
            "\nimpl RpcClient {{\n    pub async fn {handler}(&mut self, context: &roze_context::Context, req: proto::{request}) -> Result<proto::{response}, Status> {{\n        let options = self.options;\n        let client_config = self.client_config.clone();\n        let request_template = req.clone();\n        let context = context.clone();\n        let inner = self.inner.clone();\n        let response = roze_rpc::rpc::retry_status(\n            || {{\n                let mut request = Request::new(request_template.clone());\n                let context = context.clone();\n                let mut inner = inner.clone();\n                let client_config = client_config.clone();\n                async move {{\n                    if let Some(timeout) = context.remaining_timeout() {{\n                        request.set_timeout(timeout);\n                    }} else {{\n                        request.set_timeout(options.request_timeout);\n                    }}\n                    roze_rpc::rpc::apply_request_context(&mut request, &context);\n                    roze_rpc::rpc::apply_client_auth(&mut request, &options, client_config.as_ref());\n                    inner.{handler}(request).await\n                }}\n            }},\n            options,\n        ).await?;\n        Ok(response.into_inner())\n    }}\n}}\n",
            handler = handler,
            request = route.request,
            response = route.response
        ));
    }

    for method in &spec.rpc_methods {
        let method_name = to_snake_case(&method.name);
        out.push_str(&format!(
            "\nimpl RpcClient {{\n    pub async fn {method_name}(&mut self, context: &roze_context::Context, req: proto::{request}) -> Result<proto::{response}, Status> {{\n        let options = self.options;\n        let client_config = self.client_config.clone();\n        let request_template = req.clone();\n        let context = context.clone();\n        let inner = self.inner.clone();\n        let response = roze_rpc::rpc::retry_status(\n            || {{\n                let mut request = Request::new(request_template.clone());\n                let context = context.clone();\n                let mut inner = inner.clone();\n                let client_config = client_config.clone();\n                async move {{\n                    if let Some(timeout) = context.remaining_timeout() {{\n                        request.set_timeout(timeout);\n                    }} else {{\n                        request.set_timeout(options.request_timeout);\n                    }}\n                    roze_rpc::rpc::apply_request_context(&mut request, &context);\n                    roze_rpc::rpc::apply_client_auth(&mut request, &options, client_config.as_ref());\n                    inner.{method_name}(request).await\n                }}\n            }},\n            options,\n        ).await?;\n        Ok(response.into_inner())\n    }}\n}}\n",
            method_name = method_name,
            request = method.request,
            response = method.response
        ));
    }

    out
}

fn render_route_method(spec: &ApiSpec, route: &RestRoute) -> String {
    let handler = resolved_handler_name(route);
    let req_ty = &route.request;
    let resp_ty = &route.response;
    let mut out = String::new();

    out.push_str(&format!(
        "    async fn {handler}(&self, request: Request<proto::{req_ty}>) -> Result<Response<proto::{resp_ty}>, Status> {{\n",
        handler = handler,
        req_ty = req_ty,
        resp_ty = resp_ty
    ));
    out.push_str("        let request_ctx = roze_rpc::rpc::request_context(&request);\n");
    out.push_str(&format!(
        "        let (request_ctx, method_guard) = roze_rpc::rpc::begin_method(self.ctx.config.name.clone(), {:?}, request_ctx, Some(&self.ctx.config.governance))?;\n",
        handler
    ));
    out.push_str("        let req = request.into_inner();\n");
    out.push_str(&format!(
        "        let req = {};\n",
        proto_to_app(spec, req_ty, "req")
    ));
    out.push_str(&format!(
        "        let result = crate::logic::{handler}({args}).await;\n",
        handler = handler,
        args = "self.ctx.clone(), request_ctx, req"
    ));
    out.push_str("        match result {\n");
    out.push_str(&format!(
        "            Ok(resp) => {{\n                roze_rpc::rpc::finish_method(method_guard, \"ok\");\n                Ok(Response::new({}))\n            }}\n",
        app_to_proto(spec, resp_ty, "resp")
    ));
    out.push_str("            Err(err) => {\n                roze_rpc::rpc::finish_method(method_guard, \"internal\");\n                Err(Status::internal(err.to_string()))\n            }\n        }\n");
    out.push_str("    }\n");
    out
}

fn render_rpc_method(spec: &ApiSpec, method: &RpcMethod) -> String {
    let method_name = to_snake_case(&method.name);
    let req_ty = &method.request;
    let resp_ty = &method.response;
    let mut out = String::new();

    out.push_str(&format!(
        "    async fn {method_name}(&self, request: Request<proto::{req_ty}>) -> Result<Response<proto::{resp_ty}>, Status> {{\n",
        method_name = method_name,
        req_ty = req_ty,
        resp_ty = resp_ty
    ));
    out.push_str("        let request_ctx = roze_rpc::rpc::request_context(&request);\n");
    out.push_str(&format!(
        "        let (_request_ctx, method_guard) = roze_rpc::rpc::begin_method(self.ctx.config.name.clone(), {:?}, request_ctx, Some(&self.ctx.config.governance))?;\n",
        method.name
    ));
    out.push_str("        let req = request.into_inner();\n");
    out.push_str(&format!(
        "        let req = {};\n",
        proto_to_app(spec, req_ty, "req")
    ));
    out.push_str("        let _ = req;\n");
    out.push_str(&format!(
        "        let resp = {} {{ {} }};\n",
        resp_ty,
        default_fields(spec, resp_ty)
    ));
    out.push_str("        roze_rpc::rpc::finish_method(method_guard, \"ok\");\n");
    out.push_str(&format!(
        "        Ok(Response::new({}))\n",
        app_to_proto(spec, resp_ty, "resp")
    ));
    out.push_str("    }\n");
    out
}

fn resolved_handler_name(route: &RestRoute) -> String {
    route
        .handler
        .as_ref()
        .map(|handler| to_snake_case(handler))
        .unwrap_or_else(|| handler_name(&route.method, &route.path))
}

fn proto_to_app(spec: &ApiSpec, ty_name: &str, var: &str) -> String {
    let Some(ty) = find_type(spec, ty_name) else {
        return format!("{ty_name} {{ }}");
    };

    let fields = ty
        .fields
        .iter()
        .map(|field| {
            let name = rust_field_name(field);
            format!("{name}: {var}.{name}")
        })
        .collect::<Vec<_>>()
        .join(", ");

    format!("{ty_name} {{ {fields} }}")
}

fn app_to_proto(spec: &ApiSpec, ty_name: &str, var: &str) -> String {
    let Some(ty) = find_type(spec, ty_name) else {
        return format!("proto::{ty_name} {{ }}");
    };

    let fields = ty
        .fields
        .iter()
        .map(|field| {
            let name = rust_field_name(field);
            format!("{name}: {var}.{name}")
        })
        .collect::<Vec<_>>()
        .join(", ");

    format!("proto::{ty_name} {{ {fields} }}")
}

fn find_type<'a>(spec: &'a ApiSpec, ty_name: &str) -> Option<&'a TypeDef> {
    spec.types.iter().find(|ty| ty.name == ty_name)
}

fn rust_field_name(field: &Field) -> String {
    field
        .json_name
        .clone()
        .unwrap_or_else(|| to_snake_case(&field.name))
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

fn default_fields(spec: &ApiSpec, name: &str) -> String {
    spec.types
        .iter()
        .find(|ty| ty.name == name)
        .map(|ty| {
            ty.fields
                .iter()
                .map(|field| {
                    let name = field
                        .json_name
                        .clone()
                        .unwrap_or_else(|| to_snake_case(&field.name));
                    format!("{}: {}", name, default_value(&field.ty))
                })
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default()
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
    use super::*;
    use crate::parser::parse_api;

    #[test]
    fn client_supports_config_based_connection_and_auth() {
        let spec = parse_api(
            r#"
            service user {
                rpc GetUser (GetUserReq) returns (GetUserResp)
            }

            type GetUserReq {
                id: u64
            }

            type GetUserResp {
                name: string
            }
            "#,
        )
        .expect("api");

        let client = render_client(&spec);
        assert!(client.contains("client_config: Option<roze_config::RpcClientConfig>"));
        assert!(client.contains("pub async fn connect_from_config"));
        assert!(client.contains("RpcClientOptions::from_config(&config)"));
        assert!(
            client.contains("apply_client_auth(&mut request, &options, client_config.as_ref())")
        );
    }
}
