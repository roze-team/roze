use std::{
    fmt,
    future::Future,
    ops::{Deref, DerefMut},
    pin::Pin,
    sync::Arc,
};

use bytes::Bytes;
use http::{header, request::Parts as HttpParts, Extensions, HeaderMap, Method, Uri, Version};
use serde::de::{
    self,
    value::{Error as ValueError, MapDeserializer, SeqDeserializer, StrDeserializer},
    DeserializeOwned, Deserializer, IntoDeserializer, Visitor,
};
use tower::{util::BoxCloneSyncService, Layer, Service};

use crate::{
    body::{self, BodyError},
    response::{IntoResponse, Json},
    rest::{HttpResponse, IncomingRequest},
    route_params::RouteParams,
};

pub type Request = IncomingRequest;
pub type ExtractFuture<'a, T, E> = Pin<Box<dyn Future<Output = Result<T, E>> + Send + 'a>>;
pub const DEFAULT_BODY_LIMIT: usize = 2 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DefaultBodyLimit {
    kind: DefaultBodyLimitKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DefaultBodyLimitKind {
    Disable,
    Limit(usize),
}

impl DefaultBodyLimit {
    pub const fn disable() -> Self {
        Self {
            kind: DefaultBodyLimitKind::Disable,
        }
    }

    pub const fn max(limit: usize) -> Self {
        Self {
            kind: DefaultBodyLimitKind::Limit(limit),
        }
    }

    pub fn apply(self, request: &mut IncomingRequest) {
        request.extensions_mut().insert(self.kind);
    }
}

impl Layer<BoxCloneSyncService<IncomingRequest, HttpResponse, std::convert::Infallible>>
    for DefaultBodyLimit
{
    type Service = DefaultBodyLimitService;

    fn layer(
        &self,
        inner: BoxCloneSyncService<IncomingRequest, HttpResponse, std::convert::Infallible>,
    ) -> Self::Service {
        DefaultBodyLimitService {
            kind: self.kind,
            inner,
        }
    }
}

#[derive(Clone)]
pub struct DefaultBodyLimitService {
    kind: DefaultBodyLimitKind,
    inner: BoxCloneSyncService<IncomingRequest, HttpResponse, std::convert::Infallible>,
}

impl std::fmt::Debug for DefaultBodyLimitService {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DefaultBodyLimitService")
            .field("kind", &self.kind)
            .finish_non_exhaustive()
    }
}

impl Service<IncomingRequest> for DefaultBodyLimitService {
    type Response = HttpResponse;
    type Error = std::convert::Infallible;
    type Future = Pin<
        Box<dyn Future<Output = Result<HttpResponse, std::convert::Infallible>> + Send + 'static>,
    >;

    fn poll_ready(
        &mut self,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut request: IncomingRequest) -> Self::Future {
        request.extensions_mut().insert(self.kind);
        let mut inner = self.inner.clone();
        Box::pin(async move { inner.call(request).await })
    }
}

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
        Box::pin(async move { collect_limited_body(request).await })
    }
}

async fn collect_limited_body(request: IncomingRequest) -> Result<Bytes, roze_error::RozeError> {
    let kind = request
        .extensions()
        .get::<DefaultBodyLimitKind>()
        .copied()
        .unwrap_or(DefaultBodyLimitKind::Limit(DEFAULT_BODY_LIMIT));
    let limit = match kind {
        DefaultBodyLimitKind::Disable => usize::MAX,
        DefaultBodyLimitKind::Limit(limit) => limit,
    };
    body::to_bytes(request.into_body(), limit)
        .await
        .map_err(body_error_to_bad_request)
}

fn body_error_to_bad_request(error: BodyError) -> roze_error::RozeError {
    roze_error::RozeError::BadRequest(error.to_string())
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MatchedPath(Arc<str>);

impl MatchedPath {
    pub(crate) fn new(path: impl Into<Arc<str>>) -> Self {
        Self(path.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.as_ref()
    }

    pub fn from_request_extensions(extensions: &Extensions) -> Result<Self, roze_error::RozeError> {
        extensions
            .get::<Self>()
            .cloned()
            .ok_or_else(|| roze_error::RozeError::Internal("missing matched path".to_string()))
    }

    #[must_use]
    pub fn optional_from_request_extensions(extensions: &Extensions) -> Option<Self> {
        extensions.get::<Self>().cloned()
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

impl FromRequestParts for MatchedPath {
    type Rejection = roze_error::RozeError;

    fn from_request_parts(parts: &mut HttpParts) -> ExtractFuture<'_, Self, Self::Rejection> {
        let matched_path = Self::from_request_extensions(&parts.extensions);
        Box::pin(async move { matched_path })
    }
}

impl FromRequest for MatchedPath {
    type Rejection = roze_error::RozeError;

    fn from_request(request: IncomingRequest) -> ExtractFuture<'static, Self, Self::Rejection> {
        let matched_path = Self::from_request_extensions(request.extensions());
        Box::pin(async move { matched_path })
    }
}

impl OptionalFromRequestParts for MatchedPath {
    type Rejection = std::convert::Infallible;

    fn optional_from_request_parts(
        parts: &mut HttpParts,
    ) -> ExtractFuture<'_, Option<Self>, Self::Rejection> {
        let matched_path = Self::optional_from_request_extensions(&parts.extensions);
        Box::pin(async move { Ok(matched_path) })
    }
}

impl OptionalFromRequest for MatchedPath {
    type Rejection = std::convert::Infallible;

    fn optional_from_request(
        request: IncomingRequest,
    ) -> ExtractFuture<'static, Option<Self>, Self::Rejection> {
        let matched_path = Self::optional_from_request_extensions(request.extensions());
        Box::pin(async move { Ok(matched_path) })
    }
}

#[derive(Clone, Debug)]
pub struct Path<T>(pub T);

struct PathDeserializer<'de> {
    params: &'de RouteParams,
}

impl<'de> PathDeserializer<'de> {
    fn new(params: &'de RouteParams) -> Self {
        Self { params }
    }

    fn single(self) -> Result<&'de str, ValueError> {
        let mut params = self.params.iter();
        let Some((_, value)) = params.next() else {
            return Err(de::Error::custom("expected one path parameter, got 0"));
        };
        if params.next().is_some() {
            return Err(de::Error::custom(format!(
                "expected one path parameter, got {}",
                self.params.iter().count()
            )));
        }
        Ok(value)
    }
}

#[derive(Clone, Copy)]
struct PathValueDeserializer<'de>(&'de str);

impl<'de> IntoDeserializer<'de, ValueError> for PathValueDeserializer<'de> {
    type Deserializer = Self;

    fn into_deserializer(self) -> Self::Deserializer {
        self
    }
}

macro_rules! deserialize_parsed_path_value {
    ($method:ident, $ty:ty, $visit:ident) => {
        fn $method<V>(self, visitor: V) -> Result<V::Value, Self::Error>
        where
            V: Visitor<'de>,
        {
            let value = self
                .0
                .parse::<$ty>()
                .map_err(|error| de::Error::custom(error.to_string()))?;
            visitor.$visit(value)
        }
    };
}

impl<'de> Deserializer<'de> for PathValueDeserializer<'de> {
    type Error = ValueError;

    deserialize_parsed_path_value!(deserialize_bool, bool, visit_bool);
    deserialize_parsed_path_value!(deserialize_i8, i8, visit_i8);
    deserialize_parsed_path_value!(deserialize_i16, i16, visit_i16);
    deserialize_parsed_path_value!(deserialize_i32, i32, visit_i32);
    deserialize_parsed_path_value!(deserialize_i64, i64, visit_i64);
    deserialize_parsed_path_value!(deserialize_i128, i128, visit_i128);
    deserialize_parsed_path_value!(deserialize_u8, u8, visit_u8);
    deserialize_parsed_path_value!(deserialize_u16, u16, visit_u16);
    deserialize_parsed_path_value!(deserialize_u32, u32, visit_u32);
    deserialize_parsed_path_value!(deserialize_u64, u64, visit_u64);
    deserialize_parsed_path_value!(deserialize_u128, u128, visit_u128);
    deserialize_parsed_path_value!(deserialize_f32, f32, visit_f32);
    deserialize_parsed_path_value!(deserialize_f64, f64, visit_f64);
    deserialize_parsed_path_value!(deserialize_char, char, visit_char);

    fn deserialize_any<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_str(visitor)
    }

    fn deserialize_str<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        visitor.visit_borrowed_str(self.0)
    }

    fn deserialize_string<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        visitor.visit_string(self.0.to_owned())
    }

    fn deserialize_bytes<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        visitor.visit_borrowed_bytes(self.0.as_bytes())
    }

    fn deserialize_byte_buf<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        visitor.visit_byte_buf(self.0.as_bytes().to_vec())
    }

    fn deserialize_option<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        visitor.visit_some(self)
    }

    fn deserialize_unit<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        visitor.visit_unit()
    }

    fn deserialize_unit_struct<V>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        visitor.visit_unit()
    }

    fn deserialize_newtype_struct<V>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        visitor.visit_newtype_struct(self)
    }

    fn deserialize_enum<V>(
        self,
        name: &'static str,
        variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        StrDeserializer::<ValueError>::new(self.0).deserialize_enum(name, variants, visitor)
    }

    fn deserialize_identifier<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_str(visitor)
    }

    fn deserialize_ignored_any<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        visitor.visit_unit()
    }

    fn deserialize_seq<V>(self, _visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        Err(de::Error::custom("nested path sequences are not supported"))
    }

    fn deserialize_tuple<V>(self, _len: usize, _visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        Err(de::Error::custom("nested path tuples are not supported"))
    }

    fn deserialize_tuple_struct<V>(
        self,
        _name: &'static str,
        _len: usize,
        _visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        Err(de::Error::custom(
            "nested path tuple structs are not supported",
        ))
    }

    fn deserialize_map<V>(self, _visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        Err(de::Error::custom("nested path maps are not supported"))
    }

    fn deserialize_struct<V>(
        self,
        _name: &'static str,
        _fields: &'static [&'static str],
        _visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        Err(de::Error::custom("nested path structs are not supported"))
    }
}

macro_rules! deserialize_single_path_value {
    ($method:ident) => {
        fn $method<V>(self, visitor: V) -> Result<V::Value, Self::Error>
        where
            V: Visitor<'de>,
        {
            PathValueDeserializer(self.single()?).$method(visitor)
        }
    };
}

impl<'de> Deserializer<'de> for PathDeserializer<'de> {
    type Error = ValueError;

    deserialize_single_path_value!(deserialize_bool);
    deserialize_single_path_value!(deserialize_i8);
    deserialize_single_path_value!(deserialize_i16);
    deserialize_single_path_value!(deserialize_i32);
    deserialize_single_path_value!(deserialize_i64);
    deserialize_single_path_value!(deserialize_i128);
    deserialize_single_path_value!(deserialize_u8);
    deserialize_single_path_value!(deserialize_u16);
    deserialize_single_path_value!(deserialize_u32);
    deserialize_single_path_value!(deserialize_u64);
    deserialize_single_path_value!(deserialize_u128);
    deserialize_single_path_value!(deserialize_f32);
    deserialize_single_path_value!(deserialize_f64);
    deserialize_single_path_value!(deserialize_char);
    deserialize_single_path_value!(deserialize_str);
    deserialize_single_path_value!(deserialize_string);
    deserialize_single_path_value!(deserialize_bytes);
    deserialize_single_path_value!(deserialize_byte_buf);
    deserialize_single_path_value!(deserialize_identifier);

    fn deserialize_any<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_str(visitor)
    }

    fn deserialize_option<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        visitor.visit_some(self)
    }

    fn deserialize_unit<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        visitor.visit_unit()
    }

    fn deserialize_unit_struct<V>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        visitor.visit_unit()
    }

    fn deserialize_newtype_struct<V>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        visitor.visit_newtype_struct(self)
    }

    fn deserialize_seq<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        SeqDeserializer::new(
            self.params
                .iter()
                .map(|(_, value)| PathValueDeserializer(value)),
        )
        .deserialize_seq(visitor)
    }

    fn deserialize_tuple<V>(self, len: usize, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let actual = self.params.iter().count();
        if actual != len {
            return Err(de::Error::custom(format!(
                "expected {len} path parameters, got {actual}"
            )));
        }
        self.deserialize_seq(visitor)
    }

    fn deserialize_tuple_struct<V>(
        self,
        _name: &'static str,
        len: usize,
        visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_tuple(len, visitor)
    }

    fn deserialize_map<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        MapDeserializer::new(
            self.params
                .iter()
                .map(|(key, value)| (key, PathValueDeserializer(value))),
        )
        .deserialize_map(visitor)
    }

    fn deserialize_struct<V>(
        self,
        _name: &'static str,
        _fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_map(visitor)
    }

    fn deserialize_enum<V>(
        self,
        name: &'static str,
        variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        StrDeserializer::<ValueError>::new(self.single()?).deserialize_enum(name, variants, visitor)
    }

    fn deserialize_ignored_any<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        visitor.visit_unit()
    }
}

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

impl<T> Path<T>
where
    T: DeserializeOwned,
{
    fn from_route_params(params: &RouteParams) -> Result<Self, roze_error::RozeError> {
        params.validate()?;
        let value = T::deserialize(PathDeserializer::new(params))
            .map_err(|error| roze_error::RozeError::BadRequest(error.to_string()))?;
        Ok(Self(value))
    }

    fn optional_from_route_params(
        params: Option<&RouteParams>,
    ) -> Result<Option<Self>, roze_error::RozeError> {
        let Some(params) = params else {
            return Ok(None);
        };
        if params.is_empty() {
            return Ok(None);
        }
        Self::from_route_params(params).map(Some)
    }

    pub fn from_request_extensions(extensions: &Extensions) -> Result<Self, roze_error::RozeError> {
        match extensions.get::<RouteParams>() {
            Some(params) => Self::from_route_params(params),
            None => Self::from_route_params(&RouteParams::default()),
        }
    }

    pub fn optional_from_request_extensions(
        extensions: &Extensions,
    ) -> Result<Option<Self>, roze_error::RozeError> {
        Self::optional_from_route_params(extensions.get::<RouteParams>())
    }
}

impl<T> FromRequest for Path<T>
where
    T: DeserializeOwned + Send + 'static,
{
    type Rejection = roze_error::RozeError;

    fn from_request(request: IncomingRequest) -> ExtractFuture<'static, Self, Self::Rejection> {
        let path = Self::from_request_extensions(request.extensions());
        Box::pin(async move { path })
    }
}

impl<T> FromRequestParts for Path<T>
where
    T: DeserializeOwned + Send + 'static,
{
    type Rejection = roze_error::RozeError;

    fn from_request_parts(parts: &mut HttpParts) -> ExtractFuture<'_, Self, Self::Rejection> {
        let path = Self::from_request_extensions(&parts.extensions);
        Box::pin(async move { path })
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
        let path = Self::optional_from_request_extensions(&parts.extensions);
        Box::pin(async move { path })
    }
}

impl<T> OptionalFromRequest for Path<T>
where
    T: DeserializeOwned + Send + 'static,
{
    type Rejection = roze_error::RozeError;

    fn optional_from_request(
        request: IncomingRequest,
    ) -> ExtractFuture<'static, Option<Self>, Self::Rejection> {
        let path = Self::optional_from_request_extensions(request.extensions());
        Box::pin(async move { path })
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RawPathParams(RouteParams);

impl RawPathParams {
    pub fn from_request_extensions(extensions: &Extensions) -> Result<Self, roze_error::RozeError> {
        let params = extensions
            .get::<RouteParams>()
            .ok_or_else(|| roze_error::RozeError::Internal("missing path params".to_string()))?;
        params.validate()?;
        Ok(Self(params.clone()))
    }

    pub fn optional_from_request_extensions(
        extensions: &Extensions,
    ) -> Result<Option<Self>, roze_error::RozeError> {
        let Some(params) = extensions.get::<RouteParams>() else {
            return Ok(None);
        };
        params.validate()?;
        Ok(Some(Self(params.clone())))
    }

    #[must_use]
    pub fn get(&self, name: &str) -> Option<&str> {
        self.0.get(name)
    }

    #[must_use]
    pub fn iter(&self) -> RawPathParamsIter<'_> {
        self.into_iter()
    }

    #[must_use]
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
        let params = Self::from_request_extensions(request.extensions());
        Box::pin(async move { params })
    }
}

impl FromRequestParts for RawPathParams {
    type Rejection = roze_error::RozeError;

    fn from_request_parts(parts: &mut HttpParts) -> ExtractFuture<'_, Self, Self::Rejection> {
        let params = Self::from_request_extensions(&parts.extensions);
        Box::pin(async move { params })
    }
}

impl OptionalFromRequest for RawPathParams {
    type Rejection = roze_error::RozeError;

    fn optional_from_request(
        request: IncomingRequest,
    ) -> ExtractFuture<'static, Option<Self>, Self::Rejection> {
        let params = Self::optional_from_request_extensions(request.extensions());
        Box::pin(async move { params })
    }
}

impl OptionalFromRequestParts for RawPathParams {
    type Rejection = roze_error::RozeError;

    fn optional_from_request_parts(
        parts: &mut HttpParts,
    ) -> ExtractFuture<'_, Option<Self>, Self::Rejection> {
        let params = Self::optional_from_request_extensions(&parts.extensions);
        Box::pin(async move { params })
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

#[derive(Clone, Debug)]
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

impl<T> Query<T>
where
    T: DeserializeOwned,
{
    pub fn try_from_uri(uri: &Uri) -> Result<Self, roze_error::RozeError> {
        let query = uri.query().unwrap_or_default();
        let value = serde_urlencoded::from_str(query)
            .map_err(|error| roze_error::RozeError::BadRequest(error.to_string()))?;
        Ok(Self(value))
    }

    pub fn optional_from_uri(uri: &Uri) -> Result<Option<Self>, roze_error::RozeError> {
        let Some(query) = uri.query() else {
            return Ok(None);
        };
        if query.is_empty() {
            return Ok(None);
        }
        let value = serde_urlencoded::from_str(query)
            .map_err(|error| roze_error::RozeError::BadRequest(error.to_string()))?;
        Ok(Some(Self(value)))
    }
}

impl<T> FromRequest for Query<T>
where
    T: DeserializeOwned + Send + 'static,
{
    type Rejection = roze_error::RozeError;

    fn from_request(request: IncomingRequest) -> ExtractFuture<'static, Self, Self::Rejection> {
        Box::pin(async move { Self::try_from_uri(request.uri()) })
    }
}

impl<T> FromRequestParts for Query<T>
where
    T: DeserializeOwned + Send + 'static,
{
    type Rejection = roze_error::RozeError;

    fn from_request_parts(parts: &mut HttpParts) -> ExtractFuture<'_, Self, Self::Rejection> {
        let result = Self::try_from_uri(&parts.uri);
        Box::pin(async move { result })
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
        Box::pin(async move { Self::optional_from_uri(request.uri()) })
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
        let result = Self::optional_from_uri(&parts.uri);
        Box::pin(async move { result })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawForm(pub Bytes);

impl FromRequest for RawForm {
    type Rejection = roze_error::RozeError;

    fn from_request(request: IncomingRequest) -> ExtractFuture<'static, Self, Self::Rejection> {
        Box::pin(async move {
            if request.method() == Method::GET || request.method() == Method::HEAD {
                let query = request.uri().query().unwrap_or_default();
                return Ok(Self(Bytes::copy_from_slice(query.as_bytes())));
            }
            if !has_urlencoded_content_type(request.headers()) {
                return Err(roze_error::RozeError::BadRequest(
                    "expected application/x-www-form-urlencoded content type".to_string(),
                ));
            }
            let body = Bytes::from_request(request).await?;
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

#[derive(Clone, Debug)]
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

impl<T> Form<T>
where
    T: DeserializeOwned,
{
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, roze_error::RozeError> {
        let value = serde_urlencoded::from_bytes(bytes)
            .map_err(|error| roze_error::RozeError::BadRequest(error.to_string()))?;
        Ok(Self(value))
    }
}

impl<T> FromRequest for Form<T>
where
    T: DeserializeOwned + Send + 'static,
{
    type Rejection = roze_error::RozeError;

    fn from_request(request: IncomingRequest) -> ExtractFuture<'static, Self, Self::Rejection> {
        Box::pin(async move {
            let body = Bytes::from_request(request).await?;
            Self::from_bytes(&body)
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
            let body = Bytes::from_request(request).await?;
            if body.is_empty() {
                return Ok(None);
            }
            Self::from_bytes(&body).map(Some)
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

#[derive(Clone)]
#[must_use = "services do nothing unless called"]
pub struct ConnectInfoService<S, T> {
    pub(crate) inner: S,
    connect_info: ConnectInfo<T>,
}

impl<S, T> ConnectInfoService<S, T> {
    pub(crate) fn new(inner: S, connect_info: T) -> Self {
        Self {
            inner,
            connect_info: ConnectInfo(connect_info),
        }
    }
}

impl<S, T> fmt::Debug for ConnectInfoService<S, T>
where
    S: fmt::Debug,
    T: fmt::Debug,
{
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ConnectInfoService")
            .field("inner", &self.inner)
            .field("connect_info", &self.connect_info)
            .finish()
    }
}

impl<S, T> Service<IncomingRequest> for ConnectInfoService<S, T>
where
    S: Service<IncomingRequest>,
    T: Clone + Send + Sync + 'static,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = S::Future;

    fn poll_ready(
        &mut self,
        context: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(context)
    }

    fn call(&mut self, mut request: IncomingRequest) -> Self::Future {
        request.extensions_mut().insert(self.connect_info.clone());
        self.inner.call(request)
    }
}

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

fn resolve_original_uri(original_uri: Option<&OriginalUri>, current_uri: &Uri) -> OriginalUri {
    original_uri
        .cloned()
        .unwrap_or_else(|| OriginalUri(current_uri.clone()))
}

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
        let uri = resolve_original_uri(request.extensions().get::<Self>(), request.uri());
        Box::pin(async move { Ok(uri) })
    }
}

impl FromRequestParts for OriginalUri {
    type Rejection = std::convert::Infallible;

    fn from_request_parts(parts: &mut HttpParts) -> ExtractFuture<'_, Self, Self::Rejection> {
        let uri = resolve_original_uri(parts.extensions.get::<Self>(), &parts.uri);
        Box::pin(async move { Ok(uri) })
    }
}

impl OptionalFromRequest for OriginalUri {
    type Rejection = std::convert::Infallible;

    fn optional_from_request(
        request: IncomingRequest,
    ) -> ExtractFuture<'static, Option<Self>, Self::Rejection> {
        let uri = resolve_original_uri(request.extensions().get::<Self>(), request.uri());
        Box::pin(async move { Ok(Some(uri)) })
    }
}

impl OptionalFromRequestParts for OriginalUri {
    type Rejection = std::convert::Infallible;

    fn optional_from_request_parts(
        parts: &mut HttpParts,
    ) -> ExtractFuture<'_, Option<Self>, Self::Rejection> {
        let uri = resolve_original_uri(parts.extensions.get::<Self>(), &parts.uri);
        Box::pin(async move { Ok(Some(uri)) })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NestedPath(String);

impl NestedPath {
    pub(crate) fn new(path: impl Into<String>) -> Self {
        Self(path.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Deref for NestedPath {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        self.as_str()
    }
}

impl FromRequest for NestedPath {
    type Rejection = roze_error::RozeError;

    fn from_request(request: IncomingRequest) -> ExtractFuture<'static, Self, Self::Rejection> {
        Box::pin(async move {
            request
                .extensions()
                .get::<Self>()
                .cloned()
                .ok_or_else(|| roze_error::RozeError::Internal("missing nested path".to_string()))
        })
    }
}

impl FromRequestParts for NestedPath {
    type Rejection = roze_error::RozeError;

    fn from_request_parts(parts: &mut HttpParts) -> ExtractFuture<'_, Self, Self::Rejection> {
        let nested_path = parts.extensions.get::<Self>().cloned();
        Box::pin(async move {
            nested_path
                .ok_or_else(|| roze_error::RozeError::Internal("missing nested path".to_string()))
        })
    }
}

impl OptionalFromRequest for NestedPath {
    type Rejection = std::convert::Infallible;

    fn optional_from_request(
        request: IncomingRequest,
    ) -> ExtractFuture<'static, Option<Self>, Self::Rejection> {
        Box::pin(async move { Ok(request.extensions().get::<Self>().cloned()) })
    }
}

impl OptionalFromRequestParts for NestedPath {
    type Rejection = std::convert::Infallible;

    fn optional_from_request_parts(
        parts: &mut HttpParts,
    ) -> ExtractFuture<'_, Option<Self>, Self::Rejection> {
        let nested_path = parts.extensions.get::<Self>().cloned();
        Box::pin(async move { Ok(nested_path) })
    }
}

impl<T> Json<T>
where
    T: DeserializeOwned,
{
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, roze_error::RozeError> {
        let value = serde_json::from_slice(bytes)
            .map_err(|error| roze_error::RozeError::BadRequest(error.to_string()))?;
        Ok(Self(value))
    }
}

impl<T> FromRequest for Json<T>
where
    T: DeserializeOwned + Send + 'static,
{
    type Rejection = roze_error::RozeError;

    fn from_request(request: IncomingRequest) -> ExtractFuture<'static, Self, Self::Rejection> {
        Box::pin(async move {
            let body = Bytes::from_request(request).await?;
            Self::from_bytes(&body)
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
            let body = Bytes::from_request(request).await?;
            if body.is_empty() {
                return Ok(None);
            }
            Self::from_bytes(&body).map(Some)
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

    #[test]
    fn json_from_bytes_deserializes_payload() {
        let Json(payload) =
            Json::<Payload>::from_bytes(br#"{"name":"roze"}"#).expect("json from bytes");

        assert_eq!(
            payload,
            Payload {
                name: "roze".to_string()
            }
        );
    }

    #[test]
    fn json_from_bytes_preserves_parse_errors() {
        let error = Json::<Payload>::from_bytes(br#"{"other":"roze"}"#).unwrap_err();

        assert_eq!(error.code(), 400);
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
    async fn default_body_limit_rejects_large_bytes_body() {
        let body = vec![b'a'; DEFAULT_BODY_LIMIT + 1];
        let request = Request::builder()
            .method("POST")
            .uri("/upload")
            .body(rest::full_body(body))
            .unwrap();

        let error = Bytes::from_request(request).await.unwrap_err();

        assert_eq!(error.code(), 400);
    }

    #[tokio::test]
    async fn default_body_limit_can_be_overridden_per_request() {
        let mut request = Request::builder()
            .method("POST")
            .uri("/upload")
            .body(rest::full_body("roze"))
            .unwrap();
        DefaultBodyLimit::max(3).apply(&mut request);

        let error = Bytes::from_request(request).await.unwrap_err();

        assert_eq!(error.code(), 400);
    }

    #[tokio::test]
    async fn default_body_limit_can_be_disabled_per_request() {
        let mut request = Request::builder()
            .method("POST")
            .uri("/upload")
            .body(rest::full_body("roze"))
            .unwrap();
        DefaultBodyLimit::disable().apply(&mut request);

        let body = Bytes::from_request(request).await.expect("bytes body");

        assert_eq!(&body[..], b"roze");
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

    #[test]
    fn query_try_from_uri_deserializes_query_string() {
        let uri: Uri = "/users?name=roze".parse().unwrap();

        let Query(payload) = Query::<Payload>::try_from_uri(&uri).expect("query from uri");

        assert_eq!(
            payload,
            Payload {
                name: "roze".to_string()
            }
        );
    }

    #[test]
    fn query_optional_from_uri_returns_none_without_query() {
        let uri: Uri = "/users".parse().unwrap();

        let query = Query::<Payload>::optional_from_uri(&uri).expect("optional query");

        assert!(query.is_none());
    }

    #[test]
    fn query_try_from_uri_preserves_parse_errors() {
        let uri: Uri = "/users?other=roze".parse().unwrap();

        let error = Query::<Payload>::try_from_uri(&uri).unwrap_err();

        assert_eq!(error.code(), 400);
    }

    #[test]
    fn matched_path_can_be_read_synchronously_from_extensions() {
        let mut extensions = Extensions::new();
        extensions.insert(MatchedPath::new("/users/{id}"));

        let matched_path = MatchedPath::from_request_extensions(&extensions)
            .expect("matched path from extensions");

        assert_eq!(matched_path.as_str(), "/users/{id}");
    }

    #[test]
    fn matched_path_clones_share_the_route_template() {
        let matched_path = MatchedPath::new("/users/{id}");
        let clone = matched_path.clone();

        assert!(Arc::ptr_eq(&matched_path.0, &clone.0));
    }

    #[test]
    fn optional_matched_path_from_extensions_returns_none_when_absent() {
        let extensions = Extensions::new();

        let matched_path = MatchedPath::optional_from_request_extensions(&extensions);

        assert!(matched_path.is_none());
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

    #[test]
    fn path_from_params_deserializes_route_params() {
        let params = RouteParams::from_pairs([("name".to_string(), "roze".to_string())]);

        let Path(payload) =
            Path::<Payload>::from_route_params(&params).expect("path from route params");

        assert_eq!(
            payload,
            Payload {
                name: "roze".to_string()
            }
        );
    }

    #[test]
    fn path_from_params_preserves_form_delimiters_and_decodes_percent_escapes() {
        let params = RouteParams::from_pairs([("name".to_string(), "a&b=c+d%20e".to_string())]);

        let Path(payload) =
            Path::<Payload>::from_route_params(&params).expect("path from route params");

        assert_eq!(payload.name, "a&b=c+d e");
    }

    #[test]
    fn path_from_params_deserializes_single_scalars() {
        let text = RouteParams::from_pairs([("value".to_string(), "one%20two".to_string())]);
        let number = RouteParams::from_pairs([("value".to_string(), "42".to_string())]);

        assert_eq!(
            Path::<String>::from_route_params(&text)
                .expect("string path")
                .0,
            "one two"
        );
        assert_eq!(
            Path::<u64>::from_route_params(&number)
                .expect("number path")
                .0,
            42
        );
    }

    #[test]
    fn path_from_params_deserializes_positional_tuple() {
        let params = RouteParams::from_pairs([
            ("id".to_string(), "42".to_string()),
            ("active".to_string(), "true".to_string()),
            ("name".to_string(), "roze%20team".to_string()),
        ]);

        let Path(values) =
            Path::<(u64, bool, String)>::from_route_params(&params).expect("tuple path");

        assert_eq!(values, (42, true, "roze team".to_string()));
    }

    #[test]
    fn path_from_params_deserializes_typed_struct_fields() {
        #[derive(Debug, Deserialize, PartialEq, Eq)]
        struct TypedPath {
            id: u64,
            active: bool,
        }

        let params = RouteParams::from_pairs([
            ("id".to_string(), "42".to_string()),
            ("active".to_string(), "true".to_string()),
        ]);

        let Path(value) = Path::<TypedPath>::from_route_params(&params).expect("typed struct path");

        assert_eq!(
            value,
            TypedPath {
                id: 42,
                active: true
            }
        );
    }

    #[test]
    fn path_can_be_read_synchronously_from_extensions() {
        let mut extensions = Extensions::new();
        extensions.insert(RouteParams::from_pairs([(
            "name".to_string(),
            "roze%20team".to_string(),
        )]));

        let Path(payload) =
            Path::<Payload>::from_request_extensions(&extensions).expect("path from extensions");

        assert_eq!(payload.name, "roze team");
    }

    #[test]
    fn optional_path_from_extensions_returns_none_without_params() {
        let extensions = Extensions::new();

        let path = Path::<Payload>::optional_from_request_extensions(&extensions)
            .expect("optional path from extensions");

        assert!(path.is_none());
    }

    #[test]
    fn path_optional_from_params_returns_none_without_params() {
        let path = Path::<Payload>::optional_from_route_params(None).expect("optional path params");

        assert!(path.is_none());
    }

    #[test]
    fn path_from_params_preserves_parse_errors() {
        let params = RouteParams::from_pairs([("other".to_string(), "roze".to_string())]);

        let error = Path::<Payload>::from_route_params(&params).unwrap_err();

        assert_eq!(error.code(), 400);
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

    #[test]
    fn raw_path_params_can_be_read_synchronously_from_extensions() {
        let mut extensions = Extensions::new();
        extensions.insert(RouteParams::from_pairs([(
            "team".to_string(),
            "core%20team".to_string(),
        )]));

        let params = RawPathParams::from_request_extensions(&extensions)
            .expect("raw path params from extensions");

        assert_eq!(params.get("team"), Some("core team"));
    }

    #[test]
    fn repeated_raw_path_extraction_shares_decoded_storage() {
        let mut extensions = Extensions::new();
        extensions.insert(RouteParams::from_pairs([(
            "team".to_string(),
            "core%20team".to_string(),
        )]));

        let first =
            RawPathParams::from_request_extensions(&extensions).expect("first raw path extraction");
        let second = RawPathParams::from_request_extensions(&extensions)
            .expect("second raw path extraction");

        assert!(first.0.shares_storage_with(&second.0));
    }

    #[test]
    fn optional_raw_path_params_from_extensions_returns_none_when_absent() {
        let extensions = Extensions::new();

        let params = RawPathParams::optional_from_request_extensions(&extensions)
            .expect("optional raw path params from extensions");

        assert!(params.is_none());
    }

    #[tokio::test]
    async fn raw_path_params_percent_decode_without_treating_plus_as_space() {
        let mut request = Request::builder()
            .method("GET")
            .uri("/teams/core%20team%2Fblue+green")
            .body(rest::empty_body())
            .unwrap();
        request.extensions_mut().insert(RouteParams::from_pairs([(
            "team".to_string(),
            "core%20team%2Fblue+green".to_string(),
        )]));

        let params = RawPathParams::from_request(request)
            .await
            .expect("raw path params");

        assert_eq!(params.get("team"), Some("core team/blue+green"));
    }

    #[tokio::test]
    async fn raw_path_params_reject_invalid_percent_decoded_utf8() {
        let mut request = Request::builder()
            .method("GET")
            .uri("/teams/%FF")
            .body(rest::empty_body())
            .unwrap();
        request.extensions_mut().insert(RouteParams::from_pairs([(
            "team".to_string(),
            "%FF".to_string(),
        )]));

        let error = RawPathParams::from_request(request).await.unwrap_err();

        assert_eq!(error.code(), 400);
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
    async fn optional_original_uri_prefers_request_extension() {
        let mut request = Request::builder()
            .method("GET")
            .uri("/users?name=roze")
            .body(rest::empty_body())
            .unwrap();
        request
            .extensions_mut()
            .insert(OriginalUri("/api/users?name=roze".parse().unwrap()));

        let original = Option::<OriginalUri>::from_request(request)
            .await
            .expect("optional original uri")
            .expect("original uri is always available");

        assert_eq!(original.0, "/api/users?name=roze");
    }

    #[tokio::test]
    async fn optional_original_uri_from_parts_prefers_request_extension() {
        let mut request = Request::builder()
            .method("GET")
            .uri("/users?name=roze")
            .body(rest::empty_body())
            .unwrap();
        request
            .extensions_mut()
            .insert(OriginalUri("/api/users?name=roze".parse().unwrap()));
        let (mut parts, _body) = request.into_parts();

        let original = Option::<OriginalUri>::from_request_parts(&mut parts)
            .await
            .expect("optional original uri")
            .expect("original uri is always available");

        assert_eq!(original.0, "/api/users?name=roze");
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
    async fn raw_form_extracts_query_for_head_requests() {
        let request = Request::builder()
            .method("HEAD")
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

    #[test]
    fn form_from_bytes_deserializes_urlencoded_body() {
        let Form(payload) = Form::<Payload>::from_bytes(b"name=roze").expect("form from bytes");

        assert_eq!(
            payload,
            Payload {
                name: "roze".to_string()
            }
        );
    }

    #[test]
    fn form_from_bytes_preserves_parse_errors() {
        let error = Form::<Payload>::from_bytes(b"other=roze").unwrap_err();

        assert_eq!(error.code(), 400);
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
