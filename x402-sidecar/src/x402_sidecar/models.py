"""Pydantic models for sidecar HTTP boundaries.

Boundaries:
- bot ↔ sidecar (POST /payment-request, GET /status, POST /internal/notify out)
- client ↔ sidecar (POST /payment-request/submit)
- facilitator ↔ sidecar (POST /webhook/settlement — legacy, unused by v2)

Kept as separate types so a wire-shape change on one side doesn't
silently affect the other.
"""
from __future__ import annotations

from datetime import datetime
from typing import Any
from uuid import UUID

from pydantic import BaseModel, ConfigDict, Field, field_validator


# ===== bot → sidecar =====


class PaymentRequestIn(BaseModel):
    """POST /payment-request body. Bot asks for a payment requirement."""

    model_config = ConfigDict(extra="forbid")

    room_id: str = Field(..., description="Matrix room id, e.g. !abc:example.com")
    user_mxid: str = Field(..., description="@user:server who triggered the call")
    amount_usd: float = Field(..., gt=0, description="Required top-up in USD")
    correlation_id: UUID | None = Field(
        default=None,
        description="Optional — bot's own LLM-call correlation. Echoed back on settlement.",
    )

    @field_validator("amount_usd")
    @classmethod
    def round_to_six(cls, v: float) -> float:
        return round(v, 6)


class PaymentRequestOut(BaseModel):
    """POST /payment-request response. Bot embeds this in cc.chums.x402_request.

    Field semantics (x402 v2, TRON `exact` via Permit2):
      * `permit_payload`   — TIP-712 envelope the client signs as-is
                             (`primaryType` = `PermitWitnessTransferFrom`).
      * `permit_contract`  — **Permit2** contract (Base58). This is both the
                             TRC-20 `approve` spender and the typed-data
                             `verifyingContract`. Name kept for wire
                             compatibility with the bot/client.
      * `proxy_contract`   — `x402ExactPermit2Proxy` (Base58): the `spender`
                             inside the signed message; the client pins it.
      * `payment_requirements` — v2 `PaymentRequirements` we will send to
                             the facilitator; informational for the client.
      * `network`          — hex CAIP-2 id (`tron:0xcd8690dc`).
    """

    model_config = ConfigDict(extra="forbid")

    payment_id: UUID
    expires_at: datetime
    permit_payload: dict[str, Any] = Field(
        ..., description="TIP-712 typed-data structure to sign client-side"
    )
    payment_requirements: dict[str, Any] = Field(
        ..., description="x402 v2 PaymentRequirements (scheme/network/amount/asset/payTo/...)"
    )
    facilitator_url: str
    scheme: str
    permit_contract: str = Field(..., description="Permit2 contract (Base58): approve spender")
    proxy_contract: str = Field(..., description="x402ExactPermit2Proxy (Base58): message.spender")
    network: str
    token: str
    pay_to: str = Field(..., description="agent_wallet TRON address (Base58)")
    facilitator_ok: bool | None = Field(
        default=None,
        description=(
            "Cached readiness verdict of the background facilitator watch: "
            "true = go, false = the client should not offer approve/pay, "
            "null = unknown (not probed yet / stub mode)."
        ),
    )
    facilitator_reason: str | None = Field(
        default=None, description="Why facilitator_ok is false (stable-ish code + detail)."
    )


# ===== client → sidecar =====


class PaymentSubmitIn(BaseModel):
    """POST /payment-request/submit body.

    The client signs the TIP-712 envelope from `/payment-request` and
    posts back the signature together with the payer address. The sidecar
    rebuilds `permit2Authorization` from its own record (never from
    client input), optionally cross-checks the echoed envelope, then
    runs `/verify` + `/settle` on the facilitator.
    """

    model_config = ConfigDict(extra="forbid")

    payment_id: UUID
    buyer_address: str = Field(
        ...,
        description="TRON Base58 (T...) or 0x-hex EVM address of the signer (permit owner).",
    )
    signature_hex: str = Field(
        ...,
        description="65-byte recoverable secp256k1 signature, hex (0x prefix optional).",
    )
    permit_payload: dict[str, Any] | None = Field(
        default=None,
        description=(
            "Optional echo of the signed envelope. When present its `message` "
            "must match what the sidecar built (400 otherwise)."
        ),
    )


class PaymentSubmitOut(BaseModel):
    """POST /payment-request/submit response.

    HTTP 200 → `next_step` in {settled, already-settled, awaiting-live-wiring}.
    HTTP 402 → `accepted=false`, `next_step` in {verify-failed, settle-failed},
               `error_reason` = facilitator's stable code, `error_message`
               = optional diagnostic. Row stays pending; the user may retry.
    """

    model_config = ConfigDict(extra="forbid")

    accepted: bool = True
    payment_id: UUID
    next_step: str
    tx_hash: str | None = None
    error_reason: str | None = None
    error_message: str | None = None
    payer: str | None = Field(
        default=None, description="Payer address as recovered by the facilitator (0x-hex)."
    )


# ===== facilitator → sidecar (legacy webhook; v2 facilitator does not push) =====


class SettlementWebhookIn(BaseModel):
    model_config = ConfigDict(extra="ignore")

    payment_id: UUID
    tx_hash: str
    block_number: int
    amount_paid_usd: float
    sender_wallet: str
    settled_at: datetime
    webhook_id: str


class SettlementWebhookOut(BaseModel):
    model_config = ConfigDict(extra="forbid")

    ok: bool = True


# ===== sidecar → bot (internal notify) =====


class InternalSettledNotify(BaseModel):
    """POST {bot_notify_url} body. Sidecar calls this after a successful settle.

    The matching `X-Internal-Signature: <hmac-sha256(body, X402_INTERNAL_SECRET)>`
    header MUST be set by the caller; the bot validates before INSERTing topup.
    """

    model_config = ConfigDict(extra="forbid")

    payment_id: UUID
    room_id: str
    user_mxid: str
    amount_usd: float
    tx_hash: str
    block_number: int
    settled_at: datetime
    correlation_id: UUID | None = None


# ===== status read-back =====


class PaymentStatusOut(BaseModel):
    model_config = ConfigDict(extra="forbid")

    payment_id: UUID
    status: str = Field(..., description="pending | settled | expired | failed")
    room_id: str
    user_mxid: str
    amount_usd: float
    correlation_id: UUID | None = None
    tx_hash: str | None = None
    expires_at: datetime
    settled_at: datetime | None = None
    created_at: datetime
    nonce: str | None = None
    deadline: int | None = None
    network: str | None = None
    buyer: str | None = None
    error_reason: str | None = None
