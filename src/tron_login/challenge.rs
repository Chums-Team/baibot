//! The homeserver's TRON authentication endpoints under `/_ever/tron-auth/`
//! (`synapse-ever`, `modules/chums_module/users/tron_auth.py`).
//!
//! - `GET lookup?address=` says which Matrix user a wallet is bound to;
//! - `POST challenge` issues the message to sign for a login.
//!
//! Both are unauthenticated. Errors come back as Matrix-style `{"errcode", "error"}` bodies.

use std::time::Duration;

use reqwest::Client;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// What the homeserver calls the `signMessageV2` signing scheme.
pub const SIGNING_METHOD: &str = "tronlink-message-v2";

#[derive(Debug, Error)]
pub enum ChallengeError {
    #[error("HTTP transport error: {0}")]
    Transport(#[from] reqwest::Error),

    #[error("the homeserver answered HTTP {status}: {error}")]
    BadStatus {
        status: u16,
        errcode: Option<String>,
        error: String,
    },

    #[error("the homeserver's answer is not what the bot expects: {0}")]
    BadBody(String),
}

/// `GET /_ever/tron-auth/lookup`.
#[derive(Debug, Clone, Deserialize)]
pub struct Lookup {
    pub bound: bool,
    #[serde(default)]
    pub user_id: Option<String>,
}

/// `POST /_ever/tron-auth/challenge`.
#[derive(Debug, Clone, Deserialize)]
pub struct Challenge {
    pub challenge_id: String,
    /// The exact string to sign.
    pub message: String,
    pub signing_method: String,
    #[serde(default)]
    pub expires_at: Option<String>,
}

#[derive(Serialize)]
struct ChallengeRequest<'a> {
    purpose: &'static str,
    address: &'a str,
    username: &'a str,
    origin: &'a str,
    client_nonce: &'a str,
    /// Only set for UIA challenges; the homeserver expects the key to be present.
    uia_session: Option<&'a str>,
}

#[derive(Deserialize)]
struct ErrorBody {
    #[serde(default)]
    errcode: Option<String>,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ChallengeClient {
    http: Client,
    base_url: String,
}

impl ChallengeClient {
    pub fn new(homeserver_url: &str) -> Result<Self, ChallengeError> {
        let http = Client::builder()
            .timeout(Duration::from_secs(30))
            .connect_timeout(Duration::from_secs(10))
            .build()?;

        Ok(Self {
            http,
            base_url: homeserver_url.trim_end_matches('/').to_owned(),
        })
    }

    /// Which Matrix user `address` is bound to, if any.
    pub async fn lookup(&self, address: &str) -> Result<Lookup, ChallengeError> {
        let url = format!("{}/_ever/tron-auth/lookup", self.base_url);

        let response = self
            .http
            .get(&url)
            .query(&[("address", address)])
            .send()
            .await?;

        Self::parse(response).await
    }

    /// A login challenge for `address`. `username` is the bot's localpart; the homeserver only
    /// consults it when the wallet is not bound to a user yet.
    pub async fn login_challenge(
        &self,
        address: &str,
        username: &str,
        origin: &str,
        client_nonce: &str,
    ) -> Result<Challenge, ChallengeError> {
        let url = format!("{}/_ever/tron-auth/challenge", self.base_url);

        let response = self
            .http
            .post(&url)
            .json(&ChallengeRequest {
                purpose: "login",
                address,
                username,
                origin,
                client_nonce,
                uia_session: None,
            })
            .send()
            .await?;

        Self::parse(response).await
    }

    async fn parse<T: serde::de::DeserializeOwned>(
        response: reqwest::Response,
    ) -> Result<T, ChallengeError> {
        let status = response.status();
        let body = response.text().await?;

        if !status.is_success() {
            let parsed: Option<ErrorBody> = serde_json::from_str(&body).ok();
            let (errcode, error) = match parsed {
                Some(ErrorBody { errcode, error }) => (errcode, error),
                None => (None, None),
            };

            return Err(ChallengeError::BadStatus {
                status: status.as_u16(),
                errcode,
                error: error.unwrap_or(body),
            });
        }

        serde_json::from_str(&body).map_err(|err| ChallengeError::BadBody(err.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;
    use std::sync::{Arc, Mutex};

    use axum::extract::{RawQuery, State};
    use axum::http::StatusCode;
    use axum::routing::{get, post};
    use axum::{Json, Router};
    use serde_json::{Value, json};

    use super::*;

    #[derive(Clone, Default)]
    struct Recorded {
        challenge_bodies: Arc<Mutex<Vec<Value>>>,
    }

    async fn lookup_handler(RawQuery(query): RawQuery) -> (StatusCode, Json<Value>) {
        let query = query.unwrap_or_default();
        let address = query
            .split('&')
            .find_map(|pair| pair.strip_prefix("address="))
            .unwrap_or_default();

        if address == "TUEZSdKsoDHQMeZwihtdoBiN46zxhGWYdH" {
            (
                StatusCode::OK,
                Json(json!({"bound": true, "user_id": "@baibot:example.com"})),
            )
        } else if address.starts_with('T') {
            (
                StatusCode::OK,
                Json(json!({"bound": false, "user_id": null})),
            )
        } else {
            (
                StatusCode::BAD_REQUEST,
                Json(json!({"errcode": "M_INVALID_PARAM", "error": "Invalid Tron address"})),
            )
        }
    }

    async fn challenge_handler(
        State(recorded): State<Recorded>,
        Json(body): Json<Value>,
    ) -> (StatusCode, Json<Value>) {
        recorded.challenge_bodies.lock().unwrap().push(body.clone());

        if body["origin"] != "https://allowed.example" {
            return (
                StatusCode::FORBIDDEN,
                Json(json!({"errcode": "M_FORBIDDEN", "error": "Origin is not allowed"})),
            );
        }

        (
            StatusCode::OK,
            Json(json!({
                "challenge_id": "3f1c2a6e-0d5b-4e0a-9d3e-6c1b2a4f5e7d",
                "expires_at": "2026-09-16T12:00:00Z",
                "message": "{\"purpose\":\"login\"}",
                "signing_method": SIGNING_METHOD,
            })),
        )
    }

    async fn serve(recorded: Recorded) -> String {
        let app = Router::new()
            .route("/_ever/tron-auth/lookup", get(lookup_handler))
            .route("/_ever/tron-auth/challenge", post(challenge_handler))
            .with_state(recorded);

        let listener = tokio::net::TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();

        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        // A trailing slash must not end up doubled in the request paths.
        format!("http://{address}/")
    }

    #[tokio::test]
    async fn lookup_reports_the_binding() {
        let client = ChallengeClient::new(&serve(Recorded::default()).await).unwrap();

        let bound = client
            .lookup("TUEZSdKsoDHQMeZwihtdoBiN46zxhGWYdH")
            .await
            .unwrap();
        assert!(bound.bound);
        assert_eq!(bound.user_id.as_deref(), Some("@baibot:example.com"));

        let unbound = client
            .lookup("T9yD14Nj9j7xAB4dbGeiX9h8unkKHxuWwb")
            .await
            .unwrap();
        assert!(!unbound.bound);
        assert_eq!(unbound.user_id, None);
    }

    #[tokio::test]
    async fn errors_carry_the_homeserver_message() {
        let client = ChallengeClient::new(&serve(Recorded::default()).await).unwrap();

        let err = client.lookup("not-an-address").await.unwrap_err();

        match err {
            ChallengeError::BadStatus {
                status,
                errcode,
                error,
            } => {
                assert_eq!(status, 400);
                assert_eq!(errcode.as_deref(), Some("M_INVALID_PARAM"));
                assert_eq!(error, "Invalid Tron address");
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[tokio::test]
    async fn challenge_sends_the_login_shape_and_parses_the_answer() {
        let recorded = Recorded::default();
        let client = ChallengeClient::new(&serve(recorded.clone()).await).unwrap();

        let challenge = client
            .login_challenge(
                "TUEZSdKsoDHQMeZwihtdoBiN46zxhGWYdH",
                "baibot",
                "https://allowed.example",
                "nonce-1",
            )
            .await
            .unwrap();

        assert_eq!(
            challenge.challenge_id,
            "3f1c2a6e-0d5b-4e0a-9d3e-6c1b2a4f5e7d"
        );
        assert_eq!(challenge.message, r#"{"purpose":"login"}"#);
        assert_eq!(challenge.signing_method, SIGNING_METHOD);
        assert_eq!(
            challenge.expires_at.as_deref(),
            Some("2026-09-16T12:00:00Z")
        );

        let bodies = recorded.challenge_bodies.lock().unwrap();
        assert_eq!(
            bodies[0],
            json!({
                "purpose": "login",
                "address": "TUEZSdKsoDHQMeZwihtdoBiN46zxhGWYdH",
                "username": "baibot",
                "origin": "https://allowed.example",
                "client_nonce": "nonce-1",
                "uia_session": null,
            })
        );
    }

    #[tokio::test]
    async fn challenge_rejected_origin_is_reported() {
        let client = ChallengeClient::new(&serve(Recorded::default()).await).unwrap();

        let err = client
            .login_challenge(
                "TUEZSdKsoDHQMeZwihtdoBiN46zxhGWYdH",
                "baibot",
                "https://other.example",
                "n",
            )
            .await
            .unwrap_err();

        assert!(matches!(err, ChallengeError::BadStatus { status: 403, .. }));
        assert!(err.to_string().contains("Origin is not allowed"));
    }
}
