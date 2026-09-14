-- x402 v2 / Permit2 (2026-09-12). Column additions are applied from db.py
-- (SQLite has no `ADD COLUMN IF NOT EXISTS`); this file only carries the
-- statements that are idempotent on their own.
--
-- New columns on `payments` (see db.py::_ensure_columns):
--   nonce        TEXT     -- Permit2 nonce (uint256, decimal string). Correlation key
--                         -- with the facilitator: GET /payments?network=&nonce=
--   deadline     INTEGER  -- permit deadline (unix seconds) = expires_at
--   network      TEXT     -- hex CAIP-2 id the envelope was built for
--   buyer        TEXT     -- payer address (Base58) from the last /submit
--   error_reason TEXT     -- last facilitator verify/settle rejection reason

CREATE UNIQUE INDEX IF NOT EXISTS idx_payments_nonce ON payments(nonce);
