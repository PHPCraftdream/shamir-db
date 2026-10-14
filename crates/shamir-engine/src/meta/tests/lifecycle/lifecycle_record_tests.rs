//! PR8 `completed_reseed` chunk 2 — lifecycle record (armed / clean
//! certificate) and the cold-start history-epoch bump.
//!
//! Record-level tests use `InMemoryStore` exactly like the sibling
//! `repl_bookmark_tests`; "reopen" at this level is a fresh decode of the
//! same store value (the envelope decode path is what a crash boundary
//! exercises — the store value is written atomically by `set` + `flush`).

use crate::meta::repo_identity::RepoIdentity;
use crate::meta::repo_identity::{
    ensure_armed, load_state, mark_clean, requires_unclean_bump, LifecycleState,
    SKIP_UNCLEAN_EPOCH_BUMP_FOR_TEST,
};
use crate::repo::durability_policy::{
    DurabilityPolicy, ReplicationServe, RetentionOverflow, WalCommitTier,
    DEFAULT_WAL_RETENTION_MAX_BYTES,
};
use crate::repo::{BoxRepo, RepoInstance};
use crate::table::TableConfig;
use shamir_query_types::ReplicationServe as Serve;
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
/// (the obligation returns); and switching the repo to Durable mode does
/// NOT erase an armed obligation — nothing in the record path reads the
/// mode, so a reload under any mode still shows Armed.
#[tokio::test]
async fn clean_then_rearm_survives_mode_switch() {
    let s = mem_store();
    mark_clean(&s).await.unwrap();
    assert_eq!(load_state(&s).await.unwrap(), Some(LifecycleState::Clean));
    ensure_armed(&s).await.unwrap();
    assert_eq!(load_state(&s).await.unwrap(), Some(LifecycleState::Armed));
    // Mode switch to Durable: the record is untouched.
    assert_eq!(
        load_state(&s).await.unwrap(),
        Some(LifecycleState::Armed),
        "switching to durable must not erase an armed obligation"
    );
}

/// (c) Pure decision truth table.
#[test]
fn requires_unclean_bump_truth_table() {
    assert!(requires_unclean_bump(
        Serve::CompletedReseed,
        Some(LifecycleState::Armed)
    ));
    assert!(!requires_unclean_bump(
        Serve::CompletedReseed,
        Some(LifecycleState::Clean)
    ));
    assert!(!requires_unclean_bump(Serve::CompletedReseed, None));
    assert!(!requires_unclean_bump(
        Serve::Durable,
        Some(LifecycleState::Armed)
    ));
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
    assert!(requires_unclean_bump(Serve::CompletedReseed, Some(state)));
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

/// Crash boundary (i): crash between arming and the first serve. The Armed
/// record survives the reopen and still demands the bump.
#[tokio::test]
async fn crash_after_arm_before_first_serve() {
    SKIP_UNCLEAN_EPOCH_BUMP_FOR_TEST.store(false, Ordering::Release);
    let s = mem_store();
    ensure_armed(&s).await.unwrap();
    // Reopen (store value re-decoded from scratch — the set+flush already
    // completed before the "crash").
    let state = load_state(&s).await.unwrap();
    assert_eq!(state, Some(LifecycleState::Armed));
    assert!(requires_unclean_bump(Serve::CompletedReseed, state));
}

/// Crash boundary (ii): crash after the clean certificate. The Clean record
/// survives the reopen and demands NO bump.
#[tokio::test]
async fn crash_after_clean_certificate() {
    SKIP_UNCLEAN_EPOCH_BUMP_FOR_TEST.store(false, Ordering::Release);
    let s = mem_store();
    ensure_armed(&s).await.unwrap();
    mark_clean(&s).await.unwrap();
    let state = load_state(&s).await.unwrap();
    assert_eq!(state, Some(LifecycleState::Clean));
    assert!(!requires_unclean_bump(Serve::CompletedReseed, state));
}

/// Crash boundary (iii): the epoch bump itself is an atomic single-key
/// save+flush — reopening at any point decodes either the old or the new
/// epoch, never a torn record; after a completed bump the epoch is exactly
/// old + 1.
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
