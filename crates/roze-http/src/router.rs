use std::{
    any::TypeId,
    convert::Infallible,
    fmt,
    sync::Arc,
    task::{Context, Poll},
};

#[cfg(test)]
use std::{future::Future, pin::Pin};

#[cfg(test)]
use crate::{extract::MatchedPath, route_params::RouteParams};

use http::Method;
use tower::{util::BoxCloneSyncService, Layer, Service, ServiceExt};

use crate::{
    extract::{FromRef, OriginalUri},
    handler::Handler,
    rest::{HttpResponse, IncomingRequest, SharedService},
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
    post_service, put, put_service, trace, trace_service,
};
pub use service::{RouterAsService, RouterIntoService};

use fallback::Fallback;
use method_not_allowed::{AllowHeader, MethodNotAllowed};
use method_routing::{chained_handler_fn, chained_service_fn};
use path::{normalize_nest_prefix, normalize_path};
use path_router::{method_label, routes_overlap, PathRouter};
use route::{boxed_service, layer_service};
use state::{StateFromRefLayer, StateLayer};
use strip_prefix::StripPrefixService;

#[cfg(test)]
type BoxFuture = Pin<Box<dyn Future<Output = Result<HttpResponse, Infallible>> + Send>>;

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

    #[track_caller]
    pub fn get<H, T>(self, path: impl Into<String>, handler: H) -> Self
    where
        H: Handler<T>,
    {
        self.route_handler(Method::GET, path, handler)
    }

    #[track_caller]
    pub fn post<H, T>(self, path: impl Into<String>, handler: H) -> Self
    where
        H: Handler<T>,
    {
        self.route_handler(Method::POST, path, handler)
    }

    #[track_caller]
    pub fn put<H, T>(self, path: impl Into<String>, handler: H) -> Self
    where
        H: Handler<T>,
    {
        self.route_handler(Method::PUT, path, handler)
    }

    #[track_caller]
    pub fn patch<H, T>(self, path: impl Into<String>, handler: H) -> Self
    where
        H: Handler<T>,
    {
        self.route_handler(Method::PATCH, path, handler)
    }

    #[track_caller]
    pub fn delete<H, T>(self, path: impl Into<String>, handler: H) -> Self
    where
        H: Handler<T>,
    {
        self.route_handler(Method::DELETE, path, handler)
    }

    #[track_caller]
    pub fn head<H, T>(self, path: impl Into<String>, handler: H) -> Self
    where
        H: Handler<T>,
    {
        self.route_handler(Method::HEAD, path, handler)
    }

    #[track_caller]
    pub fn options<H, T>(self, path: impl Into<String>, handler: H) -> Self
    where
        H: Handler<T>,
    {
        self.route_handler(Method::OPTIONS, path, handler)
    }

    #[track_caller]
    pub fn trace<H, T>(self, path: impl Into<String>, handler: H) -> Self
    where
        H: Handler<T>,
    {
        self.route_handler(Method::TRACE, path, handler)
    }

    #[track_caller]
    pub fn connect<H, T>(self, path: impl Into<String>, handler: H) -> Self
    where
        H: Handler<T>,
    {
        self.route_handler(Method::CONNECT, path, handler)
    }

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

#[derive(Clone)]
#[must_use = "method routers must be used as services or retained after configuration"]
pub struct MethodRouter {
    endpoints: Vec<MethodEndpoint>,
    allow_header: AllowHeader,
    method_not_allowed_fallback:
        Option<BoxCloneSyncService<IncomingRequest, HttpResponse, Infallible>>,
}

#[derive(Clone)]
struct MethodEndpoint {
    method: Option<Method>,
    service: BoxCloneSyncService<IncomingRequest, HttpResponse, Infallible>,
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

fn assert_service_not_router<S: 'static>(method: &str) {
    if TypeId::of::<S>() == TypeId::of::<Router>() {
        panic!("{method} cannot be used with Router; use Router::nest instead");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rest;
    use crate::{ConnectInfo, NestedPath, OriginalUri, Path, State};
    use http::{header, HeaderValue, Request, StatusCode, Uri};
    use http_body_util::BodyExt;
    use std::net::SocketAddr;

    #[tokio::test]
    async fn routes_requests_to_matching_handler() {
        let mut router = Router::new().get("/healthz", || async { "ok" });
        let response = router
            .call(
                Request::builder()
                    .uri("/healthz")
                    .body(empty_incoming())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"ok");
    }

    #[tokio::test]
    async fn handler_can_return_status_headers_and_body_tuple() {
        let mut router = Router::new().route(
            "/queued",
            post(|| async {
                let mut headers = http::HeaderMap::new();
                headers.insert("x-roze-test", HeaderValue::from_static("yes"));
                (StatusCode::ACCEPTED, headers, "queued")
            }),
        );
        let response = router
            .call(
                Request::builder()
                    .method(Method::POST)
                    .uri("/queued")
                    .body(empty_incoming())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::ACCEPTED);
        assert_eq!(
            response.headers().get("x-roze-test"),
            Some(&HeaderValue::from_static("yes"))
        );
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"queued");
    }

    #[tokio::test]
    async fn returns_not_found_for_missing_route() {
        let mut router = Router::new();
        let response = router
            .call(
                Request::builder()
                    .uri("/missing")
                    .body(empty_incoming())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn uses_fallback_for_missing_route() {
        let mut router = Router::new().fallback(|| async { "fallback" });
        let response = router
            .call(
                Request::builder()
                    .uri("/missing")
                    .body(empty_incoming())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"fallback");
    }

    #[tokio::test]
    async fn fallback_preserves_original_uri_without_matched_path() {
        let mut router = Router::new()
            .route("/known", get(|| async { "known" }))
            .fallback(
                |matched_path: Option<MatchedPath>, original_uri: OriginalUri| async move {
                    format!("{} {}", matched_path.is_none(), original_uri.0)
                },
            );
        let response = router
            .call(
                Request::builder()
                    .uri("/missing?trace=1")
                    .body(empty_incoming())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"true /missing?trace=1");
    }

    #[tokio::test]
    async fn nested_router_fallback_clears_outer_route_match_metadata() {
        let child = Router::new()
            .route("/known", get(|| async { "known" }))
            .fallback(
                |matched_path: Option<MatchedPath>,
                 params: Option<crate::RawPathParams>,
                 original_uri: OriginalUri,
                 nested_path: NestedPath| async move {
                    format!(
                        "{} {} {} {}",
                        matched_path.is_none(),
                        params.is_none(),
                        original_uri.0,
                        nested_path.as_str()
                    )
                },
            );
        let mut router = Router::new().nest_service("/api", child);
        let response = router
            .call(
                Request::builder()
                    .uri("/api/missing?trace=1")
                    .body(empty_incoming())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"true true /api/missing?trace=1 /api");
    }

    #[tokio::test]
    async fn uses_fallback_service_for_missing_route() {
        let service = tower::service_fn(|_request: IncomingRequest| async {
            Ok::<_, Infallible>(rest::text_response(StatusCode::OK, "service fallback"))
        });
        let mut router = Router::new().fallback_service(service);
        let response = router
            .call(
                Request::builder()
                    .uri("/missing")
                    .body(empty_incoming())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"service fallback");
    }

    #[test]
    #[should_panic(expected = "Router::fallback_service cannot be used with Router")]
    fn fallback_service_panics_when_service_is_router() {
        let child = Router::new().route("/users", get(|| async { "users" }));
        let _router = Router::new().fallback_service(child);
    }

    #[tokio::test]
    async fn reset_fallback_restores_not_found() {
        let mut router = Router::new()
            .fallback(|| async { "fallback" })
            .reset_fallback();
        let response = router
            .call(
                Request::builder()
                    .uri("/missing")
                    .body(empty_incoming())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn reports_whether_router_has_routes() {
        assert!(!Router::new().has_routes());
        assert!(Router::new()
            .route("/healthz", get(|| async { "ok" }))
            .has_routes());
    }

    #[test]
    fn router_clones_share_inner_until_a_builder_modifies_one() {
        let router = Router::new().route("/healthz", get(|| async { "ok" }));
        let clone = router.clone();

        assert!(Arc::ptr_eq(&router.inner, &clone.inner));

        let modified = clone.route("/readyz", get(|| async { "ready" }));

        assert!(!Arc::ptr_eq(&router.inner, &modified.inner));
        assert!(!router.inner.path_router.contains_exact_path("/readyz"));
        assert!(modified.inner.path_router.contains_exact_path("/readyz"));
    }

    #[test]
    fn router_debug_reports_route_summary() {
        let router = Router::new()
            .route("/healthz", get(|| async { "ok" }))
            .method_not_allowed_fallback(|| async { "nope" });
        let debug = format!("{router:?}");

        assert!(debug.contains("Router"));
        assert!(debug.contains("route_count: 1"));
        assert!(debug.contains("/healthz"));
        assert!(debug.contains("has_method_not_allowed_fallback: true"));
    }

    #[test]
    fn method_router_debug_reports_method_summary() {
        let method_router = get(|| async { "ok" }).post(|| async { "created" });
        let debug = format!("{method_router:?}");

        assert!(debug.contains("MethodRouter"));
        assert!(debug.contains("GET"));
        assert!(debug.contains("POST"));
    }

    #[tokio::test]
    async fn with_state_injects_router_state() {
        let mut router = Router::new()
            .route(
                "/state",
                get(|State(state): State<String>| async move { state }),
            )
            .with_state("router-state".to_string());
        let response = router
            .call(
                Request::builder()
                    .uri("/state")
                    .body(empty_incoming())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"router-state");
    }

    #[tokio::test]
    async fn with_state_from_ref_injects_outer_and_inner_state() {
        #[derive(Clone)]
        struct AppState {
            api: ApiState,
            name: String,
        }

        #[derive(Clone)]
        struct ApiState {
            tenant: String,
        }

        impl FromRef<AppState> for ApiState {
            fn from_ref(input: &AppState) -> Self {
                input.api.clone()
            }
        }

        let mut router = Router::new()
            .route(
                "/state",
                get(
                    |State(app): State<AppState>, State(api): State<ApiState>| async move {
                        format!("{}:{}", app.name, api.tenant)
                    },
                ),
            )
            .with_state_from_ref::<AppState, ApiState>(AppState {
                api: ApiState {
                    tenant: "tenant-a".to_string(),
                },
                name: "app".to_string(),
            });
        let response = router
            .call(
                Request::builder()
                    .uri("/state")
                    .body(empty_incoming())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"app:tenant-a");
    }

    #[tokio::test]
    async fn method_router_with_state_injects_state() {
        let mut router = Router::new().route(
            "/state",
            get(|State(state): State<String>| async move { state })
                .with_state("method-state".to_string()),
        );
        let response = router
            .call(
                Request::builder()
                    .uri("/state")
                    .body(empty_incoming())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"method-state");
    }

    #[tokio::test]
    async fn method_router_with_state_from_ref_injects_inner_state() {
        #[derive(Clone)]
        struct AppState {
            api: ApiState,
        }

        #[derive(Clone)]
        struct ApiState {
            value: String,
        }

        impl FromRef<AppState> for ApiState {
            fn from_ref(input: &AppState) -> Self {
                input.api.clone()
            }
        }

        let mut router = Router::new().route(
            "/state",
            get(|State(api): State<ApiState>| async move { api.value })
                .with_state_from_ref::<AppState, ApiState>(AppState {
                    api: ApiState {
                        value: "method-inner".to_string(),
                    },
                }),
        );
        let response = router
            .call(
                Request::builder()
                    .uri("/state")
                    .body(empty_incoming())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"method-inner");
    }

    #[tokio::test]
    async fn with_state_applies_to_fallback() {
        let mut router = Router::new()
            .fallback(|State(state): State<String>| async move { state })
            .with_state("fallback-state".to_string());
        let response = router
            .call(
                Request::builder()
                    .uri("/missing")
                    .body(empty_incoming())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"fallback-state");
    }

    #[tokio::test]
    async fn optional_state_extractor_can_be_absent_in_handler() {
        let mut router = Router::new().route(
            "/optional-state",
            get(|state: Option<State<String>>| async move {
                state
                    .map(|State(value)| value)
                    .unwrap_or_else(|| "none".to_string())
            }),
        );
        let response = router
            .call(
                Request::builder()
                    .uri("/optional-state")
                    .body(empty_incoming())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"none");
    }

    #[tokio::test]
    async fn combines_methods_for_the_same_path() {
        let mut router = Router::new().route(
            "/users",
            get(|| async { "list" }).post(|| async { "create" }),
        );

        let get_response = router
            .call(
                Request::builder()
                    .method(Method::GET)
                    .uri("/users")
                    .body(empty_incoming())
                    .unwrap(),
            )
            .await
            .unwrap();
        let get_body = get_response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&get_body[..], b"list");

        let post_response = router
            .call(
                Request::builder()
                    .method(Method::POST)
                    .uri("/users")
                    .body(empty_incoming())
                    .unwrap(),
            )
            .await
            .unwrap();
        let post_body = post_response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes();
        assert_eq!(&post_body[..], b"create");
    }

    #[tokio::test]
    async fn returns_method_not_allowed_for_known_path() {
        let mut router = Router::new().route(
            "/users",
            get(|| async { "list" }).post(|| async { "create" }),
        );
        let response = router
            .call(
                Request::builder()
                    .method(Method::PUT)
                    .uri("/users")
                    .body(empty_incoming())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
        assert_eq!(
            response.headers().get(header::ALLOW),
            Some(&HeaderValue::from_static("GET, HEAD, POST"))
        );
    }

    #[tokio::test]
    async fn empty_method_router_returns_empty_allow_header() {
        let mut service = MethodRouter::new();
        let response = service
            .call(
                Request::builder()
                    .method(Method::GET)
                    .uri("/")
                    .body(empty_incoming())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
        assert_eq!(
            response.headers().get(header::ALLOW),
            Some(&HeaderValue::from_static(""))
        );
    }

    #[tokio::test]
    async fn empty_method_router_route_returns_empty_allow_header() {
        let mut router = Router::new().route("/empty", MethodRouter::new());
        let response = router
            .call(
                Request::builder()
                    .method(Method::GET)
                    .uri("/empty")
                    .body(empty_incoming())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
        assert_eq!(
            response.headers().get(header::ALLOW),
            Some(&HeaderValue::from_static(""))
        );
    }

    #[tokio::test]
    async fn method_router_can_override_method_not_allowed_fallback() {
        let mut router = Router::new().route(
            "/users",
            get(|| async { "list" }).method_not_allowed_fallback(|| async {
                rest::text_response(StatusCode::IM_A_TEAPOT, "custom method fallback")
            }),
        );
        let response = router
            .call(
                Request::builder()
                    .method(Method::PUT)
                    .uri("/users")
                    .body(empty_incoming())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::IM_A_TEAPOT);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"custom method fallback");
    }

    #[tokio::test]
    async fn method_router_can_override_method_not_allowed_fallback_with_service() {
        let service = tower::service_fn(|request: IncomingRequest| async move {
            Ok::<_, Infallible>(rest::text_response(
                StatusCode::IM_A_TEAPOT,
                format!("service fallback {}", request.method()),
            ))
        });
        let mut router = Router::new().route(
            "/users",
            get(|| async { "list" }).method_not_allowed_fallback_service(service),
        );
        let response = router
            .call(
                Request::builder()
                    .method(Method::PUT)
                    .uri("/users")
                    .body(empty_incoming())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::IM_A_TEAPOT);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"service fallback PUT");
    }

    #[test]
    #[should_panic(
        expected = "MethodRouter::method_not_allowed_fallback_service cannot be used with Router"
    )]
    fn method_router_method_not_allowed_fallback_service_panics_when_service_is_router() {
        let child = Router::new().route("/users", get(|| async { "users" }));
        let _router = get(|| async { "list" }).method_not_allowed_fallback_service(child);
    }

    #[tokio::test]
    async fn method_router_merge_combines_distinct_methods() {
        let mut router = get(|| async { "get" }).merge(post(|| async { "post" }));
        let get_response = router
            .call(
                Request::builder()
                    .method(Method::GET)
                    .uri("/")
                    .body(empty_incoming())
                    .unwrap(),
            )
            .await
            .unwrap();
        let get_body = get_response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&get_body[..], b"get");

        let post_response = router
            .call(
                Request::builder()
                    .method(Method::POST)
                    .uri("/")
                    .body(empty_incoming())
                    .unwrap(),
            )
            .await
            .unwrap();
        let post_body = post_response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes();
        assert_eq!(&post_body[..], b"post");
    }

    #[test]
    #[should_panic(expected = "cannot merge two method routers that both define GET")]
    fn method_router_merge_panics_on_overlapping_method() {
        let _router = get(|| async { "left" }).merge(get(|| async { "right" }));
    }

    #[test]
    #[should_panic(
        expected = "cannot merge two method routers that both have method-not-allowed fallbacks"
    )]
    fn method_router_merge_panics_on_overlapping_method_not_allowed_fallback() {
        let left = get(|| async { "left" }).method_not_allowed_fallback(|| async { "left 405" });
        let right =
            post(|| async { "right" }).method_not_allowed_fallback(|| async { "right 405" });
        let _router = left.merge(right);
    }

    #[tokio::test]
    async fn router_can_override_method_not_allowed_fallback() {
        let mut router = Router::new()
            .route("/users", get(|| async { "list" }))
            .method_not_allowed_fallback(|| async {
                rest::text_response(StatusCode::BAD_REQUEST, "router method fallback")
            });
        let response = router
            .call(
                Request::builder()
                    .method(Method::PUT)
                    .uri("/users")
                    .body(empty_incoming())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"router method fallback");
    }

    #[tokio::test]
    async fn router_can_override_method_not_allowed_fallback_with_service() {
        let service = tower::service_fn(|request: IncomingRequest| async move {
            Ok::<_, Infallible>(rest::text_response(
                StatusCode::BAD_REQUEST,
                format!("router service fallback {}", request.method()),
            ))
        });
        let mut router = Router::new()
            .route("/users", get(|| async { "list" }))
            .method_not_allowed_fallback_service(service);
        let response = router
            .call(
                Request::builder()
                    .method(Method::PUT)
                    .uri("/users")
                    .body(empty_incoming())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"router service fallback PUT");
    }

    #[test]
    #[should_panic(
        expected = "Router::method_not_allowed_fallback_service cannot be used with Router"
    )]
    fn router_method_not_allowed_fallback_service_panics_when_service_is_router() {
        let child = Router::new().route("/users", get(|| async { "users" }));
        let _router = Router::new()
            .route("/users", get(|| async { "list" }))
            .method_not_allowed_fallback_service(child);
    }

    #[tokio::test]
    async fn router_as_service_and_into_service_call_routes() {
        let mut router = Router::new().route("/healthz", get(|| async { "borrowed" }));
        let borrowed = router
            .as_service()
            .call(
                Request::builder()
                    .uri("/healthz")
                    .body(empty_incoming())
                    .unwrap(),
            )
            .await
            .unwrap();
        let borrowed_body = borrowed.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&borrowed_body[..], b"borrowed");

        let mut owned = Router::new()
            .route("/readyz", get(|| async { "owned" }))
            .into_service();
        let owned_response = owned
            .call(
                Request::builder()
                    .uri("/readyz")
                    .body(empty_incoming())
                    .unwrap(),
            )
            .await
            .unwrap();
        let owned_body = owned_response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes();
        assert_eq!(&owned_body[..], b"owned");
    }

    #[tokio::test]
    async fn router_into_make_service_builds_request_services() {
        let mut make_service = Router::new()
            .route("/healthz", get(|| async { "made" }))
            .into_make_service();
        let mut service = make_service.call(()).await.unwrap();
        let response = service
            .call(
                Request::builder()
                    .uri("/healthz")
                    .body(empty_incoming())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"made");
    }

    #[tokio::test]
    async fn router_into_make_service_with_connect_info_injects_peer_addr() {
        let peer_addr = SocketAddr::from(([127, 0, 0, 1], 4312));
        let router = Router::new().route(
            "/peer",
            get(|ConnectInfo(addr): ConnectInfo<SocketAddr>| async move { addr.to_string() }),
        );
        let shared_inner = Arc::clone(&router.inner);
        let mut make_service = router.into_make_service_with_connect_info::<SocketAddr>();
        let mut service = make_service.call(peer_addr).await.unwrap();

        assert!(Arc::ptr_eq(&shared_inner, &service.inner.inner));

        let response = service
            .call(
                Request::builder()
                    .uri("/peer")
                    .body(empty_incoming())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], peer_addr.to_string().as_bytes());
    }

    #[tokio::test]
    async fn method_router_services_route_by_method_without_path_matching() {
        let mut service = get(|| async { "get" }).post(|| async { "post" });
        let response = service
            .call(
                Request::builder()
                    .method(Method::POST)
                    .uri("/any/path")
                    .body(empty_incoming())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"post");

        let response = service
            .call(
                Request::builder()
                    .method(Method::DELETE)
                    .uri("/any/path")
                    .body(empty_incoming())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
        assert_eq!(
            response.headers().get(header::ALLOW),
            Some(&HeaderValue::from_static("GET, HEAD, POST"))
        );
    }

    #[tokio::test]
    async fn method_router_into_make_service_builds_request_services() {
        let mut make_service = get(|| async { "made" }).into_make_service();
        let mut service = make_service.call(()).await.unwrap();
        let response = service
            .call(
                Request::builder()
                    .method(Method::GET)
                    .uri("/standalone")
                    .body(empty_incoming())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"made");
    }

    #[tokio::test]
    async fn method_router_make_service_with_connect_info_injects_peer_addr() {
        let peer_addr = SocketAddr::from(([127, 0, 0, 1], 5321));
        let mut make_service =
            get(|ConnectInfo(addr): ConnectInfo<SocketAddr>| async move { addr.to_string() })
                .into_make_service_with_connect_info::<SocketAddr>();
        let mut service = make_service.call(peer_addr).await.unwrap();
        let response = service
            .call(
                Request::builder()
                    .method(Method::GET)
                    .uri("/peer")
                    .body(empty_incoming())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], peer_addr.to_string().as_bytes());
    }

    #[tokio::test]
    async fn matches_parameterized_paths() {
        let mut router = Router::new().route(
            "/users/{id}",
            get(|request: IncomingRequest| async move {
                request
                    .extensions()
                    .get::<RouteParams>()
                    .and_then(|params| params.get("id"))
                    .unwrap_or_default()
                    .to_string()
            }),
        );
        let response = router
            .call(
                Request::builder()
                    .method(Method::GET)
                    .uri("/users/42")
                    .body(empty_incoming())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"42");
    }

    #[tokio::test]
    async fn parameterized_route_deserializes_single_path_value() {
        let mut router = Router::new().route(
            "/users/{name}",
            get(|Path(name): Path<String>| async move { name }),
        );
        let response = router
            .call(
                Request::builder()
                    .uri("/users/roze%20team+core")
                    .body(empty_incoming())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"roze team+core");
    }

    #[tokio::test]
    async fn dispatch_preserves_uri_query_and_body_while_inserting_route_context() {
        let mut router = Router::new().route(
            "/users/{id}",
            post(|uri: Uri, Path(id): Path<u64>, body: String| async move {
                format!("{id}|{}|{body}", uri.query().unwrap_or_default())
            }),
        );
        let response = router
            .call(
                Request::builder()
                    .method(Method::POST)
                    .uri("/users/42?active=true")
                    .body(rest::full_body("payload"))
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"42|active=true|payload");
    }

    #[tokio::test]
    async fn optional_path_handler_supports_routes_with_and_without_captures() {
        async fn optional_path(path: Option<Path<String>>) -> String {
            path.map(|Path(value)| value)
                .unwrap_or_else(|| "none".to_string())
        }

        let mut router = Router::new()
            .route("/users/{name}", get(optional_path))
            .route("/healthz", get(optional_path));

        let captured = router
            .call(
                Request::builder()
                    .uri("/users/roze%20team")
                    .body(empty_incoming())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = captured.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"roze team");

        let uncaptured = router
            .call(
                Request::builder()
                    .uri("/healthz")
                    .body(empty_incoming())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = uncaptured.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"none");
    }

    #[tokio::test]
    async fn extracts_matched_path() {
        let mut router = Router::new().route(
            "/users/{id}",
            get(|matched_path: MatchedPath| async move { matched_path.as_str().to_string() }),
        );
        let response = router
            .call(
                Request::builder()
                    .method(Method::GET)
                    .uri("/users/42")
                    .body(empty_incoming())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"/users/{id}");
    }

    #[tokio::test]
    async fn matched_path_derefs_and_displays_as_str() {
        let mut router = Router::new().route(
            "/users/{id}",
            get(|matched_path: MatchedPath| async move {
                format!("{matched_path}:{}", matched_path.len())
            }),
        );
        let response = router
            .call(
                Request::builder()
                    .uri("/users/42")
                    .body(empty_incoming())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"/users/{id}:11");
    }

    #[tokio::test]
    async fn extracts_original_uri() {
        let mut router = Router::new().route(
            "/users/{id}",
            get(|original_uri: OriginalUri| async move { original_uri.to_string() }),
        );
        let response = router
            .call(
                Request::builder()
                    .method(Method::GET)
                    .uri("/users/42?include=roles")
                    .body(empty_incoming())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"/users/42?include=roles");
    }

    #[tokio::test]
    async fn layers_all_routes() {
        let mut router = Router::new()
            .route("/healthz", get(|| async { "ok" }))
            .layer(TestHeaderLayer);
        let response = router
            .call(
                Request::builder()
                    .uri("/healthz")
                    .body(empty_incoming())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(
            response.headers().get("x-roze-layer"),
            Some(&HeaderValue::from_static("yes"))
        );
    }

    #[tokio::test]
    async fn layers_method_router() {
        let mut router =
            Router::new().route("/healthz", get(|| async { "ok" }).layer(TestHeaderLayer));
        let response = router
            .call(
                Request::builder()
                    .uri("/healthz")
                    .body(empty_incoming())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(
            response.headers().get("x-roze-layer"),
            Some(&HeaderValue::from_static("yes"))
        );
    }

    #[tokio::test]
    async fn method_router_layer_applies_to_method_not_allowed_fallback() {
        let mut router = Router::new().route(
            "/healthz",
            get(|| async { "ok" })
                .method_not_allowed_fallback(|| async { "nope" })
                .layer(TestHeaderLayer),
        );
        let response = router
            .call(
                Request::builder()
                    .method(Method::POST)
                    .uri("/healthz")
                    .body(empty_incoming())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(
            response.headers().get("x-roze-layer"),
            Some(&HeaderValue::from_static("yes"))
        );
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"nope");
    }

    #[tokio::test]
    async fn method_router_route_layer_skips_method_not_allowed_fallback() {
        let mut router = Router::new().route(
            "/healthz",
            get(|| async { "ok" })
                .method_not_allowed_fallback(|| async { "nope" })
                .route_layer(TestHeaderLayer),
        );
        let matched = router
            .call(
                Request::builder()
                    .method(Method::GET)
                    .uri("/healthz")
                    .body(empty_incoming())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            matched.headers().get("x-roze-layer"),
            Some(&HeaderValue::from_static("yes"))
        );

        let fallback = router
            .call(
                Request::builder()
                    .method(Method::POST)
                    .uri("/healthz")
                    .body(empty_incoming())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(fallback.headers().get("x-roze-layer").is_none());
        let body = fallback.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"nope");
    }

    #[tokio::test]
    async fn route_layer_skips_fallback() {
        let mut router = Router::new()
            .route("/healthz", get(|| async { "ok" }))
            .route_layer(TestHeaderLayer);
        let matched = router
            .call(
                Request::builder()
                    .uri("/healthz")
                    .body(empty_incoming())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            matched.headers().get("x-roze-layer"),
            Some(&HeaderValue::from_static("yes"))
        );

        let missing = router
            .call(
                Request::builder()
                    .uri("/missing")
                    .body(empty_incoming())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
        assert!(missing.headers().get("x-roze-layer").is_none());
    }

    #[tokio::test]
    async fn router_route_layer_skips_method_not_allowed_fallback() {
        let mut router = Router::new()
            .route("/healthz", get(|| async { "ok" }))
            .method_not_allowed_fallback(|| async { "nope" })
            .route_layer(TestHeaderLayer);

        let matched = router
            .call(
                Request::builder()
                    .method(Method::GET)
                    .uri("/healthz")
                    .body(empty_incoming())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            matched.headers().get("x-roze-layer"),
            Some(&HeaderValue::from_static("yes"))
        );

        let fallback = router
            .call(
                Request::builder()
                    .method(Method::POST)
                    .uri("/healthz")
                    .body(empty_incoming())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(fallback.headers().get("x-roze-layer").is_none());
        let body = fallback.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"nope");
    }

    #[tokio::test]
    async fn router_layer_applies_to_method_not_allowed_fallback() {
        let mut router = Router::new()
            .route("/healthz", get(|| async { "ok" }))
            .method_not_allowed_fallback(|| async { "nope" })
            .layer(TestHeaderLayer);
        let response = router
            .call(
                Request::builder()
                    .method(Method::POST)
                    .uri("/healthz")
                    .body(empty_incoming())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(
            response.headers().get("x-roze-layer"),
            Some(&HeaderValue::from_static("yes"))
        );
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"nope");
    }

    #[test]
    #[should_panic(expected = "adding a route_layer before any routes is a no-op")]
    fn route_layer_panics_when_router_has_no_routes() {
        let _router = Router::new().route_layer(TestHeaderLayer);
    }

    #[test]
    #[should_panic(expected = "adding a route_layer before any method routes is a no-op")]
    fn method_router_route_layer_panics_when_method_router_has_no_routes() {
        let _router = MethodRouter::new().route_layer(TestHeaderLayer);
    }

    #[tokio::test]
    async fn route_layer_can_map_fallible_layer_errors() {
        let mut router = Router::new()
            .route("/healthz", get(|| async { "ok" }))
            .route_layer(crate::handle_error(FallibleLayer, |FallibleError| async {
                (StatusCode::SERVICE_UNAVAILABLE, "mapped-layer-error")
            }));
        let response = router
            .call(
                Request::builder()
                    .uri("/healthz")
                    .body(empty_incoming())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"mapped-layer-error");
    }

    #[tokio::test]
    async fn nests_routes_under_prefix() {
        let child = Router::new().route(
            "/users/{id}",
            get(|request: IncomingRequest| async move {
                request
                    .extensions()
                    .get::<RouteParams>()
                    .and_then(|params| params.get("id"))
                    .unwrap_or_default()
                    .to_string()
            }),
        );
        let mut router = Router::new().nest("/api", child);
        let response = router
            .call(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/users/42")
                    .body(empty_incoming())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"42");
    }

    #[tokio::test]
    async fn nested_routes_expose_full_matched_path() {
        let child = Router::new().route(
            "/users/{id}",
            get(|matched_path: MatchedPath| async move { matched_path.as_str().to_string() }),
        );
        let mut router = Router::new().nest("/api", child);
        let response = router
            .call(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/users/42")
                    .body(empty_incoming())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"/api/users/{id}");
    }

    #[tokio::test]
    async fn nested_routes_expose_composed_nested_path() {
        let leaf = Router::new().route(
            "/users/{id}",
            get(|nested_path: NestedPath| async move { nested_path.as_str().to_string() }),
        );
        let child = Router::new().nest("/v1", leaf);
        let mut router = Router::new().nest("/api", child);
        let response = router
            .call(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/v1/users/42")
                    .body(empty_incoming())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"/api/v1");
    }

    #[tokio::test]
    async fn nested_routes_strip_current_uri_and_preserve_external_context() {
        let child = Router::new().route(
            "/users/{id}",
            get(
                |uri: Uri,
                 original_uri: OriginalUri,
                 matched_path: MatchedPath,
                 nested_path: NestedPath| async move {
                    format!(
                        "{}|{}|{}|{}",
                        uri,
                        original_uri.0,
                        matched_path.as_str(),
                        nested_path.as_str()
                    )
                },
            ),
        );
        let mut router = Router::new().nest("/api", child);
        let response = router
            .call(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/users/42?active=1")
                    .body(empty_incoming())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(
            &body[..],
            b"/users/42?active=1|/api/users/42?active=1|/api/users/{id}|/api"
        );
    }

    #[tokio::test]
    async fn nested_capture_prefix_strips_the_actual_request_segments() {
        let child = Router::new().route(
            "/users/{id}",
            get(
                |uri: Uri, params: crate::RawPathParams, nested_path: NestedPath| async move {
                    format!(
                        "{}|{}|{}|{}",
                        uri,
                        params.get("tenant").unwrap_or_default(),
                        params.get("id").unwrap_or_default(),
                        nested_path.as_str()
                    )
                },
            ),
        );
        let mut router = Router::new().nest("/{tenant}", child);
        let response = router
            .call(
                Request::builder()
                    .method(Method::GET)
                    .uri("/acme/users/42?active=1")
                    .body(empty_incoming())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"/users/42?active=1|acme|42|/{tenant}");
    }

    #[tokio::test]
    async fn optional_original_uri_preserves_external_uri_when_nested() {
        let child = Router::new().route(
            "/users",
            get(|original_uri: Option<OriginalUri>| async move {
                original_uri
                    .map(|uri| uri.0.to_string())
                    .unwrap_or_else(|| "none".to_string())
            }),
        );
        let mut router = Router::new().nest("/api", child);
        let response = router
            .call(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/users?active=1")
                    .body(empty_incoming())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"/api/users?active=1");
    }

    #[tokio::test]
    async fn nested_method_fallback_receives_nested_uri_context() {
        let child = Router::new()
            .route("/users", get(|| async { "users" }))
            .method_not_allowed_fallback(
                |uri: Uri, original_uri: OriginalUri, nested_path: NestedPath| async move {
                    (
                        StatusCode::METHOD_NOT_ALLOWED,
                        format!("{}|{}|{}", uri, original_uri.0, nested_path.as_str()),
                    )
                },
            );
        let mut router = Router::new().nest("/api", child);
        let response = router
            .call(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/users?active=1")
                    .body(empty_incoming())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"/users?active=1|/api/users?active=1|/api");
    }

    #[tokio::test]
    async fn top_level_routes_have_no_nested_path() {
        let mut router = Router::new().route(
            "/users",
            get(|nested_path: Option<NestedPath>| async move {
                nested_path
                    .map(|path| path.as_str().to_string())
                    .unwrap_or_else(|| "none".to_string())
            }),
        );
        let response = router
            .call(
                Request::builder()
                    .method(Method::GET)
                    .uri("/users")
                    .body(empty_incoming())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"none");
    }

    #[tokio::test]
    async fn merged_top_level_method_does_not_inherit_nested_path() {
        let nested = Router::new().route(
            "/users",
            post(|nested_path: NestedPath| async move { nested_path.as_str().to_string() }),
        );
        let mut router = Router::new()
            .route(
                "/api/users",
                get(|nested_path: Option<NestedPath>| async move {
                    nested_path
                        .map(|path| path.as_str().to_string())
                        .unwrap_or_else(|| "none".to_string())
                }),
            )
            .nest("/api", nested);
        let response = router
            .call(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/users")
                    .body(empty_incoming())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"none");
    }

    #[tokio::test]
    async fn nested_routes_do_not_replace_parent_fallback() {
        let child = Router::new().route("/users", get(|| async { "users" }));
        let mut router = Router::new().nest("/api", child);
        let response = router
            .call(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/missing")
                    .body(empty_incoming())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn merges_routers() {
        let left = Router::new().route("/healthz", get(|| async { "health" }));
        let right = Router::new().route("/readyz", get(|| async { "ready" }));
        let mut router = left.merge(right);

        let health = router
            .call(
                Request::builder()
                    .method(Method::GET)
                    .uri("/healthz")
                    .body(empty_incoming())
                    .unwrap(),
            )
            .await
            .unwrap();
        let health_body = health.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&health_body[..], b"health");

        let ready = router
            .call(
                Request::builder()
                    .method(Method::GET)
                    .uri("/readyz")
                    .body(empty_incoming())
                    .unwrap(),
            )
            .await
            .unwrap();
        let ready_body = ready.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&ready_body[..], b"ready");
    }

    #[tokio::test]
    async fn merge_uses_other_custom_fallback_when_self_has_default() {
        let mut router = Router::new()
            .route("/healthz", get(|| async { "health" }))
            .merge(Router::new().fallback(|| async { "right fallback" }));
        let response = router
            .call(
                Request::builder()
                    .uri("/missing")
                    .body(empty_incoming())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"right fallback");
    }

    #[test]
    #[should_panic(expected = "cannot merge two routers that both have a fallback")]
    fn merge_panics_when_both_routers_have_custom_fallbacks() {
        let left = Router::new().fallback(|| async { "left" });
        let right = Router::new().fallback(|| async { "right" });
        let _router = left.merge(right);
    }

    #[tokio::test]
    async fn merges_methods_for_same_path() {
        let left = Router::new().route("/users", get(|| async { "list" }));
        let right = Router::new().route("/users", post(|| async { "create" }));
        let mut router = left.merge(right);

        let response = router
            .call(
                Request::builder()
                    .method(Method::POST)
                    .uri("/users")
                    .body(empty_incoming())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"create");
    }

    #[tokio::test]
    async fn method_router_converts_into_root_router() {
        let mut router: Router = get(|| async { "root" }).into();
        let response = router
            .call(
                Request::builder()
                    .method(Method::GET)
                    .uri("/")
                    .body(empty_incoming())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"root");
    }

    #[tokio::test]
    async fn merge_accepts_method_router_into_router() {
        let mut router = Router::new()
            .route("/healthz", get(|| async { "health" }))
            .merge(post(|| async { "root post" }));
        let response = router
            .call(
                Request::builder()
                    .method(Method::POST)
                    .uri("/")
                    .body(empty_incoming())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"root post");
    }

    #[test]
    #[should_panic(expected = "overlapping merged route")]
    fn merge_panics_on_overlapping_method() {
        let left = Router::new().route("/users", get(|| async { "left" }));
        let right = Router::new().route("/users", get(|| async { "right" }));
        let _router = left.merge(right);
    }

    #[test]
    #[should_panic(expected = "route path must not be empty")]
    fn route_panics_on_empty_path() {
        let _router = Router::new().route("", get(|| async { "empty" }));
    }

    #[test]
    #[should_panic(expected = "route path must start with `/`")]
    fn route_panics_on_path_without_leading_slash() {
        let _router = Router::new().route("users", get(|| async { "users" }));
    }

    #[test]
    #[should_panic(expected = "route path segments must not start with `:`")]
    fn route_panics_on_legacy_colon_capture() {
        let _router = Router::new().route("/users/:id", get(|| async { "users" }));
    }

    #[test]
    #[should_panic(expected = "route path segments must not start with `*`")]
    fn route_panics_on_legacy_wildcard_capture() {
        let _router = Router::new().route("/assets/*tail", get(|| async { "assets" }));
    }

    #[test]
    #[should_panic(expected = "route path must start with `/`")]
    fn nest_panics_on_prefix_without_leading_slash() {
        let _router = Router::new().nest(
            "api",
            Router::new().route("/users", get(|| async { "users" })),
        );
    }

    #[test]
    #[should_panic(expected = "nest prefix must not be root; use Router::merge instead")]
    fn nest_panics_on_root_prefix() {
        let _router = Router::new().nest(
            "/",
            Router::new().route("/users", get(|| async { "users" })),
        );
    }

    #[test]
    #[should_panic(expected = "nest prefix must not contain wildcard captures")]
    fn nest_panics_on_wildcard_prefix() {
        let _router = Router::new().nest(
            "/api/{*tail}",
            Router::new().route("/users", get(|| async { "users" })),
        );
    }

    #[test]
    #[should_panic(expected = "nest prefix must not contain wildcard captures")]
    fn nest_service_panics_on_wildcard_prefix() {
        let service = tower::service_fn(|_request: IncomingRequest| async {
            Ok::<_, Infallible>(rest::text_response(StatusCode::OK, "service"))
        });
        let _router = Router::new().nest_service("/api/{*tail}", service);
    }

    #[test]
    #[should_panic(expected = "nest prefix must not be root; use Router::fallback_service instead")]
    fn nest_service_panics_on_root_prefix() {
        let service = tower::service_fn(|_request: IncomingRequest| async {
            Ok::<_, Infallible>(rest::text_response(StatusCode::OK, "service"))
        });
        let _router = Router::new().nest_service("/", service);
    }

    #[tokio::test]
    async fn any_routes_all_methods() {
        let mut router = Router::new().route("/events", any(|| async { "accepted" }));
        let response = router
            .call(
                Request::builder()
                    .method(Method::PATCH)
                    .uri("/events")
                    .body(empty_incoming())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"accepted");
    }

    #[tokio::test]
    async fn any_routes_skip_allow_header() {
        let mut router = Router::new().route("/events", any(|| async { "accepted" }));
        let response = router
            .call(
                Request::builder()
                    .method(Method::DELETE)
                    .uri("/events")
                    .body(empty_incoming())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.headers().get(header::ALLOW).is_none());
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"accepted");
    }

    #[tokio::test]
    async fn any_service_routes_skip_allow_header() {
        let service = tower::service_fn(|request: IncomingRequest| async move {
            Ok::<_, Infallible>(rest::text_response(
                StatusCode::OK,
                format!("{} {}", request.method(), request.uri().path()),
            ))
        });
        let mut router = Router::new().route("/events", any_service(service));
        let response = router
            .call(
                Request::builder()
                    .method(Method::TRACE)
                    .uri("/events")
                    .body(empty_incoming())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.headers().get(header::ALLOW).is_none());
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"TRACE /events");
    }

    #[test]
    #[should_panic(expected = "MethodRouter::any_service cannot be used with Router")]
    fn any_service_panics_when_service_is_router() {
        let child = Router::new().route("/users", get(|| async { "users" }));
        let _router = any_service(child);
    }

    #[tokio::test]
    async fn head_uses_get_route_without_response_body() {
        let mut router = Router::new().route("/events", get(|| async { "accepted" }));
        let response = router
            .call(
                Request::builder()
                    .method(Method::HEAD)
                    .uri("/events")
                    .body(empty_incoming())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert!(body.is_empty());
    }

    #[tokio::test]
    async fn explicit_head_route_overrides_get_route() {
        let mut router = Router::new().route(
            "/events",
            get(|| async { "get" }).head(|| async { (StatusCode::CREATED, "head") }),
        );
        let response = router
            .call(
                Request::builder()
                    .method(Method::HEAD)
                    .uri("/events")
                    .body(empty_incoming())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::CREATED);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert!(body.is_empty());
    }

    #[tokio::test]
    async fn standalone_method_router_strips_explicit_head_body() {
        let mut method_router = head(|| async { "head" });
        let response = method_router
            .call(
                Request::builder()
                    .method(Method::HEAD)
                    .uri("/")
                    .body(empty_incoming())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(
            response.headers().get(header::CONTENT_LENGTH),
            Some(&HeaderValue::from_static("4"))
        );
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert!(body.is_empty());
    }

    #[tokio::test]
    async fn successful_connect_response_has_no_http_body_framing() {
        let mut method_router = connect(|| async {
            (
                [
                    (header::CONTENT_LENGTH, "6"),
                    (header::TRANSFER_ENCODING, "chunked"),
                ],
                "tunnel",
            )
        });
        let response = method_router
            .call(
                Request::builder()
                    .method(Method::CONNECT)
                    .uri("/")
                    .body(empty_incoming())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert!(response.status().is_success());
        assert!(response.headers().get(header::CONTENT_LENGTH).is_none());
        assert!(response.headers().get(header::TRANSFER_ENCODING).is_none());
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert!(body.is_empty());
    }

    #[tokio::test]
    async fn specific_method_overrides_any() {
        let mut router =
            Router::new().route("/events", any(|| async { "any" }).post(|| async { "post" }));
        let response = router
            .call(
                Request::builder()
                    .method(Method::POST)
                    .uri("/events")
                    .body(empty_incoming())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"post");
    }

    #[tokio::test]
    async fn top_level_service_helpers_route_services() {
        let service = tower::service_fn(|_request: IncomingRequest| async {
            Ok::<_, Infallible>(rest::text_response(StatusCode::OK, "service get"))
        });
        let mut router = Router::new().route("/svc", get_service(service));
        let response = router
            .call(
                Request::builder()
                    .method(Method::GET)
                    .uri("/svc")
                    .body(empty_incoming())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"service get");
    }

    #[test]
    #[should_panic(expected = "MethodRouter::on_service cannot be used with Router")]
    fn method_service_helper_panics_when_service_is_router() {
        let child = Router::new().route("/users", get(|| async { "users" }));
        let _router = get_service(child);
    }

    #[tokio::test]
    async fn route_service_routes_all_methods_to_service() {
        let service = tower::service_fn(|request: IncomingRequest| async move {
            Ok::<_, Infallible>(rest::text_response(
                StatusCode::OK,
                format!("service {}", request.method()),
            ))
        });
        let mut router = Router::new().route_service("/svc", service);
        let response = router
            .call(
                Request::builder()
                    .method(Method::PATCH)
                    .uri("/svc")
                    .body(empty_incoming())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"service PATCH");
    }

    #[test]
    #[should_panic(expected = "Router::route_service cannot be used with Router")]
    fn route_service_panics_when_service_is_router() {
        let child = Router::new().route("/users", get(|| async { "users" }));
        let _router = Router::new().route_service("/api", child);
    }

    #[tokio::test]
    async fn method_router_chains_service_helpers() {
        let post_service = tower::service_fn(|_request: IncomingRequest| async {
            Ok::<_, Infallible>(rest::text_response(StatusCode::OK, "service post"))
        });
        let mut router = Router::new().route(
            "/mixed",
            get(|| async { "handler get" }).post_service(post_service),
        );

        let get_response = router
            .call(
                Request::builder()
                    .method(Method::GET)
                    .uri("/mixed")
                    .body(empty_incoming())
                    .unwrap(),
            )
            .await
            .unwrap();
        let get_body = get_response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&get_body[..], b"handler get");

        let post_response = router
            .call(
                Request::builder()
                    .method(Method::POST)
                    .uri("/mixed")
                    .body(empty_incoming())
                    .unwrap(),
            )
            .await
            .unwrap();
        let post_body = post_response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes();
        assert_eq!(&post_body[..], b"service post");
    }

    #[tokio::test]
    async fn on_routes_multiple_methods_with_filter() {
        let mut router = Router::new().route(
            "/filtered",
            on(MethodFilter::GET | MethodFilter::DELETE, || async {
                "filtered"
            }),
        );

        for method in [Method::GET, Method::DELETE] {
            let response = router
                .call(
                    Request::builder()
                        .method(method)
                        .uri("/filtered")
                        .body(empty_incoming())
                        .unwrap(),
                )
                .await
                .unwrap();
            let body = response.into_body().collect().await.unwrap().to_bytes();
            assert_eq!(&body[..], b"filtered");
        }

        let missing_method = router
            .call(
                Request::builder()
                    .method(Method::POST)
                    .uri("/filtered")
                    .body(empty_incoming())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(missing_method.status(), StatusCode::METHOD_NOT_ALLOWED);
    }

    #[tokio::test]
    async fn on_service_routes_multiple_methods_with_filter() {
        let service = tower::service_fn(|request: IncomingRequest| async move {
            Ok::<_, Infallible>(rest::text_response(
                StatusCode::OK,
                request.method().as_str(),
            ))
        });
        let mut router = Router::new().route(
            "/filtered-service",
            on_service(MethodFilter::PUT | MethodFilter::PATCH, service),
        );

        let put = router
            .call(
                Request::builder()
                    .method(Method::PUT)
                    .uri("/filtered-service")
                    .body(empty_incoming())
                    .unwrap(),
            )
            .await
            .unwrap();
        let put_body = put.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&put_body[..], b"PUT");

        let patch = router
            .call(
                Request::builder()
                    .method(Method::PATCH)
                    .uri("/filtered-service")
                    .body(empty_incoming())
                    .unwrap(),
            )
            .await
            .unwrap();
        let patch_body = patch.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&patch_body[..], b"PATCH");
    }

    #[test]
    fn method_router_reports_method_filter() {
        let router = get(|| async { "get" }).post(|| async { "post" });
        let filter = router.method_filter().expect("method filter");
        assert!(filter.contains(MethodFilter::GET));
        assert!(filter.contains(MethodFilter::POST));
        assert!(!filter.contains(MethodFilter::DELETE));
        assert!(any(|| async { "any" }).method_filter().is_none());
        assert!(get(|| async { "get" })
            .method_not_allowed_fallback(|| async { "fallback" })
            .method_filter()
            .is_none());
    }

    #[test]
    fn method_filter_matches_and_expands_standard_methods() {
        let filter = MethodFilter::GET | MethodFilter::POST;
        assert_eq!(
            MethodFilter::from_method(&Method::GET),
            Some(MethodFilter::GET)
        );
        assert!(filter.matches(&Method::GET));
        assert!(filter.matches(&Method::POST));
        assert!(!filter.matches(&Method::DELETE));
        assert!(!filter.matches(&Method::HEAD));
        assert_eq!(filter.methods(), vec![Method::GET, Method::POST]);
        let brew = Method::from_bytes(b"BREW").unwrap();
        assert_eq!(MethodFilter::from_method(&brew), None);
        assert!(!filter.matches(&brew));
    }

    #[test]
    fn method_filter_supports_assigning_union() {
        let mut filter = MethodFilter::GET;
        filter |= MethodFilter::DELETE;
        assert!(filter.contains(MethodFilter::GET));
        assert!(filter.contains(MethodFilter::DELETE));
        assert_eq!(filter.methods(), vec![Method::GET, Method::DELETE]);
    }

    #[test]
    fn method_filter_supports_intersection_and_difference() {
        let read_methods = MethodFilter::GET | MethodFilter::HEAD | MethodFilter::OPTIONS;
        let cacheable_methods = MethodFilter::GET | MethodFilter::HEAD;
        let mut overlap = read_methods & cacheable_methods;

        assert!(read_methods.intersects(cacheable_methods));
        assert!(!MethodFilter::POST.intersects(cacheable_methods));
        assert_eq!(overlap.methods(), vec![Method::GET, Method::HEAD]);

        overlap -= MethodFilter::HEAD;
        assert_eq!(overlap.methods(), vec![Method::GET]);

        let mut write_methods = MethodFilter::POST | MethodFilter::PUT | MethodFilter::DELETE;
        write_methods &= MethodFilter::POST | MethodFilter::PATCH;
        assert_eq!(write_methods.methods(), vec![Method::POST]);

        let without_delete =
            (MethodFilter::POST | MethodFilter::PUT | MethodFilter::DELETE) - MethodFilter::DELETE;
        assert_eq!(without_delete.methods(), vec![Method::POST, Method::PUT]);
        assert_eq!(
            (MethodFilter::GET | MethodFilter::POST)
                .without(MethodFilter::POST)
                .methods(),
            vec![Method::GET]
        );
    }

    #[test]
    fn method_filter_supports_all_and_complement() {
        assert_eq!(
            MethodFilter::ALL.methods(),
            vec![
                Method::GET,
                Method::POST,
                Method::PUT,
                Method::PATCH,
                Method::DELETE,
                Method::HEAD,
                Method::OPTIONS,
                Method::TRACE,
                Method::CONNECT,
            ]
        );

        let non_read_methods = !(MethodFilter::GET | MethodFilter::HEAD);
        assert!(!non_read_methods.matches(&Method::GET));
        assert!(!non_read_methods.matches(&Method::HEAD));
        assert!(non_read_methods.matches(&Method::POST));
        assert!(non_read_methods.matches(&Method::CONNECT));
        assert_eq!(
            MethodFilter::POST.complement(),
            MethodFilter::ALL - MethodFilter::POST
        );
    }

    #[tokio::test]
    async fn options_helper_routes_options_method() {
        let mut router = Router::new().route("/events", options(|| async { "options" }));
        let response = router
            .call(
                Request::builder()
                    .method(Method::OPTIONS)
                    .uri("/events")
                    .body(empty_incoming())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"options");
    }

    #[tokio::test]
    async fn nests_service_under_prefix() {
        let service = tower::service_fn(|request: IncomingRequest| async move {
            let current_uri = request
                .uri()
                .path_and_query()
                .map(ToString::to_string)
                .unwrap_or_else(|| request.uri().path().to_string());
            let original_uri = request
                .extensions()
                .get::<OriginalUri>()
                .map(|uri| uri.0.to_string())
                .unwrap_or_default();
            let nested_path = request
                .extensions()
                .get::<NestedPath>()
                .map(|path| path.as_str())
                .unwrap_or_default();
            Ok::<_, Infallible>(rest::text_response(
                StatusCode::OK,
                format!("{current_uri}|{original_uri}|{nested_path}"),
            ))
        });
        let mut router = Router::new().nest_service("/proxy", service);
        let response = router
            .call(
                Request::builder()
                    .method(Method::DELETE)
                    .uri("/proxy/users/42?active=1")
                    .body(empty_incoming())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(
            &body[..],
            b"/users/42?active=1|/proxy/users/42?active=1|/proxy"
        );
    }

    #[tokio::test]
    async fn nested_services_compose_nested_path() {
        let service = tower::service_fn(|request: IncomingRequest| async move {
            let nested_path = request
                .extensions()
                .get::<NestedPath>()
                .map(|path| path.as_str())
                .unwrap_or_default();
            Ok::<_, Infallible>(rest::text_response(StatusCode::OK, nested_path.to_string()))
        });
        let child = Router::new().nest_service("/v1", service);
        let mut router = Router::new().nest("/api", child);
        let response = router
            .call(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/v1/users")
                    .body(empty_incoming())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"/api/v1");
    }

    #[tokio::test]
    async fn nest_service_matches_prefix_and_trailing_slash() {
        let service = tower::service_fn(|request: IncomingRequest| async move {
            Ok::<_, Infallible>(rest::text_response(
                StatusCode::OK,
                request.uri().path().to_string(),
            ))
        });
        let mut router = Router::new().nest_service("/proxy", service);

        let response = router
            .call(
                Request::builder()
                    .method(Method::POST)
                    .uri("/proxy")
                    .body(empty_incoming())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"/");

        let response = router
            .call(
                Request::builder()
                    .method(Method::POST)
                    .uri("/proxy/")
                    .body(empty_incoming())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"/");
    }

    fn empty_incoming() -> crate::rest::Body {
        crate::rest::empty_body()
    }

    #[derive(Clone)]
    struct TestHeaderLayer;

    impl Layer<BoxCloneSyncService<IncomingRequest, HttpResponse, Infallible>> for TestHeaderLayer {
        type Service = TestHeaderService;

        fn layer(
            &self,
            inner: BoxCloneSyncService<IncomingRequest, HttpResponse, Infallible>,
        ) -> Self::Service {
            TestHeaderService { inner }
        }
    }

    #[derive(Clone)]
    struct TestHeaderService {
        inner: BoxCloneSyncService<IncomingRequest, HttpResponse, Infallible>,
    }

    impl Service<IncomingRequest> for TestHeaderService {
        type Response = HttpResponse;
        type Error = Infallible;
        type Future = BoxFuture;

        fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
            self.inner.poll_ready(cx)
        }

        fn call(&mut self, request: IncomingRequest) -> Self::Future {
            let mut inner = self.inner.clone();
            Box::pin(async move {
                let mut response = inner.call(request).await?;
                response
                    .headers_mut()
                    .insert("x-roze-layer", HeaderValue::from_static("yes"));
                Ok(response)
            })
        }
    }

    #[derive(Clone)]
    struct FallibleLayer;

    impl Layer<BoxCloneSyncService<IncomingRequest, HttpResponse, Infallible>> for FallibleLayer {
        type Service = FallibleService;

        fn layer(
            &self,
            inner: BoxCloneSyncService<IncomingRequest, HttpResponse, Infallible>,
        ) -> Self::Service {
            FallibleService { inner }
        }
    }

    #[derive(Clone)]
    struct FallibleService {
        inner: BoxCloneSyncService<IncomingRequest, HttpResponse, Infallible>,
    }

    #[derive(Debug)]
    struct FallibleError;

    impl Service<IncomingRequest> for FallibleService {
        type Response = HttpResponse;
        type Error = FallibleError;
        type Future = Pin<Box<dyn Future<Output = Result<HttpResponse, FallibleError>> + Send>>;

        fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
            self.inner.poll_ready(cx).map_err(|_| FallibleError)
        }

        fn call(&mut self, _request: IncomingRequest) -> Self::Future {
            Box::pin(async { Err(FallibleError) })
        }
    }
}
