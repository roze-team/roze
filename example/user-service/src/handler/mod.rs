use poem::{
    handler,
    web::{Data, Json},
    Endpoint, EndpointExt, Route,
};
use roze_core::rest::{ApiResponse, AppError};

use crate::svc::ServiceContext;
use crate::types::*;

pub fn router(ctx: ServiceContext) -> impl Endpoint {
    Route::new()
        .at("/healthz", poem::get(health))
        .at("/user/login", poem::post(login))
        .data(ctx)
}

#[handler]
async fn health() -> Result<Json<ApiResponse<&'static str>>, AppError> {
    Ok(Json(ApiResponse::ok("ok")))
}

#[handler]
async fn login(
    Data(ctx): Data<&ServiceContext>,
    Json(req): Json<LoginReq>,
) -> Result<Json<ApiResponse<LoginResp>>, AppError> {
    let resp = crate::logic::login((*ctx).clone(), req).await?;
    Ok(Json(ApiResponse::ok(resp)))
}
