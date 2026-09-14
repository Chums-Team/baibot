"""FacilitatorClient transport against a mocked x402 v2 facilitator.

Wire contract per `external-repos/BofAI--x402-facilitator/src/server.ts`:
  * POST /verify, /settle: body `{x402Version, paymentPayload, paymentRequirements}`
    (`.strict()` — no other top-level keys), header `X-API-KEY`.
  * /verify → `{isValid, invalidReason?, invalidMessage?, payer?}` (400 with
    the same shape on `missing_parameters` / `invalid_json`).
  * /settle → `{success, transaction, network, errorReason?, errorMessage?, payer?}`.
"""
from __future__ import annotations

import importlib
import json
from uuid import uuid4

import httpx
import pytest


@pytest.fixture
def required_env(monkeypatch):
    monkeypatch.setenv("X402_FACILITATOR_API_KEY", "real-api-key" * 4)
    monkeypatch.setenv("X402_INTERNAL_SECRET", "secret-32" * 4)
    monkeypatch.setenv("X402_FACILITATOR_WEBHOOK_SECRET", "fac-webhook-32" * 4)
    monkeypatch.setenv("X402_AGENT_WALLET", "TSkeaPMSuaojcCzE7mWqw4xN1awU7NfdfY")
    monkeypatch.setenv("X402_FACILITATOR_USE_STUB", "false")
    monkeypatch.setenv("X402_NETWORK", "tron:0xcd8690dc")


def _fresh_settings():
    from himari_x402_sidecar import config as config_mod

    importlib.reload(config_mod)
    config_mod._settings = None
    return config_mod.Settings()  # type: ignore[call-arg]


async def _client_with(handler):
    from himari_x402_sidecar.facilitator_client import USER_AGENT, FacilitatorClient

    settings = _fresh_settings()
    client = FacilitatorClient(settings)
    await client._client.aclose()
    client._client = httpx.AsyncClient(
        base_url=settings.facilitator_url,
        headers={"X-API-KEY": settings.facilitator_api_key, "User-Agent": USER_AGENT},
        transport=httpx.MockTransport(handler),
    )
    return client


def _wire_body():
    from himari_x402_sidecar.permit2_builder import (
        build_facilitator_body,
        build_payment_requirements,
        build_permit2_authorization,
    )

    req = build_payment_requirements(
        network="tron:0xcd8690dc",
        amount_micros=10000,
        asset_b58="TXYZopYRdj2D9XRtbG411XZZ3kM5VkAeBf",
        pay_to_b58="TSkeaPMSuaojcCzE7mWqw4xN1awU7NfdfY",
    )
    auth = build_permit2_authorization(
        buyer_hex="0x" + "11" * 20,
        token_hex="0x" + "22" * 20,
        amount_micros=10000,
        proxy_hex="0x" + "33" * 20,
        nonce=7,
        deadline=1_900_000_000,
        pay_to_hex="0x" + "44" * 20,
    )
    return build_facilitator_body(requirements=req, authorization=auth, signature_hex="0x" + "ab" * 65)


# ---- outbound shape ----


@pytest.mark.asyncio
async def test_verify_posts_strict_v2_body_with_api_key(required_env):
    captured = {}

    def handler(request: httpx.Request) -> httpx.Response:
        captured["path"] = request.url.path
        captured["headers"] = dict(request.headers)
        captured["body"] = json.loads(request.content)
        return httpx.Response(200, json={"isValid": True, "payer": "0x" + "11" * 20})

    client = await _client_with(handler)
    try:
        result = await client.verify(_wire_body())
    finally:
        await client.aclose()

    assert captured["path"] == "/verify"
    assert captured["headers"]["x-api-key"] == "real-api-key" * 4
    body = captured["body"]
    assert set(body) == {"x402Version", "paymentPayload", "paymentRequirements"}
    assert body["x402Version"] == 2
    pp = body["paymentPayload"]
    assert set(pp) == {"x402Version", "accepted", "payload"}
    assert pp["accepted"] == body["paymentRequirements"]
    assert set(pp["payload"]) == {"signature", "permit2Authorization"}
    auth = pp["payload"]["permit2Authorization"]
    assert set(auth) == {"from", "permitted", "spender", "nonce", "deadline", "witness"}
    assert set(auth["permitted"]) == {"token", "amount"}
    assert set(auth["witness"]) == {"to", "validAfter"}
    assert result.is_valid is True
    assert result.payer == "0x" + "11" * 20


@pytest.mark.asyncio
async def test_settle_posts_same_body_to_settle(required_env):
    captured = {}

    def handler(request: httpx.Request) -> httpx.Response:
        captured["path"] = request.url.path
        captured["body"] = json.loads(request.content)
        return httpx.Response(
            200,
            json={
                "success": True,
                "transaction": "f026a093" + "00" * 28,
                "network": "tron:0xcd8690dc",
                "payer": "0x" + "11" * 20,
            },
        )

    client = await _client_with(handler)
    try:
        result = await client.settle(_wire_body())
    finally:
        await client.aclose()

    assert captured["path"] == "/settle"
    assert captured["body"] == _wire_body()
    assert result.success is True
    assert result.transaction.startswith("f026a093")
    assert result.network == "tron:0xcd8690dc"
    assert result.error_reason is None


# ---- protocol-level rejections are results, not exceptions ----


@pytest.mark.asyncio
async def test_verify_invalid_is_returned_not_raised(required_env):
    def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(
            200,
            json={
                "isValid": False,
                "invalidReason": "permit2_allowance_required",
                "invalidMessage": "approve Permit2 first",
                "payer": "0x" + "11" * 20,
            },
        )

    client = await _client_with(handler)
    try:
        result = await client.verify(_wire_body())
    finally:
        await client.aclose()
    assert result.is_valid is False
    assert result.invalid_reason == "permit2_allowance_required"
    assert result.invalid_message == "approve Permit2 first"


@pytest.mark.asyncio
async def test_verify_400_with_protocol_body_is_a_result(required_env):
    """server.ts answers schema rejections as 400 + {isValid:false,...}."""

    def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(400, json={"isValid": False, "invalidReason": "missing_parameters"})

    client = await _client_with(handler)
    try:
        result = await client.verify(_wire_body())
    finally:
        await client.aclose()
    assert result.is_valid is False
    assert result.invalid_reason == "missing_parameters"


@pytest.mark.asyncio
async def test_settle_failure_is_returned_not_raised(required_env):
    def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(
            200,
            json={
                "success": False,
                "transaction": "",
                "network": "tron:0xcd8690dc",
                "errorReason": "transaction_failed",
                "errorMessage": "REVERT",
            },
        )

    client = await _client_with(handler)
    try:
        result = await client.settle(_wire_body())
    finally:
        await client.aclose()
    assert result.success is False
    assert result.transaction is None  # empty string normalised to None
    assert result.error_reason == "transaction_failed"
    assert result.error_message == "REVERT"


# ---- transport failures raise FacilitatorError ----


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "response",
    [
        httpx.Response(500, text="Internal Server Error"),
        httpx.Response(502, text="<html>bad gateway</html>"),
        httpx.Response(200, text="not json"),
        httpx.Response(200, json={"unexpected": True}),
        httpx.Response(429, json={"code": "rate_limited", "message": "slow down"}),
    ],
)
async def test_settle_raises_on_non_protocol_responses(required_env, response):
    from himari_x402_sidecar.facilitator_client import FacilitatorError

    client = await _client_with(lambda request: response)
    try:
        with pytest.raises(FacilitatorError):
            await client.settle(_wire_body())
    finally:
        await client.aclose()


@pytest.mark.asyncio
async def test_verify_raises_on_network_error(required_env):
    from himari_x402_sidecar.facilitator_client import FacilitatorError

    def handler(request: httpx.Request) -> httpx.Response:
        raise httpx.ConnectError("boom", request=request)

    client = await _client_with(handler)
    try:
        with pytest.raises(FacilitatorError, match="network error"):
            await client.verify(_wire_body())
    finally:
        await client.aclose()


# ---- /supported ----


@pytest.mark.asyncio
async def test_supported_returns_kinds(required_env):
    def handler(request: httpx.Request) -> httpx.Response:
        assert request.url.path == "/supported"
        return httpx.Response(
            200,
            json={"kinds": [{"x402Version": 2, "scheme": "exact", "network": "tron:0xcd8690dc"}]},
        )

    client = await _client_with(handler)
    try:
        data = await client.supported()
    finally:
        await client.aclose()
    assert data["kinds"][0]["scheme"] == "exact"


# ---- build_payment_request is pure (no network) ----


def test_build_payment_request_makes_no_http_calls(required_env):
    calls = []

    def handler(request: httpx.Request) -> httpx.Response:
        calls.append(request.url.path)
        return httpx.Response(500)

    from himari_x402_sidecar.facilitator_client import FacilitatorClient

    settings = _fresh_settings()
    client = FacilitatorClient(settings)
    client._client = httpx.AsyncClient(base_url=settings.facilitator_url, transport=httpx.MockTransport(handler))
    build = client.build_payment_request(amount_usd=0.10, room_id="!a:b", payment_id=uuid4())
    assert calls == []
    assert "_stub" not in build.permit_payload  # live mode: no marker
