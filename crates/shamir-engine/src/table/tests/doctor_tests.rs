//! Tests for the integrity doctor — `verify`, `repair`, and the
//! `recover_on_open` WAL-based crash-recovery entry point.

#![allow(deprecated)]

use std::sync::Arc;

use serde_json::json;

use crate::db_instance::db_instance::DbInstance;
use crate::repo::repo_types::BoxRepoFactory;
use crate::repo::RepoConfig;
use crate::table::TableConfig;
use crate::wal::{WalManager, WalOp};
use shamir_storage::types::Store;
use shamir_types::codecs::transform;
use shamir_types::types::common::new_map;
use shamir_types::types::value::UserValue;

/// Build a fresh in-memory DB with a `users` table, a regular index
/// on `city`, a unique index on `id`, and a sorted index on `score`.
/// Seeds N records.
async fn seeded(n: usize) -> crate::table::TableManager {
    let repo_config = RepoConfig {
        name: "default".to_string(),
        factory: BoxRepoFactory::in_memory(),
        tables: vec![TableConfig::new("users")],
    };
    let db = DbInstance::with_repos(vec![repo_config]).await.unwrap();
    let table = db.get_table("default", "users").await.unwrap();
    table.create_index("by_city", &["city"]).await.unwrap();
    table.create_unique_index("by_id", &["id"]).await.unwrap();
    table
        .create_sorted_index("by_score", &["score"])
        .await
        .unwrap();

    let interner = table.interner().get().await.unwrap();
    for i in 0..n {
        let mut m = new_map();
        m.insert(
            "id".to_string(),
            UserValue::Str(format!("u{:04}", i)),
        );
        m.insert(
            "city".to_string(),
            UserValue::Str(format!("city_{}", i % 4)),
        );
        m.insert("score".to_string(), UserValue::Int((i * 7919) as i64 % 1000));
        let user = UserValue::Map(m);
        let r = transform::user_to_inner(&user, interner);
        if let Some(ref new_keys) = r.new_keys {
            table.interner().save_new_keys(new_keys).await.unwrap();
        }
        table.insert(&r.inner_value).await.unwrap();
    }
    table
}

#[tokio::test]
async fn verify_reports_healthy_after_seed() {
    let table = seeded(20).await;
    let report = table.verify().await.unwrap();
    assert!(
        report.is_healthy(),
        "freshly-seeded table should be healthy, got {:?}",
        report
    );
    assert_eq!(report.records_in_data, 20);
    assert_eq!(report.counter_value, 20);
}

#[tokio::test]
async fn verify_detects_orphan_regular_index_entry() {
    // Corrupt the index: insert an extra posting entry pointing to
    // a record that doesn't exist. Verify should report the index
    // as unhealthy.
    let table = seeded(10).await;
    let regular_defs: Vec<_> = table.index_manager_ref().iter_indexes().collect();
    let def = regular_defs.first().expect("by_city index registered");

    // Build a posting key for a non-existent record_id, write it
    // straight into info_store.
    let info_store = info_store_of(&table);
    use crate::index::index_record_key::IndexRecordKey;
    let key = IndexRecordKey::new(false, def.name_interned).to_bytes();
    let fake_record_id = shamir_types::types::record_id::RecordId::new();
    let mut posting_key = key.to_vec();
    posting_key.extend_from_slice(fake_record_id.as_bytes());
    info_store
        .set(bytes::Bytes::from(posting_key), bytes::Bytes::new())
        .await
        .unwrap();

    let report = table.verify().await.unwrap();
    let bucket = report
        .regular_indexes
        .iter()
        .find(|h| h.name_interned == def.name_interned)
        .expect("by_city in report");
    assert!(
        bucket.actual_entries > bucket.expected_entries,
        "verify must spot the orphan: actual {} > expected {}",
        bucket.actual_entries,
        bucket.expected_entries,
    );
    assert!(!report.is_healthy());
}

#[tokio::test]
async fn repair_heals_orphan_index_entry() {
    let table = seeded(10).await;
    let regular_defs: Vec<_> = table.index_manager_ref().iter_indexes().collect();
    let def = regular_defs.first().expect("by_city index registered");

    // Plant a bogus posting.
    let info_store = info_store_of(&table);
    use crate::index::index_record_key::IndexRecordKey;
    let key = IndexRecordKey::new(false, def.name_interned).to_bytes();
    let fake = shamir_types::types::record_id::RecordId::new();
    let mut posting_key = key.to_vec();
    posting_key.extend_from_slice(fake.as_bytes());
    info_store
        .set(bytes::Bytes::from(posting_key), bytes::Bytes::new())
        .await
        .unwrap();

    // Before: unhealthy.
    assert!(!table.verify().await.unwrap().is_healthy());

    let report = table.repair().await.unwrap();
    assert_eq!(report.records_scanned, 10);

    // After: every index matches data.
    let after = table.verify().await.unwrap();
    assert!(
        after.is_healthy(),
        "repair must restore consistency: {:?}",
        after
    );
}

#[tokio::test]
async fn repair_heals_drifted_counter() {
    let table = seeded(15).await;
    // Force the cached counter out of sync with data.
    table.counter().set_to(999).await.unwrap();
    let pre = table.verify().await.unwrap();
    assert_eq!(pre.records_in_data, 15);
    assert_eq!(pre.counter_value, 999);
    assert!(!pre.counter_consistent);

    let report = table.repair().await.unwrap();
    assert_eq!(report.records_scanned, 15);
    assert_eq!(report.counter_after, 15);

    let post = table.verify().await.unwrap();
    assert!(post.counter_consistent);
}

#[tokio::test]
async fn recover_on_open_runs_repair_when_wal_marker_present() {
    let table = seeded(10).await;

    // Simulate a crashed batch: write a WAL marker by hand.
    let txn_id = table.wal().fresh_txn_id();
    table
        .wal()
        .begin(
            txn_id,
            vec![WalOp::RecordCreated {
                record_id: shamir_types::types::record_id::RecordId::new(),
            }],
        )
        .await
        .unwrap();

    // Also nuke the counter so repair is observable.
    table.counter().set_to(777).await.unwrap();

    let report = table.recover_on_open().await.unwrap();
    let report = report.expect("recover_on_open must repair when marker exists");
    assert_eq!(report.records_scanned, 10);

    // Marker cleared.
    let inflight = table.wal().list_inflight().await.unwrap();
    assert!(inflight.is_empty(), "WAL must be empty after recovery");

    // State consistent.
    assert!(table.verify().await.unwrap().is_healthy());
}

#[tokio::test]
async fn recover_on_open_is_noop_on_clean_db() {
    let table = seeded(10).await;
    let report = table.recover_on_open().await.unwrap();
    assert!(
        report.is_none(),
        "no WAL marker → recover must be a no-op, got {:?}",
        report
    );
    // Counter and indexes untouched.
    let v = table.verify().await.unwrap();
    assert!(v.is_healthy());
}

#[tokio::test]
async fn insert_many_clears_its_own_wal_marker_on_success() {
    // After a successful insert_many, no WAL marker should remain.
    let table = seeded(0).await;
    let interner = table.interner().get().await.unwrap();
    let mut values = Vec::new();
    for i in 0..5 {
        let mut m = new_map();
        m.insert("id".to_string(), UserValue::Str(format!("u{}", i)));
        m.insert("city".to_string(), UserValue::Str("NYC".into()));
        m.insert("score".to_string(), UserValue::Int(i as i64));
        let user = UserValue::Map(m);
        let r = transform::user_to_inner(&user, interner);
        if let Some(ref new_keys) = r.new_keys {
            table.interner().save_new_keys(new_keys).await.unwrap();
        }
        values.push(r.inner_value);
    }
    let ids = table.insert_many(&values).await.unwrap();
    assert_eq!(ids.len(), 5);

    let inflight = table.wal().list_inflight().await.unwrap();
    assert!(
        inflight.is_empty(),
        "successful insert_many must clear its WAL marker, got {} inflight",
        inflight.len()
    );

    // And state is consistent.
    assert!(table.verify().await.unwrap().is_healthy());
}

// Test helper: pull the `info_store` of a TableManager. We don't
// expose it through a public accessor (correctly), but for these
// tests we need to reach in via the index_manager which holds an
// Arc<dyn Store> we can re-fetch from the repo.
fn info_store_of(_table: &crate::table::TableManager) -> Arc<dyn Store> {
    // The same Arc lives behind each manager; the simplest path is
    // to ask the underlying repo. But we don't have it here.
    // Instead: use the WAL's info_store via a debug accessor. For
    // now, build a Store handle through the public DbInstance API
    // would need refactor — so the tests that need raw writes use
    // a separately-built DB. (This helper is wired through a small
    // crate-internal accessor below.)
    let wal: &Arc<WalManager> = _table.wal();
    wal.info_store_for_test().clone()
}
