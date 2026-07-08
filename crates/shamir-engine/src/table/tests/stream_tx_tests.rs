//! Tests for tx-aware streaming wrappers on TableManager (Stage 3.2.B).
//!
//! At this stage the `*_tx` methods forward to their non-tx counterparts
//! regardless of `tx`. These tests pin that contract so future wiring
//! (3.2.B.2 / 3.3) can be sanity-checked without surprises.

use std::sync::Arc;

use futures::StreamExt;
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;
use shamir_tx::{IsolationLevel, TxContext, TxId};
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;

use crate::table::record_cow::RecordCow;
use crate::table::TableManager;

async fn make_table_with_n_records(n: usize) -> (TableManager, Vec<RecordId>) {
    let data: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let info: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let tbl = TableManager::create("t".into(), data, info).await.unwrap();
    let mut ids = Vec::with_capacity(n);
    for i in 0..n {
        let id = tbl.insert(&InnerValue::Str(format!("v{i}"))).await.unwrap();
        ids.push(id);
    }
    (tbl, ids)
}

fn make_tx(snapshot: u64) -> TxContext {
    TxContext::new(TxId::new(1), 0, snapshot, IsolationLevel::Snapshot)
}

async fn collect_stream<S>(stream: S) -> Vec<(RecordId, InnerValue)>
where
    S: futures::Stream<Item = shamir_storage::error::DbResult<Vec<(RecordId, RecordCow)>>>,
{
    futures::pin_mut!(stream);
    let mut out = Vec::new();
    while let Some(batch) = stream.next().await {
        for (id, cow) in batch.unwrap() {
            out.push((id, cow.into_inner().unwrap()));
        }
    }
    out
}

#[tokio::test]
async fn list_stream_tx_none_matches_list_stream() {
    let (tbl, _ids) = make_table_with_n_records(5).await;

    let baseline = collect_stream(tbl.list_stream(2)).await;
    let via_tx_none = collect_stream(tbl.list_stream_tx(None, 2)).await;
    assert_eq!(baseline.len(), via_tx_none.len());
    assert_eq!(baseline.len(), 5);
}

#[tokio::test]
async fn list_stream_tx_some_matches_list_stream_forward() {
    let (tbl, _ids) = make_table_with_n_records(3).await;
    let tx = make_tx(123);

    let baseline = collect_stream(tbl.list_stream(2)).await;
    let via_tx_some = collect_stream(tbl.list_stream_tx(Some(&tx), 2)).await;
    assert_eq!(baseline.len(), via_tx_some.len());
    assert_eq!(baseline.len(), 3);
}

/// KNOWN LIMITATION guard (C8): streaming scans do NOT overlay the tx's own
/// `write_set`, so a record staged inside a tx is INVISIBLE to an in-tx
/// stream until commit (only point reads — `read_one_tx` — do
/// read-your-own-writes). This test stages an insert and asserts the staged
/// record is ABSENT from `list_stream_tx(Some(&tx))`, pinning the *current*
/// documented behaviour.
///
/// When streaming read-your-own-writes is implemented, this test must FLIP:
/// the staged record becomes expected-present (and `list_stream_tx` should
/// then yield `n + 1` records). The assertions below will fail at that point,
/// forcing a deliberate update rather than letting the new behaviour ship as
/// a silent regression.
#[tokio::test]
async fn list_stream_tx_does_not_see_staged_insert() {
    let (tbl, ids) = make_table_with_n_records(3).await;
    let mut tx = make_tx(123);

    // Stage an insert inside the tx (populates tx.write_set for this table).
    let staged = tbl
        .insert_tx(&InnerValue::Str("staged-only".into()), Some(&mut tx))
        .await
        .unwrap();

    // The streamed scan forwards to the committed data store and does NOT
    // overlay staging, so it yields exactly the pre-staged committed set.
    let streamed = collect_stream(tbl.list_stream_tx(Some(&tx), 2)).await;

    assert_eq!(
        streamed.len(),
        ids.len(),
        "streaming scan must yield only committed records (no read-your-own-writes)"
    );
    assert!(
        streamed.iter().all(|(rid, _)| *rid != staged),
        "the staged-but-uncommitted insert must be ABSENT from the in-tx stream \
         (current limitation: scans do not overlay tx.write_set)"
    );
    // Cross-check via the point-read path, which DOES overlay staging: the
    // same record is visible there (read-your-own-writes), proving the
    // divergence is the streaming path's limitation, not a lost write.
    assert!(
        tbl.read_one_tx(staged, Some(&tx)).await.is_ok(),
        "read_one_tx must see the tx's own staged insert (point-read RYOW holds)"
    );
}

// ===========================================================================
// A3 (audit 2026-07-06-concurrency-engine.md) — scan path.
//
// `record_scan_reads` wraps every Serializable-scan batch and records each
// yielded key's version into the tx read-set. Before the fix it recorded
// `version_of(key)` (the cell's CURRENT version), which — just like the
// point-read path (`read_one_tx`) — can be strictly newer than
// `tx.snapshot_version` when a concurrent committer has published a newer
// version. That mismatch let a Serializable scan-based tx commit on stale
// data with no detected conflict. The fix clamps the recorded version to
// `min(version_of(key), tx.snapshot_version)`.
//
// This test exercises the REAL scan path (`list_stream_tx` →
// `record_scan_reads` → `validate_read_set` at commit); it does NOT
// manually call `record_read`.
// ===========================================================================

#[tokio::test]
async fn a3_record_scan_reads_records_snapshot_version_not_current_after_concurrent_commit() {
    use shamir_storage::storage_in_memory::InMemoryRepo;

    use crate::repo::repo_instance::RepoInstance;
    use crate::repo::repo_types::BoxRepo;
    use crate::table::TableConfig;
    use crate::tx::CommitError;

    let repo = Arc::new(InMemoryRepo::new());
    let repo = RepoInstance::new("test".into(), BoxRepo::InMemory(repo), Vec::new());
    repo.add_table(TableConfig::new("t"));
    let tbl = repo.get_table("t").await.unwrap();

    // Pre-populate a record outside any tx — seeds the MVCC cell at V0.
    let rid = tbl.insert(&InnerValue::Str("v0".into())).await.unwrap();

    // B begins a Serializable tx → snapshot = V0.
    let (mut tx_b, _gb) = repo
        .begin_tx(shamir_tx::IsolationLevel::Serializable)
        .await
        .unwrap();
    let snap_b = tx_b.snapshot_version;

    // A (Serializable) writes the SAME key and commits → publishes V1 > V0.
    let (mut tx_a, _ga) = repo
        .begin_tx(shamir_tx::IsolationLevel::Serializable)
        .await
        .unwrap();
    tbl.update_tx(rid, &InnerValue::Str("v1".into()), Some(&mut tx_a))
        .await
        .unwrap();
    let out_a = repo.commit_tx(tx_a).await.unwrap();
    assert!(
        out_a.commit_version > snap_b,
        "A's commit must advance the key past B's snapshot"
    );

    // B scans the table via the production Serializable-scan path. Each
    // yielded record is recorded into B's read-set by `record_scan_reads`.
    // The streaming scan reads the committed/current store (NOT snapshot-
    // gated like `read_one_tx`), so B observes A's v1 — but B's tx
    // snapshot is still V0. Before the fix, `record_scan_reads` recorded
    // the cell's current version (V1); at commit `validate_read_set` saw
    // `current == version_seen` → no conflict → B committed having
    // observed data past its own snapshot. After the fix, the recorded
    // version is clamped to `min(V1, snap_b) = snap_b`, so
    // `validate_read_set` sees `current(V1) > snap_b` → SsiConflict.
    let streamed = collect_stream(tbl.list_stream_tx(Some(&tx_b), 10)).await;
    assert!(
        streamed
            .iter()
            .any(|(r, v)| { *r == rid && matches!(v, InnerValue::Str(ref s) if s == "v1") }),
        "B's scan yields the current committed value v1 for rid {:?}, got {:?}",
        rid,
        streamed
    );

    // B stages a write (not a read-only fast-path) and commits.
    tbl.update_tx(rid, &InnerValue::Str("v_b".into()), Some(&mut tx_b))
        .await
        .unwrap();
    let result = repo.commit_tx(tx_b).await;

    // After the fix: the scan read stale data after A published V1, so B
    // must abort with SsiConflict. Before the fix: B committed (the bug).
    match result {
        Err(CommitError::SsiConflict { .. }) => {}
        other => panic!(
            "B must abort with SsiConflict (A committed a newer version of \
             a key B scanned staledly); got {:?}",
            other.map(|o| o.commit_version).map_err(|_| "Err(other)")
        ),
    }
}
