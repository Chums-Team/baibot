//! HTTP client for the bot → sidecar direction.
//!
//! When a room cannot afford the next call (or a user runs the `topup` command), the bot
//! calls `POST {sidecar_url}/payment-request` and forwards the answer to the Chums client as a
//! `cc.chums.x402_request` event, so the user can sign the payment in their wallet.
//!
//! The wire shape mirrors `PaymentRequestIn` / `PaymentRequestOut` in
//! `x402-sidecar/src/x402_sidecar/models.py`. The two sides must stay in sync: the
//! sidecar rejects unknown request fields.

use std::time::Duration;

use chrono::{DateTime, Utc};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use uuid::Uuid;

use crate::matrix::events::X402RequestContent;

#[derive(Clone, Debug)]
pub struct X402Client {
    http: Client,
    sidecar_url: String,
}

#[derive(Debug, Error)]
pub enum X402ClientError {
    #[error("HTTP transport error: {0}")]
    Transport(#[from] reqwest::Error),
    #[error("sidecar returned HTTP {status}: {body}")]
    BadStatus { status: u16, body: String },
    #[error("invalid sidecar URL `{url}`: {reason}")]
    BadUrl { url: String, reason: String },
}

#[derive(Debug, Serialize)]
pub struct PaymentRequest {
    pub room_id: String,
    pub user_mxid: String,
    pub amount_usd: f64,
    /// Echoed back by the sidecar in the settlement notification, so a top-up can be tied to
    /// the LLM call that triggered it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<Uuid>,
}

/// The fields of `PaymentRequestOut` the bot forwards to the client. The sidecar sends more
/// (e.g. `facilitator_url`); unknown fields are ignored.
#[derive(Debug, Clone, Deserialize)]
pub struct PaymentResponse {
    pub payment_id: Uuid,
    pub expires_at: DateTime<Utc>,
    /// TIP-712 typed data the user signs in their wallet. Opaque to the bot: forwarded as
    /// `permit_payload` of the `cc.chums.x402_request` event.
    pub permit_payload: Value,
    pub permit_contract: String,
    pub network: String,
    pub token: String,
    pub pay_to: String,
    /// The sidecar's facilitator readiness verdict. Absent on sidecars that predate it.
    #[serde(default)]
    pub facilitator_ok: Option<bool>,
    #[serde(default)]
    pub facilitator_reason: Option<String>,
}

impl X402Client {
    /// The sidecar runs next to the bot and answers in well under a second; the timeouts only
    /// keep a stuck sidecar from blocking a chat reply forever.
    pub fn new(sidecar_url: impl Into<String>) -> Result<Self, X402ClientError> {
        let sidecar_url = sidecar_url.into();
        if sidecar_url.is_empty() {
            return Err(X402ClientError::BadUrl {
                url: sidecar_url,
                reason: "x402.sidecar_url is empty".into(),
            });
        }
        let trimmed = sidecar_url.trim_end_matches('/').to_string();
        let http = Client::builder()
            .timeout(Duration::from_secs(10))
            .connect_timeout(Duration::from_secs(3))
            .build()?;
        Ok(Self {
            http,
            sidecar_url: trimmed,
        })
    }

    /// `POST /payment-request`: asks the sidecar to prepare a payment the user can sign.
    pub async fn create_payment_request(
        &self,
        req: &PaymentRequest,
    ) -> Result<PaymentResponse, X402ClientError> {
        let url = format!("{}/payment-request", self.sidecar_url);
        let resp = self.http.post(&url).json(req).send().await?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(X402ClientError::BadStatus {
                status: status.as_u16(),
                body,
            });
        }
        let parsed = resp.json::<PaymentResponse>().await?;
        Ok(parsed)
    }

    /// Asks the sidecar for a payment of `amount_usd` by `user_mxid` into `room_id` and shapes
    /// the answer as the content of a `cc.chums.x402_request` event. The caller sends it.
    pub async fn create_x402_request_content(
        &self,
        room_id: &str,
        user_mxid: &str,
        amount_usd: f64,
        min_topup_usd: f64,
        command_prefix: &str,
    ) -> Result<X402RequestContent, X402ClientError> {
        let payment = self
            .create_payment_request(&PaymentRequest {
                room_id: room_id.to_owned(),
                user_mxid: user_mxid.to_owned(),
                amount_usd,
                correlation_id: None,
            })
            .await?;

        Ok(X402RequestContent {
            room_id: room_id.to_owned(),
            amount_required_usd: amount_usd,
            min_topup_usd,
            agent_wallet_tron: payment.pay_to,
            permit_contract: payment.permit_contract,
            network: payment.network,
            token: payment.token,
            payment_id: payment.payment_id,
            permit_payload: payment.permit_payload,
            expires_at: payment.expires_at.to_rfc3339(),
            command_prefix: command_prefix.to_owned(),
            facilitator_ok: payment.facilitator_ok,
            facilitator_reason: payment.facilitator_reason,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Json, Router, extract::State, http::StatusCode, routing::post};
    use serde_json::json;
    use std::net::SocketAddr;
    use std::sync::{Arc, Mutex};
    use tokio::net::TcpListener;

    /// A mock sidecar that answers `/payment-request` with a fixed response and records the
    /// request bodies it received.
    async fn spawn_mock_sidecar(captured: Arc<Mutex<Vec<Value>>>) -> SocketAddr {
        async fn handler(
            State(captured): State<Arc<Mutex<Vec<Value>>>>,
            Json(body): Json<Value>,
        ) -> (StatusCode, Json<Value>) {
            captured.lock().unwrap().push(body);
            let resp = json!({
                "payment_id": "0192ab99-a000-7000-8000-000000000001",
                "expires_at": "2030-01-01T00:00:00Z",
                "permit_payload": {
                    "primaryType": "PermitWitnessTransferFrom",
                    "domain": {"name": "Permit2"},
                    "message": {"permitted": {"amount": "100000"}},
                    "types": {"PermitWitnessTransferFrom": []},
                },
                "facilitator_url": "http://test.facilitator",
                "permit_contract": "TFxDcGvS7zfQrS1YzcCMp673ta2NHHzsiH",
                "network": "tron:0xcd8690dc",
                "token": "USDT",
                "pay_to": "TSkeaPMSuaojcCzE7mWqw4xN1awU7NfdfY",
                "facilitator_ok": false,
                "facilitator_reason": "facilitator unreachable",
            });
            (StatusCode::OK, Json(resp))
        }

        let app = Router::new()
            .route("/payment-request", post(handler))
            .with_state(captured);
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        addr
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn create_payment_request_roundtrip() {
        let captured: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
        let addr = spawn_mock_sidecar(captured.clone()).await;

        let client = X402Client::new(format!("http://{addr}/")).unwrap();
        let req = PaymentRequest {
            room_id: "!r:example.com".into(),
            user_mxid: "@u:example.com".into(),
            amount_usd: 0.10,
            correlation_id: Some(Uuid::nil()),
        };
        let resp = client.create_payment_request(&req).await.unwrap();

        assert_eq!(resp.permit_contract, "TFxDcGvS7zfQrS1YzcCMp673ta2NHHzsiH");
        assert_eq!(resp.network, "tron:0xcd8690dc");
        assert_eq!(resp.token, "USDT");
        assert_eq!(resp.pay_to, "TSkeaPMSuaojcCzE7mWqw4xN1awU7NfdfY");
        assert_eq!(
            resp.permit_payload["primaryType"],
            "PermitWitnessTransferFrom"
        );
        assert_eq!(resp.facilitator_ok, Some(false));
        assert_eq!(
            resp.facilitator_reason.as_deref(),
            Some("facilitator unreachable")
        );

        // The request body matches the sidecar's `PaymentRequestIn`
        // (room_id, user_mxid, amount_usd, correlation_id and nothing else).
        let captured = captured.lock().unwrap().clone();
        assert_eq!(captured.len(), 1);
        let sent = &captured[0];
        assert_eq!(sent["room_id"], "!r:example.com");
        assert_eq!(sent["user_mxid"], "@u:example.com");
        assert!((sent["amount_usd"].as_f64().unwrap() - 0.10).abs() < 1e-12);
        assert!(sent.get("correlation_id").is_some());
        assert_eq!(sent.as_object().unwrap().len(), 4);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn request_content_forwards_the_sidecar_answer() {
        let captured: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
        let addr = spawn_mock_sidecar(captured.clone()).await;

        let client = X402Client::new(format!("http://{addr}")).unwrap();
        let content = client
            .create_x402_request_content("!r:example.com", "@u:example.com", 0.25, 0.10, "!bai")
            .await
            .unwrap();

        assert_eq!(content.room_id, "!r:example.com");
        assert!((content.amount_required_usd - 0.25).abs() < 1e-12);
        assert!((content.min_topup_usd - 0.10).abs() < 1e-12);
        assert_eq!(
            content.agent_wallet_tron,
            "TSkeaPMSuaojcCzE7mWqw4xN1awU7NfdfY"
        );
        assert_eq!(
            content.permit_contract,
            "TFxDcGvS7zfQrS1YzcCMp673ta2NHHzsiH"
        );
        assert_eq!(content.network, "tron:0xcd8690dc");
        assert_eq!(content.token, "USDT");
        assert_eq!(
            content.payment_id.to_string(),
            "0192ab99-a000-7000-8000-000000000001"
        );
        assert_eq!(content.expires_at, "2030-01-01T00:00:00+00:00");
        assert_eq!(content.command_prefix, "!bai");
        assert_eq!(content.facilitator_ok, Some(false));
        assert_eq!(
            content.facilitator_reason.as_deref(),
            Some("facilitator unreachable")
        );

        // A correlation id is not sent: at this point there is no reserve to tie the payment to.
        let sent = &captured.lock().unwrap()[0];
        assert!(sent.get("correlation_id").is_none());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn older_sidecar_without_facilitator_fields_still_parses() {
        async fn handler() -> (StatusCode, Json<Value>) {
            (
                StatusCode::OK,
                Json(json!({
                    "payment_id": "0192ab99-a000-7000-8000-000000000002",
                    "expires_at": "2030-01-01T00:00:00Z",
                    "permit_payload": {},
                    "facilitator_url": "http://test.facilitator",
                    "permit_contract": "TFxDcGvS7zfQrS1YzcCMp673ta2NHHzsiH",
                    "network": "tron:0xcd8690dc",
                    "token": "USDT",
                    "pay_to": "TSkeaPMSuaojcCzE7mWqw4xN1awU7NfdfY",
                })),
            )
        }
        let app = Router::new().route("/payment-request", post(handler));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        let client = X402Client::new(format!("http://{addr}")).unwrap();
        let resp = client
            .create_payment_request(&PaymentRequest {
                room_id: "!r:example.com".into(),
                user_mxid: "@u:example.com".into(),
                amount_usd: 0.10,
                correlation_id: None,
            })
            .await
            .unwrap();
        assert_eq!(resp.facilitator_ok, None);
        assert_eq!(resp.facilitator_reason, None);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn http_error_propagates() {
        async fn err_handler() -> (StatusCode, &'static str) {
            (StatusCode::BAD_REQUEST, "amount_usd must be > 0")
        }
        let app = Router::new().route("/payment-request", post(err_handler));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        let client = X402Client::new(format!("http://{addr}")).unwrap();
        let req = PaymentRequest {
            room_id: "!r:example.com".into(),
            user_mxid: "@u:example.com".into(),
            amount_usd: 0.10,
            correlation_id: None,
        };
        let err = client.create_payment_request(&req).await.unwrap_err();
        match err {
            X402ClientError::BadStatus { status, body } => {
                assert_eq!(status, 400);
                assert!(body.contains("amount_usd"));
            }
            other => panic!("expected BadStatus, got {other:?}"),
        }
    }

    #[test]
    fn rejects_empty_url() {
        let err = X402Client::new("").unwrap_err().to_string();
        assert!(err.contains("sidecar_url"));
    }

    #[test]
    fn trims_trailing_slash() {
        let c = X402Client::new("http://localhost:8402//").unwrap();
        assert_eq!(c.sidecar_url, "http://localhost:8402");
    }
}
