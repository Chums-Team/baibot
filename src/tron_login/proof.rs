//! The proof token: what the client sends as `token` in the login request.
//!
//! Its shape is that of `TronAuthProofPayload` in the Chums client
//! (`lib/features/auth/tronlink/logic/tronlink_auth_models.dart`): a JSON object, base64url
//! without padding. The homeserver decodes it, looks the challenge up by `challenge_id`,
//! recovers the wallet address from `signature` and compares it with `address`.

use serde::Serialize;

use crate::utils::base64::base64_url_encode_no_pad;

#[derive(Serialize)]
struct ProofPayload<'a> {
    challenge_id: &'a str,
    address: &'a str,
    username: &'a str,
    signature: &'a str,
}

/// The proof token of a `login` challenge.
pub fn login_proof_token(
    challenge_id: &str,
    address: &str,
    username: &str,
    signature: &str,
) -> String {
    let payload = ProofPayload {
        challenge_id,
        address,
        username,
        signature,
    };

    let json = serde_json::to_vec(&payload).expect("a struct of strings serializes");

    base64_url_encode_no_pad(&json)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_the_chums_client_encoding() {
        // Produced by `encodeTronAuthProofToken` of the Chums client for the same fields:
        // compact JSON in this key order, base64url, `=` stripped.
        let token = login_proof_token(
            "3f1c2a6e-0d5b-4e0a-9d3e-6c1b2a4f5e7d",
            "TUEZSdKsoDHQMeZwihtdoBiN46zxhGWYdH",
            "baibot",
            "0x01f7",
        );

        assert_eq!(
            token,
            "eyJjaGFsbGVuZ2VfaWQiOiIzZjFjMmE2ZS0wZDViLTRlMGEtOWQzZS02YzFiMmE0ZjVlN2QiLCJhZGRyZXNzIjoiVFVFWlNkS3NvREhRTWVad2lodGRvQmlONDZ6eGhHV1lkSCIsInVzZXJuYW1lIjoiYmFpYm90Iiwic2lnbmF0dXJlIjoiMHgwMWY3In0"
        );
        assert!(!token.contains('='));
        assert!(!token.contains('+'));
        assert!(!token.contains('/'));
    }
}
