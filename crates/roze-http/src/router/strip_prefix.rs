use std::{
    convert::Infallible,
    task::{Context, Poll},
};

use http::{uri::PathAndQuery, Uri};
use tower::Service;

use crate::{
    extract::NestedPath,
    rest::{HttpResponse, IncomingRequest},
};

use super::path::join_paths;

#[derive(Clone)]
pub(super) struct StripPrefixService<S> {
    prefix: String,
    inner: S,
}

impl<S> StripPrefixService<S> {
    pub(super) fn new(prefix: String, inner: S) -> Self {
        Self { prefix, inner }
    }
}

impl<S> Service<IncomingRequest> for StripPrefixService<S>
where
    S: Service<IncomingRequest, Response = HttpResponse, Error = Infallible>
        + Clone
        + Send
        + Sync
        + 'static,
    S::Future: Send + 'static,
{
    type Response = HttpResponse;
    type Error = Infallible;
    type Future = S::Future;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut request: IncomingRequest) -> Self::Future {
        let nested_path = match request.extensions().get::<NestedPath>() {
            Some(path) => join_paths(path.as_str(), &self.prefix),
            None => self.prefix.clone(),
        };
        request
            .extensions_mut()
            .insert(NestedPath::new(nested_path));
        *request.uri_mut() = strip_prefix_from_uri(request.uri(), &self.prefix);
        self.inner.call(request)
    }
}

fn strip_prefix_from_uri(uri: &Uri, prefix: &str) -> Uri {
    let path = uri.path();
    let Some(prefix_len) = matching_prefix_len(path, prefix) else {
        return uri.clone();
    };
    let stripped_path = &path[prefix_len..];
    let stripped_path = if stripped_path.is_empty() {
        "/"
    } else {
        stripped_path
    };
    let path_and_query = match uri.query() {
        Some(query) => format!("{stripped_path}?{query}"),
        None => stripped_path.to_string(),
    };

    let mut parts = uri.clone().into_parts();
    parts.path_and_query = Some(
        path_and_query
            .parse::<PathAndQuery>()
            .expect("stripped nested service path must be a valid path and query"),
    );
    Uri::from_parts(parts).expect("stripped nested service URI must be valid")
}

fn matching_prefix_len(path: &str, prefix: &str) -> Option<usize> {
    let mut path_segments = path.strip_prefix('/')?.split('/');
    let prefix_segments = prefix.strip_prefix('/')?.split('/');
    let mut matched_len = 0;

    for prefix_segment in prefix_segments {
        let path_segment = path_segments.next()?;
        if !segment_matches(prefix_segment, path_segment) {
            return None;
        }
        matched_len += 1 + path_segment.len();
    }

    Some(matched_len)
}

fn segment_matches(pattern: &str, value: &str) -> bool {
    let Some((prefix, suffix)) = capture_prefix_suffix(pattern) else {
        return pattern == value;
    };

    value.len() >= prefix.len() + suffix.len()
        && value.starts_with(prefix)
        && value.ends_with(suffix)
}

fn capture_prefix_suffix(segment: &str) -> Option<(&str, &str)> {
    let start = find_unescaped(segment.as_bytes(), b'{')?;
    let end = find_unescaped(segment.as_bytes(), b'}')?;
    (start < end).then(|| (&segment[..start], &segment[end + 1..]))
}

fn find_unescaped(haystack: &[u8], needle: u8) -> Option<usize> {
    let mut offset = 0;
    while let Some(relative) = haystack
        .get(offset..)?
        .iter()
        .position(|byte| *byte == needle)
    {
        let index = offset + relative;
        if haystack.get(index + 1) == Some(&needle) {
            offset = index + 2;
        } else {
            return Some(index);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_query_when_stripping_prefix() {
        let uri = "/api/users?active=1".parse().unwrap();

        assert_eq!(strip_prefix_from_uri(&uri, "/api"), "/users?active=1");
    }

    #[test]
    fn does_not_strip_partial_path_segment() {
        let uri = "/apiv2/users".parse().unwrap();

        assert_eq!(strip_prefix_from_uri(&uri, "/api"), uri);
    }

    #[test]
    fn strips_prefix_with_capture_using_request_segment_length() {
        let uri = "/api/v1/users?active=1".parse().unwrap();

        assert_eq!(
            strip_prefix_from_uri(&uri, "/api/{version}"),
            "/users?active=1"
        );
    }

    #[test]
    fn strips_prefix_with_embedded_capture() {
        let uri = "/api/v1.json/users".parse().unwrap();

        assert_eq!(strip_prefix_from_uri(&uri, "/api/{version}.json"), "/users");
    }
}
