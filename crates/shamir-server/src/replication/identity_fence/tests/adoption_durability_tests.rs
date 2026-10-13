//! Failed/cancelled adoption is not durable merely because it is readable.

use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use futures_util::stream::Stream;
use shamir_db::engine::tx::apply_replicated::apply_replicated_batch;
use shamir_db::engine::tx::{ApplyOutcome, ApplyPolicy, ReplicatedEvent};
use shamir_db::storage::error::{DbError, DbResult};
use shamir_db::storage::storage_in_memory::InMemoryStore;
use shamir_db::storage::types::{RecordKey, Store};
use shamir_query_types::wire::repl::ReplResponse;
use tokio::sync::Notify;

use super::identity_fence_tests::{build_memory, repo, write_rows};
use shamir_db::ShamirDb;
use tokio_util::sync::CancellationToken;

use crate::replication::error::ReplError;
use crate::replication::follower_loop::{
    run_follower_loop_with_identity_store, FollowerLoopConfig, IdentityStoreOverride,
};
use crate::replication::identity_fence::{checks, recorded};
use crate::replication::in_process::InProcessReplSource;
use crate::replication::source::ReplSource;

type TestStream = Pin<Box<dyn Stream<Item = DbResult<Vec<(RecordKey, Bytes)>>> + Send>>;

struct AdoptionStore {
    inner: Arc<dyn Store>,
    durable: AtomicBool,
    flushes: AtomicUsize,
    park: bool,
    reached: Notify,
}

#[async_trait]
impl Store for AdoptionStore {
    async fn insert(&self, value: Bytes) -> DbResult<RecordKey> {
        self.inner.insert(value).await
    }

    async fn set(&self, key: RecordKey, value: Bytes) -> DbResult<bool> {
        self.durable.store(false, Ordering::Release);
        self.inner.set(key, value).await
    }

    async fn get(&self, key: RecordKey) -> DbResult<Bytes> {
        self.inner.get(key).await
    }

    async fn remove(&self, key: RecordKey) -> DbResult<bool> {
        self.inner.remove(key).await
    }

    async fn flush(&self) -> DbResult<()> {
        if self.flushes.fetch_add(1, Ordering::AcqRel) == 0 {
            if self.park {
                self.reached.notify_one();
                std::future::pending::<()>().await;
            }
            return Err(DbError::Storage("fail-once adoption flush".into()));
        }
        self.durable.store(true, Ordering::Release);
        Ok(())
    }

    fn iter_stream(&self, size: usize) -> TestStream {
        self.inner.iter_stream(size)
    }

    fn scan_prefix_stream(&self, prefix: Bytes, size: usize) -> TestStream {
        self.inner.scan_prefix_stream(prefix, size)
    }

    fn iter_range_stream(
        &self,
        start: Option<Bytes>,
        end: Option<Bytes>,
        size: usize,
    ) -> TestStream {
        self.inner.iter_range_stream(start, end, size)
    }

    fn iter_range_stream_reverse(
        &self,
        start: Option<Bytes>,
        end: Option<Bytes>,
        size: usize,
    ) -> TestStream {
        self.inner.iter_range_stream_reverse(start, end, size)
    }
}

// Explicit negative-control seam: omit exactly the production startup barrier,
// not the identity comparison. Matching retry then wrongly trusts the live set.
async fn retry_contact(info: &Arc<dyn Store>, pull: &ReplResponse, reflush: bool) {
    if reflush {
        checks::prepare_existing(info).await.unwrap();
    }
    checks::fence_pull_identity(info, pull, None).await.unwrap();
}

#[tokio::test]
async fn failed_first_contact_retry_flushes_before_any_events() {
    full_loop_adoption_retry(true).await;
}

#[tokio::test]
async fn helper_scoped_cancelled_first_contact_retry_flushes_before_any_events() {
    adoption_retry(true, true).await;
}

#[tokio::test]
async fn startup_reflush_disabled_matching_retry_proceeds_without_durability() {
    full_loop_adoption_retry(false).await;
}

struct DurabilitySource {
    inner: InProcessReplSource,
    probe: Arc<AdoptionStore>,
    follower: ShamirDb,
    expected_durable: bool,
    hellos: AtomicUsize,
    pulls: AtomicUsize,
}

#[async_trait]
impl ReplSource for DurabilitySource {
    async fn hello(&self, node_id: &str) -> Result<ReplResponse, ReplError> {
        self.hellos.fetch_add(1, Ordering::AcqRel);
        self.inner.hello(node_id).await
    }

    async fn pull(
        &self,
        db: &str,
        repo_name: &str,
        from_version: u64,
        limit: u32,
        wait_ms: Option<u32>,
    ) -> Result<ReplResponse, ReplError> {
        // This hook runs inside the actual loop, before fetching or applying
        // its first batch. The negative control observes the unsafe ordering.
        assert_eq!(
            self.probe.durable.load(Ordering::Acquire),
            self.expected_durable
        );
        assert_eq!(
            self.probe.flushes.load(Ordering::Acquire),
            if self.expected_durable { 2 } else { 1 }
        );
        assert_eq!(from_version, 1);
        assert_eq!(
            repo(&self.follower).replication_bookmark().await.unwrap(),
            0
        );
        assert!(repo(&self.follower)
            .read_changelog_bounded(0, 1000)
            .await
            .unwrap()
            .read
            .events
            .is_empty());
        self.pulls.fetch_add(1, Ordering::AcqRel);
        self.inner
            .pull(db, repo_name, from_version, limit, wait_ms)
            .await
    }
}

async fn full_loop_adoption_retry(reflush: bool) {
    let leader = build_memory().await;
    write_rows(&leader, 0, 2).await;
    let follower = build_memory().await;
    let probe = Arc::new(AdoptionStore {
        inner: Arc::new(InMemoryStore::new()),
        durable: AtomicBool::new(false),
        flushes: AtomicUsize::new(0),
        park: false,
        reached: Notify::new(),
    });
    let info: Arc<dyn Store> = probe.clone();
    let source = Arc::new(DurabilitySource {
        inner: InProcessReplSource::new(leader),
        probe: probe.clone(),
        follower: follower.clone(),
        expected_durable: reflush,
        hellos: AtomicUsize::new(0),
        pulls: AtomicUsize::new(0),
    });
    let cfg = FollowerLoopConfig::new("adoption-test", "app", "main")
        .with_poll_wait_ms(0)
        .with_max_iterations(1);
    let before = repo(&follower).current_commit_version().await.unwrap();
    let first = run_follower_loop_with_identity_store(
        Arc::new(follower.clone()),
        source.clone(),
        cfg.clone(),
        CancellationToken::new(),
        IdentityStoreOverride {
            info: info.clone(),
            startup_reflush: reflush,
        },
    )
    .await;
    assert!(matches!(first, Err(ReplError::Bookmark(_))));
    assert_eq!(source.hellos.load(Ordering::Acquire), 1);
    assert_eq!(source.pulls.load(Ordering::Acquire), 0);
    assert!(recorded::load(&info).await.unwrap().is_some());
    assert!(!probe.durable.load(Ordering::Acquire));
    assert_eq!(probe.flushes.load(Ordering::Acquire), 1);
    assert_eq!(
        repo(&follower).current_commit_version().await.unwrap(),
        before
    );
    assert_eq!(repo(&follower).replication_bookmark().await.unwrap(), 0);
    assert!(repo(&follower)
        .read_changelog_bounded(0, 1000)
        .await
        .unwrap()
        .read
        .events
        .is_empty());

    run_follower_loop_with_identity_store(
        Arc::new(follower.clone()),
        source.clone(),
        cfg,
        CancellationToken::new(),
        IdentityStoreOverride {
            info: info.clone(),
            startup_reflush: reflush,
        },
    )
    .await
    .unwrap();
    assert_eq!(source.hellos.load(Ordering::Acquire), 2);
    assert_eq!(source.pulls.load(Ordering::Acquire), 1);
    assert!(repo(&follower).replication_bookmark().await.unwrap() > 0);
    assert_eq!(
        repo(&follower)
            .read_changelog_bounded(0, 1000)
            .await
            .unwrap()
            .read
            .events
            .len(),
        2
    );
    assert_eq!(probe.durable.load(Ordering::Acquire), reflush);
    assert_eq!(
        probe.flushes.load(Ordering::Acquire),
        if reflush { 2 } else { 1 }
    );
    // Neither Hello nor Pull may silently fall back to the real info store.
    let real_info = repo(&follower).tx_info_store().await.unwrap();
    assert!(recorded::load(&real_info).await.unwrap().is_none());
}

// Helper-scoped cancellation coverage only; full-loop failure/retry is above.
async fn adoption_retry(park: bool, reflush: bool) {
    let leader = build_memory().await;
    write_rows(&leader, 0, 2).await;
    let follower = build_memory().await;
    let pull = InProcessReplSource::new(leader)
        .pull("app", "main", 1, 1000, Some(0))
        .await
        .unwrap();
    let probe = Arc::new(AdoptionStore {
        inner: Arc::new(InMemoryStore::new()),
        durable: AtomicBool::new(false),
        flushes: AtomicUsize::new(0),
        park,
        reached: Notify::new(),
    });
    let info: Arc<dyn Store> = probe.clone();
    let before = repo(&follower).current_commit_version().await.unwrap();
    checks::prepare_existing(&info).await.unwrap();
    if park {
        // Poll to the known post-set flush barrier, then drop the borrowed
        // contact future. No spawned task or timing-dependent cancellation.
        let contact = checks::fence_pull_identity(&info, &pull, None);
        tokio::pin!(contact);
        tokio::select! {
            biased;
            _ = probe.reached.notified() => {}
            result = &mut contact => panic!("contact escaped barrier: {result:?}"),
        }
    } else {
        assert!(matches!(
            checks::fence_pull_identity(&info, &pull, None).await,
            Err(crate::replication::ReplError::Bookmark(_))
        ));
    }
    assert!(recorded::load(&info).await.unwrap().is_some());
    assert!(!probe.durable.load(Ordering::Acquire));
    assert_eq!(probe.flushes.load(Ordering::Acquire), 1);
    assert_eq!(
        repo(&follower).current_commit_version().await.unwrap(),
        before
    );
    assert_eq!(repo(&follower).replication_bookmark().await.unwrap(), 0);
    assert!(repo(&follower)
        .read_changelog_bounded(0, 1000)
        .await
        .unwrap()
        .read
        .events
        .is_empty());

    retry_contact(&info, &pull, reflush).await;
    // Observe durability BEFORE allowing the actual payload to be applied.
    assert_eq!(probe.durable.load(Ordering::Acquire), reflush);
    assert_eq!(
        probe.flushes.load(Ordering::Acquire),
        if reflush { 2 } else { 1 }
    );
    let ReplResponse::Pull { events, .. } = pull else {
        panic!("expected pull");
    };
    let events: Vec<ReplicatedEvent> = rmp_serde::from_slice(&events).unwrap();
    assert_eq!(events.len(), 2);
    let outcome = apply_replicated_batch(&repo(&follower), &events, 0, ApplyPolicy::Trusted)
        .await
        .unwrap();
    assert!(matches!(outcome, ApplyOutcome::Applied { .. }));
    assert!(repo(&follower).replication_bookmark().await.unwrap() > 0);
    // Matching responses within the prepared loop do NOT flush per Pull.
    let response = ReplResponse::Pull {
        leader_epoch: 1,
        events: Vec::new(),
        gap_at: None,
        current_version: 0,
        repo_lineage_id: Some(recorded::load(&info).await.unwrap().unwrap().lineage_id),
        history_epoch: Some(0),
        schema_digest: None,
        gap_first: None,
        gap_through: None,
    };
    checks::fence_pull_identity(&info, &response, None)
        .await
        .unwrap();
    assert_eq!(
        probe.flushes.load(Ordering::Acquire),
        if reflush { 2 } else { 1 }
    );
}
