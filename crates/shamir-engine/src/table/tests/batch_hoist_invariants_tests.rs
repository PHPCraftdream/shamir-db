//! Regression tests for cross-crate rush-review TASK_GROUPS.md group 19 —
//! "Hoist per-batch invariants out of insert/update row loops; cache
//! table_token". Three independent hoists, each proven behaviorally
//! IDENTICAL to the pre-hoist per-row recomputation:
//!
//! 1. `insert_tx_many` / `insert_tx_many_bytes` hoist the unique-index-defs
//!    snapshot (`iter_unique_indexes().collect()`) above the batch-local
//!    duplicate-detection loop. A hoisting mistake (e.g. truncating the
//!    snapshot, or only capturing the FIRST unique index) would silently
//!    stop detecting an in-batch duplicate on the SECOND unique index —
//!    the tests below use two unique indexes and put the duplicate on the
//!    second one specifically to catch that failure mode.
//! 2. The non-tx batched insert path (`insert_many` /
//!    `insert_many_returning_version`) hoists the index2
//!    `all_backends()` snapshot above the per-record loop. A hoisting
//!    mistake (backends captured empty, or only applied to the first
//!    record) would leave later records unindexed — the test below
//!    inserts 3 records and checks the index2 posting for EACH one.
//! 3. `execute_update_tx` hoists `table_token()` into one local reused for
//!    every matched row (instead of re-hashing per row), and probes the
//!    tx's own staging with `id.as_bytes()` (borrow) instead of
//!    `id.to_bytes()` (owned alloc). The test below updates a batch that
//!    mixes committed rows with one row staged earlier in the SAME tx, so
//!    the read-your-own-write probe at the hoisted `table_token` must
//!    still correctly identify the ONE staged row among several matched
//!    rows.

use std::sync::Arc;

use shamir_query_builder::write;
use shamir_query_types::admin::types::CreateIndexOp;
use shamir_storage::storage_in_memory::InMemoryRepo;
use shamir_tx::IsolationLevel;
use shamir_types::access::Actor;
use shamir_types::core::interner::{InternerKey, TouchInd};
use shamir_types::mpack;
use shamir_types::types::common::new_map;
use shamir_types::types::value::InnerValue;

use crate::index::index_definition::IndexDefinition;
use crate::index::index_info_item::IndexInfoItem;
use crate::index2::functional_backend::FunctionalBackend;
use crate::query::filter::eval_context::FilterContext;
use crate::repo::repo_instance::RepoInstance;
use crate::repo::repo_types::BoxRepo;
use crate::table::TableConfig;
use crate::table::TableManager;

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

fn record_with_two_str(k1: u64, v1: &str, k2: u64, v2: &str) -> InnerValue {
    let mut m = new_map();
    m.insert(InternerKey::new(k1), InnerValue::Str(v1.into()));
    m.insert(InternerKey::new(k2), InnerValue::Str(v2.into()));
    InnerValue::Map(m)
}

fn record_with_str(key: u64, val: &str) -> InnerValue {
    let mut m = new_map();
    m.insert(InternerKey::new(key), InnerValue::Str(val.into()));
    InnerValue::Map(m)
}

fn functional_lower_op(name: &str, table: &str, field: &str) -> CreateIndexOp {
    CreateIndexOp {
        create_index: name.into(),
        table: table.into(),
        fields: vec![vec![field.into()]],
        unique: false,
        sorted: false,
        repo: "main".into(),
        index_type: Some("functional".into()),
        fts_tokenizer: None,
        fts_language: None,
        functional_op: Some("lower".into()),
        functional_args: None,
        vector_dim: None,
        vector_metric: None,
        vector_quantization: None,
        include: Vec::new(),
        if_not_exists: false,
    }
}

async fn functional_lookup(tbl: &TableManager, index_name_id: u64, lowered: &str) -> Vec<[u8; 16]> {
    use crate::index2::backend::{IndexQuery, IndexResult};
    let backend = match tbl.index2_registry().get_by_name(index_name_id).await {
        Some(b) => b,
        None => return Vec::new(),
    };
    let key = FunctionalBackend::hash_value(&InnerValue::Str(lowered.into()));
    let mut keys: smallvec::SmallVec<[Vec<u8>; 4]> = smallvec::SmallVec::new();
    keys.push(key.to_vec());
    match backend.lookup(IndexQuery::Point { keys }).await.unwrap() {
        IndexResult::Set(s) => s.iter().map(|rid| *rid.as_bytes()).collect(),
        IndexResult::Ranked(v) => v.iter().map(|(rid, _)| *rid.as_bytes()).collect(),
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Defect 1 — insert_tx_many / insert_tx_many_bytes: hoisted unique_defs
// must still catch an in-batch duplicate on EVERY unique index, not just
// the first one in the snapshot.
// ─────────────────────────────────────────────────────────────────────────

/// Two unique indexes (email, city). A 3-row batch has distinct emails
/// throughout but rows 0 and 2 share the SAME city — the duplicate is on
/// the SECOND unique index in the hoisted snapshot. If the hoist only
/// captured (or only checked against) the first def, this duplicate would
/// go undetected and the batch would wrongly succeed.
#[tokio::test]
async fn insert_tx_many_detects_duplicate_on_second_hoisted_unique_def() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));
    let tbl = repo.get_table("t").await.unwrap();
    let email_key = key_id(&tbl, "email").await;
    let city_key = key_id(&tbl, "city").await;

    let idx_email = key_id(&tbl, "idx_email").await;
    let idx_city = key_id(&tbl, "idx_city").await;
    tbl.index_manager_ref()
        .create_unique_index(IndexDefinition::new(
            idx_email,
            vec![IndexInfoItem::new(vec![email_key])],
        ))
        .await
        .unwrap();
    tbl.index_manager_ref()
        .create_unique_index(IndexDefinition::new(
            idx_city,
            vec![IndexInfoItem::new(vec![city_key])],
        ))
        .await
        .unwrap();

    let (mut tx, _guard) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let values = vec![
        record_with_two_str(email_key, "a@x.com", city_key, "NY"),
        record_with_two_str(email_key, "b@x.com", city_key, "LA"),
        // row 2: distinct email, but city "NY" duplicates row 0.
        record_with_two_str(email_key, "c@x.com", city_key, "NY"),
    ];
    let err = tbl
        .insert_tx_many(&values, &mut tx)
        .await
        .expect_err("duplicate city at row 2 must be rejected");
    let msg = err.to_string();
    assert!(
        msg.contains("row 2"),
        "error must name the offending row (2): {msg}"
    );

    // All-or-nothing: nothing from this batch may be staged after the
    // rejection.
    let token = tbl.table_token();
    assert!(
        !tx.write_set.contains_key(&token),
        "a rejected batch must not stage any row"
    );
}

/// Bytes-path (`insert_tx_many_bytes`) twin of the test above — same
/// hoisted-snapshot risk, driven through the `RecordView` lens instead of
/// `InnerValue` trees.
#[tokio::test]
async fn insert_tx_many_bytes_detects_duplicate_on_second_hoisted_unique_def() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));
    let tbl = repo.get_table("t").await.unwrap();
    let email_key = key_id(&tbl, "email").await;
    let city_key = key_id(&tbl, "city").await;

    let idx_email = key_id(&tbl, "idx_email").await;
    let idx_city = key_id(&tbl, "idx_city").await;
    tbl.index_manager_ref()
        .create_unique_index(IndexDefinition::new(
            idx_email,
            vec![IndexInfoItem::new(vec![email_key])],
        ))
        .await
        .unwrap();
    tbl.index_manager_ref()
        .create_unique_index(IndexDefinition::new(
            idx_city,
            vec![IndexInfoItem::new(vec![city_key])],
        ))
        .await
        .unwrap();

    let (mut tx, _guard) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let staged: Vec<bytes::Bytes> = vec![
        record_with_two_str(email_key, "a@x.com", city_key, "NY")
            .to_bytes()
            .unwrap(),
        record_with_two_str(email_key, "b@x.com", city_key, "LA")
            .to_bytes()
            .unwrap(),
        record_with_two_str(email_key, "c@x.com", city_key, "NY")
            .to_bytes()
            .unwrap(),
    ];
    let err = tbl
        .insert_tx_many_bytes(&staged, &mut tx)
        .await
        .expect_err("duplicate city at row 2 must be rejected");
    let msg = err.to_string();
    assert!(
        msg.contains("row 2"),
        "error must name the offending row (2): {msg}"
    );

    let token = tbl.table_token();
    assert!(
        !tx.write_set.contains_key(&token),
        "a rejected batch must not stage any row"
    );
}

/// Positive case: a fully-distinct batch (both unique indexes) commits and
/// the durable uniqueness constraint for BOTH indexes is enforced
/// afterwards — end-to-end proof the hoisted defs correctly index every
/// row, not just guard bookkeeping.
#[tokio::test]
async fn insert_tx_many_all_distinct_batch_commits_and_enforces_both_unique_indexes() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));
    let tbl = repo.get_table("t").await.unwrap();
    let email_key = key_id(&tbl, "email").await;
    let city_key = key_id(&tbl, "city").await;

    let idx_email = key_id(&tbl, "idx_email").await;
    let idx_city = key_id(&tbl, "idx_city").await;
    tbl.index_manager_ref()
        .create_unique_index(IndexDefinition::new(
            idx_email,
            vec![IndexInfoItem::new(vec![email_key])],
        ))
        .await
        .unwrap();
    tbl.index_manager_ref()
        .create_unique_index(IndexDefinition::new(
            idx_city,
            vec![IndexInfoItem::new(vec![city_key])],
        ))
        .await
        .unwrap();

    let (mut tx, _guard) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let values = vec![
        record_with_two_str(email_key, "a@x.com", city_key, "NY"),
        record_with_two_str(email_key, "b@x.com", city_key, "LA"),
        record_with_two_str(email_key, "c@x.com", city_key, "SF"),
    ];
    let ids = tbl.insert_tx_many(&values, &mut tx).await.unwrap();
    assert_eq!(ids.len(), 3);
    // One UniqueGuard per (row, unique index) claim: 3 rows × 2 indexes.
    assert_eq!(tx.unique_guards.len(), 6);

    repo.commit_tx(tx).await.expect("commit must succeed");

    // Durable email uniqueness: reusing "b@x.com" (distinct city) must fail.
    let dup_email = tbl
        .insert(&record_with_two_str(email_key, "b@x.com", city_key, "BOS"))
        .await;
    assert!(dup_email.is_err(), "email uniqueness must be durable");

    // Durable city uniqueness: reusing "SF" (distinct email) must fail.
    let dup_city = tbl
        .insert(&record_with_two_str(email_key, "d@x.com", city_key, "SF"))
        .await;
    assert!(dup_city.is_err(), "city uniqueness must be durable");
}

// ─────────────────────────────────────────────────────────────────────────
// Defect 2 — non-tx `insert_many`: hoisted index2 `all_backends()`
// snapshot must apply to EVERY record in the batch, not just the first.
// ─────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn insert_many_hoisted_index2_backends_apply_to_every_record() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));
    let tbl = repo.get_table("t").await.unwrap();
    let name_key = key_id(&tbl, "name").await;

    tbl.create_index_v2(&functional_lower_op("lower_name", "t", "name"))
        .await
        .unwrap();
    let idx_name = key_id(&tbl, "lower_name").await;

    let values = vec![
        record_with_str(name_key, "Alice"),
        record_with_str(name_key, "Bob"),
        record_with_str(name_key, "Carol"),
    ];
    let ids = tbl.insert_many(&values).await.unwrap();
    assert_eq!(ids.len(), 3);

    for (rid, lowered) in ids.iter().zip(["alice", "bob", "carol"]) {
        let owners = functional_lookup(&tbl, idx_name, lowered).await;
        assert!(
            owners.contains(rid.as_bytes()),
            "record {lowered} must be indexed by the hoisted index2 backend \
             snapshot — a hoisting mistake (empty/stale backend list, or \
             only applied to the first record) would leave this record \
             unindexed"
        );
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Defect 3 — execute_update_tx: hoisted `table_token` local + `as_bytes()`
// probe must still correctly resolve read-your-own-write staging for the
// right row among several matched rows.
// ─────────────────────────────────────────────────────────────────────────

/// Update a batch mixing two already-COMMITTED rows with one row staged
/// earlier in the SAME tx (never committed). All three must be matched
/// (via the committed-store scan for the first two, via the FG-3
/// staged-only fold-in for the third) and all three must merge correctly:
/// the tx-staged row's `effective_old_bytes` must come from `tx.write_set`
/// (read-your-own-write), not the (nonexistent) committed snapshot. This
/// specifically exercises the hoisted `table_token` local reused across
/// every loop iteration, and the `id.as_bytes()` probe that replaced
/// `id.to_bytes()`.
#[tokio::test]
async fn execute_update_tx_hoisted_token_resolves_ryow_row_among_committed_rows() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));
    let tbl = repo.get_table("t").await.unwrap();
    let interner = tbl.interner().get().await.unwrap();
    let seq_key = key_id(&tbl, "seq").await;

    // Two committed rows (outside any tx).
    let rid1 = tbl.insert(&record_with_str_int(seq_key, 1)).await.unwrap();
    let rid2 = tbl.insert(&record_with_str_int(seq_key, 2)).await.unwrap();

    let (mut tx, _guard) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();

    // A third row staged WITHIN this tx, never committed — the row whose
    // `effective_old_bytes` lookup must hit the read-your-own-write branch.
    let rid3 = tbl
        .insert_tx(&record_with_str_int(seq_key, 3), Some(&mut tx))
        .await
        .unwrap();

    let op = write::update("t")
        .set(mpack!({ "tag": "updated" }))
        .build()
        .unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    let result = tbl
        .execute_update_tx(&op, &ctx, &mut tx, None, &Actor::System)
        .await
        .unwrap();
    assert_eq!(
        result.affected, 3,
        "all 3 rows (2 committed + 1 tx-staged) must be matched and updated"
    );

    repo.commit_tx(tx).await.expect("commit must succeed");

    for (rid, expected_seq) in [(rid1, 1), (rid2, 2), (rid3, 3)] {
        let v = tbl.get(rid).await.unwrap();
        let InnerValue::Map(m) = v else {
            panic!("expected Map for {rid:?}");
        };
        let seq = m.get(&InternerKey::new(seq_key)).cloned();
        assert_eq!(
            seq,
            Some(InnerValue::Int(expected_seq)),
            "row {rid:?} must keep its own seq={expected_seq} after merge"
        );
        let tag_key = key_id(&tbl, "tag").await;
        let tag = m.get(&InternerKey::new(tag_key)).cloned();
        assert_eq!(
            tag,
            Some(InnerValue::Str("updated".into())),
            "row {rid:?} must carry the UPDATE's tag field"
        );
    }
}

fn record_with_str_int(key: u64, val: i64) -> InnerValue {
    let mut m = new_map();
    m.insert(InternerKey::new(key), InnerValue::Int(val));
    InnerValue::Map(m)
}
