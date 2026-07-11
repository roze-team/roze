use std::{
    convert::Infallible,
    fmt,
    future::{ready, Ready},
    marker::PhantomData,
    task::{Context, Poll},
};

use tower::Service;

use crate::extract::ConnectInfoService;

use super::{MethodRouter, Router};

#[derive(Clone)]
#[must_use = "make services do nothing unless used"]
pub struct IntoMakeService {
    router: Router,
}

impl IntoMakeService {
    pub(super) fn new(router: Router) -> Self {
        Self { router }
    }
}

impl<Target> Service<Target> for IntoMakeService {
    type Response = Router;
    type Error = Infallible;
    type Future = Ready<Result<Self::Response, Infallible>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, _target: Target) -> Self::Future {
        ready(Ok(self.router.clone()))
    }
}

impl fmt::Debug for IntoMakeService {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IntoMakeService").finish_non_exhaustive()
    }
}

#[derive(Clone)]
#[must_use = "make services do nothing unless used"]
pub struct IntoMakeServiceWithConnectInfo<T> {
    router: Router,
    _marker: PhantomData<fn() -> T>,
}

impl<T> IntoMakeServiceWithConnectInfo<T> {
    pub(super) fn new(router: Router) -> Self {
        Self {
            router,
            _marker: PhantomData,
        }
    }
}

impl<T, Target> Service<Target> for IntoMakeServiceWithConnectInfo<T>
where
    T: Clone + Send + Sync + 'static,
    Target: Into<T>,
{
    type Response = ConnectInfoService<Router, T>;
    type Error = Infallible;
    type Future = Ready<Result<Self::Response, Infallible>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, target: Target) -> Self::Future {
        ready(Ok(ConnectInfoService::new(
            self.router.clone(),
            target.into(),
        )))
    }
}

impl<T> fmt::Debug for IntoMakeServiceWithConnectInfo<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IntoMakeServiceWithConnectInfo")
            .finish_non_exhaustive()
    }
}

#[derive(Clone)]
#[must_use = "make services do nothing unless used"]
pub struct MethodRouterIntoMakeServiceWithConnectInfo<T> {
    method_router: MethodRouter,
    _marker: PhantomData<fn() -> T>,
}

impl<T> MethodRouterIntoMakeServiceWithConnectInfo<T> {
    pub(super) fn new(method_router: MethodRouter) -> Self {
        Self {
            method_router,
            _marker: PhantomData,
        }
    }
}

impl<T, Target> Service<Target> for MethodRouterIntoMakeServiceWithConnectInfo<T>
where
    T: Clone + Send + Sync + 'static,
    Target: Into<T>,
{
    type Response = ConnectInfoService<MethodRouter, T>;
    type Error = Infallible;
    type Future = Ready<Result<Self::Response, Infallible>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, target: Target) -> Self::Future {
        ready(Ok(ConnectInfoService::new(
            self.method_router.clone(),
            target.into(),
        )))
    }
}

impl<T> fmt::Debug for MethodRouterIntoMakeServiceWithConnectInfo<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MethodRouterIntoMakeServiceWithConnectInfo")
            .finish_non_exhaustive()
    }
}
