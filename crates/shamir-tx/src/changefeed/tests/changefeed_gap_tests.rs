//! Deterministic journal-hole and marker persistence regressions.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::Poll;

use arc_swap::ArcSwap;
use bytes::Bytes;
use futures::poll;
use shamir_types::access::Actor;
use tokio::sync::{broadcast, mpsc, Notify};

use super::super::{
    decode_gap_marker, gap_marker_key, journal_writer_loop, version_key, ChangelogEvent,
    ChangelogStore, GapMarker, RepairPending, RepoChangefeed, BROADCAST_CAPACITY,
    JOURNAL_CHANNEL_CAPACITY,
};

#[derive(Default)]
struct GapStore {
    rows: Mutex<BTreeMap<Bytes, Bytes>>,
    fail_event: AtomicBool,
    fail_marker: AtomicBool,
    hold_marker: AtomicBool,
    marker_attempts: AtomicUsize,
    event_attempted: Notify,
    event_written: Notify,
    marker_started: Notify,
    release_marker: Notify,
    written: Notify,
}

impl GapStore {
    fn marker(&self) -> u64 {
        let rows = self.rows.lock().unwrap();
        let bytes = rows.get(&gap_marker_key()).expect("persisted marker");
        decode_gap_marker(bytes).unwrap().0
    }

    async fn wait_marker(&self, version: u64) {
        loop {
            let notified = self.written.notified();
            // Existing tests assert the minimum; the new range also carries a horizon.
            let first = self
                .rows
                .lock()
                .unwrap()
                .get(&gap_marker_key())
                .map(|bytes| decode_gap_marker(bytes).unwrap().0);
            if first == Some(version) {
                return;
            }
            notified.await;
        }
    }
}

#[async_trait::async_trait]
impl ChangelogStore for GapStore {
    async fn put(&self, key: Bytes, value: Bytes) -> Result<(), String> {
        let is_marker = key == gap_marker_key();
        if is_marker {
            self.marker_attempts.fetch_add(1, Ordering::SeqCst);
            self.marker_started.notify_one();
            if self.hold_marker.swap(false, Ordering::SeqCst) {
                self.release_marker.notified().await;
            }
            if self.fail_marker.swap(false, Ordering::SeqCst) {
                return Err("one marker failure".into());
            }
        } else {
            self.event_attempted.notify_one();
            if self.fail_event.swap(false, Ordering::SeqCst) {
                return Err("one event failure".into());
            }
        }
        self.rows.lock().unwrap().insert(key, value);
        if !is_marker {
            self.event_written.notify_one();
        }
        self.written.notify_one();
        Ok(())
    }

    async fn range_from(&self, from_key: Bytes, limit: usize) -> Result<Vec<Bytes>, String> {
        Ok(self
            .rows
            .lock()
            .unwrap()
            .range(from_key..)
            .filter(|(key, _)| key.len() == 8)
            .take(limit)
            .map(|(_, value)| value.clone())
            .collect())
    }
}

fn event(version: u64) -> ChangelogEvent {
    ChangelogEvent {
        repo: "main".into(),
        commit_version: version,
        tx_id: version,
        actor: Actor::System,
        timestamp_ns: 0,
        changes: Vec::new(),
    }
}

#[tokio::test]
async fn failed_event_put_reports_and_persists_gap() {
    let fake = Arc::new(GapStore::default());
    fake.fail_event.store(true, Ordering::SeqCst);
    fake.hold_marker.store(true, Ordering::SeqCst);
    let store: Arc<dyn ChangelogStore> = fake.clone();
    let feed = RepoChangefeed::new(store.clone());
    feed.emit(event(7));
    feed.emit(event(8));
    fake.event_attempted.notified().await;
    // The next event acknowledges that persist_one finished the failed put.
    // This also completes on the original code, so a missing gap fails here.
    fake.event_written.notified().await;
    let read = feed.read_from(&store, 1, 10).await;
    assert_eq!(read.events, vec![event(8)]);
    assert_eq!(read.gap_at, Some(7));
    assert_eq!(feed.first_gap_version(), 7);
    assert_eq!(feed.last_persisted_version(), 8);
    assert!(!fake.rows.lock().unwrap().contains_key(&gap_marker_key()));
    fake.release_marker.notify_one();
    fake.wait_marker(7).await;
    assert_eq!(fake.marker(), 7);
    feed.shutdown();
}

#[tokio::test]
async fn corrupt_record_marks_read_start_and_survives_recreation() {
    let fake = Arc::new(GapStore::default());
    fake.rows
        .lock()
        .unwrap()
        .insert(version_key(9), Bytes::from_static(b"\xc1"));
    let store: Arc<dyn ChangelogStore> = fake.clone();
    let feed = RepoChangefeed::new(store.clone());
    let read = feed.read_from(&store, 4, 10).await;
    assert!(read.events.is_empty());
    assert_eq!(read.gap_at, Some(4));
    fake.wait_marker(4).await;
    feed.shutdown();
    let restored = RepoChangefeed::new_with_gap(store.clone(), fake.marker());
    assert_eq!(restored.first_gap_version(), 4);
    assert_eq!(restored.read_from(&store, 4, 0).await.gap_at, Some(4));
    assert_eq!(restored.read_from(&store, 5, 0).await.gap_at, None);
    restored.shutdown();
}

#[tokio::test]
async fn corrupt_record_after_older_minimum_still_reports_this_read_gap() {
    let fake = Arc::new(GapStore::default());
    fake.rows
        .lock()
        .unwrap()
        .insert(gap_marker_key(), version_key(2));
    fake.rows
        .lock()
        .unwrap()
        .insert(version_key(9), Bytes::from_static(b"\xc1"));
    let store: Arc<dyn ChangelogStore> = fake.clone();
    let feed = RepoChangefeed::new_with_gap(store.clone(), 2);
    let read = feed.read_from(&store, 4, 10).await;
    assert!(read.events.is_empty());
    assert_eq!(read.gap_at, Some(4));
    assert_eq!(feed.first_gap_version(), 2);
    assert_eq!(fake.marker(), 2);
    assert_eq!(feed.read_from(&store, 1, 0).await.gap_at, Some(2));
    feed.shutdown();
}

#[tokio::test]
async fn healthy_recreated_feed_has_no_gap_or_metadata_events() {
    let fake = Arc::new(GapStore::default());
    let store: Arc<dyn ChangelogStore> = fake.clone();
    store
        .put(
            version_key(3),
            Bytes::from(rmp_serde::to_vec(&event(3)).unwrap()),
        )
        .await
        .unwrap();
    for _ in 0..2 {
        let feed = RepoChangefeed::new_with_gap(store.clone(), 0);
        let read = feed.read_from(&store, 0, 10).await;
        assert_eq!(read.events, vec![event(3)]);
        assert_eq!(read.gap_at, None);
        assert!(!fake.rows.lock().unwrap().contains_key(&gap_marker_key()));
        feed.shutdown();
    }
}

#[tokio::test]
async fn concurrent_lower_gap_cannot_be_overwritten_by_stale_put() {
    let fake = Arc::new(GapStore::default());
    fake.hold_marker.store(true, Ordering::SeqCst);
    let store: Arc<dyn ChangelogStore> = fake.clone();
    let feed = RepoChangefeed::new(store.clone());
    feed.mark_gap(20);
    fake.marker_started.notified().await;
    let other = feed.clone();
    std::thread::spawn(move || {
        other.mark_gap(5);
        other.mark_gap(30);
        other.mark_gap(5);
    })
    .join()
    .unwrap();
    assert_eq!(feed.first_gap_version(), 5);
    fake.release_marker.notify_one();
    fake.wait_marker(5).await;
    assert_eq!(fake.marker(), 5);
    assert_eq!(feed.read_from(&store, 1, 10).await.gap_at, Some(5));
    feed.shutdown();
}

#[tokio::test]
async fn failed_marker_put_retains_gap_and_retries_on_next_hint() {
    let fake = Arc::new(GapStore::default());
    let store: Arc<dyn ChangelogStore> = fake.clone();
    // Drive the real writer future ourselves: Pending is a precise oracle
    // that it has reached its wait, not a scheduler yield or a timed guess.
    let (live, _) = broadcast::channel(BROADCAST_CAPACITY);
    let (journal_tx, journal_rx) = mpsc::channel(JOURNAL_CHANNEL_CAPACITY);
    let notify = Arc::new(Notify::new());
    let shutdown = Arc::new(AtomicBool::new(false));
    let watermark = Arc::new(AtomicU64::new(0));
    let gap = Arc::new(GapMarker {
        current: ArcSwap::from_pointee(None),
        persisted: ArcSwap::from_pointee(None),
        notify: notify.clone(),
    });
    let feed = RepoChangefeed {
        live,
        journal_tx,
        notify: notify.clone(),
        shutdown: shutdown.clone(),
        journal_dropped: AtomicU64::new(0),
        last_persisted_version: watermark.clone(),
        gap: gap.clone(),
        confirmed_through: Arc::new(AtomicU64::new(0)),
        barrier_notify: Arc::new(Notify::new()),
        repair_pending: Arc::new(RepairPending::new()),
        writer_done: tokio::sync::watch::channel(false).1,
        clean_horizon: Arc::new(AtomicU64::new(0)),
        confirm_timeouts: AtomicU64::new(0),
    };
    let writer = journal_writer_loop(
        journal_rx,
        store.clone(),
        notify,
        shutdown,
        watermark,
        gap,
        feed.confirmed_through.clone(),
        feed.barrier_notify.clone(),
        feed.repair_pending().clone(),
    );
    tokio::pin!(writer);

    feed.mark_gap(8);
    // Exhaust the initial hint BEFORE injecting the failure. Otherwise the
    // stored permit can retry automatically after the first top-of-loop put.
    let hint = feed.notify.notified();
    tokio::pin!(hint);
    assert_eq!(poll!(hint.as_mut()), Poll::Ready(()));
    assert_eq!(fake.marker_attempts.load(Ordering::SeqCst), 0);
    fake.fail_marker.store(true, Ordering::SeqCst);
    assert!(poll!(writer.as_mut()).is_pending());
    assert_eq!(fake.marker_attempts.load(Ordering::SeqCst), 1);
    assert_eq!(feed.read_from(&store, 1, 0).await.gap_at, Some(8));
    assert!(!fake.rows.lock().unwrap().contains_key(&gap_marker_key()));
    // Even explicitly polling the waiting writer cannot make another attempt.
    assert!(poll!(writer.as_mut()).is_pending());
    assert_eq!(fake.marker_attempts.load(Ordering::SeqCst), 1);

    feed.mark_gap(8);
    assert!(poll!(writer.as_mut()).is_pending());
    assert_eq!(fake.marker_attempts.load(Ordering::SeqCst), 2);
    assert_eq!(fake.marker(), 8);
    assert!(poll!(writer.as_mut()).is_pending());
    assert_eq!(fake.marker_attempts.load(Ordering::SeqCst), 2);
    feed.shutdown();
    assert!(poll!(writer.as_mut()).is_ready());
    assert_eq!(fake.marker_attempts.load(Ordering::SeqCst), 2);
}
