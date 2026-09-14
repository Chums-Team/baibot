"""HMAC-SHA256 helpers used on the bot ↔ sidecar boundary.

Both sides know `X402_INTERNAL_SECRET`. Each request from sidecar to bot
(POST /internal/x402-settled) carries:

    X-Internal-Signature: <hex(hmac_sha256(secret, raw_body))>

The receiver validates with constant-time compare. This stops on-host
attackers from crafting a fake settlement webhook against the bot's
internal port.
"""
from __future__ import annotations

import hmac
import hashlib


def sign(secret: str, body: bytes) -> str:
    """Produce the value we put in `X-Internal-Signature`."""
    if not secret:
        raise ValueError("X402_INTERNAL_SECRET is empty — refuse to sign")
    digest = hmac.new(secret.encode("utf-8"), body, hashlib.sha256).hexdigest()
    return digest


def verify(secret: str, body: bytes, signature: str) -> bool:
    """Constant-time check. False if any input is empty."""
    if not secret or not signature:
        return False
    try:
        expected = sign(secret, body)
    except ValueError:
        return False
    return hmac.compare_digest(expected, signature)


def verify_facilitator_header(secret: str, body: bytes, header: str | None) -> bool:
    """Validate the inbound facilitator settlement signature.

    Header format (case-insensitive scheme prefix):

        X-Facilitator-Signature: sha256=<hex(hmac_sha256(secret, raw_body))>

    Returns False on any of: empty header, missing/wrong scheme prefix,
    secret/body mismatch, or non-hex digest. Comparison is constant-time
    via `hmac.compare_digest` inside `verify(...)`.
    """
    if not header:
        return False
    raw = header.strip()
    # Tolerate both `sha256=...` and bare hex (some facilitator stacks omit
    # the scheme); but never accept the empty-suffix `sha256=` literal.
    if "=" in raw:
        scheme, _, digest = raw.partition("=")
        if scheme.strip().lower() != "sha256":
            return False
        digest = digest.strip()
    else:
        digest = raw
    if not digest:
        return False
    return verify(secret, body, digest)
