//! Actual StoreChangelog set/flush boundaries, legacy loading and init seeding.

use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};

use bytes::Bytes;
use futures::Stream;
use shamir_storage::error::{DbError, DbResult};
use shamir_storage::storage_in_memory::{InMemoryRepo, InMemoryStore};
use shamir_storage::types::{RecordKey, Repo, Store};
use shamir_tx::changefeed::{decode_gap_marker, encode_gap_marker, gap_marker_key, version_key};
use shamir_tx::{ChangelogStore, RepoChangefeed};
use shamir_types::types::common::THasher;

use crate::db_instance::db_instance::DbInstance;
use crate::repo::changelog_store::StoreChangelog;
use crate::repo::repo_instance::{ChangefeedHandle, RepoInstance};
use crate::repo::repo_types::BoxRepo;
use crate::repo::{BoxRepoFactory, RepoConfig};

type TestStream = Pin<Box<dyn Stream<Item = DbResult<Vec<(RecordKey, Bytes)>>> + Send>>;

struct BoundaryStore {
    inner: Arc<dyn Store>,
    fail_set: AtomicBool,
    fail_flush: AtomicBool,
    fail_j: AtomicBool,
    fail_flush_at: AtomicUsize,
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
        if self.fail_set.load(Ordering::Acquire)
            || (key.as_ref() == shamir_tx::changefeed::clean_horizon_key().as_ref()
                && self.fail_j.load(Ordering::Acquire))
        {
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
        let n = self.flushes.fetch_add(1, Ordering::AcqRel) + 1;
        let fail_at = self.fail_flush_at.load(Ordering::Acquire);
        if self.fail_flush.load(Ordering::Acquire) || fail_at == n {
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

// Per-name one-shot fixture. Production constructors contain no override.
// Names are unique per test; no global fault flag can affect another opener.
static OPEN_FIXTURES: OnceLock<scc::HashMap<String, RepoInstance, THasher>> = OnceLock::new();

pub(crate) fn take_open_fixture(name: &str) -> Option<RepoInstance> {
    OPEN_FIXTURES
        .get()?
        .remove_sync(name)
        .map(|(_, instance)| instance)
}

fn install_open_fixture(name: &str, instance: RepoInstance) {
    let fixtures = OPEN_FIXTURES.get_or_init(|| scc::HashMap::with_hasher(THasher::default()));
    assert!(fixtures.insert_sync(name.to_string(), instance).is_ok());
}

#[tokio::test]
async fn injected_certification_failures_never_enter_registry() {
    for failure in 0..3 {
        let name = format!("registry-certification-failure-{failure}");
        let backing = Arc::new(InMemoryRepo::new());
        let info = backing.store_get("__tx__").await.unwrap();
        crate::meta::recovery_marker::save_last_committed(&info, 12)
            .await
            .unwrap();
        let backend = Arc::new(BoundaryStore {
            inner: backing.store_get("__changelog__").await.unwrap(),
            fail_set: AtomicBool::new(failure == 0),
            fail_flush: AtomicBool::new(failure == 1),
            fail_j: AtomicBool::new(failure == 2),
            fail_flush_at: AtomicUsize::new(0),
            sets: AtomicUsize::new(0),
            flushes: AtomicUsize::new(0),
        });
        let store: Arc<dyn ChangelogStore> = Arc::new(StoreChangelog::new(backend.clone()));
        let feed = RepoChangefeed::new(store.clone());
        let instance = RepoInstance::new(name.clone(), BoxRepo::InMemory(backing.clone()), vec![]);
        instance.install_journal_for_test(ChangefeedHandle {
            feed: feed.clone(),
            store,
        });
        install_open_fixture(&name, instance.clone());
        let db = DbInstance::new();
        let error = db
            .add_repo(RepoConfig::new(&name, BoxRepoFactory::in_memory()))
            .await
            .unwrap_err();
        assert!(matches!(error, DbError::Storage(ref text)
            if text.contains(if failure == 1 { "flush" } else { "set" })));
        assert!(db.get_repo(&name).is_none());
        assert!(db.list_repos().is_empty());
        install_open_fixture(&name, instance);
        assert!(
            DbInstance::with_repos(vec![RepoConfig::new(&name, BoxRepoFactory::in_memory(),)])
                .await
                .is_err()
        );
        assert_eq!(feed.clean_horizon(), 0);
        assert!(backend.sets.load(Ordering::Acquire) > 0);
        assert!(matches!(
            backend
                .get(shamir_tx::changefeed::clean_horizon_key().into())
                .await,
            Err(DbError::NotFound(_))
        ));
        feed.shutdown_and_wait().await.unwrap();
        drop(feed);
        // No old writer can race this retry. The fixture also exercises the
        // public async constructor directly, rather than only registry access.
        let retry = RepoInstance::new(name.clone(), BoxRepo::InMemory(backing), vec![]);
        install_open_fixture(&name, retry);
        let reopened = RepoInstance::from_factory(name, BoxRepoFactory::in_memory(), vec![])
            .await
            .unwrap();
        let handle = reopened.changefeed().await.unwrap();
        assert_eq!(handle.feed.clean_horizon(), 12);
        assert_eq!(
            handle.feed.read_from(&handle.store, 10, 0).await.gap_at,
            Some(10),
        );
        handle.feed.shutdown_and_wait().await.unwrap();
    }
}

// Inject gap set/flush and J set failure at the real recovery entry point.
// Reopening a NEW instance on the SAME backing proves retry convergence.
#[tokio::test]
async fn cold_recovery_write_failures_retry_on_hard_reopen() {
    for failure in 0..3 {
        let backing = Arc::new(InMemoryRepo::new());
        let info = backing.store_get("__tx__").await.unwrap();
        crate::meta::recovery_marker::save_last_committed(&info, 12)
            .await
            .unwrap();
        let backend = Arc::new(BoundaryStore {
            inner: backing.store_get("__changelog__").await.unwrap(),
            fail_set: AtomicBool::new(failure == 0),
            fail_flush: AtomicBool::new(failure == 1),
            fail_j: AtomicBool::new(failure == 2),
            fail_flush_at: AtomicUsize::new(0),
            sets: AtomicUsize::new(0),
            flushes: AtomicUsize::new(0),
        });
        let store: Arc<dyn ChangelogStore> = Arc::new(StoreChangelog::new(backend.clone()));
        let feed = RepoChangefeed::new(store.clone());
        let repo = RepoInstance::new(
            "failed-open".into(),
            BoxRepo::InMemory(backing.clone()),
            vec![],
        );
        repo.install_journal_for_test(ChangefeedHandle {
            feed: feed.clone(),
            store,
        });
        assert!(repo.recover_v2_inflight().await.is_err());
        assert_eq!(feed.clean_horizon(), 0);
        // No J set happened. A failed J leaves the flushed gap independently durable.
        assert!(matches!(
            backend
                .get(shamir_tx::changefeed::clean_horizon_key().into())
                .await,
            Err(DbError::NotFound(_))
        ));
        if failure == 2 {
            let interval = backend.get(gap_marker_key().into()).await.unwrap();
            assert_eq!(decode_gap_marker(&interval).unwrap(), (1, 12));
        }
        // Sole writer has no outstanding request: acknowledged failures completed.
        feed.shutdown_and_wait().await.unwrap();
        drop(repo);
        drop(feed);
        let reopened = RepoInstance::new("failed-open".into(), BoxRepo::InMemory(backing), vec![]);
        reopened.recover_v2_inflight().await.unwrap();
        let handle = reopened.changefeed().await.unwrap();
        assert_eq!(handle.feed.clean_horizon(), 12);
        assert_eq!(
            handle.feed.read_from(&handle.store, 10, 0).await.gap_at,
            Some(10),
        );
        handle.feed.shutdown_and_wait().await.unwrap();
    }
}

#[tokio::test]
async fn failed_shutdown_checkpoint_leaves_old_j_and_reopen_certifies() {
    let backing = Arc::new(InMemoryRepo::new());
    let info = backing.store_get("__tx__").await.unwrap();
    crate::meta::recovery_marker::save_last_committed(&info, 15)
        .await
        .unwrap();
    let backend = Arc::new(BoundaryStore {
        inner: backing.store_get("__changelog__").await.unwrap(),
        fail_set: AtomicBool::new(false),
        fail_flush: AtomicBool::new(false),
        fail_j: AtomicBool::new(true),
        fail_flush_at: AtomicUsize::new(0),
        sets: AtomicUsize::new(0),
        flushes: AtomicUsize::new(0),
    });
    let store: Arc<dyn ChangelogStore> = Arc::new(StoreChangelog::new(backend.clone()));
    let feed = RepoChangefeed::new(store.clone());
    let repo = RepoInstance::new(
        "failed-stop".into(),
        BoxRepo::InMemory(backing.clone()),
        vec![],
    );
    repo.install_journal_for_test(ChangefeedHandle {
        feed: feed.clone(),
        store,
    });
    assert!(repo
        .shutdown_clean(std::time::Duration::from_secs(30))
        .await
        .is_err());
    assert_eq!(feed.clean_horizon(), 0);
    assert!(matches!(
        backend
            .get(shamir_tx::changefeed::clean_horizon_key().into())
            .await,
        Err(DbError::NotFound(_))
    ));
    feed.shutdown_and_wait().await.unwrap();
    drop(repo);
    drop(feed);
    let reopened = RepoInstance::new("failed-stop".into(), BoxRepo::InMemory(backing), vec![]);
    reopened.recover_v2_inflight().await.unwrap();
    let handle = reopened.changefeed().await.unwrap();
    assert_eq!(handle.feed.clean_horizon(), 15);
    assert_eq!(
        handle.feed.read_from(&handle.store, 15, 0).await.gap_at,
        Some(15),
    );
    handle.feed.shutdown_and_wait().await.unwrap();
}

// Removing the pre-J flush stages a certificate despite unflushed events;
// removing the post-J flush acknowledges a certificate that is not durable.
#[tokio::test]
async fn checkpoint_real_adapter_flush_failures_and_j_set_fail_closed() {
    for fail_flush_at in [1, 2, 0] {
        let backend = Arc::new(BoundaryStore {
            inner: Arc::new(InMemoryStore::new()),
            fail_set: AtomicBool::new(false),
            fail_flush: AtomicBool::new(false),
            fail_j: AtomicBool::new(fail_flush_at == 0),
            fail_flush_at: AtomicUsize::new(fail_flush_at),
            sets: AtomicUsize::new(0),
            flushes: AtomicUsize::new(0),
        });
        let store: Arc<dyn ChangelogStore> = Arc::new(StoreChangelog::new(backend.clone()));
        let feed = RepoChangefeed::new(store);
        assert!(feed.record_clean_horizon(12).await.is_err());
        assert_eq!(feed.clean_horizon(), 0);
        assert_eq!(
            backend.sets.load(Ordering::Acquire),
            usize::from(fail_flush_at != 1),
        );
        assert_eq!(
            backend.flushes.load(Ordering::Acquire),
            fail_flush_at.max(1)
        );
        feed.shutdown_and_wait().await.unwrap();
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
            fail_j: AtomicBool::new(false),
            fail_flush_at: AtomicUsize::new(0),
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
        feed.shutdown_and_wait().await.unwrap();
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
    handle.feed.shutdown_and_wait().await.unwrap();
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
    handle.feed.shutdown_and_wait().await.unwrap();
}
