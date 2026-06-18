//! V2 recovery tests (Stage 7.1.a skeleton + 7.1.c–d apply logic).

use std::path::PathBuf;
use std::sync::Arc;

use shamir_query_builder::write::{self, doc};
use shamir_storage::storage_in_memory::InMemoryRepo;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;
use shamir_wal::{WalDurability, WalEntryV2, WalOpV2};

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
    wal.begin_grouped(entry, WalDurability::Buffered)
        .await
        .unwrap();

    let inflight = wal.recover().await.unwrap();
    assert_eq!(inflight.len(), 1);

    let count = repo.recover_v2_inflight().await.unwrap();
    assert_eq!(count, 1);
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
        wal.begin_grouped(entry, WalDurability::Buffered)
            .await
            .unwrap();
    }

    let count = repo.recover_v2_inflight().await.unwrap();
    assert_eq!(count, 3);
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
    wal.begin_grouped(entry, WalDurability::Buffered)
        .await
        .unwrap();

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
    wal.begin_grouped(entry, WalDurability::Buffered)
        .await
        .unwrap();

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
    wal.begin_grouped(entry, WalDurability::Buffered)
        .await
        .unwrap();

    repo.recover_v2_inflight().await.unwrap();

    let after = tbl.counter().get().await.unwrap();
    assert_eq!(
        after, before,
        "recovery must NOT replay CounterDelta — happy-path commit owns it"
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
    wal.begin_grouped(entry, WalDurability::Buffered)
        .await
        .unwrap();

    let count = repo.recover_v2_inflight().await.unwrap();
    assert_eq!(count, 1);
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
    wal.begin_grouped(entry, WalDurability::Buffered)
        .await
        .unwrap();

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
    wal.begin_grouped(entry, WalDurability::Buffered)
        .await
        .unwrap();

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
    wal.begin_grouped(entry, WalDurability::Buffered)
        .await
        .unwrap();

    repo.recover_v2_inflight().await.unwrap();
    assert!(info.get(key).await.is_err(), "key removed by IndexDel");
}

// Renamed from `crash_simulation_inflight_recovery_replays_full_state`.
//
// NOTE: this test does NOT simulate a real crash at the storage/fsync
// layer. It constructs the on-disk state a *real* crash *would* leave
// behind — an inflight WAL entry written to the file segment but never
// followed by the data_store update — by appending the entry directly to
// the file WAL, then drops the original `RepoInstance` and reopens a fresh
// one over the SAME tempdir (so it re-reads the same `*.shamirwal/repo.wal`
// segment). That exercises the recovery replay logic — the useful coverage
// here — but it does not validate crash atomicity at the storage layer;
// that requires a subprocess-kill harness (TODO Stage 7.rest).
//
// F5e: this test is disk-backed (sled tempdir). Under the single
// WAL write path the `Mem` sink is per-instance, so an injected entry
// would not survive reopen of an in-memory repo; the file segment is the
// medium that genuinely persists across a simulated restart.
#[tokio::test]
async fn replay_inflight_v2_from_simulated_partial_commit_state() {
    use crate::repo::repo_token;

    let tempdir = tempfile::TempDir::new().expect("tempdir");
    let path = tempdir.path().to_path_buf();

    let mut rid_a_bytes = [0u8; 16];
    rid_a_bytes[15] = 1;
    let rid_a = RecordId(rid_a_bytes);
    let mut rid_b_bytes = [0u8; 16];
    rid_b_bytes[15] = 2;
    let rid_b = RecordId(rid_b_bytes);

    // === Phase A+B: open a disk repo, append an inflight V2 entry (two
    //     Puts + a counter delta) directly to the file WAL WITHOUT applying
    //     it to the data_store — exactly the on-disk shape a crash
    //     mid-commit_tx leaves behind. ===
    {
        let repo1 = reopen_sled_repo("crash_test", path.clone(), vec![TableConfig::new("t")]).await;
        let _tbl1 = repo1.get_table("t").await.unwrap();

        let wal = repo1.repo_wal().await.unwrap();
        let token = table_token_for("t");
        let txn_id = wal.fresh_txn_id();

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
        // Synced so the entry hits the segment file before the drop below.
        wal.begin_grouped(entry, WalDurability::Synced)
            .await
            .unwrap();

        // Pre-recovery state: data not visible, counter is 0, entry inflight.
        let tbl_pre = repo1.get_table("t").await.unwrap();
        assert!(tbl_pre.get(rid_a).await.is_err(), "data not yet applied");
        assert!(tbl_pre.get(rid_b).await.is_err());
        assert_eq!(tbl_pre.counter().get().await.unwrap(), 0);
        assert_eq!(wal.recover().await.unwrap().len(), 1);

        // === SIMULATED RESTART === drop without clean shutdown.
        drop(repo1);
    }

    // === Phase C: reopen a fresh instance over the SAME tempdir. ===
    let repo2 = reopen_sled_repo("crash_test", path.clone(), vec![TableConfig::new("t")]).await;
    repo2.get_table("t").await.unwrap();

    // Sanity: repo2's WAL sees the inflight entry from before.
    let wal2 = repo2.repo_wal().await.unwrap();
    assert_eq!(
        wal2.recover().await.unwrap().len(),
        1,
        "inflight WAL entry must survive restart"
    );

    // Phase D: run recovery.
    let count = repo2.recover_v2_inflight().await.unwrap();
    assert_eq!(count, 1);

    // === VERIFY ===
    // Post-recovery: data present in main store.
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

    // === IDEMPOTENT RE-RUN ===
    // The file WAL has no per-entry truncation until F6, so a second
    // recovery re-reads and replays the whole segment again. Replay is
    // idempotent at the data layer: the records converge to the same
    // single value and the counter still does not advance.
    repo2.recover_v2_inflight().await.unwrap();
    let v_a2 = tbl_post.get(rid_a).await.unwrap();
    assert!(matches!(v_a2, InnerValue::Str(ref s) if s == "alice"));
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

    wal.begin_grouped(entry, WalDurability::Buffered)
        .await
        .unwrap();

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

    wal.begin_grouped(entry_older, WalDurability::Buffered)
        .await
        .unwrap();
    wal.begin_grouped(entry_newer, WalDurability::Buffered)
        .await
        .unwrap();

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

/// F4b-4 end-to-end process-crash recovery proof for a NON-TX write over the
/// FILE WAL (level-2).
///
/// Before F4b the non-tx batch INSERT path emitted a V1 per-table WAL marker
/// (`WalManager::begin_with_delta`/`commit`); F4b routes every non-tx batch
/// write through an implicit Snapshot tx so it folds into ONE `WalEntryV2` on
/// the repo file WAL — exactly like a real tx. F4b-4 removed the now-dead V1
/// emission; this test proves the non-tx write is still crash-recoverable, now
/// purely from the file WAL.
///
/// The query_runner is not easily callable from an engine-level test (it needs
/// a `TableResolver`), so we drive the implicit-tx pipeline directly the same
/// way `run_implicit_batch_tx` does: open a Snapshot tx, mark it implicit,
/// stage the insert via `execute_insert_tx`, and commit. Then DROP the repo
/// WITHOUT clean shutdown (process-crash model, level-2 Buffered contract),
/// reopen over the SAME tempdir, run recovery, and assert the record reads
/// back — recovered purely by replaying the file WAL.
#[tokio::test]
async fn f4b_nontx_insert_crash_recovery() {
    use shamir_tx::IsolationLevel;

    let tempdir = tempfile::TempDir::new().expect("tempdir");
    let path = tempdir.path().to_path_buf();

    // === Phase A: open disk repo, perform a non-tx batch INSERT through the
    //     implicit-tx pipeline (mirrors `run_implicit_batch_tx`). ===
    let rid;
    {
        let factory = BoxRepoFactory::sled_raw(path.clone());
        let repo = RepoInstance::from_factory("f4b".into(), factory, vec![TableConfig::new("t")])
            .await
            .expect("from_factory");
        let tbl = repo.get_table("t").await.unwrap();

        let op = write::insert("t")
            .row(doc().set("name", "nontx_durable"))
            .build();

        // Implicit single-op BATCH tx: Snapshot isolation (never aborts on
        // SSI), marked implicit — exactly what the non-tx query_runner branch
        // does.
        let (mut tx, _g) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
        tx.set_implicit(true);
        let wr = tbl.execute_insert_tx(&op, &mut tx, true).await.unwrap();
        repo.commit_tx(tx).await.unwrap();

        // Capture the assigned record id from the returned result record.
        let id_str = wr.records[0]
            .get_value_owned("_id")
            .and_then(|v| v.as_str().map(str::to_owned))
            .expect("_id present in insert result");
        rid = id_str.parse::<RecordId>().expect("parse RecordId");

        // === Phase B: SIMULATED CRASH — drop without clean shutdown. ===
        // The committed entry lives in the file WAL (level 2, survives a
        // process crash). We deliberately do NOT flush/checkpoint.
        drop(repo);
    }

    // === Phase C: reopen a fresh instance over the SAME tempdir. ===
    let repo2 = reopen_sled_repo("f4b", path.clone(), vec![TableConfig::new("t")]).await;
    let tbl2 = repo2.get_table("t").await.unwrap();

    // === Phase D: recovery replays the file WAL. ===
    let recovered = repo2.recover_v2_inflight().await.unwrap();
    assert!(
        recovered >= 1,
        "expected at least one entry replayed from the file WAL for the \
         non-tx insert, got {recovered}"
    );

    // === VERIFY: the non-tx-inserted record is present (recovered from WAL). ===
    let read_back = tbl2.get(rid).await.unwrap_or_else(|e| {
        panic!("non-tx record {rid:?} must be recovered from the file WAL, got error: {e}")
    });
    let interner = tbl2.interner().get().await.unwrap();
    let json = shamir_types::codecs::interned::inner_to_json_value(&read_back, interner).unwrap();
    assert_eq!(
        json["name"], "nontx_durable",
        "expected recovered non-tx record with name=nontx_durable, got {json:?}"
    );
}

/// C5 recovery invariant: an IMPLICIT insert that introduces a BRAND-NEW field
/// name interns it **directly into base** (not the tx overlay) — but the
/// `(name, base_id)` pair MUST still reach `WalEntryV2.interner_delta` so crash
/// recovery replays it via `touch_with_id` and the record bytes (which encode
/// that base id) decode correctly after a process crash.
///
/// This guards the C5 optimisation: skipping the overlay round-trip on the
/// implicit path must NOT skip the interner-delta WAL emission. We assert the
/// recovered interner maps the new field to the SAME id it had pre-crash
/// (id-preserving recovery), proving the delta carried the exact base id.
#[tokio::test]
async fn c5_implicit_insert_new_field_recovers_with_preserved_id() {
    use shamir_tx::IsolationLevel;

    let tempdir = tempfile::TempDir::new().expect("tempdir");
    let path = tempdir.path().to_path_buf();

    // === Phase A: implicit insert of a record with a brand-new field name. ===
    let rid;
    let pre_crash_id;
    {
        let factory = BoxRepoFactory::sled_raw(path.clone());
        let repo = RepoInstance::from_factory("c5".into(), factory, vec![TableConfig::new("t")])
            .await
            .expect("from_factory");
        let tbl = repo.get_table("t").await.unwrap();

        // "c5_fresh_field" is guaranteed new to the table's interner.
        let op = write::insert("t")
            .row(doc().set("c5_fresh_field", "v"))
            .build();

        let (mut tx, _g) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
        tx.set_implicit(true);
        // Overlay MUST stay empty on the implicit path — the field is interned
        // straight into base, so the commit-time overlay-merge is a no-op.
        let wr = tbl.execute_insert_tx(&op, &mut tx, true).await.unwrap();
        assert!(
            tx.interner_overlay.is_empty(),
            "implicit insert must intern into base, leaving the overlay empty"
        );
        repo.commit_tx(tx).await.unwrap();

        let interner = tbl.interner().get().await.unwrap();
        pre_crash_id = interner
            .get_ind("c5_fresh_field")
            .expect("field must be in base interner after implicit insert")
            .id();

        let id_str = wr.records[0]
            .get_value_owned("_id")
            .and_then(|v| v.as_str().map(str::to_owned))
            .expect("_id present in insert result");
        rid = id_str.parse::<RecordId>().expect("parse RecordId");

        // === Phase B: SIMULATED CRASH — drop without clean shutdown. ===
        drop(repo);
    }

    // === Phase C: reopen over the SAME tempdir; the field name is NOT yet
    //     known to the freshly-built interner. ===
    let repo2 = reopen_sled_repo("c5", path.clone(), vec![TableConfig::new("t")]).await;
    let tbl2 = repo2.get_table("t").await.unwrap();
    {
        let interner = tbl2.interner().get().await.unwrap();
        assert!(
            interner.get_ind("c5_fresh_field").is_none(),
            "field must not exist before recovery (proves we rely on the WAL delta)"
        );
    }

    // === Phase D: recovery replays the file WAL (interner_delta first). ===
    let recovered = repo2.recover_v2_inflight().await.unwrap();
    assert!(
        recovered >= 1,
        "expected >=1 replayed entry, got {recovered}"
    );

    // === VERIFY: the field id is preserved AND the record decodes. ===
    {
        let interner = tbl2.interner().get().await.unwrap();
        let recovered_id = interner
            .get_ind("c5_fresh_field")
            .expect("field must be in interner after recovery (from WAL delta)")
            .id();
        assert_eq!(
            recovered_id, pre_crash_id,
            "recovery must preserve the exact base id the record bytes encode"
        );
    }
    let read_back = tbl2.get(rid).await.unwrap_or_else(|e| {
        panic!("record {rid:?} must be recovered from the file WAL, got error: {e}")
    });
    let interner = tbl2.interner().get().await.unwrap();
    let json = shamir_types::codecs::interned::inner_to_json_value(&read_back, interner).unwrap();
    assert_eq!(
        json["c5_fresh_field"], "v",
        "recovered record must decode the new field correctly, got {json:?}"
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

/// Stage I — cross-table shared-id invariant: the interner is per-REPO, so
/// the SAME field name must resolve to the SAME interned id across every
/// table in the repo. This is the defining property of the per-repo move.
///
/// Two tables ("alpha" and "beta") in one repo each insert a record with the
/// same field name "shared_field". After commit, both tables' interners
/// (which are the SAME repo interner, Arc-shared via `with_interner`) must
/// report the identical id for "shared_field". Pre-Stage-I (per-table
/// interners) the two ids were independent and typically differed.
#[tokio::test]
async fn stage_i_cross_table_shared_interner_id() {
    use shamir_tx::IsolationLevel;

    let repo = make_repo();
    repo.add_table(TableConfig::new("alpha"));
    repo.add_table(TableConfig::new("beta"));

    let alpha = repo.get_table("alpha").await.unwrap();
    let beta = repo.get_table("beta").await.unwrap();

    // Insert into alpha with "shared_field".
    let op_a = write::insert("alpha")
        .row(doc().set("shared_field", "a_val"))
        .build();
    let (mut tx_a, _g_a) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    alpha
        .execute_insert_tx(&op_a, &mut tx_a, true)
        .await
        .unwrap();
    repo.commit_tx(tx_a).await.unwrap();

    // Insert into beta with the SAME "shared_field" name.
    let op_b = write::insert("beta")
        .row(doc().set("shared_field", "b_val"))
        .build();
    let (mut tx_b, _g_b) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    beta.execute_insert_tx(&op_b, &mut tx_b, true)
        .await
        .unwrap();
    repo.commit_tx(tx_b).await.unwrap();

    // Both tables resolve "shared_field" through the SHARED repo interner.
    let alpha_interner = alpha.interner().get().await.unwrap();
    let beta_interner = beta.interner().get().await.unwrap();
    let id_a = alpha_interner
        .get_ind("shared_field")
        .expect("alpha must know shared_field")
        .id();
    let id_b = beta_interner
        .get_ind("shared_field")
        .expect("beta must know shared_field")
        .id();
    assert_eq!(
        id_a, id_b,
        "Stage I: the same field name MUST resolve to the same id across tables in one repo"
    );

    // Sanity: the two managers are backed by the SAME OnceCell (Arc-shared).
    assert!(
        std::ptr::eq(
            alpha.interner().interner_cell().as_ref() as *const _,
            beta.interner().interner_cell().as_ref() as *const _,
        ),
        "Stage I: both tables' InternerManager must share the same OnceCell<Interner>"
    );
}

/// Stage I — repo-scope id-preserving recovery: a WAL entry whose
/// `interner_delta` carries a repo-scope constant (0) as the first triple
/// element must have its delta applied to the SINGLE repo interner, and the
/// replayed record bytes (which encode that id) must decode correctly.
///
/// This is the keystone test for the recovery rewrite: pre-Stage-I recovery
/// routed each delta triple through `repo.table_by_token(token)`; now it
/// resolves ONE repo interner directly. The test seeds a WAL entry with a
/// Put into table "t" whose body encodes id 42, plus an interner_delta
/// naming "stage_i_field" at id 42 under the REPO scope constant (0).
/// Recovery must apply the delta to the repo interner and the record must
/// decode.
#[tokio::test]
async fn stage_i_repo_scope_interner_delta_recovers() {
    use crate::tx::pre_commit::REPO_INTERNER_SCOPE;

    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));
    let tbl = repo.get_table("t").await.unwrap();
    let token = table_token_for("t");

    let wal = repo.repo_wal().await.unwrap();

    let mut rid_bytes = [0u8; 16];
    rid_bytes[15] = 77;
    let rid = RecordId(rid_bytes);

    // Build a record body that encodes interned id 42 for "stage_i_field".
    // We intern the name into the repo interner at id 42 via touch_with_id,
    // serialize a record using that id, then DROP the repo interner's
    // in-memory state by building a fresh entry that re-introduces id 42
    // through the WAL delta. Simpler: just use a Str body (no interned ids
    // in the body) and assert the interner learns the name+id from the delta.
    let body = InnerValue::Str("stage_i_body".into()).to_bytes().unwrap();

    // Build entry with a repo-scope interner_delta (first u64 = 0).
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
    entry.interner_delta = vec![(REPO_INTERNER_SCOPE, "stage_i_field".to_string(), 42)];

    wal.begin_grouped(entry, WalDurability::Buffered)
        .await
        .unwrap();

    // Before recovery: the repo interner does NOT know "stage_i_field".
    {
        let repo_interner = repo.repo_interner().await.unwrap();
        let interner = repo_interner.get().await.unwrap();
        assert!(
            interner.get_ind("stage_i_field").is_none(),
            "stage_i_field must not exist before recovery"
        );
    }

    let count = repo.recover_v2_inflight().await.unwrap();
    assert_eq!(count, 1, "exactly one entry must be replayed");

    // After recovery: the REPO interner MUST know "stage_i_field" at id 42.
    {
        let repo_interner = repo.repo_interner().await.unwrap();
        let interner = repo_interner.get().await.unwrap();
        let key = interner.get_ind("stage_i_field");
        assert!(
            key.is_some(),
            "stage_i_field must be in the REPO interner after recovery"
        );
        assert_eq!(
            key.unwrap().id(),
            42,
            "stage_i_field must map to the id from the repo-scope delta"
        );
    }

    // And the table's interner (which is the SAME repo interner) must agree.
    let tbl_interner = tbl.interner().get().await.unwrap();
    assert_eq!(
        tbl_interner
            .get_ind("stage_i_field")
            .expect("tbl interner == repo interner post-Stage-I")
            .id(),
        42,
    );
}
