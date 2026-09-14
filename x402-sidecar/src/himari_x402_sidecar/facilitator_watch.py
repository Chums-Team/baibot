"""Background facilitator readiness watch.

Answers one question for the bot/widget: *is it worth letting the user
spend TRX on an approve right now?* Three signals, none of them on the
payment path:

1. **Service**: `GET /supported` lists our network with scheme `exact`
   and `permit2` among the asset-transfer methods, and names the TRON
   signer (relayer) address(es).
2. **Relayer can pay gas**: TronGrid `getaccountresource` + `getaccount`
   for the relayer — enough free Energy for N settles, or enough TRX to
   burn for them.
3. **Our own history**: consecutive `/settle` failures reported by the
   app (`record_settle_outcome`). This is the only signal that would
   have caught the June 2026 outage (health ok, relayer funded, settle
   broken).

The loop runs every `probe_interval_sec` (default 300s). `/payment-request`
only reads the cached verdict, so the widget never waits on it. Verdict
`None` = not probed yet (startup) or stub mode → treated as "unknown",
the client shows the normal flow.
"""
from __future__ import annotations

import asyncio
import logging
import time
from dataclasses import dataclass, field
from typing import Any, Callable, Optional

import httpx

from .utils import canonical_network

logger = logging.getLogger(__name__)

# Energy consumed by one `x402ExactPermit2Proxy.settle` (nile receipt
# 2026-09-12: 45 193). Used only for the headroom estimate.
DEFAULT_SETTLE_ENERGY = 45_000
DEFAULT_MIN_SETTLES_HEADROOM = 5
DEFAULT_FAILURE_THRESHOLD = 3
DEFAULT_PROBE_INTERVAL_SEC = 300

_TRON_RPC_BY_NETWORK = {
    "tron:0x2b6653dc": "https://api.trongrid.io",
    "tron:0xcd8690dc": "https://nile.trongrid.io",
    "tron:0x94a9059e": "https://api.shasta.trongrid.io",
}


@dataclass
class FacilitatorStatus:
    ok: Optional[bool] = None
    reason: Optional[str] = None
    checked_at: Optional[float] = None
    relayer: Optional[str] = None
    relayer_energy_available: Optional[int] = None
    relayer_trx_sun: Optional[int] = None
    consecutive_settle_failures: int = 0
    last_settle_error: Optional[str] = None
    extras: dict[str, Any] = field(default_factory=dict)

    def as_dict(self) -> dict[str, Any]:
        return {
            "ok": self.ok,
            "reason": self.reason,
            "checked_at": self.checked_at,
            "relayer": self.relayer,
            "relayer_energy_available": self.relayer_energy_available,
            "relayer_trx_sun": self.relayer_trx_sun,
            "consecutive_settle_failures": self.consecutive_settle_failures,
            "last_settle_error": self.last_settle_error,
        }


class FacilitatorWatch:
    def __init__(
        self,
        *,
        facilitator_get_json: Callable[[str], Any],
        network: str,
        tron_rpc_url: Optional[str] = None,
        probe_interval_sec: int = DEFAULT_PROBE_INTERVAL_SEC,
        settle_energy: int = DEFAULT_SETTLE_ENERGY,
        min_settles_headroom: int = DEFAULT_MIN_SETTLES_HEADROOM,
        failure_threshold: int = DEFAULT_FAILURE_THRESHOLD,
        rpc_client: Optional[httpx.AsyncClient] = None,
    ):
        self._get_json = facilitator_get_json
        self._network = canonical_network(network)
        self._rpc_url = (tron_rpc_url or _TRON_RPC_BY_NETWORK[self._network]).rstrip("/")
        self._interval = max(30, int(probe_interval_sec))
        self._settle_energy = settle_energy
        self._headroom = min_settles_headroom
        self._failure_threshold = failure_threshold
        self._rpc = rpc_client or httpx.AsyncClient(timeout=httpx.Timeout(10.0, connect=5.0))
        self._owns_rpc = rpc_client is None
        self.status = FacilitatorStatus()
        self._task: Optional[asyncio.Task] = None

    # ---- lifecycle ----

    def start(self) -> None:
        if self._task is None:
            self._task = asyncio.create_task(self._loop(), name="facilitator-watch")

    async def stop(self) -> None:
        if self._task is not None:
            self._task.cancel()
            try:
                await self._task
            except (asyncio.CancelledError, Exception):
                pass
            self._task = None
        if self._owns_rpc:
            await self._rpc.aclose()

    async def _loop(self) -> None:
        while True:
            try:
                await self.probe()
            except Exception:  # never let the loop die
                logger.warning("facilitator watch probe crashed", exc_info=True)
            await asyncio.sleep(self._interval)

    # ---- signals from the app ----

    def record_settle_outcome(self, success: bool, error: Optional[str] = None) -> None:
        s = self.status
        if success:
            s.consecutive_settle_failures = 0
            s.last_settle_error = None
            if s.reason == "settle_failures":
                s.ok, s.reason = True, None
            return
        s.consecutive_settle_failures += 1
        s.last_settle_error = error
        if s.consecutive_settle_failures >= self._failure_threshold:
            s.ok, s.reason = False, "settle_failures"
            logger.error(
                "facilitator marked unavailable: %d consecutive settle failures (last: %s)",
                s.consecutive_settle_failures, error,
            )

    # ---- probe ----

    async def probe(self) -> FacilitatorStatus:
        s = self.status
        s.checked_at = time.time()

        # 1) service + capability
        try:
            supported = await self._get_json("/supported")
        except Exception as e:
            return self._verdict(False, f"supported_unreachable: {e}")
        kinds = supported.get("kinds") or []
        kind = next(
            (k for k in kinds if isinstance(k, dict)
             and k.get("scheme") == "exact" and k.get("network") == self._network),
            None,
        )
        if kind is None:
            return self._verdict(False, "network_or_scheme_not_supported")
        methods = (kind.get("extra") or {}).get("supportedAssetTransferMethods") or []
        if "permit2" not in methods:
            return self._verdict(False, "permit2_not_supported")
        signers = (supported.get("signers") or {}).get("tron:*") or (
            supported.get("signers") or {}
        ).get(self._network) or []
        relayer = (kind.get("extra") or {}).get("permit2FacilitatorAddress") or (signers[0] if signers else None)
        s.relayer = relayer

        # 2) relayer resources (best effort: RPC trouble is "unknown", not "down")
        if relayer:
            try:
                res = await self._post_rpc("/wallet/getaccountresource", {"address": relayer, "visible": True})
                acc = await self._post_rpc("/wallet/getaccount", {"address": relayer, "visible": True})
            except Exception as e:
                logger.warning("relayer resource probe failed: %s", e)
                if s.consecutive_settle_failures >= self._failure_threshold:
                    return self._verdict(False, "settle_failures")
                return self._verdict(True, None)
            energy_avail = int(res.get("EnergyLimit", 0)) - int(res.get("EnergyUsed", 0))
            trx_sun = int(acc.get("balance", 0))
            s.relayer_energy_available = energy_avail
            s.relayer_trx_sun = trx_sun
            need_energy = self._settle_energy * self._headroom
            # 100 sun/energy (TRON since 2025); burning TRX substitutes for staked energy.
            need_sun = need_energy * 100
            if energy_avail < need_energy and trx_sun < need_sun:
                return self._verdict(
                    False,
                    f"relayer_low_resources: energy={energy_avail} trx_sun={trx_sun} "
                    f"(need energy>={need_energy} or trx_sun>={need_sun})",
                )

        # 3) our own failure streak overrides a green probe
        if s.consecutive_settle_failures >= self._failure_threshold:
            return self._verdict(False, "settle_failures")
        return self._verdict(True, None)

    def _verdict(self, ok: bool, reason: Optional[str]) -> FacilitatorStatus:
        prev = self.status.ok
        self.status.ok, self.status.reason = ok, reason
        if prev != ok:
            (logger.error if not ok else logger.info)(
                "facilitator watch: ok=%s reason=%s relayer=%s energy=%s",
                ok, reason, self.status.relayer, self.status.relayer_energy_available,
            )
        return self.status

    async def _post_rpc(self, path: str, body: dict) -> dict:
        r = await self._rpc.post(self._rpc_url + path, json=body)
        r.raise_for_status()
        data = r.json()
        return data if isinstance(data, dict) else {}
