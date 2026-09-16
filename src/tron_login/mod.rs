//! Logging in through a TRON wallet.
//!
//! The Chums homeserver (Synapse with the `synapse-ever` modules) has password login switched
//! off and accepts the custom login type `cc.chums.login.tron` instead: the client asks
//! `POST /_ever/tron-auth/challenge` for a message, signs it with the wallet key the way
//! TronLink's `signMessageV2` does, and sends the signature along in
//! `POST /_matrix/client/v3/login`. The homeserver recovers the wallet address from the
//! signature and logs the client in as the Matrix user that wallet is bound to.
//!
//! `key` holds the key material and the signing; the challenge and login requests follow.
//!
//! Enabled by the `tron-login` cargo feature (on by default).

mod key;

pub use key::{TRON_DERIVATION_PATH, TronKeyError, TronSigner};
