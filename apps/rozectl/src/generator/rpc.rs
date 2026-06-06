use crate::{
    generator::{to_pascal_case, to_snake_case},
    parser::{ApiSpec, Field, HttpMethod, RestRoute, RpcMethod, TypeDef},
};

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

    out.push_str("#[tonic::async_trait]\n");
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
    out.push_str("use tonic::transport::{Channel, Endpoint};\n\n");

    out.push_str("#[derive(Debug, Clone)]\n");
    out.push_str("pub struct RpcClient {\n");
    out.push_str("    inner: ProtoClient<Channel>,\n");
    out.push_str("    options: roze_rpc::rpc::RpcClientOptions,\n");
    out.push_str("}\n\n");
    out.push_str("impl RpcClient {\n");
    out.push_str("    pub fn new(channel: Channel) -> Self {\n");
    out.push_str("        Self {\n");
    out.push_str("            inner: ProtoClient::new(channel),\n");
    out.push_str("            options: roze_rpc::rpc::RpcClientOptions::default(),\n");
    out.push_str("        }\n");
    out.push_str("    }\n\n");
    out.push_str(
        "    pub fn with_options(channel: Channel, options: roze_rpc::rpc::RpcClientOptions) -> Self {\n",
    );
    out.push_str("        Self {\n");
    out.push_str("            inner: ProtoClient::new(channel),\n");
    out.push_str("            options,\n");
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
        let handler = route
            .handler
            .clone()
            .unwrap_or_else(|| handler_name(&route.method, &route.path));
        out.push_str(&format!(
            "\nimpl RpcClient {{\n    pub async fn {handler}(&mut self, context: &roze_context::Context, req: proto::{request}) -> Result<proto::{response}, tonic::Status> {{\n        let options = self.options;\n        let request_template = req.clone();\n        let context = context.clone();\n        let inner = self.inner.clone();\n        let response = roze_rpc::rpc::retry_status(\n            || {{\n                let mut request = tonic::Request::new(request_template.clone());\n                let context = context.clone();\n                let mut inner = inner.clone();\n                async move {{\n                    if let Some(timeout) = context.remaining_timeout() {{\n                        request.set_timeout(timeout);\n                    }} else {{\n                        request.set_timeout(options.request_timeout);\n                    }}\n                    roze_rpc::rpc::apply_request_context(&mut request, &context);\n                    inner.{handler}(request).await\n                }}\n            }},\n            options,\n        ).await?;\n        Ok(response.into_inner())\n    }}\n}}\n",
            handler = handler,
            request = route.request,
            response = route.response
        ));
    }

    for method in &spec.rpc_methods {
        let method_name = to_snake_case(&method.name);
        out.push_str(&format!(
            "\nimpl RpcClient {{\n    pub async fn {method_name}(&mut self, context: &roze_context::Context, req: proto::{request}) -> Result<proto::{response}, tonic::Status> {{\n        let options = self.options;\n        let request_template = req.clone();\n        let context = context.clone();\n        let inner = self.inner.clone();\n        let response = roze_rpc::rpc::retry_status(\n            || {{\n                let mut request = tonic::Request::new(request_template.clone());\n                let context = context.clone();\n                let mut inner = inner.clone();\n                async move {{\n                    if let Some(timeout) = context.remaining_timeout() {{\n                        request.set_timeout(timeout);\n                    }} else {{\n                        request.set_timeout(options.request_timeout);\n                    }}\n                    roze_rpc::rpc::apply_request_context(&mut request, &context);\n                    inner.{method_name}(request).await\n                }}\n            }},\n            options,\n        ).await?;\n        Ok(response.into_inner())\n    }}\n}}\n",
            method_name = method_name,
            request = method.request,
            response = method.response
        ));
    }

    out
}

fn render_route_method(spec: &ApiSpec, route: &RestRoute) -> String {
    let handler = route
        .handler
        .clone()
        .unwrap_or_else(|| handler_name(&route.method, &route.path));
    let req_ty = &route.request;
    let resp_ty = &route.response;
    let mut out = String::new();

    out.push_str(&format!(
        "    async fn {handler}(&self, request: tonic::Request<proto::{req_ty}>) -> Result<tonic::Response<proto::{resp_ty}>, tonic::Status> {{\n",
        handler = handler,
        req_ty = req_ty,
        resp_ty = resp_ty
    ));
    out.push_str("        let request_ctx = roze_rpc::rpc::request_context(&request);\n");
    out.push_str("        let req = request.into_inner();\n");
    out.push_str(&format!(
        "        let req = {};\n",
        proto_to_app(spec, req_ty, "req")
    ));
    out.push_str(&format!(
        "        let resp = crate::logic::{handler}({args})\n",
        handler = handler,
        args = match route.method {
            HttpMethod::Get | HttpMethod::Delete => "self.ctx.clone(), request_ctx",
            HttpMethod::Post | HttpMethod::Put => "self.ctx.clone(), request_ctx, req",
        }
    ));
    out.push_str("            .await\n");
    out.push_str("            .map_err(|err| tonic::Status::internal(err.to_string()))?;\n");
    out.push_str(&format!(
        "        Ok(tonic::Response::new({}))\n",
        app_to_proto(spec, resp_ty, "resp")
    ));
    out.push_str("    }\n");
    out
}

fn render_rpc_method(spec: &ApiSpec, method: &RpcMethod) -> String {
    let method_name = to_snake_case(&method.name);
    let req_ty = &method.request;
    let resp_ty = &method.response;
    let mut out = String::new();

    out.push_str(&format!(
        "    async fn {method_name}(&self, request: tonic::Request<proto::{req_ty}>) -> Result<tonic::Response<proto::{resp_ty}>, tonic::Status> {{\n",
        method_name = method_name,
        req_ty = req_ty,
        resp_ty = resp_ty
    ));
    out.push_str("        let request_ctx = roze_rpc::rpc::request_context(&request);\n");
    out.push_str("        let req = request.into_inner();\n");
    out.push_str(&format!(
        "        let req = {};\n",
        proto_to_app(spec, req_ty, "req")
    ));
    out.push_str("        let _ = request_ctx;\n");
    out.push_str("        let _ = req;\n");
    out.push_str(&format!(
        "        let resp = {} {{ {} }};\n",
        resp_ty,
        default_fields(spec, resp_ty)
    ));
    out.push_str(&format!(
        "        Ok(tonic::Response::new({}))\n",
        app_to_proto(spec, resp_ty, "resp")
    ));
    out.push_str("    }\n");
    out
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
        .replace('/', "_")
        .replace('-', "_");

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
