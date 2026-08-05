use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ApiResponse<T> {
    pub code: i32,
    pub msg: String,
    pub data: Option<T>,
}

/// An opt-in envelope for APIs that expose stable string business codes.
/// Existing [`ApiResponse`] users retain the numeric `code: 0` wire format.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CodedApiResponse<T> {
    pub code: String,
    pub msg: String,
    pub data: Option<T>,
}

impl<T> CodedApiResponse<T> {
    pub fn ok(data: T) -> Self {
        Self {
            code: "OK".to_string(),
            msg: "OK".to_string(),
            data: Some(data),
        }
    }

    pub fn success(code: impl Into<String>, msg: impl Into<String>, data: Option<T>) -> Self {
        Self {
            code: code.into(),
            msg: msg.into(),
            data,
        }
    }
}

impl<T> ApiResponse<T> {
    pub fn ok(data: T) -> Self {
        Self {
            code: 0,
            msg: "OK".to_string(),
            data: Some(data),
        }
    }

    pub fn error(code: i32, msg: impl Into<String>) -> Self {
        Self {
            code,
            msg: msg.into(),
            data: None,
        }
    }
}

pub type ApiResult<T> = Result<ApiResponse<T>, ()>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_ok_response() {
        let resp = ApiResponse::ok(123);
        assert_eq!(resp.code, 0);
        assert_eq!(resp.msg, "OK");
        assert_eq!(resp.data, Some(123));
    }

    #[test]
    fn creates_string_coded_ok_response_without_changing_legacy_envelope() {
        let response = CodedApiResponse::ok(123);
        assert_eq!(response.code, "OK");
        assert_eq!(response.msg, "OK");
        assert_eq!(response.data, Some(123));

        let legacy = ApiResponse::ok(123);
        assert_eq!(legacy.code, 0);
    }
}
