use std::convert::Infallible;

use tower::{util::BoxCloneSyncService, Layer, Service};

use crate::rest::{HttpResponse, IncomingRequest};

use super::{
    not_found::NotFound,
    route::{boxed_service, layer_service},
};

#[derive(Clone)]
pub(super) enum Fallback {
    Default(BoxCloneSyncService<IncomingRequest, HttpResponse, Infallible>),
    Custom(BoxCloneSyncService<IncomingRequest, HttpResponse, Infallible>),
}

impl Fallback {
    pub(super) fn default_route() -> Self {
        Self::Default(boxed_service(NotFound))
    }

    pub(super) fn custom(
        service: BoxCloneSyncService<IncomingRequest, HttpResponse, Infallible>,
    ) -> Self {
        Self::Custom(service)
    }

    pub(super) fn is_custom(&self) -> bool {
        matches!(self, Self::Custom(_))
    }

    pub(super) fn service(&self) -> BoxCloneSyncService<IncomingRequest, HttpResponse, Infallible> {
        match self {
            Self::Default(service) | Self::Custom(service) => service.clone(),
        }
    }

    pub(super) fn merge(self, other: Self) -> Option<Self> {
        match (self, other) {
            (Self::Default(_), pick) | (pick, Self::Default(_)) => Some(pick),
            (Self::Custom(_), Self::Custom(_)) => None,
        }
    }

    pub(super) fn layer<L>(&mut self, layer: L)
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
        let replacement = layer_service(layer, self.service());
        *self = match self {
            Self::Default(_) => Self::Default(replacement),
            Self::Custom(_) => Self::Custom(replacement),
        };
    }
}
