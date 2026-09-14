"""FacilitatorWatch: readiness verdict from /supported + relayer resources +
our own settle-failure streak. All I/O mocked; no background loop here.
"""
from __future__ import annotations

import json

import httpx
import pytest

from himari_x402_sidecar.facilitator_watch import FacilitatorWatch

NET = "tron:0xcd8690dc"
RELAYER = "TKNnggcU5Ph18hSdK1U8xeLAG7YrqKwoUu"


def _supported(*, methods=("eip3009", "permit2"), network=NET, scheme="exact"):
    return {
        "kinds": [
            {
                "x402Version": 2,
                "scheme": scheme,
                "network": network,
                "extra": {
                    "supportedAssetTransferMethods": list(methods),
                    "permit2FacilitatorAddress": RELAYER,
                },
            }
        ],
        "signers": {"tron:*": [RELAYER]},
    }


def _rpc(energy_limit=1_000_000, energy_used=0, balance_sun=0):
    def handler(request: httpx.Request) -> httpx.Response:
        body = json.loads(request.content)
        assert body["address"] == RELAYER
        if request.url.path == "/wallet/getaccountresource":
            return httpx.Response(200, json={"EnergyLimit": energy_limit, "EnergyUsed": energy_used})
        if request.url.path == "/wallet/getaccount":
            return httpx.Response(200, json={"balance": balance_sun})
        return httpx.Response(404)

    return httpx.AsyncClient(transport=httpx.MockTransport(handler))


def _watch(supported, rpc):
    async def get_json(path):
        if isinstance(supported, Exception):
            raise supported
        assert path == "/supported"
        return supported

    return FacilitatorWatch(facilitator_get_json=get_json, network=NET, rpc_client=rpc)


@pytest.mark.asyncio
async def test_green_when_supported_and_relayer_funded():
    w = _watch(_supported(), _rpc(energy_limit=1_000_000))
    s = await w.probe()
    assert s.ok is True and s.reason is None
    assert s.relayer == RELAYER
    assert s.relayer_energy_available == 1_000_000


@pytest.mark.asyncio
async def test_unreachable_supported_is_red():
    w = _watch(RuntimeError("boom"), _rpc())
    s = await w.probe()
    assert s.ok is False and s.reason.startswith("supported_unreachable")


@pytest.mark.asyncio
async def test_missing_network_or_permit2_is_red():
    s = await _watch(_supported(network="tron:0x2b6653dc"), _rpc()).probe()
    assert s.ok is False and s.reason == "network_or_scheme_not_supported"
    s = await _watch(_supported(methods=("eip3009",)), _rpc()).probe()
    assert s.ok is False and s.reason == "permit2_not_supported"


@pytest.mark.asyncio
async def test_relayer_low_resources_is_red_but_trx_burn_counts():
    # 5 settles * 45k energy = 225k needed; 100k energy and no TRX -> red.
    s = await _watch(_supported(), _rpc(energy_limit=100_000)).probe()
    assert s.ok is False and s.reason.startswith("relayer_low_resources")
    # Same energy but 50 TRX to burn (need 225k*100 sun = 22.5 TRX) -> green.
    s = await _watch(_supported(), _rpc(energy_limit=100_000, balance_sun=50_000_000)).probe()
    assert s.ok is True


@pytest.mark.asyncio
async def test_rpc_trouble_is_unknown_not_red():
    def handler(request):
        return httpx.Response(500, text="trongrid down")

    w = _watch(_supported(), httpx.AsyncClient(transport=httpx.MockTransport(handler)))
    s = await w.probe()
    assert s.ok is True  # service itself answered; resources unknown


@pytest.mark.asyncio
async def test_settle_failure_streak_flips_red_and_success_resets():
    w = _watch(_supported(), _rpc(energy_limit=1_000_000))
    await w.probe()
    w.record_settle_outcome(False, "transaction_failed")
    w.record_settle_outcome(False, "transaction_failed")
    assert w.status.ok is True  # below threshold (3)
    w.record_settle_outcome(False, "transaction_failed")
    assert w.status.ok is False and w.status.reason == "settle_failures"
    # A green probe does not override an active failure streak.
    s = await w.probe()
    assert s.ok is False and s.reason == "settle_failures"
    w.record_settle_outcome(True)
    assert w.status.ok is True and w.status.consecutive_settle_failures == 0


@pytest.mark.asyncio
async def test_default_status_is_unknown():
    w = _watch(_supported(), _rpc())
    assert w.status.ok is None and w.status.as_dict()["reason"] is None
