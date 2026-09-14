//! Tests for the chat-command handlers against an in-memory ledger.

use serde_json::json;
use uuid::Uuid;

use super::handlers::*;
use crate::billing::{BillingService, Period};

const ROOM: &str = "!r:example.com";
const ADMIN: &str = "@admin:example.com";
const PREFIX: &str = "!bai";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn balance_zero() {
    let svc = BillingService::open_in_memory().unwrap();
    let out = balance(&svc, ROOM, 5).await.unwrap();
    assert!(out.contains("$0.00"), "{out}");
    assert!(!out.contains("Recent transactions"), "{out}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn balance_with_recent() {
    let svc = BillingService::open_in_memory().unwrap();
    svc.topup(ROOM, 5.0, Some("@u:example.com"), json!({}))
        .await
        .unwrap();
    let out = balance(&svc, ROOM, 3).await.unwrap();
    assert!(out.contains("$5.00"), "{out}");
    assert!(out.contains("Recent transactions"), "{out}");
    assert!(out.contains("| topup |"), "{out}");
    assert!(out.contains("+$5.0000"), "{out}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stats_day_renders_percent() {
    let svc = BillingService::open_in_memory().unwrap();
    let corr = Uuid::now_v7();
    svc.topup(ROOM, 10.0, Some("@u:example.com"), json!({}))
        .await
        .unwrap();
    svc.reserve(ROOM, corr, 0.03, None, None).await.unwrap();
    svc.charge(corr, 0.25, json!({})).await.unwrap();
    svc.release(corr).await.unwrap();
    let out = stats(&svc, Period::Day, 1.0).await.unwrap();
    assert!(out.contains("$0.2500 / $1.00"), "{out}");
    assert!(out.contains("25.0%"), "{out}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_zombies_says_none_when_clean() {
    let svc = BillingService::open_in_memory().unwrap();
    let out = list_zombies(&svc, 0, PREFIX).await.unwrap();
    assert!(out.starts_with("No zombie"), "{out}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_zombies_shows_orphans() {
    let svc = BillingService::open_in_memory().unwrap();
    let corr = Uuid::now_v7();
    svc.reserve(ROOM, corr, 0.03, None, None).await.unwrap();
    let out = list_zombies(&svc, 0, PREFIX).await.unwrap();
    assert!(out.contains("**1 zombie reserve(s)**"), "{out}");
    assert!(out.contains(&corr.to_string()), "{out}");
    assert!(out.contains("`!bai billing manual-release"), "{out}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn manual_release_credits_back_via_handler() {
    let svc = BillingService::open_in_memory().unwrap();
    let corr = Uuid::now_v7();
    let reserve = svc.reserve(ROOM, corr, 0.03, None, None).await.unwrap();
    let out = manual_release(&svc, reserve.event_id, ADMIN, "provider outage")
        .await
        .unwrap();
    assert!(out.contains("Released zombie reserve"), "{out}");
    assert!(out.contains("$0.0300"), "{out}");
    let balance = svc.compute_balance(ROOM).await.unwrap();
    assert!(balance.abs() < 1e-9, "balance restored to 0: {balance}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn manual_refund_inserts_credit() {
    let svc = BillingService::open_in_memory().unwrap();
    svc.topup(ROOM, 5.0, Some("@u:example.com"), json!({}))
        .await
        .unwrap();
    let out = manual_refund(&svc, ROOM, 1.5, ADMIN, "duplicate top-up")
        .await
        .unwrap();
    assert!(out.contains("Refunded $1.5000"), "{out}");
    let balance = svc.compute_balance(ROOM).await.unwrap();
    assert!((balance - 6.5).abs() < 1e-9, "balance: {balance}");
}

#[test]
fn help_includes_admin_section_only_for_admin() {
    let user = help(PREFIX, false);
    let admin = help(PREFIX, true);
    assert!(user.contains("`!bai balance`"), "{user}");
    assert!(!user.contains("Administration commands"), "{user}");
    assert!(admin.contains("Administration commands"), "{admin}");
    assert!(admin.contains("stats day | month"), "{admin}");
    assert!(admin.contains("billing zombies"), "{admin}");
    assert!(admin.contains("manual-release"), "{admin}");
    assert!(admin.contains("manual-refund"), "{admin}");
}
