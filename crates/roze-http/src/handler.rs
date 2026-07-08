use std::{convert::Infallible, future::Future, marker::PhantomData};

use tower::{service_fn, util::BoxCloneService, ServiceExt};

use crate::{
    extract::{FromRequest, FromRequestParts},
    response::IntoResponse,
    rest::{HttpResponse, IncomingRequest},
};

pub trait Handler<Args>: Clone + Send + 'static {
    type Future: Future<Output = HttpResponse> + Send + 'static;

    fn call(self, request: IncomingRequest) -> Self::Future;

    fn into_service(self) -> BoxCloneService<IncomingRequest, HttpResponse, Infallible> {
        service_fn(move |request| {
            let handler = self.clone();
            async move { Ok::<_, Infallible>(handler.call(request).await) }
        })
        .boxed_clone()
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

impl<F, Fut, R, A> Handler<(A,)> for F
where
    F: Fn(A) -> Fut + Clone + Send + 'static,
    Fut: Future<Output = R> + Send + 'static,
    R: IntoResponse + 'static,
    A: FromRequest + Send + 'static,
{
    type Future = std::pin::Pin<Box<dyn Future<Output = HttpResponse> + Send>>;

    fn call(self, request: IncomingRequest) -> Self::Future {
        Box::pin(async move {
            let a = match A::from_request(request).await {
                Ok(value) => value,
                Err(rejection) => return rejection.into_response(),
            };
            self(a).await.into_response()
        })
    }
}

impl<F, Fut, R, A, B> Handler<(A, B)> for F
where
    F: Fn(A, B) -> Fut + Clone + Send + 'static,
    Fut: Future<Output = R> + Send + 'static,
    R: IntoResponse + 'static,
    A: FromRequestParts + Send + 'static,
    B: FromRequest + Send + 'static,
{
    type Future = std::pin::Pin<Box<dyn Future<Output = HttpResponse> + Send>>;

    fn call(self, request: IncomingRequest) -> Self::Future {
        Box::pin(async move {
            let (mut parts, body) = request.into_parts();
            let a = match A::from_request_parts(&mut parts).await {
                Ok(value) => value,
                Err(rejection) => return rejection.into_response(),
            };
            let request = http::Request::from_parts(parts, body);
            let b = match B::from_request(request).await {
                Ok(value) => value,
                Err(rejection) => return rejection.into_response(),
            };
            self(a, b).await.into_response()
        })
    }
}

impl<F, Fut, R, A, B, C> Handler<(A, B, C)> for F
where
    F: Fn(A, B, C) -> Fut + Clone + Send + 'static,
    Fut: Future<Output = R> + Send + 'static,
    R: IntoResponse + 'static,
    A: FromRequestParts + Send + 'static,
    B: FromRequestParts + Send + 'static,
    C: FromRequest + Send + 'static,
{
    type Future = std::pin::Pin<Box<dyn Future<Output = HttpResponse> + Send>>;

    fn call(self, request: IncomingRequest) -> Self::Future {
        Box::pin(async move {
            let (mut parts, body) = request.into_parts();
            let a = match A::from_request_parts(&mut parts).await {
                Ok(value) => value,
                Err(rejection) => return rejection.into_response(),
            };
            let b = match B::from_request_parts(&mut parts).await {
                Ok(value) => value,
                Err(rejection) => return rejection.into_response(),
            };
            let request = http::Request::from_parts(parts, body);
            let c = match C::from_request(request).await {
                Ok(value) => value,
                Err(rejection) => return rejection.into_response(),
            };
            self(a, b, c).await.into_response()
        })
    }
}

pub struct HandlerArgs<T>(PhantomData<T>);

#[cfg(test)]
mod tests {
    use http::Request;
    use http_body_util::BodyExt;
    use serde::Deserialize;

    use super::*;
    use crate::{
        extract::{Path, Query},
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
}
