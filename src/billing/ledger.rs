//! `BillingService` — async wrapper around a SQLite connection.
//!
//! Implementation strategy: keep a single `rusqlite::Connection` behind an
//! `Arc<Mutex<_>>`. Each public async method does the SQL work inside
//! `spawn_blocking`. SQLite is single-writer regardless, so the mutex
//! costs nothing in practice and keeps the surface 100% safe.

use std::path::Path;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Datelike, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc};
use rusqlite::{Connection, params};
use uuid::Uuid;

use super::types::{BillingError, BillingEvent, BillingEventType, Period, ZombieReserve};

const SCHEMA: &str = include_str!("schema.sql");

#[derive(Clone)]
pub struct BillingService {
    /// `pub(super)` so sibling test module can hit the raw connection to
    /// verify the append-only triggers fire. External callers must go
    /// through the public API.
    pub(super) inner: Arc<Mutex<Connection>>,
}

impl BillingService {
    /// Open or create the on-disk DB at `path` and apply the schema.
    /// Parent directories are NOT created — caller is responsible.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, BillingError> {
        let conn = Connection::open(path.as_ref())?;
        Self::init(conn)
    }

    /// In-memory DB for tests. Each instance is isolated.
    pub fn open_in_memory() -> Result<Self, BillingError> {
        let conn = Connection::open_in_memory()?;
        Self::init(conn)
    }

    fn init(conn: Connection) -> Result<Self, BillingError> {
        // Foreign keys + reasonable journaling for our embedded write rate.
        conn.execute_batch(
            "PRAGMA foreign_keys = ON;\
             PRAGMA journal_mode = WAL;\
             PRAGMA synchronous = NORMAL;",
        )?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self {
            inner: Arc::new(Mutex::new(conn)),
        })
    }

    // -------- Reads --------

    pub async fn compute_balance(&self, room_id: &str) -> Result<f64, BillingError> {
        let inner = self.inner.clone();
        let room_id = room_id.to_owned();
        spawn_blocking(move || {
            let conn = inner.lock().expect("billing mutex poisoned");
            let value: f64 = conn
                .query_row(
                    "SELECT COALESCE(SUM(amount_usd), 0.0) FROM billing_events WHERE room_id = ?1",
                    params![room_id],
                    |row| row.get(0),
                )
                .map_err(BillingError::from)?;
            Ok(value)
        })
        .await
    }

    /// Sum of `charge` rows in the current calendar period (UTC).
    /// Returned as a positive number (cost spent).
    pub async fn compute_period_spent(&self, period: Period) -> Result<f64, BillingError> {
        let start = period_start(period, Utc::now());
        let inner = self.inner.clone();
        spawn_blocking(move || {
            let conn = inner.lock().expect("billing mutex poisoned");
            let value: f64 = conn
                .query_row(
                    "SELECT COALESCE(-SUM(amount_usd), 0.0) FROM billing_events \
                     WHERE type = 'charge' AND created_at >= ?1",
                    params![iso8601(start)],
                    |row| row.get(0),
                )
                .map_err(BillingError::from)?;
            Ok(value)
        })
        .await
    }

    pub async fn recent_events(
        &self,
        room_id: &str,
        limit: usize,
    ) -> Result<Vec<BillingEvent>, BillingError> {
        let inner = self.inner.clone();
        let room_id = room_id.to_owned();
        spawn_blocking(move || {
            let conn = inner.lock().expect("billing mutex poisoned");
            let mut stmt = conn.prepare(
                "SELECT event_id, room_id, type, amount_usd, correlation_id, \
                        user_mxid, matrix_event_id, meta_json, created_at \
                 FROM billing_events \
                 WHERE room_id = ?1 \
                 ORDER BY created_at DESC, event_id DESC \
                 LIMIT ?2",
            )?;
            let rows = stmt.query_map(params![room_id, limit as i64], event_from_row)?;
            let mut out = Vec::new();
            for r in rows {
                out.push(r.map_err(BillingError::from)?);
            }
            Ok(out)
        })
        .await
    }

    /// Has a `topup` event ever been credited for this `payment_id`?
    /// Used by the x402 webhook handler for cross-repo idempotency
    /// so even a hostile/buggy sidecar can't double-credit.
    pub async fn topup_exists_by_payment_id(
        &self,
        payment_id: uuid::Uuid,
    ) -> Result<bool, BillingError> {
        let inner = self.inner.clone();
        spawn_blocking(move || {
            let conn = inner.lock().expect("billing mutex poisoned");
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM billing_events \
                     WHERE type = 'topup' \
                       AND json_extract(meta_json, '$.payment_id') = ?1",
                    params![payment_id.to_string()],
                    |row| row.get(0),
                )
                .map_err(BillingError::from)?;
            Ok(count > 0)
        })
        .await
    }

    pub async fn list_zombies(
        &self,
        older_than_minutes: u32,
    ) -> Result<Vec<ZombieReserve>, BillingError> {
        let inner = self.inner.clone();
        spawn_blocking(move || {
            let conn = inner.lock().expect("billing mutex poisoned");
            // A "zombie" is a reserve whose correlation_id has neither a
            // matching charge nor release row, and that is older than the
            // threshold. Older = created_at < (now - threshold).
            let cutoff = Utc::now() - chrono::Duration::minutes(older_than_minutes as i64);
            let mut stmt = conn.prepare(
                "SELECT r.event_id, r.room_id, r.type, r.amount_usd, r.correlation_id, \
                        r.user_mxid, r.matrix_event_id, r.meta_json, r.created_at \
                 FROM billing_events r \
                 WHERE r.type = 'reserve' \
                   AND r.created_at <= ?1 \
                   AND NOT EXISTS ( \
                       SELECT 1 FROM billing_events x \
                       WHERE x.correlation_id = r.correlation_id \
                         AND x.type IN ('charge', 'release') \
                   ) \
                 ORDER BY r.created_at ASC",
            )?;
            let rows = stmt.query_map(params![iso8601(cutoff)], event_from_row)?;
            let now = Utc::now();
            let mut out = Vec::new();
            for r in rows {
                let event = r.map_err(BillingError::from)?;
                let age_seconds = now.signed_duration_since(event.created_at).num_seconds();
                out.push(ZombieReserve { event, age_seconds });
            }
            Ok(out)
        })
        .await
    }

    // -------- Writes --------

    pub async fn reserve(
        &self,
        room_id: &str,
        correlation_id: Uuid,
        amount: f64,
        user_mxid: Option<&str>,
        matrix_event_id: Option<&str>,
    ) -> Result<BillingEvent, BillingError> {
        let inner = self.inner.clone();
        let room_id = room_id.to_owned();
        let user_mxid = user_mxid.map(str::to_owned);
        let matrix_event_id = matrix_event_id.map(str::to_owned);
        spawn_blocking(move || {
            let mut conn = inner.lock().expect("billing mutex poisoned");
            let tx = conn.transaction()?;

            // Refuse duplicate reserve for the same correlation_id.
            let exists: i64 = tx.query_row(
                "SELECT COUNT(*) FROM billing_events \
                 WHERE correlation_id = ?1 AND type = 'reserve'",
                params![correlation_id.to_string()],
                |row| row.get(0),
            )?;
            if exists > 0 {
                return Err(BillingError::DuplicateReserve(correlation_id));
            }

            let event = build_event(
                BillingEventType::Reserve,
                &room_id,
                -amount.abs(),
                Some(correlation_id),
                user_mxid.as_deref(),
                matrix_event_id.as_deref(),
                serde_json::json!({}),
            );
            insert(&tx, &event)?;
            tx.commit()?;
            Ok(event)
        })
        .await
    }

    pub async fn charge(
        &self,
        correlation_id: Uuid,
        charged_usd: f64,
        meta: serde_json::Value,
    ) -> Result<BillingEvent, BillingError> {
        let inner = self.inner.clone();
        spawn_blocking(move || {
            let mut conn = inner.lock().expect("billing mutex poisoned");
            let tx = conn.transaction()?;

            // Find the matching reserve to inherit room_id / user_mxid /
            // matrix_event_id (they should match by definition).
            let reserve = match find_reserve(&tx, correlation_id)? {
                Some(r) => r,
                None => return Err(BillingError::ReserveNotFound(correlation_id)),
            };

            let prior_charge: i64 = tx.query_row(
                "SELECT COUNT(*) FROM billing_events \
                 WHERE correlation_id = ?1 AND type = 'charge'",
                params![correlation_id.to_string()],
                |row| row.get(0),
            )?;
            if prior_charge > 0 {
                return Err(BillingError::DuplicateCharge(correlation_id));
            }

            let event = build_event(
                BillingEventType::Charge,
                &reserve.room_id,
                -charged_usd.abs(),
                Some(correlation_id),
                reserve.user_mxid.as_deref(),
                reserve.matrix_event_id.as_deref(),
                meta,
            );
            insert(&tx, &event)?;
            tx.commit()?;
            Ok(event)
        })
        .await
    }

    pub async fn release(&self, correlation_id: Uuid) -> Result<BillingEvent, BillingError> {
        let inner = self.inner.clone();
        spawn_blocking(move || {
            let mut conn = inner.lock().expect("billing mutex poisoned");
            let tx = conn.transaction()?;

            let reserve = match find_reserve(&tx, correlation_id)? {
                Some(r) => r,
                None => return Err(BillingError::ReserveNotFound(correlation_id)),
            };

            let prior_release: i64 = tx.query_row(
                "SELECT COUNT(*) FROM billing_events \
                 WHERE correlation_id = ?1 AND type = 'release'",
                params![correlation_id.to_string()],
                |row| row.get(0),
            )?;
            if prior_release > 0 {
                return Err(BillingError::DuplicateRelease(correlation_id));
            }

            let event = build_event(
                BillingEventType::Release,
                &reserve.room_id,
                reserve.amount_usd.abs(), // restore the held amount
                Some(correlation_id),
                reserve.user_mxid.as_deref(),
                reserve.matrix_event_id.as_deref(),
                serde_json::json!({}),
            );
            insert(&tx, &event)?;
            tx.commit()?;
            Ok(event)
        })
        .await
    }

    pub async fn topup(
        &self,
        room_id: &str,
        amount: f64,
        user_mxid: Option<&str>,
        meta: serde_json::Value,
    ) -> Result<BillingEvent, BillingError> {
        let inner = self.inner.clone();
        let room_id = room_id.to_owned();
        let user_mxid = user_mxid.map(str::to_owned);
        // Pull the payment_id out of meta so we can re-emit it
        // verbatim if the partial UNIQUE index in
        // `002_payment_id_unique.sql` rejects the INSERT — converts a
        // raw SQLITE_CONSTRAINT_UNIQUE into the typed
        // BillingError::DuplicatePaymentId the webhook handler expects.
        let payment_id_for_err = meta
            .get("payment_id")
            .and_then(|v| v.as_str())
            .and_then(|s| Uuid::parse_str(s).ok());
        spawn_blocking(move || {
            let mut conn = inner.lock().expect("billing mutex poisoned");
            let tx = conn.transaction()?;
            let event = build_event(
                BillingEventType::Topup,
                &room_id,
                amount.abs(),
                None,
                user_mxid.as_deref(),
                None,
                meta,
            );
            match insert(&tx, &event) {
                Ok(()) => {}
                Err(BillingError::Sqlite(rusqlite::Error::SqliteFailure(err, msg)))
                    if err.code == rusqlite::ErrorCode::ConstraintViolation
                        && err.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE
                        && msg
                            .as_deref()
                            .is_some_and(|m| m.contains("idx_billing_topup_payment_id")) =>
                {
                    // Roll back the open tx so the next call doesn't
                    // see a poisoned transaction state.
                    let _ = tx.rollback();
                    return Err(match payment_id_for_err {
                        Some(pid) => BillingError::DuplicatePaymentId(pid),
                        None => BillingError::Sqlite(rusqlite::Error::SqliteFailure(err, msg)),
                    });
                }
                Err(other) => return Err(other),
            }
            tx.commit()?;
            Ok(event)
        })
        .await
    }

    pub async fn manual_release(
        &self,
        reserve_event_id: Uuid,
        admin_mxid: &str,
        reason: &str,
    ) -> Result<BillingEvent, BillingError> {
        let inner = self.inner.clone();
        let admin_mxid = admin_mxid.to_owned();
        let reason = reason.to_owned();
        spawn_blocking(move || {
            let mut conn = inner.lock().expect("billing mutex poisoned");
            let tx = conn.transaction()?;

            // Look up the reserve by its event_id.
            let mut stmt = tx.prepare(
                "SELECT event_id, room_id, type, amount_usd, correlation_id, \
                        user_mxid, matrix_event_id, meta_json, created_at \
                 FROM billing_events \
                 WHERE event_id = ?1 AND type = 'reserve'",
            )?;
            let reserve_opt = stmt
                .query_row(params![reserve_event_id.to_string()], event_from_row)
                .map(Some)
                .or_else(|e| match e {
                    rusqlite::Error::QueryReturnedNoRows => Ok(None),
                    other => Err(other),
                })?;
            drop(stmt);
            let reserve = match reserve_opt {
                Some(r) => r,
                None => return Err(BillingError::ReserveEventNotFound(reserve_event_id)),
            };
            let correlation_id = reserve.correlation_id.ok_or_else(|| {
                // A reserve without correlation_id is a data invariant violation.
                BillingError::ReserveNotFound(reserve_event_id)
            })?;

            let prior_release: i64 = tx.query_row(
                "SELECT COUNT(*) FROM billing_events \
                 WHERE correlation_id = ?1 AND type = 'release'",
                params![correlation_id.to_string()],
                |row| row.get(0),
            )?;
            if prior_release > 0 {
                return Err(BillingError::DuplicateRelease(correlation_id));
            }

            let event = build_event(
                BillingEventType::Release,
                &reserve.room_id,
                reserve.amount_usd.abs(),
                Some(correlation_id),
                reserve.user_mxid.as_deref(),
                reserve.matrix_event_id.as_deref(),
                serde_json::json!({
                    "manual": true,
                    "admin_mxid": admin_mxid,
                    "reason": reason,
                }),
            );
            insert(&tx, &event)?;
            tx.commit()?;
            Ok(event)
        })
        .await
    }

    pub async fn manual_refund(
        &self,
        room_id: &str,
        amount: f64,
        admin_mxid: &str,
        reason: &str,
    ) -> Result<BillingEvent, BillingError> {
        let inner = self.inner.clone();
        let room_id = room_id.to_owned();
        let admin_mxid = admin_mxid.to_owned();
        let reason = reason.to_owned();
        spawn_blocking(move || {
            let mut conn = inner.lock().expect("billing mutex poisoned");
            let tx = conn.transaction()?;
            let event = build_event(
                BillingEventType::RefundManual,
                &room_id,
                amount.abs(),
                None,
                Some(&admin_mxid),
                None,
                serde_json::json!({
                    "admin_mxid": admin_mxid,
                    "reason": reason,
                }),
            );
            insert(&tx, &event)?;
            tx.commit()?;
            Ok(event)
        })
        .await
    }
}

// -------- helpers --------

fn build_event(
    event_type: BillingEventType,
    room_id: &str,
    amount_usd: f64,
    correlation_id: Option<Uuid>,
    user_mxid: Option<&str>,
    matrix_event_id: Option<&str>,
    meta: serde_json::Value,
) -> BillingEvent {
    BillingEvent {
        event_id: Uuid::now_v7(),
        room_id: room_id.to_owned(),
        event_type,
        amount_usd,
        correlation_id,
        user_mxid: user_mxid.map(str::to_owned),
        matrix_event_id: matrix_event_id.map(str::to_owned),
        meta,
        created_at: Utc::now(),
    }
}

fn insert(tx: &rusqlite::Transaction<'_>, event: &BillingEvent) -> Result<(), BillingError> {
    tx.execute(
        "INSERT INTO billing_events ( \
            event_id, room_id, type, amount_usd, correlation_id, \
            user_mxid, matrix_event_id, meta_json, created_at \
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            event.event_id.to_string(),
            event.room_id,
            event.event_type.as_str(),
            event.amount_usd,
            event.correlation_id.map(|u| u.to_string()),
            event.user_mxid,
            event.matrix_event_id,
            serde_json::to_string(&event.meta)?,
            iso8601(event.created_at),
        ],
    )?;
    Ok(())
}

fn find_reserve(
    tx: &rusqlite::Transaction<'_>,
    correlation_id: Uuid,
) -> Result<Option<BillingEvent>, BillingError> {
    let mut stmt = tx.prepare(
        "SELECT event_id, room_id, type, amount_usd, correlation_id, \
                user_mxid, matrix_event_id, meta_json, created_at \
         FROM billing_events \
         WHERE correlation_id = ?1 AND type = 'reserve' \
         LIMIT 1",
    )?;
    let opt = stmt
        .query_row(params![correlation_id.to_string()], event_from_row)
        .map(Some)
        .or_else(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            other => Err(other),
        })?;
    Ok(opt)
}

fn event_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<BillingEvent> {
    let event_id_s: String = row.get(0)?;
    let event_id = Uuid::parse_str(&event_id_s).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
    })?;
    let room_id: String = row.get(1)?;
    let type_s: String = row.get(2)?;
    let event_type = BillingEventType::parse(&type_s).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            2,
            rusqlite::types::Type::Text,
            format!("unknown billing event type: {type_s}").into(),
        )
    })?;
    let amount_usd: f64 = row.get(3)?;
    let correlation_id_opt: Option<String> = row.get(4)?;
    let correlation_id = correlation_id_opt
        .map(|s| Uuid::parse_str(&s))
        .transpose()
        .map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(4, rusqlite::types::Type::Text, Box::new(e))
        })?;
    let user_mxid: Option<String> = row.get(5)?;
    let matrix_event_id: Option<String> = row.get(6)?;
    let meta_json: String = row.get(7)?;
    let meta: serde_json::Value = serde_json::from_str(&meta_json).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(7, rusqlite::types::Type::Text, Box::new(e))
    })?;
    let created_at_s: String = row.get(8)?;
    let created_at = parse_iso8601(&created_at_s).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(8, rusqlite::types::Type::Text, e.into())
    })?;
    Ok(BillingEvent {
        event_id,
        room_id,
        event_type,
        amount_usd,
        correlation_id,
        user_mxid,
        matrix_event_id,
        meta,
        created_at,
    })
}

fn iso8601(ts: DateTime<Utc>) -> String {
    // Match the schema's strftime format ('%Y-%m-%dT%H:%M:%fZ' = millis precision).
    ts.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
}

fn parse_iso8601(s: &str) -> Result<DateTime<Utc>, String> {
    // Accept either millis ('%.3f') or seconds.
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .or_else(|_| {
            // Fallback for SQLite's default datetime() returning seconds-precision.
            chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S")
                .map(|n| Utc.from_utc_datetime(&n))
                .or_else(|_| {
                    chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.fZ")
                        .map(|n| Utc.from_utc_datetime(&n))
                })
        })
        .map_err(|e| format!("could not parse '{s}' as datetime: {e}"))
}

fn period_start(period: Period, now: DateTime<Utc>) -> DateTime<Utc> {
    match period {
        Period::Day => {
            let d: NaiveDate = now.date_naive();
            Utc.from_utc_datetime(&NaiveDateTime::new(d, NaiveTime::MIN))
        }
        Period::Month => {
            let d = NaiveDate::from_ymd_opt(now.year(), now.month(), 1)
                .expect("year/month always valid");
            Utc.from_utc_datetime(&NaiveDateTime::new(d, NaiveTime::MIN))
        }
    }
}

async fn spawn_blocking<F, R>(f: F) -> Result<R, BillingError>
where
    F: FnOnce() -> Result<R, BillingError> + Send + 'static,
    R: Send + 'static,
{
    tokio::task::spawn_blocking(f).await?
}
