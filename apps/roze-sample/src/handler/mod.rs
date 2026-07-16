use roze_http::{routing::get, Json, Router};

use crate::{openapi, svc::ServiceContext};

pub fn router(ctx: ServiceContext) -> Router {
    Router::new()
        .route("/healthz", get(health))
        .route("/metrics", get(metrics))
        .route("/openapi.json", get(openapi_doc))
        .with_state(ctx)
}

async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "status": "ok" }))
}

async fn metrics() -> String {
    roze_metrics::http_metrics()
}

async fn openapi_doc() -> Json<serde_json::Value> {
    Json(openapi::document())
}
