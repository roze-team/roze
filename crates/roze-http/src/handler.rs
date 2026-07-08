use std::{convert::Infallible, future::Future, marker::PhantomData, pin::Pin};

use tower::{service_fn, util::BoxCloneService, Layer, Service, ServiceExt};

use crate::{
    extract::{FromRef, FromRequest, FromRequestParts},
    response::IntoResponse,
    rest::{HttpResponse, IncomingRequest},
};

pub trait Handler<Args>: Clone + Send + 'static {
    type Future: Future<Output = HttpResponse> + Send + 'static;

    fn call(self, request: IncomingRequest) -> Self::Future;

    fn with_state<T>(self, state: T) -> LayeredHandler<Args>
    where
        T: Clone + Send + Sync + 'static,
    {
        self.layer(HandlerStateLayer { state })
    }

    fn with_state_from_ref<Outer, Inner>(self, state: Outer) -> LayeredHandler<Args>
    where
        Outer: Clone + Send + Sync + 'static,
        Inner: FromRef<Outer> + Clone + Send + Sync + 'static,
    {
        self.layer(HandlerStateFromRefLayer::<Outer, Inner>::new(state))
    }

    fn layer<L>(self, layer: L) -> LayeredHandler<Args>
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
        LayeredHandler {
            service: layer.layer(self.into_service()).boxed_clone(),
            _marker: PhantomData,
        }
    }

    fn into_service(self) -> BoxCloneService<IncomingRequest, HttpResponse, Infallible> {
        service_fn(move |request| {
            let handler = self.clone();
            async move { Ok::<_, Infallible>(handler.call(request).await) }
        })
        .boxed_clone()
    }
}

#[derive(Clone)]
struct HandlerStateLayer<T> {
    state: T,
}

impl<T> Layer<BoxCloneService<IncomingRequest, HttpResponse, Infallible>> for HandlerStateLayer<T>
where
    T: Clone + Send + Sync + 'static,
{
    type Service = HandlerStateService<T>;

    fn layer(
        &self,
        inner: BoxCloneService<IncomingRequest, HttpResponse, Infallible>,
    ) -> Self::Service {
        HandlerStateService {
            state: self.state.clone(),
            inner,
        }
    }
}

#[derive(Clone)]
struct HandlerStateService<T> {
    state: T,
    inner: BoxCloneService<IncomingRequest, HttpResponse, Infallible>,
}

impl<T> Service<IncomingRequest> for HandlerStateService<T>
where
    T: Clone + Send + Sync + 'static,
{
    type Response = HttpResponse;
    type Error = Infallible;
    type Future = Pin<Box<dyn Future<Output = Result<HttpResponse, Infallible>> + Send>>;

    fn poll_ready(
        &mut self,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut request: IncomingRequest) -> Self::Future {
        request.extensions_mut().insert(self.state.clone());
        let mut inner = self.inner.clone();
        Box::pin(async move { inner.call(request).await })
    }
}

#[derive(Clone)]
struct HandlerStateFromRefLayer<Outer, Inner> {
    state: Outer,
    _marker: PhantomData<fn() -> Inner>,
}

impl<Outer, Inner> HandlerStateFromRefLayer<Outer, Inner> {
    fn new(state: Outer) -> Self {
        Self {
            state,
            _marker: PhantomData,
        }
    }
}

impl<Outer, Inner> Layer<BoxCloneService<IncomingRequest, HttpResponse, Infallible>>
    for HandlerStateFromRefLayer<Outer, Inner>
where
    Outer: Clone + Send + Sync + 'static,
    Inner: FromRef<Outer> + Clone + Send + Sync + 'static,
{
    type Service = HandlerStateFromRefService<Outer, Inner>;

    fn layer(
        &self,
        inner: BoxCloneService<IncomingRequest, HttpResponse, Infallible>,
    ) -> Self::Service {
        HandlerStateFromRefService {
            state: self.state.clone(),
            inner,
            _marker: PhantomData,
        }
    }
}

#[derive(Clone)]
struct HandlerStateFromRefService<Outer, Inner> {
    state: Outer,
    inner: BoxCloneService<IncomingRequest, HttpResponse, Infallible>,
    _marker: PhantomData<fn() -> Inner>,
}

impl<Outer, Inner> Service<IncomingRequest> for HandlerStateFromRefService<Outer, Inner>
where
    Outer: Clone + Send + Sync + 'static,
    Inner: FromRef<Outer> + Clone + Send + Sync + 'static,
{
    type Response = HttpResponse;
    type Error = Infallible;
    type Future = Pin<Box<dyn Future<Output = Result<HttpResponse, Infallible>> + Send>>;

    fn poll_ready(
        &mut self,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
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

pub struct LayeredHandler<Args> {
    service: BoxCloneService<IncomingRequest, HttpResponse, Infallible>,
    _marker: PhantomData<fn() -> Args>,
}

impl<Args> Clone for LayeredHandler<Args> {
    fn clone(&self) -> Self {
        Self {
            service: self.service.clone(),
            _marker: PhantomData,
        }
    }
}

impl<Args> Handler<Args> for LayeredHandler<Args>
where
    Args: 'static,
{
    type Future = Pin<Box<dyn Future<Output = HttpResponse> + Send>>;

    fn call(self, request: IncomingRequest) -> Self::Future {
        Box::pin(async move {
            let mut service = self.service.clone();
            service
                .ready()
                .await
                .expect("infallible handler layer")
                .call(request)
                .await
                .expect("infallible handler layer")
        })
    }
}

pub struct NoArgs;

impl<T> Handler<StaticResponse> for T
where
    T: IntoResponse + Clone + Send + 'static,
{
    type Future = std::future::Ready<HttpResponse>;

    fn call(self, _request: IncomingRequest) -> Self::Future {
        std::future::ready(self.into_response())
    }
}

pub struct StaticResponse;

impl<F, Fut, R> Handler<NoArgs> for F
where
    F: Fn() -> Fut + Clone + Send + 'static,
    Fut: Future<Output = R> + Send + 'static,
    R: IntoResponse + 'static,
{
    type Future = std::pin::Pin<Box<dyn Future<Output = HttpResponse> + Send>>;

    fn call(self, _request: IncomingRequest) -> Self::Future {
        Box::pin(async move { self().await.into_response() })
    }
}

macro_rules! impl_handler {
    ($last:ident) => {
        impl<F, Fut, R, $last> Handler<($last,)> for F
        where
            F: Fn($last) -> Fut + Clone + Send + 'static,
            Fut: Future<Output = R> + Send + 'static,
            R: IntoResponse + 'static,
            $last: FromRequest + Send + 'static,
        {
            type Future = Pin<Box<dyn Future<Output = HttpResponse> + Send>>;

            #[allow(non_snake_case)]
            fn call(self, request: IncomingRequest) -> Self::Future {
                Box::pin(async move {
                    let $last = match $last::from_request(request).await {
                        Ok(value) => value,
                        Err(rejection) => return rejection.into_response(),
                    };
                    self($last).await.into_response()
                })
            }
        }
    };
    ($($ty:ident),+ => $last:ident) => {
        impl<F, Fut, R, $($ty,)* $last> Handler<($($ty,)* $last)> for F
        where
            F: Fn($($ty,)* $last) -> Fut + Clone + Send + 'static,
            Fut: Future<Output = R> + Send + 'static,
            R: IntoResponse + 'static,
            $($ty: FromRequestParts + Send + 'static,)*
            $last: FromRequest + Send + 'static,
        {
            type Future = Pin<Box<dyn Future<Output = HttpResponse> + Send>>;

            #[allow(non_snake_case)]
            fn call(self, request: IncomingRequest) -> Self::Future {
                Box::pin(async move {
                    let (mut parts, body) = request.into_parts();
                    $(
                        let $ty = match $ty::from_request_parts(&mut parts).await {
                            Ok(value) => value,
                            Err(rejection) => return rejection.into_response(),
                        };
                    )*
                    let request = http::Request::from_parts(parts, body);
                    let $last = match $last::from_request(request).await {
                        Ok(value) => value,
                        Err(rejection) => return rejection.into_response(),
                    };
                    self($($ty,)* $last).await.into_response()
                })
            }
        }
    };
}

impl_handler!(A);
impl_handler!(A => B);
impl_handler!(A, B => C);
impl_handler!(A, B, C => D);
impl_handler!(A, B, C, D => E);
impl_handler!(A, B, C, D, E => G);
impl_handler!(A, B, C, D, E, G => H);
impl_handler!(A, B, C, D, E, G, H => I);

pub struct HandlerArgs<T>(PhantomData<T>);

#[cfg(test)]
mod tests {
    use http::{HeaderMap, HeaderValue, Request};
    use http_body_util::BodyExt;
    use serde::Deserialize;
    use std::task::{Context, Poll};

    use super::*;
    use crate::{
        extract::{Host, OriginalUri, Parts, Path, Query, State},
        response::Json,
        rest,
        router::RouteParams,
    };

    #[derive(Debug, Deserialize)]
    struct IdPath {
        id: String,
    }

    #[derive(Debug, Deserialize)]
    struct SearchQuery {
        q: String,
    }

    #[derive(Debug, Deserialize)]
    struct BodyPayload {
        name: String,
    }

    #[tokio::test]
    async fn extracts_handler_arguments() {
        let handler = |Path(path): Path<IdPath>, Query(query): Query<SearchQuery>| async move {
            format!("{}:{}", path.id, query.q)
        };
        let mut request = Request::builder()
            .uri("/users/42?q=roze")
            .body(rest::empty_body())
            .unwrap();
        request.extensions_mut().insert(RouteParams::from_pairs([(
            "id".to_string(),
            "42".to_string(),
        )]));

        let response = Handler::<(Path<IdPath>, Query<SearchQuery>)>::call(handler, request).await;
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"42:roze");
    }

    #[tokio::test]
    async fn extracts_parts_then_body_argument() {
        let handler = |Path(path): Path<IdPath>,
                       Query(query): Query<SearchQuery>,
                       Json(body): Json<BodyPayload>| async move {
            format!("{}:{}:{}", path.id, query.q, body.name)
        };
        let mut request = Request::builder()
            .method("POST")
            .uri("/users/42?q=roze")
            .body(crate::rest::full_body(r#"{"name":"body"}"#))
            .unwrap();
        request.extensions_mut().insert(RouteParams::from_pairs([(
            "id".to_string(),
            "42".to_string(),
        )]));

        let response = Handler::<(Path<IdPath>, Query<SearchQuery>, Json<BodyPayload>)>::call(
            handler, request,
        )
        .await;
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"42:roze:body");
    }

    #[tokio::test]
    async fn extracts_four_handler_arguments() {
        let handler = |Path(path): Path<IdPath>,
                       Query(query): Query<SearchQuery>,
                       headers: HeaderMap,
                       Json(body): Json<BodyPayload>| async move {
            format!(
                "{}:{}:{}:{}",
                path.id,
                query.q,
                headers
                    .get("x-roze-test")
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or_default(),
                body.name
            )
        };
        let mut request = Request::builder()
            .method("POST")
            .uri("/users/42?q=roze")
            .header("x-roze-test", "yes")
            .body(rest::full_body(r#"{"name":"body"}"#))
            .unwrap();
        request.extensions_mut().insert(RouteParams::from_pairs([(
            "id".to_string(),
            "42".to_string(),
        )]));

        let response = Handler::<(
            Path<IdPath>,
            Query<SearchQuery>,
            HeaderMap,
            Json<BodyPayload>,
        )>::call(handler, request)
        .await;
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"42:roze:yes:body");
    }

    #[tokio::test]
    async fn extracts_basic_http_parts_in_handler() {
        let handler = |method: http::Method,
                       uri: http::Uri,
                       version: http::Version,
                       Json(body): Json<BodyPayload>| async move {
            format!("{}:{}:{:?}:{}", method, uri, version, body.name)
        };
        let request = Request::builder()
            .method("PUT")
            .uri("/users/42?name=roze")
            .version(http::Version::HTTP_2)
            .body(rest::full_body(r#"{"name":"body"}"#))
            .unwrap();

        let response =
            Handler::<(http::Method, http::Uri, http::Version, Json<BodyPayload>)>::call(
                handler, request,
            )
            .await;
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"PUT:/users/42?name=roze:HTTP/2.0:body");
    }

    #[tokio::test]
    async fn extracts_host_in_handler() {
        let handler = |Host(host): Host| async move { host };
        let request = Request::builder()
            .method("GET")
            .uri("/users")
            .header(http::header::HOST, "tenant.example.com")
            .body(rest::empty_body())
            .unwrap();

        let response = Handler::<(Host,)>::call(handler, request).await;
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"tenant.example.com");
    }

    #[tokio::test]
    async fn result_body_extractor_lets_handler_handle_rejection() {
        let handler = |payload: Result<Json<BodyPayload>, roze_error::RozeError>| async move {
            match payload {
                Ok(Json(body)) => body.name,
                Err(_) => "bad-json".to_string(),
            }
        };
        let request = Request::builder()
            .method("POST")
            .uri("/users")
            .body(rest::full_body("{bad json"))
            .unwrap();

        let response =
            Handler::<(Result<Json<BodyPayload>, roze_error::RozeError>,)>::call(handler, request)
                .await;
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"bad-json");
    }

    #[tokio::test]
    async fn extracts_raw_bytes_body_in_handler() {
        let handler = |body: bytes::Bytes| async move { body.len().to_string() };
        let request = Request::builder()
            .method("POST")
            .uri("/webhook")
            .body(rest::full_body("raw-body"))
            .unwrap();

        let response = Handler::<(bytes::Bytes,)>::call(handler, request).await;
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"8");
    }

    #[tokio::test]
    async fn extracts_string_body_in_handler() {
        let handler = |body: String| async move { format!("body:{body}") };
        let request = Request::builder()
            .method("POST")
            .uri("/webhook")
            .body(rest::full_body("hello"))
            .unwrap();

        let response = Handler::<(String,)>::call(handler, request).await;
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"body:hello");
    }

    #[tokio::test]
    async fn extracts_request_alias_in_handler() {
        let handler =
            |request: crate::extract::Request| async move { request.uri().path().to_string() };
        let request = Request::builder()
            .method("POST")
            .uri("/webhook")
            .body(rest::empty_body())
            .unwrap();

        let response = Handler::<(crate::extract::Request,)>::call(handler, request).await;
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"/webhook");
    }

    #[tokio::test]
    async fn extracts_eight_handler_arguments() {
        let handler = |Path(path): Path<IdPath>,
                       Query(query): Query<SearchQuery>,
                       headers: HeaderMap,
                       Parts(parts): Parts,
                       state: Option<State<String>>,
                       optional_extension: Option<crate::extract::Extension<usize>>,
                       original_uri: OriginalUri,
                       Json(body): Json<BodyPayload>| async move {
            format!(
                "{}:{}:{}:{}:{}:{}:{}:{}",
                path.id,
                query.q,
                headers
                    .get("x-roze-test")
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or_default(),
                parts.method(),
                state.map(|State(value)| value).unwrap_or_default(),
                optional_extension
                    .map(|crate::extract::Extension(value)| value.to_string())
                    .unwrap_or_else(|| "none".to_string()),
                original_uri.to_string(),
                body.name
            )
        };
        let mut request = Request::builder()
            .method("POST")
            .uri("/users/42?q=roze")
            .header("x-roze-test", HeaderValue::from_static("yes"))
            .body(rest::full_body(r#"{"name":"body"}"#))
            .unwrap();
        request.extensions_mut().insert(RouteParams::from_pairs([(
            "id".to_string(),
            "42".to_string(),
        )]));
        request.extensions_mut().insert(String::from("state"));

        let response = Handler::<(
            Path<IdPath>,
            Query<SearchQuery>,
            HeaderMap,
            Parts,
            Option<State<String>>,
            Option<crate::extract::Extension<usize>>,
            OriginalUri,
            Json<BodyPayload>,
        )>::call(handler, request)
        .await;
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(
            &body[..],
            b"42:roze:yes:POST:state:none:/users/42?q=roze:body"
        );
    }

    #[tokio::test]
    async fn handler_layer_wraps_single_handler() {
        let handler = (|| async { "layered" }).layer(TestHeaderLayer);
        let response = Handler::<NoArgs>::call(
            handler,
            Request::builder().body(rest::empty_body()).unwrap(),
        )
        .await;

        assert_eq!(
            response.headers().get("x-roze-handler-layer"),
            Some(&HeaderValue::from_static("yes"))
        );
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"layered");
    }

    #[tokio::test]
    async fn handler_layer_can_map_fallible_layer_errors() {
        let handler = (|| async { "unreachable" }).layer(crate::error_handling::handle_error(
            FallibleLayer,
            |FallibleError| async { (http::StatusCode::BAD_GATEWAY, "handler-layer-error") },
        ));
        let response = Handler::<NoArgs>::call(
            handler,
            Request::builder().body(rest::empty_body()).unwrap(),
        )
        .await;

        assert_eq!(response.status(), http::StatusCode::BAD_GATEWAY);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"handler-layer-error");
    }

    #[tokio::test]
    async fn handler_with_state_injects_state_for_single_handler() {
        let handler = (|State(state): State<String>| async move { state })
            .with_state("handler-state".to_string());
        let response = Handler::<(State<String>,)>::call(
            handler,
            Request::builder().body(rest::empty_body()).unwrap(),
        )
        .await;

        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"handler-state");
    }

    #[tokio::test]
    async fn handler_with_state_from_ref_injects_outer_and_inner_state() {
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

        let handler = (|State(app): State<AppState>, State(api): State<ApiState>| async move {
            format!("{}:{}", app.name, api.tenant)
        })
        .with_state_from_ref::<AppState, ApiState>(AppState {
            api: ApiState {
                tenant: "tenant-a".to_string(),
            },
            name: "app".to_string(),
        });
        let response = Handler::<(State<AppState>, State<ApiState>)>::call(
            handler,
            Request::builder().body(rest::empty_body()).unwrap(),
        )
        .await;

        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"app:tenant-a");
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
        type Future = Pin<Box<dyn Future<Output = Result<HttpResponse, Infallible>> + Send>>;

        fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
            self.inner.poll_ready(cx)
        }

        fn call(&mut self, request: IncomingRequest) -> Self::Future {
            let mut inner = self.inner.clone();
            Box::pin(async move {
                let mut response = inner.call(request).await?;
                response
                    .headers_mut()
                    .insert("x-roze-handler-layer", HeaderValue::from_static("yes"));
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
