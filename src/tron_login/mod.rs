//! Logging in through a TRON wallet.
//!
//! The Chums homeserver (Synapse with the `synapse-ever` modules) has password login switched
//! off and accepts the custom login type `cc.chums.login.tron` instead: the client asks
//! `POST /_ever/tron-auth/challenge` for a message, signs it with the wallet key the way
//! TronLink's `signMessageV2` does, and sends the signature along in
//! `POST /_matrix/client/v3/login`. The homeserver recovers the wallet address from the
//! signature and logs the client in as the Matrix user that wallet is bound to.
//!
//! - `key`: the key material and the signing;
//! - `challenge`: the homeserver's `/_ever/tron-auth/` endpoints;
//! - `proof`: the token carrying the signature;
//! - `matrix`: the login request itself;
//! - [`login`]: the whole sequence, as the bot runs it at start-up when it has no session yet.
//!
//! Enabled by the `tron-login` cargo feature (on by default). Configured by `user.tron`, see
//! docs/configuration/authentication.md.

mod challenge;
mod key;
mod matrix;
mod proof;

use mxlink::matrix_sdk::ruma::UserId;
use thiserror::Error;

pub use challenge::{ChallengeClient, ChallengeError, SIGNING_METHOD};
pub use key::{TRON_DERIVATION_PATH, TronKeyError, TronSigner};
pub use matrix::{LOGIN_TYPE, LoginSession, MatrixLoginError};

#[derive(Debug, Error)]
pub enum LoginError {
    #[error("{0}")]
    Challenge(#[from] ChallengeError),

    #[error("{0}")]
    Signing(#[from] TronKeyError),

    #[error("{0}")]
    Matrix(#[from] MatrixLoginError),

    #[error(
        "the wallet {address} is not bound to any user on the homeserver; bind it to {expected_user_id} first (from the Chums client, logged in as that user)"
    )]
    WalletNotBound {
        address: String,
        expected_user_id: String,
    },

    #[error(
        "the wallet {address} is bound to {bound_user_id}, not to {expected_user_id} (user.mxid_localpart); use that user's wallet or change the localpart"
    )]
    WalletBoundToAnotherUser {
        address: String,
        bound_user_id: String,
        expected_user_id: String,
    },

    #[error(
        "the homeserver signs challenges with `{0}`, this bot only knows `{SIGNING_METHOD}`; the bot needs updating"
    )]
    UnsupportedSigningMethod(String),

    #[error("the homeserver logged the bot in as {actual}, expected {expected}")]
    UnexpectedUser { actual: String, expected: String },
}

impl LoginError {
    /// What the operator can do about it, for the errors with a known cause.
    pub fn hint(&self) -> Option<&'static str> {
        match self {
            Self::Challenge(ChallengeError::BadStatus {
                status: 403, error, ..
            }) if error.contains("Origin") => Some(
                "the homeserver rejects the bot's origin: add the value of user.tron.origin to `tron_auth_allowed_origins` of the chums module in homeserver.yaml, or set user.tron.origin to a listed value",
            ),
            Self::Challenge(ChallengeError::BadStatus {
                status: 400, error, ..
            }) if error.contains("Username") => Some(
                "the wallet is not bound to a user and the localpart is taken: bind the wallet to the bot's account first",
            ),
            _ => None,
        }
    }
}

/// What [`login`] needs.
#[derive(Debug)]
pub struct LoginRequest<'a> {
    pub homeserver_url: &'a str,
    pub signer: &'a TronSigner,
    /// The bot's `user.mxid_localpart`.
    pub localpart: &'a str,
    /// The user the bot must end up logged in as. The homeserver decides the user from the
    /// wallet binding and ignores the localpart for a bound wallet, so this is checked.
    pub expected_user_id: &'a UserId,
    /// Reported in the challenge request; must be in the homeserver's allow-list.
    pub origin: &'a str,
    pub device_display_name: &'a str,
}

/// Logs in through the wallet: checks the wallet binding, asks for a challenge, signs it and
/// sends the login request. Returns the session for mxlink to restore.
pub async fn login(request: &LoginRequest<'_>) -> Result<LoginSession, LoginError> {
    let address = request.signer.address();
    let expected_user_id = request.expected_user_id.as_str();

    let client = ChallengeClient::new(request.homeserver_url)?;

    // Fail early with a clear reason. Without this check an unbound wallet would either be
    // refused by the challenge endpoint (localpart taken) or, worse, get a new account
    // registered under the localpart.
    let lookup = client.lookup(address).await?;
    match lookup.user_id.filter(|_| lookup.bound) {
        None => {
            return Err(LoginError::WalletNotBound {
                address: address.to_owned(),
                expected_user_id: expected_user_id.to_owned(),
            });
        }
        Some(bound_user_id) if bound_user_id != expected_user_id => {
            return Err(LoginError::WalletBoundToAnotherUser {
                address: address.to_owned(),
                bound_user_id,
                expected_user_id: expected_user_id.to_owned(),
            });
        }
        Some(_) => {}
    }

    let client_nonce = uuid::Uuid::now_v7().simple().to_string();

    let challenge = client
        .login_challenge(address, request.localpart, request.origin, &client_nonce)
        .await?;

    if challenge.signing_method != SIGNING_METHOD {
        return Err(LoginError::UnsupportedSigningMethod(
            challenge.signing_method,
        ));
    }

    let signature = request.signer.sign_message_v2(&challenge.message)?;

    let proof_token = proof::login_proof_token(
        &challenge.challenge_id,
        address,
        request.localpart,
        &signature,
    );

    let session = matrix::login(
        request.homeserver_url,
        request.localpart,
        &proof_token,
        request.device_display_name,
    )
    .await?;

    if session.user_id != request.expected_user_id {
        return Err(LoginError::UnexpectedUser {
            actual: session.user_id.to_string(),
            expected: expected_user_id.to_owned(),
        });
    }

    Ok(session)
}
