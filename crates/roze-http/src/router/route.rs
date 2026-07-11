use std::convert::Infallible;

use http::Method;
use tower::{util::BoxCloneSyncService, Layer, Service};

use crate::rest::{HttpResponse, IncomingRequest};

#[derive(Clone)]
pub(super) struct Route {
    pub(super) method: Option<Method>,
    pub(super) service: BoxCloneSyncService<IncomingRequest, HttpResponse, Infallible>,
}

pub(super) fn boxed_service<S>(
    service: S,
) -> BoxCloneSyncService<IncomingRequest, HttpResponse, Infallible>
where
    S: Service<IncomingRequest, Response = HttpResponse, Error = Infallible>
        + Clone
        + Send
        + Sync
        + 'static,
    S::Future: Send + 'static,
{
    BoxCloneSyncService::new(service)
}

pub(super) fn layer_service<L>(
    layer: L,
    service: BoxCloneSyncService<IncomingRequest, HttpResponse, Infallible>,
) -> BoxCloneSyncService<IncomingRequest, HttpResponse, Infallible>
where
    L: Layer<BoxCloneSyncService<IncomingRequest, HttpResponse, Infallible>>
        + Clone
        + Send
        + Sync
        + 'static,
    L::Service: Service<IncomingRequest, Response = HttpResponse, Error = Infallible>
        + Clone
        + Send
        + Sync
        + 'static,
    <L::Service as Service<IncomingRequest>>::Future: Send + 'static,
{
    BoxCloneSyncService::new(layer.layer(service))
}
