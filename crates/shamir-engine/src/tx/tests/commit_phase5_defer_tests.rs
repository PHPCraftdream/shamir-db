//! Vector I.3 / MED-A: the WAL entry (Phase 4 `wal.begin`) is the commit
//! point. After it succeeds the tx is COMMITTED; the data/index/HNSW
//! projections (Phases 5a–5d) are eager materializations that may be
//! deferred to recovery on failure — they must NEVER turn a committed tx
//! into an abort.
//!
//! These tests pin the boundary from both sides:
//!
//!  * `post_commit_phase5_failure_is_committed_then_recovered` — inject a
//!    Phase 5c (index apply) failure AFTER the Phase 5a data write. The
//!    tx must report `Ok(MaterializationState::Deferred)` (NOT `Err`),
//!    leave its WAL marker inflight (Phase 7 skipped), and have recovery
//!    materialize the index posting + data. This FAILS against the old
//!    code, which propagated the Phase 5c `Err` as an abort.
//!
//!  * `pre_commit_failure_aborts_and_leaves_nothing` — a PRE-commit-point
//!    failure (SSI conflict) still returns `Err` (abort) and leaves no
//!    durable WAL marker. The commit point works both ways.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use bytes::Bytes;
use shamir_storage::storage_in_memory::InMemoryRepo;
use shamir_tx::{IndexWriteOp, IsolationLevel, StagingStore, TxContext, TxId, VersionProvider};
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;

use crate::repo::repo_instance::RepoInstance;
use crate::repo::repo_types::BoxRepo;
use crate::table::table_manager::table_token_for;
use crate::table::TableConfig;
use crate::tx::commit::{MaterializationState, FAIL_PHASE_5C_TX_ID};

fn make_repo() -> RepoInstance {
    let repo = Arc::new(InMemoryRepo::new());
    RepoInstance::new("test".into(), BoxRepo::InMemory(repo), Vec::new())
}

/// Sentinel tx_id used to arm the Phase 5c failure injection. Picked far
/// above any id a fresh-repo gate or hand-built test allocates so the
/// process-global injection register can't collide with a parallel test.
const INJECT_TX_ID: u64 = 7_000_001;

/// Decisive proof of Vector I.3: a Phase-5 sub-phase failure that occurs
/// AFTER the commit point (successful Phase 4 `wal.begin`) is reported as
/// COMMITTED-with-deferred-materialization, and recovery reconciles the
/// not-yet-applied projection.
///
/// `current_thread` flavor keeps the whole commit on one task/thread, so
/// the armed injection register is observed deterministically.
#[tokio::test(flavor = "current_thread")]
async fn post_commit_phase5_failure_is_committed_then_recovered() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));
    let tbl = repo.get_table("t").await.unwrap();
    let token = table_token_for("t");

    // A data record (16-byte RecordId so `wal_ops_from_tx` emits a Put)
    // staged over the table's real data_store — this is the Phase 5a write.
    let rid = RecordId::new();
    let body = InnerValue::Str("materialized-by-recovery".into())
        .to_bytes()
        .unwrap();

    let staging = StagingStore::new(Arc::clone(tbl.data_store()));
    staging.set(rid.to_bytes(), body.clone()).await;

    // An index posting (Phase 5c). Recovery replays this as an IndexPut
    // against the table's info_store (table_id_interned = token).
    let posting_key = Bytes::from_static(b"phase5c_posting_key");
    let posting_val = Bytes::from_static(b"phase5c_posting_value");

    let mut tx = TxContext::new(TxId::new(INJECT_TX_ID), 0, 0, IsolationLevel::Snapshot);
    tx.write_set.insert(token, staging);
    tx.index_write_set.push((
        token,
        IndexWriteOp::SetPosting {
            key: posting_key.clone(),
            value: posting_val.clone(),
        },
    ));

    // Arm the Phase 5c failure for exactly this tx, then commit. Reset the
    // register immediately after so no later test is affected.
    FAIL_PHASE_5C_TX_ID.store(INJECT_TX_ID, Ordering::SeqCst);
    let outcome = repo.commit_tx(tx).await;
    FAIL_PHASE_5C_TX_ID.store(0, Ordering::SeqCst);

    // (1) COMMITTED, not aborted — this is the bug fix. Old code returned
    //     Err here because Phase 5c propagated via `?`.
    let outcome = outcome.expect("post-commit Phase 5c failure must NOT abort the tx");
    assert_eq!(
        outcome.materialization,
        MaterializationState::Deferred,
        "materialization must be reported as Deferred"
    );
    assert!(
        !outcome.materialized(),
        "materialized() must be false on the deferred path"
    );
    assert!(outcome.commit_version > 0, "version must be assigned");

    // (2) WAL marker still inflight — Phase 7 was skipped so recovery
    //     re-applies the entry on the next open.
    let wal = repo.repo_wal().await.unwrap();
    let inflight = wal.list_inflight().await.unwrap();
    assert_eq!(
        inflight.len(),
        1,
        "deferred materialization must leave the WAL marker inflight"
    );
    assert_eq!(inflight[0].txn_id, INJECT_TX_ID);

    // (3) The index posting did NOT materialize inline (Phase 5c failed).
    assert!(
        tbl.info_store().get(posting_key.clone()).await.is_err(),
        "index posting must be absent before recovery (Phase 5c was injected to fail)"
    );

    // (4) Recovery materializes the deferred projection — index posting +
    //     data both land, and the marker is cleaned.
    let count = repo.recover_v2_inflight().await.unwrap();
    assert_eq!(count, 1, "recovery must replay the one inflight entry");

    let recovered_posting = tbl
        .info_store()
        .get(posting_key)
        .await
        .expect("recovery must materialize the index posting (IndexPut replay)");
    assert_eq!(recovered_posting, posting_val);

    let recovered_data = tbl
        .get(rid)
        .await
        .expect("recovery must materialize the data record (Put replay)");
    assert!(
        matches!(recovered_data, InnerValue::Str(ref s) if s == "materialized-by-recovery"),
        "recovered data must match the committed body, got {recovered_data:?}"
    );

    assert!(
        wal.list_inflight().await.unwrap().is_empty(),
        "recovery must remove the WAL marker after reconciliation"
    );

    // Final state is consistent + queryable: both the data record and the
    // index posting are present and a re-run of recovery is a no-op.
    let count2 = repo.recover_v2_inflight().await.unwrap();
    assert_eq!(count2, 0, "second recovery pass must be a no-op");
}

/// The other half of the boundary: a PRE-commit-point failure (here an
/// SSI conflict surfaced before Phase 4) still returns `Err` (a clean
/// abort) and leaves NOTHING durable — no inflight WAL marker.
#[tokio::test]
async fn pre_commit_failure_aborts_and_leaves_nothing() {
    use crate::tx::CommitError;

    struct ConflictProvider;
    impl VersionProvider for ConflictProvider {
        fn version_of(&self, _t: u64, _k: &Bytes) -> Option<u64> {
            // A version far above what the tx read → SSI conflict.
            Some(u64::MAX)
        }
    }

    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));

    let mut tx = TxContext::new(TxId::new(424_242), 0, 1, IsolationLevel::Serializable);
    tx.record_read(table_token_for("t"), Bytes::from_static(b"k"), 0);
    tx.set_version_provider(Arc::new(ConflictProvider));

    let result = repo.commit_tx(tx).await;
    match result {
        Err(CommitError::SsiConflict { key }) => {
            assert_eq!(key, Bytes::from_static(b"k"));
        }
        other => panic!(
            "pre-commit SSI conflict must return Err(SsiConflict), got {:?}",
            other.map(|o| o.commit_version).map_err(|_| "Err(other)")
        ),
    }

    // Nothing durable: the abort happened before Phase 4, so no WAL marker
    // was ever written.
    let wal = repo.repo_wal().await.unwrap();
    assert!(
        wal.list_inflight().await.unwrap().is_empty(),
        "a pre-commit abort must leave no inflight WAL marker"
    );
}
