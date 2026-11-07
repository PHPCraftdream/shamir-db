//! Test/verification seam for the Fjall write worker's `submit` admission
//! path. Child module of `storage_fjall` (declared there with `#[path]`),
//! so `use super::*` keeps giving access to the private `submit`/`WriteJob`
//! with NO visibility widening.

use super::*;

fn tjob(reply: oneshot::Sender<DbResult<()>>) -> WriteJob {
    WriteJob::Transact {
        ops: Vec::new(),
        reply,
    }
}

pub async fn submit_closed() -> DbResult<()> {
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    drop(rx);
    submit(&tx, tjob).await
}

pub async fn submit_dropped_reply() -> DbResult<()> {
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    let worker = std::thread::spawn(move || {
        if let Ok(WriteJob::Transact { reply, .. }) = rx.recv() {
            drop(reply);
        }
    });
    let result = submit(&tx, tjob).await;
    drop(tx);
    let _ = worker.join();
    result
}

/// Test-only, uncontended saturation signal (RV-STORAGE-9E9A0A71 round 2):
/// fired by `submit`'s `TrySendError::Full` arm, so a test can PROVE the
/// submitter actually observed a full queue before releasing the worker.
static SUBMIT_FULL_SIGNAL: std::sync::Mutex<Option<tokio::sync::oneshot::Sender<()>>> =
    std::sync::Mutex::new(None);

/// Arm the signal; returns the receiver the test awaits.
pub(crate) fn arm_submit_full_signal() -> oneshot::Receiver<()> {
    let (tx, rx) = oneshot::channel();
    *SUBMIT_FULL_SIGNAL.lock().unwrap() = Some(tx);
    rx
}

/// Fire the saturation signal, if armed (test-only).
pub(crate) fn notify_submit_full_for_tests() {
    if let Some(tx) = SUBMIT_FULL_SIGNAL.lock().unwrap().take() {
        let _ = tx.send(());
    }
}

/// RV-STORAGE-9E9A0A71 oracle: admitting against a FULL bounded queue must
/// not block the polling executor thread. The worker drains only once
/// `release_rx` fires — producible only by another task polled on the SAME
/// runtime while `submit` is pending — so blocking admission deadlocks it.
pub async fn submit_full_queue_pends_until_released(
    release_rx: oneshot::Receiver<()>,
) -> DbResult<()> {
    let (tx, rx) = std::sync::mpsc::sync_channel::<WriteJob>(1);
    tx.send(tjob(oneshot::channel().0)).ok();
    let worker = std::thread::spawn(move || {
        if release_rx.blocking_recv().is_err() {
            return;
        }
        while let Ok(job) = rx.recv() {
            if let WriteJob::Transact { reply, .. } = job {
                let _ = reply.send(Ok(()));
            }
        }
    });
    let result = submit(&tx, tjob).await;
    drop(tx);
    let _ = worker.join();
    result
}
