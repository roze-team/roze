use std::{
    any::TypeId,
    convert::Infallible,
    fmt,
    sync::Arc,
    task::{Context, Poll},
};

use http::Method;
use tower::{util::BoxCloneSyncService, Layer, Service, ServiceExt};

use crate::{
    extract::{FromRef, OriginalUri},
    handler::Handler,
    rest::{HttpResponse, IncomingRequest},
};

mod fallback;
mod future;
mod into_make_service;
mod method_filter;
mod method_not_allowed;
mod method_routing;
mod not_found;
mod path;
mod path_router;
mod route;
mod service;
mod state;
mod strip_prefix;

pub use future::RouteFuture;
pub use into_make_service::{
    IntoMakeService, IntoMakeServiceWithConnectInfo, MethodRouterIntoMakeServiceWithConnectInfo,
};
pub use method_filter::{MethodFilter, MethodSelection};
pub use method_routing::{
    any, any_service, connect, connect_service, delete, delete_service, get, get_service, head,
    head_service, on, on_service, options, options_service, patch, patch_service, post,
    post_service, put, put_service, trace, trace_service, MethodRouter,
};
pub use service::{RouterAsService, RouterIntoService};

use fallback::Fallback;
use method_routing::router_handler_fn;
use path::{normalize_nest_prefix, normalize_path};
use path_router::PathRouter;
use route::boxed_service;
use state::{StateFromRefLayer, StateLayer};
use strip_prefix::StripPrefixService;

#[must_use = "routers must be used as services or retained after configuration"]
pub struct Router {
    inner: Arc<RouterInner>,
}

#[derive(Clone)]
struct RouterInner {
    path_router: PathRouter,
    fallback: Fallback,
    method_not_allowed_fallback:
        Option<BoxCloneSyncService<IncomingRequest, HttpResponse, Infallible>>,
}

impl Clone for Router {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl Default for Router {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for Router {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Router")
            .field("route_count", &self.inner.path_router.route_count())
            .field(
                "routes",
                &self.inner.path_router.paths().collect::<Vec<_>>(),
            )
            .field("has_custom_fallback", &self.inner.fallback.is_custom())
            .field(
                "has_method_not_allowed_fallback",
                &self.inner.method_not_allowed_fallback.is_some(),
            )
            .finish()
    }
}

impl Router {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RouterInner {
                path_router: PathRouter::default(),
                fallback: Fallback::default_route(),
                method_not_allowed_fallback: None,
            }),
        }
    }

    fn inner_mut(&mut self) -> &mut RouterInner {
        Arc::make_mut(&mut self.inner)
    }

    fn into_inner(self) -> RouterInner {
        Arc::try_unwrap(self.inner).unwrap_or_else(|inner| (*inner).clone())
    }

    #[track_caller]
    pub fn route(mut self, path: impl Into<String>, method_router: MethodRouter) -> Self {
        let path = normalize_path(path.into());
        let fallback = self.inner.method_not_allowed_fallback.clone();
        self.inner_mut()
            .path_router
            .route(path, method_router, fallback);
        self
    }

    #[track_caller]
    pub fn nest(mut self, prefix: impl Into<String>, router: Router) -> Self {
        let prefix = normalize_nest_prefix(prefix.into(), "use Router::merge instead");
        let path_router = router.into_inner().path_router;
        let fallback = self.inner.method_not_allowed_fallback.clone();
        self.inner_mut()
            .path_router
            .nest(prefix, path_router, fallback);
        self
    }

    #[track_caller]
    pub fn merge<R>(mut self, router: R) -> Self
    where
        R: Into<Router>,
    {
        let router = router.into().into_inner();
        let fallback = self
            .inner
            .fallback
            .clone()
            .merge(router.fallback)
            .unwrap_or_else(|| panic!("cannot merge two routers that both have a fallback"));
        self.inner_mut().fallback = fallback;
        let fallback = self.inner.method_not_allowed_fallback.clone();
        self.inner_mut()
            .path_router
            .merge(router.path_router, fallback);
        self
    }

    #[track_caller]
    pub fn nest_service<S>(self, prefix: impl Into<String>, service: S) -> Self
    where
        S: Service<IncomingRequest, Response = HttpResponse, Error = Infallible>
            + Clone
            + Send
            + Sync
            + 'static,
        S::Future: Send + 'static,
    {
        let prefix = normalize_nest_prefix(prefix.into(), "use Router::fallback_service instead");
        let service = StripPrefixService::new(prefix.clone(), service);
        self.route(prefix.clone(), any_service(service.clone()))
            .route(format!("{prefix}/"), any_service(service.clone()))
            .route(format!("{prefix}/{{*tail}}"), any_service(service))
    }

    #[track_caller]
    pub fn route_service<S>(self, path: impl Into<String>, service: S) -> Self
    where
        S: Service<IncomingRequest, Response = HttpResponse, Error = Infallible>
            + Clone
            + Send
            + Sync
            + 'static,
        S::Future: Send + 'static,
    {
        assert_service_not_router::<S>("Router::route_service");
        self.route(path, any_service(service))
    }

    fn route_method_service<S>(self, method: Method, path: impl Into<String>, service: S) -> Self
    where
        S: Service<IncomingRequest, Response = HttpResponse, Error = Infallible>
            + Clone
            + Send
            + Sync
            + 'static,
        S::Future: Send + 'static,
    {
        self.route(path, MethodRouter::new().on_service(method, service))
    }

    fn route_handler<H, T>(self, method: Method, path: impl Into<String>, handler: H) -> Self
    where
        H: Handler<T>,
    {
        self.route_method_service(method, path, handler.into_service())
    }

    router_handler_fn!(get, GET);
    router_handler_fn!(post, POST);
    router_handler_fn!(put, PUT);
    router_handler_fn!(patch, PATCH);
    router_handler_fn!(delete, DELETE);
    router_handler_fn!(head, HEAD);
    router_handler_fn!(options, OPTIONS);
    router_handler_fn!(trace, TRACE);
    router_handler_fn!(connect, CONNECT);

    fn fallback_route_service<S>(mut self, service: S) -> Self
    where
        S: Service<IncomingRequest, Response = HttpResponse, Error = Infallible>
            + Clone
            + Send
            + Sync
            + 'static,
        S::Future: Send + 'static,
    {
        self.inner_mut().fallback = Fallback::custom(boxed_service(service));
        self
    }

    #[track_caller]
    pub fn fallback<H, T>(self, handler: H) -> Self
    where
        H: Handler<T>,
    {
        let mut router = self;
        let inner = router.inner_mut();
        inner.fallback = Fallback::custom(handler.into_service());
        router
    }

    #[track_caller]
    pub fn fallback_service<S>(self, service: S) -> Self
    where
        S: Service<IncomingRequest, Response = HttpResponse, Error = Infallible>
            + Clone
            + Send
            + Sync
            + 'static,
        S::Future: Send + 'static,
    {
        assert_service_not_router::<S>("Router::fallback_service");
        self.fallback_route_service(service)
    }

    pub fn reset_fallback(mut self) -> Self {
        let inner = self.inner_mut();
        inner.fallback = Fallback::default_route();
        self
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
        assert_service_not_router::<S>("Router::method_not_allowed_fallback_service");
        let fallback = boxed_service(service);
        let inner = self.inner_mut();
        inner.method_not_allowed_fallback = Some(fallback.clone());
        inner.path_router.set_method_not_allowed_fallback(fallback);
        self
    }

    pub fn has_routes(&self) -> bool {
        self.inner.path_router.has_routes()
    }

    pub fn as_service(&mut self) -> RouterAsService<'_> {
        RouterAsService::new(self)
    }

    pub fn into_service(self) -> RouterIntoService {
        RouterIntoService::new(self)
    }

    pub fn into_make_service(self) -> IntoMakeService {
        IntoMakeService::new(self)
    }

    pub fn into_make_service_with_connect_info<T>(self) -> IntoMakeServiceWithConnectInfo<T> {
        IntoMakeServiceWithConnectInfo::new(self)
    }

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
        self.inner_mut()
            .path_router
            .layer_routes(layer.clone(), true);
        self.inner_mut().fallback.layer(layer);
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
        if !self.inner.path_router.has_routes() {
            panic!(
                "adding a route_layer before any routes is a no-op; add the routes you want the layer to apply to first"
            );
        }
        self.inner_mut().path_router.layer_routes(layer, false);
        self
    }
}

impl Service<IncomingRequest> for Router {
    type Response = HttpResponse;
    type Error = Infallible;
    type Future = RouteFuture;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, request: IncomingRequest) -> Self::Future {
        let (mut parts, body) = request.into_parts();
        if parts.extensions.get::<OriginalUri>().is_none() {
            let original_uri = parts.uri.clone();
            parts.extensions.insert(OriginalUri(original_uri));
        }
        let method = parts.method.clone();
        let service = self
            .inner
            .path_router
            .match_service(&mut parts)
            .unwrap_or_else(|| self.inner.fallback.service());
        let request = IncomingRequest::from_parts(parts, body);
        RouteFuture::new(method, Box::pin(service.oneshot(request)))
    }
}

fn assert_service_not_router<S: 'static>(method: &str) {
    if TypeId::of::<S>() == TypeId::of::<Router>() {
        panic!("{method} cannot be used with Router; use Router::nest instead");
    }
}

#[cfg(test)]
mod tests;
