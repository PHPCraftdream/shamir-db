//! P0-2 (#958) — base_index (regular + unique) ops-plan re-derivation + TOCTOU +
//! retired-ops cleanup tests.
//!
//! Mirrors `f50_index_lifecycle_spike_tests.rs`'s red→green proof pattern for
//! the base_index `IndexManager` family (the 2a/2b/2c sub-bugs):
//!
//! - **2a regular/unique**: a tx that STAGES its insert BEFORE a base_index index
//!   is created, then COMMITS after `create_index`/`create_unique_index` has
//!   completed, MUST have its row's posting present and correct.
//! - **2a unique (critical)**: after the first tx commits, a second tx
//!   attempting to insert the SAME unique value MUST be rejected — proving
//!   the UniqueGuard was retroactively recorded and the constraint enforced.
//! - **2a UPDATE/DELETE**: same re-derivation for update and delete paths.
//! - **2c retired ops**: a tx stages ops against an index that is DROP'd
//!   before commit; the stale ops are retracted.

use std::sync::Arc;

use shamir_storage::storage_in_memory::InMemoryRepo;
use shamir_tx::IsolationLevel;
use shamir_types::core::interner::{InternerKey, TouchInd};
use shamir_types::types::common::new_map_wc;
use shamir_types::types::value::InnerValue;

use crate::index::index_definition::IndexDefinition;
use crate::index::index_info_item::IndexInfoItem;
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

fn record_with_str(key: u64, val: &str) -> InnerValue {
    let mut m = new_map_wc(1);
    m.insert(InternerKey::new(key), InnerValue::Str(val.into()));
    InnerValue::Map(m)
}

fn record_with_two_str(k1: u64, v1: &str, k2: u64, v2: &str) -> InnerValue {
    let mut m = new_map_wc(2);
    m.insert(InternerKey::new(k1), InnerValue::Str(v1.into()));
    m.insert(InternerKey::new(k2), InnerValue::Str(v2.into()));
    InnerValue::Map(m)
}

// ─────────────────────────────────────────────────────────────────────────────
// 2a — regular index: INSERT staged before CREATE INDEX
// ─────────────────────────────────────────────────────────────────────────────

/// A tx that stages an INSERT before a regular (non-unique) index is created,
/// then commits after `create_index` completed, MUST have its row's posting
/// present in the new index.
#[tokio::test]
async fn p02_regular_insert_before_create_posting_present() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));
    let tbl = repo.get_table("t").await.unwrap();

    let name_key = key_id(&tbl, "name").await;

    // Step 1: STAGE an insert BEFORE any index exists.
    let (mut tx, _guard) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let rid = tbl
        .insert_tx(&record_with_str(name_key, "alice"), Some(&mut tx))
        .await
        .unwrap();

    // Step 2: CREATE a regular index (DDL completes fully).
    let idx_name = key_id(&tbl, "idx_name").await;
    let def = IndexDefinition::new(idx_name, vec![IndexInfoItem::new(vec![name_key])]);
    tbl.index_manager_ref().create_index(def).await.unwrap();

    // Step 3: COMMIT — the rederive MUST fire (base_index gen advanced).
    repo.commit_tx(tx).await.expect("commit must succeed");

    // Assert: the row's posting IS present in the new index.
    let found = tbl
        .index_manager_ref()
        .lookup_by_index(idx_name, &[InnerValue::Str("alice".into())])
        .await
        .unwrap();
    assert!(
        found.iter().any(|r| r == &rid),
        "P0-2 2a regular: the row staged before CREATE INDEX must be \
         present in the new index after commit. Found {:?}, expected {:?}",
        found.as_ref(),
        rid
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// 2a — regular index: UPDATE staged before CREATE INDEX
// ─────────────────────────────────────────────────────────────────────────────

/// A tx that stages an UPDATE before a regular index is created, then commits
/// after `create_index`, MUST have the updated value's posting present.
#[tokio::test]
async fn p02_regular_update_before_create_posting_present() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));
    let tbl = repo.get_table("t").await.unwrap();

    let name_key = key_id(&tbl, "name").await;

    // Pre-existing row (committed before any tx).
    let rid = tbl.insert(&record_with_str(name_key, "old")).await.unwrap();

    // Step 1: STAGE an update BEFORE any index exists.
    let (mut tx, _guard) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    tbl.update_tx(rid, &record_with_str(name_key, "new"), Some(&mut tx))
        .await
        .unwrap();

    // Step 2: CREATE a regular index.
    let idx_name = key_id(&tbl, "idx_name").await;
    let def = IndexDefinition::new(idx_name, vec![IndexInfoItem::new(vec![name_key])]);
    tbl.index_manager_ref().create_index(def).await.unwrap();

    // Step 3: COMMIT.
    repo.commit_tx(tx).await.expect("commit must succeed");

    // Assert: the NEW value's posting is present (the rederive planned an
    // update, which writes the new posting for the updated record).
    let found_new = tbl
        .index_manager_ref()
        .lookup_by_index(idx_name, &[InnerValue::Str("new".into())])
        .await
        .unwrap();
    assert!(
        found_new.iter().any(|r| r == &rid),
        "P0-2 2a regular UPDATE: the new value's posting must be present"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// 2a — regular index: DELETE staged before CREATE INDEX
// ─────────────────────────────────────────────────────────────────────────────

/// A tx that stages a DELETE before a regular index is created, then commits
/// after `create_index`. The rederive plans a delete op against the new index
/// (harmless idempotent RemovePosting if the index was just created with no
/// backfill data for this row).
#[tokio::test]
async fn p02_regular_delete_before_create_commits_cleanly() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));
    let tbl = repo.get_table("t").await.unwrap();

    let name_key = key_id(&tbl, "name").await;

    // Pre-existing row.
    let rid = tbl
        .insert(&record_with_str(name_key, "victim"))
        .await
        .unwrap();

    // Step 1: STAGE a delete BEFORE any index exists.
    let (mut tx, _guard) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    tbl.delete_tx(rid, Some(&mut tx)).await.unwrap();

    // Step 2: CREATE a regular index (backfills from committed snapshot,
    // which includes the pre-existing row — but the tx hasn't committed yet).
    let idx_name = key_id(&tbl, "idx_name").await;
    let def = IndexDefinition::new(idx_name, vec![IndexInfoItem::new(vec![name_key])]);
    tbl.index_manager_ref().create_index(def).await.unwrap();

    // Step 3: COMMIT — the rederive plans a delete op for the new index
    // (RemovePosting on the key the backfill wrote — harmless idempotent).
    repo.commit_tx(tx).await.expect("commit must succeed");

    // Assert: the row is deleted (data store).
    let gone = tbl.get(rid).await;
    assert!(
        gone.is_err(),
        "P0-2 2a regular DELETE: row must be physically deleted after commit"
    );

    // Assert: the index lookup for the deleted value returns no results.
    let found = tbl
        .index_manager_ref()
        .lookup_by_index(idx_name, &[InnerValue::Str("victim".into())])
        .await
        .unwrap();
    assert!(
        found.is_empty(),
        "P0-2 2a regular DELETE: the deleted value must not be in the index"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// 2a — unique index: INSERT staged before CREATE UNIQUE INDEX (THE critical one)
// ─────────────────────────────────────────────────────────────────────────────

/// A tx that stages an INSERT before a UNIQUE index is created, then commits
/// after `create_unique_index`, MUST have:
/// (a) the unique posting present, AND
/// (b) a SECOND tx inserting the SAME value rejected as a duplicate.
#[tokio::test]
async fn p02_unique_insert_before_create_posting_and_guard_present() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));
    let tbl = repo.get_table("t").await.unwrap();

    let email_key = key_id(&tbl, "email").await;

    // Step 1: STAGE an insert BEFORE any unique index exists.
    let (mut tx, _guard) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let rid = tbl
        .insert_tx(
            &record_with_str(email_key, "user@example.com"),
            Some(&mut tx),
        )
        .await
        .unwrap();

    // Step 2: CREATE the FIRST unique index (backfilling from committed
    // snapshot only — the tx's staged insert is invisible).
    let idx_name = key_id(&tbl, "idx_email").await;
    let def = IndexDefinition::new(idx_name, vec![IndexInfoItem::new(vec![email_key])]);
    tbl.index_manager_ref()
        .create_unique_index(def)
        .await
        .unwrap();

    // Step 3: COMMIT — the rederive MUST fire and record a UniqueGuard.
    repo.commit_tx(tx).await.expect("commit must succeed");

    // Assert (a): the row is physically present.
    let _ = tbl.get(rid).await.expect("row must be present");

    // Assert (b): a SECOND tx attempting to insert the SAME unique value
    // MUST be rejected. After T1's commit, the unique posting is in
    // info_store, so T2's stage-time validate_unique_for_create catches it.
    let (mut tx2, _guard2) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let result = tbl
        .insert_tx(
            &record_with_str(email_key, "user@example.com"),
            Some(&mut tx2),
        )
        .await;
    assert!(
        result.is_err(),
        "P0-2 2a unique: a second insert with the SAME unique value must \
         be rejected after the first tx's rederive recorded the guard and \
         committed the posting"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// 2a — unique index: UPDATE staged before CREATE UNIQUE INDEX
// ─────────────────────────────────────────────────────────────────────────────

/// A tx that stages an UPDATE changing the unique-keyed field before the
/// unique index exists, then commits after creation, MUST have the new value
/// constraint-enforced.
#[tokio::test]
async fn p02_unique_update_before_create_enforced() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));
    let tbl = repo.get_table("t").await.unwrap();

    let email_key = key_id(&tbl, "email").await;

    // Pre-existing row with old email.
    let rid = tbl
        .insert(&record_with_str(email_key, "old@example.com"))
        .await
        .unwrap();

    // Step 1: STAGE an update changing the email BEFORE any unique index.
    let (mut tx, _guard) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    tbl.update_tx(
        rid,
        &record_with_str(email_key, "new@example.com"),
        Some(&mut tx),
    )
    .await
    .unwrap();

    // Step 2: CREATE the unique index.
    let idx_name = key_id(&tbl, "idx_email").await;
    let def = IndexDefinition::new(idx_name, vec![IndexInfoItem::new(vec![email_key])]);
    tbl.index_manager_ref()
        .create_unique_index(def)
        .await
        .unwrap();

    // Step 3: COMMIT.
    repo.commit_tx(tx).await.expect("commit must succeed");

    // Assert: a new insert with the NEW email value is rejected (proves the
    // rederive recorded a UniqueGuard for the updated value).
    let (mut tx2, _guard2) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let result = tbl
        .insert_tx(
            &record_with_str(email_key, "new@example.com"),
            Some(&mut tx2),
        )
        .await;
    assert!(
        result.is_err(),
        "P0-2 2a unique UPDATE: the new email value must be constraint-enforced"
    );

    // Assert: the OLD email value is now free (the row was updated).
    let (mut tx3, _guard3) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let result_old = tbl
        .insert_tx(
            &record_with_str(email_key, "old@example.com"),
            Some(&mut tx3),
        )
        .await;
    assert!(
        result_old.is_ok(),
        "P0-2 2a unique UPDATE: the old email value must be free after update"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// 2a — unique index: concurrent duplicate (T1 stages before unique, T2 after)
// ─────────────────────────────────────────────────────────────────────────────

/// Two txs: T1 stages its insert BEFORE the unique index exists (so it has no
/// UniqueGuard). T2 stages the SAME value AFTER the unique index exists (so it
/// DOES get a UniqueGuard at stage time). Both commit. Only one must win.
///
/// Determinism: sequential ordering (T1 stage → DDL → T2 stage → T1 commit →
/// T2 commit). T2's stage-time validation sees the committed state, which
/// does NOT include T1's staged value — so T2 passes stage validation. At
/// T2's commit, Phase 2.6 checks the UniqueGuard against committed state,
/// which now includes T1's posting (T1 committed first) → T2 is rejected.
#[tokio::test]
async fn p02_unique_concurrent_duplicate_one_wins() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));
    let tbl = repo.get_table("t").await.unwrap();

    let email_key = key_id(&tbl, "email").await;

    // T1: stage insert BEFORE unique index exists (no UniqueGuard recorded
    // at stage time — the rederive will record one at commit).
    let (mut tx1, _guard1) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let _rid1 = tbl
        .insert_tx(
            &record_with_str(email_key, "dup@example.com"),
            Some(&mut tx1),
        )
        .await
        .unwrap();

    // DDL: create the unique index.
    let idx_name = key_id(&tbl, "idx_email").await;
    let def = IndexDefinition::new(idx_name, vec![IndexInfoItem::new(vec![email_key])]);
    tbl.index_manager_ref()
        .create_unique_index(def)
        .await
        .unwrap();

    // T2: stage the SAME value AFTER the unique index exists. Stage-time
    // validation checks committed state — T1 hasn't committed yet, so the
    // value is free → T2's validation passes, T2 records a UniqueGuard.
    let (mut tx2, _guard2) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let _rid2 = tbl
        .insert_tx(
            &record_with_str(email_key, "dup@example.com"),
            Some(&mut tx2),
        )
        .await;

    // T1 commits FIRST: rederive fires (base_index gen advanced), records a
    // UniqueGuard, Phase 2.6 validates (value free in committed state → OK),
    // Phase 5c writes the unique posting.
    repo.commit_tx(tx1).await.expect("T1 commit must succeed");

    // T2 commits: Phase 2.6 validates T2's UniqueGuard against committed
    // state. T1's posting is now in info_store → T2's guard check finds
    // the key owned by T1's rid ≠ T2's rid → REJECT.
    let t2_result = repo.commit_tx(tx2).await;
    assert!(
        t2_result.is_err(),
        "P0-2 2a concurrent: T2 must be rejected when T1 already committed \
         the same unique value. The rederive's retroactive UniqueGuard on \
         T1 is what made T1's posting visible for T2's Phase 2.6 check."
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// 2c — retired ops: DROP INDEX between stage and commit
// ─────────────────────────────────────────────────────────────────────────────

/// A tx stages ops against a regular index, then that index is DROP'd before
/// the tx commits. The stale posting op MUST be retracted — no orphan posting
/// for the dropped index after commit.
#[tokio::test]
async fn p02_regular_drop_index_before_commit_no_orphan() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));
    let tbl = repo.get_table("t").await.unwrap();

    let name_key = key_id(&tbl, "name").await;

    // Create a regular index FIRST.
    let idx_name = key_id(&tbl, "idx_name").await;
    let def = IndexDefinition::new(idx_name, vec![IndexInfoItem::new(vec![name_key])]);
    tbl.index_manager_ref().create_index(def).await.unwrap();

    // Stage an insert (the index exists, so ops are planned at stage time).
    let (mut tx, _guard) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let rid = tbl
        .insert_tx(&record_with_str(name_key, "alice"), Some(&mut tx))
        .await
        .unwrap();

    // DROP the index BEFORE commit.
    tbl.index_manager_ref().drop_index(idx_name).await.unwrap();

    // Commit — the rederive MUST retract the stale posting op for the
    // dropped index (2c fix).
    repo.commit_tx(tx).await.expect("commit must succeed");

    // Assert: the row IS physically present (data write is unaffected).
    let _ = tbl.get(rid).await.expect("row must be present");

    // Assert: looking up via the dropped index returns nothing (the index
    // is gone — its postings were swept by drop_index, and the commit did
    // NOT write a new orphan posting for it).
    // We verify indirectly: the commit succeeded without error, and no
    // orphan posting was written (the 2c retain dropped the op).
    // A direct check would scan info_store for the index's prefix, but
    // since drop_index already swept all postings, any new posting written
    // by the commit would be an orphan. The test's PASS (no error/panic)
    // + the row being present proves the commit pipeline didn't fail on
    // the dropped index.
    //
    // For a stronger assertion: try to create the SAME index again — if an
    // orphan posting was left, the lookup would return the committed row
    // under a freshly-created index that was NOT backfilled with this row.
    // But create_index backfills from the committed snapshot, which DOES
    // include this row (it was committed in step 3). So a re-create would
    // legitimately index it. Instead, verify via the info_store directly.
    // For a stronger assertion: verify via info_store that no orphan
    // posting was written. Regular posting key = 25-byte IndexRecordKey
    // || 16-byte RecordId = 41 bytes. Build it directly.
    let irk = crate::index::index_keys::build_index_key_from_record(
        false,
        idx_name,
        &record_with_str(name_key, "alice"),
        &[IndexInfoItem::new(vec![name_key])],
    );
    if let Some(irk) = irk {
        let index_key_bytes = irk.to_bytes();
        let mut posting_key = Vec::with_capacity(41);
        posting_key.extend_from_slice(&index_key_bytes);
        posting_key.extend_from_slice(rid.as_bytes());
        let orphan = tbl
            .info_store()
            .get(shamir_storage::types::RecordKey::from_slice(&posting_key))
            .await;
        // The posting should NOT exist (drop_index swept it, and the commit
        // retracted the stale op — no new posting was written).
        assert!(
            orphan.is_err(),
            "P0-2 2c: no orphan posting for the dropped index must exist \
             after commit. The 2c retain must have retracted the stale op."
        );
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 2c — retired ops: DROP UNIQUE INDEX between stage and commit
// ─────────────────────────────────────────────────────────────────────────────

/// Same as above but for a UNIQUE index. The unique posting op staged at
/// stage time must be retracted when the unique index is dropped before commit.
#[tokio::test]
async fn p02_unique_drop_index_before_commit_no_orphan() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));
    let tbl = repo.get_table("t").await.unwrap();

    let email_key = key_id(&tbl, "email").await;

    // Create a unique index FIRST.
    let idx_name = key_id(&tbl, "idx_email").await;
    let def = IndexDefinition::new(idx_name, vec![IndexInfoItem::new(vec![email_key])]);
    tbl.index_manager_ref()
        .create_unique_index(def)
        .await
        .unwrap();

    // Stage an insert.
    let (mut tx, _guard) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let rid = tbl
        .insert_tx(
            &record_with_str(email_key, "alice@example.com"),
            Some(&mut tx),
        )
        .await
        .unwrap();

    // DROP the unique index BEFORE commit.
    tbl.index_manager_ref()
        .drop_unique_index(idx_name)
        .await
        .unwrap();

    // Commit — the rederive MUST retract the stale unique posting op.
    repo.commit_tx(tx).await.expect("commit must succeed");

    // Assert: the row IS present.
    let _ = tbl.get(rid).await.expect("row must be present");

    // Assert: no orphan unique posting in info_store.
    let irk = crate::index::index_keys::build_index_key_from_record(
        true,
        idx_name,
        &record_with_str(email_key, "alice@example.com"),
        &[IndexInfoItem::new(vec![email_key])],
    );
    if let Some(irk) = irk {
        let key = irk.to_bytes();
        let orphan = tbl
            .info_store()
            .get(shamir_storage::types::RecordKey::from_slice(&key))
            .await;
        assert!(
            orphan.is_err(),
            "P0-2 2c unique: no orphan unique posting for the dropped index \
             must exist after commit."
        );
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 2a — fast-path: table with NO indexes at stage time (has_any_index() == false)
// ─────────────────────────────────────────────────────────────────────────────

/// Verify the rederive fires even when `has_any_index()` was false at stage
/// time (the L9 fast-path skipped all planning). The rederive must add ops
/// for the new regular index.
#[tokio::test]
async fn p02_no_indexes_at_stage_rederive_fires() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));
    let tbl = repo.get_table("t").await.unwrap();

    let name_key = key_id(&tbl, "name").await;

    assert!(!tbl.has_any_index(), "fresh table has no indexes");

    // Stage an insert with NO indexes (L9 fast-path: zero index ops staged).
    let (mut tx, _guard) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let rid = tbl
        .insert_tx(&record_with_str(name_key, "bob"), Some(&mut tx))
        .await
        .unwrap();

    // Verify NO index ops were staged (the L9 fast-path).
    assert!(
        tx.index_write_set.is_empty(),
        "L9 fast-path: no index ops should be staged on an unindexed table"
    );

    // Create a regular index.
    let idx_name = key_id(&tbl, "idx_name").await;
    let def = IndexDefinition::new(idx_name, vec![IndexInfoItem::new(vec![name_key])]);
    tbl.index_manager_ref().create_index(def).await.unwrap();

    // Commit — the rederive MUST fire and add ops for the new index.
    repo.commit_tx(tx).await.expect("commit must succeed");

    // Assert: the posting IS present.
    let found = tbl
        .index_manager_ref()
        .lookup_by_index(idx_name, &[InnerValue::Str("bob".into())])
        .await
        .unwrap();
    assert!(
        found.iter().any(|r| r == &rid),
        "P0-2 2a fast-path: the row staged on an unindexed table must be \
         present in the index created after staging, after commit."
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// 2a — unique index with multiple fields
// ─────────────────────────────────────────────────────────────────────────────

/// Verify the rederive handles a multi-field unique index created after staging.
#[tokio::test]
async fn p02_unique_multi_field_before_create_enforced() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));
    let tbl = repo.get_table("t").await.unwrap();

    let a_key = key_id(&tbl, "a").await;
    let b_key = key_id(&tbl, "b").await;

    // Stage an insert BEFORE the unique index exists.
    let (mut tx, _guard) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let _rid = tbl
        .insert_tx(&record_with_two_str(a_key, "x", b_key, "y"), Some(&mut tx))
        .await
        .unwrap();

    // Create a multi-field unique index.
    let idx_name = key_id(&tbl, "idx_ab").await;
    let def = IndexDefinition::new(
        idx_name,
        vec![
            IndexInfoItem::new(vec![a_key]),
            IndexInfoItem::new(vec![b_key]),
        ],
    );
    tbl.index_manager_ref()
        .create_unique_index(def)
        .await
        .unwrap();

    // Commit.
    repo.commit_tx(tx).await.expect("commit must succeed");

    // Assert: a second insert with the SAME (a, b) values is rejected.
    let (mut tx2, _guard2) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let result = tbl
        .insert_tx(&record_with_two_str(a_key, "x", b_key, "y"), Some(&mut tx2))
        .await;
    assert!(
        result.is_err(),
        "P0-2 2a unique multi-field: duplicate (a, b) must be rejected"
    );
}
