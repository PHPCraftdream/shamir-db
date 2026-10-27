//! Regression tests for the 2026-10-26 Fjall remediation stage F2
//! (RV-STORAGE-CC8D9EB5, RV-STORAGE-9E9A0A71).

#![allow(deprecated)]

use crate::storage_fjall::FjallRepo;
use crate::types::{RecordKey, Repo, Store};
use bytes::Bytes;
use futures::StreamExt;
use std::fs;

fn fresh_path(tag: &str) -> String {
    let path = format!("./test_data/fjall_remediation_{tag}");
    if std::path::Path::new(&path).exists() {
        fs::remove_dir_all(&path).unwrap();
    }
    path
}

#[tokio::test]
async fn rv_storage_5e99bede_fjall_zero_batch_is_empty() {
    let path = fresh_path("5e99bede");
    let repo = FjallRepo::new(&path).unwrap();
    let store = repo.store_get("zero-batch").await.unwrap();
    store
        .set(
            RecordKey::from(Bytes::from_static(b"present")),
            Bytes::from_static(b"v"),
        )
        .await
        .unwrap();
    assert!(store.iter_stream(0).next().await.is_none());
    assert!(store
        .scan_prefix_stream(Bytes::from_static(b"p"), 0)
        .next()
        .await
        .is_none());
    assert!(store
        .iter_range_stream(None, None, 0)
        .next()
        .await
        .is_none());
    assert!(store
        .iter_range_stream_reverse(None, None, 0)
        .next()
        .await
        .is_none());
    drop(store);
    drop(repo);
    fs::remove_dir_all(path).ok();
}

/// RV-STORAGE-CC8D9EB5: the pinned fjall 3.1.6 `WriteBatch::commit` applies
/// batch items to the memtables individually and only afterwards publishes
/// the snapshot watermark. The pinned API DOES expose publication-bounded
/// snapshot reads (`Database::snapshot()` reads at the nonce instant), but
/// this shim's raw read paths (`Keyspace::get`/`range`/`prefix`) read at
/// `SeqNo::MAX`, unclamped by the published watermark — a reader running
/// while a batch is mid-apply can observe a partially-applied batch. The
/// card-sanctioned remedy is to WITHDRAW the atomic-visibility capability
/// the shim advertised via `supports_atomic_transact() == true` rather than
/// reroute hot-path reads through held snapshots. The flag must stay
/// honestly `false` for the raw fjall backend (and every wrapper that
/// forwards to it, e.g. `CachedStore`).
#[tokio::test]
async fn rv_storage_cc8d9eb5_atomic_transact_visibility_capability_is_withdrawn() {
    let path = fresh_path("cc8d9eb5");
    let repo = FjallRepo::new(&path).unwrap();
    let store = repo.store_get("cc8d9eb5").await.unwrap();

    assert!(
        !store.supports_atomic_transact(),
        "raw fjall reads are SeqNo::MAX (publication-unbounded) on the pinned \
         3.1.6 backend, so native batch visibility atomicity must NOT be \
         advertised"
    );

    // A wrapper that forwards the inner flag inherits the withdrawn
    // capability rather than re-advertising it.
    let cached = crate::storage_cached::CachedStore::new_sync(store.clone())
        .await
        .unwrap();
    assert!(!cached.supports_atomic_transact());

    // Withdrawal is a static backend property, not per-state: it stays
    // `false` across writes on the same store instance.
    let key = RecordKey::from(Bytes::from_static(b"cc8d9eb5-key"));
    store.set(key, Bytes::from_static(b"v")).await.unwrap();
    assert!(!store.supports_atomic_transact());

    fs::remove_dir_all(&path).ok();
}

/// RV-STORAGE-9E9A0A71: `submit` used a blocking `SyncSender::send` into the
/// bounded 1024-slot worker queue, so saturation parked the EXECUTING tokio
/// runtime thread. Admission must now `try_send` and cooperatively
/// `yield_now` while the queue is full, so other tasks keep making progress
/// on the same runtime thread while admission is pending.
///
/// Oracle (OS-thread-based, no sleeps): the scenario runs on a DEDICATED OS
/// thread with its OWN current-thread runtime, and the TEST thread does the
/// timing out via `recv_timeout` — a real OS-level deadline. `tokio::time`
/// cannot interrupt a blocking `SyncSender::send` parked inside a poll, so
/// with the old blocking send the scenario thread parks inside `send`
/// forever and `recv_timeout` returns `Err` after 30 s, failing the test
/// instead of hanging CI. The saturation signal (`arm_submit_full_signal`,
/// fired by `submit` inside its `TrySendError::Full` arm) is kept, and the
/// release task fires only after that proof, so the worker drains only once
/// the submitter is provably stuck against a full queue.
#[test]
fn rv_storage_9e9a0a71_submit_admission_does_not_block_runtime_thread() {
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let result = rt.block_on(async {
            // Saturation proof: fires ONLY when `submit` actually observed a
            // full queue (armed inside the production `TrySendError::Full` arm).
            let full_rx = crate::storage_fjall::write_worker_test_seam::arm_submit_full_signal();
            let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
            let scenario =
                crate::storage_fjall::write_worker_test_seam::submit_full_queue_pends_until_released(
                    release_rx,
                );
            // Release fires ONLY after the submitter is provably saturated —
            // releasing earlier would not exercise the full-queue retry path.
            let release = async move {
                let _ = full_rx.await;
                let _ = release_tx.send(());
            };
            let (result, ()) = tokio::join!(scenario, release);
            result
        });
        let _ = done_tx.send(result);
    });
    // The deadline lives on the TEST thread: the old blocking-send deadlock
    // fails as a `recv_timeout` error instead of hanging the test.
    let result = done_rx
        .recv_timeout(std::time::Duration::from_secs(30))
        .expect("submit must not block the runtime thread while the worker queue is full");
    assert!(
        result.is_ok(),
        "worker drained after release; reply expected"
    );
    let _ = worker.join();
}
