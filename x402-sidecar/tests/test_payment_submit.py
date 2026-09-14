"""`POST /payment-request/submit` (client → sidecar) — stub + live modes.

FastAPI TestClient against the real ASGI app, in-memory DB, facilitator
mocked at the httpx transport layer.

Status contract:
  200  settled | already-settled | awaiting-live-wiring (stub)
  400  bad signature / bad address / envelope mismatch
  402  verify-failed | settle-failed (row stays pending)
  404  unknown payment_id
  410  expired
  502  facilitator transport error
"""
from __future__ import annotations

import importlib
import json
from datetime import datetime, timedelta, timezone
from uuid import UUID, uuid4

import httpx
import pytest
from fastapi.testclient import TestClient

# Real Base58Check TRON address (nile test buyer from the 2026-09-12 vector).
BUYER_B58 = "TGVvAYNsroS1c25q2gvmoSFkvEdy3Qdmbz"
BUYER_HEX = "0x479f985951446f042d2ddaa4b3013ff909852ba9"
SIG = "0x" + "ab" * 65


def _env(monkeypatch, *, stub: bool):
    monkeypatch.setenv("X402_FACILITATOR_API_KEY", "test-key" * 4)
    monkeypatch.setenv("X402_INTERNAL_SECRET", "secret-32" * 4)
    monkeypatch.setenv("X402_FACILITATOR_WEBHOOK_SECRET", "fac-webhook-32" * 4)
    monkeypatch.setenv("X402_AGENT_WALLET", "TSkeaPMSuaojcCzE7mWqw4xN1awU7NfdfY")
    monkeypatch.setenv("SIDECAR_DB_PATH", ":memory:")
    monkeypatch.setenv("BOT_X402_NOTIFY_URL", "http://bot.test/internal/x402-settled")
    monkeypatch.setenv("X402_FACILITATOR_USE_STUB", "true" if stub else "false")
    monkeypatch.setenv("X402_NETWORK", "tron:0xcd8690dc")
    from himari_x402_sidecar import app as app_mod
    from himari_x402_sidecar import config as config_mod

    importlib.reload(config_mod)
    importlib.reload(app_mod)
    config_mod._settings = None
    return app_mod.create_app()


@pytest.fixture
def app(monkeypatch):
    return _env(monkeypatch, stub=True)


@pytest.fixture
def live_app(monkeypatch):
    return _env(monkeypatch, stub=False)


def _swap_facilitator_transport(app, handler):
    import asyncio

    from himari_x402_sidecar.facilitator_client import USER_AGENT

    fac = app.state.facilitator
    asyncio.run(fac._client.aclose())
    fac._client = httpx.AsyncClient(
        base_url=fac._settings.facilitator_url,
        headers={"X-API-KEY": fac._settings.facilitator_api_key, "User-Agent": USER_AGENT},
        transport=httpx.MockTransport(handler),
    )


def _swap_notify_transport(app, captured: list, status_code: int = 200):
    def handler(request: httpx.Request) -> httpx.Response:
        captured.append({"headers": dict(request.headers), "body": json.loads(request.content)})
        return httpx.Response(status_code, json={"ok": True})

    app.state.notify_client = httpx.AsyncClient(transport=httpx.MockTransport(handler))


def _create_payment(client, amount_usd: float = 0.10) -> dict:
    r = client.post(
        "/payment-request",
        json={"room_id": "!abc:tron.mx", "user_mxid": "@user:tron.mx", "amount_usd": amount_usd},
    )
    assert r.status_code == 200, r.text
    return r.json()


def _submit(client, payment_id, *, buyer=BUYER_B58, sig=SIG, envelope=None):
    body = {"payment_id": payment_id, "buyer_address": buyer, "signature_hex": sig}
    if envelope is not None:
        body["permit_payload"] = envelope
    return client.post("/payment-request/submit", json=body)


def _facilitator(verify=None, settle=None, captured=None):
    def handler(request: httpx.Request) -> httpx.Response:
        if captured is not None:
            captured.append((request.url.path, json.loads(request.content)))
        if request.url.path == "/verify":
            return verify(request) if callable(verify) else httpx.Response(200, json=verify or {"isValid": True, "payer": BUYER_HEX})
        if request.url.path == "/settle":
            return settle(request) if callable(settle) else httpx.Response(
                200,
                json=settle or {"success": True, "transaction": "f026" + "00" * 30, "network": "tron:0xcd8690dc", "payer": BUYER_HEX},
            )
        return httpx.Response(404)

    return handler


# ---------------------------------------------------------------- stub mode


def test_submit_stub_acks_without_forwarding(app):
    with TestClient(app) as client:
        created = _create_payment(client)
        r = _submit(client, created["payment_id"], envelope=created["permit_payload"])
        assert r.status_code == 200, r.text
        body = r.json()
        assert body["accepted"] is True
        assert body["next_step"] == "awaiting-live-wiring"
        assert body["tx_hash"] is None


def test_submit_rejects_unknown_payment_id(app):
    with TestClient(app) as client:
        r = _submit(client, str(uuid4()))
        assert r.status_code == 404


def test_submit_idempotent_on_already_settled(app):
    from himari_x402_sidecar import db

    with TestClient(app) as client:
        created = _create_payment(client)
        pid = UUID(created["payment_id"])
        with db.transaction(app.state.conn) as conn:
            db.mark_settled(conn, pid, tx_hash="deadbeef", settled_at=datetime.now(timezone.utc))
        r = _submit(client, created["payment_id"])
        assert r.status_code == 200
        assert r.json()["next_step"] == "already-settled"
        assert r.json()["tx_hash"] == "deadbeef"


def test_submit_rejects_expired_payment(app):
    from himari_x402_sidecar import db

    with TestClient(app) as client:
        created = _create_payment(client)
        past = datetime.now(timezone.utc) - timedelta(hours=1)
        app.state.conn.execute(
            "UPDATE payments SET expires_at = ? WHERE payment_id = ?",
            (db._iso(past), created["payment_id"]),
        )
        r = _submit(client, created["payment_id"])
        assert r.status_code == 410
        assert "expired" in r.json()["detail"].lower()


@pytest.mark.parametrize("sig", ["ab" * 64, "0x" + "zz" * 65, "", "0x" + "ab" * 66])
def test_submit_rejects_malformed_signature(app, sig):
    with TestClient(app) as client:
        created = _create_payment(client)
        r = _submit(client, created["payment_id"], sig=sig)
        assert r.status_code == 400
        assert "signature" in r.json()["detail"]


def test_submit_rejects_bad_buyer_address(app):
    with TestClient(app) as client:
        created = _create_payment(client)
        r = _submit(client, created["payment_id"], buyer="not-an-address")
        assert r.status_code == 400
        assert "buyer_address" in r.json()["detail"]


def test_submit_accepts_signature_without_0x_and_hex_buyer(app):
    with TestClient(app) as client:
        created = _create_payment(client)
        r = _submit(client, created["payment_id"], buyer=BUYER_HEX, sig="AB" * 65)
        assert r.status_code == 200, r.text


def test_submit_rejects_envelope_mismatch(app):
    """A client echo whose message differs from what we issued → 400
    (e.g. a stale envelope from a previous network)."""
    with TestClient(app) as client:
        created = _create_payment(client)
        tampered = json.loads(json.dumps(created["permit_payload"]))
        tampered["message"]["witness"]["to"] = "0x" + "99" * 20
        tampered["message"]["nonce"] = "1"
        r = _submit(client, created["payment_id"], envelope=tampered)
        assert r.status_code == 400
        detail = r.json()["detail"]
        assert "witness.to" in detail and "nonce" in detail


def test_submit_redacts_buyer_in_logs(app, caplog):
    import logging

    with TestClient(app) as client:
        created = _create_payment(client)
        with caplog.at_level(logging.INFO, logger="himari_x402_sidecar.app"):
            r = _submit(client, created["payment_id"])
        assert r.status_code == 200
        assert not any(BUYER_B58 in rec.message for rec in caplog.records)
        assert any("signature_len=130" in rec.message for rec in caplog.records)


# ---------------------------------------------------------------- live mode


def test_live_happy_path_verify_settle_notify(live_app):
    from himari_x402_sidecar import db

    fac_calls: list = []
    notifies: list = []
    with TestClient(live_app) as client:
        _swap_facilitator_transport(live_app, _facilitator(captured=fac_calls))
        _swap_notify_transport(live_app, notifies)
        created = _create_payment(client, amount_usd=0.01)
        r = _submit(client, created["payment_id"], envelope=created["permit_payload"])
        assert r.status_code == 200, r.text
        body = r.json()
        assert body["accepted"] is True
        assert body["next_step"] == "settled"
        assert body["tx_hash"].startswith("f026")
        assert body["payer"] == BUYER_HEX

        # Order: verify first, then settle, identical bodies.
        assert [p for p, _ in fac_calls] == ["/verify", "/settle"]
        assert fac_calls[0][1] == fac_calls[1][1]
        wire = fac_calls[1][1]
        assert set(wire) == {"x402Version", "paymentPayload", "paymentRequirements"}
        auth = wire["paymentPayload"]["payload"]["permit2Authorization"]
        assert auth["from"] == BUYER_HEX
        # Authorization rebuilt from OUR record matches the issued envelope.
        msg = created["permit_payload"]["message"]
        assert auth["nonce"] == msg["nonce"]
        assert auth["deadline"] == msg["deadline"]
        assert auth["permitted"] == msg["permitted"]
        assert auth["spender"] == msg["spender"]
        assert auth["witness"] == msg["witness"]
        assert wire["paymentRequirements"] == created["payment_requirements"]
        assert wire["paymentPayload"]["payload"]["signature"] == SIG

        row = db.get_payment(live_app.state.conn, UUID(created["payment_id"]))
        assert row["status"] == "settled"
        assert row["tx_hash"].startswith("f026")
        assert row["buyer"] == BUYER_B58
        assert row["nonce"] == msg["nonce"]

        # Bot notified once, HMAC header present, amount = payment amount.
        assert len(notifies) == 1
        assert "x-internal-signature" in notifies[0]["headers"]
        assert notifies[0]["body"]["payment_id"] == created["payment_id"]
        assert notifies[0]["body"]["amount_usd"] == 0.01
        assert notifies[0]["body"]["tx_hash"].startswith("f026")


def test_live_submit_without_envelope_is_fine(live_app):
    """The echo is optional: the sidecar never needs client data beyond
    signature + buyer."""
    with TestClient(live_app) as client:
        _swap_facilitator_transport(live_app, _facilitator())
        _swap_notify_transport(live_app, [])
        created = _create_payment(client)
        r = _submit(client, created["payment_id"])
        assert r.status_code == 200, r.text
        assert r.json()["next_step"] == "settled"


def test_live_verify_failed_returns_402_and_keeps_pending(live_app):
    from himari_x402_sidecar import db

    fac_calls: list = []
    with TestClient(live_app) as client:
        _swap_facilitator_transport(
            live_app,
            _facilitator(
                verify={
                    "isValid": False,
                    "invalidReason": "permit2_allowance_required",
                    "invalidMessage": "approve Permit2",
                    "payer": BUYER_HEX,
                },
                captured=fac_calls,
            ),
        )
        created = _create_payment(client)
        r = _submit(client, created["payment_id"])
        assert r.status_code == 402, r.text
        body = r.json()
        assert body["accepted"] is False
        assert body["next_step"] == "verify-failed"
        assert body["error_reason"] == "permit2_allowance_required"
        assert body["error_message"] == "approve Permit2"
        # /settle never called.
        assert [p for p, _ in fac_calls] == ["/verify"]
        row = db.get_payment(live_app.state.conn, UUID(created["payment_id"]))
        assert row["status"] == "pending"
        assert row["error_reason"] == "permit2_allowance_required"
        assert row["buyer"] == BUYER_B58
        # Retry after fixing allowance works (row still submittable).
        _swap_facilitator_transport(live_app, _facilitator())
        _swap_notify_transport(live_app, [])
        r2 = _submit(client, created["payment_id"])
        assert r2.status_code == 200
        assert r2.json()["next_step"] == "settled"
        assert db.get_payment(live_app.state.conn, UUID(created["payment_id"]))["error_reason"] is None


def test_live_settle_failed_returns_402_and_keeps_pending(live_app):
    from himari_x402_sidecar import db

    notifies: list = []
    with TestClient(live_app) as client:
        _swap_facilitator_transport(
            live_app,
            _facilitator(
                settle={
                    "success": False,
                    "transaction": "",
                    "network": "tron:0xcd8690dc",
                    "errorReason": "transaction_failed",
                    "errorMessage": "REVERT opcode",
                }
            ),
        )
        _swap_notify_transport(live_app, notifies)
        created = _create_payment(client)
        r = _submit(client, created["payment_id"])
        assert r.status_code == 402, r.text
        body = r.json()
        assert body["next_step"] == "settle-failed"
        assert body["error_reason"] == "transaction_failed"
        assert body["error_message"] == "REVERT opcode"
        assert body["tx_hash"] is None
        row = db.get_payment(live_app.state.conn, UUID(created["payment_id"]))
        assert row["status"] == "pending"
        assert row["tx_hash"] is None
        assert row["error_reason"] == "transaction_failed"
        assert notifies == []


@pytest.mark.parametrize("path", ["/verify", "/settle"])
def test_live_facilitator_transport_error_returns_502(live_app, path):
    def handler(request: httpx.Request) -> httpx.Response:
        if request.url.path == path:
            return httpx.Response(500, text="Internal server error")
        return httpx.Response(200, json={"isValid": True})

    with TestClient(live_app) as client:
        _swap_facilitator_transport(live_app, handler)
        created = _create_payment(client)
        r = _submit(client, created["payment_id"])
        assert r.status_code == 502, r.text
        assert "facilitator" in r.json()["detail"].lower()


def test_live_success_with_empty_tx_hash_marks_settled_and_warns(live_app, caplog):
    import logging

    from himari_x402_sidecar import db

    with TestClient(live_app) as client:
        _swap_facilitator_transport(
            live_app, _facilitator(settle={"success": True, "transaction": "", "network": "tron:0xcd8690dc"})
        )
        _swap_notify_transport(live_app, [])
        created = _create_payment(client)
        with caplog.at_level(logging.WARNING, logger="himari_x402_sidecar.app"):
            r = _submit(client, created["payment_id"])
        assert r.status_code == 200
        assert r.json()["next_step"] == "settled"
        assert r.json()["tx_hash"] is None
        row = db.get_payment(live_app.state.conn, UUID(created["payment_id"]))
        assert row["status"] == "settled" and row["tx_hash"] == ""
        assert any("empty transaction hash" in rec.message for rec in caplog.records)


def test_live_bot_notify_failure_does_not_fail_submit(live_app, caplog):
    import logging

    with TestClient(live_app) as client:
        _swap_facilitator_transport(live_app, _facilitator())
        _swap_notify_transport(live_app, [], status_code=500)
        created = _create_payment(client)
        with caplog.at_level(logging.ERROR, logger="himari_x402_sidecar.app"):
            r = _submit(client, created["payment_id"])
        assert r.status_code == 200
        assert r.json()["next_step"] == "settled"
        assert any("bot notify failed" in rec.message for rec in caplog.records)


def test_live_legacy_row_without_nonce_returns_409(live_app):
    """Rows created before the v2 migration have no nonce/deadline; they
    cannot be settled under Permit2 — ask for a fresh permit."""
    with TestClient(live_app) as client:
        _swap_facilitator_transport(live_app, _facilitator())
        created = _create_payment(client)
        live_app.state.conn.execute(
            "UPDATE payments SET nonce = NULL, deadline = NULL WHERE payment_id = ?",
            (created["payment_id"],),
        )
        r = _submit(client, created["payment_id"])
        assert r.status_code == 409
        assert "fresh permit" in r.json()["detail"]
