#[cfg(feature = "tron-login")]
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::{Engine as _, engine::general_purpose::STANDARD};

pub(crate) fn base64_decode(base64_string: &str) -> Result<Vec<u8>, base64::DecodeError> {
    STANDARD.decode(base64_string)
}

pub(crate) fn base64_encode(data: &[u8]) -> String {
    STANDARD.encode(data)
}

/// Base64url without padding, as in JWTs and the Chums client's TRON login proof token.
#[cfg(feature = "tron-login")]
pub(crate) fn base64_url_encode_no_pad(data: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(data)
}
