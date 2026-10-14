use std::collections::BTreeMap;
use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::task::Poll;
use std::time::Duration;

use arc_swap::ArcSwap;
use bytes::Bytes;
use futures::poll;
use shamir_types::access::Actor;
use tokio::sync::{broadcast, mpsc, Notify};

use super::super::{
    drain_and_persist, gap_marker_key, journal_writer_loop, version_key, ChangelogEvent,
    ChangelogStore, GapMarker, JournalMsg, RepairPending, RepoChangefeed, BROADCAST_CAPACITY,
};

#[derive(Default)]
struct BarrierStore {
    rows: Mutex<BTreeMap<Bytes, Bytes>>,
    hold_ten: AtomicBool,
    fail_ten: AtomicBool,
    release_ten: Notify,
}

#[async_trait::async_trait]
impl ChangelogStore for BarrierStore {
    async fn put(&self, key: Bytes, value: Bytes) -> Result<(), String> {
        if key == version_key(10) {
            if self.hold_ten.swap(false, Ordering::AcqRel) {
                self.release_ten.notified().await;
            }
            if self.fail_ten.swap(false, Ordering::AcqRel) {
                return Err("failed put(10)".into());
            }
        }
        self.rows.lock().unwrap().insert(key, value);
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

fn fixture(capacity: usize) -> (RepoChangefeed, mpsc::Receiver<JournalMsg>) {
    let (live, _) = broadcast::channel(BROADCAST_CAPACITY);
    let (journal_tx, journal_rx) = mpsc::channel(capacity);
    let notify = Arc::new(Notify::new());
    let gap = Arc::new(GapMarker {
        current: ArcSwap::from_pointee(None),
        persisted: ArcSwap::from_pointee(None),
        notify: notify.clone(),
    });
    (
        RepoChangefeed {
            live,
            journal_tx,
            notify,
            shutdown: Arc::new(AtomicBool::new(false)),
            journal_dropped: AtomicU64::new(0),
            last_persisted_version: Arc::new(AtomicU64::new(0)),
            gap,
            confirmed_through: Arc::new(AtomicU64::new(0)),
            barrier_notify: Arc::new(Notify::new()),
            repair_pending: Arc::new(RepairPending::new()),
            writer_done: tokio::sync::watch::channel(false).1,
            clean_horizon: Arc::new(AtomicU64::new(0)),
            confirm_timeouts: AtomicU64::new(0),
        },
        journal_rx,
    )
}

fn writer(
    feed: &RepoChangefeed,
    rx: mpsc::Receiver<JournalMsg>,
    store: Arc<dyn ChangelogStore>,
) -> impl Future<Output = ()> {
    journal_writer_loop(
        rx,
        store,
        feed.notify.clone(),
        feed.shutdown.clone(),
        feed.last_persisted_version.clone(),
        feed.gap.clone(),
        feed.confirmed_through.clone(),
        feed.barrier_notify.clone(),
        feed.repair_pending().clone(),
    )
}

#[tokio::test]
async fn journal_barrier_waits_for_out_of_order_gated_put() {
    let fake = Arc::new(BarrierStore::default());
    fake.hold_ten.store(true, Ordering::Release);
    let store: Arc<dyn ChangelogStore> = fake.clone();
    let (feed, rx) = fixture(8);
    let writer = writer(&feed, rx, store.clone());
    tokio::pin!(writer);
    feed.journal_enqueue(&Arc::new(event(11)));
    feed.journal_enqueue(&Arc::new(event(10)));
    let confirm = feed.confirm_through(11, Duration::from_secs(1));
    tokio::pin!(confirm);
    assert!(poll!(confirm.as_mut()).is_pending());
    assert!(poll!(writer.as_mut()).is_pending());
    assert_eq!(feed.last_persisted_version(), 11);
    assert!(poll!(confirm.as_mut()).is_pending());
    assert_eq!(feed.read_from(&store, 10, 8).await.events, vec![event(11)]);
    fake.release_ten.notify_one();
    assert!(poll!(writer.as_mut()).is_pending());
    assert_eq!(poll!(confirm.as_mut()), Poll::Ready(11));
    let read = feed.read_from_bounded(&store, 10, 8, 11).await;
    assert_eq!(read.events, vec![event(10), event(11)]);
    assert_eq!(read.gap_at, None);
    assert_eq!(fake.rows.lock().unwrap().len(), 2);
    assert_eq!(feed.last_persisted_version(), 11);
    feed.shutdown();
    assert!(poll!(writer.as_mut()).is_ready());
}

// start_paused keeps the clock aligned with the timer wheel; a late pause() can
// land a whole-ms rounding short of an exact deadline.
#[tokio::test(start_paused = true)]
async fn journal_barrier_timeout_returns_previous_prefix() {
    let fake = Arc::new(BarrierStore::default());
    let store: Arc<dyn ChangelogStore> = fake.clone();
    let (feed, rx) = fixture(8);
    let writer = writer(&feed, rx, store.clone());
    tokio::pin!(writer);
    feed.emit_journal_only(event(9));
    let previous = feed.confirm_through(9, Duration::from_secs(1));
    tokio::pin!(previous);
    assert!(poll!(previous.as_mut()).is_pending());
    assert!(poll!(writer.as_mut()).is_pending());
    assert_eq!(poll!(previous.as_mut()), Poll::Ready(9));
    fake.hold_ten.store(true, Ordering::Release);
    feed.emit_journal_only(event(11));
    feed.emit_journal_only(event(10));
    let confirm = feed.confirm_through(11, Duration::from_secs(1));
    tokio::pin!(confirm);
    assert!(poll!(confirm.as_mut()).is_pending());
    assert!(poll!(writer.as_mut()).is_pending());
    tokio::time::advance(Duration::from_millis(1001)).await;
    assert_eq!(poll!(confirm.as_mut()), Poll::Ready(9));
    assert_eq!(feed.confirm_timeouts(), 1);
    assert_eq!(
        feed.read_from_bounded(&store, 0, 8, 9).await.events,
        vec![event(9)],
    );
    assert!(feed
        .read_from_bounded(&store, 10, 1, 9)
        .await
        .events
        .is_empty(),);
    fake.release_ten.notify_one();
    feed.shutdown();
    assert!(poll!(writer.as_mut()).is_ready());
}

// start_paused keeps the clock aligned with the timer wheel; a late pause() can
// land a whole-ms rounding short of an exact deadline.
#[tokio::test(start_paused = true)]
async fn journal_barrier_budget_includes_full_channel_send_and_wait() {
    let store: Arc<dyn ChangelogStore> = Arc::new(BarrierStore::default());
    let (feed, mut rx) = fixture(1);
    feed.journal_enqueue(&Arc::new(event(9)));
    let confirm = feed.confirm_through(11, Duration::from_secs(1));
    tokio::pin!(confirm);
    assert!(poll!(confirm.as_mut()).is_pending());
    tokio::time::advance(Duration::from_millis(750)).await;
    assert!(matches!(rx.try_recv(), Ok(JournalMsg::Event(_))));
    assert!(poll!(confirm.as_mut()).is_pending());
    assert_eq!(feed.journal_tx.capacity(), 0);
    tokio::time::advance(Duration::from_millis(251)).await;
    assert_eq!(poll!(confirm.as_mut()), Poll::Ready(0));
    assert_eq!(feed.confirm_timeouts(), 1);
    let writer = writer(&feed, rx, store.clone());
    tokio::pin!(writer);
    assert!(poll!(writer.as_mut()).is_pending());
    assert_eq!(feed.confirm_through(11, Duration::ZERO).await, 11);
    assert_eq!(feed.last_persisted_version(), 0);
    assert!(feed.read_from(&store, 0, 8).await.events.is_empty());
    feed.shutdown();
    assert!(poll!(writer.as_mut()).is_ready());
}

// start_paused keeps the clock aligned with the timer wheel; a late pause() can
// land a whole-ms rounding short of an exact deadline.
#[tokio::test(start_paused = true)]
async fn journal_barrier_full_send_times_out_without_enqueuing() {
    let (feed, mut rx) = fixture(1);
    feed.journal_enqueue(&Arc::new(event(10)));
    let confirm = feed.confirm_through(10, Duration::from_secs(1));
    tokio::pin!(confirm);
    assert!(poll!(confirm.as_mut()).is_pending());
    tokio::time::advance(Duration::from_millis(1001)).await;
    assert_eq!(poll!(confirm.as_mut()), Poll::Ready(0));
    assert_eq!(feed.confirm_timeouts(), 1);
    assert!(matches!(rx.try_recv(), Ok(JournalMsg::Event(_))));
    assert!(rx.try_recv().is_err());
    assert_eq!(feed.first_gap_version(), 0);
}

#[tokio::test]
async fn journal_barrier_idle_fast_path_sends_nothing() {
    let store: Arc<dyn ChangelogStore> = Arc::new(BarrierStore::default());
    let (feed, rx) = fixture(1);
    let writer = writer(&feed, rx, store);
    tokio::pin!(writer);
    let confirm = feed.confirm_through(20, Duration::from_secs(1));
    tokio::pin!(confirm);
    assert!(poll!(confirm.as_mut()).is_pending());
    assert!(poll!(writer.as_mut()).is_pending());
    assert_eq!(poll!(confirm.as_mut()), Poll::Ready(20));
    assert_eq!(feed.journal_tx.capacity(), 1);
    assert_eq!(feed.confirm_through(10, Duration::ZERO).await, 10);
    assert_eq!(feed.confirm_through(20, Duration::ZERO).await, 20);
    assert_eq!(feed.journal_tx.capacity(), 1);
    assert_eq!(feed.last_persisted_version(), 0);
    feed.shutdown();
    assert!(poll!(writer.as_mut()).is_ready());
}

#[tokio::test]
async fn journal_barrier_full_and_closed_enqueue_mark_gap_immediately() {
    let store: Arc<dyn ChangelogStore> = Arc::new(BarrierStore::default());
    let (feed, rx) = fixture(1);
    feed.journal_enqueue(&Arc::new(event(11)));
    feed.journal_enqueue(&Arc::new(event(10)));
    assert_eq!(feed.first_gap_version(), 10);
    assert_eq!(feed.journal_dropped(), 1);
    assert_eq!(
        feed.read_from_bounded(&store, 9, 0, 0).await.gap_at,
        Some(10),
    );
    drop(rx);
    feed.journal_enqueue(&Arc::new(event(8)));
    assert_eq!(feed.first_gap_version(), 8);
    assert_eq!(feed.confirm_through(11, Duration::from_secs(1)).await, 0);
    assert_eq!(feed.confirm_timeouts(), 0);
    assert_eq!(feed.read_from(&store, 0, 8).await.gap_at, Some(8));
}

#[tokio::test]
async fn journal_barrier_counter_increment_order_is_not_channel_order() {
    let fake = Arc::new(BarrierStore::default());
    let store: Arc<dyn ChangelogStore> = fake.clone();
    let (feed, rx) = fixture(8);
    let writer = writer(&feed, rx, store.clone());
    tokio::pin!(writer);
    let enqueued = AtomicU64::new(0);
    // Producer 11 pauses after send, before its counter increment; producer 10
    // increments first. One persisted row falsely satisfies the counter snapshot.
    feed.journal_enqueue(&Arc::new(event(11)));
    assert!(poll!(writer.as_mut()).is_pending());
    feed.journal_enqueue(&Arc::new(event(10)));
    enqueued.fetch_add(1, Ordering::Relaxed);
    let snapshot = enqueued.load(Ordering::Relaxed);
    let persisted_count = fake.rows.lock().unwrap().len() as u64;
    assert_eq!(persisted_count, snapshot);
    assert_eq!(snapshot, 1);
    assert_eq!(feed.read_from(&store, 10, 8).await.events, vec![event(11)]);
    let confirm = feed.confirm_through(11, Duration::from_secs(1));
    tokio::pin!(confirm);
    assert!(poll!(confirm.as_mut()).is_pending());
    enqueued.fetch_add(1, Ordering::Relaxed);
    feed.journal_enqueue(&Arc::new(event(12)));
    assert!(poll!(writer.as_mut()).is_pending());
    assert_eq!(poll!(confirm.as_mut()), Poll::Ready(11));
    assert_eq!(
        feed.read_from_bounded(&store, 10, 8, 11).await.events,
        vec![event(10), event(11)],
    );
    assert_eq!(feed.read_from(&store, 12, 8).await.events, vec![event(12)]);
    assert_eq!(fake.rows.lock().unwrap().len(), 3);
    feed.shutdown();
    assert!(poll!(writer.as_mut()).is_ready());
}

#[tokio::test]
async fn journal_barrier_rearms_after_lower_and_spurious_notifications() {
    let fake = Arc::new(BarrierStore::default());
    fake.hold_ten.store(true, Ordering::Release);
    let store: Arc<dyn ChangelogStore> = fake.clone();
    let (feed, rx) = fixture(8);
    feed.journal_tx.try_send(JournalMsg::Barrier(9)).unwrap();
    feed.journal_enqueue(&Arc::new(event(10)));
    let writer = writer(&feed, rx, store);
    tokio::pin!(writer);
    let low = feed.confirm_through(9, Duration::from_secs(1));
    let high = feed.confirm_through(10, Duration::from_secs(1));
    tokio::pin!(low, high);
    assert!(poll!(low.as_mut()).is_pending());
    assert!(poll!(high.as_mut()).is_pending());
    assert!(poll!(writer.as_mut()).is_pending());
    assert_eq!(poll!(low.as_mut()), Poll::Ready(9));
    assert!(poll!(high.as_mut()).is_pending());
    feed.barrier_notify.notify_waiters();
    assert!(poll!(high.as_mut()).is_pending());
    fake.release_ten.notify_one();
    assert!(poll!(writer.as_mut()).is_pending());
    assert_eq!(poll!(high.as_mut()), Poll::Ready(10));
    assert_eq!(feed.confirm_timeouts(), 0);
    feed.shutdown();
    assert!(poll!(writer.as_mut()).is_ready());
}

#[tokio::test]
async fn journal_barrier_shutdown_and_explicit_drain_process_every_message() {
    for shutdown_first in [false, true] {
        let fake = Arc::new(BarrierStore::default());
        let store: Arc<dyn ChangelogStore> = fake.clone();
        let (feed, mut rx) = fixture(8);
        feed.journal_enqueue(&Arc::new(event(11)));
        feed.journal_tx.try_send(JournalMsg::Barrier(11)).unwrap();
        feed.journal_enqueue(&Arc::new(event(10)));
        feed.journal_tx.try_send(JournalMsg::Barrier(20)).unwrap();
        if shutdown_first {
            feed.shutdown();
            let writer = writer(&feed, rx, store.clone());
            tokio::pin!(writer);
            assert!(poll!(writer.as_mut()).is_ready());
        } else {
            drain_and_persist(
                &mut rx,
                &store,
                &feed.last_persisted_version,
                &feed.gap,
                &feed.confirmed_through,
                &feed.barrier_notify,
                &feed.repair_pending,
            )
            .await;
        }
        assert_eq!(feed.confirm_through(20, Duration::ZERO).await, 20);
        assert_eq!(feed.last_persisted_version(), 11);
        assert_eq!(fake.rows.lock().unwrap().len(), 2);
        assert_eq!(
            feed.read_from(&store, 0, 8).await.events,
            vec![event(10), event(11)],
        );
    }
}

#[tokio::test]
async fn journal_barrier_failed_put_finishes_with_gap_before_confirmation() {
    let fake = Arc::new(BarrierStore::default());
    fake.fail_ten.store(true, Ordering::Release);
    let store: Arc<dyn ChangelogStore> = fake.clone();
    let (feed, rx) = fixture(8);
    feed.journal_enqueue(&Arc::new(event(10)));
    let writer = writer(&feed, rx, store.clone());
    tokio::pin!(writer);
    let confirm = feed.confirm_through(10, Duration::from_secs(1));
    tokio::pin!(confirm);
    assert!(poll!(confirm.as_mut()).is_pending());
    assert!(poll!(writer.as_mut()).is_pending());
    assert_eq!(poll!(confirm.as_mut()), Poll::Ready(10));
    let read = feed.read_from_bounded(&store, 10, 8, 10).await;
    assert!(read.events.is_empty());
    assert_eq!(read.gap_at, Some(10));
    assert_eq!(feed.last_persisted_version(), 0);
    assert!(!fake.rows.lock().unwrap().contains_key(&version_key(10)));
    assert!(fake.rows.lock().unwrap().contains_key(&gap_marker_key()));
    feed.shutdown();
    assert!(poll!(writer.as_mut()).is_ready());
}

#[tokio::test]
async fn journal_barrier_split_tracks_and_emit_preserve_event_identity() {
    let store: Arc<dyn ChangelogStore> = Arc::new(BarrierStore::default());
    let (feed, rx) = fixture(8);
    let mut live = feed.subscribe();
    let shared = Arc::new(event(9));
    feed.journal_enqueue(&shared);
    assert!(live.try_recv().is_err());
    feed.broadcast(shared.clone());
    assert!(Arc::ptr_eq(&live.try_recv().unwrap(), &shared));
    feed.emit(event(10));
    assert_eq!(*live.try_recv().unwrap(), event(10));
    feed.emit_journal_only(event(11));
    assert!(live.try_recv().is_err());
    let writer = writer(&feed, rx, store.clone());
    tokio::pin!(writer);
    feed.shutdown();
    assert!(poll!(writer.as_mut()).is_ready());
    assert_eq!(
        feed.read_from(&store, 0, 8).await.events,
        vec![event(9), event(10), event(11)],
    );
    assert_eq!(feed.last_persisted_version(), 11);
}
