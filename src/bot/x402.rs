//! The Matrix side of x402 top-ups: brings up the webhook server the payment sidecar posts
//! settlements to, announces every credited top-up in its room, and forwards the signatures
//! payers send as `cc.chums.x402_submit` events to the instance's own sidecar.
//!
//! The submit path exists because the sidecar is not reachable from a user's device: it listens
//! on the instance's Docker network and, at most, on the host's loopback. The bot is already in
//! the room and already talks to its sidecar, so the event addresses the right instance by
//! construction, even with several bots on one host.

use std::sync::Arc;

use mxlink::MessageResponseType;
use mxlink::matrix_sdk::Room;
use mxlink::matrix_sdk::ruma::events::OriginalSyncMessageLikeEvent;
use mxlink::matrix_sdk::ruma::{RoomId, UserId};

use crate::matrix::events::{
    X402ErrorCode, X402ErrorContent, X402SubmitContent, X402TopupConfirmedContent,
    emit_x402_error, emit_x402_topup_confirmed,
};
use crate::strings;
use crate::x402::{PaymentSubmit, TopupAnnouncement, WebhookState, start_webhook_server};

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

    /// Registers the handler for `cc.chums.x402_submit`, the event a payer's client sends after
    /// signing the permit. Registered unconditionally: when x402 is off, the handler only logs.
    pub(super) fn attach_x402_submit_event_handler(&self) {
        let bot = self.clone();

        self.matrix_link().client().add_event_handler(
            move |event: OriginalSyncMessageLikeEvent<X402SubmitContent>, room: Room| {
                let bot = bot.clone();
                async move {
                    bot.handle_x402_submit(event, room).await;
                }
            },
        );
    }

    /// Checks that the payment belongs to this bot, this room and this sender, then hands the
    /// signature to the sidecar. A successful submit produces no event: the confirmation comes
    /// later through the settlement webhook as `cc.chums.x402_topup_confirmed`. Anything else
    /// is reported to the room as `cc.chums.x402_error`, so the client can stop waiting.
    async fn handle_x402_submit(
        &self,
        event: OriginalSyncMessageLikeEvent<X402SubmitContent>,
        room: Room,
    ) {
        let payment_id = event.content.payment_id;
        let sender = event.sender;

        let Some(client) = self.x402_client() else {
            tracing::debug!(%payment_id, %sender, "x402 submit ignored: x402 is not configured");
            return;
        };

        // The payment record is the only source of truth about who may pay and where.
        let status = match client.payment_status(payment_id).await {
            Ok(Some(status)) => status,
            Ok(None) => {
                tracing::info!(%payment_id, %sender, "x402 submit rejected: unknown payment");
                self.send_x402_error(&room, payment_id, X402ErrorCode::UnknownPayment, None)
                    .await;
                return;
            }
            Err(e) => {
                tracing::warn!(error = %e, %payment_id, "x402 submit: payment status lookup failed");
                self.send_x402_error(
                    &room,
                    payment_id,
                    X402ErrorCode::SidecarUnavailable,
                    Some(e.to_string()),
                )
                .await;
                return;
            }
        };

        if status.room_id != room.room_id().as_str() {
            tracing::warn!(
                %payment_id,
                %sender,
                event_room = %room.room_id(),
                "x402 submit rejected: the payment belongs to another room",
            );
            self.send_x402_error(&room, payment_id, X402ErrorCode::WrongRoom, None)
                .await;
            return;
        }

        if status.user_mxid != sender.as_str() {
            tracing::warn!(
                %payment_id,
                %sender,
                "x402 submit rejected: the payment was requested for another user",
            );
            self.send_x402_error(&room, payment_id, X402ErrorCode::WrongSender, None)
                .await;
            return;
        }

        if status.status == "settled" {
            tracing::info!(
                %payment_id,
                tx_hash = ?status.tx_hash,
                "x402 submit ignored: the payment is already settled",
            );
            return;
        }

        let outcome = match client
            .submit_signed_permit(&PaymentSubmit {
                payment_id,
                buyer_address: event.content.buyer_address,
                signature_hex: event.content.signature_hex,
                permit_payload: event.content.permit_payload,
            })
            .await
        {
            Ok(outcome) => outcome,
            Err(e) => {
                tracing::warn!(error = %e, %payment_id, "x402 submit: the sidecar call failed");
                self.send_x402_error(
                    &room,
                    payment_id,
                    X402ErrorCode::SidecarUnavailable,
                    Some(e.to_string()),
                )
                .await;
                return;
            }
        };

        if outcome.is_accepted() {
            tracing::info!(
                %payment_id,
                %sender,
                next_step = ?outcome.next_step,
                tx_hash = ?outcome.tx_hash,
                "x402 submit forwarded to the sidecar",
            );
            return;
        }

        // The permit stays payable on a facilitator refusal, so the user may retry until it
        // expires; the other codes are final for this payment_id.
        let code = match outcome.status {
            400 | 409 => X402ErrorCode::BadRequest,
            402 => X402ErrorCode::FacilitatorRejected,
            404 => X402ErrorCode::UnknownPayment,
            410 => X402ErrorCode::Expired,
            status if status >= 500 => X402ErrorCode::SidecarUnavailable,
            _ => X402ErrorCode::Internal,
        };
        tracing::warn!(
            %payment_id,
            %sender,
            status = outcome.status,
            detail = ?outcome.detail,
            "x402 submit refused by the sidecar",
        );
        self.send_x402_error(&room, payment_id, code, outcome.detail)
            .await;
    }

    async fn send_x402_error(
        &self,
        room: &Room,
        payment_id: uuid::Uuid,
        code: X402ErrorCode,
        reason: Option<String>,
    ) {
        let content = X402ErrorContent {
            payment_id,
            code,
            reason,
        };
        if let Err(e) = emit_x402_error(room, &content).await {
            tracing::warn!(error = %e, %payment_id, "emit_x402_error failed");
        }
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

            // The confirmation is addressed to whoever paid, so it is in their locale.
            let locale = match UserId::parse(&announcement.credited_to_user) {
                Ok(user_id) => self.resolve_user_locale(&user_id).await,
                Err(e) => {
                    tracing::warn!(
                        credited_to_user = %announcement.credited_to_user,
                        error = %e,
                        "topup announcement: credited_to_user is not a user id; using the fallback locale",
                    );
                    self.i18n_config().fallback_locale.clone()
                }
            };
            let text = strings::billing::topup_confirmed(
                &locale,
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
