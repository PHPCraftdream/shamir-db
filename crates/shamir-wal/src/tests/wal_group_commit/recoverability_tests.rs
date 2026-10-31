//! Exact coverage and sync phase regressions.
use super::*;
use crate::segment_set::sync_test_seam::InjectedSealedError;
use crate::wal_group_commit::WalCoverage;
use crate::wal_ticket::WalAppendError;
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::Poll;

/// Bounded test-only bitmap oracle (versions 1..=63). Production coverage
/// lives in engine's scc observer; this fixture never represents unbounded IDs.
struct TestCoverage {
    written: AtomicU64,
    covered: AtomicU64,
}
impl WalCoverage for TestCoverage {
    fn written(&self, versions: &[u64], _memory: bool) {
        for &version in versions {
            if version >= 64 {
                continue;
            } // unrelated legacy batching fixtures use larger IDs
            let bit = 1_u64 << version;
            self.written.fetch_or(bit, Ordering::AcqRel);
        }
    }
    fn memory_completed(&self, through: u64) {
        assert!(through < 64);
        self.covered
            .fetch_or((1_u64 << (through + 1)) - 2, Ordering::AcqRel);
    }
    fn capture(&self) -> u64 {
        self.written.load(Ordering::Acquire)
    }
    fn synced(&self, captured: u64) {
        self.covered.fetch_or(captured, Ordering::AcqRel);
    }
    fn settle(&self, version: u64) {
        assert!(version < 64);
        self.covered.fetch_or(1_u64 << version, Ordering::AcqRel);
    }
    fn recoverable_through(&self) -> u64 {
        let bits = self.covered.load(Ordering::Acquire);
        (1..64)
            .take_while(|version| bits & (1_u64 << version) != 0)
            .count() as u64
    }
}
pub(super) fn tracked_group(sink: Arc<WalSink>) -> WalGroupCommit {
    let group = WalGroupCommit::new(sink);
    group
        .install_coverage(Arc::new(TestCoverage {
            written: AtomicU64::new(0),
            covered: AtomicU64::new(0),
        }))
        .unwrap();
    group
}

// ===== R = contiguous recoverability prefix + coalesced sync_now =====

/// `R` is monotonic: exact synced bytes plus settled unwritten/history
/// predecessors advance the contiguous prefix, never an appended maximum.
#[tokio::test]
async fn recoverable_through_monotonic_and_never_claims_unsynced() {
    let dir = TempDir::new().unwrap();
    let sink = Arc::new(open_sink(&dir).await);
    let gc = tracked_group(Arc::clone(&sink));
    assert_eq!(gc.recoverable_through(), 0);

    gc.settle(1);
    gc.settle(2);
    gc.append(entry(1, 3).encode().unwrap(), 3, WalDurability::Synced)
        .await
        .unwrap();
    assert!(gc.recoverable_through() >= 3);

    // A Buffered append's bytes are NOT fsynced — R must stay below 5.
    gc.append(entry(2, 5).encode().unwrap(), 5, WalDurability::Buffered)
        .await
        .unwrap();
    assert!(
        gc.recoverable_through() < 5,
        "R claimed a version whose bytes were never fsynced"
    );

    gc.settle(4);
    gc.settle(6);
    gc.append(entry(3, 7).encode().unwrap(), 7, WalDurability::Synced)
        .await
        .unwrap();
    assert!(gc.recoverable_through() >= 7);
    assert!(gc.recoverable_through() <= sink.max_committed());
}

/// (a-bis) A failed fsync must NOT advance `R`.
#[tokio::test]
async fn sync_now_failure_does_not_advance_recoverable_through() {
    let dir = TempDir::new().unwrap();
    let sink = Arc::new(open_sink(&dir).await);
    let gc = tracked_group(Arc::clone(&sink));
    gc.append(entry(1, 1).encode().unwrap(), 1, WalDurability::Buffered)
        .await
        .unwrap();
    assert_eq!(gc.recoverable_through(), 0);
    gc.sync_now().await.unwrap();
    assert_eq!(gc.recoverable_through(), 1);
    gc.append(entry(2, 2).encode().unwrap(), 2, WalDurability::Buffered)
        .await
        .unwrap();
    gc.arm_fail_next_sync_for_test();
    assert!(gc.sync_now().await.is_err());
    assert_eq!(
        gc.recoverable_through(),
        1,
        "nonempty unsynced tail stays uncovered"
    );
    assert!(gc.is_dirty(), "failed worker re-arms timer admission");
    gc.sync_now().await.unwrap();
    assert_eq!(gc.recoverable_through(), 2);
    assert!(!gc.is_dirty());
}

/// (b) N concurrent `sync_now` calls coalesce into exactly ONE fsync.
/// The leader is frozen mid-fsync via the File pause handshake so every
/// caller deterministically registers before the round completes.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sync_now_coalesces_into_one_fsync() {
    let dir = TempDir::new().unwrap();
    let sink = Arc::new(open_sink(&dir).await);
    let gc = Arc::new(tracked_group(Arc::clone(&sink)));

    gc.settle(1);
    gc.append(entry(1, 2).encode().unwrap(), 2, WalDurability::Buffered)
        .await
        .unwrap();

    WalSink::arm_pause_next_file_sync();
    let mut handles = Vec::new();
    for _ in 0..8 {
        let gc = Arc::clone(&gc);
        handles.push(tokio::spawn(async move { gc.sync_now().await }));
    }
    // The elected leader is now parked INSIDE its (single) fsync.
    WalSink::wait_file_sync_paused().await;
    gc.wait_sync_waiters(8).await;

    WalSink::resume_file_sync();
    for handle in handles {
        let synced = handle.await.unwrap().unwrap();
        assert!(synced >= 2, "each caller must see R cover version 2");
    }
    assert_eq!(gc.fsync_count(), 1, "same target shares exactly one fsync");
    assert_eq!(gc.recoverable_through(), 2);
}

/// (c) A waiter parked on `R >= v` is NOT released by a Buffered append at
/// the same version, and IS released when the fsync publishes `R >= v`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wait_recoverable_released_when_fsync_publishes() {
    let dir = TempDir::new().unwrap();
    let sink = Arc::new(open_sink(&dir).await);
    let gc = Arc::new(tracked_group(Arc::clone(&sink)));

    for version in 1..10 {
        gc.settle(version);
    }
    let waiter = {
        let gc = Arc::clone(&gc);
        tokio::spawn(async move {
            gc.wait_recoverable(10, std::time::Duration::from_secs(10))
                .await
        })
    };

    // Buffered append at v = 10: bytes written but NOT fsynced — the waiter
    // must stay parked.
    gc.append(entry(1, 10).encode().unwrap(), 10, WalDurability::Buffered)
        .await
        .unwrap();
    assert_eq!(gc.recoverable_through(), 9);
    assert!(!waiter.is_finished());

    // The Synced window's fsync publishes R = 10 and releases the waiter.
    gc.append(entry(2, 10).encode().unwrap(), 10, WalDurability::Synced)
        .await
        .unwrap();
    assert!(waiter.await.unwrap(), "fsync must release the waiter");
    assert!(gc.recoverable_through() >= 10);
}

/// (d) RAM R follows completed W; raw append alone does not certify it.
#[tokio::test]
async fn mem_sink_recoverable_through_tracks_w_without_fsync() {
    let sink = Arc::new(WalSink::mem());
    let gc = tracked_group(Arc::clone(&sink));

    gc.settle(1);
    gc.append(entry(1, 2).encode().unwrap(), 2, WalDurability::Buffered)
        .await
        .unwrap();
    for version in 3..6 {
        gc.settle(version);
    }
    gc.append(entry(2, 6).encode().unwrap(), 6, WalDurability::Buffered)
        .await
        .unwrap();

    assert_eq!(
        gc.recoverable_through(),
        1,
        "append alone is not completion W"
    );
    gc.coverage_observer().unwrap().memory_completed(6);
    assert_eq!(gc.fsync_count(), 0, "Mem sink must never fsync");
    assert_eq!(gc.recoverable_through(), 6, "R must equal W on a Mem sink");
    assert_eq!(sink.max_committed(), 6);
}

/// B3 point 5 (holes): R never skips a hole. A version whose coverage is
/// absent (Buffered-only append = out-of-order coverage; a failed sync) keeps
/// R below it; `settle` marks an aborted/unwritten version covered so the
/// next contiguous covered run carries R past it.
#[tokio::test]
async fn recoverable_prefix_never_skips_a_hole_until_settled() {
    let dir = TempDir::new().unwrap();
    let sink = Arc::new(open_sink(&dir).await);
    let gc = tracked_group(Arc::clone(&sink));

    // v3 fsynced; v5 written but never synced (simulates a version whose
    // coverage failed / an aborted allocation at 4..=5).
    gc.settle(1);
    gc.settle(2);
    gc.append(entry(1, 3).encode().unwrap(), 3, WalDurability::Synced)
        .await
        .unwrap();
    gc.append(entry(2, 5).encode().unwrap(), 5, WalDurability::Buffered)
        .await
        .unwrap();
    assert_eq!(gc.recoverable_through(), 3, "R stays below the hole at 4");

    // Settling ONLY the hole at 4 must NOT let R pass 5 (still uncovered).
    gc.settle(4);
    assert_eq!(gc.recoverable_through(), 4, "R never jumps an uncovered 5");

    // Covering 5 (a successful sync) carries R contiguously past the hole.
    gc.sync_now().await.unwrap();
    assert!(
        gc.recoverable_through() >= 5,
        "settled hole + sync: R advances"
    );
}

/// B3 point 5 (shared work): a failed sync does not advance R, and a
/// CANCELLED waiter does not cancel or abandon the shared sync — other
/// waiters still complete and a subsequent sync works.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn failed_sync_and_cancelled_waiter_leave_shared_sync_intact() {
    let dir = TempDir::new().unwrap();
    let sink = Arc::new(open_sink(&dir).await);
    let gc = Arc::new(tracked_group(Arc::clone(&sink)));
    gc.append(entry(1, 1).encode().unwrap(), 1, WalDurability::Buffered)
        .await
        .unwrap();
    WalSink::arm_pause_next_file_sync();
    sink.arm_fail_next_sync();
    let leader = {
        let gc = Arc::clone(&gc);
        tokio::spawn(async move { gc.sync_now().await })
    };
    WalSink::wait_file_sync_paused().await;
    let follower = {
        let gc = Arc::clone(&gc);
        tokio::spawn(async move { gc.sync_now().await })
    };
    gc.wait_sync_waiters(2).await;
    leader.abort();
    assert!(leader.await.unwrap_err().is_cancelled());
    WalSink::resume_file_sync();
    assert!(follower.await.unwrap().is_err());
    assert_eq!(gc.recoverable_through(), 0);
    assert_eq!(gc.instance_sync_errors(), 1);
    assert!(gc.is_dirty());
    gc.sync_now().await.unwrap();
    assert_eq!(gc.recoverable_through(), 1);
    assert_eq!(gc.fsync_count(), 2);
}

/// Regression: cancelling the first demander must not strand shared work.
#[tokio::test]
async fn cancelled_leader_waiter_does_not_abandon_sync() {
    let sink = Arc::new(WalSink::mem());
    let gc = Arc::new(tracked_group(Arc::clone(&sink)));
    sink.arm_pause_next_sync();
    let first = {
        let gc = Arc::clone(&gc);
        tokio::spawn(async move { gc.sync_now().await })
    };
    sink.wait_sync_paused().await;
    first.abort();
    assert!(first.await.unwrap_err().is_cancelled());
    let remaining = {
        let gc = Arc::clone(&gc);
        tokio::spawn(async move { gc.sync_now().await })
    };
    gc.wait_sync_waiters(1).await;
    sink.resume_sync();
    remaining.await.unwrap().unwrap();
    assert_eq!(
        gc.fsync_count(),
        1,
        "same captured target joins the existing cycle"
    );
}

#[tokio::test]
async fn one_demand_runs_exactly_one_sync() {
    let gc = WalGroupCommit::new(Arc::new(WalSink::mem()));
    gc.sync_now().await.unwrap();
    assert_eq!(gc.fsync_count(), 1);
    gc.sync_now().await.unwrap();
    assert_eq!(gc.fsync_count(), 1, "covered target needs no extra fsync");
}

#[tokio::test]
async fn late_demand_cannot_complete_with_first_cycle() {
    let sink = Arc::new(WalSink::mem());
    let gc = Arc::new(tracked_group(Arc::clone(&sink)));
    sink.arm_pause_next_sync();
    let first = {
        let gc = Arc::clone(&gc);
        tokio::spawn(async move { gc.sync_now().await })
    };
    sink.wait_sync_paused().await;
    gc.append(entry(1, 1).encode().unwrap(), 1, WalDurability::Buffered)
        .await
        .unwrap();
    let late = {
        let gc = Arc::clone(&gc);
        tokio::spawn(async move { gc.sync_now().await })
    };
    gc.wait_sync_waiters(2).await;
    sink.arm_pause_next_sync();
    sink.resume_sync();
    sink.wait_sync_paused().await;
    first.await.unwrap().unwrap();
    assert!(
        !late.is_finished(),
        "late ticket waits for second captured cycle"
    );
    sink.resume_sync();
    late.await.unwrap().unwrap();
    assert_eq!(gc.fsync_count(), 2);
}

/// A worker unwind must release leadership and wake its accepted demands.
#[tokio::test]
async fn worker_panic_releases_leadership_and_allows_retry() {
    struct PanicOnce {
        armed: std::sync::atomic::AtomicBool,
    }
    impl WalCoverage for PanicOnce {
        fn written(&self, _versions: &[u64], _memory: bool) {}
        fn capture(&self) -> u64 {
            assert!(
                !self.armed.swap(false, Ordering::AcqRel),
                "injected worker panic"
            );
            0
        }
        fn synced(&self, _generation: u64) {}
        fn settle(&self, _version: u64) {}
        fn recoverable_through(&self) -> u64 {
            0
        }
    }
    let dir = TempDir::new().unwrap();
    let gc = WalGroupCommit::new(Arc::new(open_sink(&dir).await));
    gc.install_coverage(Arc::new(PanicOnce {
        armed: std::sync::atomic::AtomicBool::new(true),
    }))
    .unwrap();
    assert!(gc.sync_now().await.is_err());
    assert_eq!(gc.sync_waiters(), 0);
    assert_eq!(gc.instance_sync_errors(), 1);
    gc.sync_now().await.unwrap();
    assert_eq!(gc.fsync_count(), 1);
}

/// A write during the captured fsync must survive the workers dirty clear.
#[tokio::test]
async fn postcapture_write_remains_dirty_until_next_sync() {
    let dir = TempDir::new().unwrap();
    let sink = Arc::new(open_sink(&dir).await);
    let gc = Arc::new(tracked_group(Arc::clone(&sink)));
    gc.append(entry(1, 1).encode().unwrap(), 1, WalDurability::Buffered)
        .await
        .unwrap();
    WalSink::arm_pause_next_file_sync();
    let first = {
        let gc = Arc::clone(&gc);
        tokio::spawn(async move { gc.sync_now().await })
    };
    WalSink::wait_file_sync_paused().await;
    gc.append(entry(2, 2).encode().unwrap(), 2, WalDurability::Buffered)
        .await
        .unwrap();
    WalSink::resume_file_sync();
    first.await.unwrap().unwrap();
    assert!(gc.is_dirty());
    assert_eq!(gc.recoverable_through(), 1, "capture excludes later bytes");
    gc.sync_now().await.unwrap();
    assert!(!gc.is_dirty());
    assert_eq!(gc.recoverable_through(), 2);
    assert_eq!(gc.fsync_count(), 2);
}

/// Snapshot capture holds the gate through read/fsync; history unlink waits.
#[tokio::test]
async fn snapshot_capture_serializes_history_truncate_and_retry_succeeds() {
    let dir = TempDir::new().unwrap();
    let sink = Arc::new(WalSink::File(
        SegmentSet::open(dir.path().to_path_buf(), 1).await.unwrap(),
    ));
    let gc = Arc::new(tracked_group(Arc::clone(&sink)));
    gc.append(entry(1, 1).encode().unwrap(), 1, WalDurability::Buffered)
        .await
        .unwrap();
    let WalSink::File(segments) = sink.as_ref() else {
        unreachable!()
    };
    segments
        .sync_test
        .pause_snapshot
        .store(true, Ordering::Release);
    let demand = {
        let gc = Arc::clone(&gc);
        tokio::spawn(async move { gc.sync_now().await })
    };
    segments.sync_test.snapshot_paused.notified().await;
    assert_eq!(gc.recoverable_through(), 0);
    let truncate = gc.truncate_below(1);
    tokio::pin!(truncate);
    std::future::poll_fn(|cx| {
        assert!(truncate.as_mut().poll(cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    assert!(
        dir.path().join("00000000.wal").exists(),
        "captured seal cannot be unlinked"
    );
    segments.sync_test.snapshot_resume.notify_one();
    assert_eq!(demand.await.unwrap().unwrap(), 1);
    assert_eq!(truncate.await.unwrap(), 1);
    assert!(!dir.path().join("00000000.wal").exists());
    assert_eq!(gc.sync_now().await.unwrap(), 1);
    assert_eq!(gc.recoverable_through(), 1);
}

/// Count actual active sync calls, including rotation, not worker rounds.
#[tokio::test]
async fn rotation_and_shared_sync_have_at_most_one_actual_in_flight_call() {
    let dir = TempDir::new().unwrap();
    let payload = entry(1, 1).encode().unwrap();
    let threshold = (payload.len() as u64 + 8) * 3;
    let sink = Arc::new(WalSink::File(
        SegmentSet::open(dir.path().to_path_buf(), threshold)
            .await
            .unwrap(),
    ));
    let gc = Arc::new(tracked_group(Arc::clone(&sink)));
    gc.append(payload.clone(), 1, WalDurability::Buffered)
        .await
        .unwrap();
    let WalSink::File(segments) = sink.as_ref() else {
        unreachable!()
    };
    segments.sync_test.pause_sync.store(true, Ordering::Release);
    let demand = {
        let gc = Arc::clone(&gc);
        tokio::spawn(async move { gc.sync_now().await })
    };
    segments.sync_test.sync_paused.notified().await;
    // Ordinary append completes while the shared worker holds the gate.
    gc.append(entry(2, 2).encode().unwrap(), 2, WalDurability::Buffered)
        .await
        .unwrap();
    let active = segments.active_segment_for_test();
    let crossing = gc.append(entry(3, 3).encode().unwrap(), 3, WalDurability::Buffered);
    tokio::pin!(crossing);
    std::future::poll_fn(|cx| {
        assert!(crossing.as_mut().poll(cx).is_pending());
        if active.approx_len_bytes() >= threshold {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    })
    .await;
    assert_eq!(segments.sync_test.in_flight.load(Ordering::Acquire), 1);
    segments.sync_test.sync_resume.notify_one();
    demand.await.unwrap().unwrap();
    crossing.await.unwrap();
    assert_eq!(segments.sync_test.maximum.load(Ordering::Acquire), 1);
    assert_eq!(segments.sync_test.calls.load(Ordering::Acquire), 2);
    assert_eq!(segments.sync_test.in_flight.load(Ordering::Acquire), 0);
    gc.sync_now().await.unwrap();
    assert_eq!(gc.recoverable_through(), 3);
    assert_eq!(segments.sync_test.maximum.load(Ordering::Acquire), 1);
}

/// Rotation must use the same sync-only gate as the detached worker.
#[tokio::test]
async fn rotation_waits_for_existing_sync_without_blocking_ordinary_append() {
    let dir = TempDir::new().unwrap();
    let segments = SegmentSet::open(dir.path().to_path_buf(), 1).await.unwrap();
    let gate = Arc::clone(&segments.sync_gate);
    let held = gate.lock().await;
    let active = segments.active_segment_for_test();
    let append = segments.append_batch(vec![entry(1, 1).encode().unwrap()], 1);
    tokio::pin!(append);
    // A write can finish while sync is excluded, but crossing the rotation
    // threshold cannot complete until its sync acquires this same gate.
    std::future::poll_fn(|cx| {
        assert!(append.as_mut().poll(cx).is_pending());
        if active.approx_len_bytes() > 0 {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    })
    .await;
    drop(held);
    append.await.unwrap();
    assert_eq!(segments.replay().await.unwrap()[0].commit_version, 1);
}

// A reopened seal has uncertain durability; neither read nor sync failure may
// be certified or lose its original I/O cause in the ticket.
#[tokio::test]
async fn rv_wal_5839ff0d_sealed_snapshot_ticket_preserves_io_cause() {
    for injection in [1, 2] {
        let dir = TempDir::new().unwrap();
        let old = crate::wal_segment::WalSegment::open(dir.path().join("00000000.wal"))
            .await
            .unwrap();
        old.append_batch(Arc::new(vec![entry(1, 1).encode().unwrap()]), 1)
            .await
            .unwrap();
        drop(old);
        drop(
            crate::wal_segment::WalSegment::open(dir.path().join("00000001.wal"))
                .await
                .unwrap(),
        );
        let segments = SegmentSet::open(dir.path().to_path_buf(), u64::MAX)
            .await
            .unwrap();
        segments
            .sync_test
            .sealed_error
            .store(injection, Ordering::Release);
        let sink = Arc::new(WalSink::File(segments));
        let gc = tracked_group(Arc::clone(&sink));
        let (ticket, completion) =
            gc.append_ticketed(entry(2, 2).encode().unwrap(), 2, WalDurability::Synced);
        assert_eq!(completion.await, Err(WalAppendError::WrittenNotDurable));
        let cause = ticket.failure_cause().unwrap();
        let shamir_storage::error::DbError::Io(io) = &*cause else {
            panic!("expected typed I/O cause: {cause}");
        };
        assert_eq!(io.kind(), std::io::ErrorKind::PermissionDenied);
        let original = io
            .get_ref()
            .unwrap()
            .source()
            .unwrap()
            .downcast_ref::<std::io::Error>()
            .unwrap();
        assert_eq!(original.kind(), std::io::ErrorKind::PermissionDenied);
        assert!(original
            .get_ref()
            .unwrap()
            .downcast_ref::<InjectedSealedError>()
            .is_some());
        assert_eq!(gc.recoverable_through(), 0);
        gc.sync_now().await.unwrap();
    }
}
