"""End-to-end style tests for the sidecar HTTP surface.

Uses FastAPI TestClient against the real ASGI app, with these mocks:
- DB → in-memory SQLite via `SIDECAR_DB_PATH=:memory:`.
- Bot notify → captured via `httpx.MockTransport` injected into
  `app.state.notify_client`.
- Facilitator → the existing stub `FacilitatorClient.build_payment_request`,
  which returns deterministic permit-payload + expires_at without any I/O.
"""
from __future__ import annotations

import importlib
import json
from uuid import uuid4

import httpx
import pytest
from fastapi.testclient import TestClient

from x402_sidecar import auth


FAC_SECRET = "fac-webhook-32" * 4


@pytest.fixture
def env(monkeypatch):
    monkeypatch.setenv("X402_FACILITATOR_API_KEY", "test-key" * 4)
    monkeypatch.setenv("X402_INTERNAL_SECRET", "secret-32" * 4)
    monkeypatch.setenv("X402_FACILITATOR_WEBHOOK_SECRET", FAC_SECRET)
    monkeypatch.setenv("X402_AGENT_WALLET", "TSkeaPMSuaojcCzE7mWqw4xN1awU7NfdfY")
    monkeypatch.setenv("SIDECAR_DB_PATH", ":memory:")
    monkeypatch.setenv("BOT_X402_NOTIFY_URL", "http://bot.test/internal/x402-settled")
    # Force reload so Settings picks the new env.
    from x402_sidecar import config as config_mod, app as app_mod
    importlib.reload(config_mod)
    importlib.reload(app_mod)
    config_mod._settings = None
    return app_mod


def _settle(client, payload: dict) -> "object":
    """POST /webhook/settlement with a valid X-Facilitator-Signature header."""
    body = json.dumps(payload).encode("utf-8")
    sig = auth.sign(FAC_SECRET, body)
    return client.post(
        "/webhook/settlement",
        content=body,
        headers={
            "Content-Type": "application/json",
            "X-Facilitator-Signature": f"sha256={sig}",
        },
    )


@pytest.fixture
def captured_notifies():
    """Container for whatever the sidecar POSTs to the bot."""
    return []


@pytest.fixture
def app(env, captured_notifies):
    application = env.create_app()

    # We need to wait for lifespan to set state.notify_client, so we create
    # the TestClient (which triggers lifespan), then swap the client.
    return application, captured_notifies


def _make_mock_notify_client(captured: list, status_code: int = 200) -> httpx.AsyncClient:
    def handler(request: httpx.Request) -> httpx.Response:
        captured.append(
            {
                "url": str(request.url),
                "headers": dict(request.headers),
                "body": request.read(),
            }
        )
        return httpx.Response(status_code, json={"ok": True})

    return httpx.AsyncClient(transport=httpx.MockTransport(handler))


def test_health_still_works(app):
    application, _ = app
    with TestClient(application) as client:
        r = client.get("/health")
        assert r.status_code == 200
        body = r.json()
        assert body["ok"] is True
        assert body["agent_wallet"] == "TSkeaPMSuaojcCzE7mWqw4xN1awU7NfdfY"


def test_payment_request_creates_record(app):
    application, _ = app
    with TestClient(application) as client:
        r = client.post(
            "/payment-request",
            json={
                "room_id": "!abc:example.com",
                "user_mxid": "@user:example.com",
                "amount_usd": 0.10,
            },
        )
        assert r.status_code == 200, r.text
        body = r.json()
        assert "payment_id" in body
        assert body["pay_to"] == "TSkeaPMSuaojcCzE7mWqw4xN1awU7NfdfY"
        assert body["network"] == "tron:0xcd8690dc"  # default: nile, hex CAIP-2
        assert body["scheme"] == "exact"
        # Permit2 (approve spender + typed-data verifyingContract) and the
        # exact proxy (message.spender) for nile.
        assert body["permit_contract"] == "TYQuuhGbEMxF7nZxUHV3uHJxAVVAegNU9h"
        assert body["proxy_contract"] == "TFGoaq2KjizijgjtkVxT7yjffW1A5T1j6F"
        assert body["payment_requirements"]["extra"] == {"assetTransferMethod": "permit2"}
        assert body["permit_payload"]["primaryType"] == "PermitWitnessTransferFrom"
        assert body["permit_payload"]["_stub"] is True

        # Status read-back works.
        pid = body["payment_id"]
        r2 = client.get(f"/status/{pid}")
        assert r2.status_code == 200
        s = r2.json()
        assert s["status"] == "pending"
        assert s["amount_usd"] == 0.10
        assert s["nonce"] == body["permit_payload"]["message"]["nonce"]
        assert s["deadline"] == int(body["permit_payload"]["message"]["deadline"])
        assert s["network"] == "tron:0xcd8690dc"


def test_settlement_webhook_notifies_bot(app):
    application, captured = app
    with TestClient(application) as client:
        # Replace the notify client AFTER lifespan started it.
        application.state.notify_client = _make_mock_notify_client(captured)

        r = client.post(
            "/payment-request",
            json={
                "room_id": "!abc:example.com",
                "user_mxid": "@user:example.com",
                "amount_usd": 0.10,
            },
        )
        pid = r.json()["payment_id"]

        # Simulate facilitator webhook (signed).
        wr = _settle(
            client,
            {
                "payment_id": pid,
                "tx_hash": "0xabc",
                "block_number": 71_000_000,
                "amount_paid_usd": 0.10,
                "sender_wallet": "Tabc",
                "settled_at": "2026-05-02T15:00:00Z",
                "webhook_id": "wh-001",
            },
        )
        assert wr.status_code == 200, wr.text
        assert wr.json() == {"ok": True}

        # Bot got exactly one notify with the right HMAC.
        assert len(captured) == 1
        msg = captured[0]
        assert msg["url"] == "http://bot.test/internal/x402-settled"
        body = msg["body"]
        sig = msg["headers"]["x-internal-signature"]
        assert auth.verify("secret-32" * 4, body, sig)
        decoded = json.loads(body)
        assert decoded["payment_id"] == pid
        assert decoded["room_id"] == "!abc:example.com"
        assert decoded["amount_usd"] == 0.10
        assert decoded["tx_hash"] == "0xabc"

        # Status now `settled`.
        s = client.get(f"/status/{pid}").json()
        assert s["status"] == "settled"
        assert s["tx_hash"] == "0xabc"


def test_settlement_webhook_idempotent(app):
    application, captured = app
    with TestClient(application) as client:
        application.state.notify_client = _make_mock_notify_client(captured)
        r = client.post(
            "/payment-request",
            json={"room_id": "!a:b", "user_mxid": "@u:b", "amount_usd": 0.10},
        )
        pid = r.json()["payment_id"]
        payload = {
            "payment_id": pid,
            "tx_hash": "0xabc",
            "block_number": 1,
            "amount_paid_usd": 0.10,
            "sender_wallet": "T",
            "settled_at": "2026-05-02T15:00:00Z",
            "webhook_id": "wh-dup",
        }
        _settle(client, payload)
        _settle(client, payload)  # duplicate
        # Bot should only have been notified once.
        assert len(captured) == 1


def test_settlement_unknown_payment_404(app):
    application, captured = app
    with TestClient(application) as client:
        application.state.notify_client = _make_mock_notify_client(captured)
        r = _settle(
            client,
            {
                "payment_id": str(uuid4()),
                "tx_hash": "0x",
                "block_number": 0,
                "amount_paid_usd": 0.0,
                "sender_wallet": "T",
                "settled_at": "2026-05-02T15:00:00Z",
                "webhook_id": "wh-unknown",
            },
        )
        assert r.status_code == 404
        assert captured == []


def test_status_unknown_404(app):
    application, _ = app
    with TestClient(application) as client:
        r = client.get(f"/status/{uuid4()}")
        assert r.status_code == 404
