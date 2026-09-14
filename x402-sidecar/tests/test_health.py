"""Smoke test for the /health endpoint.

Uses FastAPI TestClient + httpx ASGITransport so no real network is needed.
Sets required env-vars via monkeypatch so Pydantic Settings doesn't fail.
"""
from __future__ import annotations

import os
import importlib

import pytest
from fastapi.testclient import TestClient


@pytest.fixture
def app_with_env(monkeypatch: pytest.MonkeyPatch):
    """Return an app instance with valid required env vars set."""
    monkeypatch.setenv("X402_FACILITATOR_API_KEY", "test-key-32-chars-long-aaaaaaaaaa")
    monkeypatch.setenv("X402_INTERNAL_SECRET", "test-internal-secret-bbbbbbbbbbbbbbbb")
    monkeypatch.setenv("X402_FACILITATOR_WEBHOOK_SECRET", "test-fac-secret-cccccccccccccccccc")
    monkeypatch.setenv("X402_AGENT_WALLET", "TSkeaPMSuaojcCzE7mWqw4xN1awU7NfdfY")
    # Reload config + app modules so Settings picks up monkeypatched env.
    from x402_sidecar import config as config_mod
    from x402_sidecar import app as app_mod
    importlib.reload(config_mod)
    importlib.reload(app_mod)
    config_mod._settings = None  # reset singleton
    return app_mod.create_app()


def test_health_returns_ok(app_with_env):
    client = TestClient(app_with_env)
    r = client.get("/health")
    assert r.status_code == 200
    body = r.json()
    assert body["ok"] is True
    assert body["facilitator_api_key_present"] is True
    assert body["internal_secret_present"] is True
    # Secret value itself MUST NOT leak.
    assert "test-key-32" not in r.text
    assert "test-internal-secret" not in r.text


def test_health_redacts_keys(app_with_env):
    client = TestClient(app_with_env)
    body = client.get("/health").json()
    # Length is exposed (non-sensitive sanity); value is not.
    assert body["facilitator_api_key_length"] == len("test-key-32-chars-long-aaaaaaaaaa")
    # Presence flag exists; the value-bearing field name does not.
    assert "facilitator_api_key_present" in body
    assert "facilitator_api_key_value" not in body
    # Don't leak the actual key in any field.
    assert "test-key-32-chars-long" not in client.get("/health").text


def test_missing_required_env_fails_fast(monkeypatch: pytest.MonkeyPatch):
    """Bot/Compose should crash early if required vars are missing."""
    monkeypatch.delenv("X402_FACILITATOR_API_KEY", raising=False)
    monkeypatch.delenv("X402_INTERNAL_SECRET", raising=False)
    monkeypatch.delenv("X402_FACILITATOR_WEBHOOK_SECRET", raising=False)
    monkeypatch.delenv("X402_AGENT_WALLET", raising=False)
    from x402_sidecar import config as config_mod
    importlib.reload(config_mod)
    config_mod._settings = None
    with pytest.raises(Exception):  # Pydantic ValidationError
        config_mod.get_settings()
