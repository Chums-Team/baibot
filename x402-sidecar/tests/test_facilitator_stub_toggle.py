"""`X402_FACILITATOR_USE_STUB` semantics under x402 v2.

The flag no longer selects between two envelope builders (v2 `exact`
needs no fee quote, so there is exactly one, pure builder). It now means:
  * False (default): envelope has no marker; `/submit` runs /verify + /settle.
  * True (dev only): envelope carries `_stub: true`; `/submit` acks without
    calling the facilitator.
"""
from __future__ import annotations

import importlib
from uuid import uuid4

import pytest


@pytest.fixture
def required_env(monkeypatch):
    monkeypatch.setenv("X402_FACILITATOR_API_KEY", "test-key" * 4)
    monkeypatch.setenv("X402_INTERNAL_SECRET", "secret-32" * 4)
    monkeypatch.setenv("X402_FACILITATOR_WEBHOOK_SECRET", "fac-webhook-32" * 4)
    monkeypatch.setenv("X402_AGENT_WALLET", "TSkeaPMSuaojcCzE7mWqw4xN1awU7NfdfY")


def _fresh_settings():
    from x402_sidecar import config as config_mod

    importlib.reload(config_mod)
    config_mod._settings = None
    return config_mod.Settings()  # type: ignore[call-arg]


def _build(settings):
    from x402_sidecar.facilitator_client import FacilitatorClient

    return FacilitatorClient(settings).build_payment_request(
        amount_usd=0.10, room_id="!stub:test", payment_id=uuid4()
    )


def test_default_flag_is_live(required_env, monkeypatch):
    monkeypatch.delenv("X402_FACILITATOR_USE_STUB")
    settings = _fresh_settings()
    assert settings.facilitator_use_stub is False
    build = _build(settings)
    assert "_stub" not in build.permit_payload
    assert build.permit_payload["primaryType"] == "PermitWitnessTransferFrom"


@pytest.mark.parametrize("value", ["true", "True", "1", "yes"])
def test_truthy_values_keep_stub(required_env, monkeypatch, value):
    monkeypatch.setenv("X402_FACILITATOR_USE_STUB", value)
    settings = _fresh_settings()
    assert settings.facilitator_use_stub is True
    assert _build(settings).permit_payload.get("_stub") is True


@pytest.mark.parametrize("value", ["false", "False", "0", "no"])
def test_falsy_values_go_live(required_env, monkeypatch, value):
    monkeypatch.setenv("X402_FACILITATOR_USE_STUB", value)
    settings = _fresh_settings()
    assert settings.facilitator_use_stub is False
    assert "_stub" not in _build(settings).permit_payload


def test_stub_and_live_envelopes_share_shape(required_env, monkeypatch):
    """Only the marker differs — so a client exercised against stub mode
    signs exactly what live mode will send to the facilitator."""
    stub = _build(_fresh_settings()).permit_payload
    monkeypatch.setenv("X402_FACILITATOR_USE_STUB", "false")
    live = _build(_fresh_settings()).permit_payload
    stub.pop("_stub")
    for k in ("primaryType", "types", "domain"):
        assert stub[k] == live[k]
    assert set(stub["message"]) == set(live["message"])


def test_unsupported_scheme_fails_fast(required_env, monkeypatch):
    """`exact_permit` is gone on the v2 facilitator; refuse to boot."""
    monkeypatch.setenv("X402_SCHEME", "exact_permit")
    with pytest.raises(Exception, match="exact"):
        _fresh_settings()


def test_unknown_network_fails_fast(required_env, monkeypatch):
    monkeypatch.setenv("X402_NETWORK", "tron:fake")
    with pytest.raises(Exception, match="unknown network"):
        _fresh_settings()
