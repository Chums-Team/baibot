"""Permit2 typed-data + x402 v2 wire builders for the TRON `exact` scheme.

Single place that knows the byte-exact shapes, so the sidecar, its
tests, and (via the fixture under `tests/fixtures/`) the Flutter client
stay aligned.

Ground truth (dereferenced 2026-09-12):
  * TIP-712 types + domain:
    `external-repos/BofAI--x402/typescript/packages/mechanisms/tron/src/constants.ts`
    (`permit2WitnessTypes`) and `exact/client/permit2.ts`
    (`signPermit2Authorization`: domain `{name:"Permit2", chainId, verifyingContract}`).
  * Witness type string on-chain:
    `external-repos/BofAI--x402-contracts/contracts/x402ExactPermit2Proxy.sol`
    (`WITNESS_TYPEHASH = keccak256("Witness(address to,uint256 validAfter)")`).
  * Wire shape (`paymentRequirements`, `payload.permit2Authorization`):
    `external-repos/BofAI--x402/specs/schemes/exact/scheme_exact_tron.md`.
  * Facilitator request envelope (`x402Version`, `paymentPayload`,
    `paymentRequirements`): `external-repos/BofAI--x402-facilitator/src/server.ts`
    (`SettleBodySchema`, `.strict()` — no extra top-level keys).

Numeric fields inside `message` are emitted as **decimal strings** so
the envelope survives JSON in Rust/Dart/JS without precision loss; every
consumer (TronLink `signTypedData`, eth_account, our Dart encoder)
coerces strings to uint256.
"""
from __future__ import annotations

import os
from datetime import datetime, timedelta, timezone
from typing import Any

# 20-byte EVM zero address.
ZERO_EVM_ADDRESS = "0x" + "00" * 20

# USDT-TRC20 has 6 decimals.
_USDT_DECIMALS = 6
_USDT_MULTIPLIER = 10 ** _USDT_DECIMALS

# Deadline window for the signed permit (= `maxTimeoutSeconds`). The
# facilitator only requires >= 6s of remaining validity at verify time;
# mainnet GasFree caps at ~595s, so 600s keeps us portable across schemes.
DEFAULT_MAX_TIMEOUT_SECONDS = 600

SCHEME_EXACT = "exact"
ASSET_TRANSFER_METHOD_PERMIT2 = "permit2"
PRIMARY_TYPE = "PermitWitnessTransferFrom"
DOMAIN_NAME = "Permit2"
X402_VERSION = 2

# Pinned by `tests/test_permit_shape.py` against the contract's
# WITNESS_TYPE_STRING. Order inside each struct is load-bearing.
PERMIT2_TYPES: dict[str, list[dict[str, str]]] = {
    "EIP712Domain": [
        {"name": "name", "type": "string"},
        {"name": "chainId", "type": "uint256"},
        {"name": "verifyingContract", "type": "address"},
    ],
    "PermitWitnessTransferFrom": [
        {"name": "permitted", "type": "TokenPermissions"},
        {"name": "spender", "type": "address"},
        {"name": "nonce", "type": "uint256"},
        {"name": "deadline", "type": "uint256"},
        {"name": "witness", "type": "Witness"},
    ],
    "TokenPermissions": [
        {"name": "token", "type": "address"},
        {"name": "amount", "type": "uint256"},
    ],
    "Witness": [
        {"name": "to", "type": "address"},
        {"name": "validAfter", "type": "uint256"},
    ],
}


def make_nonce() -> int:
    """Random uint256 Permit2 nonce (unordered nonce bitmap on-chain)."""
    return int.from_bytes(os.urandom(32), "big")


def usd_to_micros(amount_usd: float) -> int:
    """USD float -> USDT-TRC20 smallest units (6 decimals)."""
    return int(round(amount_usd * _USDT_MULTIPLIER))


def make_deadline(
    *, now: datetime | None = None, max_timeout_seconds: int = DEFAULT_MAX_TIMEOUT_SECONDS
) -> tuple[int, datetime]:
    """Return `(deadline_unix, expires_at_dt)` = now + max_timeout_seconds."""
    if now is None:
        now = datetime.now(timezone.utc)
    expires_at = now + timedelta(seconds=max_timeout_seconds)
    return int(expires_at.timestamp()), expires_at


def build_permit2_typed_data(
    *,
    chain_id: int,
    permit2_hex: str,
    proxy_hex: str,
    token_hex: str,
    amount_micros: int,
    pay_to_hex: str,
    nonce: int,
    deadline: int,
    valid_after: int = 0,
    include_stub_marker: bool = False,
) -> dict[str, Any]:
    """The envelope the client signs (`primaryType`/`types`/`domain`/`message`).

    Note there is **no `from`/owner field** in the signed message: Permit2
    recovers the owner from the signature. The client therefore signs the
    envelope as-is (no buyer substitution, unlike the legacy PaymentPermit).
    """
    return {
        "primaryType": PRIMARY_TYPE,
        "types": PERMIT2_TYPES,
        "domain": {
            "name": DOMAIN_NAME,
            "chainId": chain_id,
            "verifyingContract": permit2_hex,
        },
        "message": {
            "permitted": {"token": token_hex, "amount": str(amount_micros)},
            "spender": proxy_hex,
            "nonce": str(nonce),
            "deadline": str(deadline),
            "witness": {"to": pay_to_hex, "validAfter": str(valid_after)},
        },
        **({"_stub": True} if include_stub_marker else {}),
    }


def build_payment_requirements(
    *,
    network: str,
    amount_micros: int,
    asset_b58: str,
    pay_to_b58: str,
    max_timeout_seconds: int = DEFAULT_MAX_TIMEOUT_SECONDS,
) -> dict[str, Any]:
    """`PaymentRequirements` (v2) for TRON `exact` via Permit2."""
    return {
        "scheme": SCHEME_EXACT,
        "network": network,
        "amount": str(amount_micros),
        "asset": asset_b58,
        "payTo": pay_to_b58,
        "maxTimeoutSeconds": int(max_timeout_seconds),
        "extra": {"assetTransferMethod": ASSET_TRANSFER_METHOD_PERMIT2},
    }


def build_permit2_authorization(
    *,
    buyer_hex: str,
    token_hex: str,
    amount_micros: int,
    proxy_hex: str,
    nonce: int,
    deadline: int,
    pay_to_hex: str,
    valid_after: int = 0,
) -> dict[str, Any]:
    """`payload.permit2Authorization` the facilitator verifies + settles."""
    return {
        "from": buyer_hex,
        "permitted": {"token": token_hex, "amount": str(amount_micros)},
        "spender": proxy_hex,
        "nonce": str(nonce),
        "deadline": str(deadline),
        "witness": {"to": pay_to_hex, "validAfter": str(valid_after)},
    }


def build_facilitator_body(
    *,
    requirements: dict[str, Any],
    authorization: dict[str, Any],
    signature_hex: str,
) -> dict[str, Any]:
    """Request body for both `POST /verify` and `POST /settle`."""
    return {
        "x402Version": X402_VERSION,
        "paymentPayload": {
            "x402Version": X402_VERSION,
            "accepted": requirements,
            "payload": {
                "signature": signature_hex,
                "permit2Authorization": authorization,
            },
        },
        "paymentRequirements": requirements,
    }


def normalize_signature_hex(sig: str) -> str:
    """`0x`-prefixed lowercase 65-byte hex. Raises ValueError on bad shape."""
    s = sig.strip().lower().removeprefix("0x")
    if len(s) != 130:
        raise ValueError(f"signature must be 65 bytes (130 hex chars), got {len(s)}")
    int(s, 16)
    return "0x" + s


def typed_data_mismatches(
    typed_data: dict[str, Any], authorization: dict[str, Any]
) -> list[str]:
    """Compare a client-returned envelope's `message` with the
    authorization we rebuilt from our own records. Returns a list of
    human-readable mismatches (empty = consistent).

    Defence in depth against client/sidecar drift (e.g. operator changed
    X402_NETWORK between /payment-request and /submit) — the facilitator
    would reject such a signature anyway, this just gives a clearer 400.
    """
    problems: list[str] = []
    msg = typed_data.get("message") if isinstance(typed_data, dict) else None
    if not isinstance(msg, dict):
        return ["permit_payload.message missing"]

    def _norm(v: Any) -> str:
        return str(v).strip().lower()

    def _int(v: Any) -> int | None:
        try:
            s = str(v).strip()
            return int(s, 16) if s.lower().startswith("0x") else int(s)
        except (TypeError, ValueError):
            return None

    permitted = msg.get("permitted") if isinstance(msg.get("permitted"), dict) else {}
    witness = msg.get("witness") if isinstance(msg.get("witness"), dict) else {}
    checks = [
        ("permitted.token", _norm(permitted.get("token")), _norm(authorization["permitted"]["token"])),
        ("spender", _norm(msg.get("spender")), _norm(authorization["spender"])),
        ("witness.to", _norm(witness.get("to")), _norm(authorization["witness"]["to"])),
    ]
    for name, got, want in checks:
        if got != want:
            problems.append(f"{name}: signed {got!r} != expected {want!r}")
    for name, got, want in [
        ("permitted.amount", _int(permitted.get("amount")), _int(authorization["permitted"]["amount"])),
        ("nonce", _int(msg.get("nonce")), _int(authorization["nonce"])),
        ("deadline", _int(msg.get("deadline")), _int(authorization["deadline"])),
        ("witness.validAfter", _int(witness.get("validAfter")), _int(authorization["witness"]["validAfter"])),
    ]:
        if got != want:
            problems.append(f"{name}: signed {got!r} != expected {want!r}")
    return problems
