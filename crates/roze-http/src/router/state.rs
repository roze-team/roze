use std::{
    convert::Infallible,
    future::Future,
    marker::PhantomData,
    pin::Pin,
    task::{Context, Poll},
};

use tower::{util::BoxCloneSyncService, Layer, Service};

use crate::{
    extract::FromRef,
    rest::{HttpResponse, IncomingRequest},
};

type BoxFuture = Pin<Box<dyn Future<Output = Result<HttpResponse, Infallible>> + Send>>;

#[derive(Clone)]
pub(super) struct StateLayer<T> {
    state: T,
}

impl<T> StateLayer<T> {
    pub(super) fn new(state: T) -> Self {
        Self { state }
    }
}

impl<T> Layer<BoxCloneSyncService<IncomingRequest, HttpResponse, Infallible>> for StateLayer<T>
where
    T: Clone + Send + Sync + 'static,
{
    type Service = StateService<T>;

    fn layer(
        &self,
        inner: BoxCloneSyncService<IncomingRequest, HttpResponse, Infallible>,
    ) -> Self::Service {
        StateService {
            state: self.state.clone(),
            inner,
        }
    }
}

#[derive(Clone)]
pub(super) struct StateService<T> {
    state: T,
    inner: BoxCloneSyncService<IncomingRequest, HttpResponse, Infallible>,
}

impl<T> Service<IncomingRequest> for StateService<T>
where
    T: Clone + Send + Sync + 'static,
{
    type Response = HttpResponse;
    type Error = Infallible;
    type Future = BoxFuture;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut request: IncomingRequest) -> Self::Future {
        request.extensions_mut().insert(self.state.clone());
        let mut inner = self.inner.clone();
        Box::pin(async move { inner.call(request).await })
    }
}

#[derive(Clone)]
pub(super) struct StateFromRefLayer<Outer, Inner> {
    state: Outer,
    _marker: PhantomData<fn() -> Inner>,
}

impl<Outer, Inner> StateFromRefLayer<Outer, Inner> {
    pub(super) fn new(state: Outer) -> Self {
        Self {
            state,
            _marker: PhantomData,
        }
    }
}

impl<Outer, Inner> Layer<BoxCloneSyncService<IncomingRequest, HttpResponse, Infallible>>
    for StateFromRefLayer<Outer, Inner>
where
    Outer: Clone + Send + Sync + 'static,
    Inner: FromRef<Outer> + Clone + Send + Sync + 'static,
{
    type Service = StateFromRefService<Outer, Inner>;

    fn layer(
        &self,
        inner: BoxCloneSyncService<IncomingRequest, HttpResponse, Infallible>,
    ) -> Self::Service {
        StateFromRefService {
            state: self.state.clone(),
            inner,
            _marker: PhantomData,
        }
    }
}

#[derive(Clone)]
pub(super) struct StateFromRefService<Outer, Inner> {
    state: Outer,
    inner: BoxCloneSyncService<IncomingRequest, HttpResponse, Infallible>,
    _marker: PhantomData<fn() -> Inner>,
}

impl<Outer, Inner> Service<IncomingRequest> for StateFromRefService<Outer, Inner>
where
    Outer: Clone + Send + Sync + 'static,
    Inner: FromRef<Outer> + Clone + Send + Sync + 'static,
{
    type Response = HttpResponse;
    type Error = Infallible;
    type Future = BoxFuture;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut request: IncomingRequest) -> Self::Future {
        let inner_state = Inner::from_ref(&self.state);
        request.extensions_mut().insert(self.state.clone());
        request.extensions_mut().insert(inner_state);
        let mut inner = self.inner.clone();
        Box::pin(async move { inner.call(request).await })
    }
}
