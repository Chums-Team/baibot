//! Tests for the billing ledger. All tests use in-memory SQLite, so they
//! are isolated and fast. Each test gets its own `BillingService` instance.
//!
//! We use `tokio::test(flavor = "multi_thread")` because `spawn_blocking`
//! requires a real worker pool, not the single-thread current_thread runtime.

use super::*;
use serde_json::json;
use uuid::Uuid;

const ROOM: &str = "!test-room:example.com";
const USER: &str = "@user:example.com";
const MATRIX_EVT: &str = "$evt-abc";
const RESERVE: f64 = 0.03;

fn db() -> BillingService {
    BillingService::open_in_memory().expect("init in-memory ledger")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn balance_zero_on_empty_room() {
    let svc = db();
    let bal = svc.compute_balance(ROOM).await.unwrap();
    assert!(bal.abs() < f64::EPSILON, "expected zero balance, got {bal}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn topup_increases_balance() {
    let svc = db();
    svc.topup(ROOM, 5.0, Some(USER), json!({"txid": "0xabc"}))
        .await
        .unwrap();
    let bal = svc.compute_balance(ROOM).await.unwrap();
    assert!((bal - 5.0).abs() < 1e-9, "expected 5.0, got {bal}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reserve_charge_release_roundtrip() {
    let svc = db();
    svc.topup(ROOM, 10.0, Some(USER), json!({})).await.unwrap();
    let corr = Uuid::now_v7();
    svc.reserve(ROOM, corr, RESERVE, Some(USER), Some(MATRIX_EVT))
        .await
        .unwrap();
    // After reserve: balance = 10.0 - 0.03 = 9.97
    let bal = svc.compute_balance(ROOM).await.unwrap();
    assert!((bal - 9.97).abs() < 1e-9, "after reserve: {bal}");

    svc.charge(corr, 0.012, json!({"prompt_tokens": 100}))
        .await
        .unwrap();
    // After charge: 10.0 - 0.03 - 0.012 = 9.958
    let bal = svc.compute_balance(ROOM).await.unwrap();
    assert!((bal - 9.958).abs() < 1e-9, "after charge: {bal}");

    svc.release(corr).await.unwrap();
    // After release: 10.0 - 0.012 = 9.988
    let bal = svc.compute_balance(ROOM).await.unwrap();
    assert!((bal - 9.988).abs() < 1e-9, "after release: {bal}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn zombie_reserve_persists_when_charge_skipped() {
    let svc = db();
    svc.topup(ROOM, 1.0, Some(USER), json!({})).await.unwrap();
    let corr = Uuid::now_v7();
    svc.reserve(ROOM, corr, RESERVE, Some(USER), Some(MATRIX_EVT))
        .await
        .unwrap();
    // Imitate panic — we just don't call charge/release.
    let bal = svc.compute_balance(ROOM).await.unwrap();
    assert!((bal - 0.97).abs() < 1e-9, "zombie balance: {bal}");

    let zombies = svc.list_zombies(0).await.unwrap();
    assert_eq!(zombies.len(), 1, "should see 1 zombie reserve");
    assert_eq!(zombies[0].event.correlation_id, Some(corr));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_reserves_do_not_overlap() {
    let svc = db();
    svc.topup(ROOM, 100.0, Some(USER), json!({})).await.unwrap();

    let mut joins = tokio::task::JoinSet::new();
    for _ in 0..50 {
        let svc = svc.clone();
        joins.spawn(async move {
            let corr = Uuid::now_v7();
            svc.reserve(ROOM, corr, RESERVE, Some(USER), Some(MATRIX_EVT))
                .await
        });
    }
    let mut ok = 0;
    while let Some(res) = joins.join_next().await {
        if res.unwrap().is_ok() {
            ok += 1;
        }
    }
    assert_eq!(
        ok, 50,
        "all 50 reserves should succeed (each unique corr_id)"
    );

    let bal = svc.compute_balance(ROOM).await.unwrap();
    assert!(
        (bal - (100.0 - 50.0 * RESERVE)).abs() < 1e-9,
        "expected 100 - 50 * 0.03 = 98.5, got {bal}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn append_only_update_blocked_by_trigger() {
    let svc = db();
    svc.topup(ROOM, 1.0, Some(USER), json!({})).await.unwrap();

    // Reach into the inner connection and try to UPDATE — should ABORT.
    let inner = svc.inner.clone();
    let res = tokio::task::spawn_blocking(move || {
        let conn = inner.lock().unwrap();
        conn.execute(
            "UPDATE billing_events SET amount_usd = 999 WHERE room_id = ?1",
            rusqlite::params![ROOM],
        )
    })
    .await
    .unwrap();
    assert!(res.is_err(), "UPDATE should be blocked by trigger");
    let msg = format!("{}", res.unwrap_err());
    assert!(msg.contains("append-only"), "trigger msg: {msg}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn append_only_delete_blocked_by_trigger() {
    let svc = db();
    svc.topup(ROOM, 1.0, Some(USER), json!({})).await.unwrap();
    let inner = svc.inner.clone();
    let res = tokio::task::spawn_blocking(move || {
        let conn = inner.lock().unwrap();
        conn.execute("DELETE FROM billing_events", rusqlite::params![])
    })
    .await
    .unwrap();
    assert!(res.is_err(), "DELETE should be blocked by trigger");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn duplicate_reserve_rejected() {
    let svc = db();
    let corr = Uuid::now_v7();
    svc.reserve(ROOM, corr, RESERVE, None, None).await.unwrap();
    let err = svc
        .reserve(ROOM, corr, RESERVE, None, None)
        .await
        .unwrap_err();
    assert!(matches!(err, BillingError::DuplicateReserve(c) if c == corr));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn charge_without_reserve_rejected() {
    let svc = db();
    let corr = Uuid::now_v7();
    let err = svc.charge(corr, 0.01, json!({})).await.unwrap_err();
    assert!(matches!(err, BillingError::ReserveNotFound(c) if c == corr));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn duplicate_charge_rejected() {
    let svc = db();
    let corr = Uuid::now_v7();
    svc.reserve(ROOM, corr, RESERVE, None, None).await.unwrap();
    svc.charge(corr, 0.01, json!({})).await.unwrap();
    let err = svc.charge(corr, 0.02, json!({})).await.unwrap_err();
    assert!(matches!(err, BillingError::DuplicateCharge(c) if c == corr));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn duplicate_release_rejected() {
    let svc = db();
    let corr = Uuid::now_v7();
    svc.reserve(ROOM, corr, RESERVE, None, None).await.unwrap();
    svc.release(corr).await.unwrap();
    let err = svc.release(corr).await.unwrap_err();
    assert!(matches!(err, BillingError::DuplicateRelease(c) if c == corr));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn manual_release_resolves_zombie() {
    let svc = db();
    let corr = Uuid::now_v7();
    let reserve_event = svc
        .reserve(ROOM, corr, RESERVE, Some(USER), Some(MATRIX_EVT))
        .await
        .unwrap();

    let zombies_before = svc.list_zombies(0).await.unwrap();
    assert_eq!(zombies_before.len(), 1);

    let release = svc
        .manual_release(reserve_event.event_id, "@admin:example.com", "outage")
        .await
        .unwrap();
    assert_eq!(release.event_type, BillingEventType::Release);
    assert_eq!(release.correlation_id, Some(corr));
    assert_eq!(release.meta["manual"], json!(true));
    assert_eq!(release.meta["admin_mxid"], json!("@admin:example.com"));

    let zombies_after = svc.list_zombies(0).await.unwrap();
    assert!(zombies_after.is_empty(), "zombie should be resolved");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn period_spent_only_counts_charges() {
    let svc = db();
    svc.topup(ROOM, 100.0, Some(USER), json!({})).await.unwrap();
    let corr1 = Uuid::now_v7();
    svc.reserve(ROOM, corr1, RESERVE, None, None).await.unwrap();
    svc.charge(corr1, 0.50, json!({})).await.unwrap();
    svc.release(corr1).await.unwrap();

    let corr2 = Uuid::now_v7();
    svc.reserve(ROOM, corr2, RESERVE, None, None).await.unwrap();
    svc.charge(corr2, 0.25, json!({})).await.unwrap();
    svc.release(corr2).await.unwrap();

    let day = svc.compute_period_spent(Period::Day).await.unwrap();
    let month = svc.compute_period_spent(Period::Month).await.unwrap();
    assert!((day - 0.75).abs() < 1e-9, "day: {day}");
    assert!((month - 0.75).abs() < 1e-9, "month: {month}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn manual_refund_credits_balance() {
    let svc = db();
    svc.topup(ROOM, 5.0, Some(USER), json!({})).await.unwrap();
    svc.manual_refund(ROOM, 1.0, "@admin:example.com", "test")
        .await
        .unwrap();
    let bal = svc.compute_balance(ROOM).await.unwrap();
    assert!((bal - 6.0).abs() < 1e-9, "expected 6.0, got {bal}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn recent_events_returns_in_descending_time() {
    let svc = db();
    svc.topup(ROOM, 1.0, Some(USER), json!({"n": 1}))
        .await
        .unwrap();
    svc.topup(ROOM, 2.0, Some(USER), json!({"n": 2}))
        .await
        .unwrap();
    svc.topup(ROOM, 3.0, Some(USER), json!({"n": 3}))
        .await
        .unwrap();
    let evts = svc.recent_events(ROOM, 2).await.unwrap();
    assert_eq!(evts.len(), 2);
    // Newest first: amounts 3.0 then 2.0.
    assert_eq!(evts[0].amount_usd, 3.0);
    assert_eq!(evts[1].amount_usd, 2.0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn event_id_lex_sorts_by_time() {
    // UUID v7 is time-ordered. Two reserves a few ms apart should sort
    // correctly by string comparison of their event_ids.
    let svc = db();
    let corr1 = Uuid::now_v7();
    let e1 = svc.reserve(ROOM, corr1, RESERVE, None, None).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    let corr2 = Uuid::now_v7();
    let e2 = svc.reserve(ROOM, corr2, RESERVE, None, None).await.unwrap();
    assert!(
        e1.event_id.to_string() < e2.event_id.to_string(),
        "uuid v7 should be lex-sortable: {} < {}",
        e1.event_id,
        e2.event_id,
    );
}
