//! Tests for `TableManager::read_one_tx` and `with_mvcc_store`.

use std::sync::Arc;

use shamir_storage::storage_in_memory::{InMemoryRepo, InMemoryStore};
use shamir_storage::types::Store;
use shamir_tx::{IsolationLevel, MvccStore, RepoTxGate, TxContext, TxId};
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;

use crate::repo::repo_instance::RepoInstance;
use crate::repo::repo_types::BoxRepo;
use crate::table::TableConfig;
use crate::table::TableManager;
use crate::tx::CommitError;

async fn make_table() -> TableManager {
    let data: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let info: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    TableManager::create("t".into(), data, info).await.unwrap()
}

fn make_tx(snapshot: u64) -> TxContext {
    TxContext::new(TxId::new(1), 0, snapshot, IsolationLevel::Snapshot)
}

#[tokio::test]
async fn read_one_tx_none_equals_get() {
    let tbl = make_table().await;
    let unknown_id = RecordId::new();

    let value = InnerValue::Str("v".to_string());
    let inserted_id = tbl.insert(&value).await.unwrap();

    let via_get = tbl.get(inserted_id).await.unwrap();
    let via_tx = tbl.read_one_tx(inserted_id, None).await.unwrap();
    assert_eq!(format!("{:?}", via_get), format!("{:?}", via_tx));

    let _ = tbl.get(unknown_id).await.unwrap_err();
    let _ = tbl.read_one_tx(unknown_id, None).await.unwrap_err();
}

#[tokio::test]
async fn read_one_tx_some_without_mvcc_falls_back_to_get() {
    let tbl = make_table().await;
    let value = InnerValue::Str("a".to_string());
    let id = tbl.insert(&value).await.unwrap();

    let tx = make_tx(100);
    let via = tbl.read_one_tx(id, Some(&tx)).await.unwrap();
    let direct = tbl.get(id).await.unwrap();
    assert_eq!(format!("{:?}", via), format!("{:?}", direct));
}

#[tokio::test]
async fn read_one_tx_routes_through_mvcc_when_attached() {
    let data: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let info: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let history: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let tbl = TableManager::create("t".into(), Arc::clone(&data), info)
        .await
        .unwrap();
    let gate = Arc::new(RepoTxGate::fresh());
    let mvcc = Arc::new(MvccStore::new(history, Arc::clone(&gate)));
    let tbl = tbl.with_mvcc_store(Arc::clone(&mvcc));

    let value = InnerValue::Str("x".to_string());
    let id = tbl.insert(&value).await.unwrap();

    let tx = make_tx(u64::MAX);
    let via_tx = tbl.read_one_tx(id, Some(&tx)).await.unwrap();
    let direct = tbl.get(id).await.unwrap();
    assert_eq!(format!("{:?}", via_tx), format!("{:?}", direct));
}

#[tokio::test]
async fn read_one_tx_with_mvcc_not_found_maps_to_error() {
    let data: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let info: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let history: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let tbl = TableManager::create("t".into(), Arc::clone(&data), info)
        .await
        .unwrap();
    let gate = Arc::new(RepoTxGate::fresh());
    let mvcc = Arc::new(MvccStore::new(history, Arc::clone(&gate)));
    let tbl = tbl.with_mvcc_store(mvcc);

    let id = RecordId::new();
    let tx = make_tx(100);
    let err = tbl.read_one_tx(id, Some(&tx)).await.unwrap_err();
    assert!(
        matches!(err, shamir_storage::error::DbError::NotFound(_)),
        "expected NotFound, got {:?}",
        err
    );
}

// ===========================================================================
// A3 (audit 2026-07-06-concurrency-engine.md) — SSI read-set must record the
// version of the value ACTUALLY READ, not the cell's current version.
//
// `read_one_tx` reads via `get_at(key, tx.snapshot_version)` (the snapshot-
// consistent value) but, before the fix, recorded `version_of(key)` (the
// cell's CURRENT version, which a concurrent committer may have just bumped
// past `tx.snapshot_version`). That mismatch let a Serializable tx commit
// having read stale data, with `validate_read_set` blind to the conflict
// (`current == version_seen`). The fix clamps the recorded version to
// `version_of(key).min(tx.snapshot_version)`.
//
// These tests exercise the REAL production path (`read_one_tx` →
// `record_read_shared` → `validate_read_set` at commit) — they do NOT
// manually call `record_read`, which is what every prior SSI test did and
// why the bug was masked (the manual recording used the CORRECT
// `tx.snapshot_version` semantics).
// ===========================================================================

fn make_repo() -> RepoInstance {
    let repo = Arc::new(InMemoryRepo::new());
    RepoInstance::new("test".into(), BoxRepo::InMemory(repo), Vec::new())
}

/// Reproduces the exact A3 interleaving: B (Serializable) reads a key whose
/// cell a concurrent committer A has already pushed past B's snapshot. B's
/// `read_one_tx` returns the stale (snapshot-consistent) value, but under the
/// bug records the cell's NEW current version → `validate_read_set` sees no
/// advance → B commits on stale data. After the fix, the recorded version is
/// clamped to `min(current, snapshot)`, so the post-snapshot commit by A is
/// detected and B's commit is rejected with `SsiConflict`.
#[tokio::test]
async fn a3_read_one_tx_records_snapshot_version_not_current_after_concurrent_commit() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));
    let tbl = repo.get_table("t").await.unwrap();

    // Pre-populate a record outside any transaction. This seeds the MVCC
    // cell and advances `last_committed` to some version V0.
    let rid = tbl.insert(&InnerValue::Str("v0".into())).await.unwrap();

    // B begins a Serializable tx → snapshot = V0 (the current committed
    // version). B has NOT read anything yet.
    let (mut tx_b, _gb) = repo
        .begin_tx(shamir_tx::IsolationLevel::Serializable)
        .await
        .unwrap();
    let snap_b = tx_b.snapshot_version;

    // A (also Serializable) begins, writes the SAME key, and commits —
    // publishing a new version V1 > V0 through the normal commit path
    // (realistic interleaving, not a raw MVCC poke).
    let (mut tx_a, _ga) = repo
        .begin_tx(shamir_tx::IsolationLevel::Serializable)
        .await
        .unwrap();
    tbl.update_tx(rid, &InnerValue::Str("v1".into()), Some(&mut tx_a))
        .await
        .unwrap();
    let out_a = repo.commit_tx(tx_a).await.unwrap();
    let v_a = out_a.commit_version;
    assert!(
        v_a > snap_b,
        "A's commit must advance the key past B's snapshot (v_a={}, snap_b={})",
        v_a,
        snap_b
    );

    // Now B reads the key via the production point-read path. The value
    // returned is the stale v0 (snapshot-gated to snap_b). Before the A3
    // fix, `read_one_tx` recorded the cell's NEW current version (v_a) —
    // masking the conflict at commit. After the fix, the recorded version
    // is clamped to `min(v_a, snap_b) = snap_b`, so the post-snapshot
    // commit by A is detectable.
    let val = tbl.read_one_tx(rid, Some(&tx_b)).await.unwrap();
    assert!(
        matches!(val, InnerValue::Str(ref s) if s == "v0"),
        "B must read its snapshot-consistent value v0, got {:?}",
        val
    );

    // B stages a write to the same key (so it is not a read-only fast-path)
    // and attempts to commit.
    tbl.update_tx(rid, &InnerValue::Str("v_b".into()), Some(&mut tx_b))
        .await
        .unwrap();
    let result = repo.commit_tx(tx_b).await;

    // After the fix: B read stale data (v0) after A already published v_a,
    // so first-committer-wins must reject B with SsiConflict. Before the
    // fix: B committed successfully (the bug — `validate_read_set` saw
    // `current == version_seen` because the recorded version was the
    // cell's current, not the snapshot, version).
    match result {
        Err(CommitError::SsiConflict { .. }) => {}
        other => panic!(
            "B must abort with SsiConflict (A committed a newer version of \
             the key B read staledly); got {:?}",
            other.map(|o| o.commit_version).map_err(|_| "Err(other)")
        ),
    }
}

/// Same interleaving under Snapshot isolation: B must commit (Snapshot does
/// not detect stale reads). This pins that the clamp does not regress the
/// Snapshot path (where `record_read_shared` is a no-op).
#[tokio::test]
async fn a3_read_one_tx_snapshot_isolation_commits_on_same_interleaving() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));
    let tbl = repo.get_table("t").await.unwrap();
    let rid = tbl.insert(&InnerValue::Str("v0".into())).await.unwrap();

    let (mut tx_b, _gb) = repo
        .begin_tx(shamir_tx::IsolationLevel::Snapshot)
        .await
        .unwrap();

    let (mut tx_a, _ga) = repo
        .begin_tx(shamir_tx::IsolationLevel::Serializable)
        .await
        .unwrap();
    tbl.update_tx(rid, &InnerValue::Str("v1".into()), Some(&mut tx_a))
        .await
        .unwrap();
    repo.commit_tx(tx_a).await.unwrap();

    let val = tbl.read_one_tx(rid, Some(&tx_b)).await.unwrap();
    assert!(
        matches!(val, InnerValue::Str(ref s) if s == "v0"),
        "Snapshot tx B reads its snapshot value v0, got {:?}",
        val
    );

    tbl.update_tx(rid, &InnerValue::Str("v_b".into()), Some(&mut tx_b))
        .await
        .unwrap();
    // Snapshot isolation: no SSI read-set validation → B commits (last
    // writer wins). This must remain true after the clamp fix.
    let out = repo.commit_tx(tx_b).await.unwrap();
    assert!(out.commit_version > 0);
}
