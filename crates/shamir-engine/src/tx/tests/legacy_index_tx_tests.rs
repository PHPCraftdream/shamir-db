//! HIGH-6: legacy `IndexManager` + `SortedIndexManager` tx hooks.
//!
//! `TableManager::insert_tx` / `update_tx` / `delete_tx` now stage legacy
//! secondary-index and sorted-index posting writes into
//! `tx.index_write_set`, alongside the index2 ops. The planners emit
//! `IndexWriteOp`s carrying the *exact* physical key layout the non-tx
//! readers expect (`lookup_by_index`, `check_unique_constraint`,
//! `lookup_range`), so:
//!
//! * a committed tx, once its `index_write_set` lands in `info_store`,
//!   produces postings indistinguishable from the non-tx
//!   `on_record_created` path — the legacy reader finds the record;
//! * a dropped tx never applies those ops, so the legacy reader finds
//!   nothing (rollback safety).
//!
//! Scope note — the happy-path commit applies `index_write_set` to
//! `info_store` in `commit.rs::commit_tx_inner` Phase 5c-d, which is
//! owned by a separate workstream. To assert the *staging-side* contract
//! (the key scheme HIGH-6 is responsible for) independently of that
//! pipeline, these tests drain `tx.index_write_set` and apply the ops to
//! the table's `info_store` directly — the same store the legacy
//! managers read through. This proves the posting keys match the reader,
//! which is the load-bearing correctness property: a posting written
//! with the wrong key scheme would be silent corruption.

use std::sync::Arc;

use bytes::Bytes;
use shamir_storage::storage_in_memory::InMemoryRepo;
use shamir_storage::types::Store;
use shamir_tx::{IndexWriteOp, IsolationLevel, TxContext};
use shamir_types::core::interner::{InternerKey, TouchInd};
use shamir_types::core::sort_codec;
use shamir_types::types::common::new_map_wc;
use shamir_types::types::value::InnerValue;

use crate::repo::repo_instance::RepoInstance;
use crate::repo::repo_types::BoxRepo;
use crate::table::table_manager::TableManager;
use crate::table::TableConfig;

fn make_repo() -> RepoInstance {
    let repo = Arc::new(InMemoryRepo::new());
    RepoInstance::new("test".into(), BoxRepo::InMemory(repo), Vec::new())
}

async fn key_id(tbl: &TableManager, name: &str) -> u64 {
    let interner = tbl.interner().get().await.unwrap();
    match interner.touch_ind(name).unwrap() {
        TouchInd::Exists(k) | TouchInd::New(k) => k.id(),
    }
}

fn record_with_str(key: u64, val: &str) -> InnerValue {
    let mut m = new_map_wc(1);
    m.insert(InternerKey::new(key), InnerValue::Str(val.into()));
    InnerValue::Map(m)
}

fn record_with_int(key: u64, val: i64) -> InnerValue {
    let mut m = new_map_wc(1);
    m.insert(InternerKey::new(key), InnerValue::Int(val));
    InnerValue::Map(m)
}

/// Apply the staged index ops to `info_store`, simulating what the
/// commit pipeline's Phase 5c-d does on a successful commit. Used so the
/// staging-side key-scheme contract is testable independently of the
/// (separately-owned) commit-apply wiring.
async fn apply_staged_index_ops(info_store: &Arc<dyn Store>, tx: &TxContext) {
    for (_token, op) in &tx.index_write_set {
        match op {
            IndexWriteOp::SetPosting { key, value } => {
                info_store.set(key.clone(), value.clone()).await.unwrap();
            }
            IndexWriteOp::RemovePosting { key } => {
                let _ = info_store.remove(key.clone()).await;
            }
            IndexWriteOp::BumpFtsStats { .. } => {}
        }
    }
}

/// A committed `insert_tx` (with its staged index ops applied) populates
/// the legacy secondary index so `lookup_by_index` finds the record.
#[tokio::test]
async fn committed_tx_populates_secondary_index() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));
    let tbl = repo.get_table("t").await.unwrap();
    repo.create_index("t", "by_name", &["name"]).await.unwrap();
    let name_id = key_id(&tbl, "name").await;

    let (mut tx, _g) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let rid = tbl
        .insert_tx(&record_with_str(name_id, "alice"), Some(&mut tx))
        .await
        .unwrap();

    // The staged ops must carry the legacy posting key scheme.
    assert!(
        !tx.index_write_set.is_empty(),
        "insert_tx must stage legacy index postings"
    );
    apply_staged_index_ops(tbl.info_store(), &tx).await;

    let hits = tbl
        .lookup_by_index("by_name", &[InnerValue::Str("alice".into())])
        .await
        .unwrap();
    assert!(
        hits.contains(&rid),
        "committed insert_tx posting must be findable via legacy index; got {:?}, rid {:?}",
        hits,
        rid
    );
}

/// A committed `insert_tx` populates the sorted/range index so
/// `lookup_range` finds the record.
#[tokio::test]
async fn committed_tx_populates_sorted_index() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));
    let tbl = repo.get_table("t").await.unwrap();
    tbl.create_sorted_index("by_age", &["age"]).await.unwrap();
    let age_id = key_id(&tbl, "age").await;
    let sidx_name = key_id(&tbl, "by_age").await;

    let (mut tx, _g) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let rid = tbl
        .insert_tx(&record_with_int(age_id, 42), Some(&mut tx))
        .await
        .unwrap();

    assert!(
        !tx.index_write_set.is_empty(),
        "insert_tx must stage sorted index postings"
    );
    apply_staged_index_ops(tbl.info_store(), &tx).await;

    // Range covering 42.
    let mut lo = Vec::new();
    sort_codec::encode_i64(&mut lo, 40);
    let mut hi = Vec::new();
    sort_codec::encode_i64(&mut hi, 50);
    let hits = tbl
        .sorted_indexes()
        .lookup_range(sidx_name, Some(&lo), Some(&hi))
        .await
        .unwrap();
    assert!(
        hits.contains(&rid),
        "committed insert_tx posting must be findable via sorted index; got {:?}, rid {:?}",
        hits,
        rid
    );
}

/// A dropped (rolled-back) `insert_tx` leaves no legacy secondary-index
/// postings — the staged ops are never applied to `info_store`.
#[tokio::test]
async fn dropped_tx_no_secondary_postings() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));
    let tbl = repo.get_table("t").await.unwrap();
    repo.create_index("t", "by_name", &["name"]).await.unwrap();
    let name_id = key_id(&tbl, "name").await;

    {
        let (mut tx, guard) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
        let _ = tbl
            .insert_tx(&record_with_str(name_id, "bob"), Some(&mut tx))
            .await
            .unwrap();
        // Drop WITHOUT applying index_write_set — models rollback.
        drop(tx);
        drop(guard);
    }

    let hits = tbl
        .lookup_by_index("by_name", &[InnerValue::Str("bob".into())])
        .await
        .unwrap();
    assert!(
        hits.is_empty(),
        "dropped insert_tx must leave no legacy index posting; got {:?}",
        hits
    );
}

/// `insert_tx` stages unique-index postings using the exact physical key
/// layout `check_unique_constraint` reads (25-byte index key → record id
/// value). Once applied, `lookup_by_unique_index` resolves the record;
/// and stage-time validation rejects a committed duplicate.
#[tokio::test]
async fn committed_tx_populates_unique_index_and_rejects_duplicate() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));
    let tbl = repo.get_table("t").await.unwrap();
    tbl.create_unique_index("by_email", &["email"])
        .await
        .unwrap();
    let email_id = key_id(&tbl, "by_email").await;
    let email_field = key_id(&tbl, "email").await;

    let (mut tx, _g) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let rid = tbl
        .insert_tx(&record_with_str(email_field, "a@x"), Some(&mut tx))
        .await
        .unwrap();
    apply_staged_index_ops(tbl.info_store(), &tx).await;

    let found = tbl
        .index_manager()
        .lookup_by_unique_index(email_id, &[InnerValue::Str("a@x".into())])
        .await
        .unwrap();
    assert_eq!(
        found,
        Some(rid),
        "unique posting must resolve to the staged record id"
    );

    // Stage-time validation now sees the committed posting and rejects a
    // second insert of the same unique value.
    let (mut tx2, _g2) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let dup = tbl
        .insert_tx(&record_with_str(email_field, "a@x"), Some(&mut tx2))
        .await;
    assert!(
        matches!(dup, Err(shamir_storage::error::DbError::DuplicateKey(_))),
        "insert_tx must reject a committed duplicate at stage time; got {:?}",
        dup
    );
}

/// Posting-key scheme confirmation: the bytes `insert_tx` stages are
/// *byte-for-byte identical* to what the non-tx `IndexManager` /
/// `SortedIndexManager` planners produce. This is the load-bearing
/// guard — a divergent key scheme would be silent corruption.
#[tokio::test]
async fn staged_keys_match_non_tx_planner_output() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));
    let tbl = repo.get_table("t").await.unwrap();
    repo.create_index("t", "by_name", &["name"]).await.unwrap();
    tbl.create_sorted_index("by_name_sorted", &["name"])
        .await
        .unwrap();
    let name_id = key_id(&tbl, "name").await;

    let rec = record_with_str(name_id, "carol");

    let (mut tx, _g) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let rid = tbl.insert_tx(&rec, Some(&mut tx)).await.unwrap();

    let mut staged: Vec<(Bytes, Bytes)> = tx
        .index_write_set
        .iter()
        .filter_map(|(_t, op)| match op {
            IndexWriteOp::SetPosting { key, value } => Some((key.clone(), value.clone())),
            _ => None,
        })
        .collect();

    // Expected: legacy regular planner + sorted planner for the same rid.
    let mut expected: Vec<(Bytes, Bytes)> = Vec::new();
    for op in tbl
        .index_manager()
        .plan_record_created(&rid, &rec)
        .await
        .unwrap()
    {
        if let IndexWriteOp::SetPosting { key, value } = op {
            expected.push((key, value));
        }
    }
    for op in tbl
        .sorted_indexes()
        .plan_record_created(&rid, &rec)
        .unwrap()
    {
        if let IndexWriteOp::SetPosting { key, value } = op {
            expected.push((key, value));
        }
    }

    staged.sort();
    expected.sort();
    // The staged set must CONTAIN every legacy/sorted posting key the
    // non-tx planner would emit (index2 ops may add more, hence subset).
    for e in &expected {
        assert!(
            staged.contains(e),
            "staged postings must include the non-tx key {:?}; staged = {:?}",
            e,
            staged
        );
    }
    assert!(
        !expected.is_empty(),
        "sanity: expected at least the regular + sorted postings"
    );
}
