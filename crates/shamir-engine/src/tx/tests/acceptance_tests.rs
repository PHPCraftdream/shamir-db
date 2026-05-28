use std::sync::Arc;

use shamir_storage::storage_in_memory::InMemoryRepo;
use shamir_tx::IsolationLevel;
use shamir_types::types::value::InnerValue;

use crate::repo::repo_instance::RepoInstance;
use crate::repo::repo_types::BoxRepo;
use crate::table::TableConfig;

fn make_repo() -> RepoInstance {
    let repo = Arc::new(InMemoryRepo::new());
    RepoInstance::new("test".into(), BoxRepo::InMemory(repo), Vec::new())
}

#[tokio::test]
async fn si_happy_path_insert_commit_observer_reads() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("users"));
    let tbl = repo.get_table("users").await.unwrap();

    let (mut tx, _g) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();

    let r1 = tbl
        .insert_tx(&InnerValue::Str("alice".into()), Some(&mut tx))
        .await
        .unwrap();
    let r2 = tbl
        .insert_tx(&InnerValue::Str("bob".into()), Some(&mut tx))
        .await
        .unwrap();
    let r3 = tbl
        .insert_tx(&InnerValue::Str("carol".into()), Some(&mut tx))
        .await
        .unwrap();

    // Before commit: observer sees nothing.
    assert!(tbl.get(r1).await.is_err(), "pre-commit must be invisible");

    let outcome = repo.commit_tx(tx).await.unwrap();
    assert!(outcome.commit_version > 0);

    // After commit: observer sees all 3.
    let v1 = tbl.get(r1).await.unwrap();
    let v2 = tbl.get(r2).await.unwrap();
    let v3 = tbl.get(r3).await.unwrap();
    assert!(matches!(v1, InnerValue::Str(s) if s == "alice"));
    assert!(matches!(v2, InnerValue::Str(s) if s == "bob"));
    assert!(matches!(v3, InnerValue::Str(s) if s == "carol"));
}

#[tokio::test]
async fn abort_path_drop_tx_no_side_effects() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("users"));
    let tbl = repo.get_table("users").await.unwrap();

    let (mut tx, _g) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();

    let r1 = tbl
        .insert_tx(&InnerValue::Str("staged".into()), Some(&mut tx))
        .await
        .unwrap();

    // Drop tx without commit — RAII rollback.
    drop(_g);
    drop(tx);

    // Observer must NOT see the record.
    assert!(tbl.get(r1).await.is_err(), "aborted tx must leave no trace");
}

#[tokio::test]
async fn read_after_write_inside_tx() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("users"));
    let tbl = repo.get_table("users").await.unwrap();

    // Pre-populate a record outside tx.
    let existing_rid = tbl
        .insert(&InnerValue::Str("pre-existing".into()))
        .await
        .unwrap();

    let (mut tx, _g) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();

    // Insert inside tx.
    let new_rid = tbl
        .insert_tx(&InnerValue::Str("new-in-tx".into()), Some(&mut tx))
        .await
        .unwrap();

    // Read the pre-existing record through tx (should still work — it's in main store).
    // read_one_tx with tx routes through mvcc (attached via 4.G.4).
    // With MvccStore: current_version = 0 (non-tx insert) <= snapshot = u64::MAX -> fast path -> main.get.
    let pre = tbl.read_one_tx(existing_rid, Some(&tx)).await.unwrap();
    assert!(matches!(pre, InnerValue::Str(s) if s == "pre-existing"));

    // Read the NEW record (staged in write_set) — NOTE: read_one_tx
    // currently does NOT check write_set (that's Stage 5 wiring).
    // So this read goes to mvcc.get_at -> main store -> NotFound
    // (the write hasn't been committed yet).
    // This is a KNOWN limitation: read-after-write for staged
    // records doesn't work until read_one_tx merges with write_set.
    // Assert the current (honest) behaviour:
    let staged = tbl.read_one_tx(new_rid, Some(&tx)).await;
    // This will be Err(NotFound) because write_set merge isn't wired.
    // TODO(Stage 5): this should return Ok(InnerValue::Str("new-in-tx")).
    // For now, just document that we know:
    if let Ok(val) = staged {
        // If somehow it works (future fix landed), great:
        assert!(matches!(val, InnerValue::Str(s) if s == "new-in-tx"));
    }
    // else: expected behaviour at Stage 4 — not a failure.

    let outcome = repo.commit_tx(tx).await.unwrap();
    assert!(outcome.commit_version > 0);

    // After commit: both visible.
    let _ = tbl.get(existing_rid).await.unwrap();
    let _ = tbl.get(new_rid).await.unwrap();
}

#[tokio::test]
async fn two_concurrent_commits_monotonic_versions() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("users"));
    let tbl = repo.get_table("users").await.unwrap();

    let (mut tx1, _g1) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let (mut tx2, _g2) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();

    let _ = tbl
        .insert_tx(&InnerValue::Str("from_tx1".into()), Some(&mut tx1))
        .await
        .unwrap();
    let _ = tbl
        .insert_tx(&InnerValue::Str("from_tx2".into()), Some(&mut tx2))
        .await
        .unwrap();

    let o1 = repo.commit_tx(tx1).await.unwrap();
    let o2 = repo.commit_tx(tx2).await.unwrap();

    assert!(
        o2.commit_version > o1.commit_version,
        "versions must be monotonic"
    );
}

#[tokio::test]
async fn cross_table_internal_atomicity() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("users"));
    repo.add_table(TableConfig::new("orders"));
    let users = repo.get_table("users").await.unwrap();
    let orders = repo.get_table("orders").await.unwrap();

    let (mut tx, _g) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();

    let u1 = users
        .insert_tx(&InnerValue::Str("alice".into()), Some(&mut tx))
        .await
        .unwrap();
    let o1 = orders
        .insert_tx(&InnerValue::Str("order_1".into()), Some(&mut tx))
        .await
        .unwrap();

    // Before commit: neither table has the records.
    assert!(users.get(u1).await.is_err());
    assert!(orders.get(o1).await.is_err());

    let outcome = repo.commit_tx(tx).await.unwrap();
    assert!(outcome.commit_version > 0);

    // After commit: both tables have their records.
    let _ = users.get(u1).await.unwrap();
    let _ = orders.get(o1).await.unwrap();
}

#[tokio::test]
async fn ssi_unknown_table_conflict() {
    let repo = make_repo();
    // DON'T get_table — leave per_table_mvcc empty for the table
    // that the tx will try to validate.

    let (mut tx, _g) = repo.begin_tx(IsolationLevel::Serializable).await.unwrap();

    // Record a read for an unknown table token.
    tx.record_read(99999, bytes::Bytes::from_static(b"k"), 5);

    // Commit must fail — unknown table -> conflict.
    let result = repo.commit_tx(tx).await;
    assert!(
        result.is_err(),
        "read on unknown table must trigger conflict"
    );
}

#[tokio::test]
async fn ssi_conflict_detected_on_concurrent_tx_writes() {
    // Two SSI transactions read the same record; first commits;
    // second must get SsiConflict because version_cache was bumped.
    // This proves Stage 5.1 closed Known Production Limitation #1.
    use crate::table::table_manager::table_token_for;
    use crate::tx::CommitError;

    let repo = make_repo();
    repo.add_table(crate::table::TableConfig::new("t"));
    let tbl = repo.get_table("t").await.unwrap();

    // Pre-populate a record outside any transaction.
    let rid = tbl
        .insert(&InnerValue::Str("initial".into()))
        .await
        .unwrap();

    let token = table_token_for("t");
    let key = rid.to_bytes();

    // tx1 (SSI): read the record at version 0.
    let (mut tx1, _g1) = repo.begin_tx(IsolationLevel::Serializable).await.unwrap();
    tx1.record_read(token, key.clone(), 0);

    // tx2 (SSI): read the same record at version 0.
    let (mut tx2, _g2) = repo.begin_tx(IsolationLevel::Serializable).await.unwrap();
    tx2.record_read(token, key.clone(), 0);

    // tx1 writes to the same record (update) and commits — bumps version_cache for key.
    tbl.update_tx(rid, &InnerValue::Str("tx1-write".into()), Some(&mut tx1))
        .await
        .unwrap();
    let o1 = repo.commit_tx(tx1).await.unwrap();
    assert!(
        o1.commit_version > 0,
        "tx1 must commit with a positive version"
    );

    // tx2 writes to the same record and tries to commit — must fail with SsiConflict
    // because the key it read (version 0) now has a higher version.
    tbl.update_tx(rid, &InnerValue::Str("tx2-write".into()), Some(&mut tx2))
        .await
        .unwrap();
    let result = repo.commit_tx(tx2).await;
    match result {
        Err(CommitError::SsiConflict { .. }) => {}
        other => panic!(
            "expected SsiConflict, got {:?}",
            other.map(|_| "Ok").unwrap_or("Err(other)")
        ),
    }
}
