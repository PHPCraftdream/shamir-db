//! Async-index commit visibility (opt-in) — behavioural contract.
//!
//! The opt-in [`shamir_tx::CommitVisibility::AsyncIndex`] mode lets
//! `commit_tx` return to the caller right after the durable WAL entry
//! (Phase 4) + data application (Phase 5a) + MVCC publish (Phase 6).
//! The remaining tail — Phase 5c (index postings), Phase 6.5 (recovery
//! markers), Phase 7 (WAL marker removal), Phase 5d (HNSW promote) —
//! runs on a background `tokio::task` carried back via
//! [`TxOutcome::background`].
//!
//! These tests pin the four sides of the contract:
//!
//! (a) data is immediately visible after the async ack (read-your-own-
//!     writes on data holds — 5a + publish ran before ack).
//! (b) index converges shortly after — awaiting the background handle
//!     makes the secondary index posting observable.
//! (c) a Phase-5c failure injected on the background tail leaves the
//!     inflight WAL marker, and `recover_v2_inflight` reconciles the
//!     missing index posting — i.e. crash-equivalent semantics.
//! (d) default sync mode is byte-identical: a tx that DOESN'T opt in
//!     produces no background handle.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use bytes::Bytes;
use shamir_storage::storage_in_memory::InMemoryRepo;
use shamir_tx::{CommitVisibility, IndexWriteOp, IsolationLevel, StagingStore, TxContext, TxId};
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

/// Sentinel range that doesn't collide with other `FAIL_PHASE_5C_TX_ID`
/// armers (`commit_phase5_defer_tests` uses 7_000_001 / 7_000_002).
const ASYNC_INJECT_TX_ID: u64 = 7_100_001;

/// Borrow the process-global Phase-5c injection lock from the defer-tests
/// module so async-mode + sync-mode injection tests don't clobber each
/// other's arm windows when running in parallel.
use super::commit_phase5_defer_tests::PHASE_5C_INJECT_LOCK;

/// (a) Data is immediately visible after an async-mode commit (no
/// `background.join()`): read-your-own-writes on DATA holds because
/// Phase 5a + Phase 6 ran on the client path.
#[tokio::test(flavor = "current_thread")]
async fn async_commit_data_visible_immediately() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));
    let tbl = repo.get_table("t").await.unwrap();
    let token = table_token_for("t");

    let rid = RecordId::new();
    let body = InnerValue::Str("async-data-visible".into())
        .to_bytes()
        .unwrap();

    let staging = StagingStore::new(Arc::clone(tbl.data_store()));
    staging.set(rid.to_bytes(), body).await;

    let mut tx = TxContext::new(
        TxId::new(ASYNC_INJECT_TX_ID),
        0,
        0,
        IsolationLevel::Snapshot,
    );
    tx.write_set.insert(token, staging);
    tx.set_visibility(CommitVisibility::AsyncIndex);

    let outcome = repo.commit_tx(tx).await.expect("async commit must succeed");
    assert_eq!(
        outcome.materialization,
        MaterializationState::Complete,
        "ack-time materialization must report Complete (sync-prefix landed)"
    );
    assert!(
        outcome.background.is_some(),
        "async-index mode must carry a background handle"
    );
    assert!(
        outcome.commit_version > 0,
        "the version must be published before ack (Phase 6 ran on the client path)"
    );

    // Data is immediately observable via the table's MvccStore (Phase 5a
    // ran inline before the ack returned).
    let observed = tbl
        .get(rid)
        .await
        .expect("data record must be visible immediately after the async ack");
    assert!(
        matches!(observed, InnerValue::Str(ref s) if s == "async-data-visible"),
        "read-your-own-writes on DATA must hold without awaiting the tail"
    );
}

/// (b) Index converges after the background tail finishes — observable by
/// awaiting `BackgroundCommitHandle::join`.
#[tokio::test(flavor = "current_thread")]
async fn async_commit_index_converges_after_tail() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));
    let tbl = repo.get_table("t").await.unwrap();
    let token = table_token_for("t");

    let rid = RecordId::new();
    let body = InnerValue::Str("async-index-converges".into())
        .to_bytes()
        .unwrap();
    let staging = StagingStore::new(Arc::clone(tbl.data_store()));
    staging.set(rid.to_bytes(), body).await;

    let posting_key = Bytes::from_static(b"async_posting_k");
    let posting_val = Bytes::from_static(b"async_posting_v");

    let mut tx = TxContext::new(
        TxId::new(ASYNC_INJECT_TX_ID + 1),
        0,
        0,
        IsolationLevel::Snapshot,
    );
    tx.write_set.insert(token, staging);
    tx.index_write_set.push((
        token,
        IndexWriteOp::SetPosting {
            key: posting_key.clone(),
            value: posting_val.clone(),
        },
    ));
    tx.set_visibility(CommitVisibility::AsyncIndex);

    let mut outcome = repo.commit_tx(tx).await.expect("async commit must succeed");
    let bg = outcome
        .take_background()
        .expect("async mode hands back a tail");

    // Drive the tail to completion: the index posting must materialize and
    // the WAL marker must be cleared (Phase 7 ran on the background task).
    let state = bg.join().await;
    assert_eq!(
        state,
        MaterializationState::Complete,
        "happy-path tail must finalize Complete"
    );

    let observed = tbl
        .info_store()
        .get(posting_key)
        .await
        .expect("secondary-index posting must be present after the tail finishes");
    assert_eq!(observed, posting_val);

    let wal = repo.repo_wal().await.unwrap();
    assert!(
        wal.list_inflight().await.unwrap().is_empty(),
        "Phase 7 must remove the WAL marker on a Complete tail"
    );
}

/// (c) A persistent Phase-5c failure on the BACKGROUND task is the
/// crash-equivalent: the inflight WAL marker survives, the tail reports
/// `Deferred`, and `recover_v2_inflight` reconciles the missing posting —
/// proving the recovery contract still backs the async path.
#[tokio::test(flavor = "current_thread")]
async fn async_commit_background_failure_is_recovered() {
    const TX_ID: u64 = ASYNC_INJECT_TX_ID + 2;
    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));
    let tbl = repo.get_table("t").await.unwrap();
    let token = table_token_for("t");

    let rid = RecordId::new();
    let body = InnerValue::Str("async-recovered".into())
        .to_bytes()
        .unwrap();
    let staging = StagingStore::new(Arc::clone(tbl.data_store()));
    staging.set(rid.to_bytes(), body).await;

    let posting_key = Bytes::from_static(b"async_recover_posting_k");
    let posting_val = Bytes::from_static(b"async_recover_posting_v");

    let mut tx = TxContext::new(TxId::new(TX_ID), 0, 0, IsolationLevel::Snapshot);
    tx.write_set.insert(token, staging);
    tx.index_write_set.push((
        token,
        IndexWriteOp::SetPosting {
            key: posting_key.clone(),
            value: posting_val.clone(),
        },
    ));
    tx.set_visibility(CommitVisibility::AsyncIndex);

    // Arm the Phase 5c failure for this tx, run the commit (ack returns
    // straight away — sync prefix only), then drive the tail to completion
    // BEFORE disarming so the injected error reaches `apply_index_batch`.
    let inject_guard = PHASE_5C_INJECT_LOCK.lock().await;
    FAIL_PHASE_5C_TX_ID.store(TX_ID, Ordering::SeqCst);

    let mut outcome = repo.commit_tx(tx).await.expect("ack must succeed");
    assert_eq!(
        outcome.materialization,
        MaterializationState::Complete,
        "ack-time outcome reports the sync prefix (the tail is still in flight)"
    );

    let bg = outcome
        .take_background()
        .expect("async mode hands back a tail");
    let state = bg.join().await;

    FAIL_PHASE_5C_TX_ID.store(0, Ordering::SeqCst);
    drop(inject_guard);

    assert_eq!(
        state,
        MaterializationState::Deferred,
        "a failed Phase 5c on the tail must surface as Deferred"
    );

    // Inflight marker survives → recovery is the guarantor.
    let wal = repo.repo_wal().await.unwrap();
    let inflight = wal.list_inflight().await.unwrap();
    assert_eq!(
        inflight.len(),
        1,
        "deferred async tail must leave the WAL marker inflight"
    );

    // Index posting absent before recovery (the injected failure stopped it).
    assert!(
        tbl.info_store().get(posting_key.clone()).await.is_err(),
        "secondary index posting must be absent before recovery"
    );

    // Recovery materializes the deferred posting.
    let count = repo.recover_v2_inflight().await.unwrap();
    assert_eq!(count, 1, "recovery must replay the one inflight entry");
    let recovered = tbl
        .info_store()
        .get(posting_key)
        .await
        .expect("recovery must materialize the deferred index posting");
    assert_eq!(recovered, posting_val);
    assert!(
        wal.list_inflight().await.unwrap().is_empty(),
        "recovery must clean the inflight marker"
    );
}

/// (d) Default (sync) mode is byte-identical: no background handle is
/// returned, and the outcome shape mirrors the historical `commit_tx`
/// contract. Acts as a regression gate against accidentally promoting any
/// tx to async mode.
#[tokio::test(flavor = "current_thread")]
async fn sync_default_mode_returns_no_background_handle() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));
    let tbl = repo.get_table("t").await.unwrap();
    let token = table_token_for("t");

    let rid = RecordId::new();
    let body = InnerValue::Str("sync-default".into()).to_bytes().unwrap();
    let staging = StagingStore::new(Arc::clone(tbl.data_store()));
    staging.set(rid.to_bytes(), body).await;

    let posting_key = Bytes::from_static(b"sync_default_posting_k");
    let posting_val = Bytes::from_static(b"sync_default_posting_v");

    let mut tx = TxContext::new(
        TxId::new(ASYNC_INJECT_TX_ID + 3),
        0,
        0,
        IsolationLevel::Snapshot,
    );
    tx.write_set.insert(token, staging);
    tx.index_write_set.push((
        token,
        IndexWriteOp::SetPosting {
            key: posting_key.clone(),
            value: posting_val.clone(),
        },
    ));
    // Visibility intentionally NOT set — exercises the `Default` impl.
    assert_eq!(tx.visibility, CommitVisibility::Synchronous);

    let outcome = repo.commit_tx(tx).await.expect("sync commit must succeed");
    assert_eq!(
        outcome.materialization,
        MaterializationState::Complete,
        "sync mode finalizes inline"
    );
    assert!(
        outcome.background.is_none(),
        "sync mode must NOT hand back a background handle"
    );

    // Both data and index are immediately observable WITHOUT any wait.
    let _ = tbl
        .get(rid)
        .await
        .expect("data must be visible immediately in sync mode");
    let posted = tbl
        .info_store()
        .get(posting_key)
        .await
        .expect("index posting must be visible immediately in sync mode");
    assert_eq!(posted, posting_val);

    // WAL marker is removed inline (Phase 7 ran).
    let wal = repo.repo_wal().await.unwrap();
    assert!(
        wal.list_inflight().await.unwrap().is_empty(),
        "sync mode removes the WAL marker inline (Phase 7)"
    );
}
