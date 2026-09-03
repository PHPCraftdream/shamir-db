//! #1096: Stage-time unique check must be tx-aware of the transaction's
//! own prior releases.
//!
//! Tests the fix for the bug where `insert_tx`'s stage-time unique check
//! rejects legitimate release-then-reclaim patterns (DELETE or UPDATE-off
//! of a durable record, followed by INSERT claiming the same unique key
//! in the same transaction).

use std::sync::Arc;

use shamir_storage::storage_in_memory::InMemoryRepo;
use shamir_tx::IsolationLevel;
use shamir_types::core::interner::{InternerKey, TouchInd};
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

fn record_with_two_str(key1: u64, val1: &str, key2: u64, val2: &str) -> InnerValue {
    let mut m = new_map_wc(2);
    m.insert(InternerKey::new(key1), InnerValue::Str(val1.into()));
    m.insert(InternerKey::new(key2), InnerValue::Str(val2.into()));
    InnerValue::Map(m)
}

/// #1096 - DELETE-then-reclaim scenario:
/// tx1: INSERT A {email:"x"}; COMMIT
/// tx2: DELETE A; INSERT B {email:"x"}  // must succeed, B owns the unique value
#[tokio::test]
async fn tx_delete_then_reclaim_unique_key_succeeds() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));
    let tbl = repo.get_table("t").await.unwrap();
    tbl.create_unique_index("by_email", &["email"])
        .await
        .unwrap();
    let email_id = key_id(&tbl, "by_email").await;
    let email_field = key_id(&tbl, "email").await;

    // tx1: INSERT A {email:"x"}; COMMIT
    let (mut tx1, _g1) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let rid_a = tbl
        .insert_tx(&record_with_str(email_field, "x"), Some(&mut tx1))
        .await
        .expect("tx1 INSERT A must succeed");
    repo.commit_tx(tx1).await.expect("tx1 commit must succeed");

    // Verify A is in the unique index
    let owner = tbl
        .index_manager()
        .lookup_by_unique_index(email_id, &[InnerValue::Str("x".into())])
        .await
        .unwrap();
    assert_eq!(
        owner,
        Some(rid_a),
        "A must own the unique value after tx1 commits"
    );

    // tx2: DELETE A; INSERT B {email:"x"}  // must succeed, B owns the unique value
    let (mut tx2, _g2) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();

    // DELETE A - this stages a RemovePosting for the unique key
    tbl.delete_tx(rid_a, Some(&mut tx2))
        .await
        .expect("tx2 DELETE A must succeed");

    // INSERT B with the same unique value - before the fix, this would fail
    // with DuplicateKey at stage time because the stage-time check only sees
    // durable state (A still there). After the fix, it recognizes that the key
    // was released earlier in this tx and allows the reclaim.
    let rid_b = tbl
        .insert_tx(&record_with_str(email_field, "x"), Some(&mut tx2))
        .await
        .expect("tx2 INSERT B must succeed - key was released by DELETE A");

    // Commit tx2
    repo.commit_tx(tx2).await.expect("tx2 commit must succeed");

    // Verify B now owns the unique value
    let owner = tbl
        .index_manager()
        .lookup_by_unique_index(email_id, &[InnerValue::Str("x".into())])
        .await
        .unwrap();
    assert_eq!(
        owner,
        Some(rid_b),
        "B must own the unique value after tx2 commits"
    );

    // Verify A is gone from data store
    let a_gone_result = tbl.get(rid_a).await;
    assert!(
        a_gone_result.is_err(),
        "A must be deleted from the data store"
    );
}

/// #1096 - UPDATE-off-then-reclaim scenario:
/// tx1: INSERT A {email:"x"}; COMMIT
/// tx2: UPDATE A SET email="z"; INSERT B {email:"x"}  // must succeed
#[tokio::test]
async fn tx_update_off_then_reclaim_unique_key_succeeds() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));
    let tbl = repo.get_table("t").await.unwrap();
    tbl.create_unique_index("by_email", &["email"])
        .await
        .unwrap();
    let email_id = key_id(&tbl, "by_email").await;
    let email_field = key_id(&tbl, "email").await;
    let name_field = key_id(&tbl, "name").await;

    // tx1: INSERT A {email:"x", name:"alice"}; COMMIT
    let (mut tx1, _g1) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let rid_a = tbl
        .insert_tx(
            &record_with_two_str(email_field, "x", name_field, "alice"),
            Some(&mut tx1),
        )
        .await
        .expect("tx1 INSERT A must succeed");
    repo.commit_tx(tx1).await.expect("tx1 commit must succeed");

    // Verify A owns "x" in the unique index
    let owner = tbl
        .index_manager()
        .lookup_by_unique_index(email_id, &[InnerValue::Str("x".into())])
        .await
        .unwrap();
    assert_eq!(
        owner,
        Some(rid_a),
        "A must own the unique value 'x' after tx1 commits"
    );

    // tx2: UPDATE A SET email="z"; INSERT B {email:"x"}  // must succeed
    let (mut tx2, _g2) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();

    // UPDATE A SET email="z" - this stages a RemovePosting for key "x"
    let a_updated = record_with_two_str(email_field, "z", name_field, "alice");
    tbl.update_tx(rid_a, &a_updated, Some(&mut tx2))
        .await
        .expect("tx2 UPDATE A must succeed");

    // INSERT B with the old unique value "x" - before the fix, this would fail
    // with DuplicateKey at stage time. After the fix, it recognizes that the key
    // was released by the UPDATE and allows the reclaim.
    let rid_b = tbl
        .insert_tx(
            &record_with_two_str(email_field, "x", name_field, "bob"),
            Some(&mut tx2),
        )
        .await
        .expect("tx2 INSERT B must succeed - key was released by UPDATE A");

    // Commit tx2
    repo.commit_tx(tx2).await.expect("tx2 commit must succeed");

    // Verify B now owns "x" in the unique index
    let owner_x = tbl
        .index_manager()
        .lookup_by_unique_index(email_id, &[InnerValue::Str("x".into())])
        .await
        .unwrap();
    assert_eq!(
        owner_x,
        Some(rid_b),
        "B must own the unique value 'x' after tx2 commits"
    );

    // Verify A now owns "z" in the unique index
    let owner_z = tbl
        .index_manager()
        .lookup_by_unique_index(email_id, &[InnerValue::Str("z".into())])
        .await
        .unwrap();
    assert_eq!(
        owner_z,
        Some(rid_a),
        "A must own the unique value 'z' after tx2 commits"
    );

    // Verify both records exist with correct names
    let a_val = tbl.get(rid_a).await.unwrap();
    let b_val = tbl.get(rid_b).await.unwrap();
    let name_key = InternerKey::new(name_field);
    let a_name = match &a_val {
        InnerValue::Map(m) => m.get(&name_key).and_then(|v| match v {
            InnerValue::Str(s) => Some(s.as_str()),
            _ => None,
        }),
        _ => None,
    };
    let b_name = match &b_val {
        InnerValue::Map(m) => m.get(&name_key).and_then(|v| match v {
            InnerValue::Str(s) => Some(s.as_str()),
            _ => None,
        }),
        _ => None,
    };
    assert_eq!(a_name, Some("alice"), "A's name must be 'alice'");
    assert_eq!(b_name, Some("bob"), "B's name must be 'bob'");
}

/// Test that a genuine double-claim still rejects:
/// tx: INSERT A {email:"x"}; INSERT B {email:"x"}  // commit must abort
///
/// Neither A nor B is durable while staging (both are same-tx inserts), so
/// `insert_tx`'s stage-time check (durable-only, even after #1096) cannot
/// see the conflict — `insert_tx` for B succeeds optimistically. The
/// rejection is `pre_commit.rs`'s Step 1 walk (no `RemovePosting` between
/// the two `SetPosting`s for the same key -> genuine intra-tx collision),
/// which runs at `commit_tx`. This mirrors the pre-existing coverage in
/// `base_index_tx_tests.rs::intra_tx_unique_collision_silently_overwrites`
/// (#1039) — kept here too so `released_unique_keys_in_tx`'s "still-live,
/// not released" case has direct #1096-local coverage.
#[tokio::test]
async fn tx_genuine_double_claim_still_rejects() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));
    let tbl = repo.get_table("t").await.unwrap();
    tbl.create_unique_index("by_email", &["email"])
        .await
        .unwrap();
    let _email_id = key_id(&tbl, "by_email").await;
    let email_field = key_id(&tbl, "email").await;

    let (mut tx, _g) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();

    // INSERT A {email:"x"} - succeeds
    let _rid_a = tbl
        .insert_tx(&record_with_str(email_field, "x"), Some(&mut tx))
        .await
        .expect("INSERT A must succeed");

    // INSERT B {email:"x"} - stages fine (neither claim is durable yet).
    // There is no DELETE or UPDATE-off between these, so this is a genuine
    // duplicate; the rejection happens at commit time, not here.
    let _rid_b = tbl
        .insert_tx(&record_with_str(email_field, "x"), Some(&mut tx))
        .await
        .expect("INSERT B stages fine - neither claim is durable yet");

    // Commit must abort: pre_commit's Step 1 walk sees two SetPostings for
    // the same key with no RemovePosting between them.
    let commit_result = repo.commit_tx(tx).await;
    assert!(
        matches!(
            commit_result,
            Err(crate::tx::CommitError::UniqueViolation { .. })
        ),
        "commit must abort with UniqueViolation for genuine intra-tx double-claim; got {:?}",
        commit_result
    );
}

/// Test that the released walk is correct: a key claimed multiple times
/// in the same tx but never released still rejects the duplicate.
#[tokio::test]
async fn tx_multiple_claim_without_release_still_rejects() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));
    let tbl = repo.get_table("t").await.unwrap();
    tbl.create_unique_index("by_email", &["email"])
        .await
        .unwrap();
    let _email_id = key_id(&tbl, "by_email").await;
    let email_field = key_id(&tbl, "email").await;

    // First, insert a durable record with email="y"
    let (mut tx0, _g0) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let _rid_y = tbl
        .insert_tx(&record_with_str(email_field, "y"), Some(&mut tx0))
        .await
        .expect("INSERT for email='y' must succeed");
    repo.commit_tx(tx0).await.expect("commit must succeed");

    // Now in a new tx, try to claim the same email twice without releasing
    let (mut tx, _g) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();

    // INSERT A {email:"y"} - should fail at stage time because "y" is already durable
    let dup1 = tbl
        .insert_tx(&record_with_str(email_field, "y"), Some(&mut tx))
        .await;
    assert!(
        matches!(dup1, Err(shamir_storage::error::DbError::DuplicateKey(_))),
        "INSERT A must fail because 'y' is already owned by durable record; got {:?}",
        dup1
    );
}

/// Verify the `released_unique_keys_in_tx` helper correctly tracks
/// the live/released state walking through a complex sequence of ops.
#[tokio::test]
async fn released_unique_keys_in_tx_walks_correctly() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));
    let tbl = repo.get_table("t").await.unwrap();
    tbl.create_unique_index("by_email", &["email"])
        .await
        .unwrap();

    let (mut tx, _g) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let token = tbl.table_token();

    // Sequence of staged ops in index_write_set:
    // 1. INSERT A {email:"x"} -> SetPosting "x"
    // 2. INSERT B {email:"y"} -> SetPosting "y"
    // 3. DELETE A -> RemovePosting "x"
    // 4. INSERT C {email:"z"} -> SetPosting "z"
    // 5. INSERT B again (same email) -> SetPosting "y" (same owner, not a conflict)
    // 6. DELETE B -> RemovePosting "y"
    // 7. INSERT D {email:"x"} -> SetPosting "x" (released by step 3, should be OK)

    let _email_id = key_id(&tbl, "by_email").await;
    let email_field = key_id(&tbl, "email").await;

    // 1. INSERT A {email:"x"}
    let rid_a = tbl
        .insert_tx(&record_with_str(email_field, "x"), Some(&mut tx))
        .await
        .unwrap();

    // 2. INSERT B {email:"y"}
    let rid_b = tbl
        .insert_tx(&record_with_str(email_field, "y"), Some(&mut tx))
        .await
        .unwrap();

    // 3. DELETE A
    tbl.delete_tx(rid_a, Some(&mut tx)).await.unwrap();

    // 4. INSERT C {email:"z"}
    let _rid_c = tbl
        .insert_tx(&record_with_str(email_field, "z"), Some(&mut tx))
        .await
        .unwrap();

    // 5. UPDATE B to same email (no-op for unique index, but creates staging)
    // This tests that re-claiming the same key with the same owner is OK.
    // B is only staged (not committed) in this same tx, so a plain `get`
    // (durable-only) would miss it -- reuse the known value instead.
    let b_val = record_with_str(email_field, "y");
    tbl.update_tx(rid_b, &b_val, Some(&mut tx)).await.unwrap();

    // 6. DELETE B
    tbl.delete_tx(rid_b, Some(&mut tx)).await.unwrap();

    // At this point:
    // - "x" is released (by DELETE A in step 3)
    // - "y" is released (by DELETE B in step 6)
    // - "z" is live (owned by C)

    // Refresh the cache and borrow the released set
    crate::table::refresh_released_unique_cache(&mut tx, token);
    let released = &tx.released_unique_cache[&token].released;

    // Build the expected index keys for "x" and "y"
    let index_mgr = tbl.index_manager();
    let key_x_bytes = index_mgr
        .unique_keys_for(&record_with_str(email_field, "x"))
        .into_iter()
        .next()
        .unwrap()
        .to_vec();
    let key_y_bytes = index_mgr
        .unique_keys_for(&record_with_str(email_field, "y"))
        .into_iter()
        .next()
        .unwrap()
        .to_vec();
    let key_z_bytes = index_mgr
        .unique_keys_for(&record_with_str(email_field, "z"))
        .into_iter()
        .next()
        .unwrap()
        .to_vec();

    assert!(
        released.contains(&key_x_bytes),
        "key 'x' must be in released set (DELETE A)"
    );
    assert!(
        released.contains(&key_y_bytes),
        "key 'y' must be in released set (DELETE B)"
    );
    assert!(
        !released.contains(&key_z_bytes),
        "key 'z' must NOT be in released set (still owned by C)"
    );

    // 7. INSERT D {email:"x"} - this should succeed because "x" was released
    let _rid_d = tbl
        .insert_tx(&record_with_str(email_field, "x"), Some(&mut tx))
        .await
        .expect("INSERT D must succeed - key 'x' was released by DELETE A");
}

/// #1096 - insert_tx_many also benefits from the fix:
/// DELETE a durable record, then use insert_tx_many to insert a new record
/// with the same unique key.
#[tokio::test]
async fn insert_tx_many_after_delete_reclaim_succeeds() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));
    let tbl = repo.get_table("t").await.unwrap();
    tbl.create_unique_index("by_email", &["email"])
        .await
        .unwrap();
    let email_id = key_id(&tbl, "by_email").await;
    let email_field = key_id(&tbl, "email").await;

    // Insert a durable record with email="x"
    let (mut tx1, _g1) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let rid_a = tbl
        .insert_tx(&record_with_str(email_field, "x"), Some(&mut tx1))
        .await
        .expect("INSERT A must succeed");
    repo.commit_tx(tx1).await.expect("commit must succeed");

    // New tx: DELETE A, then insert_tx_many B with same email
    let (mut tx2, _g2) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();

    // DELETE A
    tbl.delete_tx(rid_a, Some(&mut tx2))
        .await
        .expect("DELETE A must succeed");

    // insert_tx_many with a record that has email="x" - should succeed
    let values = vec![record_with_str(email_field, "x")];
    let ids = tbl
        .insert_tx_many(&values, &mut tx2)
        .await
        .expect("insert_tx_many must succeed - key was released by DELETE A");

    assert_eq!(ids.len(), 1, "insert_tx_many should return one id");

    // Commit tx2
    repo.commit_tx(tx2).await.expect("commit must succeed");

    // Verify the new record owns the unique value
    let owner = tbl
        .index_manager()
        .lookup_by_unique_index(email_id, &[InnerValue::Str("x".into())])
        .await
        .unwrap();
    assert_eq!(
        owner,
        Some(ids[0]),
        "the new record must own the unique value 'x'"
    );
}

/// #1096 follow-up (found by `@oh` review): a stale-snapshot release plan
/// must NOT be tolerated when a CONCURRENT tx has already reclaimed the
/// durable key for an unrelated record. Without the `is_record_touched`
/// cross-check (#1099: an on-demand probe, formerly a pre-built
/// `touched_records_in_tx` set), `released_unique_keys_in_tx` alone would incorrectly treat
/// "this tx once planned a RemovePosting for this key" as sufficient,
/// silently admitting two live records for the same unique value.
///
/// By the time `tx2` stages its own reclaiming INSERT, the durable owner of
/// "x" has already changed to a record `tx2` never touched — the FIXED
/// stage-time check (not just the commit-time one) catches this immediately:
///
/// ```text
/// tx0: INSERT A {email:"x"}; COMMIT
/// tx2: BEGIN                                    // snapshot sees A owning "x"
/// tx1: DELETE A; INSERT B {email:"x"}; COMMIT   // durable owner is now B
/// tx2: DELETE A; INSERT C {email:"x"}           // must now reject at stage time
/// ```
#[tokio::test]
async fn stale_snapshot_release_does_not_bypass_a_concurrent_reclaim() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));
    let tbl = repo.get_table("t").await.unwrap();
    tbl.create_unique_index("by_email", &["email"])
        .await
        .unwrap();
    let email_id = key_id(&tbl, "by_email").await;
    let email_field = key_id(&tbl, "email").await;

    // tx0: INSERT A {email:"x"}; COMMIT
    let (mut tx0, _g0) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let rid_a = tbl
        .insert_tx(&record_with_str(email_field, "x"), Some(&mut tx0))
        .await
        .expect("tx0 INSERT A must succeed");
    repo.commit_tx(tx0).await.expect("tx0 commit must succeed");

    // tx2: BEGIN — snapshot sees A owning "x".
    let (mut tx2, _g2) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();

    // tx1: DELETE A; INSERT B {email:"x"}; COMMIT — concurrent, commits FIRST.
    let (mut tx1, _g1) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    tbl.delete_tx(rid_a, Some(&mut tx1))
        .await
        .expect("tx1 DELETE A must succeed");
    let rid_b = tbl
        .insert_tx(&record_with_str(email_field, "x"), Some(&mut tx1))
        .await
        .expect("tx1 INSERT B must succeed");
    repo.commit_tx(tx1).await.expect("tx1 commit must succeed");

    // Durable owner of "x" is now B, not A.
    let owner = tbl
        .index_manager()
        .lookup_by_unique_index(email_id, &[InnerValue::Str("x".into())])
        .await
        .unwrap();
    assert_eq!(
        owner,
        Some(rid_b),
        "B must durably own 'x' after tx1 commits"
    );

    // tx2: DELETE A (stale — A is no longer the durable owner of "x", but
    // A itself is still present in tx2's own snapshot/write_set); INSERT C
    // {email:"x"} — the bypass, if present, would let this stage
    // (tolerating any release-marked key), silently heading toward two live
    // records under a UNIQUE index once tx2 commits.
    tbl.delete_tx(rid_a, Some(&mut tx2))
        .await
        .expect("tx2 DELETE A stages fine (A is stale but still exists)");
    let insert_c = tbl
        .insert_tx(&record_with_str(email_field, "x"), Some(&mut tx2))
        .await;
    assert!(
        matches!(
            insert_c,
            Err(shamir_storage::error::DbError::DuplicateKey(_))
        ),
        "tx2's INSERT C must reject: its release plan for 'x' was built \
         against a stale snapshot (A), but the durable owner is now B, a \
         record tx2 never touched — admitting this would leave B and C \
         both live under a UNIQUE index; got {:?}",
        insert_c
    );

    // B must still be the sole owner — no duplicate was created.
    let owner_after = tbl
        .index_manager()
        .lookup_by_unique_index(email_id, &[InnerValue::Str("x".into())])
        .await
        .unwrap();
    assert_eq!(
        owner_after,
        Some(rid_b),
        "B must remain the sole owner of 'x' after tx2's rejected INSERT"
    );
}

/// #1096 follow-up (found by `@oh` review): the SAME stale-snapshot bypass,
/// but arranged so the race lands strictly BETWEEN this tx's own (correctly
/// tolerated) stage-time check and its commit — exercising `pre_commit.rs`'s
/// Step 2 durable check specifically, not the stage-time one.
///
/// ```text
/// tx0: INSERT A {email:"x"}; COMMIT
/// tx2: BEGIN; DELETE A; INSERT C {email:"x"}   // stages fine: A IS the
///                                                  durable owner tx2 itself
///                                                  is releasing, no race yet
/// tx1: BEGIN; DELETE A; INSERT B {email:"x"}; COMMIT   // commits BEFORE tx2
/// tx2: COMMIT                                   // must now abort
/// ```
#[tokio::test]
async fn stale_snapshot_release_does_not_bypass_a_concurrent_reclaim_at_commit_time() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));
    let tbl = repo.get_table("t").await.unwrap();
    tbl.create_unique_index("by_email", &["email"])
        .await
        .unwrap();
    let email_id = key_id(&tbl, "by_email").await;
    let email_field = key_id(&tbl, "email").await;

    // tx0: INSERT A {email:"x"}; COMMIT
    let (mut tx0, _g0) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let rid_a = tbl
        .insert_tx(&record_with_str(email_field, "x"), Some(&mut tx0))
        .await
        .expect("tx0 INSERT A must succeed");
    repo.commit_tx(tx0).await.expect("tx0 commit must succeed");

    // tx2: DELETE A; INSERT C {email:"x"} — stages fine, no race has
    // happened yet (A is still the sole durable owner tx2 itself touched).
    let (mut tx2, _g2) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    tbl.delete_tx(rid_a, Some(&mut tx2))
        .await
        .expect("tx2 DELETE A must succeed");
    let _rid_c = tbl
        .insert_tx(&record_with_str(email_field, "x"), Some(&mut tx2))
        .await
        .expect("tx2 INSERT C must stage fine — no concurrent tx yet");

    // tx1: DELETE A; INSERT B {email:"x"}; COMMIT — races in and commits
    // BEFORE tx2, becoming the new durable owner of "x".
    let (mut tx1, _g1) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    tbl.delete_tx(rid_a, Some(&mut tx1))
        .await
        .expect("tx1 DELETE A must succeed");
    let rid_b = tbl
        .insert_tx(&record_with_str(email_field, "x"), Some(&mut tx1))
        .await
        .expect("tx1 INSERT B must succeed");
    repo.commit_tx(tx1).await.expect("tx1 commit must succeed");

    // tx2 commits last — its stage-time check never saw B, so this MUST be
    // caught by pre_commit.rs's Step 2 durable check instead.
    let commit_result = repo.commit_tx(tx2).await;
    assert!(
        matches!(
            commit_result,
            Err(crate::tx::CommitError::UniqueViolation { .. })
        ),
        "tx2 must abort at commit: the durable owner of 'x' is now B, a \
         record tx2 never touched — admitting this would leave B and C \
         both live under a UNIQUE index; got {:?}",
        commit_result
    );

    let owner_after = tbl
        .index_manager()
        .lookup_by_unique_index(email_id, &[InnerValue::Str("x".into())])
        .await
        .unwrap();
    assert_eq!(
        owner_after,
        Some(rid_b),
        "B must remain the sole owner of 'x' after tx2's aborted commit"
    );
}

/// #1096 follow-up (found by `@oh` review): `insert_tx_many_bytes` is the
/// actual wire path `execute_insert_tx`/`execute_set_tx` call for every
/// transactional INSERT/UPSERT — it must get the same tx-aware
/// release-then-reclaim treatment as the direct `insert_tx` API.
#[tokio::test]
async fn insert_tx_many_bytes_after_delete_reclaim_succeeds() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));
    let tbl = repo.get_table("t").await.unwrap();
    tbl.create_unique_index("by_email", &["email"])
        .await
        .unwrap();
    let email_id = key_id(&tbl, "by_email").await;
    let email_field = key_id(&tbl, "email").await;

    let (mut tx1, _g1) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let rid_a = tbl
        .insert_tx(&record_with_str(email_field, "x"), Some(&mut tx1))
        .await
        .expect("INSERT A must succeed");
    repo.commit_tx(tx1).await.expect("commit must succeed");

    let (mut tx2, _g2) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    tbl.delete_tx(rid_a, Some(&mut tx2))
        .await
        .expect("DELETE A must succeed");

    let b_bytes = record_with_str(email_field, "x")
        .to_bytes()
        .expect("encode succeeds");
    let ids = tbl
        .insert_tx_many_bytes(&[b_bytes], &mut tx2)
        .await
        .expect("insert_tx_many_bytes must succeed - key was released by DELETE A");
    assert_eq!(ids.len(), 1);

    repo.commit_tx(tx2).await.expect("commit must succeed");

    let owner = tbl
        .index_manager()
        .lookup_by_unique_index(email_id, &[InnerValue::Str("x".into())])
        .await
        .unwrap();
    assert_eq!(owner, Some(ids[0]));
}

/// #1096 follow-up (found by `@oh` review): the RECLAIMING half of a
/// release-then-reclaim pair can also be an UPDATE, not just an INSERT —
/// `tx: DELETE A {email:"x"}; UPDATE C SET email="x"` must succeed exactly
/// like the INSERT-reclaims-a-released-key case.
#[tokio::test]
async fn tx_update_reclaims_a_key_released_by_delete_succeeds() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));
    let tbl = repo.get_table("t").await.unwrap();
    tbl.create_unique_index("by_email", &["email"])
        .await
        .unwrap();
    let email_id = key_id(&tbl, "by_email").await;
    let email_field = key_id(&tbl, "email").await;
    let name_field = key_id(&tbl, "name").await;

    let (mut tx1, _g1) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let rid_a = tbl
        .insert_tx(&record_with_str(email_field, "x"), Some(&mut tx1))
        .await
        .expect("INSERT A must succeed");
    let rid_c = tbl
        .insert_tx(
            &record_with_two_str(email_field, "y", name_field, "carol"),
            Some(&mut tx1),
        )
        .await
        .expect("INSERT C must succeed");
    repo.commit_tx(tx1).await.expect("commit must succeed");

    let (mut tx2, _g2) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    tbl.delete_tx(rid_a, Some(&mut tx2))
        .await
        .expect("DELETE A must succeed");
    tbl.update_tx(
        rid_c,
        &record_with_two_str(email_field, "x", name_field, "carol"),
        Some(&mut tx2),
    )
    .await
    .expect("UPDATE C to reclaim 'x' must succeed - key was released by DELETE A");

    repo.commit_tx(tx2).await.expect("commit must succeed");

    let owner = tbl
        .index_manager()
        .lookup_by_unique_index(email_id, &[InnerValue::Str("x".into())])
        .await
        .unwrap();
    assert_eq!(owner, Some(rid_c), "C must now own the reclaimed value 'x'");
}

/// #1096 follow-up (found by a third `@oh` review): no test previously
/// asserted a genuine (non-released) `DuplicateKey` rejection out of an
/// UPDATE path — `update_tx`'s `Some(old_val)` branch
/// (`validate_unique_for_update_with_released`) had zero coverage for its
/// non-tolerance branch, so a bug that wrongly tolerated ANY durable
/// conflict on that path would have gone undetected by this module.
#[tokio::test]
async fn tx_update_to_a_durably_owned_unreleased_key_still_rejects() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));
    let tbl = repo.get_table("t").await.unwrap();
    tbl.create_unique_index("by_email", &["email"])
        .await
        .unwrap();
    let email_field = key_id(&tbl, "email").await;

    let (mut tx1, _g1) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let _rid_a = tbl
        .insert_tx(&record_with_str(email_field, "x"), Some(&mut tx1))
        .await
        .expect("INSERT A must succeed");
    let rid_d = tbl
        .insert_tx(&record_with_str(email_field, "other"), Some(&mut tx1))
        .await
        .expect("INSERT D must succeed");
    repo.commit_tx(tx1).await.expect("commit must succeed");

    // tx2: UPDATE D SET email="x" — "x" is durably owned by A, and tx2
    // never released it (no DELETE/UPDATE-off of A in this tx) — a
    // genuine conflict that must still reject.
    let (mut tx2, _g2) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let result = tbl
        .update_tx(rid_d, &record_with_str(email_field, "x"), Some(&mut tx2))
        .await;
    assert!(
        matches!(result, Err(shamir_storage::error::DbError::DuplicateKey(_))),
        "UPDATE D to a durably-owned, never-released key must reject; got {:?}",
        result
    );
}

/// Same scenario as
/// [`tx_update_to_a_durably_owned_unreleased_key_still_rejects`], but
/// exercised through `update_tx_bytes` — the wire UPDATE path
/// `execute_update_tx` actually calls, and the one call site the third
/// `@oh` review found had zero exercise anywhere in this module.
#[tokio::test]
async fn update_tx_bytes_to_a_durably_owned_unreleased_key_still_rejects() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));
    let tbl = repo.get_table("t").await.unwrap();
    tbl.create_unique_index("by_email", &["email"])
        .await
        .unwrap();
    let email_field = key_id(&tbl, "email").await;

    let (mut tx1, _g1) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let _rid_a = tbl
        .insert_tx(&record_with_str(email_field, "x"), Some(&mut tx1))
        .await
        .expect("INSERT A must succeed");
    let rid_d = tbl
        .insert_tx(&record_with_str(email_field, "other"), Some(&mut tx1))
        .await
        .expect("INSERT D must succeed");
    repo.commit_tx(tx1).await.expect("commit must succeed");

    let (mut tx2, _g2) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let old_bytes = record_with_str(email_field, "other")
        .to_bytes()
        .expect("encode succeeds");
    let new_bytes = record_with_str(email_field, "x")
        .to_bytes()
        .expect("encode succeeds");
    let result = tbl
        .update_tx_bytes(rid_d, &old_bytes, new_bytes, &mut tx2, false)
        .await;
    assert!(
        matches!(result, Err(shamir_storage::error::DbError::DuplicateKey(_))),
        "update_tx_bytes to a durably-owned, never-released key must reject; got {:?}",
        result
    );
}

/// #1104 - DELETE-then-reclaim via `update_tx_bytes`:
/// tx: delete_tx(A{email:"x"}) then update_tx_bytes(D, ..., email="x")
/// in the SAME tx must succeed (release-then-reclaim via the wire UPDATE path).
#[tokio::test]
async fn update_tx_bytes_after_delete_reclaim_succeeds() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));
    let tbl = repo.get_table("t").await.unwrap();
    tbl.create_unique_index("by_email", &["email"])
        .await
        .unwrap();
    let email_id = key_id(&tbl, "by_email").await;
    let email_field = key_id(&tbl, "email").await;

    // Insert a durable record with email="x"
    let (mut tx1, _g1) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let rid_a = tbl
        .insert_tx(&record_with_str(email_field, "x"), Some(&mut tx1))
        .await
        .expect("INSERT A must succeed");
    repo.commit_tx(tx1).await.expect("commit must succeed");

    // Verify A is in the unique index
    let owner = tbl
        .index_manager()
        .lookup_by_unique_index(email_id, &[InnerValue::Str("x".into())])
        .await
        .unwrap();
    assert_eq!(
        owner,
        Some(rid_a),
        "A must own the unique value 'x' after tx1 commits"
    );

    // New tx: DELETE A, then update_tx_bytes D with same email
    let (mut tx2, _g2) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();

    // DELETE A - this stages a RemovePosting for the unique key
    tbl.delete_tx(rid_a, Some(&mut tx2))
        .await
        .expect("DELETE A must succeed");

    // Insert a second durable record D with a different email
    let (mut tx0, _g0) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let rid_d = tbl
        .insert_tx(&record_with_str(email_field, "other"), Some(&mut tx0))
        .await
        .expect("INSERT D must succeed");
    repo.commit_tx(tx0).await.expect("commit must succeed");

    // Now in tx2, update_tx_bytes D to claim email="x" - should succeed
    // because tx2 itself released "x" by deleting A
    let old_bytes = record_with_str(email_field, "other")
        .to_bytes()
        .expect("encode succeeds");
    let new_bytes = record_with_str(email_field, "x")
        .to_bytes()
        .expect("encode succeeds");
    tbl.update_tx_bytes(rid_d, &old_bytes, new_bytes, &mut tx2, false)
        .await
        .expect("update_tx_bytes must succeed - key 'x' was released by DELETE A");

    // Commit tx2
    repo.commit_tx(tx2).await.expect("commit must succeed");

    // Verify D now owns the unique value 'x'
    let owner = tbl
        .index_manager()
        .lookup_by_unique_index(email_id, &[InnerValue::Str("x".into())])
        .await
        .unwrap();
    assert_eq!(
        owner,
        Some(rid_d),
        "D must own the unique value 'x' after tx2 commits"
    );

    // Verify A is gone
    let a_gone_result = tbl.get(rid_a).await;
    assert!(
        a_gone_result.is_err(),
        "A must be deleted from the data store"
    );
}

/// #1104 - stale-snapshot release via `update_tx_bytes` must NOT bypass
/// a concurrent reclaim:
///
/// ```text
/// tx0: INSERT A {email:"x"}; COMMIT
/// tx2: BEGIN                                    // snapshot sees A owning "x"
/// tx1: DELETE A; INSERT B {email:"x"}; COMMIT   // durable owner is now B
/// tx2: DELETE A; UPDATE D SET email="x"         // must now reject at stage time
/// ```
#[tokio::test]
async fn stale_snapshot_release_does_not_bypass_a_concurrent_reclaim_via_update_tx_bytes() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));
    let tbl = repo.get_table("t").await.unwrap();
    tbl.create_unique_index("by_email", &["email"])
        .await
        .unwrap();
    let email_id = key_id(&tbl, "by_email").await;
    let email_field = key_id(&tbl, "email").await;

    // tx0: INSERT A {email:"x"}; COMMIT
    let (mut tx0, _g0) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let rid_a = tbl
        .insert_tx(&record_with_str(email_field, "x"), Some(&mut tx0))
        .await
        .expect("tx0 INSERT A must succeed");
    repo.commit_tx(tx0).await.expect("tx0 commit must succeed");

    // tx2: BEGIN — snapshot sees A owning "x".
    let (mut tx2, _g2) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();

    // tx1: DELETE A; INSERT B {email:"x"}; COMMIT — concurrent, commits FIRST.
    let (mut tx1, _g1) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    tbl.delete_tx(rid_a, Some(&mut tx1))
        .await
        .expect("tx1 DELETE A must succeed");
    let rid_b = tbl
        .insert_tx(&record_with_str(email_field, "x"), Some(&mut tx1))
        .await
        .expect("tx1 INSERT B must succeed");
    repo.commit_tx(tx1).await.expect("tx1 commit must succeed");

    // Durable owner of "x" is now B, not A.
    let owner = tbl
        .index_manager()
        .lookup_by_unique_index(email_id, &[InnerValue::Str("x".into())])
        .await
        .unwrap();
    assert_eq!(
        owner,
        Some(rid_b),
        "B must durably own 'x' after tx1 commits"
    );

    // tx2: DELETE A (stale — A is no longer the durable owner of "x", but
    // A itself is still present in tx2's own snapshot/write_set); then
    // insert a second durable record D with a different email.
    let (mut tx0b, _g0b) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let rid_d = tbl
        .insert_tx(&record_with_str(email_field, "other"), Some(&mut tx0b))
        .await
        .expect("INSERT D must succeed");
    repo.commit_tx(tx0b).await.expect("commit must succeed");

    // Now in tx2, try to UPDATE D to email="x" via update_tx_bytes — this
    // would be a bypass if allowed (tolerating any release-marked key),
    // silently heading toward two live records under a UNIQUE index once
    // tx2 commits.
    tbl.delete_tx(rid_a, Some(&mut tx2))
        .await
        .expect("tx2 DELETE A stages fine (A is stale but still exists)");
    let old_bytes = record_with_str(email_field, "other")
        .to_bytes()
        .expect("encode succeeds");
    let new_bytes = record_with_str(email_field, "x")
        .to_bytes()
        .expect("encode succeeds");
    let update_d = tbl
        .update_tx_bytes(rid_d, &old_bytes, new_bytes, &mut tx2, false)
        .await;
    assert!(
        matches!(
            update_d,
            Err(shamir_storage::error::DbError::DuplicateKey(_))
        ),
        "tx2's update_tx_bytes(D) must reject: its release plan for 'x' was \
         built against a stale snapshot (A), but the durable owner is now B, a \
         record tx2 never touched — admitting this would leave B and D \
         both live under a UNIQUE index; got {:?}",
        update_d
    );

    // B must still be the sole owner — no duplicate was created.
    let owner_after = tbl
        .index_manager()
        .lookup_by_unique_index(email_id, &[InnerValue::Str("x".into())])
        .await
        .unwrap();
    assert_eq!(
        owner_after,
        Some(rid_b),
        "B must remain the sole owner of 'x' after tx2's rejected update_tx_bytes"
    );
}

/// #1104 - stale-snapshot release via `insert_tx_many` must NOT bypass
/// a concurrent reclaim:
///
/// ```text
/// tx0: INSERT A {email:"x"}; COMMIT
/// tx2: BEGIN                                    // snapshot sees A owning "x"
/// tx1: DELETE A; INSERT B {email:"x"}; COMMIT   // durable owner is now B
/// tx2: DELETE A; insert_tx_many([C{email:"x"}]) // must now reject at stage time
/// ```
#[tokio::test]
async fn stale_snapshot_release_does_not_bypass_a_concurrent_reclaim_via_insert_tx_many() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));
    let tbl = repo.get_table("t").await.unwrap();
    tbl.create_unique_index("by_email", &["email"])
        .await
        .unwrap();
    let email_id = key_id(&tbl, "by_email").await;
    let email_field = key_id(&tbl, "email").await;

    // tx0: INSERT A {email:"x"}; COMMIT
    let (mut tx0, _g0) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let rid_a = tbl
        .insert_tx(&record_with_str(email_field, "x"), Some(&mut tx0))
        .await
        .expect("tx0 INSERT A must succeed");
    repo.commit_tx(tx0).await.expect("tx0 commit must succeed");

    // tx2: BEGIN — snapshot sees A owning "x".
    let (mut tx2, _g2) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();

    // tx1: DELETE A; INSERT B {email:"x"}; COMMIT — concurrent, commits FIRST.
    let (mut tx1, _g1) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    tbl.delete_tx(rid_a, Some(&mut tx1))
        .await
        .expect("tx1 DELETE A must succeed");
    let rid_b = tbl
        .insert_tx(&record_with_str(email_field, "x"), Some(&mut tx1))
        .await
        .expect("tx1 INSERT B must succeed");
    repo.commit_tx(tx1).await.expect("tx1 commit must succeed");

    // Durable owner of "x" is now B, not A.
    let owner = tbl
        .index_manager()
        .lookup_by_unique_index(email_id, &[InnerValue::Str("x".into())])
        .await
        .unwrap();
    assert_eq!(
        owner,
        Some(rid_b),
        "B must durably own 'x' after tx1 commits"
    );

    // tx2: DELETE A (stale — A is no longer the durable owner of "x", but
    // A itself is still present in tx2's own snapshot/write_set); then
    // insert_tx_many C with email="x" — the bypass, if present, would let
    // this stage (tolerating any release-marked key), silently heading
    // toward two live records under a UNIQUE index once tx2 commits.
    tbl.delete_tx(rid_a, Some(&mut tx2))
        .await
        .expect("tx2 DELETE A stages fine (A is stale but still exists)");
    let values = vec![record_with_str(email_field, "x")];
    let insert_c = tbl.insert_tx_many(&values, &mut tx2).await;
    assert!(
        matches!(
            insert_c,
            Err(shamir_storage::error::DbError::DuplicateKey(_))
        ),
        "tx2's insert_tx_many must reject: its release plan for 'x' was \
         built against a stale snapshot (A), but the durable owner is now B, a \
         record tx2 never touched — admitting this would leave B and C \
         both live under a UNIQUE index; got {:?}",
        insert_c
    );

    // B must still be the sole owner — no duplicate was created.
    let owner_after = tbl
        .index_manager()
        .lookup_by_unique_index(email_id, &[InnerValue::Str("x".into())])
        .await
        .unwrap();
    assert_eq!(
        owner_after,
        Some(rid_b),
        "B must remain the sole owner of 'x' after tx2's rejected insert_tx_many"
    );
}

/// #1104 - stale-snapshot release via `insert_tx_many_bytes` must NOT bypass
/// a concurrent reclaim:
///
/// ```text
/// tx0: INSERT A {email:"x"}; COMMIT
/// tx2: BEGIN                                    // snapshot sees A owning "x"
/// tx1: DELETE A; INSERT B {email:"x"}; COMMIT   // durable owner is now B
/// tx2: DELETE A; insert_tx_many_bytes([C{email:"x"}]) // must now reject at stage time
/// ```
#[tokio::test]
async fn stale_snapshot_release_does_not_bypass_a_concurrent_reclaim_via_insert_tx_many_bytes() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));
    let tbl = repo.get_table("t").await.unwrap();
    tbl.create_unique_index("by_email", &["email"])
        .await
        .unwrap();
    let email_id = key_id(&tbl, "by_email").await;
    let email_field = key_id(&tbl, "email").await;

    // tx0: INSERT A {email:"x"}; COMMIT
    let (mut tx0, _g0) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let rid_a = tbl
        .insert_tx(&record_with_str(email_field, "x"), Some(&mut tx0))
        .await
        .expect("tx0 INSERT A must succeed");
    repo.commit_tx(tx0).await.expect("tx0 commit must succeed");

    // tx2: BEGIN — snapshot sees A owning "x".
    let (mut tx2, _g2) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();

    // tx1: DELETE A; INSERT B {email:"x"}; COMMIT — concurrent, commits FIRST.
    let (mut tx1, _g1) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    tbl.delete_tx(rid_a, Some(&mut tx1))
        .await
        .expect("tx1 DELETE A must succeed");
    let rid_b = tbl
        .insert_tx(&record_with_str(email_field, "x"), Some(&mut tx1))
        .await
        .expect("tx1 INSERT B must succeed");
    repo.commit_tx(tx1).await.expect("tx1 commit must succeed");

    // Durable owner of "x" is now B, not A.
    let owner = tbl
        .index_manager()
        .lookup_by_unique_index(email_id, &[InnerValue::Str("x".into())])
        .await
        .unwrap();
    assert_eq!(
        owner,
        Some(rid_b),
        "B must durably own 'x' after tx1 commits"
    );

    // tx2: DELETE A (stale — A is no longer the durable owner of "x", but
    // A itself is still present in tx2's own snapshot/write_set); then
    // insert_tx_many_bytes C with email="x" — the bypass, if present, would
    // let this stage (tolerating any release-marked key), silently heading
    // toward two live records under a UNIQUE index once tx2 commits.
    tbl.delete_tx(rid_a, Some(&mut tx2))
        .await
        .expect("tx2 DELETE A stages fine (A is stale but still exists)");
    let c_bytes = record_with_str(email_field, "x")
        .to_bytes()
        .expect("encode succeeds");
    let insert_c = tbl.insert_tx_many_bytes(&[c_bytes], &mut tx2).await;
    assert!(
        matches!(
            insert_c,
            Err(shamir_storage::error::DbError::DuplicateKey(_))
        ),
        "tx2's insert_tx_many_bytes must reject: its release plan for 'x' was \
         built against a stale snapshot (A), but the durable owner is now B, a \
         record tx2 never touched — admitting this would leave B and C \
         both live under a UNIQUE index; got {:?}",
        insert_c
    );

    // B must still be the sole owner — no duplicate was created.
    let owner_after = tbl
        .index_manager()
        .lookup_by_unique_index(email_id, &[InnerValue::Str("x".into())])
        .await
        .unwrap();
    assert_eq!(
        owner_after,
        Some(rid_b),
        "B must remain the sole owner of 'x' after tx2's rejected insert_tx_many_bytes"
    );
}

/// #1104 - stale-snapshot release via `update_tx` must NOT bypass
/// a concurrent reclaim:
///
/// ```text
/// tx0: INSERT A {email:"x"}; COMMIT
/// tx2: BEGIN                                    // snapshot sees A owning "x"
/// tx1: DELETE A; INSERT B {email:"x"}; COMMIT   // durable owner is now B
/// tx2: DELETE A; UPDATE C SET email="x"         // must now reject at stage time
/// ```
#[tokio::test]
async fn stale_snapshot_release_does_not_bypass_a_concurrent_reclaim_via_update_tx() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));
    let tbl = repo.get_table("t").await.unwrap();
    tbl.create_unique_index("by_email", &["email"])
        .await
        .unwrap();
    let email_id = key_id(&tbl, "by_email").await;
    let email_field = key_id(&tbl, "email").await;

    // tx0: INSERT A {email:"x"}; COMMIT
    let (mut tx0, _g0) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let rid_a = tbl
        .insert_tx(&record_with_str(email_field, "x"), Some(&mut tx0))
        .await
        .expect("tx0 INSERT A must succeed");
    repo.commit_tx(tx0).await.expect("tx0 commit must succeed");

    // tx2: BEGIN — snapshot sees A owning "x".
    let (mut tx2, _g2) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();

    // tx1: DELETE A; INSERT B {email:"x"}; COMMIT — concurrent, commits FIRST.
    let (mut tx1, _g1) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    tbl.delete_tx(rid_a, Some(&mut tx1))
        .await
        .expect("tx1 DELETE A must succeed");
    let rid_b = tbl
        .insert_tx(&record_with_str(email_field, "x"), Some(&mut tx1))
        .await
        .expect("tx1 INSERT B must succeed");
    repo.commit_tx(tx1).await.expect("tx1 commit must succeed");

    // Durable owner of "x" is now B, not A.
    let owner = tbl
        .index_manager()
        .lookup_by_unique_index(email_id, &[InnerValue::Str("x".into())])
        .await
        .unwrap();
    assert_eq!(
        owner,
        Some(rid_b),
        "B must durably own 'x' after tx1 commits"
    );

    // tx2: DELETE A (stale — A is no longer the durable owner of "x", but
    // A itself is still present in tx2's own snapshot/write_set); then
    // insert a second durable record C with a different email.
    let (mut tx0b, _g0b) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let rid_c = tbl
        .insert_tx(&record_with_str(email_field, "other"), Some(&mut tx0b))
        .await
        .expect("INSERT C must succeed");
    repo.commit_tx(tx0b).await.expect("commit must succeed");

    // Now in tx2, try to UPDATE C to email="x" via update_tx — this would be
    // a bypass if allowed (tolerating any release-marked key), silently
    // heading toward two live records under a UNIQUE index once tx2 commits.
    tbl.delete_tx(rid_a, Some(&mut tx2))
        .await
        .expect("tx2 DELETE A stages fine (A is stale but still exists)");
    let update_c = tbl
        .update_tx(rid_c, &record_with_str(email_field, "x"), Some(&mut tx2))
        .await;
    assert!(
        matches!(
            update_c,
            Err(shamir_storage::error::DbError::DuplicateKey(_))
        ),
        "tx2's update_tx(C) must reject: its release plan for 'x' was \
         built against a stale snapshot (A), but the durable owner is now B, a \
         record tx2 never touched — admitting this would leave B and C \
         both live under a UNIQUE index; got {:?}",
        update_c
    );

    // B must still be the sole owner — no duplicate was created.
    let owner_after = tbl
        .index_manager()
        .lookup_by_unique_index(email_id, &[InnerValue::Str("x".into())])
        .await
        .unwrap();
    assert_eq!(
        owner_after,
        Some(rid_b),
        "B must remain the sole owner of 'x' after tx2's rejected update_tx"
    );
}
