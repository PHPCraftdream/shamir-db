//! F-37 (#845, P0) — `schema_activation_barrier` write-barrier tests.
//!
//! Sibling of `index2_create_barrier_tests.rs` (#534). That module proved the
//! `index2_create_barrier` flag closes the lost-write race for `create_index_v2`.
//! This module proves the F-37 sibling flag (`schema_activation_barrier`) closes
//! the `keyset_safe` count-proof race for schema-activation DDL
//! (`set_table_schema` / `add_schema_rule`): while the flag is up (raised under
//! `unique_write_lock`), every non-tx writer consulting
//! [`needs_write_barrier`](crate::table::TableManager::needs_write_barrier)
//! returns `true` and serializes on the same `unique_write_lock` — so no
//! INSERT/UPDATE can land between the `count() == 0` proof and the schema's
//! persist+activate.
//!
//! ## Determinism
//!
//! Mirrors the established Notify-handshake style of this codebase's
//! race-closure tests (`fk_reverse_cache_race_tests.rs`, the
//! `BackfillPauseHook` rendezvous in `index2_create_barrier_tests.rs`): a
//! "barrier holder" task acquires the lock + raises the flag, then signals a
//! `Notify` (`barrier_up`) so the test KNOWS the barrier is engaged before it
//! spawns a writer — no race-window guesswork. The blocked-writer check then
//! uses the sibling `index2_create_barrier_tests.rs` `is_finished()` probe: it
//! is NOT timing-dependent for correctness (the writer structurally cannot
//! complete while the lock is held), the brief poll only lets the scheduler
//! park the blocked task before the assertion.

use std::sync::Arc;
use std::time::Duration;

use shamir_types::core::interner::{InternerKey, TouchInd};
use shamir_types::types::common::new_map_wc;
use shamir_types::types::value::InnerValue;
use tokio::sync::Notify;

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
// THE F-37 PROOF — a concurrent INSERT BLOCKS on the schema-activation barrier
// until the DDL-shaped holder releases it, and ONLY then completes.
//
// Determinism: the "barrier holder" task (standing in for the in-flight DDL's
// count→persist→activate window) acquires `unique_write_lock`, raises
// `schema_activation_barrier`, and signals `barrier_up` — a rendezvous that
// tells the test the barrier is engaged. The test then spawns an INSERT; with
// the barrier up it MUST take the lock, which the holder still owns, so it
// parks. Releasing the holder clears the flag + drops the lock, unblocking the
// insert.
//
// Pre-fix (no `schema_activation_barrier` term in `needs_write_barrier`): the
// table has no unique index and no index2 create in flight, so
// `needs_write_barrier()` returns `false` and the insert takes the LOCK-FREE
// fast path — it completes immediately even though the holder "holds the lock",
// proving the window was open. Post-fix the insert blocks until release.
// ============================================================================

#[tokio::test]
async fn insert_blocks_on_schema_activation_barrier_until_released() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("people"));
    let tbl = repo.get_table("people").await.unwrap();
    let name_field = key_id(&tbl, "name").await;

    // Sanity: this table has no unique index and no index2 create in flight, so
    // the pre-fix `needs_write_barrier()` is `false` — the lock-free fast path.
    // (Post-fix it is `true` ONLY because the barrier flag is up.)
    assert!(
        !tbl.needs_write_barrier(),
        "precondition: with the barrier down the table must be on the lock-free path"
    );

    let barrier_up = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());

    // "Barrier holder" — stands in for the in-flight schema DDL's
    // count→persist→activate window (which holds `unique_write_lock` and raises
    // `schema_activation_barrier`). Mirrors the real DDL sequence in
    // `admin_schema.rs::begin_schema_activation_barrier` +
    // `SchemaActivationBarrierGuard`.
    let tbl_holder = tbl.clone();
    let barrier_up_h = Arc::clone(&barrier_up);
    let release_h = Arc::clone(&release);
    let holder = tokio::spawn(async move {
        let _uwl_guard = tbl_holder.unique_write_lock().lock_owned().await;
        let _barrier = SchemaBarrierFlag::raise(&tbl_holder);
        // Engaged — tell the test it can now fire a writer.
        barrier_up_h.notify_one();
        // Park here for the DDL's persist+activate window.
        release_h.notified().await;
        // `_barrier` clears the flag, `_uwl_guard` releases the lock on drop.
    });

    // Rendezvous: the barrier is now engaged (flag up + lock held).
    barrier_up.notified().await;
    assert!(
        tbl.needs_write_barrier(),
        "post-fix: needs_write_barrier() must be true while schema_activation_barrier is up"
    );

    // Fire a concurrent INSERT. With the barrier up it MUST take
    // `unique_write_lock`, which the holder owns — so it parks.
    let tbl_insert = tbl.clone();
    let insert =
        tokio::spawn(async move { tbl_insert.insert(&record_with_str(name_field, "Bob")).await });

    // Give the insert task time to reach — and (post-fix) block on — the lock.
    // Robust: the insert structurally cannot complete while the holder owns the
    // lock, so this poll is not timing-sensitive (mirrors the sibling
    // `index2_create_barrier_tests` probe).
    tokio::time::sleep(Duration::from_millis(80)).await;
    assert!(
        !insert.is_finished(),
        "post-fix (F-37): the concurrent insert must BLOCK on the \
         schema_activation_barrier held by the in-flight DDL-shaped holder \
         (pre-fix it completes immediately on the lock-free path, proving the \
         keyset_safe count-proof race window was open)"
    );

    // Release the holder — it clears the flag and drops the lock.
    release.notify_one();
    holder.await.unwrap();

    // The insert can now acquire the lock and complete.
    let bob = insert
        .await
        .unwrap()
        .expect("insert must succeed once released");
    assert!(tbl.get(bob).await.is_ok(), "row must be physically present");
}

// ============================================================================
// Negative control: WITHOUT the barrier flag raised (and no unique index / no
// index2 create), a writer does NOT block on `unique_write_lock` — it takes the
// lock-free fast path. Holding the lock externally therefore does NOT gate the
// writer. This proves the NEW flag (not the lock alone) is what engages the
// barrier for schema activation: the lock is only consulted when
// `needs_write_barrier()` is true.
// ============================================================================

#[tokio::test]
async fn without_barrier_flag_writer_takes_lock_free_path() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("people"));
    let tbl = repo.get_table("people").await.unwrap();
    let name_field = key_id(&tbl, "name").await;

    // Hold `unique_write_lock` externally WITHOUT raising the barrier flag.
    let _held = tbl.unique_write_lock().lock_owned().await;
    assert!(
        !tbl.needs_write_barrier(),
        "no unique index, no index2 create, barrier down → lock-free fast path"
    );

    // The insert must complete immediately — it never consults the lock we hold
    // (fast path), proving the barrier FLAG is the gate, not the lock alone.
    let rid = tokio::time::timeout(
        Duration::from_secs(2),
        tbl.insert(&record_with_str(name_field, "Carol")),
    )
    .await
    .expect("insert must NOT block when the barrier flag is down (lock-free fast path)")
    .expect("insert must succeed");

    assert!(tbl.get(rid).await.is_ok(), "row must be present");
    drop(_held);
}

// ============================================================================
// F-48 (#859, P0) — the check-then-act DRAIN proof.
//
// The two tests above prove the barrier FLAG engages (a writer blocks once
// the flag is already up). This test proves the DRAIN: a writer that read
// `needs_write_barrier() == false` a moment BEFORE the DDL raised the flag,
// and is still in its validate→write→index sequence, is genuinely waited for
// — the DDL's count-proof does NOT proceed until that writer has completed.
//
// Without the drain (check-then-act): the writer reads false and proceeds
// lock-free; the DDL raises the flag + takes the lock + reads count()==0
// (the writer hasn't written yet); the writer then completes its write; the
// DDL has stamped keyset_safe=true over a table whose row history was never
// proven homogeneous. This test deterministically reproduces that exact
// interleaving via a test-only pause seam (mirrors F-46/F-47) and proves the
// F-48 drain closes it: post-fix the DDL's drain blocks until the writer
// finishes, so the count-proof sees the in-flight writer's row.
// ============================================================================

#[tokio::test]
async fn f48_drain_catches_writer_that_read_false_before_flag_went_up() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("people"));
    let tbl = repo.get_table("people").await.unwrap();
    let name_field = key_id(&tbl, "name").await;

    // Precondition: no barrier active → lock-free fast path.
    assert!(
        !tbl.needs_write_barrier(),
        "precondition: barrier down → lock-free path"
    );

    // Install the F-48 test seam (mirrors F-46's
    // `TEST_POST_VALIDATE_PRE_PUBLISH_HOOK` / F-47's
    // `TEST_POST_GENCHECK_PRE_PUBLISH_HOOK`): one-shot `armed` CAS so only the
    // FIRST writer to reach the seam parks; `reached` for busy-poll detection
    // (no sleeps); `resume` for the release signal. nextest isolates this
    // global per test process.
    use crate::table::table_manager_crud::{
        PostBarrierPreWriteHook, TEST_POST_BARRIER_PRE_WRITE_HOOK,
    };
    let hook = Arc::new(PostBarrierPreWriteHook {
        reached: std::sync::atomic::AtomicUsize::new(0),
        resume: Notify::new(),
        armed: std::sync::atomic::AtomicBool::new(true),
    });
    TEST_POST_BARRIER_PRE_WRITE_HOOK
        .set(Arc::clone(&hook))
        .expect("hook installed once per test process");

    // 1. Spawn the writer. It reads needs_write_barrier() == false, enters
    //    the drain set (post-fix), and parks at the seam — BEFORE its write.
    let tbl_w = tbl.clone();
    let writer =
        tokio::spawn(async move { tbl_w.insert(&record_with_str(name_field, "Bob")).await });

    // Rendezvous: wait until the writer has actually parked at the seam
    // (busy-poll `reached`, no sleeps — same handshake as F-46/F-47).
    while hook.reached.load(std::sync::atomic::Ordering::SeqCst) == 0 {
        tokio::task::yield_now().await;
    }
    // The writer has read false and is parked. It has NOT written.
    assert_eq!(
        tbl.count().await.unwrap(),
        0,
        "writer parked before its write — table must be empty"
    );

    // 2. The DDL raises the barrier + takes the lock + drains (post-fix) +
    //    reads its count-proof. We run the DDL's critical section as a task
    //    and receive the count result via a oneshot channel.
    //
    //    - UNFIXED code: `drain_writers()` exists but no CRUD site calls
    //      `enter_writer()`, so the active counter is always 0 → drain returns
    //      immediately → count reads 0 while the writer is still parked.
    //    - FIXED code: the writer called `enter_writer()` before parking, so
    //      the drain blocks here until we release the writer (step 3).
    let tbl_d = tbl.clone();
    let (proof_tx, proof_rx) = tokio::sync::oneshot::channel::<usize>();
    let ddl = tokio::spawn(async move {
        let _uwl = tbl_d.unique_write_lock().lock_owned().await;
        let _barrier = SchemaBarrierFlag::raise(&tbl_d);
        // F-48 drain: wait for in-flight fast-path writers.
        tbl_d.drain_writers().await;
        let count_at_proof = tbl_d.count().await.unwrap();
        let _ = proof_tx.send(count_at_proof);
    });

    // Give the DDL enough time to acquire the lock, raise the flag, and
    // (post-fix) enter the drain loop. On the unfixed code the DDL completes
    // its count-proof in well under this window.
    tokio::time::sleep(Duration::from_millis(80)).await;

    // 3. Release the parked writer. It completes its data-store write +
    //    record-counter bump + index updates, then exits the drain set
    //    (post-fix: the `WriterDrainGuard` drops).
    hook.resume.notify_one();
    let bob = writer
        .await
        .unwrap()
        .expect("insert must succeed once released");
    assert!(tbl.get(bob).await.is_ok(), "row must be present");

    // 4. The DDL finishes (on the fixed code, the drain returned after the
    //    writer exited the drain set; on the unfixed code, it finished long
    //    ago with count==0).
    ddl.await.unwrap();
    let count_at_proof = proof_rx.await.unwrap();

    // 5. THE PROOF.
    //    - UNFIXED (check-then-act): count_at_proof == 0 — the DDL proved the
    //      table empty while a lock-free writer was still in flight, then the
    //      writer landed → keyset_safe proof violated. FAIL here.
    //    - FIXED (drain): count_at_proof == 1 — the drain waited for the
    //      in-flight writer; the count-proof saw its row. PASS.
    assert_eq!(
        count_at_proof, 1,
        "F-48 drain: the DDL's count-proof must observe the in-flight writer's \
         row. Got {count_at_proof} = the check-then-act race (DDL proved \
         count()==0 while a lock-free writer was parked mid-flight, then the \
         writer landed — keyset_safe proof violated)."
    );
}

// ============================================================================
// RAII helper mirroring admin_schema's `SchemaActivationBarrierGuard`: raise the
// flag on construction, clear on drop. Local to this test module so the test
// drives the EXACT same set/clear discipline the production DDL uses.
// ============================================================================

struct SchemaBarrierFlag<'a> {
    table: &'a TableManager,
}

impl<'a> SchemaBarrierFlag<'a> {
    fn raise(table: &'a TableManager) -> Self {
        table.set_schema_activation_barrier(true);
        Self { table }
    }
}

impl Drop for SchemaBarrierFlag<'_> {
    fn drop(&mut self) {
        self.table.set_schema_activation_barrier(false);
    }
}
