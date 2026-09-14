"""TRON address conversion + per-network constants for the x402 v2 stack.

Networks are CAIP-2 ids with a **hex chain reference** (BofAI x402 SDK
>= 1.0.1): `tron:0x2b6653dc` (mainnet), `tron:0xcd8690dc` (nile),
`tron:0x94a9059e` (shasta). The legacy names (`tron:mainnet`,
`tron:nile`, `tron:shasta`) are still accepted as **input aliases** so
old `.env` files keep working, but every value we emit on the wire is
the hex form.

Every `address`-typed field inside TIP-712 typed data is **0x-prefixed
EVM hex** (TRON `0x41` version byte stripped). Addresses in
`paymentRequirements` (`asset`, `payTo`) stay TRON Base58, matching
`external-repos/BofAI--x402/specs/schemes/exact/scheme_exact_tron.md`.

Base58Check decoding is ~30 lines of stdlib (`hashlib.sha256`); we do
not pull `tronpy` for one decoder.
"""
from __future__ import annotations

import hashlib

# ---- network ids ----

# legacy name -> canonical hex CAIP-2 id
_HEX_BY_ALIAS: dict[str, str] = {
    "tron:mainnet": "tron:0x2b6653dc",
    "tron:shasta": "tron:0x94a9059e",
    "tron:nile": "tron:0xcd8690dc",
}
_ALIAS_BY_HEX: dict[str, str] = {v: k for k, v in _HEX_BY_ALIAS.items()}

# Per `external-repos/BofAI--x402/typescript/packages/mechanisms/tron/src/constants.ts`
# (TRON_CHAIN_IDS). The numeric chainId is the hex reference as unsigned int.
_CHAIN_ID_BY_NETWORK: dict[str, int] = {
    "tron:0x2b6653dc": 728_126_428,
    "tron:0x94a9059e": 2_494_104_990,
    "tron:0xcd8690dc": 3_448_148_188,
}

# Canonical Permit2 deployments (TIP-712 `verifyingContract`; approve target).
# Source: constants.ts PERMIT2_ADDRESSES + specs/schemes/exact/scheme_exact_tron.md.
_PERMIT2_BY_NETWORK: dict[str, str] = {
    "tron:0x2b6653dc": "TTJxU3P8rHycAyFY4kVtGNfmnMH4ezcuM9",
    "tron:0xcd8690dc": "TYQuuhGbEMxF7nZxUHV3uHJxAVVAegNU9h",
    "tron:0x94a9059e": "TJMkP7a3ucTMkvi17p7ChhTCw6zriFX3tg",
}

# x402ExactPermit2Proxy (the required `spender` in the signed permit; the
# facilitator calls `settle(...)` on it).
# Source: constants.ts X402_PERMIT2_PROXY_ADDRESSES + x402-contracts README.
_EXACT_PROXY_BY_NETWORK: dict[str, str] = {
    "tron:0x2b6653dc": "TN49yaJmZMZoEdDCqjB4uPzQLHvYkGw95m",
    "tron:0xcd8690dc": "TFGoaq2KjizijgjtkVxT7yjffW1A5T1j6F",
    "tron:0x94a9059e": "TGZkC38n14f2GpBWPMQLF2BpmcpWW3QNhg",
}

# USDT-TRC20 per network (Base58).
_USDT_BY_NETWORK: dict[str, str] = {
    "tron:0x2b6653dc": "TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t",
    "tron:0x94a9059e": "TG3XXyExBkPp9nzdajDZsozEu4BkaSJozs",
    "tron:0xcd8690dc": "TXYZopYRdj2D9XRtbG411XZZ3kM5VkAeBf",
}

# Bitcoin / TRON Base58 alphabet (identical).
_BASE58_ALPHABET = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"
_BASE58_INDEX = {c: i for i, c in enumerate(_BASE58_ALPHABET)}


class TronAddressError(ValueError):
    """Raised when an address / network id can't be parsed."""


# ---- Base58Check ----


def base58check_decode(s: str) -> bytes:
    """Decode a Base58Check string, validating the 4-byte SHA256d checksum.

    Returns the payload with its version prefix (21 bytes for TRON:
    `0x41` + 20-byte address). Checksum bytes are stripped.
    """
    if not s:
        raise TronAddressError("empty Base58 string")
    n = 0
    for ch in s:
        idx = _BASE58_INDEX.get(ch)
        if idx is None:
            raise TronAddressError(f"non-Base58 char in address: {ch!r}")
        n = n * 58 + idx
    leading_zeros = len(s) - len(s.lstrip("1"))
    raw = n.to_bytes((n.bit_length() + 7) // 8, "big") if n else b""
    raw = b"\x00" * leading_zeros + raw
    if len(raw) < 5:
        raise TronAddressError(f"address too short after decode: {len(raw)} bytes")
    payload, checksum = raw[:-4], raw[-4:]
    expected = hashlib.sha256(hashlib.sha256(payload).digest()).digest()[:4]
    if checksum != expected:
        raise TronAddressError("Base58Check checksum mismatch — corrupt address")
    return payload


def base58check_encode(payload: bytes) -> str:
    """Encode payload (with version prefix) as Base58Check."""
    checksum = hashlib.sha256(hashlib.sha256(payload).digest()).digest()[:4]
    raw = payload + checksum
    n = int.from_bytes(raw, "big")
    out = ""
    while n > 0:
        n, rem = divmod(n, 58)
        out = _BASE58_ALPHABET[rem] + out
    leading = len(raw) - len(raw.lstrip(b"\x00"))
    return "1" * leading + out


def tron_base58_to_evm_hex(addr: str) -> str:
    """`T...` (34 chars) -> 0x-prefixed 20-byte EVM hex (lowercase)."""
    payload = base58check_decode(addr)
    if len(payload) != 21:
        raise TronAddressError(
            f"unexpected TRON payload length {len(payload)} (expected 21)"
        )
    if payload[0] != 0x41:
        raise TronAddressError(
            f"unexpected TRON version byte 0x{payload[0]:02x} (expected 0x41)"
        )
    return "0x" + payload[1:].hex()


def evm_hex_to_tron_base58(addr: str) -> str:
    """0x-prefixed 20-byte hex -> TRON Base58 (`T...`)."""
    hex_body = addr[2:] if addr.startswith("0x") else addr
    if len(hex_body) != 40:
        raise TronAddressError(f"expected 20-byte hex address, got {addr!r}")
    return base58check_encode(b"\x41" + bytes.fromhex(hex_body))


def normalize_to_evm_hex(addr: str) -> str:
    """Coerce Base58 (`T...`, 34) or 0x-hex (`0x...`, 42) -> lowercase 0x-hex."""
    if addr.startswith("0x") and len(addr) == 42:
        int(addr[2:], 16)  # raise on garbage
        return addr.lower()
    if addr.startswith("T") and len(addr) == 34:
        return tron_base58_to_evm_hex(addr)
    raise TronAddressError(
        f"address {addr!r} not in TRON Base58 (T..., 34) or EVM hex (0x..., 42) form"
    )


def normalize_to_base58(addr: str) -> str:
    """Coerce Base58 or 0x-hex -> TRON Base58."""
    if addr.startswith("T") and len(addr) == 34:
        base58check_decode(addr)  # validate checksum
        return addr
    return evm_hex_to_tron_base58(normalize_to_evm_hex(addr))


def addresses_equal(a: str, b: str) -> bool:
    """Compare two addresses regardless of Base58 / hex encoding."""
    try:
        return normalize_to_evm_hex(a) == normalize_to_evm_hex(b)
    except TronAddressError:
        return False


# ---- networks ----


def canonical_network(network: str) -> str:
    """Accept a legacy alias or a hex CAIP-2 id; return the hex id.

    Raises `TronAddressError` for anything we have no constants for.
    """
    n = network.strip()
    n = _HEX_BY_ALIAS.get(n, n)
    if n.startswith("tron:0x"):
        n = "tron:0x" + n[len("tron:0x"):].lower()
    if n not in _CHAIN_ID_BY_NETWORK:
        raise TronAddressError(
            f"unknown network {network!r}; expected one of "
            f"{sorted(_CHAIN_ID_BY_NETWORK)} or aliases {sorted(_HEX_BY_ALIAS)}"
        )
    return n


def legacy_network_name(network: str) -> str:
    """Hex id (or alias) -> human name `tron:nile` etc. (logs / health)."""
    return _ALIAS_BY_HEX[canonical_network(network)]


def chain_id_for(network: str) -> int:
    """Network id (alias or hex) -> numeric TIP-712 chainId."""
    return _CHAIN_ID_BY_NETWORK[canonical_network(network)]


def permit2_for(network: str) -> str:
    """Network id -> canonical Permit2 contract (Base58)."""
    return _PERMIT2_BY_NETWORK[canonical_network(network)]


def exact_proxy_for(network: str) -> str:
    """Network id -> x402ExactPermit2Proxy contract (Base58)."""
    return _EXACT_PROXY_BY_NETWORK[canonical_network(network)]


def usdt_contract_for(network: str) -> str:
    """Network id -> USDT-TRC20 token address (Base58)."""
    return _USDT_BY_NETWORK[canonical_network(network)]
