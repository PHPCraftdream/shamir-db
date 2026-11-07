//! Horizon-aware recovery certification; all scheduling boundaries are explicit.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use bytes::Bytes;
use futures::poll;
use proptest::prelude::*;
use shamir_storage::error::DbError;
use tokio::sync::Notify;

use super::super::{
    decode_gap_marker, encode_gap_marker, gap_marker_key, ChangelogError, ChangelogStore,
    RepoChangefeed,
};

#[derive(Default)]
struct RepairStore {
    staged: Mutex<Option<Bytes>>,
    durable: Mutex<Option<Bytes>>,
    fail_write: AtomicBool,
    fail_flush: AtomicBool,
    hold: AtomicBool,
    started: Notify,
    release: Notify,
    calls: AtomicUsize,
}

impl RepairStore {
    fn with_durable_marker(bytes: Bytes) -> Self {
        Self {
            staged: Mutex::new(Some(bytes.clone())),
            durable: Mutex::new(Some(bytes)),
            ..Self::default()
        }
    }

    fn marker(&self) -> Option<(u64, u64)> {
        self.durable
            .lock()
            .unwrap()
            .as_ref()
            .map(|bytes| decode_gap_marker(bytes).unwrap())
    }
}

#[async_trait::async_trait]
impl ChangelogStore for RepairStore {
    async fn put(&self, key: Bytes, value: Bytes) -> Result<(), ChangelogError> {
        assert_eq!(key, gap_marker_key());
        self.calls.fetch_add(1, Ordering::AcqRel);
        if self.hold.swap(false, Ordering::AcqRel) {
            self.started.notify_one();
            self.release.notified().await;
        }
        if self.fail_write.load(Ordering::Acquire) {
            return Err(ChangelogError::Journal("injected write failure".into()));
        }
        *self.staged.lock().unwrap() = Some(value.clone());
        if self.fail_flush.load(Ordering::Acquire) {
            return Err(ChangelogError::Journal("injected flush failure".into()));
        }
        *self.durable.lock().unwrap() = Some(value);
        Ok(())
    }

    async fn range_from(&self, _: Bytes, _: usize) -> Result<Vec<Bytes>, ChangelogError> {
        Ok(Vec::new())
    }
}

// Negative control: the old decoder rejects the 16-byte range; the old read
// predicate silently skips every cursor greater than the first endpoint.
#[tokio::test]
async fn interval_reads_cover_exactly_the_affected_horizon() {
    let store: Arc<dyn ChangelogStore> = Arc::new(RepairStore::with_durable_marker(
        encode_gap_marker((10, 100)),
    ));
    let feed = RepoChangefeed::new_with_gap_interval(store.clone(), Some((10, 100)));
    for (from, expected) in [(5, Some(10)), (50, Some(50)), (100, Some(100)), (101, None)] {
        assert_eq!(feed.read_from(&store, from, 0).await.gap_at, expected);
    }
    feed.shutdown();
}

// Compatibility control: legacy bytes are independently hand-derived BE u64.
#[tokio::test]
async fn legacy_point_decode_and_reads_are_unchanged() {
    let point = decode_gap_marker(&[0, 0, 0, 0, 0, 0, 0, 10]).unwrap();
    assert_eq!(point, (10, 10));
    let store: Arc<dyn ChangelogStore> =
        Arc::new(RepairStore::with_durable_marker(Bytes::from_static(&[
            0, 0, 0, 0, 0, 0, 0, 10,
        ])));
    let feed = RepoChangefeed::new_with_gap_interval(store.clone(), Some(point));
    for from in [0, 5, 10, 11, u64::MAX] {
        let expected = if from <= 10 { Some(10) } else { None };
        assert_eq!(feed.read_from(&store, from, 0).await.gap_at, expected);
    }
    feed.shutdown();
}

// Negative control: minimum-only marking retains 10 and misses loss at 100.
#[tokio::test]
async fn repeated_disjoint_runtime_loss_after_reseed_is_not_silent() {
    let store: Arc<dyn ChangelogStore> = Arc::new(RepairStore::with_durable_marker(
        encode_gap_marker((10, 10)),
    ));
    let feed = RepoChangefeed::new_with_gap(store.clone(), 10);
    assert_eq!(feed.read_from(&store, 11, 0).await.gap_at, None);
    feed.mark_gap(100);
    assert_eq!(feed.read_from(&store, 100, 0).await.gap_at, Some(100));
    assert_eq!(feed.read_from(&store, 101, 0).await.gap_at, None);
    feed.shutdown();
}

// Negative control: minimum-only durable repair misses a later loss after reseed.
#[tokio::test]
async fn repeated_disjoint_durable_loss_after_reseed_is_not_silent() {
    let fake = Arc::new(RepairStore::with_durable_marker(encode_gap_marker((
        10, 10,
    ))));
    let store: Arc<dyn ChangelogStore> = fake.clone();
    let feed = RepoChangefeed::new_with_gap(store.clone(), 10);
    assert_eq!(feed.read_from(&store, 11, 0).await.gap_at, None);
    feed.record_recovery_gap(100, 100).await.unwrap();
    assert_eq!(fake.marker(), Some((10, 100)));
    assert_eq!(feed.read_from(&store, 100, 0).await.gap_at, Some(100));
    assert_eq!(feed.read_from(&store, 101, 0).await.gap_at, None);
    feed.shutdown();
}

// Negative control: replacing instead of merging shrinks either endpoint;
// acknowledging before store completion loses the repair on reopen.
#[tokio::test]
async fn repair_is_durable_idempotent_and_monotone_across_reopen() {
    let fake = Arc::new(RepairStore::default());
    let store: Arc<dyn ChangelogStore> = fake.clone();
    let feed = RepoChangefeed::new(store.clone());
    let repairs = [(10, 100), (10, 100), (20, 30), (200, 300), (5, 6)];
    let expected = [(10, 100), (10, 100), (10, 100), (10, 300), (5, 300)];
    for (interval, expected) in repairs.into_iter().zip(expected) {
        feed.record_recovery_gap(interval.0, interval.1)
            .await
            .unwrap();
        assert_eq!(fake.marker(), Some(expected));
    }
    assert_eq!(fake.marker(), Some((5, 300)));
    assert_eq!(feed.read_from(&store, 300, 0).await.gap_at, Some(300));
    // Channel closure is the deterministic writer-exit boundary before reopen.
    let closed = feed.journal_tx.closed();
    feed.shutdown();
    closed.await;
    let reopened = RepoChangefeed::new_with_gap_interval(store.clone(), fake.marker());
    assert_eq!(reopened.read_from(&store, 300, 0).await.gap_at, Some(300),);
    assert_eq!(reopened.read_from(&store, 301, 0).await.gap_at, None);
    reopened.shutdown();
}

// Negative controls: swallowing either write/flush error, or merging the repair
// into RAM before flush, violates these assertions. Staged != durable on flush
// failure makes the fake model the actual StoreChangelog contract.
#[tokio::test]
async fn write_and_flush_failure_do_not_publish_speculative_repair() {
    for flush_failure in [false, true] {
        let fake = Arc::new(RepairStore::default());
        let store: Arc<dyn ChangelogStore> = fake.clone();
        let feed = RepoChangefeed::new(store.clone());
        feed.record_recovery_gap(10, 10).await.unwrap();
        fake.fail_write.store(!flush_failure, Ordering::Release);
        fake.fail_flush.store(flush_failure, Ordering::Release);
        let error = feed.record_recovery_gap(50, 100).await.unwrap_err();
        let expected = if flush_failure { "flush" } else { "write" };
        assert!(matches!(error, DbError::Storage(ref text) if text.contains(expected)));
        assert_eq!(fake.marker(), Some((10, 10)));
        assert_eq!(feed.read_from(&store, 50, 0).await.gap_at, None);
        assert_eq!(feed.first_gap_version(), 10);
        assert_eq!(
            fake.staged.lock().unwrap().as_ref().unwrap().len(),
            if flush_failure { 16 } else { 8 },
        );
        fake.fail_write.store(false, Ordering::Release);
        fake.fail_flush.store(false, Ordering::Release);
        feed.record_recovery_gap(50, 100).await.unwrap();
        assert_eq!(fake.marker(), Some((10, 100)));
        feed.shutdown();
    }
}

// Negative control: a stale repair store overwrites the wider concurrent mark,
// or a plain RAM store after flush loses marks arriving while put is suspended.
#[tokio::test]
async fn runtime_marks_during_repair_put_are_merged_before_acknowledgement() {
    let fake = Arc::new(RepairStore::default());
    let store: Arc<dyn ChangelogStore> = fake.clone();
    let feed = RepoChangefeed::new(store.clone());
    fake.hold.store(true, Ordering::Release);
    let repair = feed.record_recovery_gap(10, 100);
    tokio::pin!(repair);
    assert!(poll!(repair.as_mut()).is_pending());
    fake.started.notified().await;
    assert_eq!(feed.read_from(&store, 50, 0).await.gap_at, None);
    feed.mark_gap(5);
    feed.mark_gap(200);
    fake.release.notify_one();
    repair.await.unwrap();
    assert_eq!(fake.marker(), Some((5, 200)));
    assert_eq!(feed.read_from(&store, 200, 0).await.gap_at, Some(200));
    assert_eq!(feed.read_from(&store, 201, 0).await.gap_at, None);
    assert!(fake.calls.load(Ordering::Acquire) >= 2);
    feed.shutdown();
}

// Negative control: an independent repair writer could overtake the blocked
// runtime put, then be overwritten by its stale point marker.
#[tokio::test]
async fn repair_cannot_overtake_an_inflight_runtime_marker() {
    let fake = Arc::new(RepairStore::default());
    let store: Arc<dyn ChangelogStore> = fake.clone();
    let feed = RepoChangefeed::new(store.clone());
    fake.hold.store(true, Ordering::Release);
    feed.mark_gap(10);
    fake.started.notified().await;
    let repair = feed.record_recovery_gap(50, 100);
    tokio::pin!(repair);
    assert!(poll!(repair.as_mut()).is_pending());
    assert_eq!(fake.calls.load(Ordering::Acquire), 1);
    assert_eq!(fake.marker(), None);
    feed.mark_gap(200);
    fake.release.notify_one();
    repair.await.unwrap();
    assert_eq!(fake.marker(), Some((10, 200)));
    assert_eq!(feed.read_from(&store, 200, 0).await.gap_at, Some(200));
    assert_eq!(feed.read_from(&store, 201, 0).await.gap_at, None);
    feed.shutdown();
}

#[test]
fn interval_encoding_matches_independent_big_endian_fixture() {
    let bytes = [0, 0, 0, 0, 0, 0, 0, 10, 0, 0, 0, 0, 0, 0, 0, 100];
    assert_eq!(encode_gap_marker((10, 100)).as_ref(), &bytes);
    assert_eq!(decode_gap_marker(&bytes).unwrap(), (10, 100));
    assert_eq!(
        encode_gap_marker((10, 10)).as_ref(),
        &[0, 0, 0, 0, 0, 0, 0, 10],
    );
}

#[test]
fn malformed_interval_markers_are_rejected() {
    for bytes in [vec![], vec![1; 7], vec![0; 8], vec![0; 16], vec![1; 17]] {
        assert!(decode_gap_marker(&bytes).is_err());
    }
    let mut reversed = 100u64.to_be_bytes().to_vec();
    reversed.extend_from_slice(&10u64.to_be_bytes());
    assert!(decode_gap_marker(&reversed).is_err());
}

proptest! {
    // Negative control: endpoint omission/swapping/truncation breaks the law.
    #[test]
    fn prop_decode_encode_interval_roundtrip(a in 1u64..=u64::MAX, b in 1u64..=u64::MAX) {
        let interval = (a.min(b), a.max(b));
        prop_assert_eq!(decode_gap_marker(&encode_gap_marker(interval)).unwrap(), interval);
    }
}
