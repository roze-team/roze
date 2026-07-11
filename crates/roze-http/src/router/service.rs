use std::{
    convert::Infallible,
    fmt,
    task::{Context, Poll},
};

use tower::Service;

use crate::rest::{HttpResponse, IncomingRequest};

use super::{RouteFuture, Router};

#[must_use = "service adapters do nothing unless used"]
pub struct RouterAsService<'a> {
    router: &'a mut Router,
}

impl<'a> RouterAsService<'a> {
    pub(super) fn new(router: &'a mut Router) -> Self {
        Self { router }
    }
}

impl Service<IncomingRequest> for RouterAsService<'_> {
    type Response = HttpResponse;
    type Error = Infallible;
    type Future = RouteFuture;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.router.poll_ready(cx)
    }

    fn call(&mut self, request: IncomingRequest) -> Self::Future {
        self.router.call(request)
    }
}

impl fmt::Debug for RouterAsService<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RouterAsService").finish_non_exhaustive()
    }
}

#[derive(Clone)]
#[must_use = "service adapters do nothing unless used"]
pub struct RouterIntoService {
    router: Router,
}

impl RouterIntoService {
    pub(super) fn new(router: Router) -> Self {
        Self { router }
    }
}

impl Service<IncomingRequest> for RouterIntoService {
    type Response = HttpResponse;
    type Error = Infallible;
    type Future = RouteFuture;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.router.poll_ready(cx)
    }

    fn call(&mut self, request: IncomingRequest) -> Self::Future {
        self.router.call(request)
    }
}

impl fmt::Debug for RouterIntoService {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RouterIntoService").finish_non_exhaustive()
    }
}
