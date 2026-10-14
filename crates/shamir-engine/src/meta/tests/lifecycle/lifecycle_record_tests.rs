//! PR8 `completed_reseed` chunk 2 — lifecycle record (armed / clean
//! certificate) and the cold-start history-epoch bump.
//!
//! Record-level (non-crash) tests use `InMemoryStore` exactly like the
//! sibling `repl_bookmark_tests`; "reopen" there is a fresh decode of the
//! same store value. Every CRASH/reopen test below uses a DISK-backed fjall
//! fixture and destroys the whole tokio runtime between opens (Windows fjall
//! lock release), so a skipped `flush` really is lost — that is what makes
//! the negative-control seams meaningful.

use crate::meta::repo_identity::RepoIdentity;
use crate::meta::repo_identity::{
    ensure_armed, load_state, mark_clean, requires_unclean_bump, LifecycleState,
    SKIP_UNCLEAN_EPOCH_BUMP_FOR_TEST,
};
#[cfg(feature = "fjall")]
use crate::meta::repo_identity::{
    SKIP_ARMING_FLUSH_FOR_TEST, SKIP_BUMP_FLUSH_FOR_TEST, SKIP_CLEAN_FLUSH_FOR_TEST,
};
use crate::repo::durability_policy::{
    DurabilityPolicy, ReplicationServe, RetentionOverflow, WalCommitTier,
    DEFAULT_WAL_RETENTION_MAX_BYTES,
};
use crate::repo::{BoxRepo, RepoInstance};
use crate::table::TableConfig;
use shamir_storage::storage_in_memory::{InMemoryRepo, InMemoryStore};
use shamir_storage::types::Store;
use std::sync::atomic::Ordering;
use std::sync::Arc;

fn mem_store() -> Arc<dyn Store> {
    Arc::new(InMemoryStore::new())
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

    fn reseed_open(dir: &std::path::Path) -> impl Future<Output = RepoInstance> {
        RepoInstance::from_factory_with_durability(
            "main".into(),
            crate::repo::BoxRepoFactory::fjall_raw(dir),
            vec![],
            reseed_policy(),
        )
    }

    fn durable_open(dir: &std::path::Path) -> impl Future<Output = RepoInstance> {
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

    /// CRASH BOUNDARY (iii): crash DURING the epoch bump, flush skipped
    /// (`SKIP_BUMP_FLUSH_FOR_TEST`). The on-disk record stays Armed with the
    /// OLD epoch, so the reopen hook re-derives the bump: epoch = old+1,
    /// then old+2 — a torn/lost bump CONVERGES instead of corrupting.
    #[test]
    fn crash_during_bump_flush_skipped_converges() {
        SKIP_UNCLEAN_EPOCH_BUMP_FOR_TEST.store(false, Ordering::Release);
        SKIP_BUMP_FLUSH_FOR_TEST.store(false, Ordering::Release);
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().to_path_buf();
        let before = runtime().block_on(async {
            let repo = reseed_open(&path).await.unwrap();
            repo.ensure_replication_armed().await.unwrap();
            let id = repo.repo_identity().await.unwrap();
            // Compute the bump but lose its flush.
            SKIP_BUMP_FLUSH_FOR_TEST.store(true, Ordering::Release);
            repo.note_unclean_open_bump().await.unwrap();
            SKIP_BUMP_FLUSH_FOR_TEST.store(false, Ordering::Release);
            id
        });
        for expected_bump in [1u64, 2] {
            runtime().block_on(async {
                let repo = reseed_open(&path).await.unwrap();
                let identity = repo.repo_identity().await.unwrap();
                assert_eq!(identity.lineage_id, before.lineage_id);
                assert_eq!(
                    identity.history_epoch,
                    before.history_epoch + expected_bump,
                    "the lost bump must be re-derived on reopen"
                );
                let store = repo.tx_info_store().await.unwrap();
                assert_eq!(
                    load_state(&store).await.unwrap(),
                    Some(LifecycleState::Armed)
                );
                assert_eq!(repo.durability_metrics_snapshot().counters[7], 1);
            });
        }
    }

    /// NEGATIVE CONTROL (arming flush): the protection must be load-bearing.
    /// Default path: the arming flush survives an unclean end, so the reopen
    /// reads Armed. With `SKIP_ARMING_FLUSH_FOR_TEST` set, the unflushed
    /// Armed write is lost and the reopen reads NO record at all.
    #[test]
    fn arming_flush_is_load_bearing_negative_control() {
        SKIP_ARMING_FLUSH_FOR_TEST.store(false, Ordering::Release);
        // Protection present: Armed survives the unclean end.
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().to_path_buf();
        runtime().block_on(async {
            let repo = reseed_open(&path).await.unwrap();
            repo.ensure_replication_armed().await.unwrap();
        });
        runtime().block_on(async {
            let repo = reseed_open(&path).await.unwrap();
            let store = repo.tx_info_store().await.unwrap();
            assert_eq!(
                load_state(&store).await.unwrap(),
                Some(LifecycleState::Armed),
                "the arming flush must survive an unclean process end"
            );
        });
        // Seam set: the "protection present" assertion above would FAIL —
        // the record never reached disk.
        SKIP_ARMING_FLUSH_FOR_TEST.store(true, Ordering::Release);
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().to_path_buf();
        runtime().block_on(async {
            let repo = reseed_open(&path).await.unwrap();
            repo.ensure_replication_armed().await.unwrap();
        });
        runtime().block_on(async {
            let repo = reseed_open(&path).await.unwrap();
            let store = repo.tx_info_store().await.unwrap();
            assert_eq!(
                load_state(&store).await.unwrap(),
                None,
                "without the arming flush the Armed record must be lost"
            );
        });
        SKIP_ARMING_FLUSH_FOR_TEST.store(false, Ordering::Release);
    }

    /// NEGATIVE CONTROL (clean-certificate flush): with
    /// `SKIP_CLEAN_FLUSH_FOR_TEST`, `shutdown_clean`'s Clean write never
    /// reaches disk, so the unclean reopen still sees Armed — and the epoch
    /// bumps. Proves the certificate flush is what closes the obligation.
    #[test]
    fn clean_flush_is_load_bearing_negative_control() {
        SKIP_UNCLEAN_EPOCH_BUMP_FOR_TEST.store(false, Ordering::Release);
        SKIP_CLEAN_FLUSH_FOR_TEST.store(true, Ordering::Release);
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().to_path_buf();
        let before = runtime().block_on(async {
            let repo = reseed_open(&path).await.unwrap();
            repo.ensure_replication_armed().await.unwrap();
            let id = repo.repo_identity().await.unwrap();
            repo.shutdown_clean(std::time::Duration::from_secs(5))
                .await
                .unwrap();
            id
        });
        SKIP_CLEAN_FLUSH_FOR_TEST.store(false, Ordering::Release);
        runtime().block_on(async {
            let repo = reseed_open(&path).await.unwrap();
            let identity = repo.repo_identity().await.unwrap();
            assert_eq!(identity.lineage_id, before.lineage_id);
            assert_eq!(
                identity.history_epoch,
                before.history_epoch + 1,
                "the lost Clean certificate must cost the unclean bump"
            );
            let store = repo.tx_info_store().await.unwrap();
            assert_eq!(
                load_state(&store).await.unwrap(),
                Some(LifecycleState::Armed),
                "the record must NOT be Clean: its flush was skipped"
            );
            assert_eq!(repo.durability_metrics_snapshot().counters[7], 1);
        });
    }
}
