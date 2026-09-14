//! The Matrix side of x402 top-ups: brings up the webhook server the payment sidecar posts
//! settlements to, and announces every credited top-up in its room.

use std::sync::Arc;

use mxlink::MessageResponseType;
use mxlink::matrix_sdk::ruma::RoomId;

use crate::matrix::events::{X402TopupConfirmedContent, emit_x402_topup_confirmed};
use crate::strings;
use crate::x402::{TopupAnnouncement, WebhookState, start_webhook_server};

use super::Bot;

impl Bot {
    /// Starts the settlement webhook server when the `x402` section is configured.
    /// The server and the announcement drainer live as long as the runtime.
    pub(super) async fn start_x402(&self) -> anyhow::Result<()> {
        let (Some(x402), Some(billing)) = (self.x402_config(), self.billing()) else {
            return Ok(());
        };

        // Unbounded: settlements arrive far below one per second, and a stalled Matrix
        // send must not block the sidecar's HTTP call (the top-up is already in the ledger).
        let (announce_tx, announce_rx) =
            tokio::sync::mpsc::unbounded_channel::<TopupAnnouncement>();

        let state = Arc::new(WebhookState {
            billing: billing.service.clone(),
            internal_secret: x402.internal_secret.clone(),
            announce_tx: Some(announce_tx),
        });
        let (addr, _join) = start_webhook_server(
            state,
            x402.internal_bind,
            x402.internal_port,
            x402.allow_non_loopback_bind,
        )
        .await?;
        tracing::info!(%addr, sidecar_url = %x402.sidecar_url, "x402 top-ups enabled");

        let bot = self.clone();
        tokio::spawn(async move {
            bot.drain_topup_announcements(announce_rx).await;
        });

        Ok(())
    }

    /// Posts a `cc.chums.x402_topup_confirmed` event and a plain text confirmation for every
    /// top-up the webhook credited. Failures are logged and the loop goes on: the ledger row
    /// is committed regardless.
    async fn drain_topup_announcements(
        &self,
        mut rx: tokio::sync::mpsc::UnboundedReceiver<TopupAnnouncement>,
    ) {
        while let Some(announcement) = rx.recv().await {
            let Ok(room_id) = RoomId::parse(&announcement.room_id) else {
                tracing::warn!(
                    payment_id = %announcement.payment_id,
                    room_id = %announcement.room_id,
                    "topup announcement: room_id failed to parse",
                );
                continue;
            };
            let Some(room) = self.matrix_link().client().get_room(&room_id) else {
                tracing::warn!(
                    payment_id = %announcement.payment_id,
                    %room_id,
                    "topup announcement: the bot is not in that room",
                );
                continue;
            };

            let content = X402TopupConfirmedContent {
                room_id: announcement.room_id.clone(),
                amount_usd: announcement.amount_usd,
                tx_hash: announcement.tx_hash.clone(),
                new_balance_usd: announcement.new_balance_usd,
                credited_to_user: announcement.credited_to_user.clone(),
            };
            if let Err(e) = emit_x402_topup_confirmed(&room, &content).await {
                tracing::warn!(
                    error = %e,
                    payment_id = %announcement.payment_id,
                    "emit_x402_topup_confirmed failed",
                );
            }

            let text = strings::billing::topup_confirmed(
                announcement.amount_usd,
                announcement.new_balance_usd,
                &short_tx_hash(&announcement.tx_hash),
            );
            self.messaging()
                .send_text_markdown_no_fail(&room, text, MessageResponseType::InRoom)
                .await;
        }
    }
}

/// A 64-character transaction hash wraps onto two lines in a chat bubble; the text
/// confirmation shows the first 8 and the last 4 characters instead. The event carries the
/// full hash.
fn short_tx_hash(hash: &str) -> String {
    const HEAD: usize = 8;
    const TAIL: usize = 4;

    let chars: Vec<char> = hash.chars().collect();
    if chars.len() <= HEAD + TAIL + 2 {
        return hash.to_owned();
    }

    let head: String = chars[..HEAD].iter().collect();
    let tail: String = chars[chars.len() - TAIL..].iter().collect();
    format!("{head}…{tail}")
}

#[cfg(test)]
mod tests {
    use super::short_tx_hash;

    #[test]
    fn short_hash_keeps_short_values() {
        assert_eq!(short_tx_hash("0xfeed"), "0xfeed");
        assert_eq!(short_tx_hash("12345678901234"), "12345678901234");
    }

    #[test]
    fn short_hash_abbreviates_long_values() {
        let full = "a3f9c2e17b4d6e8f0a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f6071";
        assert_eq!(short_tx_hash(full), "a3f9c2e1…6071");
    }
}
