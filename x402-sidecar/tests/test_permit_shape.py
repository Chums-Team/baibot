"""Permit2 envelope shape pins (x402 v2, TRON `exact`).

Ground truth (dereferenced 2026-09-12):
  * `external-repos/BofAI--x402-contracts/contracts/x402ExactPermit2Proxy.sol`
      WITNESS_TYPE_STRING = "Witness witness)TokenPermissions(address token,uint256 amount)
                             Witness(address to,uint256 validAfter)"
      WITNESS_TYPEHASH    = keccak256("Witness(address to,uint256 validAfter)")
  * Permit2 `PermitHash.sol` (canonical): the witness variant hashes
      "PermitWitnessTransferFrom(TokenPermissions permitted,address spender,
       uint256 nonce,uint256 deadline," + WITNESS_TYPE_STRING
  * `external-repos/BofAI--x402/typescript/packages/mechanisms/tron/src/constants.ts`
      `permit2WitnessTypes`, and `exact/client/permit2.ts` (domain shape).

Any drift here silently produces a different digest: the signature
recovers to a stranger and settle reverts with InvalidSigner.
"""
from __future__ import annotations

import importlib
import time
from uuid import uuid4

import pytest

# EIP-712 `encodeType` for the primary struct: primary first, then
# referenced structs alphabetically. This string is exactly what Permit2
# hashes on-chain when the proxy passes WITNESS_TYPE_STRING.
EXPECTED_PRIMARY_TYPESTRING = (
    "PermitWitnessTransferFrom(TokenPermissions permitted,address spender,"
    "uint256 nonce,uint256 deadline,Witness witness)"
    "TokenPermissions(address token,uint256 amount)"
    "Witness(address to,uint256 validAfter)"
)
EXPECTED_WITNESS_TYPESTRING = "Witness(address to,uint256 validAfter)"
EXPECTED_TOKEN_PERMISSIONS_TYPESTRING = "TokenPermissions(address token,uint256 amount)"
EXPECTED_DOMAIN_TYPESTRING = (
    "EIP712Domain(string name,uint256 chainId,address verifyingContract)"
)
# Suffix the proxy contract appends (must equal the tail of the primary string).
CONTRACT_WITNESS_TYPE_STRING = (
    "Witness witness)TokenPermissions(address token,uint256 amount)"
    "Witness(address to,uint256 validAfter)"
)


def _typestring(types: dict, name: str) -> str:
    """Canonical EIP-712 encodeType: named struct first, then referenced
    structs alphabetically (hand-rolled; no eth-account dependency here)."""

    def render(struct_name: str) -> str:
        return struct_name + "(" + ",".join(
            f"{f['type']} {f['name']}" for f in types[struct_name]
        ) + ")"

    referenced: set[str] = set()

    def walk(struct_name: str) -> None:
        for f in types[struct_name]:
            t = f["type"].rstrip("[]")
            if t in types and t != name and t not in referenced:
                referenced.add(t)
                walk(t)

    walk(name)
    return render(name) + "".join(render(s) for s in sorted(referenced))


@pytest.fixture
def base_env(monkeypatch):
    monkeypatch.setenv("X402_FACILITATOR_API_KEY", "test-key" * 4)
    monkeypatch.setenv("X402_INTERNAL_SECRET", "secret-32" * 4)
    monkeypatch.setenv("X402_FACILITATOR_WEBHOOK_SECRET", "fac-webhook-32" * 4)
    monkeypatch.setenv("X402_AGENT_WALLET", "TSkeaPMSuaojcCzE7mWqw4xN1awU7NfdfY")
    return monkeypatch


def _build(monkeypatch, network: str, amount_usd: float = 0.10):
    monkeypatch.setenv("X402_NETWORK", network)
    from x402_sidecar import config as config_mod

    importlib.reload(config_mod)
    config_mod._settings = None
    from x402_sidecar.facilitator_client import FacilitatorClient

    settings = config_mod.Settings()  # type: ignore[call-arg]
    client = FacilitatorClient(settings)
    return client.build_payment_request(
        amount_usd=amount_usd, room_id="!a:b", payment_id=uuid4()
    )


# ---- type strings ----


def test_primary_typestring_matches_permit2_witness_hash(base_env):
    build = _build(base_env, "tron:0x2b6653dc")
    types = build.permit_payload["types"]
    assert build.permit_payload["primaryType"] == "PermitWitnessTransferFrom"
    assert _typestring(types, "PermitWitnessTransferFrom") == EXPECTED_PRIMARY_TYPESTRING
    # The tail after "uint256 deadline," is byte-identical to the proxy's
    # WITNESS_TYPE_STRING constant.
    assert EXPECTED_PRIMARY_TYPESTRING.endswith(CONTRACT_WITNESS_TYPE_STRING)


def test_witness_and_token_permissions_typestrings(base_env):
    build = _build(base_env, "tron:0x2b6653dc")
    types = build.permit_payload["types"]
    assert _typestring(types, "Witness") == EXPECTED_WITNESS_TYPESTRING
    assert _typestring(types, "TokenPermissions") == EXPECTED_TOKEN_PERMISSIONS_TYPESTRING


def test_domain_is_three_fields_named_permit2(base_env):
    """Permit2's domain has no `version`; name is the literal "Permit2"."""
    build = _build(base_env, "tron:0x2b6653dc")
    payload = build.permit_payload
    assert _typestring(payload["types"], "EIP712Domain") == EXPECTED_DOMAIN_TYPESTRING
    assert set(payload["domain"]) == {"name", "chainId", "verifyingContract"}
    assert payload["domain"]["name"] == "Permit2"


# ---- addresses ----


def _is_0x_hex_addr(v: object) -> bool:
    return (
        isinstance(v, str)
        and v.startswith("0x")
        and len(v) == 42
        and v == v.lower()
        and all(c in "0123456789abcdef" for c in v[2:])
    )


def test_all_typed_data_addresses_are_lowercase_evm_hex(base_env):
    build = _build(base_env, "tron:0x2b6653dc")
    msg = build.permit_payload["message"]
    for label, v in [
        ("domain.verifyingContract", build.permit_payload["domain"]["verifyingContract"]),
        ("message.permitted.token", msg["permitted"]["token"]),
        ("message.spender", msg["spender"]),
        ("message.witness.to", msg["witness"]["to"]),
    ]:
        assert _is_0x_hex_addr(v), f"{label}={v!r}"


@pytest.mark.parametrize(
    "network,permit2,proxy,usdt",
    [
        (
            "tron:0x2b6653dc",
            "TTJxU3P8rHycAyFY4kVtGNfmnMH4ezcuM9",
            "TN49yaJmZMZoEdDCqjB4uPzQLHvYkGw95m",
            "TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t",
        ),
        (
            "tron:0xcd8690dc",
            "TYQuuhGbEMxF7nZxUHV3uHJxAVVAegNU9h",
            "TFGoaq2KjizijgjtkVxT7yjffW1A5T1j6F",
            "TXYZopYRdj2D9XRtbG411XZZ3kM5VkAeBf",
        ),
    ],
)
def test_contracts_resolve_per_network(base_env, network, permit2, proxy, usdt):
    from x402_sidecar.utils import normalize_to_evm_hex

    build = _build(base_env, network)
    payload = build.permit_payload
    assert build.permit2_contract == permit2
    assert build.proxy_contract == proxy
    assert build.asset_contract == usdt
    assert payload["domain"]["verifyingContract"] == normalize_to_evm_hex(permit2)
    assert payload["message"]["spender"] == normalize_to_evm_hex(proxy)
    assert payload["message"]["permitted"]["token"] == normalize_to_evm_hex(usdt)


def test_witness_to_is_agent_wallet(base_env):
    from x402_sidecar.utils import normalize_to_evm_hex

    build = _build(base_env, "tron:0x2b6653dc")
    assert build.permit_payload["message"]["witness"]["to"] == normalize_to_evm_hex(
        "TSkeaPMSuaojcCzE7mWqw4xN1awU7NfdfY"
    )


# ---- chain ids + aliases ----


@pytest.mark.parametrize(
    "network,chain_id,canonical",
    [
        ("tron:0x2b6653dc", 728126428, "tron:0x2b6653dc"),
        ("tron:0xcd8690dc", 3448148188, "tron:0xcd8690dc"),
        ("tron:mainnet", 728126428, "tron:0x2b6653dc"),
        ("tron:nile", 3448148188, "tron:0xcd8690dc"),
    ],
)
def test_chain_id_and_alias_canonicalisation(base_env, network, chain_id, canonical):
    build = _build(base_env, network)
    assert build.permit_payload["domain"]["chainId"] == chain_id
    assert build.network == canonical
    assert build.payment_requirements["network"] == canonical


# ---- numerics / requirements ----


def test_amount_in_micros_everywhere(base_env):
    build = _build(base_env, "tron:0x2b6653dc", amount_usd=0.10)
    assert build.permit_payload["message"]["permitted"]["amount"] == "100000"
    assert build.payment_requirements["amount"] == "100000"


def test_nonce_deadline_window(base_env):
    build = _build(base_env, "tron:0x2b6653dc")
    msg = build.permit_payload["message"]
    now = int(time.time())
    nonce = int(msg["nonce"])
    deadline = int(msg["deadline"])
    assert msg["witness"]["validAfter"] == "0"
    assert 0 < nonce < 2**256 and nonce.bit_length() > 200
    assert 595 <= deadline - now <= 605
    assert build.deadline == deadline
    assert build.nonce == nonce
    assert int(build.expires_at.timestamp()) == deadline


def test_payment_requirements_shape(base_env):
    build = _build(base_env, "tron:0xcd8690dc", amount_usd=0.01)
    req = build.payment_requirements
    assert req == {
        "scheme": "exact",
        "network": "tron:0xcd8690dc",
        "amount": "10000",
        "asset": "TXYZopYRdj2D9XRtbG411XZZ3kM5VkAeBf",
        "payTo": "TSkeaPMSuaojcCzE7mWqw4xN1awU7NfdfY",
        "maxTimeoutSeconds": 600,
        "extra": {"assetTransferMethod": "permit2"},
    }


def test_message_has_no_owner_field(base_env):
    """Permit2 recovers the owner from the signature — the client must
    sign the envelope as-is, no buyer substitution."""
    build = _build(base_env, "tron:0xcd8690dc")
    assert set(build.permit_payload["message"]) == {
        "permitted", "spender", "nonce", "deadline", "witness"
    }
