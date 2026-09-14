"""Tiny SQLite layer for the sidecar's payment-id mapping + webhook idempotency.

Sync `sqlite3` is fine here — sidecar is single-process FastAPI, and the
work per call is one SELECT + maybe one INSERT.
"""
from __future__ import annotations

import sqlite3
from contextlib import contextmanager
from datetime import datetime, timezone
from importlib import resources
from pathlib import Path
from typing import Iterator
from uuid import UUID

_MIGRATION_FILES = ("001_init.sql", "002_permit2.sql")

# (column, declaration) pairs added by 002. Applied programmatically because
# SQLite lacks `ADD COLUMN IF NOT EXISTS`.
_PAYMENT_COLUMNS_V2: tuple[tuple[str, str], ...] = (
    ("nonce", "TEXT"),
    ("deadline", "INTEGER"),
    ("network", "TEXT"),
    ("buyer", "TEXT"),
    ("error_reason", "TEXT"),
)


def _migration_sql(name: str) -> str:
    package = "x402_sidecar.migrations"
    return resources.files(package).joinpath(name).read_text(encoding="utf-8")


def _ensure_columns(conn: sqlite3.Connection) -> None:
    existing = {row[1] for row in conn.execute("PRAGMA table_info(payments)")}
    for col, decl in _PAYMENT_COLUMNS_V2:
        if col not in existing:
            conn.execute(f"ALTER TABLE payments ADD COLUMN {col} {decl}")


def _apply_migrations(conn: sqlite3.Connection) -> None:
    conn.executescript(_migration_sql("001_init.sql"))
    _ensure_columns(conn)
    conn.executescript(_migration_sql("002_permit2.sql"))


def open_db(path: str | Path) -> sqlite3.Connection:
    """Open or create the DB at `path` and apply migrations.

    `path` of `":memory:"` yields a private in-memory DB.
    """
    if str(path) == ":memory:":
        return open_in_memory()
    p = Path(path)
    p.parent.mkdir(parents=True, exist_ok=True)
    conn = sqlite3.connect(str(p), check_same_thread=False, isolation_level=None)
    conn.execute("PRAGMA journal_mode = WAL")
    conn.execute("PRAGMA synchronous = NORMAL")
    _apply_migrations(conn)
    return conn


def open_in_memory() -> sqlite3.Connection:
    conn = sqlite3.connect(":memory:", check_same_thread=False, isolation_level=None)
    _apply_migrations(conn)
    return conn


@contextmanager
def transaction(conn: sqlite3.Connection) -> Iterator[sqlite3.Connection]:
    """Explicit BEGIN / COMMIT, rollback on exception."""
    conn.execute("BEGIN")
    try:
        yield conn
    except Exception:
        conn.execute("ROLLBACK")
        raise
    else:
        conn.execute("COMMIT")


def _iso(dt: datetime) -> str:
    if dt.tzinfo is None:
        dt = dt.replace(tzinfo=timezone.utc)
    return dt.astimezone(timezone.utc).strftime("%Y-%m-%dT%H:%M:%S.%fZ")[:-4] + "Z"


def _parse_iso(s: str) -> datetime:
    for fmt in ("%Y-%m-%dT%H:%M:%S.%fZ", "%Y-%m-%dT%H:%M:%SZ"):
        try:
            return datetime.strptime(s, fmt).replace(tzinfo=timezone.utc)
        except ValueError:
            continue
    return datetime.fromisoformat(s.replace("Z", "+00:00"))


# ----- payments -----

_PAYMENT_COLS = [
    "payment_id",
    "room_id",
    "user_mxid",
    "amount_usd",
    "correlation_id",
    "status",
    "tx_hash",
    "expires_at",
    "created_at",
    "settled_at",
    "nonce",
    "deadline",
    "network",
    "buyer",
    "error_reason",
]


def insert_payment(
    conn: sqlite3.Connection,
    *,
    payment_id: UUID,
    room_id: str,
    user_mxid: str,
    amount_usd: float,
    correlation_id: UUID | None,
    expires_at: datetime,
    nonce: int | str | None = None,
    deadline: int | None = None,
    network: str | None = None,
) -> None:
    conn.execute(
        "INSERT INTO payments "
        "(payment_id, room_id, user_mxid, amount_usd, correlation_id, expires_at, "
        " nonce, deadline, network) "
        "VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        (
            str(payment_id),
            room_id,
            user_mxid,
            round(amount_usd, 6),
            str(correlation_id) if correlation_id else None,
            _iso(expires_at),
            str(nonce) if nonce is not None else None,
            deadline,
            network,
        ),
    )


def _row_to_payment(row: tuple) -> dict:
    out = dict(zip(_PAYMENT_COLS, row))
    out["expires_at"] = _parse_iso(out["expires_at"])
    out["created_at"] = _parse_iso(out["created_at"])
    if out["settled_at"]:
        out["settled_at"] = _parse_iso(out["settled_at"])
    if out["correlation_id"]:
        out["correlation_id"] = UUID(out["correlation_id"])
    out["payment_id"] = UUID(out["payment_id"])
    return out


def get_payment(conn: sqlite3.Connection, payment_id: UUID) -> dict | None:
    row = conn.execute(
        f"SELECT {', '.join(_PAYMENT_COLS)} FROM payments WHERE payment_id = ?",
        (str(payment_id),),
    ).fetchone()
    return _row_to_payment(row) if row is not None else None


def get_payment_by_nonce(conn: sqlite3.Connection, nonce: int | str) -> dict | None:
    row = conn.execute(
        f"SELECT {', '.join(_PAYMENT_COLS)} FROM payments WHERE nonce = ?",
        (str(nonce),),
    ).fetchone()
    return _row_to_payment(row) if row is not None else None


def mark_settled(
    conn: sqlite3.Connection,
    payment_id: UUID,
    *,
    tx_hash: str,
    settled_at: datetime,
    buyer: str | None = None,
) -> None:
    conn.execute(
        "UPDATE payments SET status = 'settled', tx_hash = ?, settled_at = ?, "
        "buyer = COALESCE(?, buyer), error_reason = NULL "
        "WHERE payment_id = ? AND status = 'pending'",
        (tx_hash, _iso(settled_at), buyer, str(payment_id)),
    )


def record_rejection(
    conn: sqlite3.Connection,
    payment_id: UUID,
    *,
    error_reason: str,
    buyer: str | None = None,
) -> None:
    """Facilitator refused verify/settle. Row stays `pending` so the user
    can retry (e.g. after topping up USDT); we keep the last reason."""
    conn.execute(
        "UPDATE payments SET error_reason = ?, buyer = COALESCE(?, buyer) "
        "WHERE payment_id = ? AND status = 'pending'",
        (error_reason[:500], buyer, str(payment_id)),
    )


# ----- webhook idempotency -----


def webhook_seen(conn: sqlite3.Connection, webhook_id: str) -> bool:
    row = conn.execute(
        "SELECT 1 FROM webhooks_received WHERE webhook_id = ?",
        (webhook_id,),
    ).fetchone()
    return row is not None


def record_webhook(
    conn: sqlite3.Connection, *, webhook_id: str, payment_id: UUID
) -> None:
    """Returns silently if the webhook_id is already present (idempotent INSERT)."""
    conn.execute(
        "INSERT OR IGNORE INTO webhooks_received (webhook_id, payment_id) VALUES (?, ?)",
        (webhook_id, str(payment_id)),
    )
