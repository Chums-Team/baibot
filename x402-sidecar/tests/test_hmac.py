"""auth.sign / auth.verify — bot ↔ sidecar integrity."""
from __future__ import annotations

import pytest

from himari_x402_sidecar import auth


def test_sign_is_deterministic():
    s = "shared-secret"
    body = b'{"hello":"world"}'
    assert auth.sign(s, body) == auth.sign(s, body)


def test_sign_changes_with_body():
    a = auth.sign("k", b"a")
    b = auth.sign("k", b"b")
    assert a != b


def test_sign_changes_with_secret():
    body = b"x"
    assert auth.sign("k1", body) != auth.sign("k2", body)


def test_verify_accepts_valid():
    body = b"hello"
    sig = auth.sign("secret", body)
    assert auth.verify("secret", body, sig) is True


def test_verify_rejects_tampered_body():
    sig = auth.sign("secret", b"hello")
    assert auth.verify("secret", b"hellp", sig) is False


def test_verify_rejects_wrong_secret():
    sig = auth.sign("secret", b"hello")
    assert auth.verify("other", b"hello", sig) is False


def test_verify_rejects_empty_secret():
    assert auth.verify("", b"x", "abcd") is False


def test_verify_rejects_empty_signature():
    assert auth.verify("secret", b"x", "") is False


def test_sign_refuses_empty_secret():
    with pytest.raises(ValueError):
        auth.sign("", b"x")
