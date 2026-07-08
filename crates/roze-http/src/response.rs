use std::convert::Infallible;

use http::StatusCode;
use serde::Serialize;

use crate::rest::{self, HttpResponse};

pub trait IntoResponse {
    fn into_response(self) -> HttpResponse;
}

impl IntoResponse for HttpResponse {
    fn into_response(self) -> HttpResponse {
        self
    }
}

impl IntoResponse for StatusCode {
    fn into_response(self) -> HttpResponse {
        rest::empty_response(self)
    }
}

impl IntoResponse for &'static str {
    fn into_response(self) -> HttpResponse {
        rest::text_response(StatusCode::OK, self)
    }
}

impl IntoResponse for String {
    fn into_response(self) -> HttpResponse {
        rest::text_response(StatusCode::OK, self)
    }
}

impl<T> IntoResponse for roze_result::ApiResponse<T>
where
    T: Serialize,
{
    fn into_response(self) -> HttpResponse {
        rest::api_response(&self)
    }
}

impl IntoResponse for roze_error::RozeError {
    fn into_response(self) -> HttpResponse {
        rest::error_response(&self)
    }
}

impl<T, E> IntoResponse for Result<T, E>
where
    T: IntoResponse,
    E: IntoResponse,
{
    fn into_response(self) -> HttpResponse {
        match self {
            Ok(value) => value.into_response(),
            Err(error) => error.into_response(),
        }
    }
}

impl IntoResponse for Infallible {
    fn into_response(self) -> HttpResponse {
        match self {}
    }
}

pub struct Json<T>(pub T);

impl<T> IntoResponse for Json<T>
where
    T: Serialize,
{
    fn into_response(self) -> HttpResponse {
        rest::json_response(StatusCode::OK, &self.0)
    }
}
