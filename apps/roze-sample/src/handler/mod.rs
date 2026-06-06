#![allow(unused_imports)]

use poem::{handler, http::HeaderMap, web::{Data, Form, Json, Path, Query}, Endpoint, EndpointExt, Route};
use serde::Deserialize;
use roze_validation::Validate;
use roze_context::Context;
use roze_error::RozeError;
use roze_result::ApiResponse;

use crate::openapi;
use crate::svc::ServiceContext;
use crate::types::*;

pub fn router(ctx: ServiceContext) -> impl Endpoint {
    Route::new()
        .at("/api/healthz", poem::get(health))
        .at("/api/metrics", poem::get(metrics))
        .at("/api/openapi.json", poem::get(openapi_doc))
        .at("/api/roze_sample/login", poem::post(post_roze_sample_login))
        .data(ctx)
}

#[handler]
async fn health() -> Result<Json<ApiResponse<&'static str>>, RozeError> {
    Ok(Json(ApiResponse::ok("ok")))
}

#[handler]
async fn metrics() -> String {
    roze_metrics::http_metrics()
}

#[handler]
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

#[handler]
async fn post_roze_sample_login(Data(ctx): Data<&ServiceContext>, Data(request_ctx): Data<&Context>, Json(body): Json<LoginReqJson>) -> Result<Json<ApiResponse<LoginResp>>, RozeError> {
    roze_validation::validate_or_message(&body).map_err(RozeError::BadRequest)?;
    let req = LoginReq {
        username: body.username,
        password: body.password,
    };
    let resp = crate::logic::post_roze_sample_login((*ctx).clone(), (*request_ctx).clone(), req).await?;
    Ok(Json(ApiResponse::ok(resp)))
}

