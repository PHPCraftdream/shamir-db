use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::ThreadId;
use std::time::Duration;

use shamir_connect::server::audit_chain::{
    AuditAppender, AuditChain, AuditChainWriter, AuditEntry,
};
use shamir_connect::server::lockout::Subnet;
use shamir_connect::server::rate_limit::RateDecision;
use shamir_connect::server::token::Endpoint;
use tokio::sync::{oneshot, Notify};
use tokio_util::sync::CancellationToken;

use super::{
    serialize_exposure, BucketCache, FailureLimiter, FailureSummary, RestMaintenance, FLUSH_LIMIT,
    NOTICE_CAPACITY,
};

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
        Arc::new(BucketCache::default()),
    )
}

struct FailingSerialize;
impl serde::Serialize for FailingSerialize {
    fn serialize<S>(&self, _: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        Err(serde::ser::Error::custom("injected serialization failure"))
    }
}

#[test]
fn exposure_serialization_failure_counts_once_per_endpoint_without_success_payload() {
    for channel in [Endpoint::Rest, Endpoint::Ws] {
        let mut summary = FailureSummary::new(channel);
        let failed = serialize_exposure(channel, [0xA5; 16], &mut summary.exposure_dropped, || {
            rmp_serde::to_vec_named(&FailingSerialize).map_err(|_| ())
        });
        assert!(failed.is_none());
        assert_eq!(summary.exposure_dropped, 1);
        let successful =
            serialize_exposure(channel, [0xA5; 16], &mut summary.exposure_dropped, || {
                rmp_serde::to_vec_named(&(super::channel_label(channel), [0xA5; 16], [0xB6; 16]))
                    .map_err(|_| ())
            });
        let successful = successful.expect("successful exposure serialization");
        let decoded: (String, [u8; 16], [u8; 16]) =
            rmp_serde::from_slice(&successful).expect("decode exposure payload");
        assert_eq!(
            decoded,
            (
                super::channel_label(channel).to_owned(),
                [0xA5; 16],
                [0xB6; 16]
            )
        );
        assert_eq!(summary.exposure_dropped, 1);
        let details: (
            String,
            String,
            u64,
            usize,
            [Option<Subnet>; 4],
            u64,
            u64,
            u64,
        ) = rmp_serde::from_slice(&summary.details().expect("summary details"))
            .expect("decode summary");
        assert_eq!(details.7, 1);
    }
}

#[test]
fn failure_limiter_throttles_full_capacity_eviction_scans() {
    let limiter = FailureLimiter::new();
    for index in 0..4096u32 {
        assert!(matches!(
            limiter.check(subnet(index), 1),
            RateDecision::Allowed
        ));
    }
    for index in 4096..5120u32 {
        assert!(matches!(
            limiter.check(subnet(index), 1),
            RateDecision::RateLimited { .. }
        ));
    }
    assert_eq!(limiter.eviction_scans(), 1);
    assert_eq!(limiter.tracked_subnets(), 4096);

    assert!(matches!(
        limiter.check(subnet(5120), 100_000_001),
        RateDecision::RateLimited { .. }
    ));
    assert_eq!(limiter.eviction_scans(), 2);
    assert_eq!(limiter.tracked_subnets(), 4096);
}

#[test]
fn failure_limiter_enforces_two_attempt_warmup_and_ten_after_anchor() {
    let limiter = FailureLimiter::with_warmup_anchor(Some(10));
    assert!(matches!(
        limiter.check(subnet(1), 10),
        RateDecision::Allowed
    ));
    assert!(matches!(
        limiter.check(subnet(1), 10),
        RateDecision::Allowed
    ));
    assert!(matches!(
        limiter.check(subnet(1), 10),
        RateDecision::RateLimited { .. }
    ));
    for _ in 0..10 {
        assert!(matches!(
            limiter.check(subnet(2), 60_000_000_010),
            RateDecision::Allowed
        ));
    }
    assert!(matches!(
        limiter.check(subnet(2), 60_000_000_010),
        RateDecision::RateLimited { .. }
    ));
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
    let limiter = FailureLimiter::with_warmup_anchor(Some(2));
    for _ in 0..2 {
        assert!(matches!(
            limiter.check(subnet(10), 2),
            RateDecision::Allowed
        ));
    }
    assert!(matches!(
        limiter.check(subnet(10), 2),
        RateDecision::RateLimited { .. }
    ));
    assert!(matches!(
        limiter.check(subnet(11), 60_000_000_002),
        RateDecision::Allowed
    ));
    for _ in 0..9 {
        assert!(matches!(
            limiter.check(subnet(11), 60_000_000_002),
            RateDecision::Allowed
        ));
    }
    assert!(matches!(
        limiter.check(subnet(11), 60_000_000_002),
        RateDecision::RateLimited { .. }
    ));
    assert_eq!(limiter.tracked_subnets(), 2);
    limiter.gc(360_000_000_003);
    assert_eq!(limiter.tracked_subnets(), 0);
}

#[test]
fn failure_summary_tracks_bounded_ipv4_ipv6_samples_and_untracked_overflow() {
    let mut summary = FailureSummary::new(Endpoint::Rest);
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
    let mut ipv6_summary = FailureSummary::new(Endpoint::Rest);
    ipv6_summary.record(&mut ipv6_tracked, v6, 51);
    assert!(ipv6_summary.samples.contains(&Some(v6)));
    assert!(summary.details().expect("summary details").len() < 256);
}

#[test]
fn failure_summary_tracks_failure_and_exposure_drops_separately() {
    let mut summary = FailureSummary::new(Endpoint::Rest);
    summary.dropped = 3;
    summary.exposure_dropped = 5;
    let details: (
        String,
        String,
        u64,
        usize,
        [Option<Subnet>; 4],
        u64,
        u64,
        u64,
    ) = rmp_serde::from_slice(&summary.details().expect("summary details"))
        .expect("decode summary");
    assert_eq!(details.5, 3);
    assert_eq!(details.7, 5);
}

#[tokio::test]
async fn exposure_queue_is_independent_and_survives_failure_flood() {
    let appender = Arc::new(Capture::new());
    let stop = CancellationToken::new();
    let worker = maintenance(appender.clone(), Arc::new(|| Some(1)), stop);
    for index in 0..NOTICE_CAPACITY + 64 {
        worker.record_failure(Endpoint::Rest, subnet(index as u32), 1);
    }
    worker.record_exposed(Endpoint::Rest, [0x12; 16], [0x34; 16], subnet(4), 1);
    worker.shutdown().await;
    assert!(appender
        .entries
        .lock()
        .expect("entries")
        .iter()
        .any(|entry| entry.event == "api_token_exposed"));
}

#[tokio::test]
async fn failures_produce_one_bounded_summary_and_account_overflow_and_drops() {
    let appender = Arc::new(Capture::new());
    let worker = maintenance(
        appender.clone(),
        Arc::new(|| Some(10)),
        CancellationToken::new(),
    );
    for index in 0..NOTICE_CAPACITY {
        worker.record_failure(Endpoint::Rest, subnet(index as u32), 10);
    }
    let permits = worker
        .sender
        .reserve_many(NOTICE_CAPACITY)
        .await
        .expect("failure channel remains open");
    drop(permits);
    for index in NOTICE_CAPACITY..NOTICE_CAPACITY + 64 {
        worker.record_failure(Endpoint::Rest, subnet(index as u32), 10);
    }
    let permits = worker
        .sender
        .reserve_many(NOTICE_CAPACITY)
        .await
        .expect("failure channel drained");
    drop(permits);
    appender.appended.notified().await;
    worker.record_failure(Endpoint::Rest, subnet(9000), 20);
    worker.shutdown().await;
    let entries = appender.entries.lock().expect("entries").clone();
    let summaries: Vec<_> = entries
        .iter()
        .filter(|entry| entry.event == "api_token_auth_failures")
        .collect();
    assert_eq!(summaries.len(), 2);
    let first: (
        String,
        String,
        u64,
        usize,
        [Option<Subnet>; 4],
        u64,
        u64,
        u64,
    ) = rmp_serde::from_slice(&summaries[0].details_canonical_msgpack).expect("first summary");
    assert_eq!((first.2, first.3, first.5, first.6), (320, 256, 0, 64));
    let second: (
        String,
        String,
        u64,
        usize,
        [Option<Subnet>; 4],
        u64,
        u64,
        u64,
    ) = rmp_serde::from_slice(&summaries[1].details_canonical_msgpack).expect("second summary");
    assert_eq!((second.2, second.3, second.5, second.6), (1, 1, 0, 0));
}

#[tokio::test]
async fn exposure_notice_channel_full_counts_drops_in_separate_summary_field() {
    let appender = Arc::new(Capture::new());
    let worker = maintenance(
        appender.clone(),
        Arc::new(|| Some(1)),
        CancellationToken::new(),
    );
    for id in 0..NOTICE_CAPACITY + 1 {
        let mut token_id = [0; 16];
        token_id[14..].copy_from_slice(&(id as u16).to_be_bytes());
        worker.record_exposed(Endpoint::Rest, token_id, [0x34; 16], subnet(4), 1);
    }
    worker.record_failure(Endpoint::Rest, subnet(7), 1);
    worker.shutdown().await;
    let entries = appender.entries.lock().expect("entries").clone();
    let summary = entries
        .iter()
        .find(|entry| entry.event == "api_token_auth_failures")
        .expect("summary");
    let details: (
        String,
        String,
        u64,
        usize,
        [Option<Subnet>; 4],
        u64,
        u64,
        u64,
    ) = rmp_serde::from_slice(&summary.details_canonical_msgpack).expect("summary details");
    assert_eq!(details.5, 0);
    assert_eq!(details.7, 1);
}

#[derive(Clone, Default)]
struct LogCapture(Arc<Mutex<Vec<u8>>>);
impl std::io::Write for LogCapture {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().expect("log capture").extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogCapture {
    type Writer = Self;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

#[tokio::test]
async fn exposure_drops_log_token_ids_when_channel_is_full_closed_or_disabled() {
    let capture = LogCapture::default();
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .without_time()
        .with_writer(capture.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    let worker = maintenance(
        Arc::new(Capture::new()),
        Arc::new(|| Some(1)),
        CancellationToken::new(),
    );
    for index in 0..NOTICE_CAPACITY + 1 {
        let mut id = [0; 16];
        id[14..].copy_from_slice(&(index as u16).to_be_bytes());
        worker.record_exposed(Endpoint::Rest, id, [3; 16], subnet(1), 1);
    }
    worker.shutdown().await;
    worker.record_exposed(Endpoint::Rest, [0xA1; 16], [3; 16], subnet(1), 1);
    worker.disabled.store(true, Ordering::Release);
    worker.record_exposed(Endpoint::Rest, [0xB2; 16], [3; 16], subnet(1), 1);
    let logs =
        String::from_utf8(capture.0.lock().expect("log capture").clone()).expect("UTF-8 logs");
    assert!(logs.contains("token exposure audit channel full or closed"));
    assert!(logs.contains("token exposure audit unavailable"));
    assert!(logs.contains("token_id=[161,"));
    assert!(logs.contains("token_id=[178,"));
}

#[tokio::test]
async fn stop_drains_exposures_in_bounded_batches_and_ignores_late_drops() {
    let inner = Arc::new(Capture::new());
    let appender = Arc::new(LateDropsCapture {
        inner: Arc::clone(&inner),
        failure_drops: Mutex::new(None),
    });
    let worker = maintenance(
        appender.clone(),
        Arc::new(|| Some(1)),
        CancellationToken::new(),
    );
    *appender.failure_drops.lock().expect("failure drop counter") =
        Some(Arc::clone(&worker.failure_drops));
    for index in 0..(FLUSH_LIMIT + 1) {
        let mut id = [0; 16];
        id[14..].copy_from_slice(&(index as u16).to_be_bytes());
        worker.record_exposed(Endpoint::Rest, id, [2; 16], subnet(1), 1);
    }
    worker.record_failure(Endpoint::Rest, subnet(1), 1);
    worker.shutdown().await;
    let entries = inner.entries.lock().expect("entries").clone();
    assert_eq!(
        entries
            .iter()
            .filter(|entry| entry.event == "api_token_exposed")
            .count(),
        FLUSH_LIMIT + 1
    );
    assert_eq!(
        entries
            .iter()
            .filter(|entry| entry.event == "api_token_auth_failures")
            .count(),
        1
    );
    assert!(worker.failure_drops.load(Ordering::Acquire) > 0);
}

#[tokio::test(start_paused = true)]
async fn disabled_writer_terminates_shutdown_without_waiting_for_deadline() {
    let worker = maintenance(
        Arc::new(Capture::new()),
        Arc::new(|| Some(1)),
        CancellationToken::new(),
    );
    worker.record_failure(Endpoint::Rest, subnet(1), 1);
    worker.record_exposed(Endpoint::Rest, [0x12; 16], [2; 16], subnet(1), 1);
    worker.disabled.store(true, Ordering::Release);
    let before_shutdown = tokio::time::Instant::now();
    worker.shutdown().await;
    assert_eq!(tokio::time::Instant::now(), before_shutdown);
}

#[tokio::test(start_paused = true)]
async fn stalled_writer_deadline_warns_before_detaching() {
    let (appender, entered, release) = BlockingCapture::new();
    let capture = LogCapture::default();
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .without_time()
        .with_writer(capture.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    let worker = maintenance(
        Arc::new(appender),
        Arc::new(|| Some(1)),
        CancellationToken::new(),
    );
    // Let the flusher arm its interval before the clock moves.
    tokio::task::yield_now().await;
    worker.record_exposed(Endpoint::Rest, [0x12; 16], [2; 16], subnet(1), 1);
    let (entered_tx, entered_rx) = oneshot::channel();
    let waiter = tokio::task::spawn_blocking(move || {
        entered.recv().expect("write started");
        let _ = entered_tx.send(());
    });
    tokio::time::advance(Duration::from_secs(1)).await;
    entered_rx.await.expect("writer entered");
    let shutdown = tokio::spawn(async move { worker.shutdown().await });
    // A running blocking task inhibits auto-advance: step the clock explicitly.
    while !shutdown.is_finished() {
        tokio::time::advance(Duration::from_millis(500)).await;
    }
    shutdown.await.expect("shutdown task");
    release.send(()).expect("release detached writer");
    waiter.await.expect("writer waiter");
    let logs =
        String::from_utf8(capture.0.lock().expect("log capture").clone()).expect("UTF-8 logs");
    assert!(logs.contains("summary_total"));
    assert!(logs.contains("pending_exposures"));
    assert!(logs.contains("failure_dropped"));
}

#[tokio::test]
async fn receiver_keeps_draining_while_blocking_audit_write_runs() {
    let (appender, entered_rx, release_tx) = BlockingCapture::new();
    let worker = Arc::new(maintenance(
        Arc::new(appender),
        Arc::new(|| Some(1)),
        CancellationToken::new(),
    ));
    worker.record_exposed(Endpoint::Rest, [1; 16], [2; 16], subnet(4), 1);
    let (started_tx, started_rx) = oneshot::channel();
    let waiter = tokio::task::spawn_blocking(move || {
        entered_rx.recv().expect("append started");
        let _ = started_tx.send(());
    });
    started_rx.await.expect("appender entered");
    for index in 0..NOTICE_CAPACITY {
        worker.record_failure(Endpoint::Rest, subnet(index as u32), 2);
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

struct FailingCapture {
    failed: Arc<AtomicBool>,
}
impl AuditAppender for FailingCapture {
    fn append_entry(&self, _: &AuditEntry) -> Result<(), std::io::Error> {
        self.failed.store(true, Ordering::Release);
        Err(std::io::Error::new(
            std::io::ErrorKind::BrokenPipe,
            "test failure",
        ))
    }
    fn checkpoint(&self, _: u64, _: &[u8; 32]) -> Result<(), std::io::Error> {
        Ok(())
    }
}

#[tokio::test]
async fn failing_writer_disables_producers_and_joins_shutdown() {
    let failed = Arc::new(AtomicBool::new(false));
    let worker = maintenance(
        Arc::new(FailingCapture {
            failed: Arc::clone(&failed),
        }),
        Arc::new(|| Some(1)),
        CancellationToken::new(),
    );
    worker.record_failure(Endpoint::Rest, subnet(9), 1);
    worker.shutdown().await;
    assert!(failed.load(Ordering::Acquire));
    assert!(worker.disabled.load(Ordering::Acquire));
    assert!(worker.task.lock().await.is_none());
}

struct LateDropsCapture {
    inner: Arc<Capture>,
    failure_drops: Mutex<Option<Arc<std::sync::atomic::AtomicU64>>>,
}
impl AuditAppender for LateDropsCapture {
    fn append_entry(&self, entry: &AuditEntry) -> Result<(), std::io::Error> {
        self.inner.append_entry(entry)?;
        if let Some(drops) = self
            .failure_drops
            .lock()
            .expect("failure drop counter")
            .as_ref()
        {
            drops.fetch_add(1, Ordering::Relaxed);
        }
        Ok(())
    }
    fn checkpoint(&self, _: u64, _: &[u8; 32]) -> Result<(), std::io::Error> {
        Ok(())
    }
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

#[tokio::test]
async fn failures_aggregate_once_and_append_off_runtime_thread() {
    let appender = Arc::new(Capture::new());
    let append_thread = Arc::new(Mutex::new(None));
    let observing = Arc::new(ThreadCapture {
        inner: appender.clone(),
        thread: Arc::clone(&append_thread),
    });
    let worker = maintenance(observing, Arc::new(|| Some(10)), CancellationToken::new());
    for _ in 0..5 {
        worker.record_failure(Endpoint::Rest, subnet(7), 10);
    }
    worker.shutdown().await;
    let entries = appender.entries.lock().expect("entries").clone();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].event, "api_token_auth_failures");
    let details: (
        String,
        String,
        u64,
        usize,
        [Option<Subnet>; 4],
        u64,
        u64,
        u64,
    ) = rmp_serde::from_slice(&entries[0].details_canonical_msgpack).expect("summary");
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

#[tokio::test]
async fn service_stop_drains_queued_exposure_and_failure_notices() {
    let appender = Arc::new(Capture::new());
    let service_stop = CancellationToken::new();
    let worker = maintenance(appender.clone(), Arc::new(|| Some(3)), service_stop.clone());
    worker.record_failure(Endpoint::Rest, subnet(1), 3);
    worker.record_exposed(Endpoint::Rest, [7; 16], [8; 16], subnet(2), 3);
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

#[tokio::test]
async fn ws_exposure_audit_summary_carries_ws_channel() {
    let appender = Arc::new(Capture::new());
    let worker = maintenance(
        appender.clone(),
        Arc::new(|| Some(1)),
        CancellationToken::new(),
    );
    let id = [0x12; 16];
    let owner = [0x34; 16];
    worker.record_exposed(Endpoint::Ws, id, owner, subnet(4), 1);
    worker.shutdown().await;
    let entries = appender.entries.lock().expect("entries").clone();
    let entry = entries
        .iter()
        .find(|entry| entry.event == "api_token_exposed")
        .expect("WS exposure");
    let details: (String, [u8; 16], [u8; 16]) =
        rmp_serde::from_slice(&entry.details_canonical_msgpack).expect("WS exposure details");
    assert_eq!(details, ("ws".into(), id, owner));
}

#[tokio::test]
async fn exposure_drops_are_reported_in_the_matching_channel_summary() {
    let appender = Arc::new(Capture::new());
    let worker = maintenance(
        appender.clone(),
        Arc::new(|| Some(1)),
        CancellationToken::new(),
    );
    let permits = worker
        .exposure_sender
        .reserve_many(NOTICE_CAPACITY)
        .await
        .expect("exposure channel remains open");
    drop(permits);
    for index in 0..NOTICE_CAPACITY {
        let mut id = [0; 16];
        id[14..].copy_from_slice(&(index as u16).to_be_bytes());
        worker.record_exposed(Endpoint::Rest, id, [0x34; 16], subnet(4), 1);
    }
    worker.record_exposed(Endpoint::Ws, [0x56; 16], [0x78; 16], subnet(5), 1);
    worker.record_failure(Endpoint::Rest, subnet(7), 1);
    worker.record_failure(Endpoint::Ws, subnet(8), 1);
    worker.shutdown().await;

    let entries = appender.entries.lock().expect("entries").clone();
    let summaries: Vec<_> = entries
        .iter()
        .filter(|entry| entry.event == "api_token_auth_failures")
        .collect();
    assert_eq!(summaries.len(), 2);
    let rest: (
        String,
        String,
        u64,
        usize,
        [Option<Subnet>; 4],
        u64,
        u64,
        u64,
    ) = rmp_serde::from_slice(&summaries[0].details_canonical_msgpack).expect("REST summary");
    let ws: (
        String,
        String,
        u64,
        usize,
        [Option<Subnet>; 4],
        u64,
        u64,
        u64,
    ) = rmp_serde::from_slice(&summaries[1].details_canonical_msgpack).expect("WS summary");
    assert_eq!((rest.0.as_str(), rest.7), ("rest", 0));
    assert_eq!((ws.0.as_str(), ws.7), ("ws", 1));
}

#[tokio::test]
async fn ws_failure_audit_summary_carries_ws_channel() {
    let appender = Arc::new(Capture::new());
    let worker = maintenance(
        appender.clone(),
        Arc::new(|| Some(1)),
        CancellationToken::new(),
    );
    worker.record_failure(Endpoint::Ws, subnet(4), 1);
    worker.shutdown().await;
    let entries = appender.entries.lock().expect("entries").clone();
    let failures: Vec<_> = entries
        .iter()
        .filter(|entry| entry.event == "api_token_auth_failures")
        .collect();
    assert_eq!(failures.len(), 1);
    let details: (
        String,
        String,
        u64,
        usize,
        [Option<Subnet>; 4],
        u64,
        u64,
        u64,
    ) = rmp_serde::from_slice(&failures[0].details_canonical_msgpack).expect("WS failure summary");
    assert_eq!(details.0, "ws");
    assert_eq!(details.1, "invalid");
    assert_eq!((details.2, details.3), (1, 1));
    assert!(details.4.contains(&Some(subnet(4))));
}

#[tokio::test]
async fn exposed_event_contains_canonical_ids_but_no_peer_secret() {
    let appender = Arc::new(Capture::new());
    let worker = maintenance(
        appender.clone(),
        Arc::new(|| Some(1)),
        CancellationToken::new(),
    );
    let id = [0x12; 16];
    let owner = [0x34; 16];
    worker.record_exposed(Endpoint::Rest, id, owner, subnet(4), 1);
    worker.shutdown().await;
    let entries = appender.entries.lock().expect("entries").clone();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].event, "api_token_exposed");
    assert_eq!(entries[0].user, "");
    let details: (String, [u8; 16], [u8; 16]) =
        rmp_serde::from_slice(&entries[0].details_canonical_msgpack).expect("details");
    assert_eq!(details, ("rest".into(), id, owner));
}
