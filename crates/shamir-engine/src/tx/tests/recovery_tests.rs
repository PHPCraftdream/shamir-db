//! V2 recovery tests (Stage 7.1.a skeleton + 7.1.c–d apply logic).

use std::sync::Arc;

use shamir_storage::storage_in_memory::InMemoryRepo;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;
use shamir_wal::{WalEntryV2, WalOpV2};

use crate::repo::{repo_token, BoxRepo, RepoInstance};
use crate::table::table_manager::table_token_for;
use crate::table::TableConfig;

fn make_repo() -> RepoInstance {
    let repo = Arc::new(InMemoryRepo::new());
    RepoInstance::new("test".into(), BoxRepo::InMemory(repo), Vec::new())
}

#[tokio::test]
async fn recover_v2_inflight_clean_repo_is_zero() {
    let repo = make_repo();
    let count = repo.recover_v2_inflight().await.unwrap();
    assert_eq!(count, 0, "clean repo has no inflight entries");
}

#[tokio::test]
async fn recover_v2_inflight_replays_and_removes_entries() {
    let repo = make_repo();
    let wal = repo.repo_wal().await.unwrap();

    let mut rid_bytes = [0u8; 16];
    rid_bytes[15] = 42;
    let entry = WalEntryV2::new(
        wal.fresh_txn_id(),
        repo_token(repo.name()),
        vec![WalOpV2::Put {
            table_id_interned: 0,
            rid: RecordId(rid_bytes),
            body: bytes::Bytes::from_static(b"payload"),
        }],
    );
    wal.begin(entry).await.unwrap();

    let inflight = wal.list_inflight().await.unwrap();
    assert_eq!(inflight.len(), 1);

    let count = repo.recover_v2_inflight().await.unwrap();
    assert_eq!(count, 1);

    let inflight = wal.list_inflight().await.unwrap();
    assert!(inflight.is_empty(), "marker must be cleaned after recovery");
}

#[tokio::test]
async fn recover_v2_inflight_handles_multiple_entries() {
    let repo = make_repo();
    let wal = repo.repo_wal().await.unwrap();

    for i in 0..3u64 {
        let entry = WalEntryV2::new(
            wal.fresh_txn_id(),
            0,
            vec![WalOpV2::CounterDelta {
                table_id_interned: i,
                delta: 1,
            }],
        );
        wal.begin(entry).await.unwrap();
    }

    let count = repo.recover_v2_inflight().await.unwrap();
    assert_eq!(count, 3);
    assert!(wal.list_inflight().await.unwrap().is_empty());
}

#[tokio::test]
async fn recover_v2_inflight_replays_put_applies_to_data_store() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));
    let tbl = repo.get_table("t").await.unwrap();

    let wal = repo.repo_wal().await.unwrap();
    let mut rid_bytes = [0u8; 16];
    rid_bytes[15] = 42;
    let rid = RecordId(rid_bytes);
    let token = table_token_for("t");

    let value = InnerValue::Str("recovered".into());
    let body = value.to_bytes().unwrap();

    let entry = WalEntryV2::new(
        wal.fresh_txn_id(),
        0,
        vec![WalOpV2::Put {
            table_id_interned: token,
            rid,
            body,
        }],
    );
    wal.begin(entry).await.unwrap();

    assert!(tbl.get(rid).await.is_err());

    let count = repo.recover_v2_inflight().await.unwrap();
    assert_eq!(count, 1);

    let read_back = tbl.get(rid).await.unwrap();
    assert!(
        matches!(read_back, InnerValue::Str(ref s) if s == "recovered"),
        "expected recovered Str, got {:?}",
        read_back
    );
}

#[tokio::test]
async fn recover_v2_inflight_replays_delete_removes_from_data_store() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));
    let tbl = repo.get_table("t").await.unwrap();

    let rid = tbl.insert(&InnerValue::Str("doomed".into())).await.unwrap();
    let _ = tbl.get(rid).await.unwrap();

    let wal = repo.repo_wal().await.unwrap();
    let token = table_token_for("t");
    let entry = WalEntryV2::new(
        wal.fresh_txn_id(),
        0,
        vec![WalOpV2::Delete {
            table_id_interned: token,
            rid,
        }],
    );
    wal.begin(entry).await.unwrap();

    repo.recover_v2_inflight().await.unwrap();

    assert!(
        tbl.get(rid).await.is_err(),
        "rid should be gone after delete recovery"
    );
}

#[tokio::test]
async fn recover_v2_inflight_skips_counter_delta_replay() {
    // CRIT-3 Option A: counter deltas are applied in the happy-path
    // commit (`commit::commit_tx_inner` Phase 5b) and intentionally
    // SKIPPED by recovery to avoid double-counting after a marker-
    // survived crash. Recovery still consumes the WAL entry and
    // removes the marker; data ops in the same entry replay normally
    // (none here, the test is counter-only).
    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));
    let tbl = repo.get_table("t").await.unwrap();
    let before = tbl.counter().get().await.unwrap();

    let wal = repo.repo_wal().await.unwrap();
    let token = table_token_for("t");
    let entry = WalEntryV2::new(
        wal.fresh_txn_id(),
        0,
        vec![WalOpV2::CounterDelta {
            table_id_interned: token,
            delta: 5,
        }],
    );
    wal.begin(entry).await.unwrap();

    repo.recover_v2_inflight().await.unwrap();

    let after = tbl.counter().get().await.unwrap();
    assert_eq!(
        after, before,
        "recovery must NOT replay CounterDelta — happy-path commit owns it"
    );
    assert!(
        wal.list_inflight().await.unwrap().is_empty(),
        "marker still removed even when CounterDelta replay is a no-op"
    );
}

#[tokio::test]
async fn recover_v2_inflight_unknown_table_skips_gracefully() {
    let repo = make_repo();
    let wal = repo.repo_wal().await.unwrap();
    let mut rid_bytes = [0u8; 16];
    rid_bytes[15] = 99;

    let entry = WalEntryV2::new(
        wal.fresh_txn_id(),
        0,
        vec![WalOpV2::Put {
            table_id_interned: 99999,
            rid: RecordId(rid_bytes),
            body: bytes::Bytes::from_static(b"orphan"),
        }],
    );
    wal.begin(entry).await.unwrap();

    let count = repo.recover_v2_inflight().await.unwrap();
    assert_eq!(count, 1);
    assert!(wal.list_inflight().await.unwrap().is_empty());
}

#[tokio::test]
async fn recover_v2_inflight_replays_index_put_with_table_id() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));
    let tbl = repo.get_table("t").await.unwrap();
    let info = tbl.info_store().clone();

    let wal = repo.repo_wal().await.unwrap();
    let token = table_token_for("t");

    let key = bytes::Bytes::from_static(b"some_posting_key");
    let value = bytes::Bytes::from_static(b"some_posting_value");

    let entry = WalEntryV2::new(
        wal.fresh_txn_id(),
        0,
        vec![WalOpV2::IndexPut {
            table_id_interned: token,
            idx_id: 0,
            key: key.clone(),
            value: value.clone(),
        }],
    );
    wal.begin(entry).await.unwrap();

    repo.recover_v2_inflight().await.unwrap();

    let read_back = info.get(key).await.unwrap();
    assert_eq!(read_back, value);
}

#[tokio::test]
async fn recover_v2_inflight_replays_index_put_broadcast() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("a"));
    repo.add_table(TableConfig::new("b"));
    let ta = repo.get_table("a").await.unwrap();
    let tb = repo.get_table("b").await.unwrap();

    let wal = repo.repo_wal().await.unwrap();
    let key = bytes::Bytes::from_static(b"broadcast_key");
    let value = bytes::Bytes::from_static(b"broadcast_val");

    let entry = WalEntryV2::new(
        wal.fresh_txn_id(),
        0,
        vec![WalOpV2::IndexPut {
            table_id_interned: 0,
            idx_id: 0,
            key: key.clone(),
            value: value.clone(),
        }],
    );
    wal.begin(entry).await.unwrap();

    repo.recover_v2_inflight().await.unwrap();

    assert_eq!(ta.info_store().get(key.clone()).await.unwrap(), value);
    assert_eq!(tb.info_store().get(key).await.unwrap(), value);
}

#[tokio::test]
async fn recover_v2_inflight_replays_index_del() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));
    let tbl = repo.get_table("t").await.unwrap();
    let info = tbl.info_store().clone();

    let key = bytes::Bytes::from_static(b"doomed");
    info.set(key.clone(), bytes::Bytes::from_static(b"val"))
        .await
        .unwrap();
    assert!(info.get(key.clone()).await.is_ok());

    let wal = repo.repo_wal().await.unwrap();
    let token = table_token_for("t");

    let entry = WalEntryV2::new(
        wal.fresh_txn_id(),
        0,
        vec![WalOpV2::IndexDel {
            table_id_interned: token,
            idx_id: 0,
            key: key.clone(),
        }],
    );
    wal.begin(entry).await.unwrap();

    repo.recover_v2_inflight().await.unwrap();
    assert!(info.get(key).await.is_err(), "key removed by IndexDel");
}

// Renamed from `crash_simulation_inflight_recovery_replays_full_state`.
//
// NOTE: this test does NOT simulate a real crash at the storage/fsync
// layer. It constructs the in-memory state a *real* crash *would*
// leave behind (an inflight WAL marker over a shared `Arc<InMemoryRepo>`)
// by injecting the marker directly, then drops the original
// `RepoInstance` and rebuilds a fresh one over the same underlying
// storage. That exercises the recovery replay logic — which is the
// useful coverage here — but it does not validate crash atomicity at
// the storage layer; that requires a subprocess-kill harness
// (TODO Stage 7.rest).
#[tokio::test]
async fn replay_inflight_v2_from_simulated_partial_commit_state() {
    use crate::repo::repo_token;

    // Shared underlying repo so "restart" sees the same persisted state.
    let underlying = Arc::new(InMemoryRepo::new());

    // Phase A: original repo opens, adds a table, has counter at 0.
    let repo1 = RepoInstance::new(
        "crash_test".into(),
        BoxRepo::InMemory(Arc::clone(&underlying)),
        Vec::new(),
    );
    repo1.add_table(TableConfig::new("t"));
    let _tbl1 = repo1.get_table("t").await.unwrap();

    // Phase B: simulate the in-memory state of a crash mid-commit_tx:
    // write a V2 WAL entry with two Put ops + counter delta, but
    // DON'T call wal.commit(txn_id) so the marker stays inflight.
    let wal = repo1.repo_wal().await.unwrap();
    let token = table_token_for("t");
    let txn_id = wal.fresh_txn_id();

    let mut rid_a_bytes = [0u8; 16];
    rid_a_bytes[15] = 1;
    let rid_a = RecordId(rid_a_bytes);
    let mut rid_b_bytes = [0u8; 16];
    rid_b_bytes[15] = 2;
    let rid_b = RecordId(rid_b_bytes);

    let body_a = InnerValue::Str("alice".into()).to_bytes().unwrap();
    let body_b = InnerValue::Str("bob".into()).to_bytes().unwrap();

    let entry = WalEntryV2::new(
        txn_id,
        repo_token(repo1.name()),
        vec![
            WalOpV2::Put {
                table_id_interned: token,
                rid: rid_a,
                body: body_a,
            },
            WalOpV2::Put {
                table_id_interned: token,
                rid: rid_b,
                body: body_b,
            },
            WalOpV2::CounterDelta {
                table_id_interned: token,
                delta: 2,
            },
        ],
    );
    wal.begin(entry).await.unwrap();

    // Pre-recovery state: data not visible, counter is 0, marker exists.
    {
        let tbl_pre = repo1.get_table("t").await.unwrap();
        assert!(tbl_pre.get(rid_a).await.is_err(), "data not yet applied");
        assert!(tbl_pre.get(rid_b).await.is_err());
        assert_eq!(tbl_pre.counter().get().await.unwrap(), 0);
        assert_eq!(wal.list_inflight().await.unwrap().len(), 1);
    }

    // === SIMULATED RESTART ===
    // Phase C: drop repo1 — this models *only* the in-memory side of
    // a restart (the underlying Arc<InMemoryRepo> is kept alive via
    // the clone we hold). Real crash atomicity at the storage/fsync
    // layer is not exercised here; see the test-level doc comment.
    drop(repo1);

    let repo2 = RepoInstance::new(
        "crash_test".into(),
        BoxRepo::InMemory(Arc::clone(&underlying)),
        Vec::new(),
    );
    repo2.add_table(TableConfig::new("t"));

    // Sanity: repo2's WAL sees the inflight entry from before.
    let wal2 = repo2.repo_wal().await.unwrap();
    assert_eq!(
        wal2.list_inflight().await.unwrap().len(),
        1,
        "inflight WAL entry must survive restart"
    );

    // Phase D: run recovery.
    let count = repo2.recover_v2_inflight().await.unwrap();
    assert_eq!(count, 1);

    // === VERIFY ===
    // Post-recovery: data present in main store; WAL clean.
    //
    // Counter is intentionally NOT applied by recovery — see CRIT-3
    // Option A in `recovery::replay_v2_op`'s CounterDelta branch.
    // The happy-path commit (which never ran in this crash
    // simulation) applies the counter in-memory in Phase 5b before
    // persisting markers in Phase 6.5. A crash before those steps
    // means the counter stays at its pre-tx value; data is still
    // recovered from the WAL.
    let tbl_post = repo2.get_table("t").await.unwrap();

    let v_a = tbl_post.get(rid_a).await.unwrap();
    let v_b = tbl_post.get(rid_b).await.unwrap();
    assert!(matches!(v_a, InnerValue::Str(ref s) if s == "alice"));
    assert!(matches!(v_b, InnerValue::Str(ref s) if s == "bob"));

    assert_eq!(
        tbl_post.counter().get().await.unwrap(),
        0,
        "CRIT-3 Option A: recovery skips CounterDelta replay; \
         counter only advances in the happy-path commit, which \
         never ran in this crash simulation"
    );

    assert!(
        wal2.list_inflight().await.unwrap().is_empty(),
        "WAL marker removed after recovery"
    );

    // === IDEMPOTENT RE-RUN ===
    // Running recovery again should be a no-op (zero entries) and
    // not double-apply.
    let count2 = repo2.recover_v2_inflight().await.unwrap();
    assert_eq!(count2, 0);
    assert_eq!(tbl_post.counter().get().await.unwrap(), 0);
}

// HIGH-5: replay must apply entries in `commit_version` order even
// when `txn_id` (the WAL active-key sort order) and `commit_version`
// disagree. Construct two inflight entries that write the same rid:
// a "newer" tx (commit_version = 2) with a low txn_id and an "older"
// tx (commit_version = 1) with a high txn_id. Lexical (txn_id) order
// would replay the older tx LAST and the data store would observe
// the wrong final value. With the sort-by-commit_version fix the
// final value must reflect the higher commit_version.
#[tokio::test]
async fn recover_v2_inflight_replays_in_commit_version_order() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));
    let tbl = repo.get_table("t").await.unwrap();
    let token = table_token_for("t");

    let wal = repo.repo_wal().await.unwrap();

    let mut rid_bytes = [0u8; 16];
    rid_bytes[15] = 7;
    let rid = RecordId(rid_bytes);

    let body_newer = InnerValue::Str("newer".into()).to_bytes().unwrap();
    let body_older = InnerValue::Str("older".into()).to_bytes().unwrap();

    // Older tx (commit_version = 1) given a LARGER txn_id so lexical
    // order over WalActiveKey would replay it LAST — overwriting the
    // newer value if we relied on txn_id sort.
    let entry_older = WalEntryV2::new(
        100,
        0,
        vec![WalOpV2::Put {
            table_id_interned: token,
            rid,
            body: body_older,
        }],
    )
    .with_commit_version(1);

    let entry_newer = WalEntryV2::new(
        1,
        0,
        vec![WalOpV2::Put {
            table_id_interned: token,
            rid,
            body: body_newer,
        }],
    )
    .with_commit_version(2);

    wal.begin(entry_older).await.unwrap();
    wal.begin(entry_newer).await.unwrap();

    let count = repo.recover_v2_inflight().await.unwrap();
    assert_eq!(count, 2);

    let read_back = tbl.get(rid).await.unwrap();
    assert!(
        matches!(read_back, InnerValue::Str(ref s) if s == "newer"),
        "expected sort-by-commit_version to leave 'newer' as the final \
         value (commit_version=2 must apply after commit_version=1), \
         got {:?}",
        read_back
    );
}
