# himari-x402-sidecar

Thin Python (FastAPI) sidecar that bridges the Rust bot (this repository,
`chums` branch) and the Flutter Chums client to an **x402 v2 facilitator**
(hosted BofAI `https://facilitator.bankofai.io`, or a self-hosted build of
[`BofAI/x402-facilitator`](https://github.com/BofAI/x402-facilitator)).

Scheme: TRON **`exact`** with the **Permit2** asset-transfer method
(`x402ExactPermit2Proxy`). No SDK dependency: the wire is plain JSON plus a
TIP-712 envelope that this module builds itself
(`permit2_builder.py`, pinned by tests against the on-chain type strings).

The bot speaks a narrow HTTP API to the sidecar; the sidecar holds the
facilitator API key and the `payment_id ↔ nonce` mapping.

## Flow

```
bot ──POST /payment-request──▶ sidecar ──(pure)──▶ Permit2 typed data + nonce
                                 │
client signs typed data (TronLink / wallet), then
client ──POST /payment-request/submit {payment_id, buyer_address, signature_hex}──▶ sidecar
                                 │
                                 ├─▶ POST facilitator /verify   (fast reject: bad sig, no allowance, no USDT)
                                 ├─▶ POST facilitator /settle   (on-chain: proxy.settle → Permit2 → USDT transfer)
                                 └─▶ POST bot /internal/x402-settled (HMAC)  → bot credits the room
```

One-time prerequisite for the payer: `USDT.approve(Permit2, MAX)`. The
client reads the spender from `permit_contract` in the payment request.

## Endpoints

| Endpoint | Purpose |
|---|---|
| `GET  /health` | Liveness + config echo (secrets redacted): network, Permit2 / proxy / asset contracts, stub or live mode. |
| `POST /payment-request` | Bot asks for a payment. Returns `payment_id`, `permit_payload` (TIP-712 envelope to sign as-is), `payment_requirements`, `permit_contract` (Permit2), `proxy_contract`, `network` (hex CAIP-2), `pay_to`. |
| `POST /payment-request/submit` | Client returns `{payment_id, buyer_address, signature_hex[, permit_payload]}`. 200 `settled` / `already-settled` / `awaiting-live-wiring`; **402** `verify-failed` / `settle-failed` with `error_reason`; 400 bad input; 410 expired; 502 facilitator transport error. |
| `GET  /status/{payment_id}` | Read-back: status, tx_hash, nonce, deadline, buyer, last `error_reason`. |
| `POST /webhook/settlement` | Legacy push path (HMAC). The v2 facilitator does not push; kept for a future reconciliation job. |

Reconciliation with the facilitator: `GET {facilitator}/payments?network=<hex>&nonce=<decimal>`
under our `X-API-KEY` returns the settlement row for a payment's `nonce`.

## Networks

| Network | `X402_NETWORK` | Permit2 | x402ExactPermit2Proxy | USDT |
|---|---|---|---|---|
| TRON mainnet | `tron:0x2b6653dc` | `TTJxU3P8rHycAyFY4kVtGNfmnMH4ezcuM9` | `TN49yaJmZMZoEdDCqjB4uPzQLHvYkGw95m` | `TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t` |
| TRON nile | `tron:0xcd8690dc` | `TYQuuhGbEMxF7nZxUHV3uHJxAVVAegNU9h` | `TFGoaq2KjizijgjtkVxT7yjffW1A5T1j6F` | `TXYZopYRdj2D9XRtbG411XZZ3kM5VkAeBf` |

Legacy names `tron:mainnet` / `tron:nile` are accepted in `.env` and
canonicalised to the hex ids on the wire.

## Quick start (docker)

```sh
cd x402-sidecar
cp .env.example .env       # fill the secrets
docker compose up -d --build
curl -s http://localhost:8402/health | jq
```

## Quick start (local, no docker)

```sh
cd x402-sidecar
python -m venv .venv && source .venv/bin/activate
pip install -e ".[dev]"
uvicorn himari_x402_sidecar.app:app --reload --port 8402
```

## Tests

```sh
pip install -e ".[dev]"
pytest
```

`tests/fixtures/permit2_nile_vector_2026_09_12.json` is a permit the hosted
facilitator verified **and** settled on nile (tx `f026a093…`). It pins the
typed-data shape, the EIP-712 digest and the payer recovery; reuse it to
check any other signer implementation (Dart, Rust).

## Facilitator readiness watch

A background loop (`facilitator_watch.py`, every `X402_FACILITATOR_PROBE_INTERVAL_SEC`,
default 300s) checks `GET /supported` (our network, scheme `exact`, `permit2`),
the relayer's Energy/TRX via TronGrid, and our own streak of failed `/settle`
calls (`X402_FACILITATOR_FAILURE_THRESHOLD`, default 3). The verdict is cached
and returned as `facilitator_ok` / `facilitator_reason` on `/payment-request`
(and under `facilitator_watch` in `/health`), so the client can hide approve/pay
while the facilitator is known-down. Nothing on the payment path waits for it.

## Stub vs live

`X402_FACILITATOR_USE_STUB=false` (default): `/submit` runs facilitator
`/verify` + `/settle` for real.

`X402_FACILITATOR_USE_STUB=true` is **dev only**: `/payment-request` emits a
real-shaped envelope marked `_stub: true`, `/submit` acks with
`awaiting-live-wiring` without calling the facilitator, nothing is settled
and the bot is never notified. The sidecar logs a warning at startup and on
every payment request in this mode, and `/health` reports
`facilitator_mode: "stub"`. Never run it in production.

## Layout

```
src/himari_x402_sidecar/
├── app.py                 # FastAPI factory, endpoints, bot notify
├── config.py              # Pydantic Settings (env / .env)
├── facilitator_client.py  # /health /supported /verify /settle (httpx)
├── permit2_builder.py     # TIP-712 envelope + x402 v2 wire shapes
├── utils.py               # Base58 <-> hex, network ids, contract tables
├── models.py              # HTTP boundary models
├── db.py                  # SQLite: payments (nonce, deadline, buyer, ...), webhooks
├── auth.py                # HMAC helpers
└── migrations/            # 001_init.sql, 002_permit2.sql
```

## License

AGPL-3.0-or-later (matches upstream baibot license).
