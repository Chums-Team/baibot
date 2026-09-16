//! TRON key material: the secp256k1 signing key obtained from a raw private key or from a
//! BIP-39 seed phrase, the wallet address it controls, and TIP-191 message signing
//! (`signMessageV2` of TronWeb / TronLink), which is what the homeserver verifies
//! (`synapse-ever`, `modules/chums_module/users/tron_auth_utils.py`).

use std::fmt;

use k256::ecdsa::{SigningKey, VerifyingKey};
use sha3::{Digest, Keccak256};
use zeroize::Zeroizing;

/// Derivation path of the first TRON account of a seed phrase (SLIP-44 coin type 195).
///
/// The Chums wallet derives its TRON key from the same path, so a seed phrase yields the same
/// address in the client and in the bot.
pub const TRON_DERIVATION_PATH: &str = "m/44'/195'/0'/0/0";

/// Version byte of a TRON address: base58check of `0x41 || keccak256(public key)[12..]`.
const TRON_ADDRESS_PREFIX: u8 = 0x41;

/// TIP-191 prefix. The signed digest is `keccak256(prefix || <byte length> || message)`.
const TRON_MESSAGE_PREFIX: &str = "\x19TRON Signed Message:\n";

#[derive(Debug, thiserror::Error)]
pub enum TronKeyError {
    #[error("the private key must be 32 bytes in hex (64 hex characters, optionally 0x-prefixed)")]
    PrivateKeyFormat,

    #[error("the private key is not a valid secp256k1 private key")]
    PrivateKeyValue,

    #[error("the seed phrase must have 12, 15, 18, 21 or 24 words, got {0}")]
    SeedPhraseWordCount(usize),

    #[error("the seed phrase is not a valid BIP-39 mnemonic: {0}")]
    SeedPhraseInvalid(String),

    #[error("deriving the TRON key from the seed phrase failed: {0}")]
    Derivation(String),

    #[error("signing failed: {0}")]
    Signing(String),
}

/// A TRON wallet key and the address it controls.
///
/// `Debug` prints the address only.
pub struct TronSigner {
    signing_key: SigningKey,
    address: String,
}

impl TronSigner {
    /// A signer from a raw private key: 32 bytes in hex, optionally `0x`-prefixed, any case.
    pub fn from_private_key_hex(private_key: &str) -> Result<Self, TronKeyError> {
        let trimmed = private_key.trim();
        let hex_digits = trimmed
            .strip_prefix("0x")
            .or_else(|| trimmed.strip_prefix("0X"))
            .unwrap_or(trimmed);

        if hex_digits.len() != 64 {
            return Err(TronKeyError::PrivateKeyFormat);
        }

        let bytes =
            Zeroizing::new(hex::decode(hex_digits).map_err(|_| TronKeyError::PrivateKeyFormat)?);

        let signing_key =
            SigningKey::from_slice(&bytes).map_err(|_| TronKeyError::PrivateKeyValue)?;

        Ok(Self::from_signing_key(signing_key))
    }

    /// A signer from a BIP-39 seed phrase (English word list, no BIP-39 passphrase), taking the
    /// key at [`TRON_DERIVATION_PATH`].
    ///
    /// Surrounding and repeated whitespace and capital letters are tolerated.
    pub fn from_seed_phrase(seed_phrase: &str) -> Result<Self, TronKeyError> {
        let words: Zeroizing<Vec<String>> = Zeroizing::new(
            seed_phrase
                .split_whitespace()
                .map(str::to_lowercase)
                .collect(),
        );

        let phrase = Zeroizing::new(words.join(" "));

        let mnemonic = bip39::Mnemonic::parse_in_normalized(bip39::Language::English, &phrase)
            .map_err(|err| match err {
                bip39::Error::BadWordCount(count) => TronKeyError::SeedPhraseWordCount(count),
                other => TronKeyError::SeedPhraseInvalid(other.to_string()),
            })?;

        let seed = Zeroizing::new(mnemonic.to_seed(""));

        let path: bip32::DerivationPath = TRON_DERIVATION_PATH
            .parse()
            .expect("the TRON derivation path constant is well-formed");

        let extended_key = bip32::XPrv::derive_from_path(seed.as_slice(), &path)
            .map_err(|err| TronKeyError::Derivation(err.to_string()))?;

        Ok(Self::from_signing_key(extended_key.private_key().clone()))
    }

    fn from_signing_key(signing_key: SigningKey) -> Self {
        let address = address_from_verifying_key(signing_key.verifying_key());

        Self {
            signing_key,
            address,
        }
    }

    /// The base58check wallet address (`T…`).
    pub fn address(&self) -> &str {
        &self.address
    }

    /// Signs `message` the way TronLink's `signMessageV2` does (TIP-191).
    ///
    /// Returns the signature as `0x`-prefixed hex of `r || s || v`, `v` being 27 or 28, which
    /// is the format TronWeb produces and the homeserver expects.
    pub fn sign_message_v2(&self, message: &str) -> Result<String, TronKeyError> {
        let digest = message_v2_digest(message);

        let (signature, recovery_id) = self
            .signing_key
            .sign_prehash_recoverable(&digest)
            .map_err(|err| TronKeyError::Signing(err.to_string()))?;

        let mut bytes = signature.to_vec();
        bytes.push(recovery_id.to_byte() + 27);

        Ok(format!("0x{}", hex::encode(bytes)))
    }
}

impl fmt::Debug for TronSigner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TronSigner")
            .field("address", &self.address)
            .finish_non_exhaustive()
    }
}

/// The TIP-191 digest of a message: `keccak256("\x19TRON Signed Message:\n" || len || message)`,
/// `len` being the decimal byte length of the UTF-8 message.
fn message_v2_digest(message: &str) -> [u8; 32] {
    let mut hasher = Keccak256::new();
    hasher.update(TRON_MESSAGE_PREFIX.as_bytes());
    hasher.update(message.len().to_string().as_bytes());
    hasher.update(message.as_bytes());
    hasher.finalize().into()
}

fn address_from_verifying_key(verifying_key: &VerifyingKey) -> String {
    let point = verifying_key.to_encoded_point(false);
    // Drop the 0x04 tag of the uncompressed encoding: 64 bytes of `x || y`.
    let public_key = &point.as_bytes()[1..];

    let hash = Keccak256::digest(public_key);

    let mut payload = [0u8; 21];
    payload[0] = TRON_ADDRESS_PREFIX;
    payload[1..].copy_from_slice(&hash[12..]);

    bs58::encode(payload).with_check().into_string()
}

#[cfg(test)]
mod tests {
    use k256::ecdsa::{RecoveryId, Signature};

    use super::*;

    // The standard BIP-39 test mnemonic and its first TRON account (SLIP-44 coin type 195).
    // Address and private key cross-checked with the homeserver's own secp256k1 / keccak /
    // base58 primitives (`synapse-ever`, `tron_auth_utils.py`) and a stand-alone BIP-32
    // derivation, see the plan in the workspace root.
    const SEED_PHRASE: &str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
    const PRIVATE_KEY: &str = "b5a4cea271ff424d7c31dc12a3e43e401df7a40d7412a15750f3f0b6b5449a28";
    const ADDRESS: &str = "TUEZSdKsoDHQMeZwihtdoBiN46zxhGWYdH";

    fn recover_address(message: &str, signature_hex: &str) -> String {
        let bytes = hex::decode(signature_hex.strip_prefix("0x").unwrap()).unwrap();
        assert_eq!(bytes.len(), 65);

        let signature = Signature::from_slice(&bytes[..64]).unwrap();
        let recovery_id = RecoveryId::from_byte(bytes[64] - 27).unwrap();

        let verifying_key = VerifyingKey::recover_from_prehash(
            &message_v2_digest(message),
            &signature,
            recovery_id,
        )
        .unwrap();

        address_from_verifying_key(&verifying_key)
    }

    #[test]
    fn private_key_yields_the_expected_address() {
        let signer = TronSigner::from_private_key_hex(PRIVATE_KEY).unwrap();
        assert_eq!(signer.address(), ADDRESS);
    }

    #[test]
    fn private_key_accepts_prefix_case_and_whitespace() {
        let variants = [
            format!("0x{PRIVATE_KEY}"),
            format!("0X{}", PRIVATE_KEY.to_uppercase()),
            format!("  {PRIVATE_KEY}\n"),
        ];

        for variant in variants {
            let signer = TronSigner::from_private_key_hex(&variant).unwrap();
            assert_eq!(signer.address(), ADDRESS, "variant {variant:?}");
        }
    }

    #[test]
    fn private_key_rejects_bad_input() {
        let too_short = &PRIVATE_KEY[..62];
        assert!(matches!(
            TronSigner::from_private_key_hex(too_short),
            Err(TronKeyError::PrivateKeyFormat)
        ));

        let not_hex = format!("zz{}", &PRIVATE_KEY[2..]);
        assert!(matches!(
            TronSigner::from_private_key_hex(&not_hex),
            Err(TronKeyError::PrivateKeyFormat)
        ));

        let zero = "0".repeat(64);
        assert!(matches!(
            TronSigner::from_private_key_hex(&zero),
            Err(TronKeyError::PrivateKeyValue)
        ));
    }

    #[test]
    fn seed_phrase_yields_the_same_key_as_the_chums_wallet() {
        let signer = TronSigner::from_seed_phrase(SEED_PHRASE).unwrap();
        assert_eq!(signer.address(), ADDRESS);
    }

    #[test]
    fn seed_phrase_tolerates_whitespace_and_case() {
        let messy = format!("  {}  ", SEED_PHRASE.to_uppercase().replace(' ', "\n\t "));
        let signer = TronSigner::from_seed_phrase(&messy).unwrap();
        assert_eq!(signer.address(), ADDRESS);
    }

    #[test]
    fn seed_phrase_rejects_bad_input() {
        let eleven_words = SEED_PHRASE.rsplit_once(' ').unwrap().0;
        assert!(matches!(
            TronSigner::from_seed_phrase(eleven_words),
            Err(TronKeyError::SeedPhraseWordCount(11))
        ));

        let bad_checksum = SEED_PHRASE.replace("about", "abandon");
        assert!(matches!(
            TronSigner::from_seed_phrase(&bad_checksum),
            Err(TronKeyError::SeedPhraseInvalid(_))
        ));

        let unknown_word = SEED_PHRASE.replace("about", "baibot");
        assert!(matches!(
            TronSigner::from_seed_phrase(&unknown_word),
            Err(TronKeyError::SeedPhraseInvalid(_))
        ));
    }

    #[test]
    fn signature_has_the_tronweb_shape_and_recovers_to_the_address() {
        let signer = TronSigner::from_private_key_hex(PRIVATE_KEY).unwrap();
        let message = r#"{"address":"TUEZSdKsoDHQMeZwihtdoBiN46zxhGWYdH","purpose":"login"}"#;

        let signature = signer.sign_message_v2(message).unwrap();

        assert_eq!(signature.len(), 2 + 130);
        assert!(signature.starts_with("0x"));
        let v = u8::from_str_radix(&signature[130..], 16).unwrap();
        assert!(v == 27 || v == 28, "v = {v}");

        assert_eq!(recover_address(message, &signature), ADDRESS);
    }

    #[test]
    fn digest_prefix_uses_the_byte_length_of_the_message() {
        // "привет" is 6 characters but 12 bytes; TronWeb prefixes the byte length.
        let message = "привет";
        assert_eq!(message.len(), 12);

        let expected: [u8; 32] = Keccak256::digest(
            b"\x19TRON Signed Message:\n12\xd0\xbf\xd1\x80\xd0\xb8\xd0\xb2\xd0\xb5\xd1\x82",
        )
        .into();
        assert_eq!(message_v2_digest(message), expected);

        let signer = TronSigner::from_private_key_hex(PRIVATE_KEY).unwrap();
        let signature = signer.sign_message_v2(message).unwrap();
        assert_eq!(recover_address(message, &signature), ADDRESS);
    }

    #[test]
    fn signature_matches_the_homeserver_verifier() {
        // Golden value: this exact signature was fed to `verify_tronlink_message_signature` of
        // `synapse-ever` (`tron_auth_utils.py`) together with the message and the address, and
        // accepted. RFC 6979 makes the signature deterministic, so it must not change.
        let signer = TronSigner::from_private_key_hex(PRIVATE_KEY).unwrap();
        let message = concat!(
            r#"{"address":"TUEZSdKsoDHQMeZwihtdoBiN46zxhGWYdH","challenge_id":"3f1c2a6e-0d5b-4e0a-9d3e-6c1b2a4f5e7d","#,
            r#""client_nonce":"c2FtcGxlLW5vbmNl","expires_at":"2026-09-16T12:00:00Z","homeserver":"tron.mx","#,
            r#""origin":"https://baibot.tron.mx","purpose":"login","type":"cc.chums.login.tron","uia_session":null,"#,
            r#""user_id":"@baibot:tron.mx","username":"baibot","version":1}"#
        );

        assert_eq!(
            signer.sign_message_v2(message).unwrap(),
            "0x01f7e0ce7f3c5155ab2e0f715e13c04563a9b2a70d313641fa54a1713649770a7a89d4a297e8e23ead01ca54e08eda1106809daea849e8fcdffc9488e1a69f6e1c"
        );
    }

    #[test]
    fn debug_output_does_not_contain_the_key() {
        let signer = TronSigner::from_private_key_hex(PRIVATE_KEY).unwrap();
        let debug = format!("{signer:?}");

        assert!(debug.contains(ADDRESS));
        assert!(!debug.contains(PRIVATE_KEY));
        assert!(!debug.contains(&PRIVATE_KEY[..16]));
    }
}
