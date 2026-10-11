//! SNAP-1 contract tests. Pre-change these APIs do not exist; the mutation
//! counterfactuals below distinguish notification progress from timer rescue.

use crate::completion_tracker::State;
use crate::repo_tx_gate::{RepoTxGate, VisibleOutcome};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Barrier};
use std::task::{Context, Poll, Wake, Waker};
use std::time::Duration;

#[derive(Default)]
struct WakeProbe {
    woken: AtomicBool,
    signal: tokio::sync::Notify,
}

impl Wake for WakeProbe {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.woken.store(true, Ordering::SeqCst);
        self.signal.notify_one();
    }
}

fn poll_once<F: Future>(future: Pin<&mut F>, probe: &Arc<WakeProbe>) -> Poll<F::Output> {
    let waker = Waker::from(Arc::clone(probe));
    future.poll(&mut Context::from_waker(&waker))
}

#[tokio::test(start_paused = true)]
async fn materialized_advance_wakes_without_timer() {
    let gate = RepoTxGate::fresh();
    let probe = Arc::new(WakeProbe::default());
    let mut wait = Box::pin(gate.await_visible(1));
    assert!(poll_once(wait.as_mut(), &probe).is_pending());
    assert_eq!(gate.completion().waiter_count(), 1);

    gate.completion().mark(1, State::Materialized);
    // No advance/await of the runtime: missing notify cannot be timer-masked.
    assert!(probe.woken.load(Ordering::SeqCst), "no completion wake");
    assert_eq!(
        poll_once(wait.as_mut(), &probe),
        Poll::Ready(VisibleOutcome::Visible),
    );
    assert_eq!(gate.visible_floor(), 1);
    assert_eq!(gate.completion().waiter_count(), 0);
}

#[tokio::test(start_paused = true)]
async fn aborted_advance_wakes_without_timer() {
    let gate = RepoTxGate::fresh();
    let probe = Arc::new(WakeProbe::default());
    let mut wait = Box::pin(gate.await_visible(1));
    assert!(poll_once(wait.as_mut(), &probe).is_pending());
    gate.completion().mark(1, State::Aborted);
    assert!(probe.woken.load(Ordering::SeqCst), "abort did not wake");
    assert_eq!(
        poll_once(wait.as_mut(), &probe),
        Poll::Ready(VisibleOutcome::Visible),
    );
    assert_eq!(gate.completion().waiter_count(), 0);
}

#[tokio::test(start_paused = true)]
async fn out_of_order_11_then_10_releases_both_waiters() {
    let gate = RepoTxGate::new(9, 1);
    // Eager publication is intentionally NOT the visibility floor.
    gate.publish_committed_max(11);
    assert_eq!(gate.visible_floor(), 9);
    let probe10 = Arc::new(WakeProbe::default());
    let probe11 = Arc::new(WakeProbe::default());
    let mut wait10 = Box::pin(gate.await_visible(10));
    let mut wait11 = Box::pin(gate.await_visible(11));
    assert!(poll_once(wait10.as_mut(), &probe10).is_pending());
    assert!(poll_once(wait11.as_mut(), &probe11).is_pending());

    gate.completion().mark(11, State::Materialized);
    assert_eq!(gate.visible_floor(), 9);
    assert!(!probe11.woken.load(Ordering::SeqCst));
    assert!(poll_once(wait11.as_mut(), &probe11).is_pending());
    gate.completion().mark(10, State::Materialized);
    assert!(probe10.woken.load(Ordering::SeqCst));
    assert!(probe11.woken.load(Ordering::SeqCst));
    assert_eq!(
        poll_once(wait10.as_mut(), &probe10),
        Poll::Ready(VisibleOutcome::Visible),
    );
    assert_eq!(
        poll_once(wait11.as_mut(), &probe11),
        Poll::Ready(VisibleOutcome::Visible),
    );
    assert_eq!(gate.visible_floor(), 11);
    assert_eq!(gate.completion().waiter_count(), 0);
}

#[tokio::test(start_paused = true)]
async fn one_advance_broadcasts_to_every_registered_waiter() {
    let gate = RepoTxGate::fresh();
    let probes: Vec<_> = (0..16).map(|_| Arc::new(WakeProbe::default())).collect();
    let mut waits: Vec<_> = (0..16).map(|_| Box::pin(gate.await_visible(1))).collect();
    for (wait, probe) in waits.iter_mut().zip(&probes) {
        assert!(poll_once(wait.as_mut(), probe).is_pending());
    }
    assert_eq!(gate.completion().waiter_count(), 16);
    gate.completion().mark(1, State::Materialized);
    for (wait, probe) in waits.iter_mut().zip(&probes) {
        // Replacing notify_waiters with notify_one fails here.
        assert!(probe.woken.load(Ordering::SeqCst));
        assert_eq!(
            poll_once(wait.as_mut(), probe),
            Poll::Ready(VisibleOutcome::Visible),
        );
    }
    assert_eq!(gate.completion().waiter_count(), 0);
}

#[test]
fn thread_settlement_races_registration_without_timer_rescue() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .start_paused(true)
        .build()
        .expect("test runtime");
    let _entered = runtime.enter();
    // No block_on: the timer driver NEVER runs during any iteration. A pending
    // poll must have received a real wake from settlement, not a backstop tick.
    for iteration in 0..1_000 {
        let gate = Arc::new(RepoTxGate::fresh());
        let barrier = Arc::new(Barrier::new(2));
        let publisher_gate = Arc::clone(&gate);
        let publisher_barrier = Arc::clone(&barrier);
        let publisher = std::thread::spawn(move || {
            publisher_barrier.wait();
            publisher_gate.completion().mark(1, State::Materialized);
        });
        let probe = Arc::new(WakeProbe::default());
        let mut wait = Box::pin(gate.await_visible(1));
        barrier.wait();
        let first = poll_once(wait.as_mut(), &probe);
        publisher.join().expect("settlement thread panicked");
        match first {
            Poll::Ready(outcome) => assert_eq!(outcome, VisibleOutcome::Visible),
            Poll::Pending => {
                assert!(
                    probe.woken.load(Ordering::SeqCst),
                    "missed registration wake at iteration {iteration}",
                );
                assert_eq!(
                    poll_once(wait.as_mut(), &probe),
                    Poll::Ready(VisibleOutcome::Visible),
                );
            }
        }
        assert_eq!(gate.visible_floor(), 1);
        assert_eq!(gate.completion().waiter_count(), 0);
    }
}

#[tokio::test(start_paused = true)]
async fn reopen_during_wait_is_detected_by_50ms_backstop() {
    let gate = RepoTxGate::fresh();
    let probe = Arc::new(WakeProbe::default());
    let mut wait = Box::pin(gate.await_visible(1));
    assert!(poll_once(wait.as_mut(), &probe).is_pending());
    gate.set_requires_reopen();
    // The setter deliberately does not notify. Test actual timer-driven wake,
    // not an unconditional manual re-poll that would conceal a missing timer.
    assert!(!probe.woken.load(Ordering::SeqCst));
    tokio::time::advance(Duration::from_millis(49)).await;
    assert!(!probe.woken.load(Ordering::SeqCst));
    tokio::time::advance(Duration::from_millis(1)).await;
    // Await an explicit waker acknowledgement; no fixed yield count is used
    // as a proxy for the timer driver having run.
    probe.signal.notified().await;
    assert!(probe.woken.load(Ordering::SeqCst), "backstop did not wake");
    assert_eq!(
        poll_once(wait.as_mut(), &probe),
        Poll::Ready(VisibleOutcome::ReopenRequired),
    );
    assert_eq!(gate.visible_floor(), 0);
    assert_eq!(gate.completion().waiter_count(), 0);
}

#[tokio::test(start_paused = true)]
async fn cancellation_restores_counter_and_removes_notification() {
    let gate = RepoTxGate::fresh();
    let cancelled_probe = Arc::new(WakeProbe::default());
    let live_probe = Arc::new(WakeProbe::default());
    let mut cancelled = Box::pin(gate.await_visible(1));
    let mut live = Box::pin(gate.await_visible(1));
    assert!(poll_once(cancelled.as_mut(), &cancelled_probe).is_pending());
    assert!(poll_once(live.as_mut(), &live_probe).is_pending());
    assert_eq!(gate.completion().waiter_count(), 2);
    drop(cancelled);
    assert_eq!(gate.completion().waiter_count(), 1);
    gate.completion().mark(1, State::Materialized);
    assert!(!cancelled_probe.woken.load(Ordering::SeqCst));
    assert!(live_probe.woken.load(Ordering::SeqCst));
    assert_eq!(
        poll_once(live.as_mut(), &live_probe),
        Poll::Ready(VisibleOutcome::Visible),
    );
    assert_eq!(gate.completion().waiter_count(), 0);

    let mut cancelled = Box::pin(gate.await_visible(2));
    assert!(poll_once(cancelled.as_mut(), &cancelled_probe).is_pending());
    drop(cancelled);
    assert_eq!(gate.completion().waiter_count(), 0);
    gate.completion().mark(2, State::Aborted);
    assert_eq!(gate.visible_floor(), 2);
}

#[test]
fn already_visible_needs_no_runtime_or_registration() {
    let gate = RepoTxGate::new(11, 1);
    let probe = Arc::new(WakeProbe::default());
    // Stack pinning and no Tokio runtime: any timer construction would panic.
    for version in [0, 10, 11] {
        let mut wait = std::pin::pin!(gate.await_visible(version));
        assert_eq!(
            poll_once(wait.as_mut(), &probe),
            Poll::Ready(VisibleOutcome::Visible),
        );
    }
    // Unlike the current gauge, this catches transient register/unregister RMW.
    assert_eq!(gate.completion().waiter_registrations(), 0);
    assert_eq!(gate.completion().waiter_count(), 0);
    assert_eq!(gate.completion().visible_wait_slow_count(), 0);
}

#[test]
fn already_reopen_returns_without_registration() {
    let gate = RepoTxGate::fresh();
    gate.set_requires_reopen();
    let probe = Arc::new(WakeProbe::default());
    let mut wait = std::pin::pin!(gate.await_visible(1));
    assert_eq!(
        poll_once(wait.as_mut(), &probe),
        Poll::Ready(VisibleOutcome::ReopenRequired),
    );
    assert_eq!(gate.completion().waiter_registrations(), 0);
}

#[tokio::test(start_paused = true)]
async fn slow_wait_counts_once_at_5s_and_never_gives_up() {
    let gate = RepoTxGate::fresh();
    let probe = Arc::new(WakeProbe::default());
    let mut wait = Box::pin(gate.await_visible(3));
    assert!(poll_once(wait.as_mut(), &probe).is_pending());
    tokio::time::advance(Duration::from_millis(4_999)).await;
    gate.completion().mark(1, State::Aborted);
    probe.signal.notified().await;
    assert!(poll_once(wait.as_mut(), &probe).is_pending());
    assert_eq!(gate.completion().visible_wait_slow_count(), 0);
    tokio::time::advance(Duration::from_millis(1)).await;
    // Wake exactly at the threshold without relying on the refreshed timer.
    // Settling 2 leaves target 3 pending and must not reset the wait's age.
    gate.completion().mark(2, State::Materialized);
    probe.signal.notified().await;
    assert!(poll_once(wait.as_mut(), &probe).is_pending());
    assert_eq!(gate.completion().visible_wait_slow_count(), 1);
    tokio::time::advance(Duration::from_secs(10)).await;
    probe.signal.notified().await;
    assert!(poll_once(wait.as_mut(), &probe).is_pending());
    assert_eq!(gate.completion().visible_wait_slow_count(), 1);
    gate.completion().mark(3, State::Materialized);
    assert_eq!(
        poll_once(wait.as_mut(), &probe),
        Poll::Ready(VisibleOutcome::Visible),
    );
    assert_eq!(gate.completion().waiter_count(), 0);
    assert_eq!(gate.completion().visible_wait_slow_count(), 1);
}
