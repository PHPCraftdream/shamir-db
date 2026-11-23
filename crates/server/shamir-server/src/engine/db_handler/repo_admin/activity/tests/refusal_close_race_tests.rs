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
    enqueue_cancel_refusal_audit, CancelRefusalAudit, SpawnWindowProbeGuard,
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

/// `close()` must NOT return while a spawn decision is in flight: it takes the
/// same mutex, so the job's handle is registered before the sweep can run.
///
/// Mutant killed: a `close()` that only sets the bit returns immediately while
/// the decision is parked — `close_returned` then arrives inside the bounded
/// wait, and the "must not return" assertion fails.
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
    wait_for_spawn_window(&gate, "spawn window entry", |state| state.entered);

    // `close()` on another thread: with the shared decision mutex it cannot
    // return until the parked decision finishes.
    let (closed_tx, closed_rx) = std::sync::mpsc::channel();
    let closer = {
        let queue = Arc::clone(&queue);
        std::thread::spawn(move || {
            queue.close();
            let _ = closed_tx.send(());
        })
    };
    // Bounded "must not happen": a no-op close would report inside this window.
    let returned_while_parked = closed_rx.recv_timeout(Duration::from_millis(500)).is_ok();
    assert!(
        !returned_while_parked,
        "close() must wait for the in-flight spawn decision, not return while it runs"
    );

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
