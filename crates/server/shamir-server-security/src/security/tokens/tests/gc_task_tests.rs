//! `gc_task_finished` follows the collector task's real end, not the state of
//! the join-handle slot.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

use super::service_tests::service;
use crate::security::tokens::TokenService;

/// A service whose clock parks once armed, until the returned sender is dropped.
/// The collector's first pass reads the clock, so an armed clock parks the
/// collector; the clock only parks once armed, because opening the service and
/// starting the collector must not block.
async fn service_with_parkable_clock(
    temp: &TempDir,
) -> (TokenService, Arc<AtomicBool>, mpsc::Sender<()>) {
    let now = 30_000_000_000_u64;
    let armed = Arc::new(AtomicBool::new(false));
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let release_rx = Mutex::new(release_rx);
    let clock_armed = Arc::clone(&armed);
    let service = service(
        temp,
        Arc::new(move || {
            if clock_armed.load(Ordering::SeqCst) {
                // Parks until the sender is dropped (an `Err` also returns).
                let _ = release_rx.lock().expect("release receiver").recv();
            }
            Some(now)
        }),
    )
    .await;
    (service, armed, release_tx)
}

async fn wait_until_finished(service: &TokenService) {
    tokio::time::timeout(Duration::from_secs(30), async {
        while !service.gc_task_finished().await {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the collector must end once the clock is released");
}

/// A `shutdown_gc` abandoned at its deadline detaches the join handle while the
/// collector is still running; `gc_task_finished` must then stay false until the
/// collector really ends (it used to read true once the handle slot was empty
/// and the stop was requested).
///
/// Mutant: reading the handle slot (or the stop token) instead of the task's own
/// end fails the first assertion.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gc_task_finished_tracks_real_completion_after_an_abandoned_shutdown() {
    let temp = TempDir::new().expect("tempdir");
    let (service, armed, release_tx) = service_with_parkable_clock(&temp).await;
    armed.store(true, Ordering::SeqCst);
    service.start_gc(CancellationToken::new()).await;

    let abandoned = tokio::time::timeout(Duration::from_millis(200), service.shutdown_gc()).await;
    assert!(abandoned.is_err(), "a parked collector cannot be joined");
    assert!(
        !service.gc_task_finished().await,
        "a detached collector that still runs is not finished"
    );

    drop(release_tx);
    wait_until_finished(&service).await;
}

/// A collector restarted after a completed `shutdown_gc` (the deadline tests do
/// this) is a new live task: the first collector's end must not be reported for
/// it.
///
/// Mutant: one exit flag that is never cleared keeps reading true after the
/// restart.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_restarted_collector_is_not_reported_finished() {
    let temp = TempDir::new().expect("tempdir");
    let (service, armed, release_tx) = service_with_parkable_clock(&temp).await;
    service.start_gc(CancellationToken::new()).await;
    service.shutdown_gc().await;
    assert!(
        service.gc_task_finished().await,
        "the first collector has ended"
    );

    armed.store(true, Ordering::SeqCst);
    service.start_gc(CancellationToken::new()).await;
    assert!(
        !service.gc_task_finished().await,
        "a restarted collector that still runs is not finished"
    );

    drop(release_tx);
    wait_until_finished(&service).await;
}
