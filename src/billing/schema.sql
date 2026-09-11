-- Append-only billing ledger.
-- Single source of truth for room balances. Balance = SUM(amount_usd) over
-- billing_events for a given room. UPDATE/DELETE are explicitly forbidden by
-- triggers below — operational fixes (zombies, refunds) are themselves new
-- INSERTs (`refund_manual`, `release` with admin meta).
--
-- Everything is created via IF NOT EXISTS, so the schema is idempotent and
-- safe to re-apply on every start.

CREATE TABLE IF NOT EXISTS billing_events (
    -- UUID v7 (lex-sortable by time). Stored as text, no need for binary.
    event_id          TEXT PRIMARY KEY,
    room_id           TEXT NOT NULL,
    type              TEXT NOT NULL CHECK (type IN (
        'reserve', 'charge', 'release', 'topup', 'refund_manual'
    )),
    -- Signed: reserve/charge < 0; release/topup/refund_manual > 0.
    amount_usd        REAL NOT NULL,
    -- Links reserve <-> charge <-> release for one LLM call.
    -- NULL for topup / refund_manual.
    correlation_id    TEXT,
    user_mxid         TEXT,
    matrix_event_id   TEXT,
    -- Free-form per-event metadata. JSON text. e.g. for charge: openrouter
    -- request_id, model, prompt_tokens, completion_tokens. For topup:
    -- tx_hash, sender_address. For manual_*: admin_mxid, reason.
    meta_json         TEXT NOT NULL DEFAULT '{}',
    -- ISO-8601 UTC. SQLite's strftime returns the right format.
    created_at        TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE INDEX IF NOT EXISTS idx_billing_room_time
    ON billing_events(room_id, created_at);
CREATE INDEX IF NOT EXISTS idx_billing_correlation
    ON billing_events(correlation_id)
    WHERE correlation_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_billing_type_time
    ON billing_events(type, created_at);

-- Append-only invariant. The ledger is forensic / financial — any mutation
-- after insert would invalidate audits. Operational fixes (e.g. release a
-- zombie reserve, refund a top-up) are new INSERTs of type 'release' /
-- 'refund_manual', not edits.
CREATE TRIGGER IF NOT EXISTS no_update_billing
BEFORE UPDATE ON billing_events
BEGIN
    SELECT RAISE(ABORT, 'append-only ledger: UPDATE forbidden');
END;

CREATE TRIGGER IF NOT EXISTS no_delete_billing
BEFORE DELETE ON billing_events
BEGIN
    SELECT RAISE(ABORT, 'append-only ledger: DELETE forbidden');
END;

-- Prevent double-crediting a top-up at the storage layer.
--
-- The x402 webhook handler already checks `topup_exists_by_payment_id` before
-- INSERT, but the check and the INSERT are not atomic — two concurrent
-- webhooks (different `webhook_id`, same `payment_id`) can both pass the
-- check and both INSERT. The application-level race is small but real
-- once the webhook server is exposed to true parallel POSTs.
--
-- The partial UNIQUE index makes the second INSERT fail with
-- SQLITE_CONSTRAINT_UNIQUE — `BillingService::topup` then maps that to
-- `BillingError::DuplicatePaymentId` and the webhook handler returns 409,
-- mirroring the same response code as the application-level dup check.
--
-- json_extract in a partial index is supported on SQLite 3.38+; we bundle
-- 3.46 via rusqlite's `bundled` feature so this is safe.

CREATE UNIQUE INDEX IF NOT EXISTS idx_billing_topup_payment_id
    ON billing_events(json_extract(meta_json, '$.payment_id'))
    WHERE type = 'topup'
      AND json_extract(meta_json, '$.payment_id') IS NOT NULL;
