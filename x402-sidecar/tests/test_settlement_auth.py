"""Settlement webhook HMAC validation.

These tests cover the inbound `POST /webhook/settlement` boundary. The
sidecar must refuse with 401 when the `X-Facilitator-Signature` header
is missing, malformed, or signs the wrong body — *before* the body is
parsed. Without this any host that can reach sidecar:8402 can forge
settlements (CRITICAL severity).
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
    from x402_sidecar import config as config_mod, app as app_mod
    importlib.reload(config_mod)
    importlib.reload(app_mod)
    config_mod._settings = None
    return app_mod


@pytest.fixture
def captured_notifies():
    return []


@pytest.fixture
def app(env, captured_notifies):
    return env.create_app(), captured_notifies


def _mock_notify_client(captured: list) -> httpx.AsyncClient:
    def handler(request: httpx.Request) -> httpx.Response:
        captured.append({"url": str(request.url), "body": request.read()})
        return httpx.Response(200, json={"ok": True})

    return httpx.AsyncClient(transport=httpx.MockTransport(handler))


def _create_payment(client) -> str:
    r = client.post(
        "/payment-request",
        json={"room_id": "!a:b", "user_mxid": "@u:b", "amount_usd": 0.10},
    )
    assert r.status_code == 200, r.text
    return r.json()["payment_id"]


def _payload(pid: str, *, webhook_id: str = "wh-1", amount: float = 0.10) -> bytes:
    return json.dumps(
        {
            "payment_id": pid,
            "tx_hash": "0xabc",
            "block_number": 71_000_000,
            "amount_paid_usd": amount,
            "sender_wallet": "Tabc",
            "settled_at": "2026-05-02T15:00:00Z",
            "webhook_id": webhook_id,
        }
    ).encode("utf-8")


def test_empty_signature_returns_401(app):
    application, captured = app
    with TestClient(application) as client:
        application.state.notify_client = _mock_notify_client(captured)
        pid = _create_payment(client)
        body = _payload(pid)
        r = client.post(
            "/webhook/settlement",
            content=body,
            headers={"Content-Type": "application/json"},
        )
        assert r.status_code == 401, r.text
        assert "facilitator" in r.text.lower()
        # Bot was never notified, payment stays pending.
        assert captured == []
        s = client.get(f"/status/{pid}").json()
        assert s["status"] == "pending"


def test_invalid_signature_returns_401(app):
    application, captured = app
    with TestClient(application) as client:
        application.state.notify_client = _mock_notify_client(captured)
        pid = _create_payment(client)
        body = _payload(pid)
        r = client.post(
            "/webhook/settlement",
            content=body,
            headers={
                "Content-Type": "application/json",
                "X-Facilitator-Signature": "sha256=deadbeef" * 8,
            },
        )
        assert r.status_code == 401
        assert captured == []


def test_tampered_body_returns_401(app):
    """Sign the original body, but POST a different one (amount inflated)."""
    application, captured = app
    with TestClient(application) as client:
        application.state.notify_client = _mock_notify_client(captured)
        pid = _create_payment(client)
        original = _payload(pid, amount=0.10)
        sig = auth.sign(FAC_SECRET, original)
        tampered = _payload(pid, amount=10_000.00)  # inflate $10,000
        r = client.post(
            "/webhook/settlement",
            content=tampered,
            headers={
                "Content-Type": "application/json",
                "X-Facilitator-Signature": f"sha256={sig}",
            },
        )
        assert r.status_code == 401
        assert captured == []


def test_wrong_scheme_returns_401(app):
    """`md5=...` or other scheme must not be accepted."""
    application, captured = app
    with TestClient(application) as client:
        application.state.notify_client = _mock_notify_client(captured)
        pid = _create_payment(client)
        body = _payload(pid)
        sig = auth.sign(FAC_SECRET, body)
        r = client.post(
            "/webhook/settlement",
            content=body,
            headers={
                "Content-Type": "application/json",
                "X-Facilitator-Signature": f"md5={sig}",
            },
        )
        assert r.status_code == 401
        assert captured == []


def test_valid_signature_proceeds(app):
    application, captured = app
    with TestClient(application) as client:
        application.state.notify_client = _mock_notify_client(captured)
        pid = _create_payment(client)
        body = _payload(pid)
        sig = auth.sign(FAC_SECRET, body)
        r = client.post(
            "/webhook/settlement",
            content=body,
            headers={
                "Content-Type": "application/json",
                "X-Facilitator-Signature": f"sha256={sig}",
            },
        )
        assert r.status_code == 200, r.text
        # Bot got notified exactly once.
        assert len(captured) == 1
        # Payment is settled.
        s = client.get(f"/status/{pid}").json()
        assert s["status"] == "settled"


def test_bare_hex_signature_also_accepted(app):
    """Some facilitators omit the `sha256=` scheme. We tolerate bare hex."""
    application, captured = app
    with TestClient(application) as client:
        application.state.notify_client = _mock_notify_client(captured)
        pid = _create_payment(client)
        body = _payload(pid)
        sig = auth.sign(FAC_SECRET, body)
        r = client.post(
            "/webhook/settlement",
            content=body,
            headers={
                "Content-Type": "application/json",
                "X-Facilitator-Signature": sig,
            },
        )
        assert r.status_code == 200


def test_empty_after_scheme_returns_401(app):
    """`X-Facilitator-Signature: sha256=` (empty digest) is rejected."""
    application, captured = app
    with TestClient(application) as client:
        application.state.notify_client = _mock_notify_client(captured)
        pid = _create_payment(client)
        body = _payload(pid)
        r = client.post(
            "/webhook/settlement",
            content=body,
            headers={
                "Content-Type": "application/json",
                "X-Facilitator-Signature": "sha256=",
            },
        )
        assert r.status_code == 401
        assert captured == []


def test_verify_facilitator_header_unit():
    """Pure unit coverage for the helper."""
    body = b'{"x":1}'
    good = auth.sign("secret", body)
    assert auth.verify_facilitator_header("secret", body, f"sha256={good}") is True
    assert auth.verify_facilitator_header("secret", body, f"SHA256={good}") is True
    assert auth.verify_facilitator_header("secret", body, good) is True  # bare hex
    assert auth.verify_facilitator_header("secret", body, None) is False
    assert auth.verify_facilitator_header("secret", body, "") is False
    assert auth.verify_facilitator_header("secret", body, "sha256=") is False
    assert auth.verify_facilitator_header("secret", body, f"md5={good}") is False
    assert auth.verify_facilitator_header("secret", body, f"sha256=not-hex") is False
    assert auth.verify_facilitator_header("", body, f"sha256={good}") is False
