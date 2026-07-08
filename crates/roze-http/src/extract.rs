use std::{future::Future, pin::Pin};

use http::{request::Parts as HttpParts, HeaderMap, Method, Uri};
use http_body_util::BodyExt;
use serde::de::DeserializeOwned;

use crate::{
    response::{IntoResponse, Json},
    rest::IncomingRequest,
    router::{MatchedPath, RouteParams},
};

pub type ExtractFuture<'a, T, E> = Pin<Box<dyn Future<Output = Result<T, E>> + Send + 'a>>;

pub trait FromRequestParts: Sized {
    type Rejection: IntoResponse;

    fn from_request_parts(parts: &mut HttpParts) -> ExtractFuture<'_, Self, Self::Rejection>;
}

pub trait FromRequest: Sized {
    type Rejection: IntoResponse;

    fn from_request(request: IncomingRequest) -> ExtractFuture<'static, Self, Self::Rejection>;
}

impl FromRequest for IncomingRequest {
    type Rejection = std::convert::Infallible;

    fn from_request(request: IncomingRequest) -> ExtractFuture<'static, Self, Self::Rejection> {
        Box::pin(async move { Ok(request) })
    }
}

pub struct RawRequest(pub IncomingRequest);

impl FromRequest for RawRequest {
    type Rejection = std::convert::Infallible;

    fn from_request(request: IncomingRequest) -> ExtractFuture<'static, Self, Self::Rejection> {
        Box::pin(async move { Ok(Self(request)) })
    }
}

#[derive(Debug, Clone)]
pub struct RequestParts {
    method: Method,
    uri: Uri,
    headers: HeaderMap,
}

impl RequestParts {
    pub fn from_request(request: &IncomingRequest) -> Self {
        Self {
            method: request.method().clone(),
            uri: request.uri().clone(),
            headers: request.headers().clone(),
        }
    }

    pub fn method(&self) -> &Method {
        &self.method
    }

    pub fn uri(&self) -> &Uri {
        &self.uri
    }

    pub fn headers(&self) -> &HeaderMap {
        &self.headers
    }

    pub fn header_str(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|value| value.to_str().ok())
    }

    pub fn query(&self) -> Option<&str> {
        self.uri.query()
    }

    pub fn query_pairs(&self) -> Vec<(&str, &str)> {
        self.query()
            .map(|query| {
                query
                    .split('&')
                    .filter_map(|part| part.split_once('='))
                    .collect()
            })
            .unwrap_or_default()
    }
}

pub struct Parts(pub RequestParts);

impl FromRequest for Parts {
    type Rejection = std::convert::Infallible;

    fn from_request(request: IncomingRequest) -> ExtractFuture<'static, Self, Self::Rejection> {
        Box::pin(async move { Ok(Self(RequestParts::from_request(&request))) })
    }
}

impl FromRequestParts for Parts {
    type Rejection = std::convert::Infallible;

    fn from_request_parts(parts: &mut HttpParts) -> ExtractFuture<'_, Self, Self::Rejection> {
        let parts = RequestParts {
            method: parts.method.clone(),
            uri: parts.uri.clone(),
            headers: parts.headers.clone(),
        };
        Box::pin(async move { Ok(Self(parts)) })
    }
}

impl FromRequest for HeaderMap {
    type Rejection = std::convert::Infallible;

    fn from_request(request: IncomingRequest) -> ExtractFuture<'static, Self, Self::Rejection> {
        Box::pin(async move { Ok(request.headers().clone()) })
    }
}

impl FromRequestParts for HeaderMap {
    type Rejection = std::convert::Infallible;

    fn from_request_parts(parts: &mut HttpParts) -> ExtractFuture<'_, Self, Self::Rejection> {
        let headers = parts.headers.clone();
        Box::pin(async move { Ok(headers) })
    }
}

impl FromRequestParts for MatchedPath {
    type Rejection = roze_error::RozeError;

    fn from_request_parts(parts: &mut HttpParts) -> ExtractFuture<'_, Self, Self::Rejection> {
        let matched_path = parts.extensions.get::<MatchedPath>().cloned();
        Box::pin(async move {
            matched_path
                .ok_or_else(|| roze_error::RozeError::Internal("missing matched path".to_string()))
        })
    }
}

impl FromRequest for MatchedPath {
    type Rejection = roze_error::RozeError;

    fn from_request(request: IncomingRequest) -> ExtractFuture<'static, Self, Self::Rejection> {
        Box::pin(async move {
            request
                .extensions()
                .get::<MatchedPath>()
                .cloned()
                .ok_or_else(|| roze_error::RozeError::Internal("missing matched path".to_string()))
        })
    }
}

pub struct Path<T>(pub T);

impl<T> FromRequest for Path<T>
where
    T: DeserializeOwned + Send + 'static,
{
    type Rejection = roze_error::RozeError;

    fn from_request(request: IncomingRequest) -> ExtractFuture<'static, Self, Self::Rejection> {
        Box::pin(async move {
            let params = request
                .extensions()
                .get::<RouteParams>()
                .cloned()
                .unwrap_or_default()
                .encoded();
            let value = serde_urlencoded::from_str(&params)
                .map_err(|error| roze_error::RozeError::BadRequest(error.to_string()))?;
            Ok(Self(value))
        })
    }
}

impl<T> FromRequestParts for Path<T>
where
    T: DeserializeOwned + Send + 'static,
{
    type Rejection = roze_error::RozeError;

    fn from_request_parts(parts: &mut HttpParts) -> ExtractFuture<'_, Self, Self::Rejection> {
        let params = parts
            .extensions
            .get::<RouteParams>()
            .cloned()
            .unwrap_or_default()
            .encoded();
        Box::pin(async move {
            let value = serde_urlencoded::from_str(&params)
                .map_err(|error| roze_error::RozeError::BadRequest(error.to_string()))?;
            Ok(Self(value))
        })
    }
}

pub struct Query<T>(pub T);

impl<T> FromRequest for Query<T>
where
    T: DeserializeOwned + Send + 'static,
{
    type Rejection = roze_error::RozeError;

    fn from_request(request: IncomingRequest) -> ExtractFuture<'static, Self, Self::Rejection> {
        Box::pin(async move {
            let query = request.uri().query().unwrap_or_default();
            let value = serde_urlencoded::from_str(query)
                .map_err(|error| roze_error::RozeError::BadRequest(error.to_string()))?;
            Ok(Self(value))
        })
    }
}

impl<T> FromRequestParts for Query<T>
where
    T: DeserializeOwned + Send + 'static,
{
    type Rejection = roze_error::RozeError;

    fn from_request_parts(parts: &mut HttpParts) -> ExtractFuture<'_, Self, Self::Rejection> {
        let query = parts.uri.query().unwrap_or_default().to_string();
        Box::pin(async move {
            let value = serde_urlencoded::from_str(&query)
                .map_err(|error| roze_error::RozeError::BadRequest(error.to_string()))?;
            Ok(Self(value))
        })
    }
}

pub struct Form<T>(pub T);

impl<T> FromRequest for Form<T>
where
    T: DeserializeOwned + Send + 'static,
{
    type Rejection = roze_error::RozeError;

    fn from_request(request: IncomingRequest) -> ExtractFuture<'static, Self, Self::Rejection> {
        Box::pin(async move {
            let body = request
                .into_body()
                .collect()
                .await
                .map_err(|error| roze_error::RozeError::BadRequest(error.to_string()))?
                .to_bytes();
            let value = serde_urlencoded::from_bytes(&body)
                .map_err(|error| roze_error::RozeError::BadRequest(error.to_string()))?;
            Ok(Self(value))
        })
    }
}

pub struct Extension<T>(pub T);

impl<T> FromRequest for Extension<T>
where
    T: Clone + Send + Sync + 'static,
{
    type Rejection = roze_error::RozeError;

    fn from_request(request: IncomingRequest) -> ExtractFuture<'static, Self, Self::Rejection> {
        Box::pin(async move {
            request
                .extensions()
                .get::<T>()
                .cloned()
                .map(Self)
                .ok_or_else(|| roze_error::RozeError::Internal("missing extension".to_string()))
        })
    }
}

impl<T> FromRequestParts for Extension<T>
where
    T: Clone + Send + Sync + 'static,
{
    type Rejection = roze_error::RozeError;

    fn from_request_parts(parts: &mut HttpParts) -> ExtractFuture<'_, Self, Self::Rejection> {
        let value = parts.extensions.get::<T>().cloned();
        Box::pin(async move {
            value
                .map(Self)
                .ok_or_else(|| roze_error::RozeError::Internal("missing extension".to_string()))
        })
    }
}

pub struct State<T>(pub T);

impl<T> FromRequest for State<T>
where
    T: Clone + Send + Sync + 'static,
{
    type Rejection = roze_error::RozeError;

    fn from_request(request: IncomingRequest) -> ExtractFuture<'static, Self, Self::Rejection> {
        Box::pin(async move {
            request
                .extensions()
                .get::<T>()
                .cloned()
                .map(Self)
                .ok_or_else(|| roze_error::RozeError::Internal("missing state".to_string()))
        })
    }
}

impl<T> FromRequestParts for State<T>
where
    T: Clone + Send + Sync + 'static,
{
    type Rejection = roze_error::RozeError;

    fn from_request_parts(parts: &mut HttpParts) -> ExtractFuture<'_, Self, Self::Rejection> {
        let value = parts.extensions.get::<T>().cloned();
        Box::pin(async move {
            value
                .map(Self)
                .ok_or_else(|| roze_error::RozeError::Internal("missing state".to_string()))
        })
    }
}

impl<T> FromRequest for Json<T>
where
    T: DeserializeOwned + Send + 'static,
{
    type Rejection = roze_error::RozeError;

    fn from_request(request: IncomingRequest) -> ExtractFuture<'static, Self, Self::Rejection> {
        Box::pin(async move {
            let body = request
                .into_body()
                .collect()
                .await
                .map_err(|error| roze_error::RozeError::BadRequest(error.to_string()))?
                .to_bytes();
            let value = serde_json::from_slice(&body)
                .map_err(|error| roze_error::RozeError::BadRequest(error.to_string()))?;
            Ok(Self(value))
        })
    }
}

#[cfg(test)]
mod tests {
    use http::Request;
    use serde::Deserialize;

    use super::*;
    use crate::rest;

    #[derive(Debug, Deserialize, PartialEq, Eq)]
    struct Payload {
        name: String,
    }

    #[tokio::test]
    async fn extracts_json_body() {
        let request = Request::builder()
            .method("POST")
            .uri("/users")
            .body(rest::full_body(r#"{"name":"roze"}"#))
            .unwrap();

        let Json(payload) = Json::<Payload>::from_request(request)
            .await
            .expect("json body");
        assert_eq!(
            payload,
            Payload {
                name: "roze".to_string()
            }
        );
    }

    #[tokio::test]
    async fn extracts_request_parts() {
        let request = Request::builder()
            .method("GET")
            .uri("/users?name=roze")
            .header("x-roze-test", "yes")
            .body(rest::empty_body())
            .unwrap();

        let Parts(parts) = Parts::from_request(request).await.expect("parts");
        assert_eq!(parts.method(), Method::GET);
        assert_eq!(parts.header_str("x-roze-test"), Some("yes"));
        assert_eq!(parts.query_pairs(), vec![("name", "roze")]);
    }

    #[tokio::test]
    async fn extracts_query_string() {
        let request = Request::builder()
            .method("GET")
            .uri("/users?name=roze")
            .body(rest::empty_body())
            .unwrap();

        let Query(payload) = Query::<Payload>::from_request(request)
            .await
            .expect("query string");
        assert_eq!(
            payload,
            Payload {
                name: "roze".to_string()
            }
        );
    }

    #[tokio::test]
    async fn extracts_path_params() {
        let mut request = Request::builder()
            .method("GET")
            .uri("/users/roze")
            .body(rest::empty_body())
            .unwrap();
        request.extensions_mut().insert(RouteParams::from_pairs([(
            "name".to_string(),
            "roze".to_string(),
        )]));

        let Path(payload) = Path::<Payload>::from_request(request)
            .await
            .expect("path params");
        assert_eq!(
            payload,
            Payload {
                name: "roze".to_string()
            }
        );
    }
}
