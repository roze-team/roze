use std::{
    convert::{Infallible, TryInto},
    fmt,
    ops::{Deref, DerefMut},
};

use bytes::Bytes;
use http::{header, Extensions, HeaderMap, HeaderName, HeaderValue, StatusCode};
use serde::Serialize;

use crate::rest::{self, HttpResponse};

pub type Response = HttpResponse;
pub type Result<T, E = ErrorResponse> = std::result::Result<T, E>;

pub trait IntoResponse {
    fn into_response(self) -> HttpResponse;
}

pub struct ErrorResponse(HttpResponse);

impl ErrorResponse {
    pub fn into_inner(self) -> HttpResponse {
        self.0
    }
}

impl<T> From<T> for ErrorResponse
where
    T: IntoResponse,
{
    fn from(value: T) -> Self {
        Self(value.into_response())
    }
}

pub struct ResponseParts {
    response: HttpResponse,
}

impl ResponseParts {
    pub fn new(response: HttpResponse) -> Self {
        Self { response }
    }

    pub fn headers_mut(&mut self) -> &mut HeaderMap {
        self.response.headers_mut()
    }

    pub fn extensions(&self) -> &Extensions {
        self.response.extensions()
    }

    pub fn extensions_mut(&mut self) -> &mut Extensions {
        self.response.extensions_mut()
    }

    pub fn into_response(self) -> HttpResponse {
        self.response
    }
}

pub trait IntoResponseParts {
    type Error: IntoResponse;

    fn into_response_parts(
        self,
        parts: ResponseParts,
    ) -> std::result::Result<ResponseParts, Self::Error>;
}

impl IntoResponseParts for () {
    type Error = Infallible;

    fn into_response_parts(
        self,
        parts: ResponseParts,
    ) -> std::result::Result<ResponseParts, Self::Error> {
        Ok(parts)
    }
}

impl<P> IntoResponseParts for Option<P>
where
    P: IntoResponseParts,
{
    type Error = P::Error;

    fn into_response_parts(
        self,
        parts: ResponseParts,
    ) -> std::result::Result<ResponseParts, Self::Error> {
        match self {
            Some(part) => part.into_response_parts(parts),
            None => Ok(parts),
        }
    }
}

macro_rules! impl_tuple_into_response_parts {
    ($($ty:ident),+ $(,)?) => {
        impl<$($ty,)+> IntoResponseParts for ($($ty,)+)
        where
            $($ty: IntoResponseParts,)+
        {
            type Error = HttpResponse;

            #[allow(non_snake_case)]
            fn into_response_parts(
                self,
                parts: ResponseParts,
            ) -> std::result::Result<ResponseParts, Self::Error> {
                let ($($ty,)+) = self;
                $(
                    let parts = match $ty.into_response_parts(parts) {
                        Ok(parts) => parts,
                        Err(error) => return Err(error.into_response()),
                    };
                )+
                Ok(parts)
            }
        }
    };
}

impl_tuple_into_response_parts!(A, B);
impl_tuple_into_response_parts!(A, B, C);
impl_tuple_into_response_parts!(A, B, C, D);

impl IntoResponseParts for HeaderMap {
    type Error = Infallible;

    fn into_response_parts(
        self,
        mut parts: ResponseParts,
    ) -> std::result::Result<ResponseParts, Self::Error> {
        parts.headers_mut().extend(self);
        Ok(parts)
    }
}

impl<K, V, const N: usize> IntoResponseParts for [(K, V); N]
where
    K: TryInto<HeaderName>,
    K::Error: fmt::Display,
    V: TryInto<HeaderValue>,
    V::Error: fmt::Display,
{
    type Error = roze_error::RozeError;

    fn into_response_parts(
        self,
        mut parts: ResponseParts,
    ) -> std::result::Result<ResponseParts, Self::Error> {
        for (key, value) in self {
            let key = key
                .try_into()
                .map_err(|error| roze_error::RozeError::Internal(error.to_string()))?;
            let value = value
                .try_into()
                .map_err(|error| roze_error::RozeError::Internal(error.to_string()))?;
            parts.headers_mut().insert(key, value);
        }
        Ok(parts)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct AppendHeaders<I>(pub I);

impl<I, K, V> IntoResponseParts for AppendHeaders<I>
where
    I: IntoIterator<Item = (K, V)>,
    K: TryInto<HeaderName>,
    K::Error: fmt::Display,
    V: TryInto<HeaderValue>,
    V::Error: fmt::Display,
{
    type Error = roze_error::RozeError;

    fn into_response_parts(
        self,
        mut parts: ResponseParts,
    ) -> std::result::Result<ResponseParts, Self::Error> {
        for (key, value) in self.0 {
            let key = key
                .try_into()
                .map_err(|error| roze_error::RozeError::Internal(error.to_string()))?;
            let value = value
                .try_into()
                .map_err(|error| roze_error::RozeError::Internal(error.to_string()))?;
            parts.headers_mut().append(key, value);
        }
        Ok(parts)
    }
}

impl<I> IntoResponse for AppendHeaders<I>
where
    AppendHeaders<I>: IntoResponseParts,
{
    fn into_response(self) -> HttpResponse {
        (self, ()).into_response()
    }
}

impl<T> IntoResponseParts for crate::extract::Extension<T>
where
    T: Clone + Send + Sync + 'static,
{
    type Error = Infallible;

    fn into_response_parts(
        self,
        mut parts: ResponseParts,
    ) -> std::result::Result<ResponseParts, Self::Error> {
        parts.extensions_mut().insert(self.0);
        Ok(parts)
    }
}

impl IntoResponse for HttpResponse {
    fn into_response(self) -> HttpResponse {
        self
    }
}

impl IntoResponse for StatusCode {
    fn into_response(self) -> HttpResponse {
        rest::empty_response(self)
    }
}

impl IntoResponse for () {
    fn into_response(self) -> HttpResponse {
        rest::empty_response(StatusCode::OK)
    }
}

impl IntoResponse for HeaderMap {
    fn into_response(self) -> HttpResponse {
        let mut response = rest::empty_response(StatusCode::OK);
        *response.headers_mut() = self;
        response
    }
}

impl<K, V, const N: usize> IntoResponse for [(K, V); N]
where
    [(K, V); N]: IntoResponseParts,
{
    fn into_response(self) -> HttpResponse {
        (self, ()).into_response()
    }
}

impl<T> IntoResponse for crate::extract::Extension<T>
where
    T: Clone + Send + Sync + 'static,
{
    fn into_response(self) -> HttpResponse {
        (self, StatusCode::OK).into_response()
    }
}

impl IntoResponse for &'static str {
    fn into_response(self) -> HttpResponse {
        rest::text_response(StatusCode::OK, self)
    }
}

impl IntoResponse for String {
    fn into_response(self) -> HttpResponse {
        rest::text_response(StatusCode::OK, self)
    }
}

impl IntoResponse for Bytes {
    fn into_response(self) -> HttpResponse {
        bytes_response(self)
    }
}

impl IntoResponse for Vec<u8> {
    fn into_response(self) -> HttpResponse {
        bytes_response(self)
    }
}

impl IntoResponse for &'static [u8] {
    fn into_response(self) -> HttpResponse {
        bytes_response(self)
    }
}

impl<const N: usize> IntoResponse for [u8; N] {
    fn into_response(self) -> HttpResponse {
        bytes_response(self.to_vec())
    }
}

fn bytes_response(body: impl Into<Bytes>) -> HttpResponse {
    http::Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .body(rest::full_body(body))
        .expect("bytes response")
}

impl<T> IntoResponse for roze_result::ApiResponse<T>
where
    T: Serialize,
{
    fn into_response(self) -> HttpResponse {
        rest::api_response(&self)
    }
}

impl IntoResponse for roze_error::RozeError {
    fn into_response(self) -> HttpResponse {
        log_roze_error(&self);
        rest::error_response(&self)
    }
}

fn log_roze_error(error: &roze_error::RozeError) {
    let status = error.status_code();
    let ids = roze_error::current_request_ids();
    let request_id = ids
        .as_ref()
        .map(|ids| ids.request_id.as_str())
        .unwrap_or_default();
    let trace_id = ids
        .as_ref()
        .map(|ids| ids.trace_id.as_str())
        .unwrap_or_default();
    if status.is_server_error() {
        tracing::error!(
            protocol = "http",
            status = status.as_u16(),
            code = error.code(),
            error_kind = error.kind(),
            request_id,
            trace_id,
            error = %error,
            "HTTP request failed"
        );
    } else {
        tracing::warn!(
            protocol = "http",
            status = status.as_u16(),
            code = error.code(),
            error_kind = error.kind(),
            request_id,
            trace_id,
            error = %error,
            "HTTP request rejected"
        );
    }
}

impl<T, E> IntoResponse for std::result::Result<T, E>
where
    T: IntoResponse,
    E: IntoResponse,
{
    fn into_response(self) -> HttpResponse {
        match self {
            Ok(value) => value.into_response(),
            Err(error) => error.into_response(),
        }
    }
}

impl<T> IntoResponse for std::result::Result<T, ErrorResponse>
where
    T: IntoResponse,
{
    fn into_response(self) -> HttpResponse {
        match self {
            Ok(value) => value.into_response(),
            Err(error) => error.into_inner(),
        }
    }
}

impl IntoResponse for Infallible {
    fn into_response(self) -> HttpResponse {
        match self {}
    }
}

#[must_use = "needs to be returned from a handler or otherwise turned into a response"]
#[derive(Debug, Clone)]
pub struct Redirect {
    status: StatusCode,
    location: String,
}

impl Redirect {
    pub fn to(uri: impl Into<String>) -> Self {
        Self::with_status(StatusCode::SEE_OTHER, uri)
    }

    pub fn temporary(uri: impl Into<String>) -> Self {
        Self::with_status(StatusCode::TEMPORARY_REDIRECT, uri)
    }

    pub fn permanent(uri: impl Into<String>) -> Self {
        Self::with_status(StatusCode::PERMANENT_REDIRECT, uri)
    }

    pub fn with_status(status: StatusCode, uri: impl Into<String>) -> Self {
        assert!(status.is_redirection(), "not a redirection status code");
        Self {
            status,
            location: uri.into(),
        }
    }

    pub fn status(&self) -> StatusCode {
        self.status
    }

    pub fn location(&self) -> &str {
        &self.location
    }
}

impl IntoResponse for Redirect {
    fn into_response(self) -> HttpResponse {
        match HeaderValue::try_from(self.location) {
            Ok(location) => {
                (self.status, [(header::LOCATION, location)], StatusCode::OK).into_response()
            }
            Err(error) => (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()).into_response(),
        }
    }
}

impl<R> IntoResponse for (StatusCode, R)
where
    R: IntoResponse,
{
    fn into_response(self) -> HttpResponse {
        let (status, body) = self;
        let mut response = body.into_response();
        *response.status_mut() = status;
        response
    }
}

impl<P, R> IntoResponse for (P, R)
where
    P: IntoResponseParts,
    R: IntoResponse,
{
    fn into_response(self) -> HttpResponse {
        let (part, body) = self;
        let parts = ResponseParts::new(body.into_response());
        match part.into_response_parts(parts) {
            Ok(parts) => parts.into_response(),
            Err(error) => error.into_response(),
        }
    }
}

impl<P, R> IntoResponse for (StatusCode, P, R)
where
    P: IntoResponseParts,
    R: IntoResponse,
{
    fn into_response(self) -> HttpResponse {
        let (status, part, body) = self;
        let mut response = (part, body).into_response();
        *response.status_mut() = status;
        response
    }
}

macro_rules! impl_flat_parts_into_response {
    ($($part:ident),+ $(,)?) => {
        impl<$($part,)+ R> IntoResponse for ($($part,)+ R)
        where
            $($part: IntoResponseParts,)+
            R: IntoResponse,
        {
            #[allow(non_snake_case)]
            fn into_response(self) -> HttpResponse {
                let ($($part,)+ body) = self;
                (($($part,)+), body).into_response()
            }
        }
    };
}

macro_rules! impl_flat_status_parts_into_response {
    ($($part:ident),+ $(,)?) => {
        impl<$($part,)+ R> IntoResponse for (StatusCode, $($part,)+ R)
        where
            $($part: IntoResponseParts,)+
            R: IntoResponse,
        {
            #[allow(non_snake_case)]
            fn into_response(self) -> HttpResponse {
                let (status, $($part,)+ body) = self;
                (status, ($($part,)+), body).into_response()
            }
        }
    };
}

impl_flat_parts_into_response!(A, B);
impl_flat_parts_into_response!(A, B, C);
impl_flat_parts_into_response!(A, B, C, D);
impl_flat_status_parts_into_response!(A, B);
impl_flat_status_parts_into_response!(A, B, C);
impl_flat_status_parts_into_response!(A, B, C, D);

#[derive(Clone, Debug)]
pub struct Json<T>(pub T);

impl<T> Deref for Json<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> DerefMut for Json<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl<T> IntoResponse for Json<T>
where
    T: Serialize,
{
    fn into_response(self) -> HttpResponse {
        rest::json_response(StatusCode::OK, &self.0)
    }
}

impl<T> IntoResponse for crate::extract::Form<T>
where
    T: Serialize,
{
    fn into_response(self) -> HttpResponse {
        match serde_urlencoded::to_string(&self.0) {
            Ok(body) => {
                let mut response = rest::text_response(StatusCode::OK, body);
                response.headers_mut().insert(
                    header::CONTENT_TYPE,
                    HeaderValue::from_static("application/x-www-form-urlencoded"),
                );
                response
            }
            Err(error) => (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()).into_response(),
        }
    }
}

pub struct Html<T>(pub T);

impl<T> Deref for Html<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> DerefMut for Html<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl<T> IntoResponse for Html<T>
where
    T: Into<String>,
{
    fn into_response(self) -> HttpResponse {
        let mut response = rest::text_response(StatusCode::OK, self.0.into());
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/html; charset=utf-8"),
        );
        response
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use http::{HeaderMap, HeaderValue, StatusCode};
    use http_body_util::BodyExt;
    use serde::Serialize;

    use crate::extract::{Extension, Form};

    use super::{AppendHeaders, ErrorResponse, Html, IntoResponse, Json, Redirect};

    #[derive(Clone, Default)]
    struct LogBuffer {
        bytes: Arc<Mutex<Vec<u8>>>,
    }

    struct LogWriter(LogBuffer);

    impl std::io::Write for LogWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            let mut buffer = self.0.bytes.lock().expect("log buffer lock");
            std::io::Write::write(&mut *buffer, bytes)
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'writer> tracing_subscriber::fmt::MakeWriter<'writer> for LogBuffer {
        type Writer = LogWriter;

        fn make_writer(&'writer self) -> Self::Writer {
            LogWriter(self.clone())
        }
    }

    impl LogBuffer {
        fn contents(&self) -> String {
            String::from_utf8(self.bytes.lock().expect("log buffer lock").clone())
                .expect("UTF-8 tracing output")
        }
    }

    #[derive(Serialize)]
    struct Payload {
        name: &'static str,
    }

    #[derive(Serialize)]
    struct FormPayload {
        name: &'static str,
        role: &'static str,
    }

    #[test]
    fn roze_errors_emit_structured_boundary_logs() {
        let logs = LogBuffer::default();
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_max_level(tracing::Level::TRACE)
            .with_writer(logs.clone())
            .finish();

        tracing::subscriber::with_default(subscriber, || {
            let _ = roze_error::RozeError::Internal("database failed".to_string()).into_response();
            let _ = roze_error::RozeError::BadRequest("invalid name".to_string()).into_response();
        });

        let output = logs.contents();
        assert!(output.contains("ERROR"));
        assert!(output.contains("HTTP request failed"));
        assert!(output.contains("error_kind=\"internal\""));
        assert!(output.contains("database failed"));
        assert!(output.contains("WARN"));
        assert!(output.contains("HTTP request rejected"));
        assert!(output.contains("error_kind=\"bad_request\""));
        assert!(output.contains("invalid name"));
    }

    #[tokio::test]
    async fn status_tuple_overrides_response_status() {
        let response = (StatusCode::CREATED, "created").into_response();
        assert_eq!(response.status(), StatusCode::CREATED);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"created");
    }

    #[tokio::test]
    async fn unit_response_is_empty_ok() {
        let response = ().into_response();

        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert!(body.is_empty());
    }

    #[tokio::test]
    async fn response_result_alias_maps_default_error_response() {
        #[allow(clippy::result_large_err)]
        fn handler() -> super::Result<&'static str> {
            Err(roze_error::RozeError::NotFound("missing".to_string()))?
        }

        let response = handler().into_response();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], br#"{"code":404,"msg":"missing","data":null}"#);
    }

    #[tokio::test]
    async fn error_response_accepts_existing_http_response() {
        let result: super::Result<&'static str> =
            Err(ErrorResponse::from((StatusCode::BAD_REQUEST, "bad")));
        let response = result.into_response();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"bad");
    }

    #[tokio::test]
    async fn header_array_can_be_response_by_itself() {
        let response = [("x-roze-test", "headers-only")].into_response();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get("x-roze-test"),
            Some(&HeaderValue::from_static("headers-only"))
        );
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert!(body.is_empty());
    }

    #[tokio::test]
    async fn header_tuple_extends_response_headers() {
        let mut headers = HeaderMap::new();
        headers.insert("x-roze-test", HeaderValue::from_static("yes"));
        let response = (headers, Json(Payload { name: "roze" })).into_response();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get("x-roze-test"),
            Some(&HeaderValue::from_static("yes"))
        );
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], br#"{"name":"roze"}"#);
    }

    #[tokio::test]
    async fn html_response_sets_content_type() {
        let response = Html("<strong>roze</strong>").into_response();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(http::header::CONTENT_TYPE),
            Some(&HeaderValue::from_static("text/html; charset=utf-8"))
        );
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"<strong>roze</strong>");
    }

    #[tokio::test]
    async fn form_response_sets_urlencoded_content_type() {
        let response = Form(FormPayload {
            name: "roze",
            role: "admin",
        })
        .into_response();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(http::header::CONTENT_TYPE),
            Some(&HeaderValue::from_static(
                "application/x-www-form-urlencoded"
            ))
        );
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"name=roze&role=admin");
    }

    #[tokio::test]
    async fn bytes_response_sets_octet_stream_content_type() {
        let response = bytes::Bytes::from_static(b"raw").into_response();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(http::header::CONTENT_TYPE),
            Some(&HeaderValue::from_static("application/octet-stream"))
        );
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"raw");
    }

    #[tokio::test]
    async fn vec_u8_response_sets_octet_stream_content_type() {
        let response = vec![1_u8, 2, 3].into_response();

        assert_eq!(
            response.headers().get(http::header::CONTENT_TYPE),
            Some(&HeaderValue::from_static("application/octet-stream"))
        );
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], &[1, 2, 3]);
    }

    #[tokio::test]
    async fn byte_array_response_sets_octet_stream_content_type() {
        let response = [4_u8, 5, 6].into_response();

        assert_eq!(
            response.headers().get(http::header::CONTENT_TYPE),
            Some(&HeaderValue::from_static("application/octet-stream"))
        );
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], &[4, 5, 6]);
    }

    #[tokio::test]
    async fn status_and_header_tuple_combines_response_parts() {
        let mut headers = HeaderMap::new();
        headers.insert("x-roze-test", HeaderValue::from_static("yes"));
        let response = (StatusCode::ACCEPTED, headers, "queued").into_response();

        assert_eq!(response.status(), StatusCode::ACCEPTED);
        assert_eq!(
            response.headers().get("x-roze-test"),
            Some(&HeaderValue::from_static("yes"))
        );
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"queued");
    }

    #[tokio::test]
    async fn header_array_tuple_extends_response_headers() {
        let response = (
            StatusCode::CREATED,
            [("x-roze-test", "array"), ("x-roze-mode", "parts")],
            "created",
        )
            .into_response();

        assert_eq!(response.status(), StatusCode::CREATED);
        assert_eq!(
            response.headers().get("x-roze-test"),
            Some(&HeaderValue::from_static("array"))
        );
        assert_eq!(
            response.headers().get("x-roze-mode"),
            Some(&HeaderValue::from_static("parts"))
        );
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"created");
    }

    #[tokio::test]
    async fn append_headers_preserves_duplicate_header_values() {
        let response = (
            AppendHeaders([
                (http::header::SET_COOKIE, "a=1; Path=/"),
                (http::header::SET_COOKIE, "b=2; Path=/"),
            ]),
            "ok",
        )
            .into_response();

        let cookies = response
            .headers()
            .get_all(http::header::SET_COOKIE)
            .iter()
            .map(|value| value.to_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(cookies, vec!["a=1; Path=/", "b=2; Path=/"]);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"ok");
    }

    #[test]
    fn append_headers_can_be_response_by_itself() {
        let response = AppendHeaders([
            (http::header::SET_COOKIE, "a=1; Path=/"),
            (http::header::SET_COOKIE, "b=2; Path=/"),
        ])
        .into_response();

        let cookies = response
            .headers()
            .get_all(http::header::SET_COOKIE)
            .iter()
            .map(|value| value.to_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(cookies, vec!["a=1; Path=/", "b=2; Path=/"]);
    }

    #[tokio::test]
    async fn redirect_sets_status_and_location() {
        let response = Redirect::permanent("/new").into_response();

        assert_eq!(response.status(), StatusCode::PERMANENT_REDIRECT);
        assert_eq!(
            response.headers().get(http::header::LOCATION),
            Some(&HeaderValue::from_static("/new"))
        );
    }

    #[test]
    fn redirect_constructors_report_status_and_location() {
        assert_eq!(Redirect::to("/next").status(), StatusCode::SEE_OTHER);
        assert_eq!(
            Redirect::temporary("/next").status(),
            StatusCode::TEMPORARY_REDIRECT
        );
        assert_eq!(
            Redirect::permanent("/next").status(),
            StatusCode::PERMANENT_REDIRECT
        );
        assert_eq!(Redirect::permanent("/next").location(), "/next");
    }

    #[tokio::test]
    async fn redirect_invalid_location_returns_internal_error() {
        let response = Redirect::to("bad\nlocation").into_response();

        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert!(!body.is_empty());
    }

    #[tokio::test]
    async fn extension_tuple_inserts_response_extension() {
        let response = (Extension(String::from("trace-1")), "ok").into_response();

        assert_eq!(
            response.extensions().get::<String>().map(String::as_str),
            Some("trace-1")
        );
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"ok");
    }

    #[test]
    fn extension_can_be_response_by_itself() {
        let response = Extension(42usize).into_response();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.extensions().get::<usize>(), Some(&42));
    }

    #[tokio::test]
    async fn optional_response_parts_can_be_absent() {
        let headers: Option<[(&str, &str); 1]> = None;
        let response = (headers, "ok").into_response();

        assert!(response.headers().get("x-roze-optional").is_none());
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"ok");
    }

    #[tokio::test]
    async fn tuple_response_parts_apply_in_order() {
        let response = (
            (
                [("x-roze-first", "1")],
                Extension(String::from("trace-2")),
                [("x-roze-second", "2")],
            ),
            "ok",
        )
            .into_response();

        assert_eq!(
            response.headers().get("x-roze-first"),
            Some(&HeaderValue::from_static("1"))
        );
        assert_eq!(
            response.headers().get("x-roze-second"),
            Some(&HeaderValue::from_static("2"))
        );
        assert_eq!(
            response.extensions().get::<String>().map(String::as_str),
            Some("trace-2")
        );
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"ok");
    }

    #[tokio::test]
    async fn flat_response_parts_apply_before_body() {
        let response = (
            [("x-roze-first", "1")],
            Extension(String::from("trace-flat")),
            [("x-roze-second", "2")],
            "ok",
        )
            .into_response();

        assert_eq!(
            response.headers().get("x-roze-first"),
            Some(&HeaderValue::from_static("1"))
        );
        assert_eq!(
            response.headers().get("x-roze-second"),
            Some(&HeaderValue::from_static("2"))
        );
        assert_eq!(
            response.extensions().get::<String>().map(String::as_str),
            Some("trace-flat")
        );
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"ok");
    }

    #[tokio::test]
    async fn flat_status_response_parts_apply_before_body() {
        let response = (
            StatusCode::CREATED,
            [("x-roze-first", "1")],
            Extension(String::from("trace-status-flat")),
            [("x-roze-second", "2")],
            "created",
        )
            .into_response();

        assert_eq!(response.status(), StatusCode::CREATED);
        assert_eq!(
            response.headers().get("x-roze-first"),
            Some(&HeaderValue::from_static("1"))
        );
        assert_eq!(
            response.headers().get("x-roze-second"),
            Some(&HeaderValue::from_static("2"))
        );
        assert_eq!(
            response.extensions().get::<String>().map(String::as_str),
            Some("trace-status-flat")
        );
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"created");
    }
}
