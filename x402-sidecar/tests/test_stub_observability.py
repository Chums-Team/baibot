"""Stub-mode observability.

The sidecar must make it impossible to be accidentally running in stub
mode in production:
  1. Startup banner — `WARNING: STARTUP: X402 facilitator running in
     STUB mode...` logged once at app boot when USE_STUB=true.
  2. Per-call warning — `WARNING: STUB MODE: build_payment_request...`
     emitted on every `_build_stub_payment_request` invocation.
  3. /health — exposes `facilitator_mode: "stub" | "live"` so
     dashboards and smoke checks can assert the configured mode.

Without these an operator would only learn about a wrong-mode
misconfiguration through user reports of failed settlements.
"""
from __future__ import annotations

import importlib
import logging
from uuid import uuid4

import pytest
from fastapi.testclient import TestClient


@pytest.fixture
def required_env(monkeypatch):
    monkeypatch.setenv("X402_FACILITATOR_API_KEY", "test-key" * 4)
    monkeypatch.setenv("X402_INTERNAL_SECRET", "secret-32" * 4)
    monkeypatch.setenv(
        "X402_FACILITATOR_WEBHOOK_SECRET", "fac-webhook-32" * 4
    )
    monkeypatch.setenv("X402_AGENT_WALLET", "TSkeaPMSuaojcCzE7mWqw4xN1awU7NfdfY")
    return monkeypatch


def _reload_app():
    """Force-reload app + config so a fresh USE_STUB env-var is picked up."""
    from himari_x402_sidecar import config as config_mod
    from himari_x402_sidecar import app as app_mod
    importlib.reload(config_mod)
    importlib.reload(app_mod)
    config_mod._settings = None
    return app_mod


# ---- /health field ----


def test_health_reports_stub_mode_when_use_stub_true(required_env):
    required_env.setenv("X402_FACILITATOR_USE_STUB", "true")
    app_mod = _reload_app()
    with TestClient(app_mod.create_app()) as client:
        body = client.get("/health").json()
    assert body["facilitator_mode"] == "stub"


def test_health_reports_live_mode_when_use_stub_false(required_env):
    required_env.setenv("X402_FACILITATOR_USE_STUB", "false")
    app_mod = _reload_app()
    with TestClient(app_mod.create_app()) as client:
        body = client.get("/health").json()
    assert body["facilitator_mode"] == "live"


def test_health_defaults_to_live_mode(required_env, monkeypatch):
    """No env var → default = live; stub is opt-in for dev only."""
    required_env.delenv("X402_FACILITATOR_USE_STUB")
    app_mod = _reload_app()
    # Live mode starts the readiness watch; keep the test offline.
    monkeypatch.setattr(app_mod.FacilitatorWatch, "start", lambda self: None)
    with TestClient(app_mod.create_app()) as client:
        body = client.get("/health").json()
    assert body["facilitator_mode"] == "live"


# ---- startup banner ----


def test_startup_banner_logged_in_stub_mode(required_env, caplog):
    required_env.setenv("X402_FACILITATOR_USE_STUB", "true")
    app_mod = _reload_app()
    with caplog.at_level(logging.WARNING, logger="himari_x402_sidecar.app"):
        app_mod.create_app()
    # Startup-time warning (not request-time) — looks for the prefix.
    matches = [r for r in caplog.records if "STARTUP" in r.message]
    assert matches, (
        "expected at least one STARTUP warning when USE_STUB=true; "
        f"saw records: {[r.message for r in caplog.records]}"
    )
    assert "STUB mode" in matches[0].message
    assert "X402_FACILITATOR_USE_STUB" in matches[0].message


def test_no_startup_banner_in_live_mode(required_env, caplog):
    required_env.setenv("X402_FACILITATOR_USE_STUB", "false")
    app_mod = _reload_app()
    with caplog.at_level(logging.WARNING, logger="himari_x402_sidecar.app"):
        app_mod.create_app()
    # Live mode is the prod state — no spurious STUB warning at startup.
    assert not any(
        "STARTUP" in r.message and "STUB" in r.message
        for r in caplog.records
    ), "live mode must not log a STUB startup warning"


# ---- per-call warning ----


@pytest.mark.asyncio
async def test_per_call_warning_on_each_stub_payment_request(
    required_env, caplog
):
    required_env.setenv("X402_FACILITATOR_USE_STUB", "true")
    _reload_app()
    from himari_x402_sidecar import config as config_mod
    from himari_x402_sidecar.facilitator_client import FacilitatorClient

    settings = config_mod.Settings()  # type: ignore[call-arg]
    client = FacilitatorClient(settings)
    try:
        with caplog.at_level(
            logging.WARNING,
            logger="himari_x402_sidecar.facilitator_client",
        ):
            client.build_payment_request(
                amount_usd=0.10,
                room_id="!a:b",
                payment_id=uuid4(),
            )
            client.build_payment_request(
                amount_usd=0.20,
                room_id="!a:b",
                payment_id=uuid4(),
            )
    finally:
        await client.aclose()

    stub_warnings = [
        r
        for r in caplog.records
        if r.levelno == logging.WARNING and "STUB MODE" in r.message
    ]
    # Per-call: two requests → two warnings.
    assert len(stub_warnings) >= 2, (
        f"expected >= 2 STUB MODE warnings (one per call), saw "
        f"{len(stub_warnings)}: {[r.message for r in stub_warnings]}"
    )
    # Warning text names the env-var so the operator can disable it.
    assert all(
        "X402_FACILITATOR_USE_STUB" in r.message for r in stub_warnings
    )
