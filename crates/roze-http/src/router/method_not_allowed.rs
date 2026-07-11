use std::{
    convert::Infallible,
    future::{ready, Ready},
    task::{Context, Poll},
};

use http::{header, HeaderValue, Method, StatusCode};
use tower::Service;

use crate::rest::{self, HttpResponse, IncomingRequest};

#[derive(Clone, Debug)]
pub(super) enum AllowHeader {
    Value(HeaderValue),
    Skip,
}

impl AllowHeader {
    pub(super) fn from_methods<'a>(methods: impl IntoIterator<Item = Option<&'a Method>>) -> Self {
        let mut allowed = Vec::new();
        for method in methods {
            let Some(method) = method else {
                return Self::Skip;
            };
            allowed.push(method.to_string());
        }
        if allowed.iter().any(|method| method == Method::GET.as_str()) {
            allowed.push(Method::HEAD.to_string());
        }
        allowed.sort();
        allowed.dedup();
        let value = HeaderValue::from_str(&allowed.join(", "))
            .expect("registered HTTP methods must produce a valid Allow header");
        Self::Value(value)
    }

    fn value(&self) -> Option<&HeaderValue> {
        match self {
            Self::Value(value) => Some(value),
            Self::Skip => None,
        }
    }
}

impl Default for AllowHeader {
    fn default() -> Self {
        Self::Value(HeaderValue::from_static(""))
    }
}

#[derive(Clone, Debug)]
pub(super) struct MethodNotAllowed {
    allow: AllowHeader,
}

impl MethodNotAllowed {
    pub(super) fn new(allow: AllowHeader) -> Self {
        Self { allow }
    }
}

impl Service<IncomingRequest> for MethodNotAllowed {
    type Response = HttpResponse;
    type Error = Infallible;
    type Future = Ready<Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, _request: IncomingRequest) -> Self::Future {
        let mut response =
            rest::text_response(StatusCode::METHOD_NOT_ALLOWED, "method not allowed");
        if let Some(allow) = self.allow.value() {
            response.headers_mut().insert(header::ALLOW, allow.clone());
        }
        ready(Ok(response))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allow_header_is_built_once_with_implicit_head() {
        let get = Method::GET;
        let post = Method::POST;
        let allow = AllowHeader::from_methods([Some(&post), Some(&get), Some(&get)]);

        assert_eq!(
            allow.value(),
            Some(&HeaderValue::from_static("GET, HEAD, POST"))
        );
    }

    #[test]
    fn any_route_skips_allow_header() {
        let get = Method::GET;
        let allow = AllowHeader::from_methods([Some(&get), None]);

        assert!(matches!(allow, AllowHeader::Skip));
    }
}
