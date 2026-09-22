//! HTTP client for the bot → sidecar direction.
//!
//! When a room cannot afford the next call (or a user runs the `topup` command), the bot
//! calls `POST {sidecar_url}/payment-request` and forwards the answer to the Chums client as a
//! `cc.chums.x402_request` event, so the user can sign the payment in their wallet.
//!
//! Two more calls serve the payment the user signs: `GET /status/{payment_id}` tells the bot
//! whose payment it is and in which room, and `POST /payment-request/submit` hands the sidecar
//! the signature that arrived as a `cc.chums.x402_submit` event. Both are used by the handler in
//! `src/bot/x402.rs`.
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


/// The fields of `PaymentStatusOut` (`GET /status/{payment_id}`) the bot needs to decide whether
/// a submitted signature may be forwarded: whose payment it is, in which room, and whether it is
/// still payable.
#[derive(Debug, Clone, Deserialize)]
pub struct PaymentStatus {
    /// `pending` | `settled` | `expired` | `failed`.
    pub status: String,
    pub room_id: String,
    pub user_mxid: String,
    #[serde(default)]
    pub tx_hash: Option<String>,
}

/// `POST /payment-request/submit` body, mirroring `PaymentSubmitIn` of the sidecar.
#[derive(Debug, Serialize)]
pub struct PaymentSubmit {
    pub payment_id: Uuid,
    pub buyer_address: String,
    pub signature_hex: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permit_payload: Option<Value>,
}

/// What the sidecar answered to a submit. Any HTTP status is an outcome, not an error: only a
/// transport failure produces `Err`, so the caller can map a status to an error code for the
/// client.
#[derive(Debug, Clone)]
pub struct SubmitOutcome {
    pub status: u16,
    /// `next_step` of `PaymentSubmitOut`: `settled`, `already-settled`, `awaiting-live-wiring`,
    /// `verify-failed`, `settle-failed`. Absent when the sidecar answered with a plain detail.
    pub next_step: Option<String>,
    pub tx_hash: Option<String>,
    /// `error_reason` / `error_message` of `PaymentSubmitOut`, or FastAPI's `detail`.
    pub detail: Option<String>,
}

impl SubmitOutcome {
    /// The sidecar took the payment: it is settled, or it was already settled before.
    pub fn is_accepted(&self) -> bool {
        (200..300).contains(&self.status)
    }
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

    /// `GET /status/{payment_id}`. `Ok(None)` means the sidecar does not know this payment,
    /// which is the normal answer for a payment issued by another bot.
    pub async fn payment_status(
        &self,
        payment_id: Uuid,
    ) -> Result<Option<PaymentStatus>, X402ClientError> {
        let url = format!("{}/status/{}", self.sidecar_url, payment_id);
        let resp = self.http.get(&url).send().await?;
        let status = resp.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(X402ClientError::BadStatus {
                status: status.as_u16(),
                body,
            });
        }
        Ok(Some(resp.json::<PaymentStatus>().await?))
    }

    /// `POST /payment-request/submit`: hands the sidecar the signature the payer produced.
    /// The settlement itself is announced later through the webhook, not in this answer.
    pub async fn submit_signed_permit(
        &self,
        req: &PaymentSubmit,
    ) -> Result<SubmitOutcome, X402ClientError> {
        let url = format!("{}/payment-request/submit", self.sidecar_url);
        let resp = self.http.post(&url).json(req).send().await?;
        let status = resp.status().as_u16();
        let body = resp.text().await.unwrap_or_default();
        let value: Option<Value> = serde_json::from_str(&body).ok();

        let (next_step, tx_hash, detail) = match value {
            Some(v) => {
                let text = |key: &str| {
                    v.get(key)
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                        .filter(|s| !s.is_empty())
                };
                let detail = text("error_reason")
                    .map(|reason| match text("error_message") {
                        Some(message) => format!("{reason}: {message}"),
                        None => reason,
                    })
                    .or_else(|| text("detail"));
                (text("next_step"), text("tx_hash"), detail)
            }
            None => (
                None,
                None,
                Some(body.chars().take(200).collect::<String>()).filter(|s| !s.is_empty()),
            ),
        };

        Ok(SubmitOutcome {
            status,
            next_step,
            tx_hash,
            detail,
        })
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
    use axum::{
        Json, Router,
        extract::{Path, State},
        http::StatusCode,
        routing::{get, post},
    };
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

        // `/status/{id}`: a known payment for `@u:example.com` in `!r:example.com`, anything
        // else unknown. `/payment-request/submit`: the answer is driven by the signature, so a
        // test can ask for a refusal without a second mock.
        async fn status(Path(payment_id): Path<String>) -> (StatusCode, Json<Value>) {
            if payment_id == "0192ab99-a000-7000-8000-000000000001" {
                return (
                    StatusCode::OK,
                    Json(json!({
                        "payment_id": payment_id,
                        "status": "pending",
                        "room_id": "!r:example.com",
                        "user_mxid": "@u:example.com",
                        "amount_usd": 0.10,
                        "expires_at": "2030-01-01T00:00:00Z",
                        "created_at": "2020-01-01T00:00:00Z",
                    })),
                );
            }
            (
                StatusCode::NOT_FOUND,
                Json(json!({ "detail": "unknown payment_id" })),
            )
        }

        async fn submit(Json(body): Json<Value>) -> (StatusCode, Json<Value>) {
            match body["signature_hex"].as_str().unwrap_or_default() {
                "0xrefused" => (
                    StatusCode::PAYMENT_REQUIRED,
                    Json(json!({
                        "accepted": false,
                        "payment_id": body["payment_id"],
                        "next_step": "verify-failed",
                        "error_reason": "insufficient_allowance",
                        "error_message": "approve more USDT",
                    })),
                ),
                "0xexpired" => (
                    StatusCode::GONE,
                    Json(json!({ "detail": "payment_id expired — request a fresh permit" })),
                ),
                _ => (
                    StatusCode::OK,
                    Json(json!({
                        "accepted": true,
                        "payment_id": body["payment_id"],
                        "next_step": "settled",
                        "tx_hash": "0xfeed",
                    })),
                ),
            }
        }

        let app = Router::new()
            .route("/payment-request", post(handler))
            .route("/payment-request/submit", post(submit))
            .route("/status/:payment_id", get(status))
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

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn payment_status_tells_owner_and_room_and_maps_404_to_none() {
        let addr = spawn_mock_sidecar(Arc::new(Mutex::new(Vec::new()))).await;
        let client = X402Client::new(format!("http://{addr}")).unwrap();

        let known = Uuid::parse_str("0192ab99-a000-7000-8000-000000000001").unwrap();
        let status = client.payment_status(known).await.unwrap().unwrap();
        assert_eq!(status.status, "pending");
        assert_eq!(status.room_id, "!r:example.com");
        assert_eq!(status.user_mxid, "@u:example.com");

        // A payment of another bot is "not found", not an error: the handler stays quiet
        // instead of failing the event.
        assert!(client.payment_status(Uuid::nil()).await.unwrap().is_none());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn submit_signed_permit_reports_every_answer_as_an_outcome() {
        let addr = spawn_mock_sidecar(Arc::new(Mutex::new(Vec::new()))).await;
        let client = X402Client::new(format!("http://{addr}")).unwrap();
        let payment_id = Uuid::parse_str("0192ab99-a000-7000-8000-000000000001").unwrap();

        let submit = |signature: &str| PaymentSubmit {
            payment_id,
            buyer_address: "TSkeaPMSuaojcCzE7mWqw4xN1awU7NfdfY".into(),
            signature_hex: signature.into(),
            permit_payload: None,
        };

        let ok = client.submit_signed_permit(&submit("0xdeadbeef")).await.unwrap();
        assert!(ok.is_accepted());
        assert_eq!(ok.next_step.as_deref(), Some("settled"));
        assert_eq!(ok.tx_hash.as_deref(), Some("0xfeed"));

        // 402: the facilitator refused. Both halves of the reason reach the client.
        let refused = client.submit_signed_permit(&submit("0xrefused")).await.unwrap();
        assert!(!refused.is_accepted());
        assert_eq!(refused.status, 402);
        assert_eq!(
            refused.detail.as_deref(),
            Some("insufficient_allowance: approve more USDT")
        );

        // 410: FastAPI's plain `detail` is carried through as well.
        let expired = client.submit_signed_permit(&submit("0xexpired")).await.unwrap();
        assert_eq!(expired.status, 410);
        assert!(expired.detail.unwrap().contains("expired"));
    }

}
