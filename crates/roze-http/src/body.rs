use std::{error::Error, fmt};

use http_body_util::BodyExt;

pub use bytes::Bytes;

pub use crate::rest::{Body, BoxError};

pub fn full(data: impl Into<Bytes>) -> Body {
    crate::rest::full_body(data)
}

pub fn empty() -> Body {
    crate::rest::empty_body()
}

pub async fn to_bytes(body: Body, limit: usize) -> Result<Bytes, BodyError> {
    let bytes = body.collect().await.map_err(BodyError::Body)?.to_bytes();
    if bytes.len() > limit {
        return Err(BodyError::LengthLimitExceeded {
            limit,
            actual: bytes.len(),
        });
    }
    Ok(bytes)
}

#[derive(Debug)]
pub enum BodyError {
    Body(BoxError),
    LengthLimitExceeded { limit: usize, actual: usize },
}

impl fmt::Display for BodyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Body(error) => write!(formatter, "{error}"),
            Self::LengthLimitExceeded { limit, actual } => {
                write!(formatter, "body length {actual} exceeds limit {limit}")
            }
        }
    }
}

impl Error for BodyError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Body(error) => Some(error.as_ref()),
            Self::LengthLimitExceeded { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn to_bytes_collects_body_under_limit() {
        let bytes = to_bytes(full("roze"), 8).await.unwrap();
        assert_eq!(&bytes[..], b"roze");
    }

    #[tokio::test]
    async fn to_bytes_rejects_body_over_limit() {
        let error = to_bytes(full("roze"), 3).await.unwrap_err();
        assert!(matches!(
            error,
            BodyError::LengthLimitExceeded {
                limit: 3,
                actual: 4
            }
        ));
    }

    #[tokio::test]
    async fn empty_body_collects_to_empty_bytes() {
        let bytes = to_bytes(empty(), 0).await.unwrap();
        assert!(bytes.is_empty());
    }
}
