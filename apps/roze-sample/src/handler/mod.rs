use std::convert::Infallible;

use http::StatusCode;
use roze_http::rest::{self, IncomingRequest};
use tower::{util::BoxCloneService, ServiceExt};

use crate::{openapi, svc::ServiceContext};

pub fn router(
    _ctx: ServiceContext,
) -> BoxCloneService<IncomingRequest, rest::HttpResponse, Infallible> {
    tower::service_fn(|request: IncomingRequest| async move {
        let response = match request.uri().path() {
            "/healthz" => rest::api_response(&roze_result::ApiResponse::ok("ok")),
            "/metrics" => rest::text_response(StatusCode::OK, roze_metrics::http_metrics()),
            "/openapi.json" => rest::json_response(StatusCode::OK, &openapi::document()),
            _ => rest::text_response(
                StatusCode::NOT_FOUND,
                "route not migrated to Roze native HTTP",
            ),
        };
        Ok::<_, Infallible>(response)
    })
    .boxed_clone()
}
