"""HTTP client to an x402 **v2** facilitator (BofAI hosted or self-host).

This module is the single place the facilitator wire format is touched.
Endpoints used (per `external-repos/BofAI--x402-facilitator/src/server.ts`):

    GET  /health      -> {"status": "ok"}
    GET  /supported   -> {"kinds": [...], "extensions": [...], "signers": {...}}
    POST /verify      -> VerifyResponse  {isValid, invalidReason?, invalidMessage?, payer?}
    POST /settle      -> SettleResponse  {success, transaction, network, errorReason?,
                                          errorMessage?, payer?}

Both POSTs take `{x402Version, paymentPayload, paymentRequirements}` and
are authenticated with `X-API-KEY` (seller-scoped settlement feed; the
anonymous tier is rate-limited harder).

`build_payment_request` is **pure** (no network): the v2 `exact`
scheme has no fee-quote step, everything needed to build the signable
envelope is local config + randomness.
"""
from __future__ import annotations

import logging
from dataclasses import dataclass
from datetime import datetime
from typing import Any
from uuid import UUID

import httpx

from .config import Settings
from .permit2_builder import (
    build_payment_requirements,
    build_permit2_typed_data,
    make_deadline,
    make_nonce,
    usd_to_micros,
)
from .utils import (
    TronAddressError,
    canonical_network,
    chain_id_for,
    exact_proxy_for,
    normalize_to_evm_hex,
    permit2_for,
    usdt_contract_for,
)

logger = logging.getLogger(__name__)

USER_AGENT = "himari-x402-sidecar/0.2"


class FacilitatorError(Exception):
    """Transport / protocol failure talking to the facilitator (not a
    payment-level rejection — those come back as result objects)."""


@dataclass(frozen=True)
class VerifyResult:
    is_valid: bool
    invalid_reason: str | None
    invalid_message: str | None
    payer: str | None


@dataclass(frozen=True)
class SettleResult:
    """Decoded `/settle` response. `success=False` is an expected outcome
    (on-chain revert, allowance missing, ...) and is returned, not raised."""

    success: bool
    transaction: str | None
    network: str | None
    error_reason: str | None
    error_message: str | None
    payer: str | None


@dataclass(frozen=True)
class PaymentRequestBuild:
    """Everything `/payment-request` needs to persist + hand to the bot."""

    permit_payload: dict[str, Any]
    payment_requirements: dict[str, Any]
    expires_at: datetime
    nonce: int
    deadline: int
    network: str
    permit2_contract: str
    proxy_contract: str
    asset_contract: str


class FacilitatorClient:
    def __init__(self, settings: Settings):
        self._settings = settings
        self._client = httpx.AsyncClient(
            base_url=settings.facilitator_url,
            headers={
                "X-API-KEY": settings.facilitator_api_key,
                "User-Agent": USER_AGENT,
            },
            # /settle blocks until the facilitator sees the receipt
            # (TRON ~3-6s, upstream SDK default timeout 90s).
            timeout=httpx.Timeout(95.0, connect=5.0),
        )

    async def aclose(self) -> None:
        await self._client.aclose()

    # ---- meta ----

    async def health(self) -> bool:
        try:
            r = await self._client.get("/health")
            return r.status_code == 200
        except httpx.HTTPError as e:
            logger.warning("facilitator /health failed: %s", e)
            return False

    async def supported(self) -> dict[str, Any]:
        return await self._get_json("/supported")

    # ---- payment request (pure) ----

    def build_payment_request(
        self,
        *,
        amount_usd: float,
        room_id: str,
        payment_id: UUID,
    ) -> PaymentRequestBuild:
        s = self._settings
        try:
            network = canonical_network(s.network)
            chain_id = chain_id_for(network)
            permit2_b58 = permit2_for(network)
            proxy_b58 = exact_proxy_for(network)
            usdt_b58 = usdt_contract_for(network)
        except TronAddressError as e:
            raise FacilitatorError(
                f"cannot build a payment request for network {s.network!r}: {e}"
            ) from None

        nonce = make_nonce()
        deadline, expires_at = make_deadline(max_timeout_seconds=s.max_timeout_seconds)
        amount_micros = usd_to_micros(amount_usd)

        if s.facilitator_use_stub:
            # Loud marker: the envelope is real-shaped, but /submit will not
            # forward to the facilitator in stub mode.
            logger.warning(
                "STUB MODE: build_payment_request emitting envelope for %s; "
                "/submit will ack without settling (X402_FACILITATOR_USE_STUB=true)",
                payment_id,
            )

        permit_payload = build_permit2_typed_data(
            chain_id=chain_id,
            permit2_hex=normalize_to_evm_hex(permit2_b58),
            proxy_hex=normalize_to_evm_hex(proxy_b58),
            token_hex=normalize_to_evm_hex(usdt_b58),
            amount_micros=amount_micros,
            pay_to_hex=normalize_to_evm_hex(s.agent_wallet),
            nonce=nonce,
            deadline=deadline,
            include_stub_marker=s.facilitator_use_stub,
        )
        requirements = build_payment_requirements(
            network=network,
            amount_micros=amount_micros,
            asset_b58=usdt_b58,
            pay_to_b58=s.agent_wallet,
            max_timeout_seconds=s.max_timeout_seconds,
        )
        logger.info(
            "build_payment_request: payment_id=%s amount_usd=%.6f room_id=%s "
            "network=%s permit2=%s proxy=%s payTo=%s deadline=%d",
            payment_id, amount_usd, room_id, network, permit2_b58, proxy_b58,
            s.agent_wallet, deadline,
        )
        return PaymentRequestBuild(
            permit_payload=permit_payload,
            payment_requirements=requirements,
            expires_at=expires_at,
            nonce=nonce,
            deadline=deadline,
            network=network,
            permit2_contract=permit2_b58,
            proxy_contract=proxy_b58,
            asset_contract=usdt_b58,
        )

    # ---- verify / settle ----

    async def verify(self, body: dict[str, Any]) -> VerifyResult:
        data = await self._post_json("/verify", body, marker="isValid")
        result = VerifyResult(
            is_valid=bool(data.get("isValid")),
            invalid_reason=data.get("invalidReason"),
            invalid_message=data.get("invalidMessage"),
            payer=data.get("payer"),
        )
        logger.info(
            "facilitator /verify: isValid=%s reason=%s payer=%s",
            result.is_valid, result.invalid_reason or "<none>", result.payer or "<none>",
        )
        return result

    async def settle(self, body: dict[str, Any]) -> SettleResult:
        data = await self._post_json("/settle", body, marker="success")
        result = SettleResult(
            success=bool(data.get("success")),
            transaction=data.get("transaction") or None,
            network=data.get("network"),
            error_reason=data.get("errorReason") or data.get("error_reason"),
            error_message=data.get("errorMessage") or data.get("error_message"),
            payer=data.get("payer"),
        )
        logger.info(
            "facilitator /settle: success=%s tx=%s network=%s reason=%s",
            result.success, result.transaction or "<none>",
            result.network or "<none>", result.error_reason or "<none>",
        )
        return result

    # ---- transport ----

    async def _get_json(self, path: str) -> dict[str, Any]:
        try:
            r = await self._client.get(path)
        except httpx.HTTPError as e:
            raise FacilitatorError(f"facilitator {path} network error: {e}") from e
        if r.status_code != 200:
            raise FacilitatorError(
                f"facilitator {path} returned HTTP {r.status_code}: {r.text[:300]}"
            )
        try:
            data = r.json()
        except ValueError as e:
            raise FacilitatorError(f"facilitator {path} returned non-JSON body: {e}") from e
        if not isinstance(data, dict):
            raise FacilitatorError(f"facilitator {path} returned non-object JSON")
        return data

    async def _post_json(
        self, path: str, body: dict[str, Any], *, marker: str
    ) -> dict[str, Any]:
        """POST and decode. A JSON object carrying `marker` is returned even
        on 4xx (the facilitator answers protocol rejections as
        `{isValid:false,...}` / `{success:false,...}` with 400). Anything
        else (transport error, 5xx without a decodable body, HTML) raises."""
        try:
            r = await self._client.post(path, json=body)
        except httpx.HTTPError as e:
            logger.error("facilitator %s network error: %s", path, e)
            raise FacilitatorError(f"facilitator {path} network error: {e}") from e

        data: Any = None
        try:
            data = r.json()
        except ValueError:
            data = None

        if isinstance(data, dict) and marker in data:
            if r.status_code != 200:
                logger.warning(
                    "facilitator %s answered HTTP %d with protocol body: %s",
                    path, r.status_code, str(data)[:300],
                )
            return data

        preview = r.text[:300] if r.text else "<empty>"
        logger.error("facilitator %s rejected: status=%d body=%s", path, r.status_code, preview)
        raise FacilitatorError(
            f"facilitator {path} returned HTTP {r.status_code}: {preview}"
        )
