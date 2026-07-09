use std::{
    collections::BTreeMap,
    convert::Infallible,
    fmt,
    future::{ready, Future, Ready},
    ops::{BitAnd, BitAndAssign, BitOr, BitOrAssign, Deref, Not, Sub, SubAssign},
    pin::Pin,
    task::{Context, Poll},
};

use http::{header, Method, StatusCode};
use matchit::Router as MatchRouter;
use tower::{util::BoxCloneService, Layer, Service, ServiceExt};

use crate::{
    extract::{ConnectInfo, FromRef, OriginalUri},
    handler::Handler,
    rest::{self, HttpResponse, IncomingRequest, SharedService},
};

type BoxFuture = Pin<Box<dyn Future<Output = Result<HttpResponse, Infallible>> + Send>>;

#[derive(Clone)]
pub struct Router {
    routes: MatchRouter<usize>,
    route_groups: Vec<RouteGroup>,
    path_index: BTreeMap<String, usize>,
    fallback: BoxCloneService<IncomingRequest, HttpResponse, Infallible>,
    has_custom_fallback: bool,
    method_not_allowed_fallback: Option<BoxCloneService<IncomingRequest, HttpResponse, Infallible>>,
}

#[derive(Clone)]
struct RouteGroup {
    path: String,
    routes: Vec<Route>,
    method_not_allowed_fallback: Option<BoxCloneService<IncomingRequest, HttpResponse, Infallible>>,
}

#[derive(Clone)]
struct Route {
    method: Option<Method>,
    service: BoxCloneService<IncomingRequest, HttpResponse, Infallible>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MethodFilter(u16);

impl MethodFilter {
    pub const GET: Self = Self(1 << 0);
    pub const POST: Self = Self(1 << 1);
    pub const PUT: Self = Self(1 << 2);
    pub const PATCH: Self = Self(1 << 3);
    pub const DELETE: Self = Self(1 << 4);
    pub const HEAD: Self = Self(1 << 5);
    pub const OPTIONS: Self = Self(1 << 6);
    pub const TRACE: Self = Self(1 << 7);
    pub const CONNECT: Self = Self(1 << 8);
    pub const ALL: Self = Self(
        Self::GET.0
            | Self::POST.0
            | Self::PUT.0
            | Self::PATCH.0
            | Self::DELETE.0
            | Self::HEAD.0
            | Self::OPTIONS.0
            | Self::TRACE.0
            | Self::CONNECT.0,
    );

    pub fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    pub fn intersects(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }

    pub fn without(self, other: Self) -> Self {
        Self(self.0 & !other.0)
    }

    pub fn complement(self) -> Self {
        Self::ALL.without(self)
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub fn from_method(method: &Method) -> Option<Self> {
        match *method {
            Method::GET => Some(Self::GET),
            Method::POST => Some(Self::POST),
            Method::PUT => Some(Self::PUT),
            Method::PATCH => Some(Self::PATCH),
            Method::DELETE => Some(Self::DELETE),
            Method::HEAD => Some(Self::HEAD),
            Method::OPTIONS => Some(Self::OPTIONS),
            Method::TRACE => Some(Self::TRACE),
            Method::CONNECT => Some(Self::CONNECT),
            _ => None,
        }
    }

    pub fn matches(self, method: &Method) -> bool {
        Self::from_method(method).is_some_and(|filter| self.contains(filter))
    }

    pub fn methods(self) -> Vec<Method> {
        [
            (Self::GET, Method::GET),
            (Self::POST, Method::POST),
            (Self::PUT, Method::PUT),
            (Self::PATCH, Method::PATCH),
            (Self::DELETE, Method::DELETE),
            (Self::HEAD, Method::HEAD),
            (Self::OPTIONS, Method::OPTIONS),
            (Self::TRACE, Method::TRACE),
            (Self::CONNECT, Method::CONNECT),
        ]
        .into_iter()
        .filter_map(|(filter, method)| self.contains(filter).then_some(method))
        .collect()
    }
}

impl BitOr for MethodFilter {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

impl BitOrAssign for MethodFilter {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

impl BitAnd for MethodFilter {
    type Output = Self;

    fn bitand(self, rhs: Self) -> Self::Output {
        Self(self.0 & rhs.0)
    }
}

impl BitAndAssign for MethodFilter {
    fn bitand_assign(&mut self, rhs: Self) {
        self.0 &= rhs.0;
    }
}

impl Sub for MethodFilter {
    type Output = Self;

    fn sub(self, rhs: Self) -> Self::Output {
        self.without(rhs)
    }
}

impl SubAssign for MethodFilter {
    fn sub_assign(&mut self, rhs: Self) {
        self.0 &= !rhs.0;
    }
}

impl Not for MethodFilter {
    type Output = Self;

    fn not(self) -> Self::Output {
        self.complement()
    }
}

pub enum MethodSelection {
    Method(Method),
    Filter(MethodFilter),
}

impl MethodSelection {
    fn methods(self) -> Vec<Method> {
        match self {
            Self::Method(method) => vec![method],
            Self::Filter(filter) => filter.methods(),
        }
    }
}

impl From<Method> for MethodSelection {
    fn from(method: Method) -> Self {
        Self::Method(method)
    }
}

impl From<MethodFilter> for MethodSelection {
    fn from(filter: MethodFilter) -> Self {
        Self::Filter(filter)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RouteParams(Vec<(String, String)>);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MatchedPath(String);

impl MatchedPath {
    pub fn new(path: impl Into<String>) -> Self {
        Self(path.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Deref for MatchedPath {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        self.as_str()
    }
}

impl fmt::Display for MatchedPath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl RouteParams {
    pub fn from_pairs<I>(pairs: I) -> Self
    where
        I: IntoIterator<Item = (String, String)>,
    {
        Self(pairs.into_iter().collect())
    }

    pub fn get(&self, name: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.0
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn encoded(&self) -> String {
        self.0
            .iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect::<Vec<_>>()
            .join("&")
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
            .field("route_count", &self.route_groups.len())
            .field(
                "routes",
                &self
                    .route_groups
                    .iter()
                    .map(|group| group.path.as_str())
                    .collect::<Vec<_>>(),
            )
            .field("has_custom_fallback", &self.has_custom_fallback)
            .field(
                "has_method_not_allowed_fallback",
                &self.method_not_allowed_fallback.is_some(),
            )
            .finish()
    }
}

impl Router {
    pub fn new() -> Self {
        Self {
            routes: MatchRouter::new(),
            route_groups: Vec::new(),
            path_index: BTreeMap::new(),
            fallback: default_not_found_service(),
            has_custom_fallback: false,
            method_not_allowed_fallback: None,
        }
    }

    pub fn route(mut self, path: impl Into<String>, method_router: MethodRouter) -> Self {
        let path = normalize_path(path.into());
        let group_index = self.ensure_route_group(path);

        let group = &mut self.route_groups[group_index];
        for endpoint in method_router.endpoints {
            if group
                .routes
                .iter()
                .any(|route| routes_overlap(&route.method, &endpoint.method))
            {
                panic!(
                    "overlapping method route for {}",
                    method_label(&endpoint.method)
                );
            }
            group.routes.push(Route {
                method: endpoint.method,
                service: endpoint.service,
            });
        }
        if let Some(fallback) = method_router.method_not_allowed_fallback {
            if group.method_not_allowed_fallback.is_some() {
                panic!("overlapping method-not-allowed fallback for {}", group.path);
            }
            group.method_not_allowed_fallback = Some(fallback);
        }
        self
    }

    pub fn nest(mut self, prefix: impl Into<String>, router: Router) -> Self {
        let prefix = normalize_nest_prefix(prefix.into());
        for (path, group_index) in router.path_index {
            let nested_path = join_paths(&prefix, &path);
            let group = router.route_groups[group_index].clone();
            self.insert_group_routes(nested_path, group, "nested");
        }
        self
    }

    pub fn merge<R>(mut self, router: R) -> Self
    where
        R: Into<Router>,
    {
        let router = router.into();
        if self.has_custom_fallback && router.has_custom_fallback {
            panic!("cannot merge two routers that both have a fallback");
        }
        if !self.has_custom_fallback && router.has_custom_fallback {
            self.fallback = router.fallback.clone();
            self.has_custom_fallback = true;
        }
        for (path, group_index) in router.path_index {
            let group = router.route_groups[group_index].clone();
            self.insert_group_routes(path, group, "merged");
        }
        self
    }

    pub fn nest_service<S>(self, prefix: impl Into<String>, service: S) -> Self
    where
        S: Service<IncomingRequest, Response = HttpResponse, Error = Infallible>
            + Clone
            + Send
            + 'static,
        S::Future: Send + 'static,
    {
        let prefix = normalize_nest_prefix(prefix.into());
        self.route(prefix.clone(), any_service(service.clone()))
            .route(format!("{prefix}/{{*tail}}"), any_service(service))
    }

    pub fn route_service<S>(self, path: impl Into<String>, service: S) -> Self
    where
        S: Service<IncomingRequest, Response = HttpResponse, Error = Infallible>
            + Clone
            + Send
            + 'static,
        S::Future: Send + 'static,
    {
        self.route(path, any_service(service))
    }

    fn route_method_service<S>(self, method: Method, path: impl Into<String>, service: S) -> Self
    where
        S: Service<IncomingRequest, Response = HttpResponse, Error = Infallible>
            + Clone
            + Send
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

    pub fn get<H, T>(self, path: impl Into<String>, handler: H) -> Self
    where
        H: Handler<T>,
    {
        self.route_handler(Method::GET, path, handler)
    }

    pub fn post<H, T>(self, path: impl Into<String>, handler: H) -> Self
    where
        H: Handler<T>,
    {
        self.route_handler(Method::POST, path, handler)
    }

    pub fn put<H, T>(self, path: impl Into<String>, handler: H) -> Self
    where
        H: Handler<T>,
    {
        self.route_handler(Method::PUT, path, handler)
    }

    pub fn patch<H, T>(self, path: impl Into<String>, handler: H) -> Self
    where
        H: Handler<T>,
    {
        self.route_handler(Method::PATCH, path, handler)
    }

    pub fn delete<H, T>(self, path: impl Into<String>, handler: H) -> Self
    where
        H: Handler<T>,
    {
        self.route_handler(Method::DELETE, path, handler)
    }

    pub fn head<H, T>(self, path: impl Into<String>, handler: H) -> Self
    where
        H: Handler<T>,
    {
        self.route_handler(Method::HEAD, path, handler)
    }

    pub fn options<H, T>(self, path: impl Into<String>, handler: H) -> Self
    where
        H: Handler<T>,
    {
        self.route_handler(Method::OPTIONS, path, handler)
    }

    pub fn trace<H, T>(self, path: impl Into<String>, handler: H) -> Self
    where
        H: Handler<T>,
    {
        self.route_handler(Method::TRACE, path, handler)
    }

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
            + 'static,
        S::Future: Send + 'static,
    {
        self.fallback = service.boxed_clone();
        self.has_custom_fallback = true;
        self
    }

    pub fn fallback<H, T>(self, handler: H) -> Self
    where
        H: Handler<T>,
    {
        let mut router = self;
        router.fallback = handler.into_service();
        router.has_custom_fallback = true;
        router
    }

    pub fn fallback_service<S>(self, service: S) -> Self
    where
        S: Service<IncomingRequest, Response = HttpResponse, Error = Infallible>
            + Clone
            + Send
            + 'static,
        S::Future: Send + 'static,
    {
        self.fallback_route_service(service)
    }

    pub fn reset_fallback(mut self) -> Self {
        self.fallback = default_not_found_service();
        self.has_custom_fallback = false;
        self
    }

    pub fn method_not_allowed_fallback<H, T>(self, handler: H) -> Self
    where
        H: Handler<T>,
    {
        self.method_not_allowed_fallback_service(handler.into_service())
    }

    pub fn method_not_allowed_fallback_service<S>(mut self, service: S) -> Self
    where
        S: Service<IncomingRequest, Response = HttpResponse, Error = Infallible>
            + Clone
            + Send
            + 'static,
        S::Future: Send + 'static,
    {
        let fallback = service.boxed_clone();
        self.method_not_allowed_fallback = Some(fallback.clone());
        for group in &mut self.route_groups {
            group.method_not_allowed_fallback = Some(fallback.clone());
        }
        self
    }

    pub fn has_routes(&self) -> bool {
        !self.route_groups.is_empty()
    }

    pub fn as_service(&mut self) -> RouterAsService<'_> {
        RouterAsService { router: self }
    }

    pub fn into_service(self) -> RouterIntoService {
        RouterIntoService { router: self }
    }

    pub fn into_make_service(self) -> IntoMakeService {
        IntoMakeService { router: self }
    }

    pub fn into_make_service_with_connect_info<T>(self) -> IntoMakeServiceWithConnectInfo<T> {
        IntoMakeServiceWithConnectInfo {
            router: self,
            _marker: std::marker::PhantomData,
        }
    }

    pub fn with_state<T>(self, state: T) -> Self
    where
        T: Clone + Send + Sync + 'static,
    {
        self.layer(StateLayer { state })
    }

    pub fn with_state_from_ref<Outer, Inner>(self, state: Outer) -> Self
    where
        Outer: Clone + Send + Sync + 'static,
        Inner: FromRef<Outer> + Clone + Send + Sync + 'static,
    {
        self.layer(StateFromRefLayer::<Outer, Inner>::new(state))
    }

    fn ensure_route_group(&mut self, path: String) -> usize {
        match self.path_index.get(&path).copied() {
            Some(index) => index,
            None => {
                let index = self.route_groups.len();
                self.routes
                    .insert(path.clone(), index)
                    .expect("invalid route path");
                self.route_groups.push(RouteGroup {
                    path: path.clone(),
                    routes: Vec::new(),
                    method_not_allowed_fallback: self.method_not_allowed_fallback.clone(),
                });
                self.path_index.insert(path, index);
                index
            }
        }
    }

    fn insert_group_routes(&mut self, path: String, group: RouteGroup, action: &str) {
        let RouteGroup {
            routes,
            method_not_allowed_fallback,
            ..
        } = group;
        let group_index = self.ensure_route_group(path);
        for route in routes {
            if self.route_groups[group_index]
                .routes
                .iter()
                .any(|existing| routes_overlap(&existing.method, &route.method))
            {
                panic!(
                    "overlapping {action} route for {}",
                    method_label(&route.method)
                );
            }
            self.route_groups[group_index].routes.push(route);
        }
        if let Some(fallback) = method_not_allowed_fallback {
            if self.route_groups[group_index]
                .method_not_allowed_fallback
                .is_some()
            {
                panic!("overlapping {action} method-not-allowed fallback");
            }
            self.route_groups[group_index].method_not_allowed_fallback = Some(fallback);
        }
    }

    pub fn layer<L>(mut self, layer: L) -> Self
    where
        L: Layer<BoxCloneService<IncomingRequest, HttpResponse, Infallible>>
            + Clone
            + Send
            + 'static,
        L::Service: Service<IncomingRequest, Response = HttpResponse, Error = Infallible>
            + Clone
            + Send
            + 'static,
        <L::Service as Service<IncomingRequest>>::Future: Send + 'static,
    {
        self.layer_routes(layer.clone());
        self.fallback = layer_service(layer, self.fallback);
        self
    }

    pub fn route_layer<L>(mut self, layer: L) -> Self
    where
        L: Layer<BoxCloneService<IncomingRequest, HttpResponse, Infallible>>
            + Clone
            + Send
            + 'static,
        L::Service: Service<IncomingRequest, Response = HttpResponse, Error = Infallible>
            + Clone
            + Send
            + 'static,
        <L::Service as Service<IncomingRequest>>::Future: Send + 'static,
    {
        if self.route_groups.is_empty() {
            panic!(
                "adding a route_layer before any routes is a no-op; add the routes you want the layer to apply to first"
            );
        }
        self.layer_routes(layer);
        self
    }

    fn layer_routes<L>(&mut self, layer: L)
    where
        L: Layer<BoxCloneService<IncomingRequest, HttpResponse, Infallible>>
            + Clone
            + Send
            + 'static,
        L::Service: Service<IncomingRequest, Response = HttpResponse, Error = Infallible>
            + Clone
            + Send
            + 'static,
        <L::Service as Service<IncomingRequest>>::Future: Send + 'static,
    {
        for group in &mut self.route_groups {
            for route in &mut group.routes {
                route.service = layer_service(layer.clone(), route.service.clone());
            }
            if let Some(fallback) = group.method_not_allowed_fallback.take() {
                group.method_not_allowed_fallback = Some(layer_service(layer.clone(), fallback));
            }
        }
    }
}

pub struct RouterAsService<'a> {
    router: &'a mut Router,
}

impl Service<IncomingRequest> for RouterAsService<'_> {
    type Response = HttpResponse;
    type Error = Infallible;
    type Future = BoxFuture;

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
pub struct RouterIntoService {
    router: Router,
}

impl Service<IncomingRequest> for RouterIntoService {
    type Response = HttpResponse;
    type Error = Infallible;
    type Future = BoxFuture;

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

#[derive(Clone)]
pub struct IntoMakeService {
    router: Router,
}

impl<Target> Service<Target> for IntoMakeService {
    type Response = Router;
    type Error = Infallible;
    type Future = Ready<Result<Router, Infallible>>;

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
pub struct IntoMakeServiceWithConnectInfo<T> {
    router: Router,
    _marker: std::marker::PhantomData<fn() -> T>,
}

impl<T, Target> Service<Target> for IntoMakeServiceWithConnectInfo<T>
where
    T: Clone + Send + Sync + 'static,
    Target: Into<T>,
{
    type Response = Router;
    type Error = Infallible;
    type Future = Ready<Result<Router, Infallible>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, target: Target) -> Self::Future {
        ready(Ok(self
            .router
            .clone()
            .with_state(ConnectInfo(target.into()))))
    }
}

impl<T> fmt::Debug for IntoMakeServiceWithConnectInfo<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IntoMakeServiceWithConnectInfo")
            .finish_non_exhaustive()
    }
}

impl Service<IncomingRequest> for Router {
    type Response = HttpResponse;
    type Error = Infallible;
    type Future = BoxFuture;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, mut request: IncomingRequest) -> Self::Future {
        if request.extensions().get::<OriginalUri>().is_none() {
            let original_uri = request.uri().clone();
            request.extensions_mut().insert(OriginalUri(original_uri));
        }
        let strip_body = *request.method() == Method::HEAD;
        let path = request.uri().path().to_string();
        let matched = self.routes.at(&path).ok();
        let mut service = match matched {
            Some(matched) => {
                let route_params = RouteParams(
                    matched
                        .params
                        .iter()
                        .map(|(key, value)| (key.to_string(), value.to_string()))
                        .collect(),
                );
                request.extensions_mut().insert(route_params);
                let group = &self.route_groups[*matched.value];
                request
                    .extensions_mut()
                    .insert(MatchedPath::new(group.path.clone()));
                group
                    .routes
                    .iter()
                    .find(|route| route.method.as_ref() == Some(request.method()))
                    .or_else(|| {
                        (*request.method() == Method::HEAD).then(|| {
                            group
                                .routes
                                .iter()
                                .find(|route| route.method.as_ref() == Some(&Method::GET))
                        })?
                    })
                    .or_else(|| group.routes.iter().find(|route| route.method.is_none()))
                    .map(|route| route.service.clone())
                    .unwrap_or_else(|| {
                        group
                            .method_not_allowed_fallback
                            .clone()
                            .unwrap_or_else(|| method_not_allowed_service(allow_header(group)))
                    })
            }
            None => self.fallback.clone(),
        };
        Box::pin(async move {
            let response = service
                .ready()
                .await
                .expect("infallible service")
                .call(request)
                .await?;
            if strip_body {
                Ok(response.map(|_| rest::empty_body()))
            } else {
                Ok(response)
            }
        })
    }
}

#[derive(Clone)]
pub struct MethodRouter {
    endpoints: Vec<MethodEndpoint>,
    method_not_allowed_fallback: Option<BoxCloneService<IncomingRequest, HttpResponse, Infallible>>,
}

#[derive(Clone)]
struct MethodEndpoint {
    method: Option<Method>,
    service: BoxCloneService<IncomingRequest, HttpResponse, Infallible>,
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
            method_not_allowed_fallback: None,
        }
    }

    pub fn on<M, H, T>(self, method: M, handler: H) -> Self
    where
        M: Into<MethodSelection>,
        H: Handler<T>,
    {
        self.on_service(method, handler.into_service())
    }

    pub fn on_service<M, S>(mut self, method: M, service: S) -> Self
    where
        M: Into<MethodSelection>,
        S: Service<IncomingRequest, Response = HttpResponse, Error = Infallible>
            + Clone
            + Send
            + 'static,
        S::Future: Send + 'static,
    {
        let methods = method.into().methods();
        if methods.is_empty() {
            panic!("method filter must not be empty");
        }
        let service = service.boxed_clone();
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
        self
    }

    pub fn any<H, T>(self, handler: H) -> Self
    where
        H: Handler<T>,
    {
        self.any_service(handler.into_service())
    }

    pub fn any_service<S>(mut self, service: S) -> Self
    where
        S: Service<IncomingRequest, Response = HttpResponse, Error = Infallible>
            + Clone
            + Send
            + 'static,
        S::Future: Send + 'static,
    {
        if self
            .endpoints
            .iter()
            .any(|endpoint| endpoint.method.is_none())
        {
            panic!("overlapping method route for any");
        }
        self.endpoints.push(MethodEndpoint {
            method: None,
            service: service.boxed_clone(),
        });
        self
    }

    pub fn get<H, T>(self, handler: H) -> Self
    where
        H: Handler<T>,
    {
        self.on(Method::GET, handler)
    }

    pub fn get_service<S>(self, service: S) -> Self
    where
        S: Service<IncomingRequest, Response = HttpResponse, Error = Infallible>
            + Clone
            + Send
            + 'static,
        S::Future: Send + 'static,
    {
        self.on_service(Method::GET, service)
    }

    pub fn post<H, T>(self, handler: H) -> Self
    where
        H: Handler<T>,
    {
        self.on(Method::POST, handler)
    }

    pub fn post_service<S>(self, service: S) -> Self
    where
        S: Service<IncomingRequest, Response = HttpResponse, Error = Infallible>
            + Clone
            + Send
            + 'static,
        S::Future: Send + 'static,
    {
        self.on_service(Method::POST, service)
    }

    pub fn put<H, T>(self, handler: H) -> Self
    where
        H: Handler<T>,
    {
        self.on(Method::PUT, handler)
    }

    pub fn put_service<S>(self, service: S) -> Self
    where
        S: Service<IncomingRequest, Response = HttpResponse, Error = Infallible>
            + Clone
            + Send
            + 'static,
        S::Future: Send + 'static,
    {
        self.on_service(Method::PUT, service)
    }

    pub fn patch<H, T>(self, handler: H) -> Self
    where
        H: Handler<T>,
    {
        self.on(Method::PATCH, handler)
    }

    pub fn patch_service<S>(self, service: S) -> Self
    where
        S: Service<IncomingRequest, Response = HttpResponse, Error = Infallible>
            + Clone
            + Send
            + 'static,
        S::Future: Send + 'static,
    {
        self.on_service(Method::PATCH, service)
    }

    pub fn delete<H, T>(self, handler: H) -> Self
    where
        H: Handler<T>,
    {
        self.on(Method::DELETE, handler)
    }

    pub fn delete_service<S>(self, service: S) -> Self
    where
        S: Service<IncomingRequest, Response = HttpResponse, Error = Infallible>
            + Clone
            + Send
            + 'static,
        S::Future: Send + 'static,
    {
        self.on_service(Method::DELETE, service)
    }

    pub fn head<H, T>(self, handler: H) -> Self
    where
        H: Handler<T>,
    {
        self.on(Method::HEAD, handler)
    }

    pub fn head_service<S>(self, service: S) -> Self
    where
        S: Service<IncomingRequest, Response = HttpResponse, Error = Infallible>
            + Clone
            + Send
            + 'static,
        S::Future: Send + 'static,
    {
        self.on_service(Method::HEAD, service)
    }

    pub fn options<H, T>(self, handler: H) -> Self
    where
        H: Handler<T>,
    {
        self.on(Method::OPTIONS, handler)
    }

    pub fn options_service<S>(self, service: S) -> Self
    where
        S: Service<IncomingRequest, Response = HttpResponse, Error = Infallible>
            + Clone
            + Send
            + 'static,
        S::Future: Send + 'static,
    {
        self.on_service(Method::OPTIONS, service)
    }

    pub fn trace<H, T>(self, handler: H) -> Self
    where
        H: Handler<T>,
    {
        self.on(Method::TRACE, handler)
    }

    pub fn trace_service<S>(self, service: S) -> Self
    where
        S: Service<IncomingRequest, Response = HttpResponse, Error = Infallible>
            + Clone
            + Send
            + 'static,
        S::Future: Send + 'static,
    {
        self.on_service(Method::TRACE, service)
    }

    pub fn connect<H, T>(self, handler: H) -> Self
    where
        H: Handler<T>,
    {
        self.on(Method::CONNECT, handler)
    }

    pub fn connect_service<S>(self, service: S) -> Self
    where
        S: Service<IncomingRequest, Response = HttpResponse, Error = Infallible>
            + Clone
            + Send
            + 'static,
        S::Future: Send + 'static,
    {
        self.on_service(Method::CONNECT, service)
    }

    pub fn with_state<T>(self, state: T) -> Self
    where
        T: Clone + Send + Sync + 'static,
    {
        self.layer(StateLayer { state })
    }

    pub fn with_state_from_ref<Outer, Inner>(self, state: Outer) -> Self
    where
        Outer: Clone + Send + Sync + 'static,
        Inner: FromRef<Outer> + Clone + Send + Sync + 'static,
    {
        self.layer(StateFromRefLayer::<Outer, Inner>::new(state))
    }

    pub fn method_not_allowed_fallback<H, T>(self, handler: H) -> Self
    where
        H: Handler<T>,
    {
        self.method_not_allowed_fallback_service(handler.into_service())
    }

    pub fn method_not_allowed_fallback_service<S>(mut self, service: S) -> Self
    where
        S: Service<IncomingRequest, Response = HttpResponse, Error = Infallible>
            + Clone
            + Send
            + 'static,
        S::Future: Send + 'static,
    {
        if self.method_not_allowed_fallback.is_some() {
            panic!("overlapping method-not-allowed fallback");
        }
        self.method_not_allowed_fallback = Some(service.boxed_clone());
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
        self
    }

    pub fn layer<L>(mut self, layer: L) -> Self
    where
        L: Layer<BoxCloneService<IncomingRequest, HttpResponse, Infallible>>
            + Clone
            + Send
            + 'static,
        L::Service: Service<IncomingRequest, Response = HttpResponse, Error = Infallible>
            + Clone
            + Send
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

    pub fn route_layer<L>(mut self, layer: L) -> Self
    where
        L: Layer<BoxCloneService<IncomingRequest, HttpResponse, Infallible>>
            + Clone
            + Send
            + 'static,
        L::Service: Service<IncomingRequest, Response = HttpResponse, Error = Infallible>
            + Clone
            + Send
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
        MethodRouterIntoMakeServiceWithConnectInfo {
            method_router: self,
            _marker: std::marker::PhantomData,
        }
    }
}

impl Service<IncomingRequest> for MethodRouter {
    type Response = HttpResponse;
    type Error = Infallible;
    type Future = BoxFuture;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, request: IncomingRequest) -> Self::Future {
        let strip_body = request.method() == Method::HEAD
            && self
                .endpoints
                .iter()
                .any(|endpoint| endpoint.method.as_ref() == Some(&Method::GET))
            && !self
                .endpoints
                .iter()
                .any(|endpoint| endpoint.method.as_ref() == Some(&Method::HEAD));
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
                self.method_not_allowed_fallback
                    .clone()
                    .unwrap_or_else(|| method_not_allowed_service(method_router_allow_header(self)))
            });
        Box::pin(async move {
            let mut service = service;
            let response = service
                .ready()
                .await
                .expect("infallible service")
                .call(request)
                .await?;
            if strip_body {
                Ok(response.map(|_| rest::empty_body()))
            } else {
                Ok(response)
            }
        })
    }
}

#[derive(Clone)]
pub struct MethodRouterIntoMakeServiceWithConnectInfo<T> {
    method_router: MethodRouter,
    _marker: std::marker::PhantomData<fn() -> T>,
}

impl<T, Target> Service<Target> for MethodRouterIntoMakeServiceWithConnectInfo<T>
where
    T: Clone + Send + Sync + 'static,
    Target: Into<T>,
{
    type Response = MethodRouter;
    type Error = Infallible;
    type Future = Ready<Result<MethodRouter, Infallible>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, target: Target) -> Self::Future {
        ready(Ok(self
            .method_router
            .clone()
            .with_state(ConnectInfo(target.into()))))
    }
}

impl<T> fmt::Debug for MethodRouterIntoMakeServiceWithConnectInfo<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MethodRouterIntoMakeServiceWithConnectInfo")
            .finish_non_exhaustive()
    }
}

pub fn get<H, T>(handler: H) -> MethodRouter
where
    H: Handler<T>,
{
    MethodRouter::new().get(handler)
}

pub fn any<H, T>(handler: H) -> MethodRouter
where
    H: Handler<T>,
{
    MethodRouter::new().any(handler)
}

pub fn any_service<S>(service: S) -> MethodRouter
where
    S: Service<IncomingRequest, Response = HttpResponse, Error = Infallible>
        + Clone
        + Send
        + 'static,
    S::Future: Send + 'static,
{
    MethodRouter::new().any_service(service)
}

pub fn on<H, T>(filter: MethodFilter, handler: H) -> MethodRouter
where
    H: Handler<T>,
{
    MethodRouter::new().on(filter, handler)
}

pub fn on_service<S>(filter: MethodFilter, service: S) -> MethodRouter
where
    S: Service<IncomingRequest, Response = HttpResponse, Error = Infallible>
        + Clone
        + Send
        + 'static,
    S::Future: Send + 'static,
{
    MethodRouter::new().on_service(filter, service)
}

pub fn get_service<S>(service: S) -> MethodRouter
where
    S: Service<IncomingRequest, Response = HttpResponse, Error = Infallible>
        + Clone
        + Send
        + 'static,
    S::Future: Send + 'static,
{
    MethodRouter::new().get_service(service)
}

pub fn post<H, T>(handler: H) -> MethodRouter
where
    H: Handler<T>,
{
    MethodRouter::new().post(handler)
}

pub fn post_service<S>(service: S) -> MethodRouter
where
    S: Service<IncomingRequest, Response = HttpResponse, Error = Infallible>
        + Clone
        + Send
        + 'static,
    S::Future: Send + 'static,
{
    MethodRouter::new().post_service(service)
}

pub fn put<H, T>(handler: H) -> MethodRouter
where
    H: Handler<T>,
{
    MethodRouter::new().put(handler)
}

pub fn put_service<S>(service: S) -> MethodRouter
where
    S: Service<IncomingRequest, Response = HttpResponse, Error = Infallible>
        + Clone
        + Send
        + 'static,
    S::Future: Send + 'static,
{
    MethodRouter::new().put_service(service)
}

pub fn patch<H, T>(handler: H) -> MethodRouter
where
    H: Handler<T>,
{
    MethodRouter::new().patch(handler)
}

pub fn patch_service<S>(service: S) -> MethodRouter
where
    S: Service<IncomingRequest, Response = HttpResponse, Error = Infallible>
        + Clone
        + Send
        + 'static,
    S::Future: Send + 'static,
{
    MethodRouter::new().patch_service(service)
}

pub fn delete<H, T>(handler: H) -> MethodRouter
where
    H: Handler<T>,
{
    MethodRouter::new().delete(handler)
}

pub fn delete_service<S>(service: S) -> MethodRouter
where
    S: Service<IncomingRequest, Response = HttpResponse, Error = Infallible>
        + Clone
        + Send
        + 'static,
    S::Future: Send + 'static,
{
    MethodRouter::new().delete_service(service)
}

pub fn head<H, T>(handler: H) -> MethodRouter
where
    H: Handler<T>,
{
    MethodRouter::new().head(handler)
}

pub fn head_service<S>(service: S) -> MethodRouter
where
    S: Service<IncomingRequest, Response = HttpResponse, Error = Infallible>
        + Clone
        + Send
        + 'static,
    S::Future: Send + 'static,
{
    MethodRouter::new().head_service(service)
}

pub fn options<H, T>(handler: H) -> MethodRouter
where
    H: Handler<T>,
{
    MethodRouter::new().options(handler)
}

pub fn options_service<S>(service: S) -> MethodRouter
where
    S: Service<IncomingRequest, Response = HttpResponse, Error = Infallible>
        + Clone
        + Send
        + 'static,
    S::Future: Send + 'static,
{
    MethodRouter::new().options_service(service)
}

pub fn trace<H, T>(handler: H) -> MethodRouter
where
    H: Handler<T>,
{
    MethodRouter::new().trace(handler)
}

pub fn trace_service<S>(service: S) -> MethodRouter
where
    S: Service<IncomingRequest, Response = HttpResponse, Error = Infallible>
        + Clone
        + Send
        + 'static,
    S::Future: Send + 'static,
{
    MethodRouter::new().trace_service(service)
}

pub fn connect<H, T>(handler: H) -> MethodRouter
where
    H: Handler<T>,
{
    MethodRouter::new().connect(handler)
}

pub fn connect_service<S>(service: S) -> MethodRouter
where
    S: Service<IncomingRequest, Response = HttpResponse, Error = Infallible>
        + Clone
        + Send
        + 'static,
    S::Future: Send + 'static,
{
    MethodRouter::new().connect_service(service)
}

fn method_not_allowed_service(
    allow: Option<String>,
) -> BoxCloneService<IncomingRequest, HttpResponse, Infallible> {
    tower::service_fn(move |_request: IncomingRequest| {
        let allow = allow.clone();
        async move {
            let mut response =
                rest::text_response(StatusCode::METHOD_NOT_ALLOWED, "method not allowed");
            if let Some(allow) = allow {
                if let Ok(value) = allow.parse() {
                    response.headers_mut().insert(header::ALLOW, value);
                }
            }
            Ok::<_, Infallible>(response)
        }
    })
    .boxed_clone()
}

fn default_not_found_service() -> BoxCloneService<IncomingRequest, HttpResponse, Infallible> {
    tower::service_fn(|_request: IncomingRequest| async {
        Ok::<_, Infallible>(rest::text_response(StatusCode::NOT_FOUND, "not found"))
    })
    .boxed_clone()
}

fn layer_service<L>(
    layer: L,
    service: BoxCloneService<IncomingRequest, HttpResponse, Infallible>,
) -> BoxCloneService<IncomingRequest, HttpResponse, Infallible>
where
    L: Layer<BoxCloneService<IncomingRequest, HttpResponse, Infallible>> + Clone + Send + 'static,
    L::Service: Service<IncomingRequest, Response = HttpResponse, Error = Infallible>
        + Clone
        + Send
        + 'static,
    <L::Service as Service<IncomingRequest>>::Future: Send + 'static,
{
    layer.layer(service).boxed_clone()
}

#[derive(Clone)]
struct StateLayer<T> {
    state: T,
}

impl<T> Layer<BoxCloneService<IncomingRequest, HttpResponse, Infallible>> for StateLayer<T>
where
    T: Clone + Send + Sync + 'static,
{
    type Service = StateService<T>;

    fn layer(
        &self,
        inner: BoxCloneService<IncomingRequest, HttpResponse, Infallible>,
    ) -> Self::Service {
        StateService {
            state: self.state.clone(),
            inner,
        }
    }
}

#[derive(Clone)]
struct StateService<T> {
    state: T,
    inner: BoxCloneService<IncomingRequest, HttpResponse, Infallible>,
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
struct StateFromRefLayer<Outer, Inner> {
    state: Outer,
    _marker: std::marker::PhantomData<fn() -> Inner>,
}

impl<Outer, Inner> StateFromRefLayer<Outer, Inner> {
    fn new(state: Outer) -> Self {
        Self {
            state,
            _marker: std::marker::PhantomData,
        }
    }
}

impl<Outer, Inner> Layer<BoxCloneService<IncomingRequest, HttpResponse, Infallible>>
    for StateFromRefLayer<Outer, Inner>
where
    Outer: Clone + Send + Sync + 'static,
    Inner: FromRef<Outer> + Clone + Send + Sync + 'static,
{
    type Service = StateFromRefService<Outer, Inner>;

    fn layer(
        &self,
        inner: BoxCloneService<IncomingRequest, HttpResponse, Infallible>,
    ) -> Self::Service {
        StateFromRefService {
            state: self.state.clone(),
            inner,
            _marker: std::marker::PhantomData,
        }
    }
}

#[derive(Clone)]
struct StateFromRefService<Outer, Inner> {
    state: Outer,
    inner: BoxCloneService<IncomingRequest, HttpResponse, Infallible>,
    _marker: std::marker::PhantomData<fn() -> Inner>,
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

fn routes_overlap(left: &Option<Method>, right: &Option<Method>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => left == right,
        (None, None) => true,
        (Some(_), None) | (None, Some(_)) => false,
    }
}

fn method_label(method: &Option<Method>) -> String {
    method
        .as_ref()
        .map(ToString::to_string)
        .unwrap_or_else(|| "any".to_string())
}

fn allow_header(group: &RouteGroup) -> Option<String> {
    if group.routes.iter().any(|route| route.method.is_none()) {
        return None;
    }
    Some(allow_header_from_methods(
        group
            .routes
            .iter()
            .filter_map(|route| route.method.as_ref())
            .cloned(),
    ))
}

fn method_router_allow_header(method_router: &MethodRouter) -> Option<String> {
    if method_router
        .endpoints
        .iter()
        .any(|endpoint| endpoint.method.is_none())
    {
        return None;
    }
    Some(allow_header_from_methods(
        method_router
            .endpoints
            .iter()
            .filter_map(|endpoint| endpoint.method.as_ref())
            .cloned(),
    ))
}

fn allow_header_from_methods(methods: impl IntoIterator<Item = Method>) -> String {
    let mut methods = methods
        .into_iter()
        .map(|method| method.to_string())
        .collect::<Vec<_>>();
    if methods.iter().any(|method| method == Method::GET.as_str()) {
        methods.push(Method::HEAD.to_string());
    }
    methods.sort();
    methods.dedup();
    methods.join(", ")
}

fn normalize_path(path: String) -> String {
    if path.is_empty() {
        panic!("route path must not be empty");
    }
    if !path.starts_with('/') {
        panic!("route path must start with `/`");
    }
    path
}

fn normalize_nest_prefix(prefix: String) -> String {
    let prefix = normalize_path(prefix);
    let prefix = prefix.trim_end_matches('/').to_string();
    if prefix.is_empty() || prefix == "/" {
        panic!("nest prefix must not be root");
    }
    prefix
}

fn join_paths(prefix: &str, path: &str) -> String {
    if path == "/" {
        prefix.to_string()
    } else {
        format!("{prefix}{}", normalize_path(path.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ConnectInfo, OriginalUri, State};
    use http::{HeaderValue, Request};
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
        let mut make_service = Router::new()
            .route(
                "/peer",
                get(|ConnectInfo(addr): ConnectInfo<SocketAddr>| async move { addr.to_string() }),
            )
            .into_make_service_with_connect_info::<SocketAddr>();
        let mut service = make_service.call(peer_addr).await.unwrap();
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
    #[should_panic(expected = "route path must start with `/`")]
    fn nest_panics_on_prefix_without_leading_slash() {
        let _router = Router::new().nest(
            "api",
            Router::new().route("/users", get(|| async { "users" })),
        );
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
            Ok::<_, Infallible>(rest::text_response(
                StatusCode::OK,
                request.uri().path().to_string(),
            ))
        });
        let mut router = Router::new().nest_service("/proxy", service);
        let response = router
            .call(
                Request::builder()
                    .method(Method::DELETE)
                    .uri("/proxy/users/42")
                    .body(empty_incoming())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"/proxy/users/42");
    }

    fn empty_incoming() -> crate::rest::Body {
        crate::rest::empty_body()
    }

    #[derive(Clone)]
    struct TestHeaderLayer;

    impl Layer<BoxCloneService<IncomingRequest, HttpResponse, Infallible>> for TestHeaderLayer {
        type Service = TestHeaderService;

        fn layer(
            &self,
            inner: BoxCloneService<IncomingRequest, HttpResponse, Infallible>,
        ) -> Self::Service {
            TestHeaderService { inner }
        }
    }

    #[derive(Clone)]
    struct TestHeaderService {
        inner: BoxCloneService<IncomingRequest, HttpResponse, Infallible>,
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

    impl Layer<BoxCloneService<IncomingRequest, HttpResponse, Infallible>> for FallibleLayer {
        type Service = FallibleService;

        fn layer(
            &self,
            inner: BoxCloneService<IncomingRequest, HttpResponse, Infallible>,
        ) -> Self::Service {
            FallibleService { inner }
        }
    }

    #[derive(Clone)]
    struct FallibleService {
        inner: BoxCloneService<IncomingRequest, HttpResponse, Infallible>,
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
