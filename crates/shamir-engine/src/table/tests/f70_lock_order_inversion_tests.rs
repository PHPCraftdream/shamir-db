//! F-70 (#897, P0) — fix the commit/DDL lock-order inversion.
//!
//! # The bug this closes
//!
//! `pre_commit_prelock` (`tx/pre_commit.rs`, Phase 2.5) enters a table's
//! `WriterDrainBarrier` drain set (`enter_writer_drain`) BEFORE reading
//! `needs_write_barrier()` for that table, and — on the fast path (flag
//! still `false`) — KEEPS that drain guard alive while it goes on to acquire
//! `unique_write_lock` for a DIFFERENT table this same transaction also
//! wrote to. So a committer can be a drain-set member on table X (guard kept
//! alive) while blocked acquiring `unique_write_lock(Y)`, all within the
//! same call.
//!
//! Every DDL create path (`create_index_v2`/`create_index`/
//! `create_unique_index`/sorted-index create), as wired by F-57 (#883),
//! used to do the OPPOSITE order: acquire `unique_write_lock` FIRST, THEN
//! raise the intent bit and `drain_writers()`.
//!
//! This is a genuine 3-party lock-order inversion, reachable with two
//! tables X, Y:
//! - **DDL** on X: holds `unique_write_lock(X)`, blocks in `drain(X)`
//!   waiting for committer A's drain-set membership on X to clear.
//! - **Committer A**: holds its drain guard on X (fast path — it read
//!   `needs_write_barrier() == false` on X before the DDL raised the bit),
//!   blocked acquiring `unique_write_lock(Y)` — held by committer B.
//! - **Committer B**: holds `unique_write_lock(Y)`, blocked acquiring
//!   `unique_write_lock(X)` — held by the DDL.
//!
//! DDL → A → B → DDL: a real deadlock, not a race.
//!
//! # What this file proves
//!
//! 1. [`f70_lock_then_drain_ddl_order_deadlocks_against_committer_cycle`]
//!    manually reproduces the OLD (pre-F-70) DDL acquisition order —
//!    `unique_write_lock(X)` FIRST, `drain(X)` SECOND — against the SAME
//!    committer-side shape `pre_commit_prelock` actually uses (drain-guard
//!    on X kept alive, then blocked on `unique_write_lock(Y)`, with a third
//!    party B holding `unique_write_lock(Y)` and itself blocked on
//!    `unique_write_lock(X)`). Wrapped in `tokio::time::timeout` (NOT a bare
//!    `sleep`) so a real deadlock shows up as a bounded, deterministic test
//!    failure (timeout elapses) instead of hanging the suite.
//! 2. [`f70_drain_then_lock_ddl_order_completes_the_same_cycle`] runs the
//!    IDENTICAL cycle but with the DDL side going through
//!    [`TableManager::begin_write_barrier`] (the shipped F-70 fix —
//!    drain-then-lock). This closes the cycle: while draining, the DDL
//!    holds no table lock at all, so it cannot be a link in any lock-wait
//!    cycle during that wait. The same `timeout` bound that fails test 1
//!    passes comfortably here, proving the fix (not just a slower race)
//!    closes the deadlock.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Notify;

use crate::index::write_barrier_flags::REGULAR_INDEX_CREATE;
use crate::repo::repo_instance::RepoInstance;
use crate::repo::repo_types::BoxRepo;
use crate::table::TableConfig;
use crate::table::TableManager;
use shamir_storage::storage_in_memory::InMemoryRepo;

/// Generous but bounded — long enough that a healthy (non-deadlocked) run
/// never brushes it (each step below is either an uncontended lock
/// acquisition or a single atomic load), short enough that a genuine
/// deadlock fails the test in well under a second instead of hanging the
/// suite for the nextest per-test kill window (180s).
const DEADLOCK_BOUND: Duration = Duration::from_millis(500);

fn make_repo() -> RepoInstance {
    let repo = Arc::new(InMemoryRepo::new());
    RepoInstance::new("test".into(), BoxRepo::InMemory(repo), Vec::new())
}

/// Handshake used to sequence the three parties into the exact interleaving
/// the cycle needs, with no timing assumptions — every step advances only
/// after the previous one has provably happened (`Notify` + poll-on-a-flag,
/// same convention as this crate's other pause-seam tests, NOT `sleep`).
struct Sequencer {
    /// Fires once committer A has entered table X's drain set (fast path)
    /// and is about to block on `unique_write_lock(Y)`.
    a_holds_x_drain: Notify,
    a_holds_x_drain_reached: std::sync::atomic::AtomicBool,
    /// Fires once committer B holds `unique_write_lock(Y)` and is about to
    /// block on `unique_write_lock(X)`.
    b_holds_y_lock: Notify,
    b_holds_y_lock_reached: std::sync::atomic::AtomicBool,
    /// Fires once the DDL has raised its intent bit on X (so the reasoning
    /// about "committer A's fast-path read happened before the DDL raised
    /// the bit" is pinned down deterministically rather than left to
    /// scheduling luck).
    ddl_ready_to_raise: Notify,
    ddl_ready_to_raise_reached: std::sync::atomic::AtomicBool,
}

impl Sequencer {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            a_holds_x_drain: Notify::new(),
            a_holds_x_drain_reached: std::sync::atomic::AtomicBool::new(false),
            b_holds_y_lock: Notify::new(),
            b_holds_y_lock_reached: std::sync::atomic::AtomicBool::new(false),
            ddl_ready_to_raise: Notify::new(),
            ddl_ready_to_raise_reached: std::sync::atomic::AtomicBool::new(false),
        })
    }
}

async fn wait_flag(flag: &std::sync::atomic::AtomicBool) {
    while !flag.load(std::sync::atomic::Ordering::SeqCst) {
        tokio::task::yield_now().await;
    }
}

/// Builds the two tables (X = "orders", Y = "customers") with one seed row
/// each, so `needs_write_barrier()` starts `false` on both and every table
/// has a live `TableManager` instance (required for `table_by_token_if_live`
/// in the real prelock path, though this test drives the primitives
/// directly rather than through the tx-commit pipeline).
async fn make_two_tables() -> (RepoInstance, TableManager, TableManager) {
    let repo = make_repo();
    repo.add_table(TableConfig::new("orders"));
    repo.add_table(TableConfig::new("customers"));
    let x = repo.get_table("orders").await.unwrap();
    let y = repo.get_table("customers").await.unwrap();
    (repo, x, y)
}

/// Committer A's shape, mirroring `pre_commit_prelock`'s Phase 2.5 exactly:
/// enter X's drain set, read `needs_write_barrier()` (expected `false` at
/// this point — the DDL hasn't raised its bit on X yet), keep the drain
/// guard ALIVE (fast path), then block acquiring `unique_write_lock(Y)`.
async fn committer_a(x: TableManager, y: TableManager, seq: Arc<Sequencer>) {
    let drain_guard = x.enter_writer_drain();
    assert!(
        !x.needs_write_barrier(),
        "precondition: DDL must not have raised X's bit yet when A reads the flag"
    );
    seq.a_holds_x_drain_reached
        .store(true, std::sync::atomic::Ordering::SeqCst);
    seq.a_holds_x_drain.notify_one();

    // Wait for B to actually hold Y's lock before attempting to acquire it
    // ourselves — pins down the "A blocks on Y, held by B" edge
    // deterministically instead of racing B for the lock.
    wait_flag(&seq.b_holds_y_lock_reached).await;

    let _y_guard = y.unique_write_lock().lock_owned().await;
    // Only reachable once B releases Y's lock (which, in the deadlock case,
    // never happens because B is itself waiting on X, held by the DDL).
    drop(drain_guard);
}

/// Committer B's shape: hold `unique_write_lock(Y)`, then block acquiring
/// `unique_write_lock(X)` (held by the DDL in the pre-fix, lock-then-drain
/// order — closing the 3-party cycle back to the DDL).
async fn committer_b(x: TableManager, y: TableManager, seq: Arc<Sequencer>) {
    let _y_guard = y.unique_write_lock().lock_owned().await;
    seq.b_holds_y_lock_reached
        .store(true, std::sync::atomic::Ordering::SeqCst);
    seq.b_holds_y_lock.notify_one();

    // Let the DDL get far enough to hold X's lock (pre-fix order) before B
    // contends for it — otherwise B might win X's lock uncontended and the
    // intended cycle never forms.
    wait_flag(&seq.ddl_ready_to_raise_reached).await;

    let _x_guard = x.unique_write_lock().lock_owned().await;
}

/// The OLD (pre-F-70) DDL acquisition order: `unique_write_lock(X)` FIRST,
/// intent bit + `drain_writers()` SECOND. Manually replicated here (rather
/// than calling a production fn) because every production DDL call site now
/// goes through the shipped fix (`begin_write_barrier`, drain-then-lock) —
/// this closure is the "red" order the bug report describes, preserved as a
/// permanent regression fixture even though no call site can regress into
/// it silently anymore (F-70 also makes `begin_write_barrier` the sole
/// entry point).
async fn ddl_lock_then_drain(x: TableManager, seq: Arc<Sequencer>) {
    wait_flag(&seq.a_holds_x_drain_reached).await;

    let _x_guard = x.unique_write_lock().lock_owned().await;
    seq.ddl_ready_to_raise_reached
        .store(true, std::sync::atomic::Ordering::SeqCst);
    seq.ddl_ready_to_raise.notify_one();

    x.write_barrier_flags.set(REGULAR_INDEX_CREATE);
    // Pre-fix order: drain AFTER the lock is already held. This is the call
    // that deadlocks against committer A's kept-alive drain guard, because A
    // is blocked on Y (held by B), and B is blocked on X (held by this
    // DDL) — DDL -> A -> B -> DDL.
    x.drain_writers().await;
    x.write_barrier_flags.clear(REGULAR_INDEX_CREATE);
}

/// The NEW (F-70) DDL acquisition order via the shipped canonical entry
/// point: raise the bit, drain, THEN acquire the lock.
async fn ddl_drain_then_lock(x: TableManager, seq: Arc<Sequencer>) {
    wait_flag(&seq.a_holds_x_drain_reached).await;

    // Signal readiness up front — under drain-then-lock the DDL holds no
    // lock while draining, so B does not need to wait for a lock-acquired
    // signal to form its side of the (now non-cyclic) wait graph. Firing
    // this immediately keeps the two scenarios' sequencing symmetric.
    seq.ddl_ready_to_raise_reached
        .store(true, std::sync::atomic::Ordering::SeqCst);
    seq.ddl_ready_to_raise.notify_one();

    let (_barrier, _x_guard) = x.begin_write_barrier(REGULAR_INDEX_CREATE).await;
}

/// RED: the pre-F-70 DDL order (lock-then-drain) genuinely deadlocks against
/// the committer-side shape `pre_commit_prelock` uses (drain-guard-then-lock
/// for a different table). Bounded by `tokio::time::timeout` so a real
/// deadlock is a fast, deterministic test FAILURE rather than a hang.
#[tokio::test]
async fn f70_lock_then_drain_ddl_order_deadlocks_against_committer_cycle() {
    let (_repo, x, y) = make_two_tables().await;
    let seq = Sequencer::new();

    let a = tokio::spawn(committer_a(x.clone(), y.clone(), Arc::clone(&seq)));
    let b = tokio::spawn(committer_b(x.clone(), y.clone(), Arc::clone(&seq)));
    let ddl = tokio::spawn(ddl_lock_then_drain(x.clone(), Arc::clone(&seq)));

    let result = tokio::time::timeout(DEADLOCK_BOUND, async {
        let _ = tokio::join!(a, b, ddl);
    })
    .await;

    assert!(
        result.is_err(),
        "F-70 RED: the pre-fix lock-then-drain DDL order was expected to \
         deadlock against the committer-side drain-guard-then-lock cycle \
         (DDL -> A -> B -> DDL), but all three parties completed within \
         {DEADLOCK_BOUND:?}. If this assertion fails, the reproduction no \
         longer demonstrates the bug this task fixes — re-derive the cycle \
         before trusting the fix's proof below."
    );

    // Leave the table's barrier bit as we found it: the timed-out `ddl` task
    // may still be parked holding `_x_guard`/the bit depending on exactly
    // where it got stuck; this test process exits right after, so no
    // further cleanup is required (nextest runs each test in its own
    // process — see this crate's other pause-seam tests for the same
    // reasoning).
}

/// GREEN: the SAME 3-party cycle, but the DDL now goes through
/// `TableManager::begin_write_barrier` (drain-then-lock, the shipped F-70
/// fix). Completes comfortably inside the SAME bound that test 1 uses to
/// prove a deadlock — demonstrating the fix, not merely a narrower race
/// window.
#[tokio::test]
async fn f70_drain_then_lock_ddl_order_completes_the_same_cycle() {
    let (_repo, x, y) = make_two_tables().await;
    let seq = Sequencer::new();

    let a = tokio::spawn(committer_a(x.clone(), y.clone(), Arc::clone(&seq)));
    let b = tokio::spawn(committer_b(x.clone(), y.clone(), Arc::clone(&seq)));
    let ddl = tokio::spawn(ddl_drain_then_lock(x.clone(), Arc::clone(&seq)));

    let result = tokio::time::timeout(DEADLOCK_BOUND, async {
        let (a_res, b_res, ddl_res) = tokio::join!(a, b, ddl);
        a_res.unwrap();
        b_res.unwrap();
        ddl_res.unwrap();
    })
    .await;

    assert!(
        result.is_ok(),
        "F-70 GREEN: the fixed drain-then-lock DDL order must complete the \
         SAME 3-party cycle within {DEADLOCK_BOUND:?}. A timeout here means \
         the fix did not actually close the deadlock (or reintroduced a \
         different one)."
    );

    assert!(
        !x.needs_write_barrier(),
        "the DDL's WriteBarrierGuard must clear REGULAR_INDEX_CREATE on drop \
         once the barrier scope ends, leaving X's predicate false again"
    );
}
