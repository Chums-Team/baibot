"""Tests for utils.py — TRON address conversion + per-network config.

Cross-references against `tronpy.keys.to_hex_address` would be ideal but
`tronpy` is not a dependency. The fixed vectors below come from the BofAI
facilitator's network tables, so any drift in our own decoder shows up here
without an extra dependency.
"""
from __future__ import annotations

import pytest

from x402_sidecar.utils import (
    TronAddressError,
    base58check_decode,
    canonical_network,
    chain_id_for,
    evm_hex_to_tron_base58,
    exact_proxy_for,
    legacy_network_name,
    normalize_to_base58,
    normalize_to_evm_hex,
    permit2_for,
    tron_base58_to_evm_hex,
    usdt_contract_for,
)


# Known good vectors (Base58Check ↔ EVM hex). USDT-TRC20 mainnet pair is
# pinned in `external-repos/BofAI--x402/.claude/rules/networks/tron.md`
# (`TR7NHqj…` ↔ `0x...`); the 0x form below is what `tronpy.keys
# .to_hex_address` returns minus the leading `41`.
USDT_MAINNET_BASE58 = "TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t"
USDT_MAINNET_HEX = "0xa614f803b6fd780986a42c78ec9c7f77e6ded13c"

PERMIT_MAINNET_BASE58 = "TT8rEWbCoNX7vpEUauxb7rWJsTgs8vDLAn"
AGENT_WALLET_BASE58 = "TSkeaPMSuaojcCzE7mWqw4xN1awU7NfdfY"


class TestBase58Check:
    def test_decoder_returns_21_byte_payload_for_tron_address(self) -> None:
        payload = base58check_decode(USDT_MAINNET_BASE58)
        assert len(payload) == 21
        # TRON addresses always start with the 0x41 version byte.
        assert payload[0] == 0x41

    def test_rejects_corrupted_address(self) -> None:
        # Flip the last char — checksum validation must catch it.
        bad = USDT_MAINNET_BASE58[:-1] + (
            "u" if USDT_MAINNET_BASE58[-1] != "u" else "v"
        )
        with pytest.raises(TronAddressError, match="checksum"):
            base58check_decode(bad)

    def test_rejects_non_base58_characters(self) -> None:
        with pytest.raises(TronAddressError, match="non-Base58"):
            base58check_decode("0OIl11111111111111111111111111")  # 0/O/I/l illegal

    def test_rejects_empty(self) -> None:
        with pytest.raises(TronAddressError):
            base58check_decode("")


class TestTronBase58ToEvmHex:
    def test_usdt_mainnet_round_trip_matches_known_hex(self) -> None:
        # Pinned against BofAI's `.claude/rules/networks/tron.md` USDT
        # entry (mainnet `TR7NHqj…`). If this drifts, every permit on
        # mainnet would settle against a different address — so this is
        # the load-bearing assertion of the whole utils module.
        assert tron_base58_to_evm_hex(USDT_MAINNET_BASE58) == USDT_MAINNET_HEX

    def test_strips_tron_version_byte(self) -> None:
        out = tron_base58_to_evm_hex(PERMIT_MAINNET_BASE58)
        assert out.startswith("0x")
        assert len(out) == 42  # 0x + 40 hex chars (20 bytes)

    def test_lowercase_output(self) -> None:
        out = tron_base58_to_evm_hex(AGENT_WALLET_BASE58)
        assert out == out.lower()


class TestNormalizeToEvmHex:
    def test_passes_through_evm_hex_lowercased(self) -> None:
        upper = "0x" + "AB" * 20
        assert normalize_to_evm_hex(upper) == upper.lower()

    def test_converts_tron_base58(self) -> None:
        assert normalize_to_evm_hex(USDT_MAINNET_BASE58) == USDT_MAINNET_HEX

    def test_rejects_non_address(self) -> None:
        with pytest.raises(TronAddressError):
            normalize_to_evm_hex("not-an-address")

    def test_rejects_truncated_evm_hex(self) -> None:
        with pytest.raises(TronAddressError):
            normalize_to_evm_hex("0x123")

    def test_rejects_non_hex_after_0x(self) -> None:
        # 42 chars but garbage after 0x.
        with pytest.raises(ValueError):
            normalize_to_evm_hex("0x" + "z" * 40)


class TestNetworkLookups:
    def test_chain_ids_match_sdk_constants(self) -> None:
        # `mechanisms/tron/src/constants.ts` TRON_CHAIN_IDS.
        assert chain_id_for("tron:0x2b6653dc") == 728_126_428
        assert chain_id_for("tron:0xcd8690dc") == 3_448_148_188
        assert chain_id_for("tron:0x94a9059e") == 2_494_104_990

    def test_legacy_aliases_canonicalise(self) -> None:
        assert canonical_network("tron:mainnet") == "tron:0x2b6653dc"
        assert canonical_network("tron:nile") == "tron:0xcd8690dc"
        assert canonical_network("tron:shasta") == "tron:0x94a9059e"
        assert canonical_network("tron:0xCD8690DC") == "tron:0xcd8690dc"
        assert chain_id_for("tron:nile") == 3_448_148_188
        assert legacy_network_name("tron:0xcd8690dc") == "tron:nile"

    def test_unknown_network_raises(self) -> None:
        with pytest.raises(TronAddressError):
            chain_id_for("tron:fake")
        with pytest.raises(TronAddressError):
            canonical_network("eip155:56")

    def test_permit2_and_proxy_per_network(self) -> None:
        assert permit2_for("tron:0x2b6653dc") == "TTJxU3P8rHycAyFY4kVtGNfmnMH4ezcuM9"
        assert permit2_for("tron:0xcd8690dc") == "TYQuuhGbEMxF7nZxUHV3uHJxAVVAegNU9h"
        assert exact_proxy_for("tron:0x2b6653dc") == "TN49yaJmZMZoEdDCqjB4uPzQLHvYkGw95m"
        assert exact_proxy_for("tron:0xcd8690dc") == "TFGoaq2KjizijgjtkVxT7yjffW1A5T1j6F"
        assert exact_proxy_for("tron:nile") == "TFGoaq2KjizijgjtkVxT7yjffW1A5T1j6F"

    def test_usdt_contract_per_network(self) -> None:
        assert usdt_contract_for("tron:0x2b6653dc") == "TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t"
        assert usdt_contract_for("tron:0xcd8690dc") == "TXYZopYRdj2D9XRtbG411XZZ3kM5VkAeBf"


class TestBase58RoundTrip:
    def test_hex_to_base58_round_trips(self) -> None:
        assert evm_hex_to_tron_base58(USDT_MAINNET_HEX) == USDT_MAINNET_BASE58
        assert normalize_to_base58(USDT_MAINNET_HEX) == USDT_MAINNET_BASE58
        assert normalize_to_base58(USDT_MAINNET_BASE58) == USDT_MAINNET_BASE58

    def test_vector_buyer_round_trip(self) -> None:
        # From tests/fixtures/permit2_nile_vector_2026_09_12.json.
        assert normalize_to_evm_hex("TGVvAYNsroS1c25q2gvmoSFkvEdy3Qdmbz") == (
            "0x479f985951446f042d2ddaa4b3013ff909852ba9"
        )
        assert normalize_to_base58("0x479f985951446f042d2ddaa4b3013ff909852ba9") == (
            "TGVvAYNsroS1c25q2gvmoSFkvEdy3Qdmbz"
        )
