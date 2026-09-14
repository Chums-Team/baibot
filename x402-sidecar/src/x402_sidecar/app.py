"""FastAPI app — full endpoint surface.

Endpoints:
- GET  /health                    — liveness + config echo
- POST /payment-request           — bot asks: build a signable Permit2 envelope
- POST /payment-request/submit    — client returns the signature; sidecar runs
                                    facilitator /verify + /settle and notifies the bot
- POST /webhook/settlement        — legacy push path (v2 facilitator does not push)
- GET  /status/{payment_id}       — read-back state
"""
from __future__ import annotations

import logging
import sqlite3
from contextlib import asynccontextmanager
from datetime import datetime, timezone
from typing import Any, AsyncIterator
from uuid import UUID

import httpx
from fastapi import Depends, FastAPI, HTTPException, Request, status
from fastapi.responses import JSONResponse

from . import auth, db
from .config import Settings, get_settings
from .facilitator_client import FacilitatorClient, FacilitatorError
from .facilitator_watch import FacilitatorWatch
from .models import (
    InternalSettledNotify,
    PaymentRequestIn,
    PaymentRequestOut,
    PaymentStatusOut,
    PaymentSubmitIn,
    PaymentSubmitOut,
    SettlementWebhookIn,
    SettlementWebhookOut,
)
from .permit2_builder import (
    build_facilitator_body,
    build_payment_requirements,
    build_permit2_authorization,
    normalize_signature_hex,
    typed_data_mismatches,
    usd_to_micros,
)
from .utils import (
    TronAddressError,
    canonical_network,
    exact_proxy_for,
    legacy_network_name,
    normalize_to_base58,
    normalize_to_evm_hex,
    permit2_for,
    usdt_contract_for,
)

logger = logging.getLogger(__name__)


# ---- Lifespan: open DB + facilitator client; close on shutdown ----


def _open_components(settings: Settings) -> tuple[sqlite3.Connection, FacilitatorClient]:
    return (db.open_db(settings.db_path), FacilitatorClient(settings))


@asynccontextmanager
async def lifespan(app: FastAPI) -> AsyncIterator[None]:
    settings = get_settings()
    conn, client = _open_components(settings)
    app.state.conn = conn
    app.state.facilitator = client
    app.state.notify_client = httpx.AsyncClient(timeout=httpx.Timeout(10.0, connect=5.0))
    watch = FacilitatorWatch(
        facilitator_get_json=client._get_json,
        network=settings.network,
        tron_rpc_url=settings.tron_rpc_url,
        probe_interval_sec=settings.facilitator_probe_interval_sec,
        min_settles_headroom=settings.facilitator_min_settles_headroom,
        failure_threshold=settings.facilitator_failure_threshold,
    )
    app.state.watch = watch
    # Stub mode never settles, so readiness is meaningless there (stays None).
    if not settings.facilitator_use_stub:
        watch.start()
    try:
        yield
    finally:
        await watch.stop()
        await client.aclose()
        await app.state.notify_client.aclose()
        try:
            conn.close()
        except Exception:
            logger.warning("sqlite close failed", exc_info=True)


# ---- App factory ----


def create_app() -> FastAPI:
    settings = get_settings()
    logging.basicConfig(
        level=settings.log_level,
        format="%(asctime)s %(levelname)-7s %(name)s :: %(message)s",
    )

    if settings.facilitator_use_stub:
        logger.warning(
            "STARTUP: X402 facilitator running in STUB mode "
            "(X402_FACILITATOR_USE_STUB=true). /payment-request/submit will "
            "ack without calling the facilitator; settlements WILL NOT happen."
        )

    app = FastAPI(
        title="x402 sidecar",
        version="0.2.0",
        description=(
            "Bridges the Rust bot and the Flutter client to an x402 v2 "
            "facilitator (TRON `exact` scheme via Permit2). Holds the facilitator "
            "API key and the payment_id ↔ nonce mapping."
        ),
        lifespan=lifespan,
    )

    # ===== CORS =====
    # Web clients call `POST /payment-request/submit` cross-origin; the
    # browser preflight needs this. Bot traffic is server-to-server.
    from fastapi.middleware.cors import CORSMiddleware

    app.add_middleware(
        CORSMiddleware,
        allow_origin_regex=r"^https?://([^/]+\.)?chums\.chat$"
        r"|^https?://([^/]+\.)?tron\.mx$"
        r"|^http://10\.79\.144\.226(:\d+)?$"
        r"|^http://localhost(:\d+)?$"
        r"|^http://127\.0\.0\.1(:\d+)?$",
        allow_methods=["GET", "POST", "OPTIONS"],
        allow_headers=["*"],
        allow_credentials=False,
        max_age=3600,
    )

    # ===== /health =====
    @app.get("/health", tags=["meta"])
    async def health(request: Request, s: Settings = Depends(get_settings)) -> dict[str, object]:
        return {
            "ok": True,
            "version": "0.2.0",
            "facilitator_url": s.facilitator_url,
            "network": s.network,
            "network_name": legacy_network_name(s.network),
            "token": s.token,
            "payment_scheme": s.payment_scheme,
            "asset_transfer_method": "permit2",
            "permit2_contract": permit2_for(s.network),
            "exact_proxy_contract": exact_proxy_for(s.network),
            "asset_contract": usdt_contract_for(s.network),
            "max_timeout_seconds": s.max_timeout_seconds,
            "agent_wallet": s.agent_wallet,
            "bot_notify_url": s.bot_notify_url,
            "facilitator_api_key_present": bool(s.facilitator_api_key),
            "facilitator_api_key_length": len(s.facilitator_api_key) if s.facilitator_api_key else 0,
            "internal_secret_present": bool(s.internal_secret),
            "facilitator_mode": "stub" if s.facilitator_use_stub else "live",
            # Absent when the lifespan has not run (bare TestClient without `with`).
            "facilitator_watch": (
                request.app.state.watch.status.as_dict()
                if getattr(request.app.state, "watch", None) is not None
                else None
            ),
        }

    # ===== POST /payment-request =====
    @app.post("/payment-request", response_model=PaymentRequestOut, tags=["payments"])
    async def create_payment_request(
        body: PaymentRequestIn,
        request: Request,
        s: Settings = Depends(get_settings),
    ) -> PaymentRequestOut:
        payment_id = uuid7()
        facilitator: FacilitatorClient = request.app.state.facilitator
        try:
            build = facilitator.build_payment_request(
                amount_usd=body.amount_usd,
                room_id=body.room_id,
                payment_id=payment_id,
            )
        except FacilitatorError as e:
            logger.error("payment-request build failed: %s", e)
            raise HTTPException(status_code=status.HTTP_500_INTERNAL_SERVER_ERROR, detail=str(e))

        with db.transaction(request.app.state.conn) as conn:
            db.insert_payment(
                conn,
                payment_id=payment_id,
                room_id=body.room_id,
                user_mxid=body.user_mxid,
                amount_usd=body.amount_usd,
                correlation_id=body.correlation_id,
                expires_at=build.expires_at,
                nonce=build.nonce,
                deadline=build.deadline,
                network=build.network,
            )
        logger.info(
            "payment-request created: payment_id=%s room=%s amount_usd=%.6f nonce=%s",
            payment_id, body.room_id, body.amount_usd, build.nonce,
        )
        watch_obj = getattr(request.app.state, "watch", None)
        watch_status = watch_obj.status if watch_obj is not None else None
        return PaymentRequestOut(
            payment_id=payment_id,
            expires_at=build.expires_at,
            permit_payload=build.permit_payload,
            payment_requirements=build.payment_requirements,
            facilitator_url=s.facilitator_url,
            scheme=s.payment_scheme,
            permit_contract=build.permit2_contract,
            proxy_contract=build.proxy_contract,
            network=build.network,
            token=s.token,
            pay_to=s.agent_wallet,
            facilitator_ok=watch_status.ok if watch_status else None,
            facilitator_reason=watch_status.reason if watch_status else None,
        )

    # ===== POST /payment-request/submit =====
    @app.post("/payment-request/submit", tags=["payments"])
    async def submit_payment_signature(
        body: PaymentSubmitIn,
        request: Request,
        s: Settings = Depends(get_settings),
    ) -> JSONResponse:
        """Receive the client's signature and drive verify → settle → notify.

        Status codes:
          200 settled / already-settled / awaiting-live-wiring (stub)
          400 malformed signature / address / envelope mismatch
          402 facilitator rejected (verify-failed | settle-failed) — row stays pending
          404 unknown payment_id, 410 expired, 502 facilitator transport error
        """
        conn = request.app.state.conn
        payment = db.get_payment(conn, body.payment_id)
        if payment is None:
            logger.warning(
                "submit rejected: unknown payment_id=%s (buyer=%s)",
                body.payment_id, _redact_addr(body.buyer_address),
            )
            raise HTTPException(status_code=status.HTTP_404_NOT_FOUND, detail="unknown payment_id")

        if payment["status"] == "settled":
            logger.info(
                "submit ignored: payment_id=%s already settled (tx=%s)",
                body.payment_id, payment.get("tx_hash"),
            )
            return _submit_response(
                200,
                PaymentSubmitOut(
                    accepted=True,
                    payment_id=body.payment_id,
                    next_step="already-settled",
                    tx_hash=payment.get("tx_hash") or None,
                ),
            )

        if payment["expires_at"] < datetime.now(timezone.utc):
            logger.warning(
                "submit rejected: payment_id=%s expired at %s",
                body.payment_id, payment["expires_at"].isoformat(),
            )
            raise HTTPException(
                status_code=status.HTTP_410_GONE,
                detail="payment_id expired — request a fresh permit",
            )

        try:
            signature = normalize_signature_hex(body.signature_hex)
        except ValueError as e:
            raise HTTPException(status_code=status.HTTP_400_BAD_REQUEST, detail=f"bad signature_hex: {e}")
        try:
            buyer_hex = normalize_to_evm_hex(body.buyer_address)
            buyer_b58 = normalize_to_base58(body.buyer_address)
        except TronAddressError as e:
            raise HTTPException(status_code=status.HTTP_400_BAD_REQUEST, detail=f"bad buyer_address: {e}")

        logger.info(
            "payment-request submit: payment_id=%s buyer=%s signature_len=%d (hex chars) "
            "room=%s amount_usd=%.6f mode=%s",
            body.payment_id, _redact_addr(buyer_b58), len(signature) - 2,
            payment["room_id"], payment["amount_usd"],
            "stub" if s.facilitator_use_stub else "live",
        )

        # Rebuild the authorization from OUR record, never from client input.
        try:
            requirements, authorization = _rebuild_wire_objects(s, payment, buyer_hex)
        except (TronAddressError, KeyError, TypeError) as e:
            logger.error("submit: cannot rebuild authorization for %s: %s", body.payment_id, e)
            raise HTTPException(
                status_code=status.HTTP_409_CONFLICT,
                detail=f"payment record incomplete for v2 settle ({e}); request a fresh permit",
            )

        if body.permit_payload is not None:
            problems = typed_data_mismatches(body.permit_payload, authorization)
            if problems:
                logger.warning(
                    "submit rejected: payment_id=%s envelope mismatch: %s",
                    body.payment_id, "; ".join(problems),
                )
                raise HTTPException(
                    status_code=status.HTTP_400_BAD_REQUEST,
                    detail="permit_payload does not match the issued envelope: " + "; ".join(problems),
                )

        if s.facilitator_use_stub:
            return _submit_response(
                200,
                PaymentSubmitOut(
                    accepted=True, payment_id=body.payment_id, next_step="awaiting-live-wiring"
                ),
            )

        facilitator: FacilitatorClient = request.app.state.facilitator
        wire_body = build_facilitator_body(
            requirements=requirements, authorization=authorization, signature_hex=signature
        )

        try:
            verify = await facilitator.verify(wire_body)
        except FacilitatorError as e:
            logger.error("submit failed: payment_id=%s facilitator /verify error: %s", body.payment_id, e)
            raise HTTPException(status_code=status.HTTP_502_BAD_GATEWAY, detail=f"facilitator verify error: {e}")

        if not verify.is_valid:
            reason = verify.invalid_reason or "invalid"
            with db.transaction(conn) as tx_conn:
                db.record_rejection(tx_conn, body.payment_id, error_reason=reason, buyer=buyer_b58)
            logger.warning(
                "verify-failed: payment_id=%s reason=%s message=%s — row left pending",
                body.payment_id, reason, verify.invalid_message or "<none>",
            )
            return _submit_response(
                402,
                PaymentSubmitOut(
                    accepted=False,
                    payment_id=body.payment_id,
                    next_step="verify-failed",
                    error_reason=reason,
                    error_message=verify.invalid_message,
                    payer=verify.payer,
                ),
            )

        watch: FacilitatorWatch = request.app.state.watch
        try:
            settle = await facilitator.settle(wire_body)
        except FacilitatorError as e:
            watch.record_settle_outcome(False, str(e))
            logger.error("submit failed: payment_id=%s facilitator /settle error: %s", body.payment_id, e)
            raise HTTPException(status_code=status.HTTP_502_BAD_GATEWAY, detail=f"facilitator settle error: {e}")

        watch.record_settle_outcome(settle.success, settle.error_reason)
        if not settle.success:
            reason = settle.error_reason or "settle_failed"
            with db.transaction(conn) as tx_conn:
                db.record_rejection(tx_conn, body.payment_id, error_reason=reason, buyer=buyer_b58)
            logger.warning(
                "settle-failed: payment_id=%s reason=%s message=%s tx=%s — row left pending",
                body.payment_id, reason, settle.error_message or "<none>",
                settle.transaction or "<none>",
            )
            return _submit_response(
                402,
                PaymentSubmitOut(
                    accepted=False,
                    payment_id=body.payment_id,
                    next_step="settle-failed",
                    tx_hash=settle.transaction,
                    error_reason=reason,
                    error_message=settle.error_message,
                    payer=settle.payer,
                ),
            )

        tx_hash = settle.transaction or ""
        if not tx_hash:
            logger.warning(
                "facilitator /settle returned success=True with empty transaction hash "
                "for payment_id=%s — marking settled anyway, operator review recommended",
                body.payment_id,
            )
        settled_at = datetime.now(timezone.utc)
        with db.transaction(conn) as tx_conn:
            db.mark_settled(tx_conn, body.payment_id, tx_hash=tx_hash, settled_at=settled_at, buyer=buyer_b58)
        logger.info(
            "settle-confirmed: payment_id=%s tx=%s network=%s payer=%s",
            body.payment_id, tx_hash or "<empty>", settle.network or "<unknown>",
            settle.payer or "<unknown>",
        )

        # Synchronous bot notify. The v2 facilitator never pushes webhooks,
        # so this is THE path that credits the room. Idempotent on the bot
        # side (409 on duplicate payment_id).
        await _notify_bot(
            request,
            s,
            InternalSettledNotify(
                payment_id=body.payment_id,
                room_id=payment["room_id"],
                user_mxid=payment["user_mxid"],
                amount_usd=payment["amount_usd"],
                tx_hash=tx_hash,
                block_number=0,
                settled_at=settled_at,
                correlation_id=payment.get("correlation_id"),
            ),
            path_label="sync settle path",
        )

        return _submit_response(
            200,
            PaymentSubmitOut(
                accepted=True,
                payment_id=body.payment_id,
                next_step="settled",
                tx_hash=tx_hash or None,
                payer=settle.payer,
            ),
        )

    # ===== POST /webhook/settlement (legacy push path) =====
    @app.post("/webhook/settlement", response_model=SettlementWebhookOut, tags=["payments"])
    async def settlement_webhook(
        request: Request,
        s: Settings = Depends(get_settings),
    ) -> SettlementWebhookOut:
        raw_body = await request.body()
        sig_header = request.headers.get("X-Facilitator-Signature")
        if not auth.verify_facilitator_header(s.facilitator_webhook_secret, raw_body, sig_header):
            logger.warning(
                "settlement webhook rejected: bad/absent X-Facilitator-Signature "
                "(sig_present=%s, body_len=%d)",
                bool(sig_header), len(raw_body),
            )
            raise HTTPException(status_code=status.HTTP_401_UNAUTHORIZED, detail="bad facilitator signature")

        try:
            body = SettlementWebhookIn.model_validate_json(raw_body)
        except Exception as e:
            logger.warning("settlement webhook malformed body: %s", e)
            raise HTTPException(status_code=status.HTTP_400_BAD_REQUEST, detail=f"malformed body: {e}")

        if db.webhook_seen(request.app.state.conn, body.webhook_id):
            logger.info("settlement webhook duplicate ignored: %s", body.webhook_id)
            return SettlementWebhookOut(ok=True)

        payment = db.get_payment(request.app.state.conn, body.payment_id)
        if payment is None:
            logger.warning(
                "settlement webhook for unknown payment_id=%s (webhook_id=%s)",
                body.payment_id, body.webhook_id,
            )
            raise HTTPException(status_code=status.HTTP_404_NOT_FOUND, detail="unknown payment_id")

        with db.transaction(request.app.state.conn) as conn:
            db.record_webhook(conn, webhook_id=body.webhook_id, payment_id=body.payment_id)
            db.mark_settled(conn, body.payment_id, tx_hash=body.tx_hash, settled_at=body.settled_at)

        await _notify_bot(
            request,
            s,
            InternalSettledNotify(
                payment_id=body.payment_id,
                room_id=payment["room_id"],
                user_mxid=payment["user_mxid"],
                amount_usd=body.amount_paid_usd,
                tx_hash=body.tx_hash,
                block_number=body.block_number,
                settled_at=body.settled_at,
                correlation_id=payment.get("correlation_id"),
            ),
            path_label="webhook path",
        )
        return SettlementWebhookOut(ok=True)

    # ===== GET /status/{payment_id} =====
    @app.get("/status/{payment_id}", response_model=PaymentStatusOut, tags=["payments"])
    async def get_status(payment_id: UUID, request: Request) -> PaymentStatusOut:
        payment = db.get_payment(request.app.state.conn, payment_id)
        if payment is None:
            raise HTTPException(status_code=404, detail="unknown payment_id")
        if payment["status"] == "pending" and payment["expires_at"] < datetime.now(timezone.utc):
            payment["status"] = "expired"
        return PaymentStatusOut(**payment)

    return app


# ---- helpers ----


def _submit_response(status_code: int, out: PaymentSubmitOut) -> JSONResponse:
    return JSONResponse(status_code=status_code, content=out.model_dump(mode="json"))


def _rebuild_wire_objects(
    s: Settings, payment: dict[str, Any], buyer_hex: str
) -> tuple[dict[str, Any], dict[str, Any]]:
    """Reconstruct `paymentRequirements` + `permit2Authorization` from the
    stored payment row + current config. Raises KeyError/TypeError when the
    row predates the v2 columns (nonce/deadline/network)."""
    network = canonical_network(payment["network"] or s.network)
    nonce = int(payment["nonce"])  # KeyError/TypeError on legacy rows
    deadline = int(payment["deadline"])
    amount_micros = usd_to_micros(payment["amount_usd"])
    usdt_b58 = usdt_contract_for(network)
    requirements = build_payment_requirements(
        network=network,
        amount_micros=amount_micros,
        asset_b58=usdt_b58,
        pay_to_b58=s.agent_wallet,
        max_timeout_seconds=s.max_timeout_seconds,
    )
    authorization = build_permit2_authorization(
        buyer_hex=buyer_hex,
        token_hex=normalize_to_evm_hex(usdt_b58),
        amount_micros=amount_micros,
        proxy_hex=normalize_to_evm_hex(exact_proxy_for(network)),
        nonce=nonce,
        deadline=deadline,
        pay_to_hex=normalize_to_evm_hex(s.agent_wallet),
    )
    return requirements, authorization


async def _notify_bot(
    request: Request, s: Settings, notify: InternalSettledNotify, *, path_label: str
) -> None:
    """HMAC-signed POST to the bot. Failures are logged, never raised: the
    payment is already settled on-chain; a lost notify is an ops
    reconciliation, not a reason to fail the client call."""
    notify_body = notify.model_dump_json().encode("utf-8")
    signature = auth.sign(s.internal_secret, notify_body)
    try:
        r = await request.app.state.notify_client.post(
            s.bot_notify_url,
            content=notify_body,
            headers={"Content-Type": "application/json", "X-Internal-Signature": signature},
        )
        if r.status_code >= 400 and r.status_code != 409:
            logger.error(
                "bot notify failed (%s): status=%d body=%s payment_id=%s",
                path_label, r.status_code, r.text[:300], notify.payment_id,
            )
        else:
            logger.info(
                "bot notify ok (%s): payment_id=%s status=%d",
                path_label, notify.payment_id, r.status_code,
            )
    except Exception as e:  # httpx.HTTPError or anything else
        logger.error("bot notify network error (%s): %s payment_id=%s", path_label, e, notify.payment_id)


def _redact_addr(addr: str) -> str:
    """Last-4-chars redaction: wallet addresses are PII in logs."""
    if not addr:
        return "<empty>"
    if len(addr) <= 4:
        return addr
    return "***" + addr[-4:]


def uuid7() -> UUID:
    """UUID v7 generator (stdlib `uuid` lacks v7 in Python 3.12)."""
    import os
    import time

    ts_ms = int(time.time() * 1000) & ((1 << 48) - 1)
    rand = int.from_bytes(os.urandom(10), "big")
    rand_a = rand & 0xFFF
    rand_b = (rand >> 12) & ((1 << 62) - 1)
    int_value = (ts_ms << 80) | (0x7 << 76) | (rand_a << 64) | (0b10 << 62) | rand_b
    return UUID(int=int_value)


# Module-level instance for `uvicorn x402_sidecar.app:app`.
app = create_app()
