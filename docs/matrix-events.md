## 📡 Matrix events for the Chums client

When [billing](./configuration/billing.md) is enabled, the bot talks to the Chums client through custom Matrix events under the `cc.chums.*` namespace. They are plain message-like events (not state events). The Chums client registers a handler per event `type` and renders a widget instead of a text bubble; other clients ignore the events and only see the plain text message the bot sends alongside.

Two events go the other way: the client sends [`cc.chums.set_user_locale`](#ccchumsset_user_locale-client--bot) to tell the bot the user's language, and [`cc.chums.x402_submit`](#ccchumsx402_submit-client--bot) to hand the bot a signed payment.

The Rust source of truth is [`src/matrix/events.rs`](../src/matrix/events.rs). The tests in that file lock the wire shape: the event type strings and field names are a contract with the client, and any change needs a coordinated client release.

### USD amounts are strings

USD amounts are decimal **strings** with 6 decimals (`"1.000000"`), never JSON numbers. Matrix canonical JSON forbids floats: Synapse rejects a plaintext event carrying `1.0` with `400 M_BAD_JSON: Bad JSON value: float`, so an event with a float would never reach an unencrypted room (in encrypted rooms the content is hidden inside the ciphertext, which masks the problem).

### `cc.chums.cap_hit`

A bot-wide daily or monthly spending cap (see `daily_cap_usd` / `monthly_cap_usd` in the billing configuration) was reached. This is not about the room balance. The bot does not call the LLM until the next window.

```json
{
  "type": "cc.chums.cap_hit",
  "content": {
    "period": "daily",
    "cap_usd": "50.000000",
    "current_spent_usd": "49.950000",
    "retries_at": "2026-05-03T00:00:00Z"
  }
}
```

`period` is `daily` or `monthly`. `retries_at` is the next window boundary in UTC: the next 00:00 UTC for `daily`, the next month start for `monthly`. The client shows a "service paused until …" banner.

### `cc.chums.x402_request`

The room balance is below `reserve_amount_usd` (the bot will not call the LLM until the room is topped up), or a user ran the `topup` command. Sent only when the [x402 integration](./configuration/x402.md) is configured; the plain text message that accompanies it explains how to pay.

```json
{
  "type": "cc.chums.x402_request",
  "content": {
    "room_id": "!abc:example.com",
    "amount_required_usd": "0.100000",
    "min_topup_usd": "0.100000",
    "agent_wallet_tron": "TSkeaPMSuaojcCzE7mWqw4xN1awU7NfdfY",
    "permit_contract": "TTJxU3P8rHycAyFY4kVtGNfmnMH4ezcuM9",
    "network": "tron:0x2b6653dc",
    "token": "USDT",
    "payment_id": "018f37c2-0000-7000-8000-000000000000",
    "permit_payload": { "types": { "...": "..." }, "primaryType": "PaymentPermit", "domain": {}, "message": {} },
    "expires_at": "2026-05-02T15:00:00Z",
    "command_prefix": "!bai",
    "facilitator_ok": true
  }
}
```

- `amount_required_usd` is the suggested amount; the client must not offer less than `min_topup_usd`.
- `agent_wallet_tron`, `permit_contract`, `network`, `token`, `payment_id`, `permit_payload` and `expires_at` come from the x402 payment sidecar and are forwarded verbatim. The client passes `permit_payload` to its TRON wallet for TIP-712 signing and submits the signed payment through the sidecar.
- `command_prefix` lets the client mention the `topup` command in hints without hardcoding the bot's prefix.
- `facilitator_ok` / `facilitator_reason` are optional. `facilitator_ok: false` means the sidecar cannot currently settle payments and the client should not offer to pay; `facilitator_reason` says why. Both are absent when unknown.

### `cc.chums.x402_topup_confirmed`

A payment settled on-chain and was credited to the room.

```json
{
  "type": "cc.chums.x402_topup_confirmed",
  "content": {
    "room_id": "!abc:example.com",
    "amount_usd": "0.100000",
    "tx_hash": "0xabc...",
    "new_balance_usd": "0.100000",
    "credited_to_user": "@user:example.com"
  }
}
```

The bot also sends a plain text confirmation for clients that do not recognize the event.

### `cc.chums.x402_submit` (client → bot)

Sent by the payer's client into the room of the payment, after the user signed the permit of a `cc.chums.x402_request`. The bot forwards it to its own payment sidecar, which is not reachable from a user's device; the event also addresses the right bot when several bots share a host.

```json
{
  "type": "cc.chums.x402_submit",
  "content": {
    "payment_id": "0192ab99-a000-7000-8000-000000000001",
    "buyer_address": "TUEZSdKsoDHQMeZwihtdoBiN46zxhGWYdH",
    "signature_hex": "0x…",
    "permit_payload": { "…": "the signed envelope, optional" }
  }
}
```

The sender of the event is the payer: the bot checks it, and the room, against its sidecar's payment record (`GET /status/{payment_id}`) and refuses anything that does not match. `permit_payload` is an optional echo of the signed envelope; the sidecar compares it with the one it issued and never builds the authorization from it.

A successful submit is not acknowledged. The confirmation is the `cc.chums.x402_topup_confirmed` event that follows once the payment settles on-chain, which may take a while. Anything that went wrong comes back as `cc.chums.x402_error`.

### `cc.chums.x402_error`

The bot could not forward a `cc.chums.x402_submit`, or the sidecar refused it. Sent into the same room so the client can stop waiting and show the reason.

```json
{
  "type": "cc.chums.x402_error",
  "content": {
    "payment_id": "0192ab99-a000-7000-8000-000000000001",
    "code": "facilitator_rejected",
    "reason": "insufficient_allowance: approve more USDT"
  }
}
```

| `code` | Meaning |
|---|---|
| `unknown_payment` | No payment with this id. A payment issued by another bot looks the same way. |
| `wrong_room` | The payment was requested in another room. |
| `wrong_sender` | The payment was requested for another user. |
| `expired` | The permit is past `expires_at`, or the payment is no longer payable. |
| `bad_request` | The sidecar refused the signature, the address or the echoed envelope. |
| `facilitator_rejected` | The facilitator refused to verify or settle. The permit stays payable until it expires, so a retry makes sense. |
| `sidecar_unavailable` | The bot could not reach its sidecar, or the sidecar failed internally. |
| `internal` | Anything else on the bot's side. |

`reason` is optional diagnostic detail for a human and carries no secrets; the client localizes `code` and may show `reason` next to it.

### `cc.chums.set_user_locale` (client → bot)

Sent by the user's client into any room shared with the bot when the user picks a language, and again on start-up (the bot does not persist it). The sender of the event is the user whose locale it is; the bot replies to that user in this language from then on. See [🌍 Localization](./configuration/i18n.md).

```json
{
  "type": "cc.chums.set_user_locale",
  "content": {
    "locale": "ru"
  }
}
```

`locale` is a BCP 47 language tag, usually just the language. A regional variant maps onto its language (`pt-BR` → `pt`); a language the bot has no translations for is ignored. The bot does not acknowledge the event.

### Implementation notes

- Outbound events are sent with `Room::send_raw(event_type, content)`. Encrypted rooms work transparently: matrix-sdk encrypts the content regardless of the event type. Inbound events are typed `EventContent`s and arrive through matrix-sdk event handlers.
- The bot does not expect the client to acknowledge an event. Idempotency is decided by the ledger: a `topup` row is written at most once per `payment_id`.
- New events should also live under `cc.chums.*`, be defined in `src/matrix/events.rs`, and get the same shape-locking tests.
