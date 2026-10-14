//! PR8 `completed_reseed` chunk 2 — lifecycle record (armed / clean
//! certificate) and the cold-start history-epoch bump.
//!
//! Record-level (non-crash) tests use `InMemoryStore` exactly like the
//! sibling `repl_bookmark_tests`; "reopen" there is a fresh decode of the
//! same store value. The FLUSH-LOSS negative controls below run against a
//! local pending/durable store fake (`FlushStore`) with REAL flush
//! semantics: unflushed writes are invisible to `crash_image()`. The
//! remaining CRASH/reopen tests use a DISK-backed fjall fixture and destroy
//! the whole tokio runtime between opens (Windows fjall lock release).

use crate::meta::repo_identity::RepoIdentity;
use crate::meta::repo_identity::{
    ensure_armed, load_state, mark_clean, requires_unclean_bump, LifecycleState,
    SKIP_ARMING_FLUSH_FOR_TEST, SKIP_BUMP_FLUSH_FOR_TEST, SKIP_CLEAN_FLUSH_FOR_TEST,
    SKIP_UNCLEAN_EPOCH_BUMP_FOR_TEST,
};
use crate::repo::durability_policy::{
    DurabilityPolicy, ReplicationServe, RetentionOverflow, WalCommitTier,
    DEFAULT_WAL_RETENTION_MAX_BYTES,
};
use crate::repo::{BoxRepo, RepoInstance};
use crate::table::TableConfig;
use async_trait::async_trait;
use bytes::Bytes;
use shamir_storage::error::{DbError, DbResult};
use shamir_storage::storage_in_memory::{InMemoryRepo, InMemoryStore};
use shamir_storage::types::{RecordKey, Store};
use std::collections::BTreeMap;
use std::pin::Pin;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

fn mem_store() -> Arc<dyn Store> {
    Arc::new(InMemoryStore::new())
}

type TestStream = Pin<Box<dyn futures::Stream<Item = DbResult<Vec<(RecordKey, Bytes)>>> + Send>>;

/// Local pending/durable store fake with REAL flush semantics: `set` writes
/// ONLY into `pending`; `flush()` copies pending into durable; `crash()`
/// simulates process death by discarding pending; `crash_image()` returns a
/// snapshot over ONLY the durable map (unflushed writes are lost). Models
/// the `DiskImage`/`SnapshotStore` pair of `crash_image_tests.rs` at the
/// record layer, without a fjall dependency.
#[derive(Default)]
struct FlushStore {
    pending: Mutex<BTreeMap<RecordKey, Bytes>>,
    durable: Mutex<BTreeMap<RecordKey, Bytes>>,
}

impl FlushStore {
    /// Process death: the unflushed write buffer is lost.
    fn crash(&self) {
        *self.pending.lock().unwrap() = self.durable.lock().unwrap().clone();
    }

    /// Everything an unclean reopen would find: ONLY flushed bytes.
    fn crash_image(&self) -> Arc<dyn Store> {
        Arc::new(DurableSnapshot {
            records: Mutex::new(self.durable.lock().unwrap().clone()),
        })
    }
}

/// Immutable snapshot of one durable state.
struct DurableSnapshot {
    records: Mutex<BTreeMap<RecordKey, Bytes>>,
}

#[async_trait]
impl Store for DurableSnapshot {
    async fn insert(&self, value: Bytes) -> DbResult<RecordKey> {
        let key = RecordKey::from_slice(&(self.records.lock().unwrap().len() as u64).to_be_bytes());
        self.set(key.clone(), value).await?;
        Ok(key)
    }
    async fn set(&self, key: RecordKey, value: Bytes) -> DbResult<bool> {
        Ok(self.records.lock().unwrap().insert(key, value).is_none())
    }
    async fn get(&self, key: RecordKey) -> DbResult<Bytes> {
        self.records
            .lock()
            .unwrap()
            .get(&key)
            .cloned()
            .ok_or_else(|| DbError::NotFound("fake store key".into()))
    }
    async fn remove(&self, key: RecordKey) -> DbResult<bool> {
        Ok(self.records.lock().unwrap().remove(&key).is_some())
    }
    fn iter_stream(&self, _batch_size: usize) -> TestStream {
        Box::pin(futures::stream::empty())
    }
    fn scan_prefix_stream(&self, _prefix: Bytes, _batch_size: usize) -> TestStream {
        Box::pin(futures::stream::empty())
    }
    fn iter_range_stream(
        &self,
        _start: Option<Bytes>,
        _end: Option<Bytes>,
        _batch_size: usize,
    ) -> TestStream {
        Box::pin(futures::stream::empty())
    }
    fn iter_range_stream_reverse(
        &self,
        _start: Option<Bytes>,
        _end: Option<Bytes>,
        _batch_size: usize,
    ) -> TestStream {
        Box::pin(futures::stream::empty())
    }
}

#[async_trait]
impl Store for FlushStore {
    async fn insert(&self, value: Bytes) -> DbResult<RecordKey> {
        let key = RecordKey::from_slice(&(self.pending.lock().unwrap().len() as u64).to_be_bytes());
        self.set(key.clone(), value).await?;
        Ok(key)
    }
    async fn set(&self, key: RecordKey, value: Bytes) -> DbResult<bool> {
        Ok(self.pending.lock().unwrap().insert(key, value).is_none())
    }
    async fn get(&self, key: RecordKey) -> DbResult<Bytes> {
        self.pending
            .lock()
            .unwrap()
            .get(&key)
            .cloned()
            .or_else(|| self.durable.lock().unwrap().get(&key).cloned())
            .ok_or_else(|| DbError::NotFound("fake store key".into()))
    }
    async fn remove(&self, key: RecordKey) -> DbResult<bool> {
        let in_pending = self.pending.lock().unwrap().remove(&key).is_some();
        Ok(in_pending | self.durable.lock().unwrap().remove(&key).is_some())
    }
    async fn flush(&self) -> DbResult<()> {
        *self.durable.lock().unwrap() = self.pending.lock().unwrap().clone();
        Ok(())
    }
    fn iter_stream(&self, _batch_size: usize) -> TestStream {
        Box::pin(futures::stream::empty())
    }
    fn scan_prefix_stream(&self, _prefix: Bytes, _batch_size: usize) -> TestStream {
        Box::pin(futures::stream::empty())
    }
    fn iter_range_stream(
        &self,
        _start: Option<Bytes>,
        _end: Option<Bytes>,
        _batch_size: usize,
    ) -> TestStream {
        Box::pin(futures::stream::empty())
    }
    fn iter_range_stream_reverse(
        &self,
        _start: Option<Bytes>,
        _end: Option<Bytes>,
        _batch_size: usize,
    ) -> TestStream {
        Box::pin(futures::stream::empty())
    }
}

/// Repo whose effective serve mode is CompletedReseed (in-memory backend —
/// the mode is explicitly allowed on ephemeral backends).
fn reseed_repo(name: &str) -> RepoInstance {
    let repo = RepoInstance::new(
        name.into(),
        BoxRepo::InMemory(Arc::new(InMemoryRepo::new())),
        vec![TableConfig::new("items")],
    );
    repo.set_durability(DurabilityPolicy::new(
        WalCommitTier::Buffered,
        DEFAULT_WAL_RETENTION_MAX_BYTES,
        RetentionOverflow::RefuseWrites,
        ReplicationServe::CompletedReseed,
    ))
    .unwrap();
    repo
}

/// (a) Arming is idempotent (writing Armed twice leaves one Armed state)
/// and the record survives a reload of the same store.
#[tokio::test]
async fn armed_is_idempotent_and_survives_reload() {
    let s = mem_store();
    assert_eq!(load_state(&s).await.unwrap(), None);
    ensure_armed(&s).await.unwrap();
    ensure_armed(&s).await.unwrap();
    // Fresh "reopen": decode the stored value again.
    assert_eq!(load_state(&s).await.unwrap(), Some(LifecycleState::Armed));
}

/// (b) mark_clean writes Clean; ensure_armed over a stale Clean re-arms
/// (the obligation returns).
#[tokio::test]
async fn clean_then_rearm() {
    let s = mem_store();
    mark_clean(&s).await.unwrap();
    assert_eq!(load_state(&s).await.unwrap(), Some(LifecycleState::Clean));
    ensure_armed(&s).await.unwrap();
    assert_eq!(load_state(&s).await.unwrap(), Some(LifecycleState::Armed));
}

/// (c) Pure decision truth table: the bump decision depends ONLY on the
/// record (Armed → true; Clean / None → false) — the CURRENT serve mode is
/// deliberately not an input (review finding 2a).
#[test]
fn requires_unclean_bump_truth_table() {
    assert!(requires_unclean_bump(Some(LifecycleState::Armed)));
    assert!(!requires_unclean_bump(Some(LifecycleState::Clean)));
    assert!(!requires_unclean_bump(None));
}

/// (d) Repo-level: arming writes Armed into the repo's info store; the
/// recovery decision on the reloaded record is "bump"; the negative-control
/// seam skips the bump; the real hook bumps exactly once per open and
/// counts it in the durability metrics.
#[tokio::test]
async fn repo_level_arm_and_unclean_bump() {
    SKIP_UNCLEAN_EPOCH_BUMP_FOR_TEST.store(false, Ordering::Release);
    let repo = reseed_repo("lifecycle_repo_level");
    repo.ensure_replication_armed().await.unwrap();
    let store = repo.tx_info_store().await.unwrap();
    let state = load_state(&store).await.unwrap().unwrap();
    assert_eq!(state, LifecycleState::Armed);
    assert!(requires_unclean_bump(Some(state)));
    let epoch_before = repo.repo_identity().await.unwrap().history_epoch;
    let bumps_before = repo.durability_metrics_snapshot().counters[7];

    // Negative control: the seam skips the bump entirely.
    SKIP_UNCLEAN_EPOCH_BUMP_FOR_TEST.store(true, Ordering::Release);
    repo.note_unclean_open_bump().await.unwrap();
    assert_eq!(
        repo.repo_identity().await.unwrap().history_epoch,
        epoch_before
    );
    assert_eq!(repo.durability_metrics_snapshot().counters[7], bumps_before);

    // Real hook: exactly one bump, one metric increment, per open.
    SKIP_UNCLEAN_EPOCH_BUMP_FOR_TEST.store(false, Ordering::Release);
    repo.note_unclean_open_bump().await.unwrap();
    assert_eq!(
        repo.repo_identity().await.unwrap().history_epoch,
        epoch_before + 1
    );
    assert_eq!(
        repo.durability_metrics_snapshot().counters[7],
        bumps_before + 1
    );
}

/// The epoch bump itself is an atomic single-key save — reopening at any
/// point decodes either the old or the new epoch, never a torn record; after
/// a completed bump the epoch is exactly old + 1.
#[tokio::test]
async fn epoch_bump_is_atomic_single_key() {
    let s = mem_store();
    let identity = RepoIdentity::load_or_create(&s).await.unwrap();
    let old = identity.history_epoch;
    let bumped = RepoIdentity::bump(&s, None).await.unwrap();
    assert_eq!(bumped.history_epoch, old.checked_add(1).unwrap());
    // Reopen: the record decodes and the epoch is exactly old + 1.
    let reloaded = RepoIdentity::load_or_create(&s).await.unwrap();
    assert_eq!(reloaded.history_epoch, old + 1);
}

/// NEGATIVE CONTROL (arming flush): the protection must be load-bearing.
/// Default path: the arming flush survives an unclean end, so the reopen
/// reads Armed. With `SKIP_ARMING_FLUSH_FOR_TEST` set, the unflushed Armed
/// write is lost and the reopen reads NO record at all.
#[tokio::test]
async fn arming_flush_is_load_bearing_negative_control() {
    // Protection present: the arming flush survives process death.
    let live = Arc::new(FlushStore::default());
    let store: Arc<dyn Store> = live.clone();
    ensure_armed(&store).await.unwrap();
    assert_eq!(
        load_state(&live.crash_image()).await.unwrap(),
        Some(LifecycleState::Armed),
        "the arming flush must survive an unclean process end"
    );
    // Seam set: without the flush the Armed record never reaches disk.
    let live = Arc::new(FlushStore::default());
    let store: Arc<dyn Store> = live.clone();
    SKIP_ARMING_FLUSH_FOR_TEST.store(true, Ordering::Release);
    ensure_armed(&store).await.unwrap();
    SKIP_ARMING_FLUSH_FOR_TEST.store(false, Ordering::Release);
    assert_eq!(
        load_state(&live.crash_image()).await.unwrap(),
        None,
        "without the arming flush the Armed record must be lost"
    );
}

/// NEGATIVE CONTROL (clean-certificate flush): with
/// `SKIP_CLEAN_FLUSH_FOR_TEST`, `mark_clean`'s Clean write never reaches
/// disk, so the unclean reopen still sees Armed — and pays the unclean
/// bump. Proves the certificate flush is what closes the obligation.
#[tokio::test]
async fn clean_flush_is_load_bearing_negative_control() {
    // Seam: the Clean write is never flushed, so the durable record is
    // still Armed and the reopen must cost the unclean bump.
    let live = Arc::new(FlushStore::default());
    let store: Arc<dyn Store> = live.clone();
    ensure_armed(&store).await.unwrap();
    SKIP_CLEAN_FLUSH_FOR_TEST.store(true, Ordering::Release);
    mark_clean(&store).await.unwrap();
    SKIP_CLEAN_FLUSH_FOR_TEST.store(false, Ordering::Release);
    let state = load_state(&live.crash_image()).await.unwrap();
    assert_eq!(
        state,
        Some(LifecycleState::Armed),
        "the lost Clean certificate must leave the Armed obligation on disk"
    );
    assert!(
        requires_unclean_bump(state),
        "so the reopen must cost the unclean bump"
    );
    // Protection present: the flushed Clean certificate closes it.
    let live = Arc::new(FlushStore::default());
    let store: Arc<dyn Store> = live.clone();
    ensure_armed(&store).await.unwrap();
    mark_clean(&store).await.unwrap();
    let state = load_state(&live.crash_image()).await.unwrap();
    assert_eq!(state, Some(LifecycleState::Clean));
    assert!(!requires_unclean_bump(state));
}

/// CRASH BOUNDARY (iii): crash DURING the epoch bump, flush skipped
/// (`SKIP_BUMP_FLUSH_FOR_TEST`). The durable record stays Armed with the
/// OLD epoch, so each reopen re-derives the bump: old+1, then old+2 — a
/// torn/lost bump CONVERGES instead of corrupting.
#[tokio::test]
async fn crash_during_bump_flush_skipped_converges() {
    SKIP_BUMP_FLUSH_FOR_TEST.store(false, Ordering::Release);
    let live = Arc::new(FlushStore::default());
    let store: Arc<dyn Store> = live.clone();
    let before = RepoIdentity {
        lineage_id: [7; 16],
        history_epoch: 5,
    };
    RepoIdentity::save_record(&store, before, None)
        .await
        .unwrap();
    ensure_armed(&store).await.unwrap();
    // Crash DURING the bump: the bumped value lands in pending only.
    SKIP_BUMP_FLUSH_FOR_TEST.store(true, Ordering::Release);
    RepoIdentity::bump(&store, None).await.unwrap();
    SKIP_BUMP_FLUSH_FOR_TEST.store(false, Ordering::Release);
    live.crash();
    let image = live.crash_image();
    assert_eq!(
        RepoIdentity::load_or_create(&image).await.unwrap(),
        before,
        "the bumped value was lost with the unflushed write"
    );
    assert_eq!(
        load_state(&image).await.unwrap(),
        Some(LifecycleState::Armed),
        "the bump must NOT touch the lifecycle record"
    );
    // Reopen: Armed on the durable image → the hook re-derives the bump,
    // this time flushed, and Armed persists until a clean shutdown.
    for expected_bump in [1u64, 2] {
        RepoIdentity::bump(&store, None).await.unwrap();
        let image = live.crash_image();
        let identity = RepoIdentity::load_or_create(&image).await.unwrap();
        assert_eq!(identity.lineage_id, before.lineage_id);
        assert_eq!(
            identity.history_epoch,
            before.history_epoch + expected_bump,
            "the lost bump must be re-derived on reopen"
        );
        assert_eq!(
            load_state(&image).await.unwrap(),
            Some(LifecycleState::Armed),
            "the obligation persists until a clean shutdown"
        );
    }
}

// ---------------------------------------------------------------------------
// Disk-backed crash/reopen tests. Every "open" runs in its own current-thread
// tokio runtime that is DESTROYED at the end of the block, which releases the
// fjall file lock deterministically (no sleep retries) and simulates a real
// process end — including the unclean ones, which call no `shutdown_clean`.
// ---------------------------------------------------------------------------

#[cfg(feature = "fjall")]
mod disk_crash {
    use super::*;
    use std::future::Future;

    /// CompletedReseed policy on the fjall backend (Buffered commit tier).
    fn reseed_policy() -> DurabilityPolicy {
        DurabilityPolicy::new(
            WalCommitTier::Buffered,
            DEFAULT_WAL_RETENTION_MAX_BYTES,
            RetentionOverflow::RefuseWrites,
            ReplicationServe::CompletedReseed,
        )
    }

    /// Default `durable` serve mode, same durability tiers.
    fn durable_policy() -> DurabilityPolicy {
        DurabilityPolicy::new(
            WalCommitTier::Buffered,
            DEFAULT_WAL_RETENTION_MAX_BYTES,
            RetentionOverflow::RefuseWrites,
            ReplicationServe::Durable,
        )
    }

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }

    fn reseed_open(dir: &std::path::Path) -> impl Future<Output = DbResult<RepoInstance>> {
        RepoInstance::from_factory_with_durability(
            "main".into(),
            crate::repo::BoxRepoFactory::fjall_raw(dir),
            vec![],
            reseed_policy(),
        )
    }

    fn durable_open(dir: &std::path::Path) -> impl Future<Output = DbResult<RepoInstance>> {
        RepoInstance::from_factory_with_durability(
            "main".into(),
            crate::repo::BoxRepoFactory::fjall_raw(dir),
            vec![],
            durable_policy(),
        )
    }

    /// MODE SWITCH + UNCLEAN REOPEN (review finding 2a, replaces the vacuous
    /// in-memory `clean_then_rearm_survives_mode_switch`): an Armed record
    /// survives a switch to `durable` serving, and an UNCLEAN restart under
    /// the new durable mode STILL bumps the epoch (exactly old+1, lineage
    /// unchanged) — the served tail is gone regardless of the current mode.
    /// A second unclean reopen bumps again (+2): the obligation persists
    /// until a clean shutdown.
    #[test]
    fn armed_survives_mode_switch_and_unclean_durable_reopen_bumps() {
        SKIP_BUMP_FLUSH_FOR_TEST.store(false, Ordering::Release);
        SKIP_UNCLEAN_EPOCH_BUMP_FOR_TEST.store(false, Ordering::Release);
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().to_path_buf();
        let before = runtime().block_on(async {
            let repo = reseed_open(&path).await.unwrap();
            repo.ensure_replication_armed().await.unwrap();
            let store = repo.tx_info_store().await.unwrap();
            assert_eq!(
                load_state(&store).await.unwrap(),
                Some(LifecycleState::Armed)
            );
            // Switch the CURRENT mode to durable: the obligation must survive
            // the configuration change untouched.
            repo.set_durability(durable_policy()).unwrap();
            assert_eq!(
                load_state(&store).await.unwrap(),
                Some(LifecycleState::Armed),
                "switching to durable must not erase an armed obligation"
            );
            repo.repo_identity().await.unwrap()
        });
        // UNCLEAN drop: the runtime is destroyed with no shutdown_clean.
        runtime().block_on(async {
            let repo = durable_open(&path).await.unwrap();
            let identity = repo.repo_identity().await.unwrap();
            assert_eq!(
                identity.lineage_id, before.lineage_id,
                "the bump must NEVER change the lineage"
            );
            assert_eq!(
                identity.history_epoch,
                before.history_epoch + 1,
                "the Armed record must bump even though the CURRENT mode is durable"
            );
            let store = repo.tx_info_store().await.unwrap();
            assert_eq!(
                load_state(&store).await.unwrap(),
                Some(LifecycleState::Armed),
                "the obligation persists until a clean shutdown"
            );
            assert_eq!(repo.durability_metrics_snapshot().counters[7], 1);
        });
        // Second unclean reopen under durable: exactly one more bump.
        runtime().block_on(async {
            let repo = durable_open(&path).await.unwrap();
            let identity = repo.repo_identity().await.unwrap();
            assert_eq!(identity.lineage_id, before.lineage_id);
            assert_eq!(identity.history_epoch, before.history_epoch + 2);
            assert_eq!(repo.durability_metrics_snapshot().counters[7], 1);
        });
    }

    /// CRASH BOUNDARY (i): crash between arming and the first serve. The
    /// Armed record survives the reopen; each fresh repo instance bumps
    /// exactly once (counter == 1 on every reopened instance) and the
    /// lineage is identical across all three opens.
    #[test]
    fn crash_after_arm_before_first_serve_bumps_per_reopen() {
        SKIP_BUMP_FLUSH_FOR_TEST.store(false, Ordering::Release);
        SKIP_UNCLEAN_EPOCH_BUMP_FOR_TEST.store(false, Ordering::Release);
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().to_path_buf();
        let before = runtime().block_on(async {
            let repo = reseed_open(&path).await.unwrap();
            repo.ensure_replication_armed().await.unwrap();
            repo.repo_identity().await.unwrap()
        });
        for expected_bump in [1u64, 2] {
            runtime().block_on(async {
                let repo = reseed_open(&path).await.unwrap();
                let identity = repo.repo_identity().await.unwrap();
                assert_eq!(identity.lineage_id, before.lineage_id);
                assert_eq!(identity.history_epoch, before.history_epoch + expected_bump);
                let store = repo.tx_info_store().await.unwrap();
                assert_eq!(
                    load_state(&store).await.unwrap(),
                    Some(LifecycleState::Armed),
                    "the obligation persists until a clean shutdown"
                );
                // Fresh instance per open: the counter reflects only THIS
                // open's bump.
                assert_eq!(
                    repo.durability_metrics_snapshot().counters[7],
                    1,
                    "reopen #{expected_bump} must bump exactly once"
                );
            });
        }
    }

    /// CRASH BOUNDARY (ii): crash after the clean certificate. Clean was
    /// written and flushed by `shutdown_clean`, so every reopen finds Clean,
    /// does NOT bump (epoch unchanged, counter 0), and the lineage is stable.
    #[test]
    fn crash_after_clean_certificate_does_not_bump() {
        SKIP_UNCLEAN_EPOCH_BUMP_FOR_TEST.store(false, Ordering::Release);
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().to_path_buf();
        let before = runtime().block_on(async {
            let repo = reseed_open(&path).await.unwrap();
            repo.ensure_replication_armed().await.unwrap();
            repo.shutdown_clean(std::time::Duration::from_secs(5))
                .await
                .unwrap();
            let store = repo.tx_info_store().await.unwrap();
            assert_eq!(
                load_state(&store).await.unwrap(),
                Some(LifecycleState::Clean)
            );
            repo.repo_identity().await.unwrap()
        });
        for _reopen in 0..2 {
            runtime().block_on(async {
                let repo = reseed_open(&path).await.unwrap();
                let identity = repo.repo_identity().await.unwrap();
                assert_eq!(identity.lineage_id, before.lineage_id);
                assert_eq!(
                    identity.history_epoch, before.history_epoch,
                    "a valid Clean certificate closes the obligation"
                );
                let store = repo.tx_info_store().await.unwrap();
                assert_eq!(
                    load_state(&store).await.unwrap(),
                    Some(LifecycleState::Clean)
                );
                assert_eq!(
                    repo.durability_metrics_snapshot().counters[7],
                    0,
                    "no unclean bump after a clean certificate"
                );
            });
        }
    }
}
