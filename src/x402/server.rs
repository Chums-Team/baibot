//! Binds the internal HTTP endpoint the sidecar posts settlements to.
//!
//! The bot exposes a single route, `POST /internal/x402-settled`, on
//! `x402.internal_bind:x402.internal_port` (default `127.0.0.1:9000`). The sidecar signs
//! each notification with `x402.internal_secret`; `webhook` verifies the signature and
//! credits the ledger.
//!
//! The endpoint is HMAC-protected, but a leaked secret plus an open port would be a free
//! credit injection vector, so a non-loopback bind is refused unless the operator opted in
//! (`x402.allow_non_loopback_bind`), and is logged as a warning even then.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use anyhow::Context;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

use super::webhook::{WebhookState, router};

/// Binds on `(bind, port)` and serves the webhook router on a spawned task. Returns the
/// bound address (useful with `port == 0` in tests) and the handle of the serve loop.
///
/// The task lives as long as the runtime: the bot has no shutdown hook besides the process
/// exiting, so the handle is never awaited.
pub async fn start_webhook_server(
    state: Arc<WebhookState>,
    bind: IpAddr,
    port: u16,
    allow_non_loopback_bind: bool,
) -> anyhow::Result<(SocketAddr, JoinHandle<()>)> {
    if state.internal_secret.is_empty() {
        return Err(anyhow::anyhow!(
            "x402: refusing to start the webhook server with an empty internal_secret"
        ));
    }

    if !bind.is_loopback() {
        tracing::warn!(
            bind = %bind,
            allow_non_loopback_bind,
            "x402: internal_bind is a non-loopback address; the HMAC and your firewall are the only protection of the endpoint"
        );
        if !allow_non_loopback_bind {
            return Err(anyhow::anyhow!(
                "x402: refusing to bind the webhook server on the non-loopback address {bind}. \
                 Bind on 127.0.0.1, or set x402.allow_non_loopback_bind to true and firewall the endpoint yourself"
            ));
        }
    }

    let addr = SocketAddr::new(bind, port);
    let listener = TcpListener::bind(addr)
        .await
        .with_context(|| format!("x402: failed to bind the webhook server on {addr}"))?;
    let local_addr = listener.local_addr()?;

    tracing::info!(bind = %local_addr, "x402 webhook server listening");

    let app = router(state);
    let join = tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app).await {
            tracing::error!(error = %e, "x402 webhook server exited with error");
        }
    });

    Ok((local_addr, join))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::billing::BillingService;
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    use uuid::Uuid;

    fn sign(secret: &str, body: &[u8]) -> String {
        let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(body);
        hex::encode(mac.finalize().into_bytes())
    }

    fn make_state(secret: &str) -> Arc<WebhookState> {
        Arc::new(WebhookState {
            billing: BillingService::open_in_memory().unwrap(),
            internal_secret: secret.into(),
            announce_tx: None,
        })
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn binds_loopback_and_serves_endpoint() {
        // A real TCP listener on a random loopback port and a full HTTP round trip.
        let state = make_state("test-server-secret-32-bytes-long");
        let (addr, _join) =
            start_webhook_server(state.clone(), IpAddr::from([127, 0, 0, 1]), 0, false)
                .await
                .expect("server should bind on 127.0.0.1:auto");
        assert!(
            addr.ip().is_loopback(),
            "must bind loopback only, got {addr}"
        );

        let pid = Uuid::now_v7();
        let body = serde_json::json!({
            "payment_id": pid,
            "room_id": "!server-test:example.com",
            "user_mxid": "@u:example.com",
            "amount_usd": 0.42,
            "tx_hash": "0xfeed",
            "block_number": 71_000_000,
            "settled_at": chrono::Utc::now(),
            "correlation_id": null,
        })
        .to_string();
        let sig = sign(&state.internal_secret, body.as_bytes());

        let resp = reqwest::Client::new()
            .post(format!("http://{addr}/internal/x402-settled"))
            .header("content-type", "application/json")
            .header("x-internal-signature", sig)
            .body(body)
            .send()
            .await
            .expect("HTTP send");
        assert_eq!(
            resp.status().as_u16(),
            200,
            "{}",
            resp.text().await.unwrap()
        );

        let bal = state
            .billing
            .compute_balance("!server-test:example.com")
            .await
            .unwrap();
        assert!(
            (bal - 0.42).abs() < 1e-9,
            "balance after server topup: {bal}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn unsigned_request_returns_401_over_real_tcp() {
        let state = make_state("test-server-secret-32-bytes-long");
        let (addr, _join) = start_webhook_server(state, IpAddr::from([127, 0, 0, 1]), 0, false)
            .await
            .unwrap();

        let resp = reqwest::Client::new()
            .post(format!("http://{addr}/internal/x402-settled"))
            .header("content-type", "application/json")
            .body(r#"{"x":1}"#)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status().as_u16(), 401);
    }

    #[tokio::test]
    async fn refuses_to_start_with_empty_secret() {
        let err = start_webhook_server(make_state(""), IpAddr::from([127, 0, 0, 1]), 0, false)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("internal_secret"), "{err}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn non_loopback_bind_is_refused_without_opt_in() {
        let err = start_webhook_server(
            make_state("test-server-secret-32-bytes-long"),
            IpAddr::from([0, 0, 0, 0]),
            0,
            false,
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(err.contains("non-loopback"), "{err}");
        assert!(err.contains("allow_non_loopback_bind"), "{err}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn non_loopback_bind_is_allowed_with_opt_in() {
        let (addr, _join) = start_webhook_server(
            make_state("test-server-secret-32-bytes-long"),
            IpAddr::from([0, 0, 0, 0]),
            0,
            true,
        )
        .await
        .expect("opt-in should permit a non-loopback bind");
        assert!(addr.ip().is_unspecified());
    }
}
