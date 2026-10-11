//! Actual StoreChangelog set/flush boundaries, legacy loading and init seeding.

use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use bytes::Bytes;
use futures::Stream;
use shamir_storage::error::{DbError, DbResult};
use shamir_storage::storage_in_memory::{InMemoryRepo, InMemoryStore};
use shamir_storage::types::{RecordKey, Repo, Store};
use shamir_tx::changefeed::{decode_gap_marker, encode_gap_marker, gap_marker_key, version_key};
use shamir_tx::{ChangelogStore, RepoChangefeed};

use crate::repo::changelog_store::StoreChangelog;
use crate::repo::repo_instance::RepoInstance;
use crate::repo::repo_types::BoxRepo;

type TestStream = Pin<Box<dyn Stream<Item = DbResult<Vec<(RecordKey, Bytes)>>> + Send>>;

struct BoundaryStore {
    inner: Arc<dyn Store>,
    fail_set: AtomicBool,
    fail_flush: AtomicBool,
    sets: AtomicUsize,
    flushes: AtomicUsize,
}

#[async_trait::async_trait]
impl Store for BoundaryStore {
    async fn insert(&self, value: Bytes) -> DbResult<RecordKey> {
        self.inner.insert(value).await
    }

    async fn set(&self, key: RecordKey, value: Bytes) -> DbResult<bool> {
        self.sets.fetch_add(1, Ordering::AcqRel);
        if self.fail_set.load(Ordering::Acquire) {
            return Err(DbError::Storage("injected set failure".into()));
        }
        self.inner.set(key, value).await
    }

    async fn get(&self, key: RecordKey) -> DbResult<Bytes> {
        self.inner.get(key).await
    }

    async fn remove(&self, key: RecordKey) -> DbResult<bool> {
        self.inner.remove(key).await
    }

    async fn flush(&self) -> DbResult<()> {
        self.flushes.fetch_add(1, Ordering::AcqRel);
        if self.fail_flush.load(Ordering::Acquire) {
            return Err(DbError::Storage("injected flush failure".into()));
        }
        self.inner.flush().await
    }

    fn iter_stream(&self, batch_size: usize) -> TestStream {
        self.inner.iter_stream(batch_size)
    }

    fn scan_prefix_stream(&self, prefix: Bytes, batch_size: usize) -> TestStream {
        self.inner.scan_prefix_stream(prefix, batch_size)
    }
}

// Negative controls: omitting the real adapter flush, swallowing its failure,
// or seeding RAM before flush lets the second case wrongly return success/gap.
#[tokio::test]
async fn actual_adapter_set_and_flush_errors_reach_repair_caller() {
    for flush_failure in [false, true] {
        let backend = Arc::new(BoundaryStore {
            inner: Arc::new(InMemoryStore::new()),
            fail_set: AtomicBool::new(!flush_failure),
            fail_flush: AtomicBool::new(flush_failure),
            sets: AtomicUsize::new(0),
            flushes: AtomicUsize::new(0),
        });
        let store: Arc<dyn ChangelogStore> = Arc::new(StoreChangelog::new(backend.clone()));
        let feed = RepoChangefeed::new(store.clone());
        let error = feed.record_recovery_gap(10, 100).await.unwrap_err();
        assert!(matches!(error, DbError::Storage(ref text)
            if text.contains(if flush_failure { "flush" } else { "set" })));
        assert_eq!(backend.sets.load(Ordering::Acquire), 1);
        assert_eq!(
            backend.flushes.load(Ordering::Acquire),
            usize::from(flush_failure),
        );
        assert_eq!(feed.read_from(&store, 50, 0).await.gap_at, None);
        assert_eq!(feed.first_gap_version(), 0);
        // Flush failure occurs after set, not instead of set.
        if flush_failure {
            let staged = backend.get(gap_marker_key().into()).await.unwrap();
            assert_eq!(decode_gap_marker(&staged).unwrap(), (10, 100));
        } else {
            assert!(matches!(
                backend.get(gap_marker_key().into()).await,
                Err(DbError::NotFound(_))
            ));
        }
        feed.shutdown();
    }
}

// Negative control: the old repo decoder sees 16 bytes as corrupt point 1,
// missing cursor 50. Loading a pending minimum alone also drops horizon 100.
#[tokio::test]
async fn actual_marker_roundtrip_and_pending_init_preserve_horizon() {
    let repo = Arc::new(InMemoryRepo::new());
    let backend = repo.store_get("__changelog__").await.unwrap();
    let adapter = StoreChangelog::new(backend.clone());
    adapter
        .put(gap_marker_key(), encode_gap_marker((10, 100)))
        .await
        .unwrap();
    assert_eq!(
        adapter.load_gap_marker().await.unwrap(),
        (Some((10, 100)), false),
    );
    let instance = RepoInstance::new("range-marker".into(), BoxRepo::InMemory(repo), vec![]);
    instance.record_pending_changefeed_gap(5);
    let handle = instance.changefeed().await.unwrap();
    assert_eq!(
        handle.feed.read_from(&handle.store, 50, 0).await.gap_at,
        Some(50),
    );
    assert_eq!(
        handle.feed.read_from(&handle.store, 101, 0).await.gap_at,
        None,
    );
    let persisted = backend.get(gap_marker_key().into()).await.unwrap();
    assert_eq!(decode_gap_marker(&persisted).unwrap(), (5, 100));
    handle.feed.shutdown();
}

// Compatibility control plus corruption safety: the new loader must not claim
// an unflushed replacement as persisted or limit unknown corruption to point 1.
#[tokio::test]
async fn adapter_loads_legacy_and_init_flushes_corrupt_replacement() {
    let repo = Arc::new(InMemoryRepo::new());
    let backend = repo.store_get("__changelog__").await.unwrap();
    backend
        .set(gap_marker_key().into(), version_key(17))
        .await
        .unwrap();
    let adapter = StoreChangelog::new(backend.clone());
    assert_eq!(
        adapter.load_gap_marker().await.unwrap(),
        (Some((17, 17)), false),
    );
    backend
        .set(gap_marker_key().into(), Bytes::from_static(b"bad"))
        .await
        .unwrap();
    let instance = RepoInstance::new("corrupt-marker".into(), BoxRepo::InMemory(repo), vec![]);
    let handle = instance.changefeed().await.unwrap();
    assert_eq!(
        handle.feed.read_from(&handle.store, 100, 0).await.gap_at,
        Some(100),
    );
    let persisted = backend.get(gap_marker_key().into()).await.unwrap();
    assert_eq!(decode_gap_marker(&persisted).unwrap(), (1, u64::MAX));
    handle.feed.shutdown();
}
