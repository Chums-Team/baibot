//! `POST /_matrix/client/v3/login` with the custom login type.
//!
//! A throw-away matrix-sdk client (in-memory stores) sends the request; only the resulting
//! session is kept. mxlink then builds the bot's real client from it, exactly as with a
//! pre-issued access token.

use mxlink::matrix_sdk::Client;
use mxlink::matrix_sdk::ruma::serde::JsonObject;
use mxlink::matrix_sdk::ruma::{OwnedDeviceId, OwnedUserId};
use serde_json::json;
use thiserror::Error;

/// The login type the Chums homeserver registers for TRON wallets.
pub const LOGIN_TYPE: &str = "cc.chums.login.tron";

#[derive(Debug, Error)]
pub enum MatrixLoginError {
    #[error("could not create a Matrix client for {homeserver_url}: {source}")]
    ClientBuild {
        homeserver_url: String,
        #[source]
        source: Box<mxlink::matrix_sdk::ClientBuildError>,
    },

    #[error("the login request failed: {0}")]
    Login(#[source] Box<mxlink::matrix_sdk::Error>),

    #[error("could not build the login request: {0}")]
    Request(#[source] serde_json::Error),
}

/// What the homeserver hands out on a successful login.
#[derive(Debug, Clone)]
pub struct LoginSession {
    pub user_id: OwnedUserId,
    pub device_id: OwnedDeviceId,
    pub access_token: String,
}

/// Logs in with the proof token, as user `localpart`.
///
/// The identifier is the standard `m.id.user` shape; the homeserver pre-validates it before the
/// TRON checker sees the request, then decides the user from the wallet binding.
pub async fn login(
    homeserver_url: &str,
    localpart: &str,
    proof_token: &str,
    device_display_name: &str,
) -> Result<LoginSession, MatrixLoginError> {
    let client = Client::builder()
        .homeserver_url(homeserver_url)
        .build()
        .await
        .map_err(|source| MatrixLoginError::ClientBuild {
            homeserver_url: homeserver_url.to_owned(),
            source: Box::new(source),
        })?;

    let mut data = JsonObject::new();
    data.insert(
        "identifier".to_owned(),
        json!({"type": "m.id.user", "user": localpart}),
    );
    data.insert("token".to_owned(), json!(proof_token));

    let response = client
        .matrix_auth()
        .login_custom(LOGIN_TYPE, data)
        .map_err(MatrixLoginError::Request)?
        .initial_device_display_name(device_display_name)
        .send()
        .await
        .map_err(|err| MatrixLoginError::Login(Box::new(err)))?;

    Ok(LoginSession {
        user_id: response.user_id,
        device_id: response.device_id,
        access_token: response.access_token,
    })
}
