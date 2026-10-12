//! FOLLOWER-DURABILITY: adversarial explicit durability boundaries.

use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use futures::{Stream, StreamExt};
use shamir_storage::error::{DbError, DbResult};
use shamir_storage::storage_in_memory::{InMemoryRepo, InMemoryStore};
use shamir_storage::types::{RecordKey, Store};
use shamir_tx::{ChangeOp, ChangelogEvent, MvccStore, RecordChange};
use shamir_types::access::Actor;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;
use tokio::sync::Notify;

use crate::meta::recovery_marker::load_last_committed;
use crate::meta::{MetaEnvelope, MetaKey};
use crate::repo::{BoxRepo, RepoInstance};
use crate::table::table_manager::table_token_for;
use crate::table::TableConfig;
use crate::tx::apply_replicated::{apply_batch_with_info, apply_replicated_batch};
use crate::tx::{ApplyOutcome, ApplyPolicy, ReplicatedEvent};

type TestStream = Pin<Box<dyn Stream<Item = DbResult<Vec<(RecordKey, Bytes)>>> + Send>>;

struct RecordingHistory {
    inner: Arc<dyn Store>,
    info: Arc<dyn Store>,
    flushes: AtomicUsize,
    fail_flush: AtomicBool,
}

#[async_trait]
impl Store for RecordingHistory {
    async fn insert(&self, value: Bytes) -> DbResult<RecordKey> {
        self.inner.insert(value).await
    }

    async fn set(&self, key: RecordKey, value: Bytes) -> DbResult<bool> {
        // Negative control: moving the floor after writes fails here even if
        // every explicit flush succeeds. Unflushed history can survive too.
        assert!(load_last_committed(&self.info).await?.unwrap_or(0) >= 2);
        self.inner.set(key, value).await
    }

    async fn get(&self, key: RecordKey) -> DbResult<Bytes> {
        self.inner.get(key).await
    }

    async fn remove(&self, key: RecordKey) -> DbResult<bool> {
        self.inner.remove(key).await
    }

    async fn flush(&self) -> DbResult<()> {
        // The covering bookmark must not even be set until history is flushed.
        assert_eq!(
            crate::meta::repl_bookmark::load_replication_bookmark(&self.info).await?,
            0,
        );
        self.flushes.fetch_add(1, Ordering::AcqRel);
        if self.fail_flush.load(Ordering::Acquire) {
            return Err(DbError::Storage("injected history flush failure".into()));
        }
        self.inner.flush().await
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

fn repo() -> RepoInstance {
    RepoInstance::new(
        "follower".into(),
        BoxRepo::InMemory(Arc::new(InMemoryRepo::new())),
        vec![TableConfig::new("items")],
    )
}

fn events() -> Vec<ReplicatedEvent> {
    (1..=2u8)
        .map(|id| ReplicatedEvent {
            event: ChangelogEvent {
                repo: "leader".into(),
                commit_version: u64::from(id),
                tx_id: u64::from(id),
                actor: Actor::System,
                timestamp_ns: 0,
                changes: vec![RecordChange {
                    table: "items".into(),
                    key: RecordId([id; 16]).to_bytes(),
                    op: ChangeOp::Put,
                    value: Some(InnerValue::Str("row".into()).to_bytes().unwrap()),
                }],
            },
            field_bindings: Vec::new(),
        })
        .collect()
}

async fn fixture(fail: bool) -> (RepoInstance, Arc<RecordingHistory>, Arc<MvccStore>) {
    let repo = repo();
    repo.get_table("items").await.unwrap();
    let gate = repo.tx_gate().await.unwrap();
    let history = Arc::new(RecordingHistory {
        inner: Arc::new(InMemoryStore::new()),
        info: repo.tx_info_store().await.unwrap(),
        flushes: AtomicUsize::new(0),
        fail_flush: AtomicBool::new(fail),
    });
    let mvcc = Arc::new(MvccStore::new(history.clone(), gate));
    let token = table_token_for("items");
    assert!(repo.per_table_mvcc().remove_sync(&token).is_some());
    assert!(repo
        .per_table_mvcc()
        .insert_sync(token, mvcc.clone())
        .is_ok());
    (repo, history, mvcc)
}

// Behavioral counterfactual: omitting history flush or reserving after writes
// violates the recording barriers. This batch API did not exist on baseline.
#[tokio::test]
async fn batch_reserves_floor_then_flushes_history_once_before_bookmark() {
    let (repo, history, mvcc) = fixture(false).await;
    assert_eq!(
        apply_replicated_batch(&repo, &events(), 0, ApplyPolicy::Trusted)
            .await
            .unwrap(),
        ApplyOutcome::Applied { local_version: 2 },
    );
    assert_eq!(history.flushes.load(Ordering::Acquire), 1);
    assert_eq!(repo.replication_bookmark().await.unwrap(), 2);
    assert_eq!(load_last_committed(&history.info).await.unwrap(), Some(2));
    for id in 1..=2 {
        assert!(mvcc
            .get_current_bytes(&RecordId([id; 16]).to_bytes())
            .await
            .unwrap()
            .is_some());
    }
    assert_eq!(repo.tx_gate().await.unwrap().durable_watermark(), 2);
}

// Behavioral counterfactual: treating a failed flush as success advances
// durability and returns Applied instead of preserving the old bookmark.
#[tokio::test]
async fn published_batch_flush_failure_stops_and_keeps_old_bookmark() {
    let (repo, history, mvcc) = fixture(true).await;
    let error = apply_replicated_batch(&repo, &events(), 0, ApplyPolicy::Trusted)
        .await
        .unwrap_err();
    assert!(matches!(error, DbError::RepoRequiresReopen(_)), "{error:?}");
    let gate = repo.tx_gate().await.unwrap();
    assert!(gate.requires_reopen());
    assert_eq!(gate.last_committed(), 2);
    assert_eq!(gate.durable_watermark(), 0);
    assert_eq!(repo.replication_bookmark().await.unwrap(), 0);
    assert_eq!(load_last_committed(&history.info).await.unwrap(), Some(2));
    assert!(mvcc
        .get_current_bytes(&RecordId([1; 16]).to_bytes())
        .await
        .unwrap()
        .is_some());
    let error = apply_replicated_batch(&repo, &events(), 0, ApplyPolicy::Trusted)
        .await
        .unwrap_err();
    assert!(matches!(error, DbError::RepoRequiresReopen(_)), "{error:?}");
    assert_eq!(gate.version_allocation_high_water_mark(), 2);
}

// Red before fix: marker errors were best-effort and Applied escaped.
#[tokio::test]
async fn marker_failure_fail_stops_before_history_can_survive() {
    let repo = repo();
    repo.get_table("items").await.unwrap();
    let gate = repo.tx_gate().await.unwrap();
    repo.tx_info_store()
        .await
        .unwrap()
        .set(
            RecordKey::from_slice(MetaKey::LastCommittedVersion.as_record_id().as_bytes()),
            Bytes::from_static(b"corrupt"),
        )
        .await
        .unwrap();
    let event = events().remove(0);
    let error = repo
        .apply_replicated(&event.event, &event.field_bindings, 0, ApplyPolicy::Trusted)
        .await
        .unwrap_err();
    assert!(matches!(error, DbError::RepoRequiresReopen(_)), "{error:?}");
    assert!(gate.requires_reopen());
    assert_eq!(gate.durable_watermark(), 0);
    assert_eq!(repo.replication_bookmark().await.unwrap(), 0);
    assert_eq!(
        repo.get_table("items")
            .await
            .unwrap()
            .get_bytes(RecordId([1; 16]))
            .await
            .unwrap(),
        None,
    );
}

// Red before fix: corrupt marker decoded as absent and gate seeded at zero.
#[tokio::test]
async fn corrupt_marker_refuses_gate_initialization() {
    let repo = repo();
    repo.tx_info_store()
        .await
        .unwrap()
        .set(
            RecordKey::from_slice(MetaKey::LastCommittedVersion.as_record_id().as_bytes()),
            Bytes::from_static(b"corrupt"),
        )
        .await
        .unwrap();
    assert!(repo.tx_gate().await.is_err());
}

// Covers the other gate seed: no silent fallback to tx id 1.
#[tokio::test]
async fn corrupt_tx_id_refuses_gate_initialization() {
    let repo = repo();
    let info = repo.tx_info_store().await.unwrap();
    info.set(
        RecordKey::from_slice(MetaKey::LastCommittedVersion.as_record_id().as_bytes()),
        Bytes::from(MetaEnvelope::new(8u64).encode().unwrap()),
    )
    .await
    .unwrap();
    info.set(
        RecordKey::from_slice(MetaKey::NextTxId.as_record_id().as_bytes()),
        Bytes::from_static(b"corrupt"),
    )
    .await
    .unwrap();
    assert!(repo.tx_gate().await.is_err());
}

struct CrashInfo {
    inner: Arc<dyn Store>,
    durable: Arc<dyn Store>,
    history: Arc<RecordingHistory>,
    flushes: AtomicUsize,
    fail_floor: bool,
    park_bookmark: bool,
    park_set: bool,
    reached: Notify,
    resume: Notify,
}

async fn copy_store(source: &Arc<dyn Store>, destination: &Arc<dyn Store>) {
    let mut stream = source.iter_stream(100);
    while let Some(batch) = stream.next().await {
        for (key, value) in batch.unwrap() {
            destination.set(key, value).await.unwrap();
        }
    }
}

#[async_trait]
impl Store for CrashInfo {
    async fn insert(&self, value: Bytes) -> DbResult<RecordKey> {
        self.inner.insert(value).await
    }

    async fn set(&self, key: RecordKey, value: Bytes) -> DbResult<bool> {
        if self.park_set
            && key == RecordKey::from_slice(MetaKey::ReplicationBookmark.as_record_id().as_bytes())
        {
            assert_eq!(self.history.flushes.load(Ordering::Acquire), 1);
            self.reached.notify_one();
            self.resume.notified().await;
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
        let number = self.flushes.fetch_add(1, Ordering::AcqRel) + 1;
        if number == 1 && self.fail_floor {
            return Err(DbError::Storage("floor flush failed".into()));
        }
        if number == 2 {
            assert_eq!(self.history.flushes.load(Ordering::Acquire), 1);
            if self.park_bookmark {
                self.reached.notify_one();
                self.resume.notified().await;
            }
        }
        copy_store(&self.inner, &self.durable).await;
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

async fn crash_info(history: Arc<RecordingHistory>, fail: bool, park_set: bool) -> Arc<CrashInfo> {
    Arc::new(CrashInfo {
        inner: Arc::clone(&history.info),
        durable: Arc::new(InMemoryStore::new()),
        history,
        flushes: AtomicUsize::new(0),
        fail_floor: fail,
        park_bookmark: !fail && !park_set,
        park_set,
        reached: Notify::new(),
        resume: Notify::new(),
    })
}

// Counterfactual: a bookmark set without flush survives in the live view but
// not in the crash image. Removing history flush trips the recording barrier.
#[tokio::test]
async fn crash_between_bookmark_set_and_flush_keeps_old_cursor_and_converges() {
    crash_and_converge(false).await;
}

#[tokio::test]
async fn crash_after_data_flush_before_bookmark_set_replays_old_cursor() {
    crash_and_converge(true).await;
}

async fn crash_and_converge(park_set: bool) {
    let (repo, history, _) = fixture(false).await;
    let info = crash_info(history.clone(), false, park_set).await;
    let store: Arc<dyn Store> = info.clone();
    let events = events();
    {
        let apply = apply_batch_with_info(&repo, &events, 0, ApplyPolicy::Trusted, &store);
        tokio::pin!(apply);
        tokio::select! {
            biased;
            _ = info.reached.notified() => {}
            result = &mut apply => panic!("batch escaped bookmark barrier: {result:?}"),
        }
        assert_eq!(
            crate::meta::repl_bookmark::load_replication_bookmark(&info.inner)
                .await
                .unwrap(),
            if park_set { 0 } else { 2 },
        );
        assert_eq!(
            crate::meta::repl_bookmark::load_replication_bookmark(&info.durable)
                .await
                .unwrap(),
            0,
        );
        assert_eq!(load_last_committed(&info.durable).await.unwrap(), Some(2));
        // Drop the borrowed, still-pending batch to simulate interruption.
    }
    assert!(repo.tx_gate().await.unwrap().requires_reopen());
    let reopened = super::follower_durability_tests::repo();
    let reopened_info = reopened.tx_info_store().await.unwrap();
    copy_store(&info.durable, &reopened_info).await;
    reopened.get_table("items").await.unwrap();
    let gate = reopened.tx_gate().await.unwrap();
    assert_eq!(gate.version_allocation_high_water_mark(), 2);
    let token = table_token_for("items");
    let mvcc = Arc::new(MvccStore::new(history.inner.clone(), gate.clone()));
    reopened.per_table_mvcc().remove_sync(&token);
    assert!(reopened
        .per_table_mvcc()
        .insert_sync(token, mvcc.clone())
        .is_ok());
    let outcome = apply_replicated_batch(&reopened, &events, 0, ApplyPolicy::Trusted)
        .await
        .unwrap();
    assert_eq!(outcome, ApplyOutcome::Applied { local_version: 4 });
    assert_eq!(reopened.replication_bookmark().await.unwrap(), 2);
    for id in 1..=2 {
        assert!(mvcc
            .get_current_bytes(&RecordId([id; 16]).to_bytes())
            .await
            .unwrap()
            .is_some());
    }
    assert!(!gate.requires_reopen());
}

// Counterfactual: best-effort marker failures permit publication and Applied.
#[tokio::test]
async fn floor_flush_failure_is_terminal_and_publishes_no_history() {
    let (repo, history, mvcc) = fixture(false).await;
    let info = crash_info(history.clone(), true, false).await;
    let store: Arc<dyn Store> = info.clone();
    let error = apply_batch_with_info(&repo, &events(), 0, ApplyPolicy::Trusted, &store)
        .await
        .unwrap_err();
    assert!(matches!(error, DbError::RepoRequiresReopen(_)), "{error:?}");
    let gate = repo.tx_gate().await.unwrap();
    assert!(gate.requires_reopen());
    assert_eq!(gate.last_committed(), 0);
    assert_eq!(load_last_committed(&info.durable).await.unwrap(), None);
    assert_eq!(repo.replication_bookmark().await.unwrap(), 0);
    assert_eq!(
        mvcc.get_current_bytes(&RecordId([1; 16]).to_bytes())
            .await
            .unwrap(),
        None,
    );
}

// Behavioral counterfactual: a delayed normal commit tail must not overwrite
// the reserved floor or a larger transaction-id snapshot with its lower view.
#[tokio::test]
async fn normal_commit_tail_preserves_reserved_floor_and_transaction_id() {
    let repo = repo();
    let gate = repo.tx_gate().await.unwrap();
    let info = repo.tx_info_store().await.unwrap();
    crate::meta::recovery_marker::save_last_committed_flushed(&info, 12)
        .await
        .unwrap();
    crate::meta::recovery_marker::save_next_tx_id_snapshot(&info, 99)
        .await
        .unwrap();
    info.flush().await.unwrap();
    crate::tx::commit_phases::persist_markers(&repo, &gate, 0)
        .await
        .unwrap();
    assert_eq!(load_last_committed(&info).await.unwrap(), Some(12));
    assert_eq!(
        crate::meta::recovery_marker::load_next_tx_id_snapshot(&info)
            .await
            .unwrap(),
        Some(99),
    );
}

// Exercise public factory open, not merely lazy gate construction.
#[cfg(feature = "fjall")]
#[test]
fn corrupt_marker_refuses_factory_open() {
    let directory = tempfile::TempDir::new().unwrap();
    {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let seed = RepoInstance::from_factory(
                "follower".into(),
                crate::repo::BoxRepoFactory::fjall_raw(directory.path()),
                vec![TableConfig::new("items")],
            )
            .await
            .unwrap();
            let info = seed.tx_info_store().await.unwrap();
            info.set(
                RecordKey::from_slice(MetaKey::LastCommittedVersion.as_record_id().as_bytes()),
                Bytes::from_static(b"corrupt"),
            )
            .await
            .unwrap();
            info.flush().await.unwrap();
        });
    }
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(async {
        let result = RepoInstance::from_factory(
            "follower".into(),
            crate::repo::BoxRepoFactory::fjall_raw(directory.path()),
            vec![TableConfig::new("items")],
        )
        .await;
        assert!(result.is_err(), "factory must propagate marker corruption");
    });
}
