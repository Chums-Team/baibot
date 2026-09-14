"""SQLite layer for payment-id mapping + webhook idempotency."""
from __future__ import annotations

from datetime import datetime, timedelta, timezone
from uuid import uuid4

import pytest

from x402_sidecar import db


@pytest.fixture
def conn():
    c = db.open_in_memory()
    yield c
    c.close()


def test_insert_and_get_payment(conn):
    pid = uuid4()
    expires = datetime.now(timezone.utc) + timedelta(minutes=10)
    db.insert_payment(
        conn,
        payment_id=pid,
        room_id="!a:b",
        user_mxid="@u:b",
        amount_usd=0.10,
        correlation_id=None,
        expires_at=expires,
    )
    row = db.get_payment(conn, pid)
    assert row is not None
    assert row["payment_id"] == pid
    assert row["room_id"] == "!a:b"
    assert row["status"] == "pending"
    assert row["amount_usd"] == 0.10
    assert row["correlation_id"] is None


def test_get_payment_returns_none_for_unknown(conn):
    assert db.get_payment(conn, uuid4()) is None


def test_mark_settled_only_acts_on_pending(conn):
    pid = uuid4()
    db.insert_payment(
        conn,
        payment_id=pid,
        room_id="!a:b",
        user_mxid="@u:b",
        amount_usd=1.0,
        correlation_id=None,
        expires_at=datetime.now(timezone.utc) + timedelta(minutes=5),
    )
    db.mark_settled(conn, pid, tx_hash="0xabc", settled_at=datetime.now(timezone.utc))
    row = db.get_payment(conn, pid)
    assert row["status"] == "settled"
    assert row["tx_hash"] == "0xabc"

    # Second mark — current code is "WHERE status = 'pending'", so it's a no-op.
    db.mark_settled(conn, pid, tx_hash="0xdef", settled_at=datetime.now(timezone.utc))
    row = db.get_payment(conn, pid)
    assert row["tx_hash"] == "0xabc"  # unchanged


def test_webhook_seen_and_record(conn):
    pid = uuid4()
    assert db.webhook_seen(conn, "wh-1") is False
    db.record_webhook(conn, webhook_id="wh-1", payment_id=pid)
    assert db.webhook_seen(conn, "wh-1") is True
    # Double-record is a no-op (idempotent INSERT).
    db.record_webhook(conn, webhook_id="wh-1", payment_id=pid)
    assert db.webhook_seen(conn, "wh-1") is True
