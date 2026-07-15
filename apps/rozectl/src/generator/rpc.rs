use crate::{
    generator::{rust_identifier, to_pascal_case, to_snake_case},
    parser::{ApiSpec, Field, HttpMethod, RestRoute, RpcMethod, TypeDef},
};

pub fn render_main(spec: &ApiSpec) -> String {
    let package = to_snake_case(&spec.service);
    let service = to_pascal_case(&spec.service);
    let server_mod = format!("{}_server", to_snake_case(&service));

    format!(
        r#"mod config;
mod client;
mod logic;
mod pb;
mod server;
mod svc;
mod types;

use std::path::PathBuf;

use crate::pb::{package}::{{{server_mod}::{service}Server}};
use roze_rpc::rpc::RpcServer;
use roze_service::ServiceGroup;

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
    let mut registration = roze_rpc::rpc::ServiceRegistrationGuard::start_with_advertise_addr(
        registry,
        config.name.clone(),
        rpc.addr,
        rpc.advertise_addr.unwrap_or(rpc.addr),
    )
    .await?;
    let service_name = config.name.clone();
    let rpc_addr = rpc.addr;
    let ctx = svc::ServiceContext::new(config).await?;
    let health = ctx.health.clone();
    let (rpc_health, grpc_health_service) =
        roze_rpc::health::RpcHealthReporter::new_for::<{service}Server<server::RpcService>>(
            health,
        );
    rpc_health.refresh().await;
    let mut group = ServiceGroup::new();
    group.add_fn(service_name, move |shutdown| {{
        let ctx = ctx.clone();
        let grpc_health_service = grpc_health_service.clone();
        async move {{
            let mut builder = RpcServer::new(rpc_addr).builder();
            builder
                .add_service(grpc_health_service)
                .add_service({service}Server::new(server::RpcService::new(ctx)))
                .serve_with_shutdown(rpc_addr, async move {{
                    shutdown.wait().await;
                }})
                .await
                .map_err(|error| anyhow::anyhow!("RPC service failed: {{error}}"))
        }}
    }});
    group.add_fn("grpc-health-sync", move |shutdown| {{
        let rpc_health = rpc_health.clone();
        async move {{
            rpc_health
                .run_until(std::time::Duration::from_secs(1), shutdown.wait())
                .await;
            Ok(())
        }}
    }});
    let result = group.start().await;
    registration.shutdown().await?;
    result?;

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

pub fn render_lib() -> String {
    "pub mod client;\npub mod pb;\n".to_string()
}

pub fn render_rpc(spec: &ApiSpec) -> String {
    let package = to_snake_case(&spec.service);
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

    out.push_str("#[derive(Clone)]\n");
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
    let package = to_snake_case(&spec.service);
    let service = to_pascal_case(&spec.service);
    let service_name = &spec.service;
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
    out.push_str("use roze_grpc::transport::{Channel, Endpoint, Status};\n\n");

    out.push_str("#[derive(Debug, Clone)]\n");
    out.push_str("pub struct RpcClient {\n");
    out.push_str("    inner: ProtoClient<Channel>,\n");
    out.push_str("    options: roze_rpc::rpc::RpcClientOptions,\n");
    out.push_str("    client_config: Option<roze_config::RpcClientConfig>,\n");
    out.push_str("    governance: Option<roze_config::GovernanceConfig>,\n");
    out.push_str("}\n\n");
    out.push_str("impl RpcClient {\n");
    out.push_str("    pub fn new(channel: Channel) -> Self {\n");
    out.push_str("        Self {\n");
    out.push_str("            inner: ProtoClient::new(channel),\n");
    out.push_str("            options: roze_rpc::rpc::RpcClientOptions::default(),\n");
    out.push_str("            client_config: None,\n");
    out.push_str("            governance: None,\n");
    out.push_str("        }\n");
    out.push_str("    }\n\n");
    out.push_str(
        "    pub fn with_options(channel: Channel, options: roze_rpc::rpc::RpcClientOptions) -> Self {\n",
    );
    out.push_str("        Self {\n");
    out.push_str("            inner: ProtoClient::new(channel),\n");
    out.push_str("            options,\n");
    out.push_str("            client_config: None,\n");
    out.push_str("            governance: None,\n");
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
    out.push_str("            governance: None,\n");
    out.push_str("        }\n");
    out.push_str("    }\n\n");
    out.push_str(
        "    pub fn with_governance(mut self, governance: roze_config::GovernanceConfig) -> Self {\n",
    );
    out.push_str("        self.governance = Some(governance);\n");
    out.push_str("        self\n");
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
        let retry_request_expr = retry_request_template_expr(spec, &route.request);
        out.push_str(&format!(
            "\nimpl RpcClient {{\n    pub async fn {handler}(&mut self, context: &roze_context::Context, req: proto::{request}) -> Result<proto::{response}, Status> {{\n        let options = self.options;\n        let client_config = self.client_config.clone();\n        let governance = self.governance.clone();\n        let request_template = req;\n        let context = context.clone();\n        let inner = self.inner.clone();\n        let response = roze_rpc::rpc::retry_status_for_method(\n            {service_name:?},\n            &context,\n            || {{\n                let context = context.clone();\n                let mut inner = inner.clone();\n                let client_config = client_config.clone();\n                let request = roze_rpc::rpc::client_request({retry_request_expr}, &context, options, client_config.as_ref());\n                async move {{ inner.{handler}(request).await }}\n            }},\n            options,\n            governance.as_ref(),\n            {governance_key:?},\n        ).await?;\n        Ok(response.into_inner())\n    }}\n}}\n",
            handler = handler,
            request = proto_type_name(&route.request),
            response = proto_type_name(&route.response),
            retry_request_expr = retry_request_expr,
            governance_key = handler.clone(),
            service_name = service_name,
        ));
    }

    for method in &spec.rpc_methods {
        let method_name = to_snake_case(&method.name);
        let retry_request_expr = retry_request_template_expr(spec, &method.request);
        out.push_str(&format!(
            "\nimpl RpcClient {{\n    pub async fn {method_name}(&mut self, context: &roze_context::Context, req: proto::{request}) -> Result<proto::{response}, Status> {{\n        let options = self.options;\n        let client_config = self.client_config.clone();\n        let governance = self.governance.clone();\n        let request_template = req;\n        let context = context.clone();\n        let inner = self.inner.clone();\n        let response = roze_rpc::rpc::retry_status_for_method(\n            {service_name:?},\n            &context,\n            || {{\n                let context = context.clone();\n                let mut inner = inner.clone();\n                let client_config = client_config.clone();\n                let request = roze_rpc::rpc::client_request({retry_request_expr}, &context, options, client_config.as_ref());\n                async move {{ inner.{method_name}(request).await }}\n            }},\n            options,\n            governance.as_ref(),\n            {governance_key:?},\n        ).await?;\n        Ok(response.into_inner())\n    }}\n}}\n",
            method_name = method_name,
            request = proto_type_name(&method.request),
            response = proto_type_name(&method.response),
            retry_request_expr = retry_request_expr,
            governance_key = &method.name,
            service_name = service_name,
        ));
    }

    out
}

fn render_route_method(spec: &ApiSpec, route: &RestRoute) -> String {
    let handler = resolved_handler_name(route);
    let req_ty = &route.request;
    let resp_ty = &route.response;
    let uses_idempotency = route_uses_idempotency(spec, route);
    let mut out = String::new();

    out.push_str(&format!(
        "    async fn {handler}(&self, request: Request<proto::{req_ty}>) -> Result<Response<proto::{resp_ty}>, Status> {{\n",
        handler = handler,
        req_ty = proto_type_name(req_ty),
        resp_ty = proto_type_name(resp_ty)
    ));
    out.push_str("        let request_ctx = roze_rpc::rpc::request_context(&request);\n");
    out.push_str(&format!(
        "        let (request_ctx, method_guard) = roze_rpc::rpc::begin_method(self.ctx.config.name.clone(), {:?}, request_ctx, Some(&self.ctx.config.governance))?;\n",
        handler
    ));
    if !route.permissions.is_empty() {
        out.push_str(&format!(
            "        if let Err(status) = roze_rpc::rpc::enforce_permissions(&request_ctx, &[{}]) {{\n            roze_rpc::rpc::finish_method(method_guard, \"permission_denied\");\n            return Err(status);\n        }}\n",
            route
                .permissions
                .iter()
                .map(|permission| format!("{permission:?}"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if uses_idempotency {
        out.push_str("        let idempotency_key = match request.metadata().get(\"idempotency-key\").and_then(|value| value.to_str().ok()).filter(|value| !value.trim().is_empty()) {\n            Some(value) => value.to_string(),\n            None => {\n                roze_rpc::rpc::finish_method(method_guard, \"invalid_argument\");\n                return Err(roze_rpc::rpc::status_from_error(roze_middleware::idempotency_error(400, roze_middleware::IDEMPOTENCY_MISSING_KEY, \"missing idempotency-key metadata\"), &request_ctx));\n            }\n        };\n");
    }
    if request_uses_proto_fields(spec, req_ty) {
        out.push_str("        let req = request.into_inner();\n");
    }
    out.push_str(&format!(
        "        let req = {};\n",
        proto_to_app(spec, req_ty, "req")
    ));
    out.push_str("        if let Err(message) = roze_validation::validate_or_message_i18n(&req, request_ctx.locale().as_deref()) {\n");
    out.push_str("            roze_rpc::rpc::finish_method(method_guard, \"invalid_argument\");\n");
    out.push_str(
        "            return Err(roze_rpc::rpc::invalid_argument_status(message, &request_ctx));\n",
    );
    out.push_str("        }\n");
    out.push_str(&render_rpc_request_validation_checks(spec, req_ty));
    if uses_idempotency {
        out.push_str(&format!(
            "        let idempotency_fingerprint = roze_middleware::idempotency_fingerprint(&req).map_err(|err| roze_rpc::rpc::status_from_error(err, &request_ctx))?;\n        match roze_middleware::begin_idempotency(self.ctx.idempotency.as_ref(), {handler:?}, &idempotency_key, &idempotency_fingerprint, roze_middleware::idempotency_now_millis()).await.map_err(|err| roze_rpc::rpc::status_from_error(err, &request_ctx))? {{\n            roze_middleware::IdempotencyDecision::Execute => {{}}\n            roze_middleware::IdempotencyDecision::Replay(value) => {{\n                let resp = serde_json::from_value(value).map_err(|err| roze_rpc::rpc::status_from_error(roze_middleware::idempotency_error(500, roze_middleware::IDEMPOTENCY_REPLAY_INVALID, &format!(\"invalid idempotency replay response: {{err}}\")), &request_ctx))?;\n                roze_rpc::rpc::finish_method(method_guard, \"ok\");\n                return Ok(Response::new({}));\n            }}\n            roze_middleware::IdempotencyDecision::InFlight => {{\n                roze_rpc::rpc::finish_method(method_guard, \"already_exists\");\n                return Err(roze_rpc::rpc::status_from_error(roze_middleware::idempotency_error(409, roze_middleware::IDEMPOTENCY_IN_FLIGHT, \"idempotency request is in progress\"), &request_ctx));\n            }}\n            roze_middleware::IdempotencyDecision::Conflict => {{\n                roze_rpc::rpc::finish_method(method_guard, \"already_exists\");\n                return Err(roze_rpc::rpc::status_from_error(roze_middleware::idempotency_error(409, roze_middleware::IDEMPOTENCY_KEY_REUSED, \"idempotency key was reused with a different request\"), &request_ctx));\n            }}\n        }}\n",
            app_to_proto(spec, resp_ty, "resp"),
            handler = handler
        ));
    }
    out.push_str(&format!(
        "        let result = crate::logic::{handler}({args}).await;\n",
        handler = handler,
        args = "self.ctx.clone(), request_ctx.clone(), req"
    ));
    out.push_str("        match result {\n");
    if uses_idempotency {
        out.push_str(&format!(
            "            Ok(resp) => {{\n                let value = serde_json::to_value(&resp).map_err(|err| roze_rpc::rpc::status_from_error(roze_middleware::idempotency_error(500, roze_middleware::IDEMPOTENCY_REPLAY_INVALID, &format!(\"invalid idempotency response: {{err}}\")), &request_ctx))?;\n                roze_middleware::complete_idempotency(self.ctx.idempotency.as_ref(), {handler:?}, &idempotency_key, &idempotency_fingerprint, value).await.map_err(|err| roze_rpc::rpc::status_from_error(err, &request_ctx))?;\n                roze_rpc::rpc::finish_method(method_guard, \"ok\");\n                Ok(Response::new({}))\n            }}\n",
            app_to_proto(spec, resp_ty, "resp"),
            handler = handler
        ));
        out.push_str(&format!(
            "            Err(mut err) => {{\n                roze_middleware::fail_idempotency(self.ctx.idempotency.as_ref(), {handler:?}, &idempotency_key, &idempotency_fingerprint).await;\n                err = roze_rpc::rpc::apply_fallback(\n                    self.ctx.config.name.as_str(),\n                    err,\n                    roze_rpc::rpc::method_fallback(Some(&self.ctx.config.governance), {handler:?}),\n                );\n                roze_rpc::rpc::finish_method(method_guard, err.kind());\n                Err(roze_rpc::rpc::status_from_error(err, &request_ctx))\n            }}\n        }}\n",
            handler = handler
        ));
    } else {
        out.push_str(&format!(
            "            Ok(resp) => {{\n                roze_rpc::rpc::finish_method(method_guard, \"ok\");\n                Ok(Response::new({}))\n            }}\n",
            app_to_proto(spec, resp_ty, "resp")
        ));
        out.push_str(&format!(
            "            Err(mut err) => {{\n                err = roze_rpc::rpc::apply_fallback(\n                    self.ctx.config.name.as_str(),\n                    err,\n                    roze_rpc::rpc::method_fallback(Some(&self.ctx.config.governance), {handler:?}),\n                );\n                roze_rpc::rpc::finish_method(method_guard, err.kind());\n                Err(roze_rpc::rpc::status_from_error(err, &request_ctx))\n            }}\n        }}\n",
            handler = handler
        ));
    }
    out.push_str("    }\n");
    out
}

fn render_rpc_method(spec: &ApiSpec, method: &RpcMethod) -> String {
    let method_name = to_snake_case(&method.name);
    let req_ty = &method.request;
    let resp_ty = &method.response;
    let uses_idempotency = rpc_method_uses_idempotency(spec, method);
    let mut out = String::new();

    out.push_str(&format!(
        "    async fn {method_name}(&self, request: Request<proto::{req_ty}>) -> Result<Response<proto::{resp_ty}>, Status> {{\n",
        method_name = method_name,
        req_ty = proto_type_name(req_ty),
        resp_ty = proto_type_name(resp_ty)
    ));
    out.push_str("        let request_ctx = roze_rpc::rpc::request_context(&request);\n");
    out.push_str(&format!(
        "        let (request_ctx, method_guard) = roze_rpc::rpc::begin_method(self.ctx.config.name.clone(), {:?}, request_ctx, Some(&self.ctx.config.governance))?;\n",
        method.name
    ));
    if !method.permissions.is_empty() {
        out.push_str(&format!(
            "        if let Err(status) = roze_rpc::rpc::enforce_permissions(&request_ctx, &[{}]) {{\n            roze_rpc::rpc::finish_method(method_guard, \"permission_denied\");\n            return Err(status);\n        }}\n",
            method
                .permissions
                .iter()
                .map(|permission| format!("{permission:?}"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if uses_idempotency {
        out.push_str("        let idempotency_key = match request.metadata().get(\"idempotency-key\").and_then(|value| value.to_str().ok()).filter(|value| !value.trim().is_empty()) {\n            Some(value) => value.to_string(),\n            None => {\n                roze_rpc::rpc::finish_method(method_guard, \"invalid_argument\");\n                return Err(roze_rpc::rpc::status_from_error(roze_middleware::idempotency_error(400, roze_middleware::IDEMPOTENCY_MISSING_KEY, \"missing idempotency-key metadata\"), &request_ctx));\n            }\n        };\n");
    }
    if request_uses_proto_fields(spec, req_ty) {
        out.push_str("        let req = request.into_inner();\n");
    }
    out.push_str(&format!(
        "        let req = {};\n",
        proto_to_app(spec, req_ty, "req")
    ));
    out.push_str("        if let Err(message) = roze_validation::validate_or_message_i18n(&req, request_ctx.locale().as_deref()) {\n");
    out.push_str("            roze_rpc::rpc::finish_method(method_guard, \"invalid_argument\");\n");
    out.push_str(
        "            return Err(roze_rpc::rpc::invalid_argument_status(message, &request_ctx));\n",
    );
    out.push_str("        }\n");
    out.push_str(&render_rpc_request_validation_checks(spec, req_ty));
    if uses_idempotency {
        out.push_str(&format!(
            "        let idempotency_fingerprint = roze_middleware::idempotency_fingerprint(&req).map_err(|err| roze_rpc::rpc::status_from_error(err, &request_ctx))?;\n        match roze_middleware::begin_idempotency(self.ctx.idempotency.as_ref(), {method:?}, &idempotency_key, &idempotency_fingerprint, roze_middleware::idempotency_now_millis()).await.map_err(|err| roze_rpc::rpc::status_from_error(err, &request_ctx))? {{\n            roze_middleware::IdempotencyDecision::Execute => {{}}\n            roze_middleware::IdempotencyDecision::Replay(value) => {{\n                let resp = serde_json::from_value(value).map_err(|err| roze_rpc::rpc::status_from_error(roze_middleware::idempotency_error(500, roze_middleware::IDEMPOTENCY_REPLAY_INVALID, &format!(\"invalid idempotency replay response: {{err}}\")), &request_ctx))?;\n                roze_rpc::rpc::finish_method(method_guard, \"ok\");\n                return Ok(Response::new({}));\n            }}\n            roze_middleware::IdempotencyDecision::InFlight => {{\n                roze_rpc::rpc::finish_method(method_guard, \"already_exists\");\n                return Err(roze_rpc::rpc::status_from_error(roze_middleware::idempotency_error(409, roze_middleware::IDEMPOTENCY_IN_FLIGHT, \"idempotency request is in progress\"), &request_ctx));\n            }}\n            roze_middleware::IdempotencyDecision::Conflict => {{\n                roze_rpc::rpc::finish_method(method_guard, \"already_exists\");\n                return Err(roze_rpc::rpc::status_from_error(roze_middleware::idempotency_error(409, roze_middleware::IDEMPOTENCY_KEY_REUSED, \"idempotency key was reused with a different request\"), &request_ctx));\n            }}\n        }}\n",
            app_to_proto(spec, resp_ty, "resp"),
            method = method.name
        ));
    }
    out.push_str(&format!(
        "        let result = crate::logic::{method_name}(self.ctx.clone(), request_ctx.clone(), req).await;\n",
        method_name = method_name
    ));
    out.push_str("        match result {\n");
    if uses_idempotency {
        out.push_str(&format!(
            "            Ok(resp) => {{\n                let value = serde_json::to_value(&resp).map_err(|err| roze_rpc::rpc::status_from_error(roze_middleware::idempotency_error(500, roze_middleware::IDEMPOTENCY_REPLAY_INVALID, &format!(\"invalid idempotency response: {{err}}\")), &request_ctx))?;\n                roze_middleware::complete_idempotency(self.ctx.idempotency.as_ref(), {method:?}, &idempotency_key, &idempotency_fingerprint, value).await.map_err(|err| roze_rpc::rpc::status_from_error(err, &request_ctx))?;\n                roze_rpc::rpc::finish_method(method_guard, \"ok\");\n                Ok(Response::new({}))\n            }}\n",
            app_to_proto(spec, resp_ty, "resp"),
            method = method.name
        ));
        out.push_str(&format!(
            "            Err(mut err) => {{\n                roze_middleware::fail_idempotency(self.ctx.idempotency.as_ref(), {method:?}, &idempotency_key, &idempotency_fingerprint).await;\n                err = roze_rpc::rpc::apply_fallback(\n                    self.ctx.config.name.as_str(),\n                    err,\n                    roze_rpc::rpc::method_fallback(Some(&self.ctx.config.governance), {method:?}),\n                );\n                roze_rpc::rpc::finish_method(method_guard, err.kind());\n                Err(roze_rpc::rpc::status_from_error(err, &request_ctx))\n            }}\n        }}\n",
            method = method.name
        ));
    } else {
        out.push_str(&format!(
            "            Ok(resp) => {{\n                roze_rpc::rpc::finish_method(method_guard, \"ok\");\n                Ok(Response::new({}))\n            }}\n",
            app_to_proto(spec, resp_ty, "resp")
        ));
        out.push_str(&format!(
            "            Err(mut err) => {{\n                err = roze_rpc::rpc::apply_fallback(\n                    self.ctx.config.name.as_str(),\n                    err,\n                    roze_rpc::rpc::method_fallback(Some(&self.ctx.config.governance), {method:?}),\n                );\n                roze_rpc::rpc::finish_method(method_guard, err.kind());\n                Err(roze_rpc::rpc::status_from_error(err, &request_ctx))\n            }}\n        }}\n",
            method = method.name
        ));
    }
    out.push_str("    }\n");
    out
}

pub fn render_logic_mod(spec: &ApiSpec) -> String {
    let mut out = String::from("#![allow(dead_code)]\n\nuse roze_error::RozeError;\n\n");
    out.push_str("use crate::svc::ServiceContext;\n");
    out.push_str("use crate::types::*;\n\n");
    out.push_str(render_auth_context_helpers());
    out.push_str("// <roze:generated-rpc-logic>\n");
    for method in rpc_logic_methods(spec) {
        out.push_str(&format!("mod {method};\n"));
        out.push_str(&format!("pub use {method}::{method};\n"));
    }
    out.push_str("// </roze:generated-rpc-logic>\n");
    out
}

fn route_middlewares(spec: &ApiSpec, route: &RestRoute) -> Vec<String> {
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

fn route_uses_idempotency(spec: &ApiSpec, route: &RestRoute) -> bool {
    route_middlewares(spec, route).iter().any(|name| {
        roze_middleware::BuiltInMiddleware::parse(name)
            == Some(roze_middleware::BuiltInMiddleware::Idempotency)
    })
}

fn rpc_method_middlewares(spec: &ApiSpec, method: &RpcMethod) -> Vec<String> {
    let mut names = spec
        .server
        .as_ref()
        .map(|server| server.middlewares.clone())
        .unwrap_or_default();
    names.extend(method.middlewares.clone());
    names
}

fn rpc_method_uses_idempotency(spec: &ApiSpec, method: &RpcMethod) -> bool {
    rpc_method_middlewares(spec, method).iter().any(|name| {
        roze_middleware::BuiltInMiddleware::parse(name)
            == Some(roze_middleware::BuiltInMiddleware::Idempotency)
    })
}

fn render_auth_context_helpers() -> &'static str {
    "pub fn current_subject(request_ctx: &roze_context::Context) -> Option<String> {\n    request_ctx\n        .subject()\n        .or_else(|| request_ctx.metadata_value(roze_context::USER_ID_METADATA_KEY))\n}\n\npub fn current_user_id(request_ctx: &roze_context::Context) -> Option<String> {\n    current_subject(request_ctx)\n}\n\npub fn current_admin_id(request_ctx: &roze_context::Context) -> Option<String> {\n    current_subject(request_ctx)\n}\n\npub fn current_tenant(request_ctx: &roze_context::Context) -> Option<String> {\n    request_ctx.tenant()\n}\n\npub fn current_roles(request_ctx: &roze_context::Context) -> Vec<String> {\n    request_ctx.roles()\n}\n\npub fn current_permissions(request_ctx: &roze_context::Context) -> Vec<String> {\n    request_ctx.permissions()\n}\n\npub fn current_scope(request_ctx: &roze_context::Context) -> Option<String> {\n    request_ctx.metadata_value(roze_context::SCOPE_METADATA_KEY)\n}\n\n"
}

pub fn render_logic_files(spec: &ApiSpec) -> Vec<(String, String)> {
    let mut files = Vec::new();
    for route in &spec.rest_routes {
        let method = resolved_handler_name(route);
        files.push((
            method.clone(),
            render_logic_file(spec, &method, &route.request, &route.response),
        ));
    }
    for rpc in &spec.rpc_methods {
        let method = to_snake_case(&rpc.name);
        files.push((
            method.clone(),
            render_logic_file(spec, &method, &rpc.request, &rpc.response),
        ));
    }
    files
}

fn render_logic_file(_spec: &ApiSpec, method: &str, req_ty: &str, resp_ty: &str) -> String {
    format!(
        "use super::*;\n\npub async fn {method}(ctx: ServiceContext, request_ctx: roze_context::Context, req: {req_ty}) -> Result<{resp_ty}, RozeError> {{\n    let _ = ctx;\n    let _ = request_ctx;\n    let _ = req;\n    Ok({resp_ty}::default())\n}}\n",
        method = method,
        req_ty = req_ty,
        resp_ty = resp_ty
    )
}

fn rpc_logic_methods(spec: &ApiSpec) -> Vec<String> {
    spec.rest_routes
        .iter()
        .map(resolved_handler_name)
        .chain(
            spec.rpc_methods
                .iter()
                .map(|method| to_snake_case(&method.name)),
        )
        .collect()
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
            format!(
                "{name}: {}",
                proto_to_app_value(spec, &field.ty, &format!("{var}.{name}"))
            )
        })
        .collect::<Vec<_>>()
        .join(", ");

    format!("{ty_name} {{ {fields} }}")
}

fn request_uses_proto_fields(spec: &ApiSpec, ty_name: &str) -> bool {
    find_type(spec, ty_name).is_some_and(|ty| !ty.fields.is_empty())
}

fn app_to_proto(spec: &ApiSpec, ty_name: &str, var: &str) -> String {
    let Some(ty) = find_type(spec, ty_name) else {
        return format!("proto::{} {{ }}", proto_type_name(ty_name));
    };

    let fields = ty
        .fields
        .iter()
        .map(|field| {
            let name = rust_field_name(field);
            format!(
                "{name}: {}",
                app_to_proto_value(spec, &field.ty, &format!("{var}.{name}"))
            )
        })
        .collect::<Vec<_>>()
        .join(", ");

    format!("proto::{} {{ {fields} }}", proto_type_name(ty_name))
}

fn proto_type_name(ty_name: &str) -> String {
    to_pascal_case(ty_name)
}

fn proto_to_app_value(spec: &ApiSpec, ty: &str, expr: &str) -> String {
    if let Some(inner) = collection_element_type(ty) {
        if is_known_type(spec, &inner) {
            return format!(
                "{expr}.into_iter().map(|item| {}).collect()",
                proto_to_app(spec, &inner, "item")
            );
        }
        return expr.to_string();
    }
    if let Some((_key, value)) = map_key_value_types(ty) {
        if is_known_type(spec, &value) {
            return format!(
                "{expr}.into_iter().map(|(key, value)| (key, {})).collect()",
                proto_to_app(spec, &value, "value")
            );
        }
        return expr.to_string();
    }
    if is_known_type(spec, ty) {
        return format!(
            "{expr}.map(|value| {}).unwrap_or_default()",
            proto_to_app(spec, ty, "value")
        );
    }
    expr.to_string()
}

fn app_to_proto_value(spec: &ApiSpec, ty: &str, expr: &str) -> String {
    if let Some(inner) = collection_element_type(ty) {
        if is_known_type(spec, &inner) {
            return format!(
                "{expr}.into_iter().map(|item| {}).collect()",
                app_to_proto(spec, &inner, "item")
            );
        }
        return expr.to_string();
    }
    if let Some((_key, value)) = map_key_value_types(ty) {
        if is_known_type(spec, &value) {
            return format!(
                "{expr}.into_iter().map(|(key, value)| (key, {})).collect()",
                app_to_proto(spec, &value, "value")
            );
        }
        return expr.to_string();
    }
    if is_known_type(spec, ty) {
        return format!("Some({})", app_to_proto(spec, ty, expr));
    }
    expr.to_string()
}

fn is_known_type(spec: &ApiSpec, ty_name: &str) -> bool {
    find_type(spec, ty_name).is_some()
}

fn retry_request_template_expr(spec: &ApiSpec, ty_name: &str) -> &'static str {
    if is_copy_proto_message(spec, ty_name) {
        "request_template"
    } else {
        "request_template.clone()"
    }
}

fn is_copy_proto_message(spec: &ApiSpec, ty_name: &str) -> bool {
    find_type(spec, ty_name).is_some_and(|ty| {
        ty.fields
            .iter()
            .all(|field| is_copy_proto_field_type(&field.ty))
    })
}

fn is_copy_proto_field_type(ty: &str) -> bool {
    matches!(
        ty,
        "bool"
            | "i8"
            | "i16"
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
            | "int32"
            | "int64"
            | "uint32"
            | "uint64"
            | "sint32"
            | "sint64"
            | "fixed32"
            | "fixed64"
            | "sfixed32"
            | "sfixed64"
            | "float"
            | "double"
    )
}

fn find_type<'a>(spec: &'a ApiSpec, ty_name: &str) -> Option<&'a TypeDef> {
    spec.types.iter().find(|ty| ty.name == ty_name)
}

fn rust_field_name(field: &Field) -> String {
    let name = field
        .json_name
        .clone()
        .unwrap_or_else(|| to_snake_case(&field.name));
    rust_identifier(&name)
}

fn render_rpc_request_validation_checks(spec: &ApiSpec, ty_name: &str) -> String {
    let Some(ty) = find_type(spec, ty_name) else {
        return String::new();
    };

    let mut out = String::new();
    for field in &ty.fields {
        out.push_str(&custom_validation_checks(
            field,
            &ty.fields,
            &format!("req.{}", rust_field_name(field)),
        ));
    }
    out
}

fn custom_validation_checks(field: &Field, fields: &[Field], expr: &str) -> String {
    let Some(rules) = field.validate.as_deref() else {
        return String::new();
    };
    if has_rule(rules, "optional") || has_rule(rules, "omitempty") {
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
            "        {{\n            let value = {expr}.to_string();\n            if ![{allowed}].contains(&value.as_str()) {{\n                roze_rpc::rpc::finish_method(method_guard, \"invalid_argument\");\n                return Err(roze_rpc::rpc::invalid_argument_status(format!(\"{field_label} must be one of: {{}}\", {allowed_message:?}), &request_ctx));\n            }}\n        }}\n"
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
            "        if !{expr}.starts_with({prefix:?}) {{\n            roze_rpc::rpc::finish_method(method_guard, \"invalid_argument\");\n            return Err(roze_rpc::rpc::invalid_argument_status(format!(\"{field_label} must start with {{}}\", {prefix:?}), &request_ctx));\n        }}\n"
        ));
    }
    if let Some(suffix) = rule_value(rules, "endswith") {
        out.push_str(&format!(
            "        if !{expr}.ends_with({suffix:?}) {{\n            roze_rpc::rpc::finish_method(method_guard, \"invalid_argument\");\n            return Err(roze_rpc::rpc::invalid_argument_status(format!(\"{field_label} must end with {{}}\", {suffix:?}), &request_ctx));\n        }}\n"
        ));
    }
    if has_rule(rules, "alpha") {
        out.push_str(&format!(
            "        if !{expr}.chars().all(|ch| ch.is_alphabetic()) {{\n            roze_rpc::rpc::finish_method(method_guard, \"invalid_argument\");\n            return Err(roze_rpc::rpc::invalid_argument_status(\"{field_label} must contain letters only\", &request_ctx));\n        }}\n"
        ));
    }
    if has_rule(rules, "alphanum") {
        out.push_str(&format!(
            "        if !{expr}.chars().all(|ch| ch.is_alphanumeric()) {{\n            roze_rpc::rpc::finish_method(method_guard, \"invalid_argument\");\n            return Err(roze_rpc::rpc::invalid_argument_status(\"{field_label} must contain letters and numbers only\", &request_ctx));\n        }}\n"
        ));
    }
    if has_rule(rules, "ascii") {
        out.push_str(&format!(
            "        if !{expr}.is_ascii() {{\n            roze_rpc::rpc::finish_method(method_guard, \"invalid_argument\");\n            return Err(roze_rpc::rpc::invalid_argument_status(\"{field_label} must contain ASCII characters only\", &request_ctx));\n        }}\n"
        ));
    }
    if has_rule(rules, "code") {
        out.push_str(&format!(
            "        if {expr}.is_empty() || !{expr}.chars().all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.')) {{\n            roze_rpc::rpc::finish_method(method_guard, \"invalid_argument\");\n            return Err(roze_rpc::rpc::invalid_argument_status(\"{field_label} must be a valid code\", &request_ctx));\n        }}\n"
        ));
    }
    if has_rule(rules, "json") {
        out.push_str(&format!(
            "        if serde_json::from_str::<serde_json::Value>(&{expr}).is_err() {{\n            roze_rpc::rpc::finish_method(method_guard, \"invalid_argument\");\n            return Err(roze_rpc::rpc::invalid_argument_status(\"{field_label} must contain valid JSON\", &request_ctx));\n        }}\n"
        ));
    }
    if has_rule(rules, "numeric") {
        out.push_str(&format!(
            "        if {expr}.parse::<f64>().is_err() {{\n            roze_rpc::rpc::finish_method(method_guard, \"invalid_argument\");\n            return Err(roze_rpc::rpc::invalid_argument_status(\"{field_label} must be numeric\", &request_ctx));\n        }}\n"
        ));
    }
    if has_rule(rules, "lowercase") {
        out.push_str(&format!(
            "        if {expr}.chars().any(|ch| ch.is_uppercase()) {{\n            roze_rpc::rpc::finish_method(method_guard, \"invalid_argument\");\n            return Err(roze_rpc::rpc::invalid_argument_status(\"{field_label} must be lowercase\", &request_ctx));\n        }}\n"
        ));
    }
    if has_rule(rules, "uppercase") {
        out.push_str(&format!(
            "        if {expr}.chars().any(|ch| ch.is_lowercase()) {{\n            roze_rpc::rpc::finish_method(method_guard, \"invalid_argument\");\n            return Err(roze_rpc::rpc::invalid_argument_status(\"{field_label} must be uppercase\", &request_ctx));\n        }}\n"
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
    let body = dive_element_body(
        element_rules,
        "item",
        element_ty,
        &format!("field `{field_name}` item"),
        "            ",
    );

    if body.is_empty() {
        String::new()
    } else {
        format!("        for item in &{expr} {{\n{body}        }}\n")
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
            "            ",
        ));
    }
    if let Some(value_rules) = value_rules {
        body.push_str(&dive_element_body(
            &value_rules,
            "value",
            value_ty,
            &format!("field `{field_name}` value"),
            "            ",
        ));
    }

    if body.is_empty() {
        String::new()
    } else {
        format!("        for (key, value) in &{expr} {{\n{body}        }}\n")
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
            "{indent}{{\n{indent}    let value = {var}.to_string();\n{indent}    if ![{allowed}].contains(&value.as_str()) {{\n{indent}        roze_rpc::rpc::finish_method(method_guard, \"invalid_argument\");\n{indent}        return Err(roze_rpc::rpc::invalid_argument_status(format!(\"{field_label} must be one of: {{}}\", {allowed_message:?}), &request_ctx));\n{indent}    }}\n{indent}}}\n"
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
                    "{indent}if {var}.chars().count() != {equal} {{\n{indent}    roze_rpc::rpc::finish_method(method_guard, \"invalid_argument\");\n{indent}    return Err(roze_rpc::rpc::invalid_argument_status(\"{field_label} length is invalid\", &request_ctx));\n{indent}}}\n"
                ));
            } else {
                if let Some(min) = min {
                    body.push_str(&format!(
                        "{indent}if {var}.chars().count() < {min} {{\n{indent}    roze_rpc::rpc::finish_method(method_guard, \"invalid_argument\");\n{indent}    return Err(roze_rpc::rpc::invalid_argument_status(\"{field_label} is too short\", &request_ctx));\n{indent}}}\n"
                    ));
                }
                if let Some(max) = max {
                    body.push_str(&format!(
                        "{indent}if {var}.chars().count() > {max} {{\n{indent}    roze_rpc::rpc::finish_method(method_guard, \"invalid_argument\");\n{indent}    return Err(roze_rpc::rpc::invalid_argument_status(\"{field_label} is too long\", &request_ctx));\n{indent}}}\n"
                    ));
                }
            }
            if has_rule(rules, "alpha") {
                body.push_str(&format!(
                    "{indent}if !{var}.chars().all(|ch| ch.is_alphabetic()) {{\n{indent}    roze_rpc::rpc::finish_method(method_guard, \"invalid_argument\");\n{indent}    return Err(roze_rpc::rpc::invalid_argument_status(\"{field_label} must contain letters only\", &request_ctx));\n{indent}}}\n"
                ));
            }
            if has_rule(rules, "alphanum") {
                body.push_str(&format!(
                    "{indent}if !{var}.chars().all(|ch| ch.is_alphanumeric()) {{\n{indent}    roze_rpc::rpc::finish_method(method_guard, \"invalid_argument\");\n{indent}    return Err(roze_rpc::rpc::invalid_argument_status(\"{field_label} must contain letters and numbers only\", &request_ctx));\n{indent}}}\n"
                ));
            }
            if has_rule(rules, "ascii") {
                body.push_str(&format!(
                    "{indent}if !{var}.is_ascii() {{\n{indent}    roze_rpc::rpc::finish_method(method_guard, \"invalid_argument\");\n{indent}    return Err(roze_rpc::rpc::invalid_argument_status(\"{field_label} must contain ASCII characters only\", &request_ctx));\n{indent}}}\n"
                ));
            }
            if has_rule(rules, "code") {
                body.push_str(&format!(
                    "{indent}if {var}.is_empty() || !{var}.chars().all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.')) {{\n{indent}    roze_rpc::rpc::finish_method(method_guard, \"invalid_argument\");\n{indent}    return Err(roze_rpc::rpc::invalid_argument_status(\"{field_label} must be a valid code\", &request_ctx));\n{indent}}}\n"
                ));
            }
            if has_rule(rules, "json") {
                body.push_str(&format!(
                    "{indent}if serde_json::from_str::<serde_json::Value>({var}).is_err() {{\n{indent}    roze_rpc::rpc::finish_method(method_guard, \"invalid_argument\");\n{indent}    return Err(roze_rpc::rpc::invalid_argument_status(\"{field_label} must contain valid JSON\", &request_ctx));\n{indent}}}\n"
                ));
            }
            if has_rule(rules, "numeric") {
                body.push_str(&format!(
                    "{indent}if {var}.parse::<f64>().is_err() {{\n{indent}    roze_rpc::rpc::finish_method(method_guard, \"invalid_argument\");\n{indent}    return Err(roze_rpc::rpc::invalid_argument_status(\"{field_label} must be numeric\", &request_ctx));\n{indent}}}\n"
                ));
            }
            if has_rule(rules, "lowercase") {
                body.push_str(&format!(
                    "{indent}if {var}.chars().any(|ch| ch.is_uppercase()) {{\n{indent}    roze_rpc::rpc::finish_method(method_guard, \"invalid_argument\");\n{indent}    return Err(roze_rpc::rpc::invalid_argument_status(\"{field_label} must be lowercase\", &request_ctx));\n{indent}}}\n"
                ));
            }
            if has_rule(rules, "uppercase") {
                body.push_str(&format!(
                    "{indent}if {var}.chars().any(|ch| ch.is_lowercase()) {{\n{indent}    roze_rpc::rpc::finish_method(method_guard, \"invalid_argument\");\n{indent}    return Err(roze_rpc::rpc::invalid_argument_status(\"{field_label} must be uppercase\", &request_ctx));\n{indent}}}\n"
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
            "{indent}if {expr} < &0 {{\n{indent}    roze_rpc::rpc::finish_method(method_guard, \"invalid_argument\");\n{indent}    return Err(roze_rpc::rpc::invalid_argument_status(\"{field_label} must be non-negative\", &request_ctx));\n{indent}}}\n"
        ));
    }
    if has_rule(rules, "page") || has_rule(rules, "limit") {
        out.push_str(&format!(
            "{indent}if {expr} < &1 {{\n{indent}    roze_rpc::rpc::finish_method(method_guard, \"invalid_argument\");\n{indent}    return Err(roze_rpc::rpc::invalid_argument_status(\"{field_label} must be at least 1\", &request_ctx));\n{indent}}}\n"
        ));
    }
    if has_rule(rules, "limit") {
        out.push_str(&format!(
            "{indent}if {expr} > &1000 {{\n{indent}    roze_rpc::rpc::finish_method(method_guard, \"invalid_argument\");\n{indent}    return Err(roze_rpc::rpc::invalid_argument_status(\"{field_label} must not exceed 1000\", &request_ctx));\n{indent}}}\n"
        ));
    }
    if let Some(min) = rule_value(rules, "min")
        .or_else(|| rule_value(rules, "gte"))
        .filter(|value| is_number_literal(value))
    {
        out.push_str(&format!(
            "{indent}if {expr} < &{min} {{\n{indent}    roze_rpc::rpc::finish_method(method_guard, \"invalid_argument\");\n{indent}    return Err(roze_rpc::rpc::invalid_argument_status(\"{field_label} is too small\", &request_ctx));\n{indent}}}\n"
        ));
    }
    if let Some(max) = rule_value(rules, "max")
        .or_else(|| rule_value(rules, "lte"))
        .filter(|value| is_number_literal(value))
    {
        out.push_str(&format!(
            "{indent}if {expr} > &{max} {{\n{indent}    roze_rpc::rpc::finish_method(method_guard, \"invalid_argument\");\n{indent}    return Err(roze_rpc::rpc::invalid_argument_status(\"{field_label} is too large\", &request_ctx));\n{indent}}}\n"
        ));
    }
    if let Some(min) = rule_value(rules, "gt").filter(|value| is_number_literal(value)) {
        out.push_str(&format!(
            "{indent}if {expr} <= &{min} {{\n{indent}    roze_rpc::rpc::finish_method(method_guard, \"invalid_argument\");\n{indent}    return Err(roze_rpc::rpc::invalid_argument_status(\"{field_label} is too small\", &request_ctx));\n{indent}}}\n"
        ));
    }
    if let Some(max) = rule_value(rules, "lt").filter(|value| is_number_literal(value)) {
        out.push_str(&format!(
            "{indent}if {expr} >= &{max} {{\n{indent}    roze_rpc::rpc::finish_method(method_guard, \"invalid_argument\");\n{indent}    return Err(roze_rpc::rpc::invalid_argument_status(\"{field_label} is too large\", &request_ctx));\n{indent}}}\n"
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
            "        if !({expr} {op} {other}) {{\n            roze_rpc::rpc::finish_method(method_guard, \"invalid_argument\");\n            return Err(roze_rpc::rpc::invalid_argument_status(\"field `{field_name}` {message} field `{other_name}`\", &request_ctx));\n        }}\n"
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
                "        if ({}) && {expr}.is_empty() {{\n            roze_rpc::rpc::finish_method(method_guard, \"invalid_argument\");\n            return Err(roze_rpc::rpc::invalid_argument_status(\"field `{field_name}` is required\", &request_ctx));\n        }}\n",
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
                "        if !({}) && {expr}.is_empty() {{\n            roze_rpc::rpc::finish_method(method_guard, \"invalid_argument\");\n            return Err(roze_rpc::rpc::invalid_argument_status(\"field `{field_name}` is required\", &request_ctx));\n        }}\n",
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
                "        if ({}) && {expr}.is_empty() {{\n            roze_rpc::rpc::finish_method(method_guard, \"invalid_argument\");\n            return Err(roze_rpc::rpc::invalid_argument_status(\"field `{field_name}` is required\", &request_ctx));\n        }}\n",
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
                "        if ({}) && {expr}.is_empty() {{\n            roze_rpc::rpc::finish_method(method_guard, \"invalid_argument\");\n            return Err(roze_rpc::rpc::invalid_argument_status(\"field `{field_name}` is required\", &request_ctx));\n        }}\n",
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

fn field_wire_name(field: &Field) -> String {
    field
        .wire_name
        .as_deref()
        .or(field.json_name.as_deref())
        .map(ToString::to_string)
        .unwrap_or_else(|| rust_field_name(field))
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
        "int" => "i64".to_string(),
        "uint" => "u64".to_string(),
        "bool" => "bool".to_string(),
        other => other.to_string(),
    }
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
    (!values.is_empty()).then_some(values)
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
    rules
        .split(',')
        .map(str::trim)
        .filter(|rule| !rule.is_empty())
        .any(|rule| rule == name)
}

fn parse_usize(value: &str) -> Option<usize> {
    value.parse::<usize>().ok()
}

fn is_number_literal(value: &str) -> bool {
    let value = value.trim();
    !value.is_empty()
        && value
            .chars()
            .enumerate()
            .all(|(idx, ch)| ch.is_ascii_digit() || ch == '.' || (idx == 0 && ch == '-'))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_api;

    #[test]
    fn client_supports_config_based_connection_and_auth() {
        let spec = parse_api(
            r#"
            service user {
                @handler getUserRoute
                get /users/:id (GetUserReq) returns (GetUserResp)

                rpc GetUser (GetUserReq) returns (GetUserResp)
                rpc CreateUser (CreateUserReq) returns (GetUserResp)
            }

            type GetUserReq {
                id: u64
            }

            type CreateUserReq {
                name: string
            }

            type GetUserResp {
                name: string
            }
            "#,
        )
        .expect("api");

        let client = render_client(&spec);
        assert!(client.contains("client_config: Option<roze_config::RpcClientConfig>"));
        assert!(client.contains("governance: Option<roze_config::GovernanceConfig>"));
        assert!(client.contains("pub fn with_governance"));
        assert!(client.contains("pub async fn connect_from_config"));
        assert!(client.contains("RpcClientOptions::from_config(&config)"));
        assert!(client.contains("roze_rpc::rpc::client_request("));
        assert!(client.contains("roze_rpc::rpc::retry_status_for_method("));
        assert!(client.contains("retry_status_for_method(\n            \"user\","));
        assert!(client.contains("\"user\",\n            &context,"));
        assert!(client.contains("governance.as_ref()"));
        assert!(client.contains("\"GetUser\""));
        assert!(client.contains("let request_template = req;"));
        assert!(client.contains("client_request(request_template, &context"));
        assert!(client.contains("client_request(request_template.clone(), &context"));
    }

    #[test]
    fn rpc_main_uses_service_group_lifecycle() {
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

        let rendered = render_main(&spec);
        assert!(rendered.contains("use roze_service::ServiceGroup;"));
        assert!(rendered.contains("let health = ctx.health.clone();"));
        assert!(rendered.contains("RpcHealthReporter::new_for"));
        assert!(rendered.contains("rpc_health.refresh().await;"));
        assert!(rendered.contains("group.add_fn(service_name"));
        assert!(rendered.contains("let mut builder = RpcServer::new(rpc_addr).builder();"));
        assert!(rendered.contains(".add_service(grpc_health_service)"));
        assert!(rendered.contains(".serve_with_shutdown(rpc_addr"));
        assert!(rendered.contains("group.add_fn(\"grpc-health-sync\""));
        assert!(rendered.contains("run_until(std::time::Duration::from_secs(1)"));
        assert!(rendered.contains("let result = group.start().await;"));
        assert!(rendered.contains("result?;"));
    }

    #[test]
    fn rpc_proto_references_use_prost_pascal_case_types() {
        let spec = parse_api(
            r#"
            service transformer {
                rpc expand (expandReq) returns (expandResp)
            }

            type expandReq {
                shorten: string
            }

            type expandResp {
                url: string
            }
            "#,
        )
        .expect("api");

        let client = render_client(&spec);
        assert!(client.contains("req: proto::ExpandReq"));
        assert!(client.contains("Result<proto::ExpandResp, Status>"));
        assert!(!client.contains("proto::expandReq"));
        assert!(!client.contains("proto::expandResp"));

        let server = render_rpc(&spec);
        assert!(server.contains("#[derive(Clone)]"));
        assert!(!server.contains("#[derive(Debug, Clone)]"));
        assert!(server.contains("Request<proto::ExpandReq>"));
        assert!(server.contains("Response<proto::ExpandResp>"));
        assert!(server.contains("Response::new(proto::ExpandResp"));
        assert!(!server.contains("proto::expandReq"));
        assert!(!server.contains("proto::expandResp"));
    }

    #[test]
    fn rpc_server_validates_request_before_logic() {
        let spec = parse_api(
            r#"
            service user {
                rpc GetUser (GetUserReq) returns (GetUserResp)
            }

            type GetUserReq {
                id: u64 `validate:"gte=1"`
                status: string `validate:"oneof=active disabled"`
                account: string
                backup: string `validate:"required_with=account"`
                lower_code: string `validate:"lowercase"`
                upper_code: string `validate:"uppercase"`
                resource_code: string `validate:"code"`
                json_config: string `validate:"json"`
                offset: int `validate:"nonnegative"`
                page: int `validate:"page"`
                limit: int `validate:"limit"`
                tags: []string `validate:"min=1,dive,required,min=2,alphanum"`
                codes: []string `validate:"min_items=1,max_items=3,dive,code"`
            }

            type GetUserResp {
                name: string
            }
            "#,
        )
        .expect("api");

        let rendered = render_rpc(&spec);
        let rendered_types = crate::generator::types::render_types(&spec.types);

        assert!(rendered.contains("roze_validation::validate_or_message_i18n(&req"));
        assert!(rendered.contains("roze_rpc::rpc::invalid_argument_status(message, &request_ctx)"));
        assert!(rendered.contains("finish_method(method_guard, \"invalid_argument\")"));
        assert!(rendered.contains("roze_rpc::rpc::apply_fallback("));
        assert!(rendered.contains("self.ctx.config.name.as_str(),\n                    err,"));
        assert!(rendered.contains(
            "roze_rpc::rpc::method_fallback(Some(&self.ctx.config.governance), \"GetUser\")"
        ));
        assert!(rendered.contains("finish_method(method_guard, err.kind())"));
        assert!(rendered.contains("if ![\"active\", \"disabled\"].contains(&value.as_str())"));
        assert!(
            rendered.contains("if (!req.account.to_string().is_empty()) && req.backup.is_empty()")
        );
        assert!(rendered.contains("if req.lower_code.chars().any(|ch| ch.is_uppercase())"));
        assert!(rendered.contains("if req.upper_code.chars().any(|ch| ch.is_lowercase())"));
        assert!(rendered.contains("if req.resource_code.is_empty()"));
        assert!(rendered
            .contains("serde_json::from_str::<serde_json::Value>(&req.json_config).is_err()"));
        assert!(rendered_types.contains("#[validate(range(min = 0))]"));
        assert!(rendered_types.contains("#[validate(range(min = 1))]"));
        assert!(rendered_types.contains("#[validate(range(min = 1, max = 1000))]"));
        assert!(rendered_types.contains("#[validate(length(min = 1, max = 3))]"));
        assert!(rendered.contains("for item in &req.codes"));
        assert!(rendered.contains("for item in &req.tags"));
        assert!(rendered.contains("if item.chars().count() < 2"));
        assert!(rendered.contains("if !item.chars().all(|ch| ch.is_alphanumeric())"));
    }

    #[test]
    fn rpc_generation_enforces_declared_permissions_and_exposes_auth_helpers() {
        let spec = parse_api(
            r#"
            service user {
                @permission users:write
                rpc CreateUser (CreateUserReq) returns (UserResp)
            }

            type CreateUserReq {
            }
            type UserResp {
            }
            "#,
        )
        .expect("valid api");

        let server = render_rpc(&spec);
        assert!(
            server.contains("roze_rpc::rpc::enforce_permissions(&request_ctx, &[\"users:write\"])")
        );
        assert!(server.contains("finish_method(method_guard, \"permission_denied\")"));

        let logic_mod = render_logic_mod(&spec);
        assert!(logic_mod.contains("pub fn current_user_id"));
        assert!(logic_mod.contains("pub fn current_permissions"));
    }

    #[test]
    fn rpc_generation_wires_idempotency_middleware() {
        let spec = parse_api(
            r#"
            service order {
                @middleware idempotency
                rpc CreateOrder (CreateOrderReq) returns (CreateOrderResp)
            }

            type CreateOrderReq {
                sku string
            }

            type CreateOrderResp {
                id string
            }
            "#,
        )
        .expect("api");

        let server = render_rpc(&spec);
        assert!(server.contains("request.metadata().get(\"idempotency-key\")"));
        assert!(server.contains("IDEMPOTENCY_MISSING_KEY"));
        assert!(server.contains("idempotency_fingerprint(&req)"));
        assert!(server.contains("begin_idempotency(self.ctx.idempotency.as_ref()"));
        assert!(server.contains("roze_middleware::IdempotencyDecision::Replay(value)"));
        assert!(server.contains("roze_middleware::IdempotencyDecision::Conflict"));
        assert!(server.contains("IDEMPOTENCY_IN_FLIGHT"));
        assert!(server.contains("IDEMPOTENCY_KEY_REUSED"));
        assert!(server.contains("complete_idempotency(self.ctx.idempotency.as_ref()"));
        assert!(server.contains("fail_idempotency(self.ctx.idempotency.as_ref()"));
    }

    #[test]
    fn rpc_converts_keyword_and_repeated_message_fields() {
        let spec = parse_api(
            r#"
            service user {
                rpc ListUsers (ListUsersReq) returns (ListUsersResp)
            }

            type UserItem {
                id: u64
                type: string
            }

            type ListUsersReq {
                items: []UserItem
            }

            type ListUsersResp {
                users: []UserItem
            }
            "#,
        )
        .expect("api");

        let rendered = render_rpc(&spec);

        assert!(rendered.contains("r#type: item.r#type"));
        assert!(rendered.contains(
            "items: req.items.into_iter().map(|item| UserItem { id: item.id, r#type: item.r#type }).collect()"
        ));
        assert!(rendered.contains(
            "users: resp.users.into_iter().map(|item| proto::UserItem { id: item.id, r#type: item.r#type }).collect()"
        ));
    }

    #[test]
    fn rpc_wraps_singular_nested_message_fields_for_prost() {
        let spec = parse_api(
            r#"
            service user {
                rpc Login (LoginRequest) returns (LoginResponse)
            }

            type User {
                id: u64
                name: string
            }

            type LoginRequest {
                user: User
            }

            type LoginResponse {
                token: string
                user: User
            }
            "#,
        )
        .expect("api");

        let rendered = render_rpc(&spec);

        assert!(rendered.contains(
            "user: req.user.map(|value| User { id: value.id, name: value.name }).unwrap_or_default()"
        ));
        assert!(
            rendered.contains("user: Some(proto::User { id: resp.user.id, name: resp.user.name })")
        );
    }

    #[test]
    fn rpc_logic_stubs_default_response_fields() {
        let spec = parse_api(
            r#"
            service system {
                rpc ListPermissions (ListPermissionsRequest) returns (ListPermissionsResponse)
            }

            type ListPermissionsRequest {
            }

            type ListPermissionsResponse {
                permissions: []string
                today_order_count: i64
                today_pay_amount: string
                enabled: bool
            }
            "#,
        )
        .expect("api");

        let files = render_logic_files(&spec);
        let logic = &files[0].1;
        assert!(logic.contains("Ok(ListPermissionsResponse::default())"));
        assert!(!logic.contains("Ok(ListPermissionsResponse {"));
    }

    #[test]
    fn rpc_empty_request_skips_unused_protobuf_binding() {
        let spec = parse_api(
            r#"
            service access {
                rpc ListRoles (ListRolesRequest) returns (ListRolesResponse)
            }

            type ListRolesRequest {
            }

            type ListRolesResponse {
                roles: []string
            }
            "#,
        )
        .expect("api");

        let rendered = render_rpc(&spec);
        assert!(rendered.contains("let req = ListRolesRequest {  };"));
        assert!(!rendered.contains("let req = request.into_inner();"));
    }

    #[test]
    fn rpc_non_empty_request_still_consumes_protobuf_request() {
        let spec = parse_api(
            r#"
            service access {
                rpc GetRole (GetRoleRequest) returns (GetRoleResponse)
            }

            type GetRoleRequest {
                id: u64
            }

            type GetRoleResponse {
                name: string
            }
            "#,
        )
        .expect("api");

        let rendered = render_rpc(&spec);
        assert!(rendered.contains("let req = request.into_inner();"));
        assert!(rendered.contains("let req = GetRoleRequest { id: req.id };"));
    }
}
