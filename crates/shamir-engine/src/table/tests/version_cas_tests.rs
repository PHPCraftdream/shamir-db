//! FG-2: `with_version` + `expected_version` CAS contour tests.
//!
//! Tests the full optimistic-concurrency pipeline:
//!   1. Read-side `with_version` returns per-record versions.
//!   2. `expected_version` matching succeeds; stale version fails with
//!      `version_conflict` and no row is modified.
//!   3. Zero-match `expected_version` is a no-op (affected: 0).
//!   4. CONCURRENT CAS: two real concurrent writers racing the same
//!      `expected_version` — exactly one succeeds, the other fails, and a
//!      retry with the fresh version succeeds.
//!   5. `versions` is `None` when `with_version` is not requested.
//!
//! Uses `RepoInstance` with real `begin_tx`/`commit_tx` so the CAS check
//! (immediate `version_of` + SSI `validate_read_set`) fires through the
//! actual commit pipeline — mirrors the `ssi_stress_tests` harness.

use std::sync::Arc;

use serial_test::serial;
use shamir_query_builder::{filter, write, Query};
use shamir_storage::error::DbError;
use shamir_storage::storage_in_memory::InMemoryRepo;
use shamir_tx::IsolationLevel;
use shamir_types::mpack;
use shamir_types::types::record_id::RecordId;

use crate::query::filter::eval_context::FilterContext;
use crate::repo::repo_instance::RepoInstance;
use crate::repo::repo_types::BoxRepo;
use crate::table::TableConfig;
use crate::table::TableManager;
use crate::tx::CommitError;

fn make_repo() -> RepoInstance {
    let repo = Arc::new(InMemoryRepo::new());
    RepoInstance::new("test".into(), BoxRepo::InMemory(repo), Vec::new())
}

/// Insert a record and return the RecordId.
async fn insert_row(tbl: &TableManager, name: &str) -> RecordId {
    use shamir_types::types::common::new_map;
    use shamir_types::types::value::InnerValue;

    let interner = tbl.interner().get().await.unwrap();
    let key = interner.touch_ind("name").unwrap().into_key();
    let _ = interner;

    let mut m = new_map();
    m.insert(key, InnerValue::Str(name.into()));
    tbl.insert(&InnerValue::Map(m)).await.unwrap()
}

/// Commit a single-record update through the full tx pipeline so the MVCC
/// version actually bumps. Returns the new committed version.
async fn commit_update(repo: &RepoInstance, tbl: &TableManager, name: &str, new_name: &str) -> u64 {
    let (mut tx, _g) = repo.begin_tx(IsolationLevel::Serializable).await.unwrap();
    let interner = tbl.interner().get().await.unwrap();
    let refs = shamir_types::types::common::new_map();
    let ctx = FilterContext::new(interner, &refs);
    let mut m = shamir_types::types::common::new_map_wc(1);
    m.insert(
        "name".to_string(),
        shamir_types::types::value::QueryValue::Str(new_name.to_string()),
    );
    let op = write::update("t")
        .where_(filter::eq("name", name))
        .set(shamir_types::types::value::QueryValue::Map(m))
        .build();
    tbl.execute_update_tx(
        &op,
        &ctx,
        &mut tx,
        None,
        &shamir_types::access::Actor::System,
    )
    .await
    .unwrap();
    let outcome = repo.commit_tx(tx).await.unwrap();
    outcome.commit_version
}

// ── Test 1: read-side with_version returns versions ───────────────────────

#[tokio::test]
async fn with_version_returns_per_record_versions() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));
    let tbl = repo.get_table("t").await.unwrap();

    let _rid1 = insert_row(&tbl, "alice").await;
    let _rid2 = insert_row(&tbl, "bob").await;

    let interner = tbl.interner().get().await.unwrap();
    let refs = shamir_types::types::common::new_map();
    let ctx = FilterContext::new(interner, &refs);

    // Without with_version → versions is None.
    let q_no = Query::from("t").build();
    let r_no = tbl.read(&q_no, &ctx).await.unwrap();
    assert!(
        r_no.versions.is_none(),
        "versions must be None when not requested"
    );

    // With with_version → versions is Some and aligned.
    let q_yes = Query::from("t").with_version().build();
    let r_yes = tbl.read(&q_yes, &ctx).await.unwrap();
    let versions = r_yes
        .versions
        .as_ref()
        .expect("versions must be Some when with_version");
    assert_eq!(
        versions.len(),
        r_yes.records.len(),
        "versions must be index-aligned with records"
    );
}

// ── Test 1b: version increases after update ────────────────────────────────

#[tokio::test]
async fn version_increases_after_update() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));
    let tbl = repo.get_table("t").await.unwrap();

    let _rid = insert_row(&tbl, "alice").await;

    let interner = tbl.interner().get().await.unwrap();
    let refs = shamir_types::types::common::new_map();
    let ctx = FilterContext::new(interner, &refs);

    let q1 = Query::from("t").with_version().build();
    let r1 = tbl.read(&q1, &ctx).await.unwrap();
    let before = r1.versions.as_ref().unwrap()[0];

    // Commit an update through the real tx pipeline.
    commit_update(&repo, &tbl, "alice", "alice2").await;

    let q2 = Query::from("t").with_version().build();
    let r2 = tbl.read(&q2, &ctx).await.unwrap();
    let after = r2.versions.as_ref().unwrap()[0];

    assert!(
        after > before,
        "version must increase after update: {before} -> {after}"
    );
}

// ── Test 2: expected_version matching succeeds; stale fails ────────────────

#[tokio::test]
async fn expected_version_matching_succeeds_stale_fails() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));
    let tbl = repo.get_table("t").await.unwrap();
    let _rid = insert_row(&tbl, "alice").await;

    let interner = tbl.interner().get().await.unwrap();
    let refs = shamir_types::types::common::new_map();
    let ctx = FilterContext::new(interner, &refs);

    // Read the current version.
    let rq = Query::from("t").with_version().build();
    let rr = tbl.read(&rq, &ctx).await.unwrap();
    let v1 = rr.versions.as_ref().unwrap()[0];

    // Commit an update to bump the version.
    commit_update(&repo, &tbl, "alice", "alice2").await;

    // Now try with the STALE v1 → the immediate check must reject it.
    let op_stale = write::update("t")
        .where_(filter::eq("name", "alice2"))
        .set(mpack!({ "name": "alice3" }))
        .expected_version(v1)
        .build();
    let (mut tx, _g) = repo.begin_tx(IsolationLevel::Serializable).await.unwrap();
    let result = tbl
        .execute_update_tx(
            &op_stale,
            &ctx,
            &mut tx,
            None,
            &shamir_types::access::Actor::System,
        )
        .await;
    assert!(
        result.is_err(),
        "stale expected_version must fail at staging"
    );
    let err = result.unwrap_err();
    assert!(
        matches!(err, DbError::VersionConflict(_)),
        "must be VersionConflict, got: {err}"
    );

    // Row is UNCHANGED after the rejected attempt.
    let read_q = Query::from("t")
        .where_(filter::eq("name", "alice2"))
        .build();
    let read_result = tbl.read(&read_q, &ctx).await.unwrap();
    assert_eq!(
        read_result.records.len(),
        1,
        "row must still be 'alice2' (unchanged after rejected CAS)"
    );

    // Correct (fresh) version succeeds.
    let fresh_v = tbl.read(&rq, &ctx).await.unwrap().versions.unwrap()[0];
    let op_ok = write::update("t")
        .where_(filter::eq("name", "alice2"))
        .set(mpack!({ "name": "alice3" }))
        .expected_version(fresh_v)
        .build();
    let (mut tx2, _g2) = repo.begin_tx(IsolationLevel::Serializable).await.unwrap();
    let result_ok = tbl
        .execute_update_tx(
            &op_ok,
            &ctx,
            &mut tx2,
            None,
            &shamir_types::access::Actor::System,
        )
        .await;
    assert!(result_ok.is_ok(), "matching expected_version must succeed");
    assert_eq!(result_ok.unwrap().affected, 1);
}

// ── Test 3: zero-match expected_version is a no-op ─────────────────────────

#[tokio::test]
async fn expected_version_zero_match_is_noop() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));
    let tbl = repo.get_table("t").await.unwrap();

    let interner = tbl.interner().get().await.unwrap();
    let refs = shamir_types::types::common::new_map();
    let ctx = FilterContext::new(interner, &refs);

    let op = write::update("t")
        .where_(filter::eq("name", "nonexistent"))
        .set(mpack!({ "name": "x" }))
        .expected_version(42)
        .build();
    let (mut tx, _g) = repo.begin_tx(IsolationLevel::Serializable).await.unwrap();
    let result = tbl
        .execute_update_tx(
            &op,
            &ctx,
            &mut tx,
            None,
            &shamir_types::access::Actor::System,
        )
        .await
        .unwrap();
    assert_eq!(result.affected, 0, "zero-match must be a no-op");
}

// ── Test 4: CONCURRENT CAS — mandatory ─────────────────────────────────────
//
// Two real concurrent Serializable txs: both read the same row at version v1,
// both call `execute_update_tx` with `expected_version(v1)`. Both pass the
// immediate check (version_of == v1 at staging time). Both try to COMMIT.
// The commit pipeline's `validate_read_set` catches the loser: the first
// committer bumps the version, the second's recorded read (v1) is now stale
// → SsiConflict abort.
//
// Exactly one commit succeeds. The loser must fail with a tx-conflict class
// error. A retry with the fresh version must then succeed.

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[serial]
async fn concurrent_cas_exactly_one_wins() {
    use shamir_types::types::common::new_map_wc;
    use shamir_types::types::value::QueryValue;

    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));
    let tbl = repo.get_table("t").await.unwrap();
    let rid = insert_row(&tbl, "counter").await;

    // Read the initial version via the with_version read path.
    let interner = tbl.interner().get().await.unwrap();
    let refs = shamir_types::types::common::new_map();
    let ctx = FilterContext::new(interner, &refs);
    let vq = Query::from("t").with_version().build();
    let vr = tbl.read(&vq, &ctx).await.unwrap();
    let v1 = vr.versions.as_ref().unwrap()[0];

    let make_op = move |new_val: String| {
        let mut m = new_map_wc(1);
        m.insert("name".to_string(), QueryValue::Str(new_val));
        write::update("t")
            .where_(filter::eq("name", "counter"))
            .set(QueryValue::Map(m))
            .expected_version(v1)
            .build()
    };

    let repo_a = repo.clone();
    let tbl_a = tbl.clone();
    let repo_b = repo.clone();
    let tbl_b = tbl.clone();

    let task_a = tokio::spawn(async move {
        let (mut tx, _g) = repo_a.begin_tx(IsolationLevel::Serializable).await.unwrap();
        let interner = tbl_a.interner().get().await.unwrap();
        let refs = shamir_types::types::common::new_map();
        let ctx = FilterContext::new(interner, &refs);
        tbl_a
            .execute_update_tx(
                &make_op("writer_a".to_string()),
                &ctx,
                &mut tx,
                None,
                &shamir_types::access::Actor::System,
            )
            .await
            .unwrap(); // both pass the immediate check
        repo_a.commit_tx(tx).await
    });

    let task_b = tokio::spawn(async move {
        let (mut tx, _g) = repo_b.begin_tx(IsolationLevel::Serializable).await.unwrap();
        let interner = tbl_b.interner().get().await.unwrap();
        let refs = shamir_types::types::common::new_map();
        let ctx = FilterContext::new(interner, &refs);
        tbl_b
            .execute_update_tx(
                &make_op("writer_b".to_string()),
                &ctx,
                &mut tx,
                None,
                &shamir_types::access::Actor::System,
            )
            .await
            .unwrap(); // both pass the immediate check
        repo_b.commit_tx(tx).await
    });

    let (res_a, res_b) = tokio::join!(task_a, task_b);
    let res_a = res_a.unwrap();
    let res_b = res_b.unwrap();

    let a_ok = res_a.is_ok();
    let b_ok = res_b.is_ok();
    let a_conflict = matches!(res_a, Err(CommitError::SsiConflict { .. }))
        || matches!(res_a, Err(CommitError::PhantomConflict { .. }));
    let b_conflict = matches!(res_b, Err(CommitError::SsiConflict { .. }))
        || matches!(res_b, Err(CommitError::PhantomConflict { .. }));

    // Exactly one must succeed; the other must abort with a conflict.
    assert!(
        (a_ok && b_conflict) || (b_ok && a_conflict),
        "expected exactly one commit success and one conflict abort, got: \
         a_ok={a_ok} b_ok={b_ok} a_conflict={a_conflict} b_conflict={b_conflict}"
    );

    // Retry with the fresh version must succeed.
    let fresh_v = tbl.read(&vq, &ctx).await.unwrap().versions.unwrap()[0];
    assert!(
        fresh_v > v1,
        "version must have advanced after the winning commit: {v1} -> {fresh_v}"
    );

    let retry_op = write::update("t")
        .where_(filter::eq("name", "counter"))
        .set(mpack!({ "name": "retry_succeeded" }))
        .expected_version(fresh_v)
        .build();
    let (mut tx3, _g3) = repo.begin_tx(IsolationLevel::Serializable).await.unwrap();
    tbl.execute_update_tx(
        &retry_op,
        &ctx,
        &mut tx3,
        None,
        &shamir_types::access::Actor::System,
    )
    .await
    .unwrap();
    let retry = repo.commit_tx(tx3).await;
    assert!(
        retry.is_ok(),
        "retry with fresh version must succeed: {:?}",
        retry.err()
    );

    // Silence unused warning.
    let _ = rid;
}
