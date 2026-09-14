//! The HMAC-authenticated `POST /internal/x402-settled` handler.
//!
//! The sidecar calls it once a payment settled on-chain. The handler verifies the signature,
//! writes a `topup` row into the ledger and pushes a `TopupAnnouncement` for the bot to post
//! into the room. It is plain HTTP with no Matrix dependency, so it is tested with `oneshot`
//! requests against the router.
//!
//! Idempotency: a `payment_id` is credited at most once. A repeated notification gets
//! HTTP 409, both when the pre-insert lookup finds it and when two notifications race and
//! the ledger's unique index rejects the second insert.

use std::sync::Arc;

use axum::{
    Router,
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
};
use hmac::{Hmac, Mac};
use serde_json::json;
use sha2::Sha256;

use crate::billing::{BillingError, BillingService};

use super::types::InternalSettledNotify;

/// What the bot posts into the room after a top-up landed in the ledger: a
/// `cc.chums.x402_topup_confirmed` event plus a plain text confirmation.
#[derive(Debug, Clone)]
pub struct TopupAnnouncement {
    pub room_id: String,
    pub amount_usd: f64,
    pub tx_hash: String,
    pub new_balance_usd: f64,
    pub credited_to_user: String,
    pub payment_id: uuid::Uuid,
}

#[derive(Clone)]
pub struct WebhookState {
    pub billing: BillingService,
    pub internal_secret: String,
    /// `None` in unit tests. At runtime the bot drains the channel and posts into Matrix.
    pub announce_tx: Option<tokio::sync::mpsc::UnboundedSender<TopupAnnouncement>>,
}

/// The router with the single `/internal/x402-settled` route.
pub fn router(state: Arc<WebhookState>) -> Router {
    Router::new()
        .route("/internal/x402-settled", post(handle_settled))
        .with_state(state)
}

async fn handle_settled(
    State(state): State<Arc<WebhookState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    // 1) The signature is checked before the body is even parsed.
    let signature = headers
        .get("x-internal-signature")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !verify_hmac(&state.internal_secret, &body, signature) {
        tracing::warn!(
            sig_len = signature.len(),
            body_len = body.len(),
            "x402 webhook rejected: bad or absent X-Internal-Signature",
        );
        return (StatusCode::UNAUTHORIZED, "bad signature").into_response();
    }

    // 2) Parse.
    let notify: InternalSettledNotify = match serde_json::from_slice(&body) {
        Ok(n) => n,
        Err(e) => {
            tracing::warn!(error = %e, "x402 webhook rejected: malformed body");
            return (StatusCode::BAD_REQUEST, format!("bad body: {e}")).into_response();
        }
    };

    // 3) Idempotency: was this payment_id credited already?
    let already = match state
        .billing
        .topup_exists_by_payment_id(notify.payment_id)
        .await
    {
        Ok(b) => b,
        Err(e) => {
            tracing::error!(error = %e, payment_id = %notify.payment_id, "ledger lookup failed");
            return (StatusCode::INTERNAL_SERVER_ERROR, "ledger lookup failed").into_response();
        }
    };
    if already {
        tracing::info!(payment_id = %notify.payment_id, "x402 webhook duplicate ignored");
        return (StatusCode::CONFLICT, "already credited").into_response();
    }

    // 4) Insert the top-up. The metadata carries everything an audit needs.
    let meta = json!({
        "payment_id": notify.payment_id,
        "tx_hash": notify.tx_hash,
        "block_number": notify.block_number,
        "settled_at": notify.settled_at,
        "correlation_id": notify.correlation_id,
        "source": "x402_settled_webhook",
    });
    let result = state
        .billing
        .topup(
            &notify.room_id,
            notify.amount_usd,
            Some(&notify.user_mxid),
            meta,
        )
        .await;

    match result {
        Ok(event) => {
            tracing::info!(
                payment_id = %notify.payment_id,
                room_id = %notify.room_id,
                amount_usd = notify.amount_usd,
                ledger_event_id = %event.event_id,
                "x402 topup credited",
            );
            // 5) Hand the announcement to the Matrix side. The balance is computed after the
            //    top-up landed so the user sees the post-credit number. A closed channel is
            //    not an error: the top-up is already in the ledger.
            if let Some(tx) = &state.announce_tx {
                let new_balance_usd = match state.billing.compute_balance(&notify.room_id).await {
                    Ok(b) => b,
                    Err(e) => {
                        tracing::warn!(
                            error = %e,
                            payment_id = %notify.payment_id,
                            "could not compute the balance after the topup; announcing the amount only",
                        );
                        notify.amount_usd
                    }
                };
                let announcement = TopupAnnouncement {
                    room_id: notify.room_id.clone(),
                    amount_usd: notify.amount_usd,
                    tx_hash: notify.tx_hash.clone(),
                    new_balance_usd,
                    credited_to_user: notify.user_mxid.clone(),
                    payment_id: notify.payment_id,
                };
                if let Err(e) = tx.send(announcement) {
                    tracing::warn!(
                        error = %e,
                        payment_id = %notify.payment_id,
                        "announce channel closed; topup not announced in the room",
                    );
                }
            }
            (StatusCode::OK, "ok").into_response()
        }
        Err(BillingError::DuplicatePaymentId(pid)) => {
            // Two notifications passed the pre-insert check before either inserted; the
            // ledger's unique index on payment_id caught the second one.
            tracing::info!(
                payment_id = %pid,
                "x402 webhook race lost on UNIQUE(payment_id); already credited",
            );
            (StatusCode::CONFLICT, "already credited").into_response()
        }
        Err(e) => {
            tracing::error!(
                error = %e,
                payment_id = %notify.payment_id,
                "x402 topup INSERT failed",
            );
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("topup failed: {e}"),
            )
                .into_response()
        }
    }
}

/// Constant-time HMAC-SHA256 verification. `signature_hex` is the lowercase hex the sidecar
/// puts in `X-Internal-Signature`.
fn verify_hmac(secret: &str, body: &[u8], signature_hex: &str) -> bool {
    if secret.is_empty() || signature_hex.is_empty() {
        return false;
    }
    let mut mac = match Hmac::<Sha256>::new_from_slice(secret.as_bytes()) {
        Ok(m) => m,
        Err(_) => return false,
    };
    mac.update(body);
    let Ok(sig_bytes) = hex::decode(signature_hex) else {
        return false;
    };
    mac.verify_slice(&sig_bytes).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{Body, to_bytes};
    use axum::http::{Method, Request, StatusCode};
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    use tower::ServiceExt;
    use uuid::Uuid;

    fn sign(secret: &str, body: &[u8]) -> String {
        let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(body);
        hex::encode(mac.finalize().into_bytes())
    }

    fn state() -> Arc<WebhookState> {
        Arc::new(WebhookState {
            billing: BillingService::open_in_memory().unwrap(),
            internal_secret: "test-secret".into(),
            announce_tx: None,
        })
    }

    fn state_with_channel() -> (
        Arc<WebhookState>,
        tokio::sync::mpsc::UnboundedReceiver<TopupAnnouncement>,
    ) {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let state = Arc::new(WebhookState {
            billing: BillingService::open_in_memory().unwrap(),
            internal_secret: "test-secret".into(),
            announce_tx: Some(tx),
        });
        (state, rx)
    }

    fn notify_body(payment_id: Uuid, room_id: &str, amount: f64) -> String {
        let n = InternalSettledNotify {
            payment_id,
            room_id: room_id.to_owned(),
            user_mxid: "@u:example.com".into(),
            amount_usd: amount,
            tx_hash: "0xdeadbeef".into(),
            block_number: 71_000_000,
            settled_at: chrono::Utc::now(),
            correlation_id: None,
        };
        serde_json::to_string(&n).unwrap()
    }

    async fn post_settled(
        state: Arc<WebhookState>,
        body: &str,
        signature: &str,
    ) -> (StatusCode, String) {
        let app = router(state);
        let req = Request::builder()
            .method(Method::POST)
            .uri("/internal/x402-settled")
            .header("content-type", "application/json")
            .header("x-internal-signature", signature)
            .body(Body::from(body.to_owned()))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        let status = resp.status();
        let body_bytes = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
        let body = String::from_utf8(body_bytes.to_vec()).unwrap();
        (status, body)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn happy_path_credits_topup() {
        let state = state();
        let body = notify_body(Uuid::now_v7(), "!r:example.com", 0.10);
        let sig = sign(&state.internal_secret, body.as_bytes());
        let (status, _) = post_settled(state.clone(), &body, &sig).await;
        assert_eq!(status, StatusCode::OK);
        let bal = state
            .billing
            .compute_balance("!r:example.com")
            .await
            .unwrap();
        assert!((bal - 0.10).abs() < 1e-9, "balance after topup: {bal}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn rejects_without_signature() {
        let state = state();
        let body = notify_body(Uuid::now_v7(), "!r:example.com", 0.10);
        let (status, body) = post_settled(state, &body, "").await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert!(body.contains("bad signature"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn rejects_wrong_signature() {
        let state = state();
        let body = notify_body(Uuid::now_v7(), "!r:example.com", 0.10);
        let (status, _) = post_settled(state, &body, "deadbeef").await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn rejects_tampered_body() {
        let state = state();
        let original = notify_body(Uuid::now_v7(), "!r:example.com", 0.10);
        let sig = sign(&state.internal_secret, original.as_bytes());
        // A different amount, still valid JSON.
        let tampered = original.replace("0.1", "100.0");
        let (status, _) = post_settled(state, &tampered, &sig).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn rejects_malformed_json() {
        let state = state();
        let body = "{not json";
        let sig = sign(&state.internal_secret, body.as_bytes());
        let (status, body) = post_settled(state, body, &sig).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body.starts_with("bad body"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn duplicate_payment_id_returns_409() {
        let state = state();
        let pid = Uuid::now_v7();
        let body = notify_body(pid, "!r:example.com", 0.10);
        let sig = sign(&state.internal_secret, body.as_bytes());
        let (s1, _) = post_settled(state.clone(), &body, &sig).await;
        assert_eq!(s1, StatusCode::OK);
        let (s2, b2) = post_settled(state.clone(), &body, &sig).await;
        assert_eq!(s2, StatusCode::CONFLICT);
        assert!(b2.contains("already credited"));
        let bal = state
            .billing
            .compute_balance("!r:example.com")
            .await
            .unwrap();
        assert!((bal - 0.10).abs() < 1e-9);
    }

    #[test]
    fn verify_hmac_unit() {
        assert!(verify_hmac("k", b"x", &sign("k", b"x")));
        assert!(!verify_hmac("k", b"x", &sign("other", b"x")));
        assert!(!verify_hmac("k", b"x", "not-hex"));
        assert!(!verify_hmac("", b"x", "ab"));
        assert!(!verify_hmac("k", b"x", ""));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn happy_path_pushes_announcement_with_post_credit_balance() {
        let (state, mut rx) = state_with_channel();
        // A pre-existing balance, so `new_balance_usd` proves the handler computes the balance
        // after the top-up rather than echoing the amount.
        state
            .billing
            .topup("!r:example.com", 0.20, Some("@u:example.com"), json!({}))
            .await
            .unwrap();

        let pid = Uuid::now_v7();
        let body = notify_body(pid, "!r:example.com", 0.10);
        let sig = sign(&state.internal_secret, body.as_bytes());
        let (status, _) = post_settled(state.clone(), &body, &sig).await;
        assert_eq!(status, StatusCode::OK);

        let announcement = tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv())
            .await
            .expect("channel timeout")
            .expect("channel closed");
        assert_eq!(announcement.room_id, "!r:example.com");
        assert!((announcement.amount_usd - 0.10).abs() < 1e-9);
        assert!(
            (announcement.new_balance_usd - 0.30).abs() < 1e-9,
            "post-credit balance: {}",
            announcement.new_balance_usd
        );
        assert_eq!(announcement.tx_hash, "0xdeadbeef");
        assert_eq!(announcement.credited_to_user, "@u:example.com");
        assert_eq!(announcement.payment_id, pid);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn unique_constraint_blocks_storage_layer_race_for_payment_id() {
        // The lost race: two handlers pass the pre-insert check before either inserts.
        // Calling `topup` directly (skipping the handler's check) makes the second call hit
        // the unique index. Sequential because SQLite serialises writes anyway; the index is
        // what matters.
        let state = state();
        let pid = Uuid::now_v7();
        let meta = json!({"payment_id": pid, "tx_hash": "0xfeed"});
        state
            .billing
            .topup("!r:race", 0.10, Some("@u:race"), meta.clone())
            .await
            .expect("first topup should land");

        let err = state
            .billing
            .topup("!r:race", 0.10, Some("@u:race"), meta)
            .await
            .expect_err("second topup with the same payment_id must fail");
        match err {
            BillingError::DuplicatePaymentId(p) => assert_eq!(p, pid),
            other => panic!("expected DuplicatePaymentId, got {other:?}"),
        }
        let bal = state.billing.compute_balance("!r:race").await.unwrap();
        assert!((bal - 0.10).abs() < 1e-9, "balance after race: {bal}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn duplicate_topup_does_not_push_second_announcement() {
        let (state, mut rx) = state_with_channel();
        let pid = Uuid::now_v7();
        let body = notify_body(pid, "!r:example.com", 0.10);
        let sig = sign(&state.internal_secret, body.as_bytes());

        let (s1, _) = post_settled(state.clone(), &body, &sig).await;
        assert_eq!(s1, StatusCode::OK);
        let _ = rx.recv().await;

        let (s2, _) = post_settled(state.clone(), &body, &sig).await;
        assert_eq!(s2, StatusCode::CONFLICT);
        assert!(rx.try_recv().is_err());
    }
}
