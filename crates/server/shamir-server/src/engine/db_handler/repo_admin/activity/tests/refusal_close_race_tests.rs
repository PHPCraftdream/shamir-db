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

use crate::db_handler::repo_audit::{enqueue_cancel_refusal_audit, CancelRefusalAudit};

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
