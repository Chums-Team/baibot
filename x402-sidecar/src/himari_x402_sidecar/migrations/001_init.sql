-- payment_id ↔ context mapping. Used to:
-- (a) reconnect a settlement webhook to the right room/user/correlation;
-- (b) provide GET /status/<id> read-back;
-- (c) enforce idempotency on incoming webhooks.

CREATE TABLE IF NOT EXISTS payments (
    payment_id     TEXT PRIMARY KEY,           -- UUID v7 (lex-sortable)
    room_id        TEXT NOT NULL,
    user_mxid      TEXT NOT NULL,
    amount_usd     REAL NOT NULL,
    correlation_id TEXT,                       -- bot's LLM-call id (optional)
    status         TEXT NOT NULL DEFAULT 'pending'  -- pending|settled|expired|failed
                   CHECK (status IN ('pending','settled','expired','failed')),
    tx_hash        TEXT,
    expires_at     TEXT NOT NULL,
    created_at     TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    settled_at     TEXT
);

CREATE INDEX IF NOT EXISTS idx_payments_status ON payments(status, created_at);
CREATE INDEX IF NOT EXISTS idx_payments_room   ON payments(room_id, created_at);

-- Facilitator-side anti-replay. We never process the same webhook_id twice,
-- regardless of payment_id. Webhook_ids that arrive after their TTL still
-- record here so audits show the late attempt.
CREATE TABLE IF NOT EXISTS webhooks_received (
    webhook_id  TEXT PRIMARY KEY,
    payment_id  TEXT NOT NULL,
    received_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);

CREATE INDEX IF NOT EXISTS idx_webhooks_payment ON webhooks_received(payment_id);
