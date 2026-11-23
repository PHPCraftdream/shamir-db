//! Ordered-shutdown close/discard race of the refusal-audit queue.
//!
//! A submission that reserved a share slot BEFORE `close()` must either be
//! spawned before the discard sweep can see the closed bit, or not spawned at
//! all and counted as dropped. Aborting a blocking job cannot stop one that has
//! already started, so the closed-bit test has to be taken before the spawn —
//! which is exactly the window this test parks in.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::server::audit_chain::{
    AuditAppender, AuditChain, AuditChainWriter, AuditEntry,
};
use shamir_connect::server::session::{Session, SessionPermissions};
use shamir_query_types::admin::activity::{CancelOutcome, RequestExecId};

use crate::db_handler::repo_audit::{
    enqueue_cancel_refusal_audit, CancelRefusalAudit, ShareDecisionProbeGuard,
    SpawnWindowProbeGuard, CANCEL_AUDIT_PRINCIPAL_SHARE,
};

const TIMEOUT: Duration = Duration::from_secs(120);

fn session(name: &str) -> Session {
    let mut session = Session::new(
        [3; 16],
        name.to_owned(),
        SessionPermissions::new(false, false, Vec::new()),
        TransportKind::Tcp,
        BindingMode::None,
        [0; 32],
        1_000,
    );
    session.session_id = [3; 32];
    session
}

/// Records every append and counts how many were entered.
#[derive(Default)]
struct CountingAppender {
    entered: AtomicUsize,
}

impl AuditAppender for CountingAppender {
    fn append_entry(&self, _entry: &AuditEntry) -> Result<(), std::io::Error> {
        self.entered.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn checkpoint(&self, _: u64, _: &[u8; 32]) -> Result<(), std::io::Error> {
        Ok(())
    }
}

#[derive(Clone, Copy, Default)]
struct RaceState {
    /// The submission reserved its slot and is paused before the spawn.
    reserved: bool,
    /// The test closed admission and swept the queue; the submission may go on.
    released: bool,
}

/// Block until `ready` holds, bounded by [`TIMEOUT`] from a deadline taken when
/// the wait starts. An expired bound is a hard failure naming the stage.
fn wait_until(gate: &(Mutex<RaceState>, Condvar), stage: &str, ready: impl Fn(RaceState) -> bool) {
    let (lock, condvar) = gate;
    let deadline = Instant::now() + TIMEOUT;
    let mut state = lock.lock().expect("race state");
    while !ready(*state) {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let (next, timed_out) = condvar
            .wait_timeout(state, remaining)
            .expect("race state wait");
        state = next;
        if timed_out.timed_out() && !ready(*state) {
            panic!("{stage} did not happen within {TIMEOUT:?}");
        }
    }
}

fn release(gate: &(Mutex<RaceState>, Condvar)) {
    let (lock, condvar) = gate;
    let mut state = lock.lock().expect("race state");
    state.released = true;
    condvar.notify_all();
}

// Two worker threads: the submission parks one of them inside the seam while
// the test body drives the close and the sweep from the other.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_refusal_reserved_before_close_is_never_spawned_after_the_sweep() {
    let appender = Arc::new(CountingAppender::default());
    let writer = Arc::new(AuditChainWriter::new_with_shared(
        Arc::new(AuditChain::new([9; 32])),
        appender.clone(),
    ));
    let gate = Arc::new((Mutex::new(RaceState::default()), Condvar::new()));
    let mut queue = CancelRefusalAudit::default();
    queue.pause_before_spawn = Some({
        let gate = Arc::clone(&gate);
        Arc::new(move || {
            {
                let (lock, condvar) = &*gate;
                let mut state = lock.lock().expect("race state");
                state.reserved = true;
                condvar.notify_all();
            }
            wait_until(&gate, "close and sweep", |state| state.released);
        })
    });
    let queue = Arc::new(queue);

    let submission = tokio::spawn({
        let queue = Arc::clone(&queue);
        let writer = Arc::clone(&writer);
        async move {
            let session = session("principal-race");
            enqueue_cancel_refusal_audit(
                &queue,
                &writer,
                &session,
                RequestExecId(7),
                &CancelOutcome::NotCancellable,
            )
        }
    });
    // The slot is reserved and the spawn has not happened yet.
    wait_until(&gate, "reservation", |state| state.reserved);

    // Exactly the window the sweep could not see before: close, then sweep.
    queue.close();
    queue.discard_queued();
    release(&gate);

    let job = tokio::time::timeout(TIMEOUT, submission)
        .await
        .expect("enqueue bounded")
        .expect("enqueue task");
    let spawned = job.is_some();
    if let Some(job) = job {
        let _ = tokio::time::timeout(TIMEOUT, job).await;
    }

    assert!(
        !spawned,
        "a submission that reserved before close() must not be spawned after the sweep"
    );
    assert_eq!(
        appender.entered.load(Ordering::SeqCst),
        0,
        "a dropped refusal must never reach the appender"
    );
    assert_eq!(
        queue.dropped_count(),
        1,
        "a refused submission is counted as a dropped event"
    );
}

/// [`RaceState`] for the spawn-window probe: the submission is parked BETWEEN
/// the closed-bit check and the spawn, holding the decision mutex.
#[derive(Clone, Copy, Default)]
struct SpawnWindowState {
    /// The spawn decision passed the closed-bit check and is parked.
    entered: bool,
    /// The test may let the decision finish.
    released: bool,
}

/// Releases a parked spawn decision when dropped, so a failing assertion can
/// never leave either worker blocked in the probe.
struct ReleaseSpawnWindow(Arc<(Mutex<SpawnWindowState>, Condvar)>);

impl Drop for ReleaseSpawnWindow {
    fn drop(&mut self) {
        open_spawn_window(&self.0);
    }
}

/// `close()` must NOT return while a spawn decision is in flight: it takes the
/// same mutex, so the job's handle is registered before the sweep can run.
///
/// Mutant killed: a `close()` that only sets the bit never encounters the held
/// decision mutex, so it never reports the test-only contention rendezvous.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn close_waits_for_an_in_flight_spawn_decision() {
    let appender = Arc::new(CountingAppender::default());
    let writer = Arc::new(AuditChainWriter::new(
        AuditChain::new([9u8; 32]),
        appender.clone(),
    ));
    let gate = Arc::new((Mutex::new(SpawnWindowState::default()), Condvar::new()));
    let queue = Arc::new(CancelRefusalAudit::default());

    let submission = tokio::task::spawn_blocking({
        let queue = Arc::clone(&queue);
        let writer = Arc::clone(&writer);
        let gate = Arc::clone(&gate);
        move || {
            let probe = queue_arm_spawn_window_probe(gate.clone());
            let session = session("principal-spawn-window");
            let job = enqueue_cancel_refusal_audit(
                &queue,
                &writer,
                &session,
                RequestExecId(9),
                &CancelOutcome::NotCancellable,
            );
            std::mem::forget(probe);
            job
        }
    });
    let _release = ReleaseSpawnWindow(Arc::clone(&gate));
    wait_for_spawn_window(&gate, "spawn window entry", |state| state.entered);

    // The close-side probe fires only after try_lock observes contention on the
    // decision mutex held by the parked submission.
    let (attempted_tx, attempted_rx) = std::sync::mpsc::channel();
    let (closed_tx, closed_rx) = std::sync::mpsc::channel();
    let closer = {
        let queue = Arc::clone(&queue);
        std::thread::spawn(move || {
            let _probe = CancelRefusalAudit::arm_close_decision_probe(move || {
                let _ = attempted_tx.send(());
            });
            queue.close();
            let _ = closed_tx.send(());
        })
    };
    attempted_rx
        .recv_timeout(TIMEOUT)
        .expect("close must attempt its decision while the submission is parked");
    let returned_while_parked = closed_rx.try_recv().is_ok();

    // Release only after close has observed the held decision mutex.
    open_spawn_window(&gate);
    let job = tokio::time::timeout(TIMEOUT, submission)
        .await
        .expect("enqueue bounded")
        .expect("enqueue task");
    closed_rx
        .recv_timeout(TIMEOUT)
        .expect("close must return once the decision finished");
    closer.join().expect("closer thread");

    assert!(
        !returned_while_parked,
        "close() must not return while the spawn decision mutex is held"
    );
    assert!(
        job.is_some(),
        "the decision that passed the closed-bit check spawns and is tracked"
    );
    queue.discard_queued();
    if let Some(job) = job {
        let _ = tokio::time::timeout(TIMEOUT, job).await;
    }
}

fn queue_arm_spawn_window_probe(
    gate: Arc<(Mutex<SpawnWindowState>, Condvar)>,
) -> SpawnWindowProbeGuard {
    CancelRefusalAudit::arm_spawn_window_probe(move || {
        {
            let (lock, condvar) = &*gate;
            let mut state = lock.lock().expect("spawn window state");
            state.entered = true;
            condvar.notify_all();
        }
        wait_for_spawn_window(&gate, "spawn window release", |state| state.released);
    })
}

fn wait_for_spawn_window(
    gate: &(Mutex<SpawnWindowState>, Condvar),
    stage: &str,
    ready: impl Fn(SpawnWindowState) -> bool,
) {
    let (lock, condvar) = gate;
    let deadline = Instant::now() + TIMEOUT;
    let mut state = lock.lock().expect("spawn window state");
    while !ready(*state) {
        let remaining = deadline.saturating_duration_since(Instant::now());
        assert!(
            !remaining.is_zero(),
            "{stage} must happen within {TIMEOUT:?}"
        );
        let (next, _) = condvar
            .wait_timeout(state, remaining)
            .expect("spawn window wait");
        state = next;
    }
}

fn open_spawn_window(gate: &(Mutex<SpawnWindowState>, Condvar)) {
    let (lock, condvar) = gate;
    *lock.lock().expect("spawn window state") = SpawnWindowState {
        entered: true,
        released: true,
    };
    condvar.notify_all();
}

/// [`ShareDecisionState`] for the share-decision probe: a doomed reservation is
/// parked BETWEEN the read-only share pre-check and the bucket-locked decision.
#[derive(Clone, Copy, Default)]
struct ShareDecisionState {
    /// The reservation passed the pre-check and has not reached the decision.
    entered: bool,
    /// The test may let the decision run.
    released: bool,
}

/// Releases a parked reservation when dropped, so a failing assertion can never
/// leave the probe parked.
struct ReleaseShareDecision(Arc<(Mutex<ShareDecisionState>, Condvar)>);

impl Drop for ReleaseShareDecision {
    fn drop(&mut self) {
        open_share_decision(&self.0);
    }
}

/// The share check and the global reservation must be ONE decision: a request
/// that is already doomed by its principal's share must never hold — not even
/// while it stalls before the decision — the global slot another principal is
/// about to need.
///
/// Two A reservations and one B: the table is filled through the production
/// admission path to `capacity - 2` with A at `share - 1`, so ONE free slot is
/// left for B after A's first reservation completes. A's second reservation
/// passes the pre-check and parks before the decision; A's first then completes
/// (A = share, global = 255); B must still be ADMITTED. The parked A, released
/// after B, must be REFUSED with A's share untouched and no global slot left
/// behind.
///
/// Mutant killed: a decision that reserves the global slot after the read-only
/// pre-check and only then tests the share (the old
/// `share_is_full` / `reserve_global_slot` / `reserve_share` order). The parked
/// A then sits on a global slot, B's reservation is refused at capacity, and
/// `b_admitted` fails although B's principal is untouched.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_share_capped_principal_never_holds_the_global_slot_b_of_a_doomed_refusal() {
    let queue = Arc::new(CancelRefusalAudit::default());
    let capacity = CancelRefusalAudit::global_capacity_for_test();
    let share = CANCEL_AUDIT_PRINCIPAL_SHARE;
    // Two slots stay free: A's first reservation and B's — so B takes the LAST
    // slot of the table, exactly the slot a doomed A must never hold.
    let seeded = capacity - 2;
    let seeded_a = share - 1;

    // Fill through the SAME counters production uses: A to one below its share,
    // the rest spread over other principals that each stay under it. Keeping
    // the returned slots alive keeps the reservations live.
    let mut seeded_slots = queue.seed_reservations_for_test("principal-a", seeded_a);
    let mut spread = 0;
    let mut index = 0;
    while spread < seeded - seeded_a {
        let take = (seeded - seeded_a - spread).min(share);
        seeded_slots
            .append(queue.seed_reservations_for_test(&format!("principal-other-{index}"), take));
        spread += take;
        index += 1;
    }
    assert_eq!(
        queue.global_held_for_test(),
        seeded,
        "the global table is filled only up to the two slots the test needs"
    );
    assert_eq!(
        queue.principal_held_for_test("principal-a"),
        seeded_a,
        "principal A starts one reservation below its share"
    );

    // Park A's SECOND reservation in the hook window: it read A below its share
    // and has not reached the decision.
    let gate = Arc::new((Mutex::new(ShareDecisionState::default()), Condvar::new()));
    let parked = {
        let queue = Arc::clone(&queue);
        let gate = Arc::clone(&gate);
        std::thread::spawn(move || {
            let probe = arm_share_decision_probe(gate.clone());
            let reservation = queue.try_reserve_for_test("principal-a");
            std::mem::forget(probe);
            reservation
        })
    };
    let _release = ReleaseShareDecision(Arc::clone(&gate));
    wait_for_share_decision(&gate, "share decision entry", |state| state.entered);

    // A's FIRST reservation completes: A reaches its share, global 255.
    let first_a = queue
        .try_reserve_for_test("principal-a")
        .expect("the first A reservation is inside the share");
    let global_after_first_a = queue.global_held_for_test();
    assert_eq!(
        queue.principal_held_for_test("principal-a"),
        share,
        "the first A reservation fills A's share"
    );

    // B arrives while the doomed A is parked: B must get the last slot.
    let drops_before_b = queue.dropped_count();
    let from_b = queue.try_reserve_for_test("principal-b");
    let b_admitted = from_b.is_some();
    let drops_after_b = queue.dropped_count();
    assert!(
        b_admitted,
        "principal B was denied a global slot by a doomed A reservation that can never take it"
    );

    // Release the parked A, then judge it.
    open_share_decision(&gate);
    let parked_a = parked.join().expect("parked reservation thread");

    assert_eq!(
        global_after_first_a,
        seeded + 1,
        "the first A reservation took exactly one global slot"
    );
    assert_eq!(
        drops_after_b, drops_before_b,
        "an admitted B reservation records no drop"
    );
    assert_eq!(
        queue.principal_held_for_test("principal-b"),
        1,
        "B's reservation is charged to B"
    );
    assert!(
        parked_a.is_none(),
        "the parked A reservation must be refused: A is at its share"
    );
    assert_eq!(
        queue.principal_held_for_test("principal-a"),
        share,
        "a refused A reservation leaves A's share untouched"
    );
    assert_eq!(
        queue.global_held_for_test(),
        seeded + 2,
        "the refused A left no global slot behind: seeded + first A + B"
    );
    assert_eq!(
        queue.dropped_count(),
        drops_before_b + 1,
        "the refused A reservation is counted exactly once"
    );
    drop(first_a);
    drop(from_b);
    drop(seeded_slots);
}

fn arm_share_decision_probe(
    gate: Arc<(Mutex<ShareDecisionState>, Condvar)>,
) -> ShareDecisionProbeGuard {
    CancelRefusalAudit::arm_share_decision_probe(move || {
        {
            let (lock, condvar) = &*gate;
            let mut state = lock.lock().expect("share decision state");
            state.entered = true;
            condvar.notify_all();
        }
        wait_for_share_decision(&gate, "share decision release", |state| state.released);
    })
}

fn wait_for_share_decision(
    gate: &(Mutex<ShareDecisionState>, Condvar),
    stage: &str,
    ready: impl Fn(ShareDecisionState) -> bool,
) {
    let (lock, condvar) = gate;
    let deadline = Instant::now() + TIMEOUT;
    let mut state = lock.lock().expect("share decision state");
    while !ready(*state) {
        let remaining = deadline.saturating_duration_since(Instant::now());
        assert!(
            !remaining.is_zero(),
            "{stage} must happen within {TIMEOUT:?}"
        );
        let (next, _) = condvar
            .wait_timeout(state, remaining)
            .expect("share decision wait");
        state = next;
    }
}

fn open_share_decision(gate: &(Mutex<ShareDecisionState>, Condvar)) {
    let (lock, condvar) = gate;
    *lock.lock().expect("share decision state") = ShareDecisionState {
        entered: true,
        released: true,
    };
    condvar.notify_all();
}
