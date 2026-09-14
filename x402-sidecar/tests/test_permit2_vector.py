"""Ground-truth vector: a Permit2 permit that the hosted BofAI x402
facilitator v2 verified AND settled on TRON nile (2026-09-12).

Pins three things at once:
  1. our builder reproduces the exact typed data that was signed;
  2. the canonical EIP-712 digest of that typed data equals the recorded
     one (so any Dart/Rust encoder can be checked against the same number);
  3. ecrecover(digest, signature) == the buyer the facilitator reported.

Requires `eth-account` (dev extra); skipped when absent.
"""
from __future__ import annotations

import json
from pathlib import Path

import pytest

eth_account = pytest.importorskip("eth_account")
from eth_account._utils.signing import to_standard_v  # noqa: E402
from eth_account.messages import encode_typed_data  # noqa: E402
from eth_keys import keys  # noqa: E402

VECTOR = json.loads(
    (Path(__file__).parent / "fixtures" / "permit2_nile_vector_2026_09_12.json").read_text()
)


def _types_without_domain(types: dict) -> dict:
    return {k: v for k, v in types.items() if k != "EIP712Domain"}


def _message_ints(auth: dict) -> dict:
    return {
        "permitted": {"token": auth["permitted"]["token"], "amount": int(auth["permitted"]["amount"])},
        "spender": auth["spender"],
        "nonce": int(auth["nonce"]),
        "deadline": int(auth["deadline"]),
        "witness": {"to": auth["witness"]["to"], "validAfter": int(auth["witness"]["validAfter"])},
    }


def test_builder_reproduces_signed_typed_data():
    from x402_sidecar.permit2_builder import build_permit2_typed_data
    from x402_sidecar.utils import chain_id_for

    auth = VECTOR["permit2Authorization"]
    built = build_permit2_typed_data(
        chain_id=chain_id_for(VECTOR["network"]),
        permit2_hex=VECTOR["domain"]["verifyingContract"],
        proxy_hex=auth["spender"],
        token_hex=auth["permitted"]["token"],
        amount_micros=int(auth["permitted"]["amount"]),
        pay_to_hex=auth["witness"]["to"],
        nonce=int(auth["nonce"]),
        deadline=int(auth["deadline"]),
    )
    assert built["domain"] == VECTOR["domain"]
    assert _types_without_domain(built["types"]) == VECTOR["types"]
    assert built["message"] == {k: v for k, v in auth.items() if k != "from"}


def test_digest_matches_recorded():
    typed = encode_typed_data(
        domain_data=VECTOR["domain"],
        message_types=VECTOR["types"],
        message_data=_message_ints(VECTOR["permit2Authorization"]),
    )
    # `encode_typed_data` returns a SignableMessage; the EIP-712 digest is
    # keccak(0x19 0x01 || domainSeparator || hashStruct(message)).
    from eth_account.messages import _hash_eip191_message

    digest = "0x" + _hash_eip191_message(typed).hex()
    assert digest == VECTOR["digest"]


def test_signature_recovers_to_buyer():
    from x402_sidecar.utils import normalize_to_evm_hex

    typed = encode_typed_data(
        domain_data=VECTOR["domain"],
        message_types=VECTOR["types"],
        message_data=_message_ints(VECTOR["permit2Authorization"]),
    )
    from eth_account.messages import _hash_eip191_message

    digest = _hash_eip191_message(typed)
    sig = bytes.fromhex(VECTOR["signature"][2:])
    r, s, v = int.from_bytes(sig[:32], "big"), int.from_bytes(sig[32:64], "big"), sig[64]
    signature = keys.Signature(vrs=(to_standard_v(v), r, s))
    recovered = signature.recover_public_key_from_msg_hash(digest).to_address()
    assert recovered.lower() == VECTOR["permit2Authorization"]["from"]
    assert recovered.lower() == VECTOR["verify_response"]["payer"]
    assert recovered.lower() == normalize_to_evm_hex(VECTOR["buyer_base58"])


def test_wire_body_matches_what_the_facilitator_accepted():
    from x402_sidecar.permit2_builder import build_facilitator_body

    body = build_facilitator_body(
        requirements=VECTOR["paymentRequirements"],
        authorization=VECTOR["permit2Authorization"],
        signature_hex=VECTOR["signature"],
    )
    assert body["paymentRequirements"] == VECTOR["paymentRequirements"]
    assert body["paymentPayload"]["payload"]["permit2Authorization"] == VECTOR["permit2Authorization"]
    assert VECTOR["settle_response"]["success"] is True
    assert VECTOR["lookup_response"][0]["nonce"] == VECTOR["permit2Authorization"]["nonce"]
    assert VECTOR["lookup_response"][0]["txHash"] == VECTOR["settle_response"]["transaction"]
