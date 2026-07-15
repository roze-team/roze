use std::{
    convert::Infallible,
    fmt,
    task::{Context, Poll},
};

use http::Method;
use tower::{util::BoxCloneSyncService, Layer, Service, ServiceExt};

use crate::{
    extract::FromRef,
    handler::Handler,
    rest::{HttpResponse, IncomingRequest, SharedService},
};

use super::{
    assert_service_not_router,
    method_filter::{MethodFilter, MethodSelection},
    method_not_allowed::{AllowHeader, MethodNotAllowed},
    path_router::{method_label, routes_overlap},
    route::{boxed_service, layer_service},
    state::{StateFromRefLayer, StateLayer},
    MethodRouterIntoMakeServiceWithConnectInfo, RouteFuture, Router,
};

macro_rules! chained_handler_fn {
    ($name:ident, $method:ident) => {
        #[doc = concat!("Chain a handler for `", stringify!($method), "` requests.")]
        #[track_caller]
        pub fn $name<H, T>(self, handler: H) -> Self
        where
            H: crate::handler::Handler<T>,
        {
            self.on(http::Method::$method, handler)
        }
    };
}

macro_rules! router_handler_fn {
    ($name:ident, $method:ident) => {
        #[doc = concat!("Route `", stringify!($method), "` requests at a path.")]
        #[track_caller]
        pub fn $name<H, T>(self, path: impl Into<String>, handler: H) -> Self
        where
            H: crate::handler::Handler<T>,
        {
            self.route_handler(http::Method::$method, path, handler)
        }
    };
}

macro_rules! chained_service_fn {
    ($name:ident, $method:ident) => {
        #[doc = concat!("Chain a service for `", stringify!($method), "` requests.")]
        pub fn $name<S>(self, service: S) -> Self
        where
            S: tower::Service<
                    crate::rest::IncomingRequest,
                    Response = crate::rest::HttpResponse,
                    Error = std::convert::Infallible,
                > + Clone
                + Send
                + Sync
                + 'static,
            S::Future: Send + 'static,
        {
            self.on_service(http::Method::$method, service)
        }
    };
}

pub(super) use router_handler_fn;

macro_rules! top_level_handler_fn {
    ($name:ident, $method:ident) => {
        #[doc = concat!("Route `", stringify!($method), "` requests to the handler.")]
        #[track_caller]
        pub fn $name<H, T>(handler: H) -> MethodRouter
        where
            H: Handler<T>,
        {
            on(MethodFilter::$method, handler)
        }
    };
}

macro_rules! top_level_service_fn {
    ($name:ident, $method:ident) => {
        #[doc = concat!("Route `", stringify!($method), "` requests to the service.")]
        #[track_caller]
        pub fn $name<S>(service: S) -> MethodRouter
        where
            S: Service<IncomingRequest, Response = HttpResponse, Error = Infallible>
                + Clone
                + Send
                + Sync
                + 'static,
            S::Future: Send + 'static,
        {
            on_service(MethodFilter::$method, service)
        }
    };
}

top_level_handler_fn!(get, GET);
top_level_handler_fn!(post, POST);
top_level_handler_fn!(put, PUT);
top_level_handler_fn!(patch, PATCH);
top_level_handler_fn!(delete, DELETE);
top_level_handler_fn!(head, HEAD);
top_level_handler_fn!(options, OPTIONS);
top_level_handler_fn!(trace, TRACE);
top_level_handler_fn!(connect, CONNECT);

top_level_service_fn!(get_service, GET);
top_level_service_fn!(post_service, POST);
top_level_service_fn!(put_service, PUT);
top_level_service_fn!(patch_service, PATCH);
top_level_service_fn!(delete_service, DELETE);
top_level_service_fn!(head_service, HEAD);
top_level_service_fn!(options_service, OPTIONS);
top_level_service_fn!(trace_service, TRACE);
top_level_service_fn!(connect_service, CONNECT);

#[track_caller]
pub fn any<H, T>(handler: H) -> MethodRouter
where
    H: Handler<T>,
{
    MethodRouter::new().any(handler)
}

#[track_caller]
pub fn any_service<S>(service: S) -> MethodRouter
where
    S: Service<IncomingRequest, Response = HttpResponse, Error = Infallible>
        + Clone
        + Send
        + Sync
        + 'static,
    S::Future: Send + 'static,
{
    MethodRouter::new().any_service(service)
}

#[track_caller]
pub fn on<H, T>(filter: MethodFilter, handler: H) -> MethodRouter
where
    H: Handler<T>,
{
    MethodRouter::new().on(filter, handler)
}

#[track_caller]
pub fn on_service<S>(filter: MethodFilter, service: S) -> MethodRouter
where
    S: Service<IncomingRequest, Response = HttpResponse, Error = Infallible>
        + Clone
        + Send
        + Sync
        + 'static,
    S::Future: Send + 'static,
{
    MethodRouter::new().on_service(filter, service)
}

#[derive(Clone)]
#[must_use = "method routers must be used as services or retained after configuration"]
pub struct MethodRouter {
    pub(super) endpoints: Vec<MethodEndpoint>,
    allow_header: AllowHeader,
    pub(super) method_not_allowed_fallback:
        Option<BoxCloneSyncService<IncomingRequest, HttpResponse, Infallible>>,
}

#[derive(Clone)]
pub(super) struct MethodEndpoint {
    pub(super) method: Option<Method>,
    pub(super) service: BoxCloneSyncService<IncomingRequest, HttpResponse, Infallible>,
}

impl Default for MethodRouter {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for MethodRouter {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MethodRouter")
            .field(
                "methods",
                &self
                    .endpoints
                    .iter()
                    .map(|endpoint| method_label(&endpoint.method))
                    .collect::<Vec<_>>(),
            )
            .field(
                "has_method_not_allowed_fallback",
                &self.method_not_allowed_fallback.is_some(),
            )
            .finish()
    }
}

impl From<MethodRouter> for Router {
    fn from(method_router: MethodRouter) -> Self {
        Router::new().route("/", method_router)
    }
}

impl MethodRouter {
    pub fn new() -> Self {
        Self {
            endpoints: Vec::new(),
            allow_header: AllowHeader::default(),
            method_not_allowed_fallback: None,
        }
    }

    fn refresh_allow_header(&mut self) {
        self.allow_header = AllowHeader::from_methods(
            self.endpoints
                .iter()
                .map(|endpoint| endpoint.method.as_ref()),
        );
    }

    #[track_caller]
    pub fn on<M, H, T>(self, method: M, handler: H) -> Self
    where
        M: Into<MethodSelection>,
        H: Handler<T>,
    {
        self.on_service(method, handler.into_service())
    }

    #[track_caller]
    pub fn on_service<M, S>(mut self, method: M, service: S) -> Self
    where
        M: Into<MethodSelection>,
        S: Service<IncomingRequest, Response = HttpResponse, Error = Infallible>
            + Clone
            + Send
            + Sync
            + 'static,
        S::Future: Send + 'static,
    {
        assert_service_not_router::<S>("MethodRouter::on_service");
        let methods = method.into().methods();
        if methods.is_empty() {
            panic!("method filter must not be empty");
        }
        let service = boxed_service(service);
        for method in methods {
            if self
                .endpoints
                .iter()
                .any(|endpoint| routes_overlap(&endpoint.method, &Some(method.clone())))
            {
                panic!("overlapping method route for {method}");
            }
            self.endpoints.push(MethodEndpoint {
                method: Some(method),
                service: service.clone(),
            });
        }
        self.refresh_allow_header();
        self
    }

    #[track_caller]
    pub fn any<H, T>(self, handler: H) -> Self
    where
        H: Handler<T>,
    {
        self.any_service(handler.into_service())
    }

    #[track_caller]
    pub fn any_service<S>(mut self, service: S) -> Self
    where
        S: Service<IncomingRequest, Response = HttpResponse, Error = Infallible>
            + Clone
            + Send
            + Sync
            + 'static,
        S::Future: Send + 'static,
    {
        assert_service_not_router::<S>("MethodRouter::any_service");
        if self
            .endpoints
            .iter()
            .any(|endpoint| endpoint.method.is_none())
        {
            panic!("overlapping method route for any");
        }
        self.endpoints.push(MethodEndpoint {
            method: None,
            service: boxed_service(service),
        });
        self.refresh_allow_header();
        self
    }

    chained_handler_fn!(get, GET);
    chained_handler_fn!(post, POST);
    chained_handler_fn!(put, PUT);
    chained_handler_fn!(patch, PATCH);
    chained_handler_fn!(delete, DELETE);
    chained_handler_fn!(head, HEAD);
    chained_handler_fn!(options, OPTIONS);
    chained_handler_fn!(trace, TRACE);
    chained_handler_fn!(connect, CONNECT);

    chained_service_fn!(get_service, GET);
    chained_service_fn!(post_service, POST);
    chained_service_fn!(put_service, PUT);
    chained_service_fn!(patch_service, PATCH);
    chained_service_fn!(delete_service, DELETE);
    chained_service_fn!(head_service, HEAD);
    chained_service_fn!(options_service, OPTIONS);
    chained_service_fn!(trace_service, TRACE);
    chained_service_fn!(connect_service, CONNECT);

    pub fn with_state<T>(self, state: T) -> Self
    where
        T: Clone + Send + Sync + 'static,
    {
        self.layer(StateLayer::new(state))
    }

    pub fn with_state_from_ref<Outer, Inner>(self, state: Outer) -> Self
    where
        Outer: Clone + Send + Sync + 'static,
        Inner: FromRef<Outer> + Clone + Send + Sync + 'static,
    {
        self.layer(StateFromRefLayer::<Outer, Inner>::new(state))
    }

    #[track_caller]
    pub fn method_not_allowed_fallback<H, T>(self, handler: H) -> Self
    where
        H: Handler<T>,
    {
        self.method_not_allowed_fallback_service(handler.into_service())
    }

    #[track_caller]
    pub fn method_not_allowed_fallback_service<S>(mut self, service: S) -> Self
    where
        S: Service<IncomingRequest, Response = HttpResponse, Error = Infallible>
            + Clone
            + Send
            + Sync
            + 'static,
        S::Future: Send + 'static,
    {
        assert_service_not_router::<S>("MethodRouter::method_not_allowed_fallback_service");
        if self.method_not_allowed_fallback.is_some() {
            panic!("overlapping method-not-allowed fallback");
        }
        self.method_not_allowed_fallback = Some(boxed_service(service));
        self
    }

    pub fn method_filter(&self) -> Option<MethodFilter> {
        if self.method_not_allowed_fallback.is_some() {
            return None;
        }
        let mut filter = MethodFilter::default();
        for endpoint in &self.endpoints {
            let method = endpoint.method.as_ref()?;
            filter |= MethodFilter::from_method(method)?;
        }
        (!filter.is_empty()).then_some(filter)
    }

    #[track_caller]
    pub fn merge(mut self, other: MethodRouter) -> Self {
        for endpoint in other.endpoints {
            if self
                .endpoints
                .iter()
                .any(|existing| routes_overlap(&existing.method, &endpoint.method))
            {
                panic!(
                    "cannot merge two method routers that both define {}",
                    method_label(&endpoint.method)
                );
            }
            self.endpoints.push(endpoint);
        }
        match (
            self.method_not_allowed_fallback.is_some(),
            other.method_not_allowed_fallback,
        ) {
            (true, Some(_)) => {
                panic!(
                    "cannot merge two method routers that both have method-not-allowed fallbacks"
                )
            }
            (false, fallback) => {
                self.method_not_allowed_fallback = fallback;
            }
            (true, None) => {}
        }
        self.refresh_allow_header();
        self
    }

    pub fn layer<L>(mut self, layer: L) -> Self
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
        for endpoint in &mut self.endpoints {
            endpoint.service = layer_service(layer.clone(), endpoint.service.clone());
        }
        if let Some(fallback) = self.method_not_allowed_fallback.take() {
            self.method_not_allowed_fallback = Some(layer_service(layer, fallback));
        }
        self
    }

    #[track_caller]
    pub fn route_layer<L>(mut self, layer: L) -> Self
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
        if self.endpoints.is_empty() {
            panic!(
                "adding a route_layer before any method routes is a no-op; add the method routes you want the layer to apply to first"
            );
        }
        for endpoint in &mut self.endpoints {
            endpoint.service = layer_service(layer.clone(), endpoint.service.clone());
        }
        self
    }

    pub fn into_make_service(self) -> SharedService<Self> {
        SharedService::new(self)
    }

    pub fn into_make_service_with_connect_info<T>(
        self,
    ) -> MethodRouterIntoMakeServiceWithConnectInfo<T> {
        MethodRouterIntoMakeServiceWithConnectInfo::new(self)
    }
}

impl Service<IncomingRequest> for MethodRouter {
    type Response = HttpResponse;
    type Error = Infallible;
    type Future = RouteFuture;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, request: IncomingRequest) -> Self::Future {
        let method = request.method().clone();
        let service = self
            .endpoints
            .iter()
            .find(|endpoint| endpoint.method.as_ref() == Some(request.method()))
            .or_else(|| {
                (*request.method() == Method::HEAD).then(|| {
                    self.endpoints
                        .iter()
                        .find(|endpoint| endpoint.method.as_ref() == Some(&Method::GET))
                })?
            })
            .or_else(|| {
                self.endpoints
                    .iter()
                    .find(|endpoint| endpoint.method.is_none())
            })
            .map(|endpoint| endpoint.service.clone())
            .unwrap_or_else(|| {
                self.method_not_allowed_fallback.clone().unwrap_or_else(|| {
                    boxed_service(MethodNotAllowed::new(self.allow_header.clone()))
                })
            });
        RouteFuture::new(method, Box::pin(service.oneshot(request)))
    }
}
