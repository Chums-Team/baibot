## 📡 Matrix events for the Chums client

When [billing](./configuration/billing.md) is enabled, the bot talks to the Chums client through custom Matrix events under the `cc.chums.*` namespace. They are plain message-like events (not state events). The Chums client registers a handler per event `type` and renders a widget instead of a text bubble; other clients ignore the events and only see the plain text message the bot sends alongside.

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

### Implementation notes

- Events are sent with `Room::send_raw(event_type, content)`. Encrypted rooms work transparently: matrix-sdk encrypts the content regardless of the event type.
- The bot does not expect the client to acknowledge an event. Idempotency is decided by the ledger: a `topup` row is written at most once per `payment_id`.
- New events should also live under `cc.chums.*`, be defined in `src/matrix/events.rs`, and get the same shape-locking tests.
