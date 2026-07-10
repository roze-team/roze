use std::{
    convert::Infallible,
    fmt,
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

use http::{header, HeaderValue, Method};
use hyper::body::Body as _;

use crate::rest::{self, HttpResponse};

type BoxFuture = Pin<Box<dyn Future<Output = Result<HttpResponse, Infallible>> + Send>>;

#[must_use = "futures do nothing unless polled"]
pub struct RouteFuture {
    inner: BoxFuture,
    method: Method,
}

impl RouteFuture {
    pub(super) fn new(method: Method, inner: BoxFuture) -> Self {
        Self { inner, method }
    }
}

impl Future for RouteFuture {
    type Output = Result<HttpResponse, Infallible>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let mut response = std::task::ready!(this.inner.as_mut().poll(cx))?;

        if this.method == Method::HEAD {
            set_content_length_if_known(&mut response);
            response = response.map(|_| rest::empty_body());
        } else if this.method == Method::CONNECT && response.status().is_success() {
            response.headers_mut().remove(header::CONTENT_LENGTH);
            response.headers_mut().remove(header::TRANSFER_ENCODING);
            response = response.map(|_| rest::empty_body());
        }

        Poll::Ready(Ok(response))
    }
}

impl fmt::Debug for RouteFuture {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RouteFuture")
            .field("method", &self.method)
            .finish_non_exhaustive()
    }
}

fn set_content_length_if_known(response: &mut HttpResponse) {
    if response.headers().contains_key(header::CONTENT_LENGTH) {
        return;
    }
    if let Some(size) = response.body().size_hint().exact() {
        let value = HeaderValue::from_str(&size.to_string())
            .expect("an exact HTTP body size must be a valid Content-Length value");
        response.headers_mut().insert(header::CONTENT_LENGTH, value);
    }
}
