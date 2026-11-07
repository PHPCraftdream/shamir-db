//! Writer checkpoint contract: FIFO gaps before J, failed J never acknowledged.

use std::future::{poll_fn, Future};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::task::Poll;

use bytes::Bytes;
use tokio::sync::Notify;

use crate::changefeed::{
    clean_horizon_key, decode_clean_horizon, decode_gap_marker, encode_clean_horizon,
    gap_marker_key, ChangelogError, ChangelogStore, RepoChangefeed,
};

#[derive(Default)]
struct CheckpointStore {
    gap: AtomicU64,
    j: AtomicU64,
    fail_gap: AtomicBool,
    fail_j: AtomicBool,
    park_gap: AtomicBool,
    reached: Notify,
    resume: Notify,
}

#[async_trait::async_trait]
impl ChangelogStore for CheckpointStore {
    async fn put(&self, key: Bytes, value: Bytes) -> Result<(), ChangelogError> {
        if key == gap_marker_key() {
            if self.park_gap.swap(false, Ordering::AcqRel) {
                self.reached.notify_one();
                self.resume.notified().await;
            }
            if self.fail_gap.load(Ordering::Acquire) {
                return Err(ChangelogError::Journal("gap failed".into()));
            }
            self.gap.store(
                decode_gap_marker(&value)
                    .map_err(ChangelogError::Journal)?
                    .1,
                Ordering::Release,
            );
        } else if key == clean_horizon_key() {
            if self.fail_j.load(Ordering::Acquire) {
                return Err(ChangelogError::Journal("J failed".into()));
            }
            self.j.store(
                decode_clean_horizon(&value).map_err(ChangelogError::Journal)?,
                Ordering::Release,
            );
        } else {
            return Err(ChangelogError::Journal("unexpected event".into()));
        }
        Ok(())
    }

    async fn range_from(&self, _: Bytes, _: usize) -> Result<Vec<Bytes>, ChangelogError> {
        Ok(vec![])
    }
}

#[test]
fn validated_versioned_horizon_roundtrip_and_rejection() {
    for j in [0, 1, 255, u64::MAX] {
        assert_eq!(decode_clean_horizon(&encode_clean_horizon(j)).unwrap(), j);
    }
    // Independent format oracle, not encoder-generated expected bytes.
    assert_eq!(
        decode_clean_horizon(b"JCLN\x01\0\0\0\0\0\0\0\x09").unwrap(),
        9,
    );
    for bad in [b"".as_slice(), b"JCLN\x02\0\0\0\0\0\0\0\x09", b"JCLN\x01"] {
        assert!(decode_clean_horizon(bad).is_err());
    }
}

// Reverting dirty-gap persistence or swallowing either write error breaks this.
#[tokio::test]
async fn checkpoint_requires_dirty_gap_and_j_success() {
    let backend = Arc::new(CheckpointStore::default());
    let store: Arc<dyn ChangelogStore> = backend.clone();
    let feed = RepoChangefeed::new(store);
    backend.fail_gap.store(true, Ordering::Release);
    feed.mark_gap(7);
    assert!(feed.record_clean_horizon(7).await.is_err());
    assert_eq!(backend.j.load(Ordering::Acquire), 0);
    backend.fail_gap.store(false, Ordering::Release);
    backend.fail_j.store(true, Ordering::Release);
    assert!(feed.record_clean_horizon(7).await.is_err());
    assert_eq!(backend.gap.load(Ordering::Acquire), 7);
    assert_eq!(feed.clean_horizon(), 0);
    backend.fail_j.store(false, Ordering::Release);
    feed.record_clean_horizon(7).await.unwrap();
    feed.record_clean_horizon(3).await.unwrap();
    assert_eq!(backend.j.load(Ordering::Acquire), 7);
    assert_eq!(feed.clean_horizon(), 7);
    feed.shutdown_and_wait().await.unwrap();
}

// A dropped checkpoint waiter must not authorize J after a parked earlier gap.
#[tokio::test]
async fn cancelled_checkpoint_before_j_does_not_write_j() {
    let backend = Arc::new(CheckpointStore::default());
    backend.park_gap.store(true, Ordering::Release);
    let store: Arc<dyn ChangelogStore> = backend.clone();
    let feed = RepoChangefeed::new(store);
    feed.mark_gap(8);
    backend.reached.notified().await;
    let mut request = Box::pin(feed.record_clean_horizon(8));
    assert!(poll_fn(|cx| Poll::Ready(request.as_mut().poll(cx)))
        .await
        .is_pending());
    drop(request);
    backend.resume.notify_one();
    feed.record_recovery_gap(8, 8).await.unwrap();
    assert_eq!(backend.j.load(Ordering::Acquire), 0);
    feed.shutdown_and_wait().await.unwrap();
}
