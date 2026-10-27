//! WalSink truncation-probe tests.

use std::sync::mpsc;
use std::time::Duration;

use crate::wal_sink::WalSink;

/// Drop-guard living INSIDE the `thread::scope` closure (its first
/// statement): signals the holder thread to release the frames mutex when the
/// closure unwinds — before the scope joins — so `thread::scope` can never
/// hang on unwind.
struct Release<'a>(&'a mpsc::Sender<()>);
impl Drop for Release<'_> {
    fn drop(&mut self) {
        let _ = self.0.send(());
    }
}

/// RV-WAL-B8E9884C: `has_truncatable` on a `Mem` sink must answer in O(1)
/// from the atomic mirror, never by scanning `frames` under its mutex.
///
/// Pure-std threads (the probe is synchronous; a Tokio timeout can neither
/// interrupt a blocking `Mutex::lock` nor tolerate a builder without the
/// time driver). The holder thread owns the frames mutex, proves it over a
/// handshake channel BEFORE the probe is spawned. The `Release` guard lives
/// INSIDE the scope closure (first statement): on the failure path the test
/// assertion panics, the closure unwinds through the guard BEFORE the scope's
/// join, the guard drops and sends the release, the holder's `recv` returns,
/// the holder releases the mutex, the probe unblocks and both join — the
/// test fails instead of hanging.
/// The probe runs with no deadline: the test thread's 10 s
/// `recv_timeout` is the failure detector — on the pre-fix code the probe
/// blocks on the held mutex, the deadline expires, and the assertion panics
/// while the drop-guard still releases the holder.
#[test]
fn rv_wal_b8e9884c_has_truncatable_does_not_take_frames_mutex() {
    let sink = WalSink::mem();
    // Two frames: one pinned (max_version 0), one truncatable at 10.
    // `append_batch` is async: a tiny current-thread runtime on the test
    // thread, torn down before the pure-std section below.
    {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime");
        rt.block_on(async {
            sink.append_batch(vec![b"pinned".to_vec()], 0)
                .await
                .unwrap();
            sink.append_batch(vec![b"x".to_vec()], 10).await.unwrap();
        });
    }
    let WalSink::Mem(mem) = &sink else {
        panic!("expected Mem sink");
    };
    let frames = mem.frames_mutex_for_test();

    let (release_tx, release_rx) = mpsc::channel::<()>();
    let (held_tx, held_rx) = mpsc::channel::<()>();
    let (probe_tx, probe_rx) = mpsc::channel();
    std::thread::scope(|s| {
        // FIRST statement in the closure: the guard must drop during THIS
        // closure's unwind — before `thread::scope` joins the spawned
        // threads — or the holder would still be blocked on `recv` at join
        // time. Any panic in the closure therefore releases the holder.
        let _guard = Release(&release_tx);
        // Holder: take the mutex, prove ownership, then block until released.
        // It receives ONLY `release_rx`; the sender stays with the guard
        // above, so a closure panic still triggers the release.
        s.spawn(move || {
            let _held = frames.lock().expect("test holds frames mutex");
            held_tx.send(()).expect("holder handshake send");
            let _ = release_rx.recv();
        });
        // Handshake FIRST: the probe can only matter once the mutex is
        // verifiably held by another thread.
        held_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("holder thread never acquired the frames mutex");
        // Probe: lock-free mirror must answer while the mutex is held.
        s.spawn(|| {
            let results = (sink.has_truncatable(0), sink.has_truncatable(10));
            let _ = probe_tx.send(results);
        });
        let (at_zero, at_ten) = probe_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("has_truncatable must not block on the frames mutex (10s deadline)");
        // Frames exist (one pinned, one at version 10), so the mirror must
        // still answer correctly even though it never took the lock.
        assert!((at_zero, at_ten) == (false, true));
    });
}

/// RV-WAL-B8E9884C: the mirror must track every mutation site — append,
/// truncate — so `has_truncatable` stays exact without a scan.
#[tokio::test]
async fn rv_wal_b8e9884c_mirror_tracks_append_and_truncate() {
    let sink = WalSink::mem();
    assert!(!sink.has_truncatable(5), "empty sink: nothing truncatable");

    // Batch with max_version 0 pins the frame (never eligible).
    sink.append_batch(vec![b"pinned".to_vec()], 0)
        .await
        .unwrap();
    assert!(!sink.has_truncatable(u64::MAX), "pinned frame only");

    sink.append_batch(vec![b"a".to_vec()], 3).await.unwrap();
    sink.append_batch(vec![b"b".to_vec()], 7).await.unwrap();
    assert!(!sink.has_truncatable(2), "nothing <= 2 yet");
    assert!(sink.has_truncatable(3), "frame 3 qualifies");
    assert!(sink.has_truncatable(7), "frame 7 qualifies");

    let n = sink.truncate_below(3).await.unwrap();
    assert_eq!(n, 1, "frame 3 dropped");
    assert!(!sink.has_truncatable(3), "mirror updated after truncate");
    assert!(sink.has_truncatable(7), "frame 7 still there");

    let n = sink.truncate_below(7).await.unwrap();
    assert_eq!(n, 1, "frame 7 dropped");
    assert!(!sink.has_truncatable(7), "mirror empty after full truncate");
    assert!(!sink.has_truncatable(u64::MAX));
}
