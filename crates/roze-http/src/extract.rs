use std::{
    future::Future,
    ops::{Deref, DerefMut},
    pin::Pin,
};

use bytes::Bytes;
use http::{header, request::Parts as HttpParts, HeaderMap, Method, Uri, Version};
use http_body_util::BodyExt;
use serde::de::DeserializeOwned;

use crate::{
    response::{IntoResponse, Json},
    rest::IncomingRequest,
    router::{MatchedPath, RouteParams},
};

pub type Request = IncomingRequest;
pub type ExtractFuture<'a, T, E> = Pin<Box<dyn Future<Output = Result<T, E>> + Send + 'a>>;

pub trait FromRequestParts: Sized {
    type Rejection: IntoResponse;

    fn from_request_parts(parts: &mut HttpParts) -> ExtractFuture<'_, Self, Self::Rejection>;
}

mod private {
    pub trait Sealed {}

    impl Sealed for super::IncomingRequest {}
    impl Sealed for http::request::Parts {}
}

pub trait RequestPartsExt: private::Sealed {
    fn extract<E>(&mut self) -> ExtractFuture<'_, E, E::Rejection>
    where
        E: FromRequestParts;

    fn extract_optional<E>(&mut self) -> ExtractFuture<'_, Option<E>, E::Rejection>
    where
        E: OptionalFromRequestParts;
}

impl RequestPartsExt for HttpParts {
    fn extract<E>(&mut self) -> ExtractFuture<'_, E, E::Rejection>
    where
        E: FromRequestParts,
    {
        E::from_request_parts(self)
    }

    fn extract_optional<E>(&mut self) -> ExtractFuture<'_, Option<E>, E::Rejection>
    where
        E: OptionalFromRequestParts,
    {
        E::optional_from_request_parts(self)
    }
}

pub trait RequestExt: private::Sealed {
    fn extract<E>(self) -> ExtractFuture<'static, E, E::Rejection>
    where
        E: FromRequest;

    fn extract_optional<E>(self) -> ExtractFuture<'static, Option<E>, E::Rejection>
    where
        E: OptionalFromRequest;

    fn extract_parts<E>(&mut self) -> ExtractFuture<'_, E, E::Rejection>
    where
        E: FromRequestParts;

    fn extract_optional_parts<E>(&mut self) -> ExtractFuture<'_, Option<E>, E::Rejection>
    where
        E: OptionalFromRequestParts;
}

impl RequestExt for IncomingRequest {
    fn extract<E>(self) -> ExtractFuture<'static, E, E::Rejection>
    where
        E: FromRequest,
    {
        E::from_request(self)
    }

    fn extract_optional<E>(self) -> ExtractFuture<'static, Option<E>, E::Rejection>
    where
        E: OptionalFromRequest,
    {
        E::optional_from_request(self)
    }

    fn extract_parts<E>(&mut self) -> ExtractFuture<'_, E, E::Rejection>
    where
        E: FromRequestParts,
    {
        Box::pin(async move {
            let mut request = http::Request::new(());
            *request.method_mut() = self.method().clone();
            *request.uri_mut() = self.uri().clone();
            *request.version_mut() = self.version();
            *request.headers_mut() = std::mem::take(self.headers_mut());
            *request.extensions_mut() = std::mem::take(self.extensions_mut());

            let (mut parts, ()) = request.into_parts();
            let result = E::from_request_parts(&mut parts).await;

            *self.method_mut() = parts.method;
            *self.uri_mut() = parts.uri;
            *self.version_mut() = parts.version;
            *self.headers_mut() = std::mem::take(&mut parts.headers);
            *self.extensions_mut() = std::mem::take(&mut parts.extensions);

            result
        })
    }

    fn extract_optional_parts<E>(&mut self) -> ExtractFuture<'_, Option<E>, E::Rejection>
    where
        E: OptionalFromRequestParts,
    {
        Box::pin(async move {
            let mut request = http::Request::new(());
            *request.method_mut() = self.method().clone();
            *request.uri_mut() = self.uri().clone();
            *request.version_mut() = self.version();
            *request.headers_mut() = std::mem::take(self.headers_mut());
            *request.extensions_mut() = std::mem::take(self.extensions_mut());

            let (mut parts, ()) = request.into_parts();
            let result = E::optional_from_request_parts(&mut parts).await;

            *self.method_mut() = parts.method;
            *self.uri_mut() = parts.uri;
            *self.version_mut() = parts.version;
            *self.headers_mut() = std::mem::take(&mut parts.headers);
            *self.extensions_mut() = std::mem::take(&mut parts.extensions);

            result
        })
    }
}

pub trait FromRequest: Sized {
    type Rejection: IntoResponse;

    fn from_request(request: IncomingRequest) -> ExtractFuture<'static, Self, Self::Rejection>;
}

pub trait FromRef<T> {
    fn from_ref(input: &T) -> Self;
}

impl<T> FromRef<T> for T
where
    T: Clone,
{
    fn from_ref(input: &T) -> Self {
        input.clone()
    }
}

pub trait OptionalFromRequestParts: Sized {
    type Rejection: IntoResponse;

    fn optional_from_request_parts(
        parts: &mut HttpParts,
    ) -> ExtractFuture<'_, Option<Self>, Self::Rejection>;
}

pub trait OptionalFromRequest: Sized {
    type Rejection: IntoResponse;

    fn optional_from_request(
        request: IncomingRequest,
    ) -> ExtractFuture<'static, Option<Self>, Self::Rejection>;
}

impl<T> FromRequestParts for Option<T>
where
    T: OptionalFromRequestParts,
{
    type Rejection = T::Rejection;

    fn from_request_parts(parts: &mut HttpParts) -> ExtractFuture<'_, Self, Self::Rejection> {
        T::optional_from_request_parts(parts)
    }
}

impl<T> FromRequest for Option<T>
where
    T: OptionalFromRequest,
{
    type Rejection = T::Rejection;

    fn from_request(request: IncomingRequest) -> ExtractFuture<'static, Self, Self::Rejection> {
        T::optional_from_request(request)
    }
}

impl<T> FromRequestParts for Result<T, T::Rejection>
where
    T: FromRequestParts,
{
    type Rejection = std::convert::Infallible;

    fn from_request_parts(parts: &mut HttpParts) -> ExtractFuture<'_, Self, Self::Rejection> {
        Box::pin(async move { Ok(T::from_request_parts(parts).await) })
    }
}

impl<T> FromRequest for Result<T, T::Rejection>
where
    T: FromRequest,
{
    type Rejection = std::convert::Infallible;

    fn from_request(request: IncomingRequest) -> ExtractFuture<'static, Self, Self::Rejection> {
        Box::pin(async move { Ok(T::from_request(request).await) })
    }
}

impl FromRequest for IncomingRequest {
    type Rejection = std::convert::Infallible;

    fn from_request(request: IncomingRequest) -> ExtractFuture<'static, Self, Self::Rejection> {
        Box::pin(async move { Ok(request) })
    }
}

impl FromRequest for Method {
    type Rejection = std::convert::Infallible;

    fn from_request(request: IncomingRequest) -> ExtractFuture<'static, Self, Self::Rejection> {
        let method = request.method().clone();
        Box::pin(async move { Ok(method) })
    }
}

impl FromRequestParts for Method {
    type Rejection = std::convert::Infallible;

    fn from_request_parts(parts: &mut HttpParts) -> ExtractFuture<'_, Self, Self::Rejection> {
        let method = parts.method.clone();
        Box::pin(async move { Ok(method) })
    }
}

impl FromRequest for Uri {
    type Rejection = std::convert::Infallible;

    fn from_request(request: IncomingRequest) -> ExtractFuture<'static, Self, Self::Rejection> {
        let uri = request.uri().clone();
        Box::pin(async move { Ok(uri) })
    }
}

impl FromRequestParts for Uri {
    type Rejection = std::convert::Infallible;

    fn from_request_parts(parts: &mut HttpParts) -> ExtractFuture<'_, Self, Self::Rejection> {
        let uri = parts.uri.clone();
        Box::pin(async move { Ok(uri) })
    }
}

impl FromRequest for Version {
    type Rejection = std::convert::Infallible;

    fn from_request(request: IncomingRequest) -> ExtractFuture<'static, Self, Self::Rejection> {
        let version = request.version();
        Box::pin(async move { Ok(version) })
    }
}

impl FromRequestParts for Version {
    type Rejection = std::convert::Infallible;

    fn from_request_parts(parts: &mut HttpParts) -> ExtractFuture<'_, Self, Self::Rejection> {
        let version = parts.version;
        Box::pin(async move { Ok(version) })
    }
}

impl FromRequest for Bytes {
    type Rejection = roze_error::RozeError;

    fn from_request(request: IncomingRequest) -> ExtractFuture<'static, Self, Self::Rejection> {
        Box::pin(async move {
            let body = request
                .into_body()
                .collect()
                .await
                .map_err(|error| roze_error::RozeError::BadRequest(error.to_string()))?
                .to_bytes();
            Ok(body)
        })
    }
}

impl FromRequest for String {
    type Rejection = roze_error::RozeError;

    fn from_request(request: IncomingRequest) -> ExtractFuture<'static, Self, Self::Rejection> {
        Box::pin(async move {
            let body = Bytes::from_request(request).await?;
            String::from_utf8(body.to_vec())
                .map_err(|error| roze_error::RozeError::BadRequest(error.to_string()))
        })
    }
}

pub struct RawRequest(pub IncomingRequest);

impl Deref for RawRequest {
    type Target = IncomingRequest;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for RawRequest {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

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

impl Deref for Parts {
    type Target = RequestParts;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for Parts {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

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

impl OptionalFromRequestParts for MatchedPath {
    type Rejection = std::convert::Infallible;

    fn optional_from_request_parts(
        parts: &mut HttpParts,
    ) -> ExtractFuture<'_, Option<Self>, Self::Rejection> {
        let matched_path = parts.extensions.get::<MatchedPath>().cloned();
        Box::pin(async move { Ok(matched_path) })
    }
}

impl OptionalFromRequest for MatchedPath {
    type Rejection = std::convert::Infallible;

    fn optional_from_request(
        request: IncomingRequest,
    ) -> ExtractFuture<'static, Option<Self>, Self::Rejection> {
        Box::pin(async move { Ok(request.extensions().get::<MatchedPath>().cloned()) })
    }
}

pub struct Path<T>(pub T);

impl<T> Deref for Path<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> DerefMut for Path<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

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

impl<T> OptionalFromRequestParts for Path<T>
where
    T: DeserializeOwned + Send + 'static,
{
    type Rejection = roze_error::RozeError;

    fn optional_from_request_parts(
        parts: &mut HttpParts,
    ) -> ExtractFuture<'_, Option<Self>, Self::Rejection> {
        let params = parts
            .extensions
            .get::<RouteParams>()
            .cloned()
            .unwrap_or_default()
            .encoded();
        Box::pin(async move {
            if params.is_empty() {
                return Ok(None);
            }
            let value = serde_urlencoded::from_str(&params)
                .map_err(|error| roze_error::RozeError::BadRequest(error.to_string()))?;
            Ok(Some(Self(value)))
        })
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RawPathParams(RouteParams);

impl RawPathParams {
    pub fn get(&self, name: &str) -> Option<&str> {
        self.0.get(name)
    }

    pub fn iter(&self) -> RawPathParamsIter<'_> {
        self.into_iter()
    }

    pub fn is_empty(&self) -> bool {
        self.0.iter().next().is_none()
    }
}

impl<'a> IntoIterator for &'a RawPathParams {
    type Item = (&'a str, &'a str);
    type IntoIter = RawPathParamsIter<'a>;

    fn into_iter(self) -> Self::IntoIter {
        RawPathParamsIter(Box::new(self.0.iter()))
    }
}

pub struct RawPathParamsIter<'a>(Box<dyn Iterator<Item = (&'a str, &'a str)> + Send + Sync + 'a>);

impl<'a> Iterator for RawPathParamsIter<'a> {
    type Item = (&'a str, &'a str);

    fn next(&mut self) -> Option<Self::Item> {
        self.0.next()
    }
}

impl FromRequest for RawPathParams {
    type Rejection = roze_error::RozeError;

    fn from_request(request: IncomingRequest) -> ExtractFuture<'static, Self, Self::Rejection> {
        let params = request.extensions().get::<RouteParams>().cloned();
        Box::pin(async move {
            params
                .map(Self)
                .ok_or_else(|| roze_error::RozeError::Internal("missing path params".to_string()))
        })
    }
}

impl FromRequestParts for RawPathParams {
    type Rejection = roze_error::RozeError;

    fn from_request_parts(parts: &mut HttpParts) -> ExtractFuture<'_, Self, Self::Rejection> {
        let params = parts.extensions.get::<RouteParams>().cloned();
        Box::pin(async move {
            params
                .map(Self)
                .ok_or_else(|| roze_error::RozeError::Internal("missing path params".to_string()))
        })
    }
}

impl OptionalFromRequest for RawPathParams {
    type Rejection = std::convert::Infallible;

    fn optional_from_request(
        request: IncomingRequest,
    ) -> ExtractFuture<'static, Option<Self>, Self::Rejection> {
        let params = request.extensions().get::<RouteParams>().cloned();
        Box::pin(async move { Ok(params.map(Self)) })
    }
}

impl OptionalFromRequestParts for RawPathParams {
    type Rejection = std::convert::Infallible;

    fn optional_from_request_parts(
        parts: &mut HttpParts,
    ) -> ExtractFuture<'_, Option<Self>, Self::Rejection> {
        let params = parts.extensions.get::<RouteParams>().cloned();
        Box::pin(async move { Ok(params.map(Self)) })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawQuery(pub Option<String>);

impl FromRequest for RawQuery {
    type Rejection = std::convert::Infallible;

    fn from_request(request: IncomingRequest) -> ExtractFuture<'static, Self, Self::Rejection> {
        let query = request.uri().query().map(ToOwned::to_owned);
        Box::pin(async move { Ok(Self(query)) })
    }
}

impl FromRequestParts for RawQuery {
    type Rejection = std::convert::Infallible;

    fn from_request_parts(parts: &mut HttpParts) -> ExtractFuture<'_, Self, Self::Rejection> {
        let query = parts.uri.query().map(ToOwned::to_owned);
        Box::pin(async move { Ok(Self(query)) })
    }
}

impl OptionalFromRequest for RawQuery {
    type Rejection = std::convert::Infallible;

    fn optional_from_request(
        request: IncomingRequest,
    ) -> ExtractFuture<'static, Option<Self>, Self::Rejection> {
        let query = request.uri().query().map(ToOwned::to_owned);
        Box::pin(async move { Ok(query.map(|query| Self(Some(query)))) })
    }
}

impl OptionalFromRequestParts for RawQuery {
    type Rejection = std::convert::Infallible;

    fn optional_from_request_parts(
        parts: &mut HttpParts,
    ) -> ExtractFuture<'_, Option<Self>, Self::Rejection> {
        let query = parts.uri.query().map(ToOwned::to_owned);
        Box::pin(async move { Ok(query.map(|query| Self(Some(query)))) })
    }
}

pub struct Query<T>(pub T);

impl<T> Deref for Query<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> DerefMut for Query<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

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

impl<T> OptionalFromRequest for Query<T>
where
    T: DeserializeOwned + Send + 'static,
{
    type Rejection = roze_error::RozeError;

    fn optional_from_request(
        request: IncomingRequest,
    ) -> ExtractFuture<'static, Option<Self>, Self::Rejection> {
        let query = request.uri().query().unwrap_or_default().to_string();
        Box::pin(async move {
            if query.is_empty() {
                return Ok(None);
            }
            let value = serde_urlencoded::from_str(&query)
                .map_err(|error| roze_error::RozeError::BadRequest(error.to_string()))?;
            Ok(Some(Self(value)))
        })
    }
}

impl<T> OptionalFromRequestParts for Query<T>
where
    T: DeserializeOwned + Send + 'static,
{
    type Rejection = roze_error::RozeError;

    fn optional_from_request_parts(
        parts: &mut HttpParts,
    ) -> ExtractFuture<'_, Option<Self>, Self::Rejection> {
        let query = parts.uri.query().unwrap_or_default().to_string();
        Box::pin(async move {
            if query.is_empty() {
                return Ok(None);
            }
            let value = serde_urlencoded::from_str(&query)
                .map_err(|error| roze_error::RozeError::BadRequest(error.to_string()))?;
            Ok(Some(Self(value)))
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawForm(pub Bytes);

impl FromRequest for RawForm {
    type Rejection = roze_error::RozeError;

    fn from_request(request: IncomingRequest) -> ExtractFuture<'static, Self, Self::Rejection> {
        Box::pin(async move {
            if request.method() == Method::GET {
                let query = request.uri().query().unwrap_or_default();
                return Ok(Self(Bytes::copy_from_slice(query.as_bytes())));
            }
            if !has_urlencoded_content_type(request.headers()) {
                return Err(roze_error::RozeError::BadRequest(
                    "expected application/x-www-form-urlencoded content type".to_string(),
                ));
            }
            let body = request
                .into_body()
                .collect()
                .await
                .map_err(|error| roze_error::RozeError::BadRequest(error.to_string()))?
                .to_bytes();
            Ok(Self(body))
        })
    }
}

fn has_urlencoded_content_type(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value.split(';').next().is_some_and(|value| {
                value
                    .trim()
                    .eq_ignore_ascii_case("application/x-www-form-urlencoded")
            })
        })
}

pub struct Form<T>(pub T);

impl<T> Deref for Form<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> DerefMut for Form<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

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

impl<T> OptionalFromRequest for Form<T>
where
    T: DeserializeOwned + Send + 'static,
{
    type Rejection = roze_error::RozeError;

    fn optional_from_request(
        request: IncomingRequest,
    ) -> ExtractFuture<'static, Option<Self>, Self::Rejection> {
        Box::pin(async move {
            let body = request
                .into_body()
                .collect()
                .await
                .map_err(|error| roze_error::RozeError::BadRequest(error.to_string()))?
                .to_bytes();
            if body.is_empty() {
                return Ok(None);
            }
            let value = serde_urlencoded::from_bytes(&body)
                .map_err(|error| roze_error::RozeError::BadRequest(error.to_string()))?;
            Ok(Some(Self(value)))
        })
    }
}

#[derive(Clone, Debug)]
pub struct Extension<T>(pub T);

impl<T> Deref for Extension<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> DerefMut for Extension<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

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

impl<T> OptionalFromRequest for Extension<T>
where
    T: Clone + Send + Sync + 'static,
{
    type Rejection = std::convert::Infallible;

    fn optional_from_request(
        request: IncomingRequest,
    ) -> ExtractFuture<'static, Option<Self>, Self::Rejection> {
        Box::pin(async move { Ok(request.extensions().get::<T>().cloned().map(Self)) })
    }
}

impl<T> OptionalFromRequestParts for Extension<T>
where
    T: Clone + Send + Sync + 'static,
{
    type Rejection = std::convert::Infallible;

    fn optional_from_request_parts(
        parts: &mut HttpParts,
    ) -> ExtractFuture<'_, Option<Self>, Self::Rejection> {
        let value = parts.extensions.get::<T>().cloned();
        Box::pin(async move { Ok(value.map(Self)) })
    }
}

#[derive(Clone, Debug)]
pub struct ConnectInfo<T>(pub T);

impl<T> Deref for ConnectInfo<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> DerefMut for ConnectInfo<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl<T> FromRequest for ConnectInfo<T>
where
    T: Clone + Send + Sync + 'static,
{
    type Rejection = roze_error::RozeError;

    fn from_request(request: IncomingRequest) -> ExtractFuture<'static, Self, Self::Rejection> {
        Box::pin(async move {
            request
                .extensions()
                .get::<Self>()
                .cloned()
                .ok_or_else(|| roze_error::RozeError::Internal("missing connect info".to_string()))
        })
    }
}

impl<T> FromRequestParts for ConnectInfo<T>
where
    T: Clone + Send + Sync + 'static,
{
    type Rejection = roze_error::RozeError;

    fn from_request_parts(parts: &mut HttpParts) -> ExtractFuture<'_, Self, Self::Rejection> {
        let value = parts.extensions.get::<Self>().cloned();
        Box::pin(async move {
            value.ok_or_else(|| roze_error::RozeError::Internal("missing connect info".to_string()))
        })
    }
}

impl<T> OptionalFromRequest for ConnectInfo<T>
where
    T: Clone + Send + Sync + 'static,
{
    type Rejection = std::convert::Infallible;

    fn optional_from_request(
        request: IncomingRequest,
    ) -> ExtractFuture<'static, Option<Self>, Self::Rejection> {
        Box::pin(async move { Ok(request.extensions().get::<Self>().cloned()) })
    }
}

impl<T> OptionalFromRequestParts for ConnectInfo<T>
where
    T: Clone + Send + Sync + 'static,
{
    type Rejection = std::convert::Infallible;

    fn optional_from_request_parts(
        parts: &mut HttpParts,
    ) -> ExtractFuture<'_, Option<Self>, Self::Rejection> {
        let value = parts.extensions.get::<Self>().cloned();
        Box::pin(async move { Ok(value) })
    }
}

pub struct State<T>(pub T);

impl<T> Deref for State<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> DerefMut for State<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

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

impl<T> OptionalFromRequest for State<T>
where
    T: Clone + Send + Sync + 'static,
{
    type Rejection = std::convert::Infallible;

    fn optional_from_request(
        request: IncomingRequest,
    ) -> ExtractFuture<'static, Option<Self>, Self::Rejection> {
        Box::pin(async move { Ok(request.extensions().get::<T>().cloned().map(Self)) })
    }
}

impl<T> OptionalFromRequestParts for State<T>
where
    T: Clone + Send + Sync + 'static,
{
    type Rejection = std::convert::Infallible;

    fn optional_from_request_parts(
        parts: &mut HttpParts,
    ) -> ExtractFuture<'_, Option<Self>, Self::Rejection> {
        let value = parts.extensions.get::<T>().cloned();
        Box::pin(async move { Ok(value.map(Self)) })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Host(pub String);

impl Deref for Host {
    type Target = String;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for Host {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl FromRequest for Host {
    type Rejection = roze_error::RozeError;

    fn from_request(request: IncomingRequest) -> ExtractFuture<'static, Self, Self::Rejection> {
        let host = extract_host(request.headers(), request.uri());
        Box::pin(async move { host })
    }
}

impl FromRequestParts for Host {
    type Rejection = roze_error::RozeError;

    fn from_request_parts(parts: &mut HttpParts) -> ExtractFuture<'_, Self, Self::Rejection> {
        let host = extract_host(&parts.headers, &parts.uri);
        Box::pin(async move { host })
    }
}

impl OptionalFromRequest for Host {
    type Rejection = roze_error::RozeError;

    fn optional_from_request(
        request: IncomingRequest,
    ) -> ExtractFuture<'static, Option<Self>, Self::Rejection> {
        let host = extract_optional_host(request.headers(), request.uri());
        Box::pin(async move { host })
    }
}

impl OptionalFromRequestParts for Host {
    type Rejection = roze_error::RozeError;

    fn optional_from_request_parts(
        parts: &mut HttpParts,
    ) -> ExtractFuture<'_, Option<Self>, Self::Rejection> {
        let host = extract_optional_host(&parts.headers, &parts.uri);
        Box::pin(async move { host })
    }
}

fn extract_host(headers: &HeaderMap, uri: &Uri) -> Result<Host, roze_error::RozeError> {
    extract_optional_host(headers, uri)?
        .ok_or_else(|| roze_error::RozeError::BadRequest("missing host".to_string()))
}

fn extract_optional_host(
    headers: &HeaderMap,
    uri: &Uri,
) -> Result<Option<Host>, roze_error::RozeError> {
    if let Some(value) = headers.get(header::HOST) {
        let host = value
            .to_str()
            .map_err(|error| roze_error::RozeError::BadRequest(error.to_string()))?;
        return Ok(Some(Host(host.to_string())));
    }
    Ok(uri.authority().map(|authority| Host(authority.to_string())))
}

#[derive(Clone, Debug)]
pub struct OriginalUri(pub Uri);

impl Deref for OriginalUri {
    type Target = Uri;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for OriginalUri {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl FromRequest for OriginalUri {
    type Rejection = std::convert::Infallible;

    fn from_request(request: IncomingRequest) -> ExtractFuture<'static, Self, Self::Rejection> {
        let uri = request
            .extensions()
            .get::<Self>()
            .cloned()
            .unwrap_or_else(|| Self(request.uri().clone()));
        Box::pin(async move { Ok(uri) })
    }
}

impl FromRequestParts for OriginalUri {
    type Rejection = std::convert::Infallible;

    fn from_request_parts(parts: &mut HttpParts) -> ExtractFuture<'_, Self, Self::Rejection> {
        let uri = parts
            .extensions
            .get::<Self>()
            .cloned()
            .unwrap_or_else(|| Self(parts.uri.clone()));
        Box::pin(async move { Ok(uri) })
    }
}

impl OptionalFromRequest for OriginalUri {
    type Rejection = std::convert::Infallible;

    fn optional_from_request(
        request: IncomingRequest,
    ) -> ExtractFuture<'static, Option<Self>, Self::Rejection> {
        Box::pin(async move { Ok(Some(Self(request.uri().clone()))) })
    }
}

impl OptionalFromRequestParts for OriginalUri {
    type Rejection = std::convert::Infallible;

    fn optional_from_request_parts(
        parts: &mut HttpParts,
    ) -> ExtractFuture<'_, Option<Self>, Self::Rejection> {
        let uri = parts.uri.clone();
        Box::pin(async move { Ok(Some(Self(uri))) })
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

impl<T> OptionalFromRequest for Json<T>
where
    T: DeserializeOwned + Send + 'static,
{
    type Rejection = roze_error::RozeError;

    fn optional_from_request(
        request: IncomingRequest,
    ) -> ExtractFuture<'static, Option<Self>, Self::Rejection> {
        Box::pin(async move {
            let body = request
                .into_body()
                .collect()
                .await
                .map_err(|error| roze_error::RozeError::BadRequest(error.to_string()))?
                .to_bytes();
            if body.is_empty() {
                return Ok(None);
            }
            let value = serde_json::from_slice(&body)
                .map_err(|error| roze_error::RozeError::BadRequest(error.to_string()))?;
            Ok(Some(Self(value)))
        })
    }
}

#[cfg(test)]
mod tests {
    use http::Request;
    use serde::Deserialize;
    use std::net::SocketAddr;

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
    async fn extracts_raw_bytes_body() {
        let request = Request::builder()
            .method("POST")
            .uri("/webhook")
            .body(rest::full_body("raw-body"))
            .unwrap();

        let body = Bytes::from_request(request).await.expect("bytes body");

        assert_eq!(&body[..], b"raw-body");
    }

    #[tokio::test]
    async fn extracts_string_body() {
        let request = Request::builder()
            .method("POST")
            .uri("/webhook")
            .body(rest::full_body("hello roze"))
            .unwrap();

        let body = String::from_request(request).await.expect("string body");

        assert_eq!(body, "hello roze");
    }

    #[tokio::test]
    async fn request_ext_extracts_body_extractor() {
        let request = Request::builder()
            .method("POST")
            .uri("/webhook")
            .body(rest::full_body("hello roze"))
            .unwrap();

        let body: String = request.extract().await.expect("string body");

        assert_eq!(body, "hello roze");
    }

    #[tokio::test]
    async fn request_ext_extracts_optional_body_extractor() {
        let request = Request::builder()
            .method("POST")
            .uri("/users")
            .body(rest::empty_body())
            .unwrap();

        let payload: Option<Json<Payload>> =
            request.extract_optional().await.expect("optional json");

        assert!(payload.is_none());
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
    async fn extracts_basic_http_parts() {
        let request = Request::builder()
            .method("PATCH")
            .uri("/users/42?name=roze")
            .version(Version::HTTP_2)
            .body(rest::empty_body())
            .unwrap();
        let (mut parts, _body) = request.into_parts();

        let method = Method::from_request_parts(&mut parts)
            .await
            .expect("method");
        let uri = Uri::from_request_parts(&mut parts).await.expect("uri");
        let version = Version::from_request_parts(&mut parts)
            .await
            .expect("version");

        assert_eq!(method, Method::PATCH);
        assert_eq!(uri.path(), "/users/42");
        assert_eq!(uri.query(), Some("name=roze"));
        assert_eq!(version, Version::HTTP_2);
    }

    #[tokio::test]
    async fn request_parts_ext_extracts_parts_extractor() {
        let request = Request::builder()
            .method("GET")
            .uri("/users?name=roze")
            .body(rest::empty_body())
            .unwrap();
        let (mut parts, _body) = request.into_parts();

        let method: Method = parts.extract().await.expect("method");
        let RawQuery(query): RawQuery = parts.extract().await.expect("raw query");

        assert_eq!(method, Method::GET);
        assert_eq!(query.as_deref(), Some("name=roze"));
    }

    #[tokio::test]
    async fn request_parts_ext_extracts_optional_parts_extractor() {
        let request = Request::builder()
            .method("GET")
            .uri("/users")
            .body(rest::empty_body())
            .unwrap();
        let (mut parts, _body) = request.into_parts();

        let query: Option<RawQuery> = parts.extract_optional().await.expect("optional query");

        assert!(query.is_none());
    }

    #[tokio::test]
    async fn request_ext_extracts_parts_without_consuming_body() {
        let mut request = Request::builder()
            .method("POST")
            .uri("/webhook?name=roze")
            .header("x-roze-test", "yes")
            .body(rest::full_body("payload"))
            .unwrap();

        let method: Method = request.extract_parts().await.expect("method");
        let RawQuery(query): RawQuery = request.extract_parts().await.expect("raw query");

        assert_eq!(method, Method::POST);
        assert_eq!(query.as_deref(), Some("name=roze"));
        assert_eq!(
            request
                .headers()
                .get("x-roze-test")
                .and_then(|value| value.to_str().ok()),
            Some("yes")
        );

        let body: String = request.extract().await.expect("body after parts");
        assert_eq!(body, "payload");
    }

    #[tokio::test]
    async fn request_ext_extracts_optional_parts_without_consuming_body() {
        let mut request = Request::builder()
            .method("POST")
            .uri("/webhook")
            .body(rest::full_body("payload"))
            .unwrap();

        let query: Option<RawQuery> = request
            .extract_optional_parts()
            .await
            .expect("optional raw query");

        assert!(query.is_none());

        let body: String = request.extract().await.expect("body after optional parts");
        assert_eq!(body, "payload");
    }

    #[tokio::test]
    async fn extracts_host_from_header() {
        let request = Request::builder()
            .method("GET")
            .uri("/users")
            .header(header::HOST, "tenant.example.com:8443")
            .body(rest::empty_body())
            .unwrap();
        let (mut parts, _body) = request.into_parts();

        let Host(host) = Host::from_request_parts(&mut parts).await.expect("host");

        assert_eq!(host, "tenant.example.com:8443");
    }

    #[tokio::test]
    async fn extracts_host_from_uri_authority() {
        let request = Request::builder()
            .method("GET")
            .uri("https://tenant.example.com/users")
            .body(rest::empty_body())
            .unwrap();

        let Host(host) = Host::from_request(request).await.expect("host");

        assert_eq!(host, "tenant.example.com");
    }

    #[tokio::test]
    async fn optional_host_returns_none_when_absent() {
        let request = Request::builder()
            .method("GET")
            .uri("/users")
            .body(rest::empty_body())
            .unwrap();

        let host = Option::<Host>::from_request(request)
            .await
            .expect("optional host");

        assert!(host.is_none());
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

    #[tokio::test]
    async fn extracts_raw_path_params_without_deserializing() {
        let mut request = Request::builder()
            .method("GET")
            .uri("/users/42/team/core")
            .body(rest::empty_body())
            .unwrap();
        request.extensions_mut().insert(RouteParams::from_pairs([
            ("user_id".to_string(), "42".to_string()),
            ("team".to_string(), "core".to_string()),
        ]));

        let params = RawPathParams::from_request(request)
            .await
            .expect("raw path params");

        assert_eq!(params.get("user_id"), Some("42"));
        assert_eq!(
            params.iter().collect::<Vec<_>>(),
            vec![("user_id", "42"), ("team", "core")]
        );
        let mut seen = Vec::new();
        for (key, value) in &params {
            seen.push((key, value));
        }
        assert_eq!(seen, vec![("user_id", "42"), ("team", "core")]);
    }

    #[tokio::test]
    async fn optional_raw_path_params_returns_none_when_absent() {
        let request = Request::builder()
            .method("GET")
            .uri("/users")
            .body(rest::empty_body())
            .unwrap();

        let params = Option::<RawPathParams>::from_request(request)
            .await
            .expect("optional raw path params");
        assert!(params.is_none());
    }

    #[tokio::test]
    async fn extracts_connect_info() {
        let peer_addr = SocketAddr::from(([127, 0, 0, 1], 8080));
        let mut request = Request::builder()
            .method("GET")
            .uri("/peer")
            .body(rest::empty_body())
            .unwrap();
        request.extensions_mut().insert(ConnectInfo(peer_addr));

        let ConnectInfo(extracted) = ConnectInfo::<SocketAddr>::from_request(request)
            .await
            .expect("connect info");
        assert_eq!(extracted, peer_addr);
    }

    #[tokio::test]
    async fn extracts_original_uri_from_request_parts() {
        let request = Request::builder()
            .method("GET")
            .uri("/api/users?name=roze")
            .body(rest::empty_body())
            .unwrap();
        let (mut parts, _body) = request.into_parts();

        let original = OriginalUri::from_request_parts(&mut parts)
            .await
            .expect("original uri");

        assert_eq!(original.path(), "/api/users");
        assert_eq!(original.query(), Some("name=roze"));
    }

    #[tokio::test]
    async fn state_derefs_to_inner_value() {
        let state = State(String::from("roze"));

        assert_eq!(state.len(), 4);
        assert_eq!(state.as_str(), "roze");
    }

    #[tokio::test]
    async fn optional_query_returns_none_when_absent() {
        let request = Request::builder()
            .method("GET")
            .uri("/users")
            .body(rest::empty_body())
            .unwrap();

        let query = Option::<Query<Payload>>::from_request(request)
            .await
            .expect("optional query");
        assert!(query.is_none());
    }

    #[tokio::test]
    async fn extracts_raw_query_without_parsing() {
        let request = Request::builder()
            .method("GET")
            .uri("/users?name=roze%20team&tag=rust")
            .body(rest::empty_body())
            .unwrap();
        let (mut parts, _body) = request.into_parts();

        let RawQuery(query) = RawQuery::from_request_parts(&mut parts)
            .await
            .expect("raw query");

        assert_eq!(query.as_deref(), Some("name=roze%20team&tag=rust"));
    }

    #[tokio::test]
    async fn optional_raw_query_returns_none_when_absent() {
        let request = Request::builder()
            .method("GET")
            .uri("/users")
            .body(rest::empty_body())
            .unwrap();

        let query = Option::<RawQuery>::from_request(request)
            .await
            .expect("optional raw query");
        assert!(query.is_none());
    }

    #[tokio::test]
    async fn raw_form_extracts_query_for_get_requests() {
        let request = Request::builder()
            .method("GET")
            .uri("/users?page=0&size=10")
            .body(rest::empty_body())
            .unwrap();

        let RawForm(form) = RawForm::from_request(request).await.expect("raw form");
        assert_eq!(&form[..], b"page=0&size=10");
    }

    #[tokio::test]
    async fn raw_form_extracts_urlencoded_body() {
        let request = Request::builder()
            .method("POST")
            .uri("/login")
            .header(
                header::CONTENT_TYPE,
                "application/x-www-form-urlencoded; charset=utf-8",
            )
            .body(rest::full_body("username=user&password=secure%20password"))
            .unwrap();

        let RawForm(form) = RawForm::from_request(request).await.expect("raw form");
        assert_eq!(&form[..], b"username=user&password=secure%20password");
    }

    #[tokio::test]
    async fn raw_form_rejects_missing_urlencoded_content_type() {
        let request = Request::builder()
            .method("POST")
            .uri("/login")
            .body(rest::full_body("username=user"))
            .unwrap();

        let error = RawForm::from_request(request).await.unwrap_err();
        assert_eq!(error.code(), 400);
    }

    #[tokio::test]
    async fn result_parts_extractor_captures_rejection() {
        let request = Request::builder()
            .method("GET")
            .uri("/users")
            .body(rest::empty_body())
            .unwrap();
        let (mut parts, _body) = request.into_parts();

        let query = Result::<Query<Payload>, roze_error::RozeError>::from_request_parts(&mut parts)
            .await
            .expect("result extractor");

        assert!(query.is_err());
    }

    #[tokio::test]
    async fn optional_extension_returns_none_when_missing() {
        let request = Request::builder()
            .method("GET")
            .uri("/users")
            .body(rest::empty_body())
            .unwrap();

        let extension = Option::<Extension<String>>::from_request(request)
            .await
            .expect("optional extension");
        assert!(extension.is_none());
    }

    #[tokio::test]
    async fn optional_json_returns_none_for_empty_body() {
        let request = Request::builder()
            .method("POST")
            .uri("/users")
            .body(rest::empty_body())
            .unwrap();

        let payload = Option::<Json<Payload>>::from_request(request)
            .await
            .expect("optional json");
        assert!(payload.is_none());
    }
}
