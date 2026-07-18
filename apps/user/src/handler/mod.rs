use roze_http::{routing::get, Json, Router};

use crate::{openapi, svc::ServiceContext};

pub fn router(_ctx: ServiceContext) -> Router {
    Router::new()
        .route("/healthz", get(health))
        .route("/metrics", get(metrics))
        .route("/openapi.json", get(openapi_doc))
}

async fn health() -> roze_result::ApiResponse<&'static str> {
    roze_result::ApiResponse::ok("ok")
}

async fn metrics() -> String {
    roze_metrics::http_metrics()
}

async fn openapi_doc() -> Json<serde_json::Value> {
    Json(openapi::document())
}
