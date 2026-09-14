//! Custom Matrix events emitted for the Chums client.
//!
//! All events are plain message-like events (not state events), sent via `Room::send_raw`:
//!
//! - `cc.chums.x402_request`: the bot needs a payment before it can serve the room. The client
//!   renders a pay-widget.
//! - `cc.chums.cap_hit`: a bot-wide daily/monthly spending cap was reached. The client shows a
//!   "service paused" banner.
//! - `cc.chums.x402_topup_confirmed`: a payment settled and the room balance was updated. The client
//!   renders a success bubble.
//!
//! The wire format is plain JSON. The event type strings and field names are a contract with the
//! client; the tests in this module lock the wire shape, and any change needs a coordinated client
//! release. See `docs/matrix-events.md`.

use mxlink::matrix_sdk::{self, Room, ruma::OwnedEventId};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const EVENT_TYPE_X402_REQUEST: &str = "cc.chums.x402_request";
pub const EVENT_TYPE_CAP_HIT: &str = "cc.chums.cap_hit";
pub const EVENT_TYPE_X402_TOPUP_CONFIRMED: &str = "cc.chums.x402_topup_confirmed";

/// USD amounts on the Matrix wire are decimal **strings** (`"1.000000"`), never JSON numbers.
///
/// Matrix canonical JSON forbids floats: Synapse rejects a plaintext event carrying `1.0` with
/// `400 M_BAD_JSON: Bad JSON value: float`. (Encrypted rooms hide the content inside the
/// ciphertext, which is why the problem only shows up in unencrypted rooms.)
///
/// Deserialization accepts both a string and a number, so events emitted by older builds (still
/// readable in encrypted rooms) keep parsing.
///
/// ```
/// use baibot::matrix::events::X402TopupConfirmedContent;
///
/// let content = X402TopupConfirmedContent {
///     room_id: "!abc:example.com".into(),
///     amount_usd: 1.0,
///     tx_hash: "0xabc".into(),
///     new_balance_usd: 1.1,
///     credited_to_user: "@user:example.com".into(),
/// };
/// let value = serde_json::to_value(&content).unwrap();
/// assert_eq!(value["amount_usd"], "1.000000");
/// assert_eq!(value["new_balance_usd"], "1.100000");
///
/// // Both the string form and the legacy number form deserialize.
/// let back: X402TopupConfirmedContent = serde_json::from_value(value).unwrap();
/// assert_eq!(back, content);
/// let legacy: X402TopupConfirmedContent = serde_json::from_value(serde_json::json!({
///     "room_id": "!abc:example.com", "amount_usd": 0.5, "tx_hash": "0x",
///     "new_balance_usd": 2, "credited_to_user": "@user:example.com"
/// })).unwrap();
/// assert_eq!(legacy.amount_usd, 0.5);
/// ```
pub mod usd_string {
    use serde::{Deserialize, Deserializer, Serializer, de};

    /// Ledger precision: 6 decimals, the same as USDT.
    pub fn serialize<S: Serializer>(value: &f64, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&format!("{value:.6}"))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<f64, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum NumberOrString {
            Number(f64),
            String(String),
        }

        match NumberOrString::deserialize(deserializer)? {
            NumberOrString::Number(number) => Ok(number),
            NumberOrString::String(text) => text
                .trim()
                .parse::<f64>()
                .map_err(|err| de::Error::custom(format!("bad USD amount {text:?}: {err}"))),
        }
    }
}

/// Body of `cc.chums.x402_request`. Sent when the room balance is insufficient for the next
/// LLM call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct X402RequestContent {
    /// The room this is about (echoed so the client can cross-check).
    pub room_id: String,
    /// Smallest amount the user must top up to unblock the next call (USD, 6-decimal precision).
    #[serde(with = "usd_string")]
    pub amount_required_usd: f64,
    /// Hard floor on a single payment. The client pre-validates against this.
    #[serde(with = "usd_string")]
    pub min_topup_usd: f64,
    /// TRON Base58 address that receives the USDT.
    pub agent_wallet_tron: String,
    /// Permit2 contract (x402 v2, TRON `exact` scheme): the TRC-20 approve spender and the
    /// TIP-712 `verifyingContract`. Opaque to the bot: forwarded verbatim from the sidecar's
    /// `/payment-request` response. (The field name predates the Permit2 migration; kept for
    /// wire compatibility.)
    pub permit_contract: String,
    /// CAIP-style network id, e.g. `tron:0x2b6653dc` (mainnet).
    pub network: String,
    /// Token symbol, typically `USDT` (TRC-20).
    pub token: String,
    /// Sidecar-issued payment id, opaque to the client.
    pub payment_id: Uuid,
    /// TIP-712 typed-data payload to sign. The client passes it to its TRON wallet as-is.
    pub permit_payload: serde_json::Value,
    /// ISO-8601 UTC. After this the `payment_id` is invalid.
    pub expires_at: String,
    /// The bot's command prefix (e.g. `!bai`), so the client can interpolate it into localized
    /// hints ("for a different amount: `{prefix} topup <amount>`") without hardcoding it.
    pub command_prefix: String,
    /// The sidecar's cached facilitator readiness: `Some(false)` means the client should not offer
    /// approve/pay right now; `None` means unknown. Forwarded verbatim; absent on older builds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub facilitator_ok: Option<bool>,
    /// Why `facilitator_ok` is false (operator-facing, may be shown to the user).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub facilitator_reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CapPeriod {
    Daily,
    Monthly,
}

/// Body of `cc.chums.cap_hit`. A bot-wide cap (not the room balance) was reached.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CapHitContent {
    pub period: CapPeriod,
    #[serde(with = "usd_string")]
    pub cap_usd: f64,
    #[serde(with = "usd_string")]
    pub current_spent_usd: f64,
    /// ISO-8601 UTC of the next window boundary: the next 00:00 UTC for `daily`, the next
    /// month start (UTC) for `monthly`.
    pub retries_at: String,
}

/// Body of `cc.chums.x402_topup_confirmed`. Sent after a payment settled and the corresponding
/// `topup` row was written to the ledger.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct X402TopupConfirmedContent {
    pub room_id: String,
    #[serde(with = "usd_string")]
    pub amount_usd: f64,
    pub tx_hash: String,
    #[serde(with = "usd_string")]
    pub new_balance_usd: f64,
    /// Matrix user id of who paid (in a DM the user; in a group whichever member paid).
    pub credited_to_user: String,
}

#[derive(Debug, thiserror::Error)]
pub enum EmitError {
    #[error("serializing event content failed: {0}")]
    Serialize(#[from] serde_json::Error),
    #[error("sending the event failed: {0}")]
    Matrix(#[from] matrix_sdk::Error),
}

/// Sends `cc.chums.x402_request` into the room. Returns the id of the sent event.
pub async fn emit_x402_request(
    room: &Room,
    content: &X402RequestContent,
) -> Result<OwnedEventId, EmitError> {
    send_raw(room, EVENT_TYPE_X402_REQUEST, content).await
}

/// Sends `cc.chums.cap_hit` into the room. Returns the id of the sent event.
pub async fn emit_cap_hit(room: &Room, content: &CapHitContent) -> Result<OwnedEventId, EmitError> {
    send_raw(room, EVENT_TYPE_CAP_HIT, content).await
}

/// Sends `cc.chums.x402_topup_confirmed` into the room. Returns the id of the sent event.
pub async fn emit_x402_topup_confirmed(
    room: &Room,
    content: &X402TopupConfirmedContent,
) -> Result<OwnedEventId, EmitError> {
    send_raw(room, EVENT_TYPE_X402_TOPUP_CONFIRMED, content).await
}

async fn send_raw<T: Serialize>(
    room: &Room,
    event_type: &str,
    content: &T,
) -> Result<OwnedEventId, EmitError> {
    let value = serde_json::to_value(content)?;
    let response = room.send_raw(event_type, value).await?;
    Ok(response.response.event_id)
}

// The tests below lock the wire shape. Sending through matrix-sdk is not exercised here.
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn x402_request() -> X402RequestContent {
        X402RequestContent {
            room_id: "!abc:example.com".into(),
            amount_required_usd: 0.10,
            min_topup_usd: 0.10,
            agent_wallet_tron: "TSkeaPMSuaojcCzE7mWqw4xN1awU7NfdfY".into(),
            permit_contract: "TT8rEWbCoNX7vpEUauxb7rWJsTgs8vDLAn".into(),
            network: "tron:0x2b6653dc".into(),
            token: "USDT".into(),
            payment_id: Uuid::nil(),
            permit_payload: json!({"types": {}, "primaryType": "PaymentPermit"}),
            expires_at: "2026-05-02T15:00:00Z".into(),
            command_prefix: "!bai".into(),
            facilitator_ok: None,
            facilitator_reason: None,
        }
    }

    #[test]
    fn x402_request_serializes_to_expected_shape() {
        let content = x402_request();

        let value = serde_json::to_value(&content).unwrap();
        assert_eq!(value["room_id"], "!abc:example.com");
        assert_eq!(value["amount_required_usd"], "0.100000");
        assert_eq!(value["min_topup_usd"], "0.100000");
        assert_eq!(
            value["agent_wallet_tron"],
            "TSkeaPMSuaojcCzE7mWqw4xN1awU7NfdfY"
        );
        assert_eq!(
            value["permit_contract"],
            "TT8rEWbCoNX7vpEUauxb7rWJsTgs8vDLAn"
        );
        assert_eq!(value["network"], "tron:0x2b6653dc");
        assert_eq!(value["token"], "USDT");
        assert_eq!(value["payment_id"], "00000000-0000-0000-0000-000000000000");
        assert_eq!(value["permit_payload"]["primaryType"], "PaymentPermit");
        assert_eq!(value["expires_at"], "2026-05-02T15:00:00Z");
        assert_eq!(value["command_prefix"], "!bai");

        let back: X402RequestContent = serde_json::from_value(value).unwrap();
        assert_eq!(back, content);
    }

    #[test]
    fn x402_request_facilitator_fields_are_optional_on_the_wire() {
        let value = serde_json::to_value(x402_request()).unwrap();
        assert!(value.get("facilitator_ok").is_none());
        assert!(value.get("facilitator_reason").is_none());

        let content = X402RequestContent {
            facilitator_ok: Some(false),
            facilitator_reason: Some("facilitator unreachable".into()),
            ..x402_request()
        };
        let value = serde_json::to_value(&content).unwrap();
        assert_eq!(value["facilitator_ok"], false);
        assert_eq!(value["facilitator_reason"], "facilitator unreachable");
        let back: X402RequestContent = serde_json::from_value(value).unwrap();
        assert_eq!(back, content);

        // Events from builds that predate the fields still parse.
        let mut without = serde_json::to_value(x402_request()).unwrap();
        without.as_object_mut().unwrap().remove("facilitator_ok");
        without
            .as_object_mut()
            .unwrap()
            .remove("facilitator_reason");
        let back: X402RequestContent = serde_json::from_value(without).unwrap();
        assert_eq!(back, x402_request());
    }

    #[test]
    fn cap_hit_period_is_lowercase() {
        let daily = CapHitContent {
            period: CapPeriod::Daily,
            cap_usd: 50.0,
            current_spent_usd: 49.95,
            retries_at: "2026-05-03T00:00:00Z".into(),
        };
        let value = serde_json::to_value(&daily).unwrap();
        assert_eq!(value["period"], "daily");
        assert_eq!(value["cap_usd"], "50.000000");
        assert_eq!(value["current_spent_usd"], "49.950000");
        assert_eq!(value["retries_at"], "2026-05-03T00:00:00Z");
        let back: CapHitContent = serde_json::from_value(value).unwrap();
        assert_eq!(back, daily);

        let monthly = CapHitContent {
            period: CapPeriod::Monthly,
            cap_usd: 1000.0,
            current_spent_usd: 999.5,
            retries_at: "2026-06-01T00:00:00Z".into(),
        };
        assert_eq!(serde_json::to_value(&monthly).unwrap()["period"], "monthly");
    }

    #[test]
    fn topup_confirmed_round_trips() {
        let content = X402TopupConfirmedContent {
            room_id: "!abc:example.com".into(),
            amount_usd: 0.10,
            tx_hash: "0xabcdef".into(),
            new_balance_usd: 0.10,
            credited_to_user: "@user:example.com".into(),
        };
        let value = serde_json::to_value(&content).unwrap();
        assert_eq!(value["amount_usd"], "0.100000");
        assert_eq!(value["new_balance_usd"], "0.100000");
        let back: X402TopupConfirmedContent = serde_json::from_value(value).unwrap();
        assert_eq!(back, content);
    }

    /// Canonical JSON (Synapse) rejects floats in plaintext events, so no `f64` may reach the wire.
    #[test]
    fn usd_amounts_never_serialize_as_json_floats() {
        fn assert_no_floats(value: &serde_json::Value, path: &str) {
            match value {
                serde_json::Value::Number(number) => {
                    assert!(
                        number.is_i64() || number.is_u64(),
                        "float on wire at {path}: {number}"
                    )
                }
                serde_json::Value::Object(map) => {
                    for (key, inner) in map {
                        assert_no_floats(inner, &format!("{path}.{key}"));
                    }
                }
                serde_json::Value::Array(items) => {
                    for (i, inner) in items.iter().enumerate() {
                        assert_no_floats(inner, &format!("{path}[{i}]"));
                    }
                }
                _ => {}
            }
        }

        let request = X402RequestContent {
            amount_required_usd: 1.0,
            min_topup_usd: 0.1,
            facilitator_ok: Some(true),
            ..x402_request()
        };
        assert_no_floats(&serde_json::to_value(&request).unwrap(), "x402_request");

        let cap = CapHitContent {
            period: CapPeriod::Daily,
            cap_usd: 50.0,
            current_spent_usd: 49.95,
            retries_at: "2026-05-03T00:00:00Z".into(),
        };
        assert_no_floats(&serde_json::to_value(&cap).unwrap(), "cap_hit");

        let topup = X402TopupConfirmedContent {
            room_id: "!abc:example.com".into(),
            amount_usd: 1.0,
            tx_hash: "0xabc".into(),
            new_balance_usd: 1.1,
            credited_to_user: "@user:example.com".into(),
        };
        assert_no_floats(&serde_json::to_value(&topup).unwrap(), "topup_confirmed");
    }

    /// Events from older builds (readable in encrypted rooms) carried JSON numbers; they must
    /// still deserialize.
    #[test]
    fn usd_amounts_deserialize_from_number_or_string() {
        let from_number: X402TopupConfirmedContent = serde_json::from_value(json!({
            "room_id": "!abc:example.com", "amount_usd": 0.10, "tx_hash": "0x",
            "new_balance_usd": 2, "credited_to_user": "@user:example.com"
        }))
        .unwrap();
        assert_eq!(from_number.amount_usd, 0.10);
        assert_eq!(from_number.new_balance_usd, 2.0);

        let from_string: X402TopupConfirmedContent = serde_json::from_value(json!({
            "room_id": "!abc:example.com", "amount_usd": "0.100000", "tx_hash": "0x",
            "new_balance_usd": " 2.000000 ", "credited_to_user": "@user:example.com"
        }))
        .unwrap();
        assert_eq!(from_string.amount_usd, 0.10);
        assert_eq!(from_string.new_balance_usd, 2.0);

        let bad: Result<X402TopupConfirmedContent, _> = serde_json::from_value(json!({
            "room_id": "!abc:example.com", "amount_usd": "lots", "tx_hash": "0x",
            "new_balance_usd": "2", "credited_to_user": "@user:example.com"
        }));
        assert!(bad.is_err());
    }

    /// The event type strings are part of the wire contract with the client.
    #[test]
    fn event_type_constants_match_the_contract() {
        assert_eq!(EVENT_TYPE_X402_REQUEST, "cc.chums.x402_request");
        assert_eq!(EVENT_TYPE_CAP_HIT, "cc.chums.cap_hit");
        assert_eq!(
            EVENT_TYPE_X402_TOPUP_CONFIRMED,
            "cc.chums.x402_topup_confirmed"
        );
    }
}
