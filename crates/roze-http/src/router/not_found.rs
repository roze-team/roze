use std::{
    convert::Infallible,
    future::{ready, Ready},
    task::{Context, Poll},
};

use http::StatusCode;
use tower::Service;

use crate::rest::{self, HttpResponse, IncomingRequest};

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct NotFound;

impl Service<IncomingRequest> for NotFound {
    type Response = HttpResponse;
    type Error = Infallible;
    type Future = Ready<Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, _request: IncomingRequest) -> Self::Future {
        ready(Ok(rest::text_response(StatusCode::NOT_FOUND, "not found")))
    }
}
