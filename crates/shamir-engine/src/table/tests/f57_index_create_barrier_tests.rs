//! F-57 (#883, P0) — unified online CREATE INDEX lifecycle across all index
//! families.
//!
//! F-56 (#882) already fixed `create_index_v2` (the index2 / fts / functional /
//! vector path): it corrected the `WriterDrainBarrier`'s memory ordering to
//! `SeqCst` and wired `drain_writers()` into `create_index_v2`. This file tests
//! the THREE remaining index families that had the same structural gap:
//!
//! 1. **Regular hash index** (`create_index`): ZERO protection pre-F-57 — no
//!    intent flag, no lock, no drain. `needs_write_barrier()` never considered
//!    regular indexes at all.
//! 2. **Unique index** (`create_unique_index` / `create_unique_index_locked`):
//!    had `unique_write_lock`, but writers only take that lock when
//!    `has_unique_indexes()` is `true`. For the FIRST unique index on a table,
//!    that predicate is `false` until the index is registered — so every
//!    concurrent fast-path writer bypassed the lock entirely.
//! 3. **Sorted index** (`create_sorted_index_with_include`): no write barrier
//!    of any kind, and register-before-backfill (a different shape).
//!
//! Each test follows the F-56 template (`f56_create_index_v2_drains_inflight_
//! fast_path_writer`): the F-48 test seam (`PostBarrierPreWriteHook` +
//! `tokio::sync::Notify`) parks a fast-path writer strictly AFTER its
//! `needs_write_barrier()` flag read and BEFORE its data-store write, with the
//! writer already in the drain set. The CREATE INDEX is then spawned and
//! (post-fix) blocks in `drain_writers()` until the writer is released.
//!
//! Red-then-green: pre-fix each create runs to completion while the writer is
//! parked → `is_finished()` is true → the blocking assertion FAILS. Post-fix
//! the create blocks on the drain → `is_finished()` is false → PASSES.

use std::sync::Arc;
use std::time::Duration;

use shamir_types::core::interner::{InternerKey, TouchInd};
use shamir_types::types::common::new_map_wc;
use shamir_types::types::value::InnerValue;

use crate::repo::repo_instance::RepoInstance;
use crate::repo::repo_types::BoxRepo;
use crate::table::TableConfig;
use crate::table::TableManager;
use shamir_storage::storage_in_memory::InMemoryRepo;

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

// ============================================================================
// 1. Regular hash index — `create_index` drain proof
// ============================================================================

/// F-57: `create_index` must drain an in-flight fast-path writer before its
/// backfill snapshot. Pre-fix, `create_index` had ZERO protection — no lock, no
/// flag, no drain — so the create ran to completion while the writer was parked
/// pre-write. Post-fix it acquires `unique_write_lock`, raises
/// `regular_index_create_barrier`, and blocks in `drain_writers()`.
#[tokio::test]
async fn f57_create_index_drains_inflight_fast_path_writer() {
    use crate::table::table_manager_crud::{
        PostBarrierPreWriteHook, TEST_POST_BARRIER_PRE_WRITE_HOOK,
    };
    use tokio::sync::Notify;

    let repo = make_repo();
    repo.add_table(TableConfig::new("people"));
    let tbl = repo.get_table("people").await.unwrap();
    let name_field = key_id(&tbl, "name").await;

    // One pre-existing row so the backfill has something to stream.
    let _alice = tbl
        .insert(&record_with_str(name_field, "Alice"))
        .await
        .unwrap();

    // Precondition: barrier down → lock-free fast path for a table with no
    // unique indexes.
    assert!(
        !tbl.needs_write_barrier(),
        "precondition: no barrier active → lock-free fast path"
    );

    // Install the F-48 test seam: park the FIRST writer strictly after its flag
    // read, before its write. The writer has already entered the drain set.
    let hook = Arc::new(PostBarrierPreWriteHook {
        reached: std::sync::atomic::AtomicUsize::new(0),
        resume: Notify::new(),
        armed: std::sync::atomic::AtomicBool::new(true),
    });
    TEST_POST_BARRIER_PRE_WRITE_HOOK
        .set(Arc::clone(&hook))
        .expect("hook installed once per test process");

    // 1. Spawn the writer. It enters the drain set, reads
    //    needs_write_barrier() == false, and parks at the seam — BEFORE its write.
    let tbl_w = tbl.clone();
    let writer =
        tokio::spawn(async move { tbl_w.insert(&record_with_str(name_field, "Bob")).await });

    // Rendezvous: the writer has parked (busy-poll `reached`, no sleeps).
    while hook.reached.load(std::sync::atomic::Ordering::SeqCst) == 0 {
        tokio::task::yield_now().await;
    }
    assert_eq!(
        tbl.count().await.unwrap(),
        1,
        "writer parked before its write — only the pre-existing row is present"
    );

    // 2. Spawn the create. It acquires unique_write_lock (uncontended: the
    //    fast-path writer holds NO lock), raises regular_index_create_barrier,
    //    and (post-fix) drains.
    let tbl_c = tbl.clone();
    let create = tokio::spawn(async move { tbl_c.create_index("name_idx", &["name"]).await });

    // Give the create enough time to acquire the lock, raise the barrier, and
    // (post-fix) enter drain_writers(). On the unfixed code the create runs to
    // completion in well under this window.
    tokio::time::sleep(Duration::from_millis(80)).await;

    // 3. THE PROOF. The create must still be parked in drain_writers() because
    //    the writer is still in the drain set (active == 1).
    //    - UNFIXED: create already finished (no drain) → assertion FAILS.
    //    - FIXED: create blocked at drain_writers() → assertion PASSES.
    assert!(
        !create.is_finished(),
        "F-57: create_index must drain the in-flight fast-path writer (parked \
         in the drain set) before proceeding to its backfill snapshot. If this \
         fails, create_index finished without waiting — drain_writers() is \
         missing from the create path (the lost-write race is still open)."
    );

    // 4. Release the parked writer. It completes its write + index updates and
    //    exits the drain set (the WriterDrainGuard drops).
    hook.resume.notify_one();
    let bob = writer
        .await
        .unwrap()
        .expect("writer must complete once released");

    // 5. The create's drain returns (writer exited the drain set), the backfill
    //    now sees BOTH rows (Alice pre-existing + Bob just written), and the
    //    index is registered.
    create
        .await
        .unwrap()
        .expect("create must complete once the writer finishes");

    // Sanity: the writer's row is indexed by the backfill (the drain ensured
    // the backfill snapshot was taken AFTER the in-flight writer landed).
    let bob_owners = tbl
        .lookup_by_index("name_idx", &[InnerValue::Str("Bob".into())])
        .await
        .unwrap();
    assert!(
        bob_owners.contains(&bob),
        "the in-flight writer's row must be backfilled into the new index"
    );
    let alice_owners = tbl
        .lookup_by_index("name_idx", &[InnerValue::Str("Alice".into())])
        .await
        .unwrap();
    assert_eq!(
        alice_owners.len(),
        1,
        "the pre-existing row must be backfilled into the new index"
    );
}

// ============================================================================
// 2. Unique index — `create_unique_index` drain proof (FIRST unique index)
// ============================================================================

/// F-57: `create_unique_index` must drain an in-flight fast-path writer before
/// its snapshot. This SPECIFICALLY tests the "first unique index on this table"
/// case (`has_unique_indexes()` is `false` going in) — the exact scenario the
/// review flagged as bypassed: pre-fix, `needs_write_barrier()` consulted
/// `has_unique_indexes()` which was `false`, so every concurrent fast-path
/// writer bypassed `unique_write_lock` entirely, and the create's lock was
/// useless against them. Post-fix, `unique_index_create_barrier` forces
/// `needs_write_barrier()` to `true` during the create so the drain catches
/// in-flight writers.
#[tokio::test]
async fn f57_create_unique_index_drains_inflight_fast_path_writer_first_unique() {
    use crate::table::table_manager_crud::{
        PostBarrierPreWriteHook, TEST_POST_BARRIER_PRE_WRITE_HOOK,
    };
    use tokio::sync::Notify;

    let repo = make_repo();
    repo.add_table(TableConfig::new("people"));
    let tbl = repo.get_table("people").await.unwrap();
    let email_field = key_id(&tbl, "email").await;

    // One pre-existing row so the backfill has something to stream.
    let _alice = tbl
        .insert(&record_with_str(email_field, "alice@example.com"))
        .await
        .unwrap();

    // CRITICAL PRECONDITION: this is the FIRST unique index on this table —
    // `has_unique_indexes()` is `false`. Pre-fix, every concurrent fast-path
    // writer read `needs_write_barrier() == false` and bypassed
    // `unique_write_lock` entirely, even though `create_unique_index` held it.
    assert!(
        !tbl.index_manager_ref().has_unique_indexes(),
        "precondition: no unique indexes yet — this is the first unique-index \
         create, the exact scenario the review flagged as bypassed"
    );
    assert!(
        !tbl.needs_write_barrier(),
        "precondition: no barrier active → lock-free fast path"
    );

    // Install the F-48 test seam (same shape as the regular-hash test above).
    let hook = Arc::new(PostBarrierPreWriteHook {
        reached: std::sync::atomic::AtomicUsize::new(0),
        resume: Notify::new(),
        armed: std::sync::atomic::AtomicBool::new(true),
    });
    TEST_POST_BARRIER_PRE_WRITE_HOOK
        .set(Arc::clone(&hook))
        .expect("hook installed once per test process");

    // 1. Spawn the writer. It enters the drain set, reads
    //    needs_write_barrier() == false, and parks — BEFORE its write.
    let tbl_w = tbl.clone();
    let writer = tokio::spawn(async move {
        tbl_w
            .insert(&record_with_str(email_field, "bob@example.com"))
            .await
    });

    while hook.reached.load(std::sync::atomic::Ordering::SeqCst) == 0 {
        tokio::task::yield_now().await;
    }
    assert_eq!(
        tbl.count().await.unwrap(),
        1,
        "writer parked before its write — only the pre-existing row is present"
    );

    // 2. Spawn the create. Pre-fix it takes unique_write_lock (uncontended),
    //    but `create_unique_index_locked` does NOT drain — the create completes
    //    while the writer is parked. Post-fix it raises
    //    `unique_index_create_barrier` and drains.
    let tbl_c = tbl.clone();
    let create =
        tokio::spawn(async move { tbl_c.create_unique_index("email_uniq", &["email"]).await });

    tokio::time::sleep(Duration::from_millis(80)).await;

    // 3. THE PROOF. The create must be blocked in drain_writers().
    //    - UNFIXED: create finished (no drain in _locked) → FAILS.
    //    - FIXED: create blocked at drain_writers() → PASSES.
    assert!(
        !create.is_finished(),
        "F-57: create_unique_index must drain the in-flight fast-path writer \
         even for the FIRST unique index (has_unique_indexes()==false going \
         in). If this fails, the unique_index_create_barrier is missing from \
         create_unique_index_locked — the first-unique-index race the review \
         flagged is still open."
    );

    // 4. Release the writer.
    hook.resume.notify_one();
    let bob = writer
        .await
        .unwrap()
        .expect("writer must complete once released");

    // 5. Create completes.
    create
        .await
        .unwrap()
        .expect("create must complete once the writer finishes");

    // Sanity: the unique index is registered.
    assert!(
        tbl.unique_index_exists("email_uniq").await,
        "unique index must be registered after create"
    );

    // Sanity: the writer's row was backfilled — a duplicate insert must now be
    // rejected. If the backfill had missed the writer's row (the lost-write
    // race), the duplicate would be accepted, violating the uniqueness
    // guarantee the index exists to enforce.
    let dup_result = tbl
        .insert(&record_with_str(email_field, "bob@example.com"))
        .await;
    assert!(
        dup_result.is_err(),
        "the writer's row must be in the unique index — a duplicate insert \
         must be rejected. If this passes, the backfill missed the writer's \
         row (the lost-write race), and the uniqueness guarantee is violated."
    );
    // The duplicate insert must not have persisted (unique validation rejected
    // it before the data write).
    let _ = bob; // keep the writer's rid alive for clarity
}

// ============================================================================
// 3. Sorted index — `create_sorted_index_with_include` drain proof
// ============================================================================

/// F-57: `create_sorted_index_with_include` must drain an in-flight fast-path
/// writer before its register→backfill sequence. Pre-fix, sorted index create
/// had NO write barrier of any kind — a concurrent writer could interleave with
/// the backfill on a registered-but-incomplete index. Post-fix it acquires
/// `unique_write_lock`, raises `sorted_index_create_barrier`, and drains.
///
/// Sorted indexes register BEFORE backfill (load-bearing: the backfill calls
/// `on_record_created` through the manager, which needs the registered def).
/// The barrier+lock+drain ensures no concurrent writer interleaves with the
/// backfill — the snapshot is a true point-in-time view.
#[tokio::test]
async fn f57_create_sorted_index_drains_inflight_fast_path_writer() {
    use crate::table::table_manager_crud::{
        PostBarrierPreWriteHook, TEST_POST_BARRIER_PRE_WRITE_HOOK,
    };
    use tokio::sync::Notify;

    let repo = make_repo();
    repo.add_table(TableConfig::new("nums"));
    let tbl = repo.get_table("nums").await.unwrap();
    let val_field = key_id(&tbl, "val").await;

    // One pre-existing row so the backfill has something to stream.
    let _pre = tbl
        .insert(&record_with_str(val_field, "100"))
        .await
        .unwrap();

    assert!(
        !tbl.needs_write_barrier(),
        "precondition: no barrier active → lock-free fast path"
    );

    let hook = Arc::new(PostBarrierPreWriteHook {
        reached: std::sync::atomic::AtomicUsize::new(0),
        resume: Notify::new(),
        armed: std::sync::atomic::AtomicBool::new(true),
    });
    TEST_POST_BARRIER_PRE_WRITE_HOOK
        .set(Arc::clone(&hook))
        .expect("hook installed once per test process");

    // 1. Spawn the writer.
    let tbl_w = tbl.clone();
    let writer = tokio::spawn(async move { tbl_w.insert(&record_with_str(val_field, "42")).await });

    while hook.reached.load(std::sync::atomic::Ordering::SeqCst) == 0 {
        tokio::task::yield_now().await;
    }
    assert_eq!(
        tbl.count().await.unwrap(),
        1,
        "writer parked before its write — only the pre-existing row is present"
    );

    // 2. Spawn the create.
    let tbl_c = tbl.clone();
    let create =
        tokio::spawn(async move { tbl_c.create_sorted_index("val_sorted", &["val"]).await });

    tokio::time::sleep(Duration::from_millis(80)).await;

    // 3. THE PROOF.
    assert!(
        !create.is_finished(),
        "F-57: create_sorted_index must drain the in-flight fast-path writer \
         before proceeding to register + backfill. If this fails, the sorted \
         index create has no drain — a concurrent writer can interleave with \
         the backfill on a registered-but-incomplete index."
    );

    // 4. Release the writer.
    hook.resume.notify_one();
    let bob = writer
        .await
        .unwrap()
        .expect("writer must complete once released");

    // 5. Create completes.
    create
        .await
        .unwrap()
        .expect("create must complete once the writer finishes");

    // Sanity: the writer's row must be in the new sorted index. Scan the def's
    // physical entry prefix directly (mirrors f50_step2's pattern).
    let name_interned = key_id(&tbl, "val_sorted").await;
    let prefix = tbl
        .info_store()
        .scan_prefix_stream(bytes::Bytes::from(sorted_entry_prefix(name_interned)), 1000);
    futures::pin_mut!(prefix);
    let mut found_rids: Vec<[u8; 16]> = Vec::new();
    while let Some(batch) = futures::StreamExt::next(&mut prefix).await {
        for (key_bytes, _) in batch.unwrap() {
            // Sorted entry keys are `<tag><name_interned_be><encoded_value><rid>`;
            // the trailing 16 bytes are the rid.
            if key_bytes.len() >= 16 {
                let mut rid = [0u8; 16];
                rid.copy_from_slice(&key_bytes[key_bytes.len() - 16..]);
                found_rids.push(rid);
            }
        }
    }
    assert!(
        found_rids.iter().any(|r| r == bob.as_bytes()),
        "the in-flight writer's row must be in the new sorted index (either \
         via backfill or the live write hook). Found rids: {:?}, expected to \
         contain {:?}",
        found_rids,
        bob
    );
}

/// Build the entry-key prefix for a sorted index def. Mirrors
/// `SortedIndexManager::entry_prefix`: `SORTED_TAG (0x80) + name_interned_be`.
fn sorted_entry_prefix(name_interned: u64) -> Vec<u8> {
    let mut buf = Vec::with_capacity(9);
    buf.push(0x80); // SORTED_TAG
    buf.extend_from_slice(&name_interned.to_be_bytes());
    buf
}
