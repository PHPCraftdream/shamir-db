use std::sync::{Arc, Mutex};
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
    service_stop: CancellationToken,
) -> RestMaintenance {
    RestMaintenance::new(
        Arc::new(AuditChainWriter::new(AuditChain::new([9; 32]), appender)),
        clock,
        service_stop,
    )
}

#[test]
fn failure_limiter_hard_caps_entries_and_preserves_existing_subnets() {
    let limiter = Arc::new(FailureLimiter::new(0));
    let threads = 8;
    let per_thread = 4096 / threads;
    let mut workers = Vec::new();
    for worker in 0..threads {
        let limiter = Arc::clone(&limiter);
        workers.push(std::thread::spawn(move || {
            for offset in 0..per_thread {
                let index = worker * per_thread + offset;
                assert!(matches!(
                    limiter.check(subnet(index as u32), 1),
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
    assert!(matches!(limiter.check(subnet(0), 2), RateDecision::Allowed));
}

#[test]
fn failure_limiter_gc_removes_idle_subnets_and_updates_count() {
    let limiter = FailureLimiter::new(1);
    assert!(matches!(limiter.check(subnet(1), 1), RateDecision::Allowed));
    assert!(matches!(limiter.check(subnet(2), 1), RateDecision::Allowed));
    limiter.gc(300_000_000_002);
    assert_eq!(limiter.tracked_subnets(), 0);
}

#[tokio::test(start_paused = true)]
async fn notice_queue_is_bounded_when_worker_is_not_scheduled() {
    let appender = Arc::new(Capture::new());
    let stop = CancellationToken::new();
    let worker = maintenance(appender, Arc::new(|| Some(1)), stop.clone());
    for index in 0..NOTICE_CAPACITY + 64 {
        worker.record_failure(subnet(index as u32), 1);
    }
    assert_eq!(worker.sender.capacity(), 0);
    assert_eq!(worker.sender.max_capacity(), NOTICE_CAPACITY);
    stop.cancel();
    worker.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn failure_aggregate_gets_a_slot_amid_exposed_events() {
    let appender = Arc::new(Capture::new());
    let stop = CancellationToken::new();
    let worker = maintenance(appender.clone(), Arc::new(|| Some(1)), stop.clone());
    worker.record_failure(subnet(9), 1);
    for index in 0..NOTICE_CAPACITY - 1 {
        worker.record_exposed([index as u8; 16], [2; 16], subnet(4), 1);
    }
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(1)).await;
    loop {
        let notified = appender.appended.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if appender
            .entries
            .lock()
            .expect("entries")
            .iter()
            .any(|entry| entry.event == "api_token_auth_failed")
        {
            break;
        }
        notified.await;
    }
    let entries = appender.entries.lock().expect("entries");
    assert!(entries
        .iter()
        .any(|entry| entry.event == "api_token_auth_failed"));
    assert!(entries.len() <= 16);
    drop(entries);
    stop.cancel();
    worker.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn failures_aggregate_once_per_subnet_and_append_off_runtime_thread() {
    let appender = Arc::new(Capture::new());
    let writer: Arc<dyn AuditAppender> = appender.clone();
    let runtime_thread = std::thread::current().id();
    let append_thread = Arc::new(Mutex::new(None));
    let seen = Arc::clone(&append_thread);
    let observing = Arc::new(ThreadCapture {
        inner: writer,
        seen,
    });
    let stop = CancellationToken::new();
    let worker = maintenance(observing, Arc::new(|| Some(10)), stop.clone());
    for _ in 0..5 {
        worker.record_failure(subnet(7), 10);
    }
    let appended = appender.appended.notified();
    tokio::pin!(appended);
    appended.as_mut().enable();
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(1)).await;
    appended.await;
    let entries = appender.entries.lock().expect("entries").clone();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].event, "api_token_auth_failed");
    let details: (String, String, u64) =
        rmp_serde::from_slice(&entries[0].details_canonical_msgpack)
            .expect("canonical summary details");
    assert_eq!(details, ("rest".into(), "invalid".into(), 5));
    assert_ne!(
        *append_thread.lock().expect("append thread"),
        Some(runtime_thread)
    );
    drop(entries);
    stop.cancel();
    worker.shutdown().await;
}

struct ThreadCapture {
    inner: Arc<dyn AuditAppender>,
    seen: Arc<Mutex<Option<std::thread::ThreadId>>>,
}

impl AuditAppender for ThreadCapture {
    fn append_entry(&self, entry: &AuditEntry) -> Result<(), std::io::Error> {
        *self.seen.lock().expect("append thread") = Some(std::thread::current().id());
        self.inner.append_entry(entry)
    }

    fn checkpoint(&self, seq: u64, hmac: &[u8; 32]) -> Result<(), std::io::Error> {
        self.inner.checkpoint(seq, hmac)
    }
}

#[tokio::test]
async fn service_stop_cancels_and_joins_the_maintenance_worker() {
    let appender = Arc::new(Capture::new());
    let service_stop = CancellationToken::new();
    let worker = maintenance(appender, Arc::new(|| Some(1)), service_stop.clone());
    service_stop.cancel();
    let task = worker.task.lock().await.take();
    assert!(task.is_some());
    task.expect("worker task").await.expect("worker joined");
    worker.shutdown().await;
    assert!(worker.task.lock().await.is_none());
}

#[tokio::test]
async fn audit_append_blocking_does_not_block_request_runtime() {
    let (entered_tx, entered_rx) = std::sync::mpsc::sync_channel(1);
    let (release_tx, release_rx) = std::sync::mpsc::sync_channel(1);
    let appender = Arc::new(BlockingCapture {
        entered: entered_tx,
        release: Mutex::new(release_rx),
    });
    let stop = CancellationToken::new();
    let worker = Arc::new(maintenance(appender, Arc::new(|| Some(1)), stop.clone()));
    worker.record_exposed([1; 16], [2; 16], subnet(4), 1);
    let (started_tx, started_rx) = oneshot::channel();
    let waiter = tokio::task::spawn_blocking(move || {
        entered_rx.recv().expect("append started");
        let _ = started_tx.send(());
    });
    started_rx.await.expect("appender entered");
    let (returned_tx, returned_rx) = oneshot::channel();
    let request_worker = Arc::clone(&worker);
    let request = tokio::spawn(async move {
        request_worker.record_failure(subnet(1), 1);
        let _ = returned_tx.send(());
    });
    returned_rx.await.expect("request path returned");
    release_tx.send(()).expect("release appender");
    waiter.await.expect("append waiter");
    request.await.expect("request task");
    stop.cancel();
    worker.shutdown().await;
}

struct BlockingCapture {
    entered: std::sync::mpsc::SyncSender<()>,
    release: Mutex<std::sync::mpsc::Receiver<()>>,
}

impl AuditAppender for BlockingCapture {
    fn append_entry(&self, _: &AuditEntry) -> Result<(), std::io::Error> {
        let _ = self.entered.send(());
        self.release
            .lock()
            .expect("release lock")
            .recv()
            .map_err(|_| {
                std::io::Error::new(std::io::ErrorKind::BrokenPipe, "test released appender")
            })
    }

    fn checkpoint(&self, _: u64, _: &[u8; 32]) -> Result<(), std::io::Error> {
        Ok(())
    }
}

#[tokio::test(start_paused = true)]
async fn exposed_event_contains_canonical_ids_but_no_peer_secret() {
    let appender = Arc::new(Capture::new());
    let stop = CancellationToken::new();
    let worker = maintenance(appender.clone(), Arc::new(|| Some(1)), stop.clone());
    let id = [0x12; 16];
    let owner = [0x34; 16];
    let subnet = subnet(4);
    worker.record_exposed(id, owner, subnet, 1);
    let appended = appender.appended.notified();
    tokio::pin!(appended);
    appended.as_mut().enable();
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(1)).await;
    appended.await;
    let entries = appender.entries.lock().expect("entries").clone();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].event, "api_token_exposed");
    assert_eq!(entries[0].user, "");
    let details: (String, [u8; 16], [u8; 16]) =
        rmp_serde::from_slice(&entries[0].details_canonical_msgpack).expect("details");
    assert_eq!(details, ("rest".into(), id, owner));
    drop(entries);
    stop.cancel();
    worker.shutdown().await;
}
