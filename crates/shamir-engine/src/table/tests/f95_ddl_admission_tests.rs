//! F-95 (#957, P0) — DDL admission mutex: serialize concurrent DDL ops
//! sharing the same write-barrier bit.
//!
//! # The bug this closes
//!
//! `WriteBarrierFlags::set`/`clear` are bare `fetch_or`/`fetch_and` on a
//! shared `AtomicU8` — there is NO reference count. Two concurrent DDL
//! operations of the same family (e.g. two `CREATE INDEX` on the same
//! table, both raising `REGULAR_INDEX_CREATE`) race:
//!
//! 1. DDL-A calls `begin_write_barrier(BIT)`: sets the bit, drains
//!    in-flight writers, takes `unique_write_lock`.
//! 2. DDL-B calls `begin_write_barrier(BIT)` concurrently: `fetch_or` on
//!    an already-set bit is a no-op; B also drains (cheap), then blocks
//!    on `unique_write_lock` (held by A).
//! 3. A finishes its backfill and drops its `WriteBarrierGuard` →
//!    unconditional `clear(BIT)`. The bit is now 0, even though B's DDL
//!    is about to start its own backfill.
//! 4. B acquires `unique_write_lock` (A released it). B's own guard never
//!    re-raises the bit (it was "set" from B's perspective in step 2, but
//!    that was a no-op `fetch_or` and the bit is currently 0).
//! 5. A brand-new writer calls `needs_write_barrier()`, sees `false`,
//!    takes the fast (unlocked) path, and races B's in-progress
//!    backfill — the exact hazard the barrier exists to prevent. B
//!    already did its drain BEFORE the lock (step 2) based on the writer
//!    population at drain time; it will not drain again for writers that
//!    start now.
//!
//! # The fix
//!
//! A per-table `ddl_admission: Arc<tokio::sync::Mutex<()>>` is acquired
//! FIRST inside `begin_write_barrier`, before raise→drain→lock, and held
//! for the caller's ENTIRE critical section (the guard is embedded inside
//! the returned `WriteBarrierGuard`, dropped after the bit is cleared).
//! This ensures DDL-B does its OWN raise→drain→lock from scratch, AFTER
//! DDL-A's entire critical section (including guard drop) has completed.
//!
//! Regular writers (`needs_write_barrier()`) do NOT take this mutex — only
//! `begin_write_barrier` callers do — so the F-70 (#897) lock-order
//! invariant is preserved.
//!
//! # What this file proves
//!
//! 1. `f95_*_serializes_and_writer_sees_barrier` (one per DDL bit family —
//!    regular index, schema activation, unique index, sorted index,
//!    index2): Two concurrent DDL ops racing the SAME bit are serialized.
//!    DDL-A is paused mid-critical-section; DDL-B cannot proceed past its
//!    admission point until DDL-A fully completes (including
//!    `WriteBarrierGuard` drop). After DDL-B is admitted, a new writer
//!    observes `needs_write_barrier() == true` and cannot fast-path past
//!    B's in-progress work. Ordering is asserted via a shared event log.
//! 2. `f95_admission_mutex_does_not_reintroduce_f70_deadlock`: the
//!    admission mutex does not re-open the F-70 (#897) lock-order
//!    inversion cycle — the DDL can drain while holding `ddl_admission`
//!    because no writer or committer ever acquires it, so it cannot be a
//!    link in any lock-wait cycle.
//! 3. `f95_admission_mutex_is_per_table_not_global`: two DDLs on
//!    DIFFERENT tables proceed concurrently (each table has its own
//!    `ddl_admission`).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Notify;

use crate::index::write_barrier_flags::{
    INDEX2_CREATE, REGULAR_INDEX_CREATE, SCHEMA_ACTIVATION, SORTED_INDEX_CREATE,
    UNIQUE_INDEX_CREATE,
};
use crate::repo::repo_instance::RepoInstance;
use crate::repo::repo_types::BoxRepo;
use crate::table::TableConfig;
use crate::table::TableManager;
use shamir_storage::storage_in_memory::InMemoryRepo;

/// Bounded wait for tests that could hang on a bug — same convention as
/// F-70's `DEADLOCK_BOUND`.
const BOUND: Duration = Duration::from_secs(2);

/// Event-log step constants — pushed into a shared `EventLog` so the test
/// can assert the EXACT ordering of DDL-A and DDL-B's critical-section
/// transitions, not just "no panic".
const EV_A_ENTERED: u32 = 1;
const EV_A_DONE: u32 = 2;
const EV_B_ENTERED: u32 = 3;
const EV_B_DONE: u32 = 4;

fn make_repo() -> RepoInstance {
    let repo = Arc::new(InMemoryRepo::new());
    RepoInstance::new("test".into(), BoxRepo::InMemory(repo), Vec::new())
}

/// Build two tables (X = "orders", Y = "customers"), mirroring F-70's
/// `make_two_tables`. Y is only needed by the F-70 regression test.
async fn make_two_tables() -> (RepoInstance, TableManager, TableManager) {
    let repo = make_repo();
    repo.add_table(TableConfig::new("orders"));
    repo.add_table(TableConfig::new("customers"));
    let x = repo.get_table("orders").await.unwrap();
    let y = repo.get_table("customers").await.unwrap();
    (repo, x, y)
}

/// Shared synchronization + event-log state for the two-DDL serialization
/// tests. Uses the same Notify + AtomicBool + `wait_flag` convention as
/// `f70_lock_order_inversion_tests.rs` — every step advances only after
/// the previous one has provably happened, with no timing assumptions.
struct Sequencer {
    /// DDL-A signals this after `begin_write_barrier` returns (guards held,
    /// bit raised, inside its critical section).
    a_entered: AtomicBool,
    /// DDL-A signals this after dropping both guards (bit cleared,
    /// admission released, unique_write_lock released).
    a_done: AtomicBool,
    /// DDL-B signals this after `begin_write_barrier` returns (admission
    /// acquired, bit re-raised, inside its critical section).
    b_entered: AtomicBool,
    /// DDL-B signals this after dropping both guards.
    b_done: AtomicBool,
    /// Main test sets this to release DDL-A from its paused critical
    /// section.
    release_a: AtomicBool,
    /// Main test sets this to release DDL-B from its paused critical
    /// section.
    release_b: AtomicBool,
    /// Monotonic append-only event log — records the order of
    /// `EV_*` transitions for post-hoc ordering assertions.
    log: EventLog,
}

/// Append-only event log backed by a `std::sync::Mutex<Vec<u32>>`.
/// Justified: test-only, extremely low frequency (a handful of pushes per
/// test), and the lock is held for a trivial `push` — no contention
/// concern. Using atomics here (e.g. `AtomicUsize` step counter) would
/// only record the LAST event, not the full sequence, which is what the
/// brief requires ("assert the actual ordering … via a shared log/sequence
/// counter both DDL tasks append to").
struct EventLog(std::sync::Mutex<Vec<u32>>);

impl EventLog {
    fn new() -> Self {
        Self(std::sync::Mutex::new(Vec::new()))
    }

    fn record(&self, event: u32) {
        self.0.lock().unwrap().push(event);
    }

    fn snapshot(&self) -> Vec<u32> {
        self.0.lock().unwrap().clone()
    }
}

impl Sequencer {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            a_entered: AtomicBool::new(false),
            a_done: AtomicBool::new(false),
            b_entered: AtomicBool::new(false),
            b_done: AtomicBool::new(false),
            release_a: AtomicBool::new(false),
            release_b: AtomicBool::new(false),
            log: EventLog::new(),
        })
    }
}

/// Poll an `AtomicBool` flag, yielding between checks — same convention
/// as `f70_lock_order_inversion_tests.rs`. NOT sleep-based: in a
/// single-threaded `#[tokio::test]` runtime, `yield_now().await` gives
/// other spawned tasks a chance to run, making the interleaving
/// deterministic.
async fn wait_flag(flag: &AtomicBool) {
    while !flag.load(Ordering::SeqCst) {
        tokio::task::yield_now().await;
    }
}

/// DDL-A's shape: acquire the write barrier, record "entered", pause
/// (simulating mid-backfill), then drop guards and record "done".
async fn ddl_a(x: TableManager, bit: u8, seq: Arc<Sequencer>) {
    let (guard, lock) = x.begin_write_barrier(bit).await;
    seq.log.record(EV_A_ENTERED);
    seq.a_entered.store(true, Ordering::SeqCst);

    // Pause mid-critical-section — simulates the backfill/snapshot work
    // that a real DDL does between begin_write_barrier and guard drop.
    wait_flag(&seq.release_a).await;

    // Explicit drop to control the recording order: the guards MUST be
    // dropped before we record "done", and `lock` before `guard`
    // (reverse declaration — mirrors every DDL call site's discipline).
    drop(lock);
    drop(guard);

    seq.log.record(EV_A_DONE);
    seq.a_done.store(true, Ordering::SeqCst);
}

/// DDL-B's shape: acquire the write barrier (blocks on `ddl_admission`
/// while DDL-A holds it), record "entered", pause, then drop and record
/// "done".
async fn ddl_b(x: TableManager, bit: u8, seq: Arc<Sequencer>) {
    let (guard, lock) = x.begin_write_barrier(bit).await;
    seq.log.record(EV_B_ENTERED);
    seq.b_entered.store(true, Ordering::SeqCst);

    // Pause — the main test checks needs_write_barrier() while we hold
    // the barrier here.
    wait_flag(&seq.release_b).await;

    drop(lock);
    drop(guard);

    seq.log.record(EV_B_DONE);
    seq.b_done.store(true, Ordering::SeqCst);
}

/// Core proof: two concurrent DDL operations sharing `bit` are serialized
/// by the admission mutex, and a new writer sees the barrier during
/// DDL-B's critical section.
///
/// Asserts:
/// 1. DDL-B does NOT enter its critical section while DDL-A is paused
///    inside its own (B is blocked on `ddl_admission`).
/// 2. After DDL-A completes, DDL-B is admitted and raises the bit.
/// 3. While DDL-B holds the barrier, `needs_write_barrier()` returns
///    `true` — a new writer would take the slow (locked) path.
/// 4. After DDL-B completes, the bit is cleared.
/// 5. The event log records the exact ordering [A_ENTERED, A_DONE,
///    B_ENTERED, B_DONE].
async fn assert_two_ddls_same_bit_serialized_and_writer_sees_barrier(bit: u8) {
    let (_repo, x, _y) = make_two_tables().await;
    let seq = Sequencer::new();

    // Pre-condition: no barrier active.
    assert!(
        !x.needs_write_barrier(),
        "precondition: table must start with no barrier active"
    );

    // ── Phase 1: DDL-A enters its critical section ──────────────────
    let a = tokio::spawn(ddl_a(x.clone(), bit, Arc::clone(&seq)));
    wait_flag(&seq.a_entered).await;
    assert_eq!(
        seq.log.snapshot(),
        vec![EV_A_ENTERED],
        "DDL-A must be the first and only event"
    );
    assert!(
        x.needs_write_barrier(),
        "DDL-A has raised the bit — needs_write_barrier must be true"
    );

    // ── Phase 2: DDL-B is launched but must NOT proceed ─────────────
    let b = tokio::spawn(ddl_b(x.clone(), bit, Arc::clone(&seq)));

    // Give DDL-B a bounded chance to run. In a single-threaded runtime,
    // `wait_flag` yields cooperatively — if DDL-B were not blocked it
    // would run to its `b_entered` signal within a few yields. The
    // timeout firing proves DDL-B is genuinely parked on `ddl_admission`.
    let b_raced_ahead =
        tokio::time::timeout(Duration::from_millis(100), wait_flag(&seq.b_entered)).await;
    assert!(
        b_raced_ahead.is_err(),
        "DDL-B must NOT enter its critical section while DDL-A holds the \
         admission mutex — B should be blocked on ddl_admission"
    );
    assert_eq!(
        seq.log.snapshot(),
        vec![EV_A_ENTERED],
        "no new events — DDL-B is still blocked on admission"
    );

    // ── Phase 3: Release DDL-A; DDL-B should then be admitted ───────
    seq.release_a.store(true, Ordering::SeqCst);

    // Wait for DDL-A to complete (bit cleared, admission released).
    wait_flag(&seq.a_done).await;

    // DDL-B should now be admitted — wait for it to enter its CS.
    let b_entered = tokio::time::timeout(BOUND, wait_flag(&seq.b_entered)).await;
    assert!(
        b_entered.is_ok(),
        "DDL-B must enter its critical section after DDL-A completes and \
         releases the admission mutex — a timeout means B is deadlocked \
         or never re-raised the bit"
    );

    // ── Phase 4: DDL-B is in its CS — verify the bit is up ──────────
    assert!(
        x.needs_write_barrier(),
        "DDL-B has re-raised the bit — a new writer starting now must \
         observe needs_write_barrier() == true and take the slow (locked) \
         path, NOT fast-path past DDL-B's in-progress work"
    );

    // ── Phase 5: Release DDL-B and verify clean state ───────────────
    seq.release_b.store(true, Ordering::SeqCst);

    // Join both tasks within a bounded timeout (guards against a
    // silent hang if the fix is wrong).
    tokio::time::timeout(BOUND, async {
        a.await.expect("DDL-A task panicked");
        b.await.expect("DDL-B task panicked");
    })
    .await
    .expect("DDL tasks did not complete within BOUND — possible deadlock");

    // Bit must be cleared after both DDLs complete.
    assert!(
        !x.needs_write_barrier(),
        "after both DDLs complete, the bit must be cleared — no leaked \
         barrier"
    );

    // ── Phase 6: Verify the EXACT event ordering ────────────────────
    // The log must be [A_ENTERED, A_DONE, B_ENTERED, B_DONE] — proving
    // DDL-B's critical section started strictly AFTER DDL-A's ended.
    assert_eq!(
        seq.log.snapshot(),
        vec![EV_A_ENTERED, EV_A_DONE, EV_B_ENTERED, EV_B_DONE],
        "event ordering must be A-entered → A-done → B-entered → B-done, \
         proving DDL admission is fully serialized"
    );
}

// ============================================================================
// Parametrized serialization + writer-visibility tests for each DDL bit.
// ============================================================================

#[tokio::test]
async fn f95_regular_index_create_serializes_and_writer_sees_barrier() {
    assert_two_ddls_same_bit_serialized_and_writer_sees_barrier(REGULAR_INDEX_CREATE).await;
}

#[tokio::test]
async fn f95_schema_activation_serializes_and_writer_sees_barrier() {
    assert_two_ddls_same_bit_serialized_and_writer_sees_barrier(SCHEMA_ACTIVATION).await;
}

#[tokio::test]
async fn f95_unique_index_create_serializes_and_writer_sees_barrier() {
    assert_two_ddls_same_bit_serialized_and_writer_sees_barrier(UNIQUE_INDEX_CREATE).await;
}

#[tokio::test]
async fn f95_sorted_index_create_serializes_and_writer_sees_barrier() {
    assert_two_ddls_same_bit_serialized_and_writer_sees_barrier(SORTED_INDEX_CREATE).await;
}

#[tokio::test]
async fn f95_index2_create_serializes_and_writer_sees_barrier() {
    assert_two_ddls_same_bit_serialized_and_writer_sees_barrier(INDEX2_CREATE).await;
}

// ============================================================================
// F-70 regression: the admission mutex must not reintroduce the
// lock-order inversion cycle that F-70 (#897) closed.
// ============================================================================

/// The F-70 3-party cycle: committer A holds a drain guard on X (fast
/// path) and blocks on `unique_write_lock(Y)` (held by B); committer B
/// holds `unique_write_lock(Y)` and acquires `unique_write_lock(X)`
/// (which the DDL has NOT taken yet — it is still draining). The DDL on
/// X holds `ddl_admission(X)` + the raised bit while draining, waiting
/// for A's drain guard to clear.
///
/// With the admission mutex in place, the cycle still resolves:
/// - B acquires X's lock (free), completes, releases Y.
/// - A acquires Y, completes its write, drops its drain guard on X.
/// - The DDL's drain completes; it acquires `unique_write_lock(X)` and
///   proceeds.
///
/// No party in the cycle ever acquires `ddl_admission`, so it cannot be
/// a link in any lock-wait cycle. This test proves that by completing the
/// entire cycle within the same bounded timeout F-70 uses.
#[tokio::test]
async fn f95_admission_mutex_does_not_reintroduce_f70_deadlock() {
    let (_repo, x, y) = make_two_tables().await;

    let ddl_done = Arc::new(AtomicBool::new(false));
    let a_drain_held = Arc::new(AtomicBool::new(false));
    let b_holds_y = Arc::new(AtomicBool::new(false));

    // Committer A: enter X's drain set (fast path), keep the guard alive,
    // then block on unique_write_lock(Y) (held by B). Mirrors
    // pre_commit_prelock's Phase 2.5 shape exactly.
    let a_drain_held_c = Arc::clone(&a_drain_held);
    let b_holds_y_c = Arc::clone(&b_holds_y);
    let x_for_a = x.clone();
    let y_for_a = y.clone();
    let committer_a = tokio::spawn(async move {
        let drain_guard = x_for_a.enter_writer_drain();
        assert!(
            !x_for_a.needs_write_barrier(),
            "precondition: DDL must not have raised X's bit yet"
        );
        a_drain_held_c.store(true, Ordering::SeqCst);

        // Wait until B provably holds Y's lock before contending for it.
        wait_flag(&b_holds_y_c).await;
        let _y_guard = y_for_a.unique_write_lock().lock_owned().await;
        drop(drain_guard);
    });

    // Committer B: hold Y's lock, then acquire X's lock (which is FREE —
    // the DDL is still draining and hasn't taken it yet).
    let b_holds_y_c = Arc::clone(&b_holds_y);
    let x_for_b = x.clone();
    let y_for_b = y.clone();
    let committer_b = tokio::spawn(async move {
        let _y_guard = y_for_b.unique_write_lock().lock_owned().await;
        b_holds_y_c.store(true, Ordering::SeqCst);

        // X's unique_write_lock is free (DDL hasn't acquired it yet — it's
        // still draining), so B acquires it immediately.
        let _x_guard = x_for_b.unique_write_lock().lock_owned().await;
    });

    // DDL on X: goes through begin_write_barrier, which now acquires
    // ddl_admission FIRST, then raises the bit, drains (waiting for A's
    // drain guard), and only then takes unique_write_lock(X). The
    // admission mutex is held throughout, but since no writer or
    // committer ever acquires it, it cannot participate in a lock-wait
    // cycle.
    let ddl_done_c = Arc::clone(&ddl_done);
    let a_drain_held_c = Arc::clone(&a_drain_held);
    let x_for_ddl = x.clone();
    let ddl = tokio::spawn(async move {
        // Wait until A has entered X's drain set so the DDL's drain has
        // someone to wait for (otherwise drain is instant and the test
        // doesn't exercise the wait-while-holding-admission path).
        wait_flag(&a_drain_held_c).await;
        let (_barrier, _lock) = x_for_ddl.begin_write_barrier(REGULAR_INDEX_CREATE).await;
        ddl_done_c.store(true, Ordering::SeqCst);
    });

    let result = tokio::time::timeout(BOUND, async {
        committer_a.await.expect("committer A panicked");
        committer_b.await.expect("committer B panicked");
        ddl.await.expect("DDL panicked");
    })
    .await;

    assert!(
        result.is_ok(),
        "F-70 regression: the admission mutex must not reintroduce the \
         lock-order inversion deadlock. The 3-party cycle (DDL drains on X \
         while holding ddl_admission; A holds drain guard on X and blocks \
         on unique_write_lock(Y); B holds Y and acquires X) must complete \
         within {BOUND:?}."
    );

    assert!(
        ddl_done.load(Ordering::SeqCst),
        "DDL must have completed its begin_write_barrier sequence"
    );
    assert!(
        !x.needs_write_barrier(),
        "DDL's WriteBarrierGuard must clear the bit on drop"
    );
}

/// Additional regression: the admission mutex must not prevent a DDL on
/// table Y from proceeding while a DDL on table X holds X's admission
/// mutex. Each table has its OWN `ddl_admission`, so cross-table DDLs are
/// independent.
#[tokio::test]
async fn f95_admission_mutex_is_per_table_not_global() {
    let (_repo, x, y) = make_two_tables().await;

    let x_entered = Arc::new(AtomicBool::new(false));
    let y_done = Arc::new(AtomicBool::new(false));
    let release_x = Arc::new(Notify::new());

    // DDL on X: acquire barrier, pause mid-CS.
    let x_entered_c = Arc::clone(&x_entered);
    let release_x_c = Arc::clone(&release_x);
    let ddl_x = tokio::spawn(async move {
        let (_guard, _lock) = x.begin_write_barrier(REGULAR_INDEX_CREATE).await;
        x_entered_c.store(true, Ordering::SeqCst);
        release_x_c.notified().await;
    });

    // Wait for X's DDL to enter its CS.
    wait_flag(&x_entered).await;

    // DDL on Y: should proceed immediately — Y has its own admission mutex.
    let y_done_c = Arc::clone(&y_done);
    let ddl_y = tokio::spawn(async move {
        let (_guard, _lock) = y.begin_write_barrier(REGULAR_INDEX_CREATE).await;
        y_done_c.store(true, Ordering::SeqCst);
    });

    // Y's DDL must complete within a short bound — it is NOT blocked by
    // X's admission mutex.
    let y_result = tokio::time::timeout(Duration::from_millis(200), wait_flag(&y_done)).await;
    assert!(
        y_result.is_ok(),
        "DDL on table Y must not be blocked by table X's admission mutex — \
         ddl_admission is per-table, not global"
    );

    // Release X's DDL.
    release_x.notify_one();

    tokio::time::timeout(BOUND, async {
        ddl_x.await.expect("DDL-X panicked");
        ddl_y.await.expect("DDL-Y panicked");
    })
    .await
    .expect("DDL tasks did not complete");
}
