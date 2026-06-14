//! V2 recovery tests (Stage 7.1.a skeleton + 7.1.c–d apply logic).

use std::path::PathBuf;
use std::sync::Arc;

use shamir_storage::storage_in_memory::InMemoryRepo;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;
use shamir_wal::{WalEntryV2, WalOpV2};

use crate::repo::{repo_token, BoxRepo, BoxRepoFactory, RepoInstance};
use crate::table::table_manager::table_token_for;
use crate::table::TableConfig;

/// Retry helper for reopening a sled-backed repo on Windows, where sled's
/// file lock is released lazily after `drop`. On non-Windows platforms this
/// almost always succeeds on the first attempt.
async fn reopen_sled_repo(name: &str, path: PathBuf, tables: Vec<TableConfig>) -> RepoInstance {
    let mut last_err = None;
    for _attempt in 0..10 {
        match RepoInstance::from_factory(
            name.into(),
            BoxRepoFactory::sled_raw(path.clone()),
            tables.clone(),
        )
        .await
        {
            Ok(r) => return r,
            Err(e) => {
                last_err = Some(e);
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
        }
    }
    panic!(
        "reopen_sled_repo({name:?}) failed after 10 retries: {:?}",
        last_err
    );
}

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

    // C2: reads resolve from the version log. A committed delete supersedes the
    // record's prior versions, so its `commit_version` is a monotonic value
    // ABOVE the insert's — the real commit path stamps it via
    // `with_commit_version`. The bare `WalEntryV2::new(..)` leaves
    // `commit_version = 0` (its 2nd arg is `repo_id_interned`, NOT the version),
    // which would place the recovery tombstone at version 0 — BELOW the insert
    // (version 1) — so the log's max version would still be the insert and the
    // delete would not supersede it. Stamp a realistic commit version.
    let insert_v = tbl
        .mvcc_store_ref()
        .expect("mvcc attached")
        .version_of(rid.to_bytes().as_ref());

    let wal = repo.repo_wal().await.unwrap();
    let token = table_token_for("t");
    let entry = WalEntryV2::new(
        wal.fresh_txn_id(),
        0,
        vec![WalOpV2::Delete {
            table_id_interned: token,
            rid,
        }],
    )
    .with_commit_version(insert_v + 1);
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

/// A3 + A4-recovery: a WAL entry with a non-empty `interner_delta` must
/// have its delta applied to the table's interner BEFORE ops are replayed.
/// This ensures intern-ids referenced in the record body are resolvable.
#[tokio::test]
async fn wal_recovery_applies_interner_delta_before_replay() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));
    let tbl = repo.get_table("t").await.unwrap();
    let token = table_token_for("t");

    let wal = repo.repo_wal().await.unwrap();

    let mut rid_bytes = [0u8; 16];
    rid_bytes[15] = 50;
    let rid = RecordId(rid_bytes);

    let body = InnerValue::Str("hello".into()).to_bytes().unwrap();

    // Build entry with an interner_delta that introduces a new field
    // "fresh_field" at id 42 for table token `token`.
    let mut entry = WalEntryV2::new(
        wal.fresh_txn_id(),
        0,
        vec![WalOpV2::Put {
            table_id_interned: token,
            rid,
            body,
        }],
    )
    .with_commit_version(1);
    entry.interner_delta = vec![(token, "fresh_field".to_string(), 42)];

    wal.begin(entry).await.unwrap();

    // Before recovery: the interner should NOT know about "fresh_field".
    {
        let interner = tbl.interner().get().await.unwrap();
        assert!(
            interner.get_ind("fresh_field").is_none(),
            "fresh_field must not exist before recovery"
        );
    }

    let count = repo.recover_v2_inflight().await.unwrap();
    assert_eq!(count, 1);

    // After recovery: the interner MUST know "fresh_field" at id 42.
    {
        let interner = tbl.interner().get().await.unwrap();
        let key = interner.get_ind("fresh_field");
        assert!(
            key.is_some(),
            "fresh_field must be in the interner after recovery"
        );
        assert_eq!(
            key.unwrap().id(),
            42,
            "fresh_field must map to the id from the delta"
        );
    }
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

/// F3 end-to-end process-crash recovery proof over the FILE WAL (level-2).
///
/// Builds a disk-backed (sled-raw) repo, commits a tx through the real tx
/// path (which now appends to the file WAL via `begin_grouped(Buffered)`),
/// then DROPS the `RepoInstance` WITHOUT a clean shutdown — modelling a
/// **process crash** (level-2 / Buffered contract: the OS page cache keeps
/// the WAL segment intact across a process exit). A fresh `RepoInstance` is
/// opened over the SAME tempdir (so it sees the same `*.shamirwal/repo.wal`
/// segment), recovery runs, and the committed record must be present —
/// recovered purely from replaying the file WAL.
///
/// NOTE: this is NOT a power-loss (level-3) test. A true power-loss test
/// would require fsync-then-kill-then-truncate-page-cache, which demands a
/// subprocess harness and is out of scope here.
#[tokio::test]
async fn f3_file_wal_process_crash_recovery_replays_committed_tx() {
    use shamir_tx::IsolationLevel;

    let tempdir = tempfile::TempDir::new().expect("tempdir");
    let path = tempdir.path().to_path_buf();

    // === Phase A: open disk repo, commit a tx via the real tx path. ===
    let rid;
    {
        let factory = BoxRepoFactory::sled_raw(path.clone());
        let repo = RepoInstance::from_factory("f3".into(), factory, vec![TableConfig::new("t")])
            .await
            .expect("from_factory");
        repo.get_table("t").await.unwrap();

        let (mut tx, _g) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
        let tbl = repo.get_table("t").await.unwrap();
        rid = tbl
            .insert_tx(&InnerValue::Str("durable".into()), Some(&mut tx))
            .await
            .unwrap();
        repo.commit_tx(tx).await.unwrap();

        // === Phase B: SIMULATED CRASH — drop without clean shutdown. ===
        // The committed entry lives in the file WAL (level 2, survives a
        // process crash). We deliberately do NOT flush/checkpoint.
        drop(repo);
    }

    // === Phase C: reopen a fresh instance over the SAME tempdir. ===
    // Use the retry helper: on Windows, sled releases its file lock lazily
    // after drop, so the reopen can occasionally fail on the first attempt.
    let repo2 = reopen_sled_repo("f3", path.clone(), vec![TableConfig::new("t")]).await;
    let tbl2 = repo2.get_table("t").await.unwrap();

    // === Phase D: recovery replays the file WAL. ===
    let recovered = repo2.recover_v2_inflight().await.unwrap();
    assert!(
        recovered >= 1,
        "expected at least one entry replayed from the file WAL, got {recovered}"
    );

    // === VERIFY: the committed record is present (recovered from WAL). ===
    let read_back = tbl2.get(rid).await.unwrap_or_else(|e| {
        panic!("record {rid:?} must be recovered from the file WAL, got error: {e}")
    });
    assert!(
        matches!(read_back, InnerValue::Str(ref s) if s == "durable"),
        "expected recovered Str(\"durable\"), got {read_back:?}"
    );
}

/// F3 file-WAL replay idempotency: running `recover_v2_inflight` more than
/// once must converge to the same DATA state and never corrupt or duplicate
/// records.
///
/// The file WAL replays the WHOLE segment on every restart (there is no
/// per-entry truncation until the F6 checkpoint), so `replay_v2_entry` is
/// documented as idempotent at the data layer. This test makes that contract
/// explicit:
///
/// 1. Commit a tx on a disk repo (sled_raw + tempdir) — same setup as the
///    f3 process-crash test.
/// 2. Reopen the repo (simulated process crash).
/// 3. Run `recover_v2_inflight()` twice on the same open instance. The second
///    call will re-read the segment and replay the same entries again (file WAL
///    has no per-entry truncation until F6) — both calls will return >= 1.
/// 4. Assert the record reads back as the single correct value after both runs
///    (no duplication, no corruption).
#[tokio::test]
async fn f3_file_wal_replay_is_idempotent() {
    use shamir_tx::IsolationLevel;

    let tempdir = tempfile::TempDir::new().expect("tempdir");
    let path = tempdir.path().to_path_buf();

    // === Phase A: open disk repo, commit a tx. ===
    let rid;
    {
        let factory = BoxRepoFactory::sled_raw(path.clone());
        let repo =
            RepoInstance::from_factory("f3_idem".into(), factory, vec![TableConfig::new("t")])
                .await
                .expect("from_factory");
        repo.get_table("t").await.unwrap();

        let (mut tx, _g) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
        let tbl = repo.get_table("t").await.unwrap();
        rid = tbl
            .insert_tx(&InnerValue::Str("idempotent".into()), Some(&mut tx))
            .await
            .unwrap();
        repo.commit_tx(tx).await.unwrap();

        // Simulated process crash — drop without clean shutdown.
        drop(repo);
    }

    // === Phase B: reopen over the same tempdir. ===
    // Use the retry helper: on Windows, sled releases its file lock lazily
    // after drop, so the reopen can occasionally fail on the first attempt.
    let repo2 = reopen_sled_repo("f3_idem", path.clone(), vec![TableConfig::new("t")]).await;
    let tbl2 = repo2.get_table("t").await.unwrap();

    // === Phase C: run recovery TWICE on the same open instance. ===
    let first = repo2.recover_v2_inflight().await.unwrap();
    assert!(
        first >= 1,
        "first recovery must replay at least one entry, got {first}"
    );

    // The second call re-reads the file segment (no F6 truncation yet) and
    // replays the same entries — idempotency means the DATA is unchanged, not
    // that the count is zero. Assert only that it does not panic/error.
    let second = repo2.recover_v2_inflight().await.unwrap();
    assert!(
        second >= 1,
        "second recovery also replays the segment (file WAL: no truncation until F6), got {second}"
    );

    // === VERIFY: single correct value after double replay — no corruption. ===
    let read_back = tbl2.get(rid).await.unwrap_or_else(|e| {
        panic!("record {rid:?} must be present after idempotent recovery, got: {e}")
    });
    assert!(
        matches!(read_back, InnerValue::Str(ref s) if s == "idempotent"),
        "expected Str(\"idempotent\") after double replay, got {read_back:?}"
    );
}
