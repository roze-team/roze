use std::{
    collections::BTreeMap,
    convert::Infallible,
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

use http::{header, Method, StatusCode};
use matchit::Router as MatchRouter;
use tower::{util::BoxCloneService, Layer, Service, ServiceExt};

use crate::{
    handler::Handler,
    rest::{self, HttpResponse, IncomingRequest},
};

type BoxFuture = Pin<Box<dyn Future<Output = Result<HttpResponse, Infallible>> + Send>>;

#[derive(Clone)]
pub struct Router {
    routes: MatchRouter<usize>,
    route_groups: Vec<RouteGroup>,
    path_index: BTreeMap<String, usize>,
    fallback: BoxCloneService<IncomingRequest, HttpResponse, Infallible>,
}

#[derive(Clone)]
struct RouteGroup {
    path: String,
    routes: Vec<Route>,
}

#[derive(Clone)]
struct Route {
    method: Option<Method>,
    service: BoxCloneService<IncomingRequest, HttpResponse, Infallible>,
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

impl Router {
    pub fn new() -> Self {
        Self {
            routes: MatchRouter::new(),
            route_groups: Vec::new(),
            path_index: BTreeMap::new(),
            fallback: default_not_found_service(),
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

    pub fn merge(mut self, router: Router) -> Self {
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

    pub fn route_service<S>(self, method: Method, path: impl Into<String>, service: S) -> Self
    where
        S: Service<IncomingRequest, Response = HttpResponse, Error = Infallible>
            + Clone
            + Send
            + 'static,
        S::Future: Send + 'static,
    {
        self.route(path, MethodRouter::new().on_service(method, service))
    }

    pub fn route_handler<H, T>(self, method: Method, path: impl Into<String>, handler: H) -> Self
    where
        H: Handler<T>,
    {
        self.route_service(method, path, handler.into_service())
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

    pub fn fallback<S>(mut self, service: S) -> Self
    where
        S: Service<IncomingRequest, Response = HttpResponse, Error = Infallible>
            + Clone
            + Send
            + 'static,
        S::Future: Send + 'static,
    {
        self.fallback = service.boxed_clone();
        self
    }

    pub fn fallback_service<S>(self, service: S) -> Self
    where
        S: Service<IncomingRequest, Response = HttpResponse, Error = Infallible>
            + Clone
            + Send
            + 'static,
        S::Future: Send + 'static,
    {
        self.fallback(service)
    }

    pub fn fallback_handler<H, T>(mut self, handler: H) -> Self
    where
        H: Handler<T>,
    {
        self.fallback = handler.into_service();
        self
    }

    pub fn reset_fallback(mut self) -> Self {
        self.fallback = default_not_found_service();
        self
    }

    pub fn has_routes(&self) -> bool {
        !self.route_groups.is_empty()
    }

    pub fn with_state<T>(self, state: T) -> Self
    where
        T: Clone + Send + Sync + 'static,
    {
        self.layer(StateLayer { state })
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
                });
                self.path_index.insert(path, index);
                index
            }
        }
    }

    fn insert_group_routes(&mut self, path: String, group: RouteGroup, action: &str) {
        let group_index = self.ensure_route_group(path);
        for route in group.routes {
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
        }
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
                    .or_else(|| group.routes.iter().find(|route| route.method.is_none()))
                    .map(|route| route.service.clone())
                    .unwrap_or_else(|| method_not_allowed_service(allow_header(group)))
            }
            None => self.fallback.clone(),
        };
        Box::pin(async move {
            service
                .ready()
                .await
                .expect("infallible service")
                .call(request)
                .await
        })
    }
}

#[derive(Clone)]
pub struct MethodRouter {
    endpoints: Vec<MethodEndpoint>,
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

impl MethodRouter {
    pub fn new() -> Self {
        Self {
            endpoints: Vec::new(),
        }
    }

    pub fn on<H, T>(self, method: Method, handler: H) -> Self
    where
        H: Handler<T>,
    {
        self.on_service(method, handler.into_service())
    }

    pub fn on_service<S>(mut self, method: Method, service: S) -> Self
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
            .any(|endpoint| routes_overlap(&endpoint.method, &Some(method.clone())))
        {
            panic!("overlapping method route for {method}");
        }
        self.endpoints.push(MethodEndpoint {
            method: Some(method),
            service: service.boxed_clone(),
        });
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

    pub fn post<H, T>(self, handler: H) -> Self
    where
        H: Handler<T>,
    {
        self.on(Method::POST, handler)
    }

    pub fn put<H, T>(self, handler: H) -> Self
    where
        H: Handler<T>,
    {
        self.on(Method::PUT, handler)
    }

    pub fn patch<H, T>(self, handler: H) -> Self
    where
        H: Handler<T>,
    {
        self.on(Method::PATCH, handler)
    }

    pub fn delete<H, T>(self, handler: H) -> Self
    where
        H: Handler<T>,
    {
        self.on(Method::DELETE, handler)
    }

    pub fn head<H, T>(self, handler: H) -> Self
    where
        H: Handler<T>,
    {
        self.on(Method::HEAD, handler)
    }

    pub fn options<H, T>(self, handler: H) -> Self
    where
        H: Handler<T>,
    {
        self.on(Method::OPTIONS, handler)
    }

    pub fn trace<H, T>(self, handler: H) -> Self
    where
        H: Handler<T>,
    {
        self.on(Method::TRACE, handler)
    }

    pub fn connect<H, T>(self, handler: H) -> Self
    where
        H: Handler<T>,
    {
        self.on(Method::CONNECT, handler)
    }

    pub fn with_state<T>(self, state: T) -> Self
    where
        T: Clone + Send + Sync + 'static,
    {
        self.layer(StateLayer { state })
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
        self
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

pub fn post<H, T>(handler: H) -> MethodRouter
where
    H: Handler<T>,
{
    MethodRouter::new().post(handler)
}

pub fn put<H, T>(handler: H) -> MethodRouter
where
    H: Handler<T>,
{
    MethodRouter::new().put(handler)
}

pub fn patch<H, T>(handler: H) -> MethodRouter
where
    H: Handler<T>,
{
    MethodRouter::new().patch(handler)
}

pub fn delete<H, T>(handler: H) -> MethodRouter
where
    H: Handler<T>,
{
    MethodRouter::new().delete(handler)
}

pub fn head<H, T>(handler: H) -> MethodRouter
where
    H: Handler<T>,
{
    MethodRouter::new().head(handler)
}

pub fn options<H, T>(handler: H) -> MethodRouter
where
    H: Handler<T>,
{
    MethodRouter::new().options(handler)
}

pub fn trace<H, T>(handler: H) -> MethodRouter
where
    H: Handler<T>,
{
    MethodRouter::new().trace(handler)
}

pub fn connect<H, T>(handler: H) -> MethodRouter
where
    H: Handler<T>,
{
    MethodRouter::new().connect(handler)
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
    let mut methods = group
        .routes
        .iter()
        .filter_map(|route| route.method.as_ref())
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    methods.sort();
    methods.dedup();
    if methods.is_empty() {
        None
    } else {
        Some(methods.join(", "))
    }
}

fn normalize_path(path: String) -> String {
    if path.starts_with('/') {
        path
    } else {
        format!("/{path}")
    }
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
    use crate::State;
    use http::{HeaderValue, Request};
    use http_body_util::BodyExt;

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
    async fn uses_fallback_handler_for_missing_route() {
        let mut router = Router::new().fallback_handler(|| async { "fallback" });
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
            .fallback_handler(|| async { "fallback" })
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
    async fn with_state_applies_to_fallback() {
        let mut router = Router::new()
            .fallback_handler(|State(state): State<String>| async move { state })
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
            Some(&HeaderValue::from_static("GET, POST"))
        );
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

    #[test]
    #[should_panic(expected = "overlapping merged route")]
    fn merge_panics_on_overlapping_method() {
        let left = Router::new().route("/users", get(|| async { "left" }));
        let right = Router::new().route("/users", get(|| async { "right" }));
        let _router = left.merge(right);
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
}
