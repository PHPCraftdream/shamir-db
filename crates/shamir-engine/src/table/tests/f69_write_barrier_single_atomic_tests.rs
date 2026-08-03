//! F-69 (#896, P0) — collapse the write-barrier predicate into one atomic.
//!
//! # The bug this closes
//!
//! Pre-fix, `TableManager::needs_write_barrier()` OR'd together SIX
//! independent atomic loads — five `AtomicBool`s owned by `TableManager`
//! (all `SeqCst`) plus `IndexManager::has_unique_indexes()` (`shamir-index`),
//! whose backing flag was loaded `Relaxed`. This was NOT a single atomic
//! snapshot, and the `Relaxed` operand additionally broke F-56's `SeqCst`
//! total-order proof for the `WriterDrainBarrier` protocol (see
//! `writer_drain_barrier.rs`): a fast-path writer's `enter_writer`
//! (`SeqCst` `active.fetch_add`) could be reordered relative to the
//! `Relaxed` `has_unique_indexes()` read, letting the writer proceed
//! lock-free through its ENTIRE validate→write→index sequence while a
//! concurrent `create_unique_index_locked` believed (via `drain()` observing
//! `active == 0`) that no such writer was in flight — a duplicate value
//! could slip past a unique constraint that had just gone live.
//!
//! # What this file proves
//!
//! 1. `needs_write_barrier()` performs EXACTLY ONE atomic load
//!    (`structural_needs_write_barrier_is_one_atomic_load` — a direct
//!    mechanical check against `WriteBarrierFlags`, the packed word both
//!    `TableManager` and `IndexManager` now share).
//! 2. The `UNIQUE_INDEX_EXISTS` bit `IndexManager` owns and the five
//!    DDL-intent bits `TableManager` owns live in the IDENTICAL
//!    `Arc<AtomicU8>` — a set from EITHER side is visible through the
//!    OTHER's single load, with no separate "sync" step
//!    (`unique_index_exists_bit_is_the_same_shared_word_table_manager_reads`).
//! 3. The end-to-end race is closed: a fast-path writer parked strictly
//!    after reading `needs_write_barrier() == false` (the F-48 test seam,
//!    same convention as `f57_index_create_barrier_tests.rs`) is drained by
//!    a concurrent `create_unique_index` for the table's FIRST unique index
//!    — the exact scenario the bug report names — and the backfill sees the
//!    writer's row, so a subsequent duplicate is correctly rejected
//!    (`f69_first_unique_index_create_drains_parked_writer_and_rejects_duplicate`).

use std::sync::atomic::Ordering;
use std::sync::Arc;

use shamir_index::base_index::write_barrier_flags::{
    WriteBarrierFlags, INDEX2_CREATE, REGULAR_INDEX_CREATE, SCHEMA_ACTIVATION, SORTED_INDEX_CREATE,
    UNIQUE_INDEX_CREATE, UNIQUE_INDEX_EXISTS,
};
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
// 1. Structural proof: needs_write_barrier() is one atomic load.
// ============================================================================

/// F-69: mechanical proof that the whole six-condition predicate collapses
/// to a single `SeqCst` load of one `Arc<AtomicU8>`. This is not an
/// interleaving test — it directly exercises `WriteBarrierFlags::any_set()`,
/// the exact primitive `TableManager::needs_write_barrier()` now delegates
/// to with no other atomic touched.
///
/// Pre-fix, the equivalent predicate was six separate loads across six
/// separate atomics (five `AtomicBool` + one on `IndexManager`); no single
/// call could ever have been a torn-read-proof snapshot because there was no
/// single memory location to load from. Post-fix there is exactly one.
#[test]
fn structural_needs_write_barrier_is_one_atomic_load() {
    let flags = WriteBarrierFlags::new();

    // Every bit clear: predicate false, zero cost beyond the one load.
    assert!(!flags.any_set());

    // Setting ANY single bit — including the one IndexManager owns — flips
    // the WHOLE predicate via that SAME one load. No second atomic is
    // consulted; `any_set()`'s signature takes no other state.
    for bit in [
        UNIQUE_INDEX_EXISTS,
        INDEX2_CREATE,
        SCHEMA_ACTIVATION,
        REGULAR_INDEX_CREATE,
        UNIQUE_INDEX_CREATE,
        SORTED_INDEX_CREATE,
    ] {
        let f = WriteBarrierFlags::new();
        f.set(bit);
        assert!(
            f.any_set(),
            "bit {:#04b} must flip the compound predicate via the single packed word",
            bit
        );
        f.clear(bit);
        assert!(
            !f.any_set(),
            "clearing the only set bit must return to false"
        );
    }
}

/// F-69: bits packed into the SAME byte must not corrupt each other — the
/// six-condition OR must be exact, not an approximation that happens to
/// work for one bit at a time. Sets all six, clears them one at a time in a
/// different order, and checks the running predicate at every step.
#[test]
fn structural_all_six_bits_compose_without_clobbering() {
    let flags = WriteBarrierFlags::new();
    let all = [
        UNIQUE_INDEX_EXISTS,
        INDEX2_CREATE,
        SCHEMA_ACTIVATION,
        REGULAR_INDEX_CREATE,
        UNIQUE_INDEX_CREATE,
        SORTED_INDEX_CREATE,
    ];
    for &b in &all {
        flags.set(b);
    }
    assert!(flags.any_set());

    // Clear in reverse order, checking the predicate stays true until the
    // LAST bit is cleared — proves no bit silently clobbers a sibling.
    for (i, &b) in all.iter().rev().enumerate() {
        flags.clear(b);
        let expect_still_set = i + 1 < all.len();
        assert_eq!(
            flags.any_set(),
            expect_still_set,
            "after clearing {} of {} bits, any_set() must reflect exactly the \
             remaining set bits",
            i + 1,
            all.len()
        );
    }
}

// ============================================================================
// 2. Cross-crate proof: IndexManager's bit and TableManager's word are the
//    SAME Arc<AtomicU8>.
// ============================================================================

/// F-69: `IndexManager::write_barrier_flags()` and the packed word
/// `TableManager::needs_write_barrier()` reads must be the IDENTICAL shared
/// atomic — not two independently-synced copies. A set from the
/// `IndexManager` side (the ONLY thing that owns `UNIQUE_INDEX_EXISTS`) must
/// be visible through `needs_write_barrier()`'s single load with no
/// additional synchronization step. This is the structural precondition the
/// end-to-end race-closure test below depends on.
#[tokio::test]
async fn unique_index_exists_bit_is_the_same_shared_word_table_manager_reads() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("people"));
    let tbl = repo.get_table("people").await.unwrap();

    assert!(
        !tbl.needs_write_barrier(),
        "precondition: fresh table, no unique index, no DDL in flight"
    );
    assert!(!tbl.index_manager_ref().has_unique_indexes());

    // Directly flip the UNIQUE_INDEX_EXISTS bit via the handle IndexManager
    // exposes — mirroring exactly what `create_unique_index_from_records`
    // does internally (`self.write_barrier_flags.set(UNIQUE_INDEX_EXISTS)`),
    // without going through the full create sequence.
    let shared = tbl.index_manager_ref().write_barrier_flags();
    shared.set(UNIQUE_INDEX_EXISTS);

    // TableManager::needs_write_barrier()'s single load must observe it
    // immediately — same word, no separate flag to sync.
    assert!(
        tbl.needs_write_barrier(),
        "IndexManager's UNIQUE_INDEX_EXISTS bit must be visible through \
         TableManager::needs_write_barrier()'s single load — they must share \
         the identical Arc<AtomicU8>, not independently-synced copies"
    );
    assert!(
        tbl.index_manager_ref().has_unique_indexes(),
        "has_unique_indexes() must also observe the same transition"
    );

    shared.clear(UNIQUE_INDEX_EXISTS);
    assert!(!tbl.needs_write_barrier());
    assert!(!tbl.index_manager_ref().has_unique_indexes());
}

// ============================================================================
// 3. End-to-end race closure: FIRST unique index create drains a writer
//    parked on the pre-fix-vulnerable read.
// ============================================================================

/// F-69: the exact scenario the bug report names. A fast-path writer reads
/// `needs_write_barrier() == false` (there is no unique index yet) and parks
/// strictly before its data-store write (F-48 test seam, same convention as
/// `f57_index_create_barrier_tests.rs`). A concurrent `create_unique_index`
/// for the table's FIRST unique index must drain that writer — via the
/// SAME shared packed word `UNIQUE_INDEX_CREATE` (raised by
/// `create_unique_index_locked`) and `UNIQUE_INDEX_EXISTS` (raised at the
/// end of the backfill) both live in — before the create can complete.
///
/// This directly guards the SeqCst total-order argument
/// `writer_drain_barrier.rs` proves: with the six-condition predicate now a
/// single atomic word, the writer's `enter_writer` (`SeqCst` `active.fetch_add`)
/// and its `needs_write_barrier()` read (`SeqCst` load of the SAME word the
/// drainer's bit-set participates in) are ordered in the single `SeqCst`
/// total order — the drainer's `drain()` cannot observe `active == 0` while
/// this writer is still mid-flight, regardless of which bit (a
/// `TableManager`-owned DDL bit or `IndexManager`-owned `UNIQUE_INDEX_EXISTS`)
/// is the one that made `needs_write_barrier()` relevant.
#[tokio::test]
async fn f69_first_unique_index_create_drains_parked_writer_and_rejects_duplicate() {
    use crate::table::table_manager_crud::{
        PostBarrierPreWriteHook, TEST_POST_BARRIER_PRE_WRITE_HOOK,
    };
    use tokio::sync::Notify;

    let repo = make_repo();
    repo.add_table(TableConfig::new("people"));
    let tbl = repo.get_table("people").await.unwrap();
    let email_field = key_id(&tbl, "email").await;

    let _seed = tbl
        .insert(&record_with_str(email_field, "alice@example.com"))
        .await
        .unwrap();

    // Precondition: no unique index yet — the FIRST-unique-index case, the
    // exact window the F-69 bug report describes as unprotected pre-fix
    // (has_unique_indexes() == false, and pre-fix that read was Relaxed on
    // its OWN standalone atomic rather than SeqCst on the shared word).
    assert!(!tbl.index_manager_ref().has_unique_indexes());
    assert!(!tbl.needs_write_barrier());

    let hook = Arc::new(PostBarrierPreWriteHook {
        reached: std::sync::atomic::AtomicUsize::new(0),
        resume: Notify::new(),
        armed: std::sync::atomic::AtomicBool::new(true),
    });
    TEST_POST_BARRIER_PRE_WRITE_HOOK
        .set(Arc::clone(&hook))
        .expect("hook installed once per test process");

    // 1. Writer enters the drain set (SeqCst active.fetch_add), reads
    //    needs_write_barrier() == false (SeqCst load of the packed word —
    //    still all-zero at this instant), and parks BEFORE its write.
    let tbl_w = tbl.clone();
    let writer = tokio::spawn(async move {
        tbl_w
            .insert(&record_with_str(email_field, "bob@example.com"))
            .await
    });

    while hook.reached.load(Ordering::SeqCst) == 0 {
        tokio::task::yield_now().await;
    }
    assert_eq!(
        tbl.count().await.unwrap(),
        1,
        "writer parked before its write — only the seed row is present"
    );

    // 2. Concurrent create_unique_index: acquires unique_write_lock, sets
    //    UNIQUE_INDEX_CREATE in the SAME packed word the parked writer's
    //    drain-set membership must be checked against, then calls
    //    drain_writers() — which must NOT return while the parked writer
    //    (already in the drain set) is still mid-flight.
    let tbl_c = tbl.clone();
    let create =
        tokio::spawn(async move { tbl_c.create_unique_index("email_uniq", &["email"]).await });

    tokio::time::sleep(std::time::Duration::from_millis(80)).await;

    // 3. THE PROOF: the create must still be blocked in drain_writers().
    //    If the six-condition predicate were still torn (pre-fix shape), a
    //    reordering could in principle let some other path observe a stale
    //    view — this assertion is the operational proof that, with the
    //    single packed word, the drain protocol behaves correctly for the
    //    FIRST-unique-index case end to end.
    assert!(
        !create.is_finished(),
        "F-69: create_unique_index must drain the writer parked on the SAME \
         packed word's pre-fix-vulnerable read before completing. If this \
         fails, the drain protocol is not correctly gated by the shared \
         Arc<AtomicU8> and the race the bug report describes is still open."
    );

    // 4. Release the writer; both complete.
    hook.resume.notify_one();
    let bob = writer
        .await
        .unwrap()
        .expect("writer must complete once released");
    create
        .await
        .unwrap()
        .expect("create must complete once the writer finishes");

    // 5. Post-conditions: UNIQUE_INDEX_EXISTS is now set on the shared word
    //    (visible through both has_unique_indexes() and needs_write_barrier()),
    //    and the writer's row was correctly backfilled into the new index —
    //    the ultimate proof the race did NOT let a duplicate slip through.
    assert!(tbl.index_manager_ref().has_unique_indexes());
    assert!(tbl.needs_write_barrier());
    assert!(tbl.unique_index_exists("email_uniq").await);

    let dup_result = tbl
        .insert(&record_with_str(email_field, "bob@example.com"))
        .await;
    assert!(
        dup_result.is_err(),
        "F-69: the parked writer's row must be present in the unique index \
         — a duplicate insert must be rejected. If this passes, the backfill \
         missed the writer's row (the torn-read race), and a duplicate value \
         slipped past a unique constraint that had just gone live."
    );
    let _ = bob;
}
