use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::ThreadId;
use std::time::Duration;

use shamir_connect::server::audit_chain::{
    AuditAppender, AuditChain, AuditChainWriter, AuditEntry,
};
use shamir_connect::server::lockout::Subnet;
use shamir_connect::server::rate_limit::RateDecision;
use tokio::sync::{oneshot, Notify};
use tokio_util::sync::CancellationToken;

use super::{FailureLimiter, RestMaintenance, NOTICE_CAPACITY};

struct Capture {
    entries: Mutex<Vec<AuditEntry>>,
    appended: Notify,
}
impl Capture {
    fn new() -> Self {
        Self {
            entries: Mutex::new(Vec::new()),
            appended: Notify::new(),
        }
    }
}
struct ThreadCapture {
    inner: Arc<dyn AuditAppender>,
    thread: Arc<Mutex<Option<ThreadId>>>,
}
impl AuditAppender for ThreadCapture {
    fn append_entry(&self, entry: &AuditEntry) -> Result<(), std::io::Error> {
        *self.thread.lock().expect("thread") = Some(std::thread::current().id());
        self.inner.append_entry(entry)
    }
    fn checkpoint(&self, sequence: u64, hmac: &[u8; 32]) -> Result<(), std::io::Error> {
        self.inner.checkpoint(sequence, hmac)
    }
}
impl AuditAppender for Capture {
    fn append_entry(&self, entry: &AuditEntry) -> Result<(), std::io::Error> {
        self.entries.lock().expect("entries").push(entry.clone());
        self.appended.notify_one();
        Ok(())
    }
    fn checkpoint(&self, _: u64, _: &[u8; 32]) -> Result<(), std::io::Error> {
        Ok(())
    }
}
fn subnet(index: u32) -> Subnet {
    let bytes = index.to_be_bytes();
    Subnet::V4([bytes[1], bytes[2], bytes[3]])
}
fn maintenance(
    appender: Arc<dyn AuditAppender>,
    clock: Arc<dyn Fn() -> Option<u64> + Send + Sync>,
    stop: CancellationToken,
) -> RestMaintenance {
    RestMaintenance::new(
        Arc::new(AuditChainWriter::new(AuditChain::new([9; 32]), appender)),
        clock,
        stop,
    )
}

#[test]
fn failure_limiter_hard_caps_entries_and_preserves_existing_subnets() {
    let limiter = Arc::new(FailureLimiter::new());
    let threads = 8;
    let per_thread = 4096 / threads;
    let mut workers = Vec::new();
    for worker in 0..threads {
        let limiter = Arc::clone(&limiter);
        workers.push(std::thread::spawn(move || {
            for offset in 0..per_thread {
                assert!(matches!(
                    limiter.check(subnet((worker * per_thread + offset) as u32), 1),
                    RateDecision::Allowed
                ));
            }
        }));
    }
    for worker in workers {
        worker.join().expect("limiter worker");
    }
    assert_eq!(limiter.tracked_subnets(), 4096);
    assert!(matches!(
        limiter.check(subnet(4096), 1),
        RateDecision::RateLimited { .. }
    ));
    assert_eq!(limiter.tracked_subnets(), 4096);
    assert!(matches!(limiter.check(subnet(0), 2), RateDecision::Allowed));
}

#[test]
fn failure_limiter_gc_removes_idle_subnets_and_new_subnet_uses_post_warmup_rate() {
    let limiter = FailureLimiter::new();
    for index in 0..4096u32 {
        assert!(matches!(
            limiter.check(subnet(index), 1),
            RateDecision::Allowed
        ));
    }
    assert!(matches!(
        limiter.check(subnet(4096), 60_000_000_002),
        RateDecision::Allowed
    ));
    assert_eq!(limiter.tracked_subnets(), 4096);
    limiter.gc(360_000_000_003);
    assert_eq!(limiter.tracked_subnets(), 0);
}

#[test]
fn failure_summary_tracks_bounded_ipv4_ipv6_samples_and_untracked_overflow() {
    let mut summary = super::FailureSummary::new();
    let mut tracked = shamir_collections::TMap::default();
    for index in 0..300u32 {
        let bytes = index.to_be_bytes();
        summary.record(&mut tracked, Subnet::V4([bytes[1], bytes[2], bytes[3]]), 50);
    }
    assert_eq!(summary.total, 300);
    assert_eq!(summary.distinct_tracked, 256);
    assert_eq!(summary.untracked, 44);
    assert_eq!(summary.sample_count, 4);
    let v6 = Subnet::V6([1, 2, 3, 4, 5, 6, 7, 8]);
    let mut ipv6_tracked = shamir_collections::TMap::default();
    let mut ipv6_summary = super::FailureSummary::new();
    ipv6_summary.record(&mut ipv6_tracked, v6, 51);
    assert!(ipv6_summary.samples.contains(&Some(v6)));
    assert!(summary.details().expect("summary details").len() < 256);
}

#[tokio::test(start_paused = true)]
async fn exposure_queue_is_independent_and_survives_failure_flood() {
    let appender = Arc::new(Capture::new());
    let stop = CancellationToken::new();
    let worker = maintenance(appender.clone(), Arc::new(|| Some(1)), stop);
    for index in 0..NOTICE_CAPACITY + 64 {
        worker.record_failure(subnet(index as u32), 1);
    }
    worker.record_exposed([0x12; 16], [0x34; 16], subnet(4), 1);
    tokio::time::advance(Duration::from_secs(1)).await;
    worker.shutdown().await;
    assert!(appender
        .entries
        .lock()
        .expect("entries")
        .iter()
        .any(|entry| entry.event == "api_token_exposed"));
}

#[tokio::test(start_paused = true)]
async fn failures_produce_one_bounded_summary_and_account_overflow_and_drops() {
    let appender = Arc::new(Capture::new());
    let worker = maintenance(
        appender.clone(),
        Arc::new(|| Some(10)),
        CancellationToken::new(),
    );
    for index in 0..NOTICE_CAPACITY + 64 {
        worker.record_failure(subnet(index as u32), 10);
    }
    tokio::time::advance(Duration::from_secs(1)).await;
    worker.shutdown().await;
    let entries = appender.entries.lock().expect("entries").clone();
    let summaries: Vec<_> = entries
        .iter()
        .filter(|entry| entry.event == "api_token_auth_failures")
        .collect();
    assert_eq!(summaries.len(), 1);
    let details: (String, String, u64, usize, [Option<Subnet>; 4], u64, u64) =
        rmp_serde::from_slice(&summaries[0].details_canonical_msgpack).expect("summary");
    assert_eq!(details.0, "rest");
    assert_eq!(details.1, "invalid");
    assert_eq!(details.2, 256);
    assert_eq!(details.3, 256);
    assert_eq!(details.5, 64);
    assert_eq!(details.6, 0);
    assert!(summaries[0].details_canonical_msgpack.len() < 256);
}

#[tokio::test]
async fn receiver_keeps_draining_while_blocking_audit_write_runs() {
    let (appender, entered_rx, release_tx) = BlockingCapture::new();
    let worker = Arc::new(maintenance(
        Arc::new(appender),
        Arc::new(|| Some(1)),
        CancellationToken::new(),
    ));
    worker.record_exposed([1; 16], [2; 16], subnet(4), 1);
    let (started_tx, started_rx) = oneshot::channel();
    let waiter = tokio::task::spawn_blocking(move || {
        entered_rx.recv().expect("append started");
        let _ = started_tx.send(());
    });
    started_rx.await.expect("appender entered");
    for index in 0..NOTICE_CAPACITY {
        worker.record_failure(subnet(index as u32), 2);
    }
    assert_eq!(worker.sender.capacity(), 0);
    let (drained_tx, drained_rx) = oneshot::channel();
    let sender = worker.sender.clone();
    let drain_waiter = tokio::spawn(async move {
        let permit = sender.reserve().await.expect("channel remains open");
        let _ = drained_tx.send(());
        drop(permit);
    });
    drained_rx.await.expect("receiver freed a channel slot");
    release_tx.send(()).expect("release appender");
    waiter.await.expect("append waiter");
    drain_waiter.await.expect("drain waiter");
    worker.shutdown().await;
}

struct BlockingCapture {
    entered: std::sync::mpsc::SyncSender<()>,
    release: Mutex<std::sync::mpsc::Receiver<()>>,
    blocked_once: AtomicBool,
}
impl BlockingCapture {
    fn new() -> (
        Self,
        std::sync::mpsc::Receiver<()>,
        std::sync::mpsc::SyncSender<()>,
    ) {
        let (entered, entered_rx) = std::sync::mpsc::sync_channel(1);
        let (release_tx, release) = std::sync::mpsc::sync_channel(1);
        (
            Self {
                entered,
                release: Mutex::new(release),
                blocked_once: AtomicBool::new(false),
            },
            entered_rx,
            release_tx,
        )
    }
}
impl AuditAppender for BlockingCapture {
    fn append_entry(&self, _: &AuditEntry) -> Result<(), std::io::Error> {
        if !self.blocked_once.swap(true, Ordering::AcqRel) {
            let _ = self.entered.send(());
            self.release
                .lock()
                .expect("release lock")
                .recv()
                .map_err(|_| std::io::Error::new(std::io::ErrorKind::BrokenPipe, "released"))?;
        }
        Ok(())
    }
    fn checkpoint(&self, _: u64, _: &[u8; 32]) -> Result<(), std::io::Error> {
        Ok(())
    }
}

#[tokio::test(start_paused = true)]
async fn failures_aggregate_once_and_append_off_runtime_thread() {
    let appender = Arc::new(Capture::new());
    let append_thread = Arc::new(Mutex::new(None));
    let observing = Arc::new(ThreadCapture {
        inner: appender.clone(),
        thread: Arc::clone(&append_thread),
    });
    let worker = maintenance(observing, Arc::new(|| Some(10)), CancellationToken::new());
    for _ in 0..5 {
        worker.record_failure(subnet(7), 10);
    }
    tokio::time::advance(Duration::from_secs(1)).await;
    worker.shutdown().await;
    let entries = appender.entries.lock().expect("entries").clone();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].event, "api_token_auth_failures");
    let details: (String, String, u64, usize, [Option<Subnet>; 4], u64, u64) =
        rmp_serde::from_slice(&entries[0].details_canonical_msgpack).expect("summary");
    assert_eq!(
        (details.0.as_str(), details.1.as_str(), details.2),
        ("rest", "invalid", 5)
    );
    assert_ne!(
        *append_thread.lock().expect("thread"),
        Some(std::thread::current().id())
    );
}

#[tokio::test]
async fn service_stop_cancels_and_joins_the_maintenance_worker() {
    let service_stop = CancellationToken::new();
    let worker = maintenance(
        Arc::new(Capture::new()),
        Arc::new(|| Some(1)),
        service_stop.clone(),
    );
    service_stop.cancel();
    let task = worker.task.lock().await.take();
    assert!(task.is_some());
    task.expect("worker").await.expect("joined");
    worker.shutdown().await;
    assert!(worker.task.lock().await.is_none());
}

#[tokio::test(start_paused = true)]
async fn service_stop_drains_queued_exposure_and_failure_notices() {
    let appender = Arc::new(Capture::new());
    let service_stop = CancellationToken::new();
    let worker = maintenance(appender.clone(), Arc::new(|| Some(3)), service_stop.clone());
    worker.record_failure(subnet(1), 3);
    worker.record_exposed([7; 16], [8; 16], subnet(2), 3);
    service_stop.cancel();
    worker.shutdown().await;
    let entries = appender.entries.lock().expect("entries");
    assert!(entries
        .iter()
        .any(|entry| entry.event == "api_token_auth_failures"));
    assert!(entries
        .iter()
        .any(|entry| entry.event == "api_token_exposed"));
}

#[tokio::test(start_paused = true)]
async fn exposed_event_contains_canonical_ids_but_no_peer_secret() {
    let appender = Arc::new(Capture::new());
    let worker = maintenance(
        appender.clone(),
        Arc::new(|| Some(1)),
        CancellationToken::new(),
    );
    let id = [0x12; 16];
    let owner = [0x34; 16];
    worker.record_exposed(id, owner, subnet(4), 1);
    tokio::time::advance(Duration::from_secs(1)).await;
    worker.shutdown().await;
    let entries = appender.entries.lock().expect("entries").clone();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].event, "api_token_exposed");
    assert_eq!(entries[0].user, "");
    let details: (String, [u8; 16], [u8; 16]) =
        rmp_serde::from_slice(&entries[0].details_canonical_msgpack).expect("details");
    assert_eq!(details, ("rest".into(), id, owner));
}
