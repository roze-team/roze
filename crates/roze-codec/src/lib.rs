use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde::{de::DeserializeOwned, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodecKind {
    Json,
    Base64,
}

pub fn encode_base64(input: impl AsRef<[u8]>) -> String {
    STANDARD.encode(input.as_ref())
}

pub fn decode_base64(input: impl AsRef<[u8]>) -> Result<Vec<u8>, base64::DecodeError> {
    STANDARD.decode(input.as_ref())
}

pub fn codec_name(kind: CodecKind) -> &'static str {
    match kind {
        CodecKind::Json => "json",
        CodecKind::Base64 => "base64",
    }
}

pub fn encode_json<T>(value: &T) -> serde_json::Result<String>
where
    T: Serialize,
{
    serde_json::to_string(value)
}

pub fn encode_json_pretty<T>(value: &T) -> serde_json::Result<String>
where
    T: Serialize,
{
    serde_json::to_string_pretty(value)
}

pub fn decode_json<T>(input: impl AsRef<str>) -> serde_json::Result<T>
where
    T: DeserializeOwned,
{
    serde_json::from_str(input.as_ref())
}

pub fn encode_json_base64<T>(value: &T) -> serde_json::Result<String>
where
    T: Serialize,
{
    encode_json(value).map(encode_base64)
}

pub fn decode_json_base64<T>(input: impl AsRef<str>) -> Result<T, CodecDecodeError>
where
    T: DeserializeOwned,
{
    let decoded = decode_base64(input.as_ref().as_bytes()).map_err(CodecDecodeError::Base64)?;
    serde_json::from_slice(&decoded).map_err(CodecDecodeError::Json)
}

#[derive(Debug)]
pub enum CodecDecodeError {
    Base64(base64::DecodeError),
    Json(serde_json::Error),
}

impl std::fmt::Display for CodecDecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CodecDecodeError::Base64(err) => write!(f, "base64 decode error: {err}"),
            CodecDecodeError::Json(err) => write!(f, "json decode error: {err}"),
        }
    }
}

impl std::error::Error for CodecDecodeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            CodecDecodeError::Base64(err) => Some(err),
            CodecDecodeError::Json(err) => Some(err),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Serialize, serde::Deserialize, PartialEq, Eq)]
    struct Payload {
        name: String,
        enabled: bool,
    }

    #[test]
    fn round_trips_base64() {
        let encoded = encode_base64("roze");
        let decoded = decode_base64(encoded).expect("decode");
        assert_eq!(decoded, b"roze");
    }

    #[test]
    fn round_trips_json() {
        let payload = Payload {
            name: "roze".into(),
            enabled: true,
        };
        let encoded = encode_json(&payload).expect("encode");
        let decoded: Payload = decode_json(&encoded).expect("decode");
        assert_eq!(decoded, payload);
    }

    #[test]
    fn round_trips_json_base64() {
        let payload = Payload {
            name: "roze".into(),
            enabled: true,
        };
        let encoded = encode_json_base64(&payload).expect("encode");
        let decoded: Payload = decode_json_base64(&encoded).expect("decode");
        assert_eq!(decoded, payload);
    }

    #[test]
    fn decode_error_exposes_source() {
        let err = decode_json_base64::<Payload>("not-base64").expect_err("decode should fail");
        assert!(std::error::Error::source(&err).is_some());
    }
}
