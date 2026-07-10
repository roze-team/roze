use std::{
    convert::Infallible,
    future::Future,
    marker::PhantomData,
    pin::Pin,
    task::{Context, Poll},
};

use tower::{util::BoxCloneSyncService, Layer, Service, ServiceExt};

use crate::{
    extract::{Extension, FromRequest, FromRequestParts},
    response::IntoResponse,
    rest::{HttpResponse, IncomingRequest},
};

type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

pub fn from_fn<F, Args>(f: F) -> FromFnLayer<F, Args, ()> {
    from_fn_with_state((), f)
}

pub fn from_fn_with_state<S, F, Args>(state: S, f: F) -> FromFnLayer<F, Args, S> {
    FromFnLayer {
        f,
        state,
        _marker: PhantomData,
    }
}

pub fn map_response<F>(f: F) -> MapResponseLayer<F> {
    MapResponseLayer { f }
}

pub fn map_request<F>(f: F) -> MapRequestLayer<F> {
    MapRequestLayer { f }
}

pub fn from_extractor<E>() -> FromExtractorLayer<E, ()> {
    from_extractor_with_state(())
}

pub fn from_extractor_with_state<E, S>(state: S) -> FromExtractorLayer<E, S> {
    FromExtractorLayer {
        state,
        _marker: PhantomData,
    }
}

pub struct AddExtensionLayer<T> {
    value: T,
}

impl<T> AddExtensionLayer<T> {
    pub fn new(value: T) -> Self {
        Self { value }
    }
}

impl<T> Clone for AddExtensionLayer<T>
where
    T: Clone,
{
    fn clone(&self) -> Self {
        Self {
            value: self.value.clone(),
        }
    }
}

impl<T> std::fmt::Debug for AddExtensionLayer<T>
where
    T: std::fmt::Debug,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AddExtensionLayer")
            .field("value", &self.value)
            .finish()
    }
}

impl<T> Layer<BoxCloneSyncService<IncomingRequest, HttpResponse, Infallible>>
    for AddExtensionLayer<T>
where
    T: Clone + Send + Sync + 'static,
{
    type Service = AddExtension<T>;

    fn layer(
        &self,
        inner: BoxCloneSyncService<IncomingRequest, HttpResponse, Infallible>,
    ) -> Self::Service {
        AddExtension {
            value: self.value.clone(),
            inner,
        }
    }
}

impl<T> Layer<BoxCloneSyncService<IncomingRequest, HttpResponse, Infallible>> for Extension<T>
where
    T: Clone + Send + Sync + 'static,
{
    type Service = AddExtension<T>;

    fn layer(
        &self,
        inner: BoxCloneSyncService<IncomingRequest, HttpResponse, Infallible>,
    ) -> Self::Service {
        AddExtension {
            value: self.0.clone(),
            inner,
        }
    }
}

pub struct AddExtension<T> {
    value: T,
    inner: BoxCloneSyncService<IncomingRequest, HttpResponse, Infallible>,
}

impl<T> Clone for AddExtension<T>
where
    T: Clone,
{
    fn clone(&self) -> Self {
        Self {
            value: self.value.clone(),
            inner: self.inner.clone(),
        }
    }
}

impl<T> std::fmt::Debug for AddExtension<T>
where
    T: std::fmt::Debug,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AddExtension")
            .field("value", &self.value)
            .finish_non_exhaustive()
    }
}

impl<T> Service<IncomingRequest> for AddExtension<T>
where
    T: Clone + Send + Sync + 'static,
{
    type Response = HttpResponse;
    type Error = Infallible;
    type Future = BoxFuture<Result<HttpResponse, Infallible>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut request: IncomingRequest) -> Self::Future {
        request.extensions_mut().insert(self.value.clone());
        let mut inner = self.inner.clone();
        Box::pin(async move { inner.call(request).await })
    }
}

pub struct FromExtractorLayer<E, S = ()> {
    state: S,
    _marker: PhantomData<fn() -> E>,
}

impl<E, S> Clone for FromExtractorLayer<E, S>
where
    S: Clone,
{
    fn clone(&self) -> Self {
        Self {
            state: self.state.clone(),
            _marker: PhantomData,
        }
    }
}

impl<E, S> std::fmt::Debug for FromExtractorLayer<E, S>
where
    S: std::fmt::Debug,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FromExtractorLayer")
            .field("state", &self.state)
            .field("extractor", &std::any::type_name::<E>())
            .finish()
    }
}

impl<E, S> Layer<BoxCloneSyncService<IncomingRequest, HttpResponse, Infallible>>
    for FromExtractorLayer<E, S>
where
    E: FromRequestParts + Send + 'static,
    E::Rejection: Send + 'static,
    S: Clone + Send + Sync + 'static,
{
    type Service = FromExtractor<E, S>;

    fn layer(
        &self,
        inner: BoxCloneSyncService<IncomingRequest, HttpResponse, Infallible>,
    ) -> Self::Service {
        FromExtractor {
            state: self.state.clone(),
            inner,
            _marker: PhantomData,
        }
    }
}

pub struct FromExtractor<E, S = ()> {
    state: S,
    inner: BoxCloneSyncService<IncomingRequest, HttpResponse, Infallible>,
    _marker: PhantomData<fn() -> E>,
}

impl<E, S> Clone for FromExtractor<E, S>
where
    S: Clone,
{
    fn clone(&self) -> Self {
        Self {
            state: self.state.clone(),
            inner: self.inner.clone(),
            _marker: PhantomData,
        }
    }
}

impl<E, S> std::fmt::Debug for FromExtractor<E, S>
where
    S: std::fmt::Debug,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FromExtractor")
            .field("state", &self.state)
            .field("extractor", &std::any::type_name::<E>())
            .finish_non_exhaustive()
    }
}

impl<E, S> Service<IncomingRequest> for FromExtractor<E, S>
where
    E: FromRequestParts + Send + 'static,
    E::Rejection: Send + 'static,
    S: Clone + Send + Sync + 'static,
{
    type Response = HttpResponse;
    type Error = Infallible;
    type Future = BoxFuture<Result<HttpResponse, Infallible>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut request: IncomingRequest) -> Self::Future {
        request.extensions_mut().insert(self.state.clone());
        let mut inner = self.inner.clone();
        Box::pin(async move {
            let (mut parts, body) = request.into_parts();
            match E::from_request_parts(&mut parts).await {
                Ok(_) => {
                    let request = http::Request::from_parts(parts, body);
                    inner.call(request).await
                }
                Err(rejection) => Ok(rejection.into_response()),
            }
        })
    }
}

pub trait IntoMapRequestResult {
    #[allow(clippy::result_large_err)]
    fn into_map_request_result(self) -> Result<IncomingRequest, HttpResponse>;
}

impl IntoMapRequestResult for IncomingRequest {
    fn into_map_request_result(self) -> Result<IncomingRequest, HttpResponse> {
        Ok(self)
    }
}

impl<E> IntoMapRequestResult for Result<IncomingRequest, E>
where
    E: IntoResponse,
{
    fn into_map_request_result(self) -> Result<IncomingRequest, HttpResponse> {
        self.map_err(IntoResponse::into_response)
    }
}

pub struct MapRequestLayer<F> {
    f: F,
}

impl<F> Clone for MapRequestLayer<F>
where
    F: Clone,
{
    fn clone(&self) -> Self {
        Self { f: self.f.clone() }
    }
}

impl<F> std::fmt::Debug for MapRequestLayer<F> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MapRequestLayer")
            .field("f", &std::any::type_name::<F>())
            .finish()
    }
}

impl<F> Layer<BoxCloneSyncService<IncomingRequest, HttpResponse, Infallible>> for MapRequestLayer<F>
where
    F: Clone + Send + Sync + 'static,
{
    type Service = MapRequest<F>;

    fn layer(
        &self,
        inner: BoxCloneSyncService<IncomingRequest, HttpResponse, Infallible>,
    ) -> Self::Service {
        MapRequest {
            f: self.f.clone(),
            inner,
        }
    }
}

pub struct MapRequest<F> {
    f: F,
    inner: BoxCloneSyncService<IncomingRequest, HttpResponse, Infallible>,
}

impl<F> Clone for MapRequest<F>
where
    F: Clone,
{
    fn clone(&self) -> Self {
        Self {
            f: self.f.clone(),
            inner: self.inner.clone(),
        }
    }
}

impl<F> std::fmt::Debug for MapRequest<F> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MapRequest")
            .field("f", &std::any::type_name::<F>())
            .finish_non_exhaustive()
    }
}

impl<F, Fut, Out> Service<IncomingRequest> for MapRequest<F>
where
    F: Fn(IncomingRequest) -> Fut + Clone + Send + Sync + 'static,
    Fut: Future<Output = Out> + Send + 'static,
    Out: IntoMapRequestResult + 'static,
{
    type Response = HttpResponse;
    type Error = Infallible;
    type Future = BoxFuture<Result<HttpResponse, Infallible>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, request: IncomingRequest) -> Self::Future {
        let mut inner = self.inner.clone();
        let f = self.f.clone();
        Box::pin(async move {
            match f(request).await.into_map_request_result() {
                Ok(request) => inner.call(request).await,
                Err(response) => Ok(response),
            }
        })
    }
}

pub struct MapResponseLayer<F> {
    f: F,
}

impl<F> Clone for MapResponseLayer<F>
where
    F: Clone,
{
    fn clone(&self) -> Self {
        Self { f: self.f.clone() }
    }
}

impl<F> std::fmt::Debug for MapResponseLayer<F> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MapResponseLayer")
            .field("f", &std::any::type_name::<F>())
            .finish()
    }
}

impl<F> Layer<BoxCloneSyncService<IncomingRequest, HttpResponse, Infallible>>
    for MapResponseLayer<F>
where
    F: Clone + Send + Sync + 'static,
{
    type Service = MapResponse<F>;

    fn layer(
        &self,
        inner: BoxCloneSyncService<IncomingRequest, HttpResponse, Infallible>,
    ) -> Self::Service {
        MapResponse {
            f: self.f.clone(),
            inner,
        }
    }
}

pub struct MapResponse<F> {
    f: F,
    inner: BoxCloneSyncService<IncomingRequest, HttpResponse, Infallible>,
}

impl<F> Clone for MapResponse<F>
where
    F: Clone,
{
    fn clone(&self) -> Self {
        Self {
            f: self.f.clone(),
            inner: self.inner.clone(),
        }
    }
}

impl<F> std::fmt::Debug for MapResponse<F> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MapResponse")
            .field("f", &std::any::type_name::<F>())
            .finish_non_exhaustive()
    }
}

impl<F, Fut, Out> Service<IncomingRequest> for MapResponse<F>
where
    F: Fn(HttpResponse) -> Fut + Clone + Send + Sync + 'static,
    Fut: Future<Output = Out> + Send + 'static,
    Out: IntoResponse + 'static,
{
    type Response = HttpResponse;
    type Error = Infallible;
    type Future = BoxFuture<Result<HttpResponse, Infallible>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, request: IncomingRequest) -> Self::Future {
        let mut inner = self.inner.clone();
        let f = self.f.clone();
        Box::pin(async move {
            let response = inner.call(request).await?;
            Ok(f(response).await.into_response())
        })
    }
}

pub struct FromFnLayer<F, Args, S = ()> {
    f: F,
    state: S,
    _marker: PhantomData<fn() -> Args>,
}

impl<F, Args, S> Clone for FromFnLayer<F, Args, S>
where
    F: Clone,
    S: Clone,
{
    fn clone(&self) -> Self {
        Self {
            f: self.f.clone(),
            state: self.state.clone(),
            _marker: PhantomData,
        }
    }
}

impl<F, Args, S> std::fmt::Debug for FromFnLayer<F, Args, S>
where
    S: std::fmt::Debug,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FromFnLayer")
            .field("f", &std::any::type_name::<F>())
            .field("state", &self.state)
            .finish()
    }
}

impl<F, Args, S> Layer<BoxCloneSyncService<IncomingRequest, HttpResponse, Infallible>>
    for FromFnLayer<F, Args, S>
where
    F: Clone + Send + Sync + 'static,
    S: Clone + Send + Sync + 'static,
    Args: 'static,
{
    type Service = FromFn<F, Args, S>;

    fn layer(
        &self,
        inner: BoxCloneSyncService<IncomingRequest, HttpResponse, Infallible>,
    ) -> Self::Service {
        FromFn {
            f: self.f.clone(),
            state: self.state.clone(),
            inner,
            _marker: PhantomData,
        }
    }
}

pub struct FromFn<F, Args, S = ()> {
    f: F,
    state: S,
    inner: BoxCloneSyncService<IncomingRequest, HttpResponse, Infallible>,
    _marker: PhantomData<fn() -> Args>,
}

impl<F, Args, S> Clone for FromFn<F, Args, S>
where
    F: Clone,
    S: Clone,
{
    fn clone(&self) -> Self {
        Self {
            f: self.f.clone(),
            state: self.state.clone(),
            inner: self.inner.clone(),
            _marker: PhantomData,
        }
    }
}

impl<F, Args, S> std::fmt::Debug for FromFn<F, Args, S>
where
    S: std::fmt::Debug,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FromFn")
            .field("f", &std::any::type_name::<F>())
            .field("state", &self.state)
            .finish_non_exhaustive()
    }
}

#[derive(Clone)]
pub struct Next {
    inner: BoxCloneSyncService<IncomingRequest, HttpResponse, Infallible>,
}

impl Next {
    pub async fn run(mut self, request: IncomingRequest) -> HttpResponse {
        self.inner
            .ready()
            .await
            .expect("infallible middleware stack")
            .call(request)
            .await
            .expect("infallible middleware stack")
    }
}

impl std::fmt::Debug for Next {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Next").finish_non_exhaustive()
    }
}

impl Service<IncomingRequest> for Next {
    type Response = HttpResponse;
    type Error = Infallible;
    type Future = BoxFuture<Result<HttpResponse, Infallible>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, request: IncomingRequest) -> Self::Future {
        let mut inner = self.inner.clone();
        Box::pin(async move { inner.call(request).await })
    }
}

impl<F, Fut, Out, Last, S> Service<IncomingRequest> for FromFn<F, (Last,), S>
where
    F: Fn(Last, Next) -> Fut + Clone + Send + Sync + 'static,
    Fut: Future<Output = Out> + Send + 'static,
    Out: IntoResponse + 'static,
    Last: FromRequest + Send + 'static,
    S: Clone + Send + Sync + 'static,
{
    type Response = HttpResponse;
    type Error = Infallible;
    type Future = BoxFuture<Result<HttpResponse, Infallible>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut request: IncomingRequest) -> Self::Future {
        let next = Next {
            inner: self.inner.clone(),
        };
        let f = self.f.clone();
        request.extensions_mut().insert(self.state.clone());
        Box::pin(async move {
            let last = match Last::from_request(request).await {
                Ok(value) => value,
                Err(rejection) => return Ok(rejection.into_response()),
            };
            Ok(f(last, next).await.into_response())
        })
    }
}

macro_rules! impl_from_fn {
    ($($ty:ident),+ => $last:ident) => {
        impl<F, Fut, Out, S, $($ty,)* $last> Service<IncomingRequest>
            for FromFn<F, ($($ty,)* $last,), S>
        where
            F: Fn($($ty,)* $last, Next) -> Fut + Clone + Send + Sync + 'static,
            Fut: Future<Output = Out> + Send + 'static,
            Out: IntoResponse + 'static,
            S: Clone + Send + Sync + 'static,
            $($ty: FromRequestParts + Send + 'static,)*
            $last: FromRequest + Send + 'static,
        {
            type Response = HttpResponse;
            type Error = Infallible;
            type Future = BoxFuture<Result<HttpResponse, Infallible>>;

            fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
                self.inner.poll_ready(cx)
            }

            #[allow(non_snake_case)]
            fn call(&mut self, mut request: IncomingRequest) -> Self::Future {
                let next = Next {
                    inner: self.inner.clone(),
                };
                let f = self.f.clone();
                request.extensions_mut().insert(self.state.clone());
                Box::pin(async move {
                    let (mut parts, body) = request.into_parts();
                    $(
                        let $ty = match $ty::from_request_parts(&mut parts).await {
                            Ok(value) => value,
                            Err(rejection) => return Ok(rejection.into_response()),
                        };
                    )*
                    let request = http::Request::from_parts(parts, body);
                    let $last = match $last::from_request(request).await {
                        Ok(value) => value,
                        Err(rejection) => return Ok(rejection.into_response()),
                    };
                    Ok(f($($ty,)* $last, next).await.into_response())
                })
            }
        }
    };
}

impl_from_fn!(T1 => Last);
impl_from_fn!(T1, T2 => Last);
impl_from_fn!(T1, T2, T3 => Last);
impl_from_fn!(T1, T2, T3, T4 => Last);

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use http::{header, request::Parts as HttpParts, HeaderMap, Method, Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use super::*;
    use crate::{
        extract::{ExtractFuture, State},
        response::Response,
        rest,
        router::{get, post, Router},
    };

    struct RequireAuth;

    #[derive(Clone, Debug)]
    struct AppName(&'static str);

    impl FromRequestParts for RequireAuth {
        type Rejection = StatusCode;

        fn from_request_parts(parts: &mut HttpParts) -> ExtractFuture<'_, Self, Self::Rejection> {
            let authorized = parts
                .headers
                .get(header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok())
                .is_some_and(|value| value == "Bearer secret");

            Box::pin(async move {
                if authorized {
                    Ok(Self)
                } else {
                    Err(StatusCode::UNAUTHORIZED)
                }
            })
        }
    }

    struct RequireAppState;

    impl FromRequestParts for RequireAppState {
        type Rejection = StatusCode;

        fn from_request_parts(parts: &mut HttpParts) -> ExtractFuture<'_, Self, Self::Rejection> {
            Box::pin(async move {
                let State(app) = State::<AppName>::from_request_parts(parts)
                    .await
                    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

                if app.0 == "roze" {
                    Ok(Self)
                } else {
                    Err(StatusCode::FORBIDDEN)
                }
            })
        }
    }

    #[tokio::test]
    async fn from_extractor_allows_request_when_extractor_succeeds() {
        let mut service = Router::new()
            .route("/", get(|| async { "ok" }))
            .route_layer(from_extractor::<RequireAuth>())
            .into_service();

        let response = service
            .ready()
            .await
            .unwrap()
            .call(
                Request::builder()
                    .uri("/")
                    .header(header::AUTHORIZATION, "Bearer secret")
                    .body(rest::empty_body())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"ok");
    }

    #[tokio::test]
    async fn from_extractor_short_circuits_when_extractor_rejects() {
        let mut service = Router::new()
            .route("/", get(|| async { "ok" }))
            .route_layer(from_extractor::<RequireAuth>())
            .into_service();

        let response = service
            .ready()
            .await
            .unwrap()
            .call(
                Request::builder()
                    .uri("/")
                    .body(rest::empty_body())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn from_extractor_with_state_makes_state_available_to_extractor() {
        let mut service = Router::new()
            .route("/", get(|| async { "ok" }))
            .route_layer(from_extractor_with_state::<RequireAppState, _>(AppName(
                "roze",
            )))
            .into_service();

        let response = service
            .ready()
            .await
            .unwrap()
            .call(
                Request::builder()
                    .uri("/")
                    .body(rest::empty_body())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"ok");
    }

    #[tokio::test]
    async fn extension_layer_inserts_request_extension() {
        async fn handler(Extension(app): Extension<AppName>) -> &'static str {
            app.0
        }

        let mut service = Router::new()
            .route("/", get(handler))
            .layer(Extension(AppName("roze")))
            .into_service();

        let response = service
            .ready()
            .await
            .unwrap()
            .call(
                Request::builder()
                    .uri("/")
                    .body(rest::empty_body())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"roze");
    }

    #[tokio::test]
    async fn add_extension_layer_inserts_request_extension() {
        async fn handler(Extension(app): Extension<AppName>) -> &'static str {
            app.0
        }

        let mut service = Router::new()
            .route("/", get(handler))
            .layer(AddExtensionLayer::new(AppName("roze")))
            .into_service();

        let response = service
            .ready()
            .await
            .unwrap()
            .call(
                Request::builder()
                    .uri("/")
                    .body(rest::empty_body())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"roze");
    }

    #[tokio::test]
    async fn from_fn_wraps_request_and_next() {
        async fn insert_header(mut request: IncomingRequest, next: Next) -> Response {
            request
                .headers_mut()
                .insert("x-roze-middleware", "yes".parse().unwrap());
            let mut response = next.run(request).await;
            response
                .headers_mut()
                .insert("x-roze-after", "yes".parse().unwrap());
            response
        }

        async fn handler(headers: HeaderMap) -> String {
            headers["x-roze-middleware"].to_str().unwrap().to_string()
        }

        let mut service = Router::new()
            .route("/", get(handler))
            .layer(from_fn(insert_header))
            .into_service();

        let response = service
            .ready()
            .await
            .unwrap()
            .call(
                Request::builder()
                    .uri("/")
                    .body(rest::empty_body())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["x-roze-after"], "yes");
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"yes");
    }

    #[tokio::test]
    async fn from_fn_runs_parts_extractors_before_request() {
        async fn require_post(method: Method, request: IncomingRequest, next: Next) -> Response {
            if method != Method::POST {
                return StatusCode::METHOD_NOT_ALLOWED.into_response();
            }
            next.run(request).await
        }

        async fn handler(body: Bytes) -> String {
            String::from_utf8(body.to_vec()).unwrap()
        }

        let mut service = Router::new()
            .route("/", post(handler))
            .layer(from_fn(require_post))
            .into_service();

        let response = service
            .ready()
            .await
            .unwrap()
            .call(
                Request::builder()
                    .method(Method::POST)
                    .uri("/")
                    .body(rest::full_body("payload"))
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"payload");
    }

    #[tokio::test]
    async fn from_fn_can_short_circuit() {
        async fn reject(_request: IncomingRequest, _next: Next) -> StatusCode {
            StatusCode::UNAUTHORIZED
        }

        let mut service = Router::new()
            .route("/", get(|| async { "ok" }))
            .layer(from_fn(reject))
            .into_service();

        let response = service
            .ready()
            .await
            .unwrap()
            .call(
                Request::builder()
                    .uri("/")
                    .body(rest::empty_body())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn from_fn_with_state_makes_state_available_to_middleware() {
        async fn middleware(
            State(app): State<AppName>,
            request: IncomingRequest,
            next: Next,
        ) -> Response {
            let mut response = next.run(request).await;
            response
                .headers_mut()
                .insert("x-roze-state", app.0.parse().unwrap());
            response
        }

        let mut service = Router::new()
            .route("/", get(|| async { "ok" }))
            .layer(from_fn_with_state(AppName("roze"), middleware))
            .into_service();

        let response = service
            .ready()
            .await
            .unwrap()
            .call(
                Request::builder()
                    .uri("/")
                    .body(rest::empty_body())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["x-roze-state"], "roze");
    }

    #[tokio::test]
    async fn from_fn_with_state_makes_state_available_to_parts_extractors() {
        async fn middleware(
            Extension(app): Extension<AppName>,
            request: IncomingRequest,
            next: Next,
        ) -> Response {
            let mut response = next.run(request).await;
            response
                .headers_mut()
                .insert("x-roze-extension", app.0.parse().unwrap());
            response
        }

        let mut service = Router::new()
            .route("/", get(|| async { "ok" }))
            .layer(from_fn_with_state(AppName("roze"), middleware))
            .into_service();

        let response = service
            .ready()
            .await
            .unwrap()
            .call(
                Request::builder()
                    .uri("/")
                    .body(rest::empty_body())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["x-roze-extension"], "roze");
    }

    #[tokio::test]
    async fn map_response_can_modify_downstream_response() {
        async fn add_header(mut response: Response) -> Response {
            response
                .headers_mut()
                .insert("x-roze-map-response", "yes".parse().unwrap());
            response
        }

        let mut service = Router::new()
            .route("/", get(|| async { "ok" }))
            .layer(map_response(add_header))
            .into_service();

        let response = service
            .ready()
            .await
            .unwrap()
            .call(
                Request::builder()
                    .uri("/")
                    .body(rest::empty_body())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["x-roze-map-response"], "yes");
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"ok");
    }

    #[tokio::test]
    async fn map_response_can_replace_with_into_response() {
        async fn replace(_response: Response) -> (StatusCode, &'static str) {
            (StatusCode::ACCEPTED, "mapped")
        }

        let mut service = Router::new()
            .route("/", get(|| async { "ok" }))
            .layer(map_response(replace))
            .into_service();

        let response = service
            .ready()
            .await
            .unwrap()
            .call(
                Request::builder()
                    .uri("/")
                    .body(rest::empty_body())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"mapped");
    }

    #[tokio::test]
    async fn map_request_can_modify_request_before_downstream() {
        async fn add_header(mut request: IncomingRequest) -> IncomingRequest {
            request
                .headers_mut()
                .insert("x-roze-map-request", "yes".parse().unwrap());
            request
        }

        async fn handler(headers: HeaderMap) -> String {
            headers["x-roze-map-request"].to_str().unwrap().to_string()
        }

        let mut service = Router::new()
            .route("/", get(handler))
            .layer(map_request(add_header))
            .into_service();

        let response = service
            .ready()
            .await
            .unwrap()
            .call(
                Request::builder()
                    .uri("/")
                    .body(rest::empty_body())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"yes");
    }

    #[tokio::test]
    async fn map_request_can_short_circuit_with_into_response_error() {
        async fn require_header(
            request: IncomingRequest,
        ) -> Result<IncomingRequest, (StatusCode, &'static str)> {
            if request.headers().contains_key("authorization") {
                Ok(request)
            } else {
                Err((StatusCode::UNAUTHORIZED, "missing authorization"))
            }
        }

        let mut service = Router::new()
            .route("/", get(|| async { "ok" }))
            .layer(map_request(require_header))
            .into_service();

        let response = service
            .ready()
            .await
            .unwrap()
            .call(
                Request::builder()
                    .uri("/")
                    .body(rest::empty_body())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"missing authorization");
    }
}
