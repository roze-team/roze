#![allow(unused_imports)]

use axum::{
    extract::{Extension, Form, Path, Query, State},
    http::HeaderMap,
    routing::{get, post},
    Json, Router,
};
use roze_context::Context;
use roze_error::RozeError;
use roze_result::ApiResponse;
use roze_validation::Validate;
use serde::Deserialize;

use crate::openapi;
use crate::svc::ServiceContext;
use crate::types::*;

pub fn router(ctx: ServiceContext) -> Router {
    Router::new()
        .route("/api/healthz", get(health))
        .route("/api/metrics", get(metrics))
        .route("/api/openapi.json", get(openapi_doc))
        .route("/api/user/login", post(post_user_login))
        .with_state(ctx)
}

async fn health() -> Result<ApiResponse<&'static str>, RozeError> {
    Ok(ApiResponse::ok("ok"))
}

async fn metrics() -> String {
    roze_metrics::http_metrics()
}

async fn openapi_doc() -> Json<serde_json::Value> {
    Json(openapi::document())
}

#[derive(Debug, Clone, Deserialize, Validate)]
struct LoginReqJson {
    #[validate(length(min = 1))]
    username: String,
    #[validate(length(min = 1))]
    password: String,
}

async fn post_user_login(
    State(ctx): State<ServiceContext>,
    Extension(request_ctx): Extension<Context>,
    Json(body): Json<LoginReqJson>,
) -> Result<ApiResponse<LoginResp>, RozeError> {
    roze_validation::validate_or_message(&body).map_err(RozeError::BadRequest)?;
    let req = LoginReq {
        username: body.username,
        password: body.password,
    };
    let resp = crate::logic::post_user_login(ctx, request_ctx, req).await?;
    Ok(ApiResponse::ok(resp))
}
