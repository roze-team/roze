use std::{
    collections::HashMap, convert::Infallible, future::Future, pin::Pin, sync::Arc, time::Instant,
};

use http::{header, StatusCode};
use http_body_util::BodyExt;
use roze_config::{GatewayConfig, GatewayRoute, GatewayService, GovernanceConfig};
use roze_http::rest::{self, HttpResponse, IncomingRequest};
use roze_jwt::JwtConfig;
use roze_rpc::registry::Registry;

#[derive(Clone)]
pub struct GatewayServiceRuntime {
    runtime: Arc<GatewayRuntime>,
}

#[derive(Clone)]
struct GatewayRuntime {
    routes: Vec<GatewayRoute>,
    services: HashMap<String, GatewayService>,
    client: reqwest::Client,
    request_body_limit_bytes: usize,
}

impl tower::Service<IncomingRequest> for GatewayServiceRuntime {
    type Response = HttpResponse;
    type Error = Infallible;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(
        &mut self,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        std::task::Poll::Ready(Ok(()))
    }

    fn call(&mut self, request: IncomingRequest) -> Self::Future {
        let runtime = self.runtime.clone();
        Box::pin(async move { Ok(runtime.handle(request).await) })
    }
}

pub fn build_router(config: GatewayConfig, jwt: Option<JwtConfig>) -> GatewayServiceRuntime {
    build_router_with_registry(config, jwt, None)
}

pub fn build_router_with_registry(
    config: GatewayConfig,
    jwt: Option<JwtConfig>,
    registry: Option<Arc<dyn Registry>>,
) -> GatewayServiceRuntime {
    build_router_with_registry_and_governance(config, jwt, registry, None)
}

pub fn build_router_with_registry_and_governance(
    config: GatewayConfig,
    jwt: Option<JwtConfig>,
    registry: Option<Arc<dyn Registry>>,
    governance: Option<GovernanceConfig>,
) -> GatewayServiceRuntime {
    build_router_with_registry_governance_and_auth(config, jwt, None, registry, governance)
}

pub fn build_router_with_registry_governance_and_auth(
    config: GatewayConfig,
    _jwt: Option<JwtConfig>,
    _api_keys: Option<roze_auth::ApiKeyConfig>,
    _registry: Option<Arc<dyn Registry>>,
    _governance: Option<GovernanceConfig>,
) -> GatewayServiceRuntime {
    let services = config
        .services
        .into_iter()
        .map(|service| (service.name.clone(), service))
        .collect();
    GatewayServiceRuntime {
        runtime: Arc::new(GatewayRuntime {
            routes: config.routes,
            services,
            client: reqwest::Client::new(),
            request_body_limit_bytes: config.request_body_limit_bytes.unwrap_or(2 * 1024 * 1024),
        }),
    }
}

impl GatewayRuntime {
    async fn handle(&self, request: IncomingRequest) -> HttpResponse {
        let started = Instant::now();
        let method = request.method().clone();
        let path = request.uri().path().to_string();

        let Some(route) = self.select_route(&path) else {
            return rest::text_response(StatusCode::NOT_FOUND, "gateway route not found");
        };
        let Some(service) = self.services.get(&route.service) else {
            return rest::text_response(StatusCode::BAD_GATEWAY, "gateway service not found");
        };

        let upstream = join_url(&service.upstream, route.rewrite.as_deref().unwrap_or(&path));
        let headers = request
            .headers()
            .iter()
            .filter_map(|(name, value)| {
                Some((name.as_str().to_string(), value.to_str().ok()?.to_string()))
            })
            .collect::<Vec<_>>();
        let query = request.uri().query().map(str::to_string);
        let body = match request.into_body().collect().await {
            Ok(collected) => {
                let body = collected.to_bytes();
                if body.len() > self.request_body_limit_bytes {
                    return rest::text_response(
                        StatusCode::PAYLOAD_TOO_LARGE,
                        "request body too large",
                    );
                }
                body
            }
            Err(error) => {
                return rest::text_response(
                    StatusCode::BAD_REQUEST,
                    format!("failed to read request body: {error}"),
                );
            }
        };

        let mut builder = self.client.request(method.clone(), upstream);
        for (name, value) in headers {
            if !name.eq_ignore_ascii_case(header::HOST.as_str()) {
                builder = builder.header(name, value);
            }
        }
        if let Some(query) = query {
            builder = builder.query(&query_pairs(&query));
        }

        let response = match builder.body(body).send().await {
            Ok(response) => response,
            Err(error) => {
                tracing::warn!(error = %error, "gateway upstream request failed");
                return rest::text_response(StatusCode::BAD_GATEWAY, error.to_string());
            }
        };

        let status =
            StatusCode::from_u16(response.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
        let mut out = http::Response::builder().status(status);
        for (name, value) in response.headers() {
            out = out.header(name, value);
        }
        let bytes = match response.bytes().await {
            Ok(bytes) => bytes,
            Err(error) => {
                return rest::text_response(
                    StatusCode::BAD_GATEWAY,
                    format!("failed to read upstream response: {error}"),
                );
            }
        };
        roze_metrics::record_http_request(status.is_success(), started.elapsed());
        out.body(rest::full_body(bytes)).expect("gateway response")
    }

    fn select_route(&self, path: &str) -> Option<&GatewayRoute> {
        self.routes
            .iter()
            .filter(|route| path.starts_with(&route.path))
            .max_by_key(|route| route.path.len())
    }
}

fn join_url(base: &str, path: &str) -> String {
    format!(
        "{}/{}",
        base.trim_end_matches('/'),
        path.trim_start_matches('/')
    )
}

fn query_pairs(query: &str) -> Vec<(&str, &str)> {
    query
        .split('&')
        .filter_map(|part| part.split_once('='))
        .collect()
}
