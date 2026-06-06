use base64::{engine::general_purpose::STANDARD, Engine as _};

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_base64() {
        let encoded = encode_base64("roze");
        let decoded = decode_base64(encoded).expect("decode");
        assert_eq!(decoded, b"roze");
    }
}
