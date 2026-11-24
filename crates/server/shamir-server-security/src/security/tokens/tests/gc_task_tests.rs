//! `gc_task_finished` follows the collector tasks' real end, not the state of
//! the join-handle slot.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

use super::service_tests::service;
use crate::security::tokens::TokenService;

/// How many collector passes the clock can park, one gate each.
const PARK_SLOTS: usize = 2;

/// A service whose clock parks once armed: the N-th armed read announces itself
/// on `entered`, then waits until its own sender is dropped. A collector pass
/// reads the clock exactly once before its storage work, so a read is one
/// collector. The clock only parks once armed, because opening the service and
/// starting a collector must not block.
struct ParkableClock {
    service: TokenService,
    armed: Arc<AtomicBool>,
    entered: mpsc::Receiver<usize>,
    releases: Vec<Option<mpsc::Sender<()>>>,
}

impl ParkableClock {
    async fn open(temp: &TempDir) -> Self {
        let now = 30_000_000_000_u64;
        let armed = Arc::new(AtomicBool::new(false));
        let reads = Arc::new(AtomicUsize::new(0));
        let (entered_tx, entered) = mpsc::channel::<usize>();
        let entered_tx = Mutex::new(entered_tx);
        let mut releases = Vec::new();
        let mut gates = Vec::new();
        for _ in 0..PARK_SLOTS {
            let (release, gate) = mpsc::channel::<()>();
            releases.push(Some(release));
            gates.push(Mutex::new(gate));
        }
        let clock_armed = Arc::clone(&armed);
        let service = service(
            temp,
            Arc::new(move || {
                if clock_armed.load(Ordering::SeqCst) {
                    let slot = reads.fetch_add(1, Ordering::SeqCst);
                    if let Some(gate) = gates.get(slot) {
                        let _ = entered_tx.lock().expect("entered sender").send(slot);
                        // Parks until the sender is dropped (an `Err` also returns).
                        let _ = gate.lock().expect("gate receiver").recv();
                    }
                }
                Some(now)
            }),
        )
        .await;
        Self {
            service,
            armed,
            entered,
            releases,
        }
    }

    fn arm(&self) {
        self.armed.store(true, Ordering::SeqCst);
    }

    /// Blocks until clock read `slot` is parked (the collector is mid-pass).
    fn await_parked(&self, slot: usize) {
        let entered = self
            .entered
            .recv_timeout(Duration::from_secs(30))
            .expect("the collector must reach the clock");
        assert_eq!(entered, slot, "collector passes must park in start order");
    }

    fn release(&mut self, slot: usize) {
        drop(self.releases[slot].take());
    }

    async fn wait_until_finished(&self) {
        tokio::time::timeout(Duration::from_secs(30), async {
            while !self.service.gc_task_finished().await {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("every collector must end once the clock is released");
    }
}

/// A `shutdown_gc` abandoned at its deadline detaches the join handle while the
/// collector is still running; `gc_task_finished` must then stay false until the
/// collector really ends (it used to read true once the handle slot was empty
/// and the stop was requested).
///
/// Mutant: reading the handle slot (or the stop token) instead of the task's own
/// end fails the first assertion.
#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn gc_task_finished_tracks_real_completion_after_an_abandoned_shutdown() {
    let temp = TempDir::new().expect("tempdir");
    let mut clock = ParkableClock::open(&temp).await;
    clock.arm();
    clock.service.start_gc(CancellationToken::new()).await;
    clock.await_parked(0);

    let abandoned =
        tokio::time::timeout(Duration::from_millis(200), clock.service.shutdown_gc()).await;
    assert!(abandoned.is_err(), "a parked collector cannot be joined");
    assert!(
        !clock.service.gc_task_finished().await,
        "a detached collector that still runs is not finished"
    );

    clock.release(0);
    clock.wait_until_finished().await;
}

/// A collector restarted after a completed `shutdown_gc` (the deadline tests do
/// this) is a new live task: the first collector's end must not be reported for
/// it.
///
/// Mutant: one exit flag that is never cleared keeps reading true after the
/// restart.
#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn a_restarted_collector_is_not_reported_finished() {
    let temp = TempDir::new().expect("tempdir");
    let mut clock = ParkableClock::open(&temp).await;
    clock.service.start_gc(CancellationToken::new()).await;
    clock.service.shutdown_gc().await;
    assert!(
        clock.service.gc_task_finished().await,
        "the first collector has ended"
    );

    clock.arm();
    clock.service.start_gc(CancellationToken::new()).await;
    clock.await_parked(0);
    assert!(
        !clock.service.gc_task_finished().await,
        "a restarted collector that still runs is not finished"
    );

    clock.release(0);
    clock.wait_until_finished().await;
}

/// Two collectors alive at once (an abandoned `shutdown_gc`, then a restart):
/// the first one ending must not report finished while the second still runs.
///
/// The window after the first release only ever lets a mutant fail (an exit flag
/// that the first collector's end sets turns the answer true within
/// milliseconds); the correct implementation never reads true in it.
#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn the_first_collector_ending_does_not_finish_a_live_second_one() {
    let temp = TempDir::new().expect("tempdir");
    let mut clock = ParkableClock::open(&temp).await;
    clock.arm();
    clock.service.start_gc(CancellationToken::new()).await;
    clock.await_parked(0);
    let abandoned =
        tokio::time::timeout(Duration::from_millis(200), clock.service.shutdown_gc()).await;
    assert!(abandoned.is_err(), "a parked collector cannot be joined");

    clock.service.start_gc(CancellationToken::new()).await;
    clock.await_parked(1);

    clock.release(0);
    for _ in 0..50 {
        assert!(
            !clock.service.gc_task_finished().await,
            "the second collector is still parked"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    clock.release(1);
    clock.wait_until_finished().await;
}
