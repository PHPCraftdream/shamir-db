use shamir_collections::TFxMap;
use shamir_query_types::batch::TransactionInfo;
use shamir_storage::error::DbError;
use shamir_tunables::instance_defaults::MAX_UNDRAINED_VERSIONS;
use shamir_tx::{IsolationLevel, TxContext};
use shamir_types::types::common::THasher;
use shamir_wal::{WalAppendError, WalOpV2};

use crate::repo::RepoInstance;
use crate::tx::commit_phases::{
    apply_data_phase, apply_vector_delta_phase, materialize_async_tail, promote_vectors,
};
use crate::tx::finalize::finalize_sync_post_publish;
use crate::tx::materialize::materialize;
use crate::tx::pre_commit::{
    pre_commit_locked, pre_commit_locked_validate, pre_commit_prelock, PreCommit, PreLockResult,
};
use crate::tx::tx_outcome::{BackgroundCommitHandle, MaterializationState, TxOutcome};

const DEFAULT_MAX_TX_LIFETIME: std::time::Duration = std::time::Duration::from_secs(300); // 5 min

/// Test-only override for [`DEFAULT_MAX_TX_LIFETIME`].
///
/// `Instant::now() - Duration::from_secs(N)` (the natural way to simulate an
/// old tx) panics on overflow if the process/host has been up for less than
/// `N` — observed on a freshly-booted CI VM. Rather than have expiry tests
/// backdate `TxContext::started_at` by a large, machine-uptime-dependent
/// amount, they shrink the effective max lifetime here (to a few
/// milliseconds) and use a short real `tokio::time::sleep` instead — fully
/// deterministic regardless of host uptime. `nextest` runs each test in its
/// own process, so this global cannot leak across test files.
#[cfg(test)]
pub(crate) static TEST_MAX_TX_LIFETIME_OVERRIDE: std::sync::OnceLock<std::time::Duration> =
    std::sync::OnceLock::new();

fn effective_max_tx_lifetime() -> std::time::Duration {
    #[cfg(test)]
    if let Some(d) = TEST_MAX_TX_LIFETIME_OVERRIDE.get() {
        return *d;
    }
    DEFAULT_MAX_TX_LIFETIME
}

/// Test-only pause/resume handshake installed via
/// [`TEST_POST_VALIDATE_PRE_PUBLISH_HOOK`]. `reached` lets the harness thread
/// detect (via polling, no sleeps) that the commit under test has actually
/// parked at the seam before the harness fires the concurrent writer;
/// `resume` is the release signal the harness fires once the concurrent
/// writer has fully committed.
///
/// `armed` closes a real hang this seam originally had: EVERY transaction
/// that reaches `commit_tx_lockfree`'s hook call site parks here, not just
/// the "parent" tx a test intends to pause — including the concurrent
/// writer the harness itself drives to commit AFTER releasing the parent
/// (once the writer acquires `commit_lock` in turn, it reaches this SAME
/// program point). A `Notify::notify_one()` wakes at most one parked waiter,
/// so a harness that calls it exactly once (to release the parent) leaves
/// the writer's later arrival parked forever with no second wakeup ever
/// coming — a genuine deadlock, not a slow test. `armed` makes the pause
/// one-shot: the first caller to reach the seam consumes it (CAS
/// true→false) and actually parks; every subsequent caller (the writer,
/// or any other concurrent committer) sees `armed == false` and passes
/// through immediately, unpaused. A test that wants exactly ONE tx to pause
/// here gets that for free without having to know in advance how many
/// OTHER committers will transit the same seam before the test finishes.
#[cfg(test)]
#[derive(Debug)]
pub(crate) struct PostValidatePrePublishHook {
    pub(crate) reached: std::sync::atomic::AtomicUsize,
    pub(crate) resume: tokio::sync::Notify,
    pub(crate) armed: std::sync::atomic::AtomicBool,
}

/// Test-only pause seam: fires in `commit_tx_lockfree` immediately AFTER
/// `pre_commit_locked_validate` returns success (Phase 2-bis's
/// `predicate_conflicts_batch` — the RI barrier's own commit-time re-check —
/// has already run and passed) and BEFORE Phase 4 (`wal.begin_grouped`, the
/// actual commit/publish point).
///
/// F-46 (#857): the existing race-test seam
/// (`fk_ri_barrier_tests.rs`'s `RaceInjectingResolver`, injecting at a
/// `resolve_repo()` call ordinal) fires the concurrent writer BEFORE the
/// parent even reaches `commit_interactive_tx` — it proves a
/// writer-already-fully-committed-before-parent-validates race, not a
/// writer-committing-DURING-the-parent's-own-commit-lock-window race. This
/// hook fires deterministically inside that window instead, so a test can
/// drive a concurrent FK-child writer to completion strictly between the
/// parent's barrier check and the parent's own publish — the exact gap the
/// 2026-07-28 readonly review's P0-1 finding describes.
///
/// Shape mirrors `TEST_MAX_TX_LIFETIME_OVERRIDE` above (a `#[cfg(test)]`
/// `OnceLock`, zero cost when unset) combined with
/// `fk_reverse_cache_race_tests.rs`'s counter-poll + `Notify` pause/resume
/// handshake: a test installs an `Arc<PostValidatePrePublishHook>` here, then
/// the seam bumps `reached` and parks on `resume.notified()` at exactly this
/// program point. The harness thread busy-polls `reached` (no sleeps) until
/// it sees the bump, fires the concurrent writer to full commit, THEN calls
/// `resume.notify_one()`, guaranteeing (by construction of the wait) that the
/// writer's publish strictly happens-before this tx resumes toward its own
/// publish — the precise interleaving the review's counter-example requires.
///
/// `nextest` runs each test in its own process (see
/// `TEST_MAX_TX_LIFETIME_OVERRIDE`'s doc), so this global cannot leak across
/// test files. Uninstalled (`None`, the default for every other test in the
/// suite) this is a single `OnceLock::get()` read with no lock, no
/// allocation, no await point taken.
#[cfg(test)]
pub(crate) static TEST_POST_VALIDATE_PRE_PUBLISH_HOOK: std::sync::OnceLock<
    std::sync::Arc<PostValidatePrePublishHook>,
> = std::sync::OnceLock::new();

/// Parks on [`TEST_POST_VALIDATE_PRE_PUBLISH_HOOK`] if a test installed one;
/// a true no-op otherwise. See that static's doc for the exact program point
/// and rationale.
async fn fire_post_validate_pre_publish_test_hook() {
    #[cfg(test)]
    if let Some(hook) = TEST_POST_VALIDATE_PRE_PUBLISH_HOOK.get() {
        hook.reached
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        // One-shot: only the FIRST tx to reach this seam actually parks (see
        // `PostValidatePrePublishHook::armed`'s doc for why this is load-
        // bearing, not cosmetic). `compare_exchange` CAS true→false so
        // exactly one racer wins even if two committers reached this line
        // concurrently; every later arrival (including the harness's own
        // concurrent writer, once IT reaches this same program point after
        // acquiring `commit_lock` in turn) sees `false` and returns at once.
        let should_pause = hook
            .armed
            .compare_exchange(
                true,
                false,
                std::sync::atomic::Ordering::AcqRel,
                std::sync::atomic::Ordering::Acquire,
            )
            .is_ok();
        if should_pause {
            hook.resume.notified().await;
        }
    }
}

#[cfg(any(test, feature = "test-util"))]
pub struct PostWalPrePublishHook {
    pub repo_name: String,
    pub pre_wal: std::sync::atomic::AtomicBool,
    pub after_counter: std::sync::atomic::AtomicBool,
    pub fail_orphan_markers: std::sync::atomic::AtomicBool,
    pub panic_after_resume: std::sync::atomic::AtomicBool,
    pub panic_resume: tokio::sync::Notify,
    pub reached: tokio::sync::Notify,
    pub resume: tokio::sync::Notify,
    pub armed: std::sync::atomic::AtomicBool,
}

#[cfg(any(test, feature = "test-util"))]
pub static TEST_POST_WAL_PRE_PUBLISH_HOOK: std::sync::OnceLock<
    std::sync::Arc<PostWalPrePublishHook>,
> = std::sync::OnceLock::new();

pub(super) async fn fire_post_wal_pre_publish_test_hook(repo: &RepoInstance, after_counter: bool) {
    #[cfg(any(test, feature = "test-util"))]
    if TEST_POST_WAL_PRE_PUBLISH_HOOK.get().is_some_and(|hook| {
        hook.after_counter
            .load(std::sync::atomic::Ordering::Acquire)
            != after_counter
    }) {
        return;
    }
    let _ = after_counter;
    fire_cancel_test_hook(repo, false).await;
}

async fn fire_cancel_test_hook(repo: &RepoInstance, pre_wal: bool) {
    #[cfg(any(test, feature = "test-util"))]
    if let Some(hook) = TEST_POST_WAL_PRE_PUBLISH_HOOK.get() {
        if hook.repo_name == repo.name()
            && hook.pre_wal.load(std::sync::atomic::Ordering::Acquire) == pre_wal
            && hook.armed.swap(false, std::sync::atomic::Ordering::AcqRel)
        {
            // Capture the one-shot panic mode before announcing the seam. A
            // separate wake targets the orphan while B1 keeps another ACTIVE
            // committer parked on the ordinary resume queue.
            let panic_after_resume = hook
                .panic_after_resume
                .swap(false, std::sync::atomic::Ordering::AcqRel);
            hook.reached.notify_one();
            if panic_after_resume {
                hook.panic_resume.notified().await;
                panic!("injected commit-tail panic");
            }
            hook.resume.notified().await;
        }
    }
    let _ = (repo, pre_wal);
}

/// Test-only one-shot pause for AsyncIndex's independently spawned tail.
#[cfg(test)]
#[derive(Debug)]
pub(crate) struct AsyncIndexTailHook {
    pub(crate) repo_name: String,
    pub(crate) reached: tokio::sync::Notify,
    pub(crate) resume: tokio::sync::Notify,
    pub(crate) armed: std::sync::atomic::AtomicBool,
}

#[cfg(test)]
pub(crate) static TEST_ASYNC_INDEX_TAIL_HOOK: std::sync::OnceLock<
    std::sync::Arc<AsyncIndexTailHook>,
> = std::sync::OnceLock::new();

async fn fire_async_index_tail_test_hook(repo: &RepoInstance) {
    #[cfg(test)]
    if let Some(hook) = TEST_ASYNC_INDEX_TAIL_HOOK.get() {
        if hook.repo_name == repo.name()
            && hook.armed.swap(false, std::sync::atomic::Ordering::AcqRel)
        {
            hook.reached.notify_one();
            hook.resume.notified().await;
        }
    }
    let _ = repo;
}

/// Test-only crash-injection seam for the real crash-recovery harness
/// (`crates/shamir-engine/tests/crash_recovery.rs`, Vector II.1).
///
/// At each labelled point in the commit pipeline the harness can force a
/// HARD process death to prove atomicity around the Phase-4 commit point:
/// a crash *before* Phase 4 leaves nothing durable (clean abort); a crash
/// *at/after* Phase 4 but before Phase 7 leaves an inflight WAL marker
/// that recovery replays idempotently (all-or-nothing materialization).
///
/// Why `std::process::abort()` and not `panic!`:
///   `panic!` UNWINDS the stack — it runs every `Drop` impl on the way
///   out, which would flush `MemBuffer`/`Cached` write-backs, drop
///   storage handles cleanly, and let `RAII` guards release. That is a
///   *graceful* shutdown, the exact opposite of a crash. `process::abort`
///   raises `SIGABRT` immediately: no unwind, no `Drop`, no flush — the
///   closest in-process analog to `kill -9`. The on-disk state left
///   behind is therefore a genuine torn-mid-commit image, which is what
///   the recovery contract must survive.
///
/// Durability of the on-disk image (the `repo` argument):
///   The crash is real, but for the recovery contract to be *meaningful*
///   the WAL entry written in Phase 4 (and the Phase-5 row writes for the
///   later seams) must already be on disk when the process dies. Several
///   backends — redb's `Store::set` among them — use a deferred-fsync
///   write mode where rows become crash-durable only on an explicit
///   `flush()` (or, in production, on the MemBuffer flush tick). To model
///   a backend whose WAL/data writes are *synchronously* durable (the
///   durability mode the recovery guarantee assumes), every seam AT or
///   AFTER the commit point flushes the repo's shared store before
///   aborting. Because all of a repo's stores (WAL `__tx__`, per-table
///   `__data__`/`__info__`) share one backend handle (e.g. one redb
///   `Database`), a single `flush()` makes the entire pending image
///   durable. This flush is NOT a graceful shutdown: it is `fsync`, not
///   `Drop` — no buffers are drained through destructors, no guards
///   release, no in-memory tx state is reconciled. The `pre_commit` seam
///   deliberately does NOT flush: it fires BEFORE the WAL entry exists,
///   so the disk must show nothing tx-related (clean abort).
///
/// Gating (zero cost in production):
///   The whole hook is `#[cfg(debug_assertions)]`. In a normal
///   `--release` build `debug_assertions` is off, so `maybe_crash`
///   compiles to the empty `#[inline(always)]` twin below — dead code,
///   no env read, no branch, the `repo`/`phase` args unused. Tests build
///   in debug, where the hook is live but still a NO-OP unless
///   `SHAMIR_TEST_CRASH_AFTER` is set to a matching phase label. With no
///   env var present every existing test pays at most one `env::var` miss
///   per seam and never crashes or flushes.
///
/// Phase labels: `pre_commit`, `phase4`, `phase5a`, `phase5c`,
/// `phase5d_delta` (#426 VR-4 — after the pre-publish delta-chunk append,
/// before publish; proves the durable delta survives a crash and
/// `restore_on_open` replays it), `phase5d_delta_async` (the AsyncIndex
/// twin), `phase6`, `phase6_5`, `phase7`. D4 drain seam (fired from the drainer's per-entry
/// loop, not the ack-path): `drain_replay` — AFTER `replay_v2_entry` made the
/// entry durable in `history` but BEFORE `mark_durable` advanced the durable
/// watermark, so the entry is still inflight and recovery re-replays it
/// idempotently. F6c truncation seams (fired from the drainer, not the
/// ack-path): `pre_truncate` (before history-flush + segment unlink) and
/// `post_truncate` (after a successful `truncate_below`). A third truncation
/// seam, `wal_mid_delete`, lives inside `shamir-wal`'s
/// `SegmentSet::truncate_below` (it cannot reach this engine hook) and aborts
/// between two segment unlinks.
#[cfg(debug_assertions)]
pub(super) async fn maybe_crash(phase: &str, repo: &RepoInstance) {
    let Ok(target) = std::env::var("SHAMIR_TEST_CRASH_AFTER") else {
        return;
    };
    if target != phase {
        return;
    }
    // AT/AFTER the commit point: make the pending on-disk image durable
    // (fsync, NOT a graceful drain) so the killed process leaves a real
    // torn-mid-commit state the reopened repo must recover. `pre_commit`
    // skips this — nothing tx-related should be durable yet.
    if phase != "pre_commit" {
        if let Ok(store) = repo.tx_info_store().await {
            let _ = store.flush().await;
        }
    }
    // HARD crash: no unwind, no Drop, no in-memory reconciliation.
    std::process::abort();
}

/// Release twin: the crash seam does not exist outside debug builds.
#[cfg(not(debug_assertions))]
#[inline(always)]
pub(super) async fn maybe_crash(_phase: &str, _repo: &RepoInstance) {}

#[derive(Debug, thiserror::Error)]
pub enum TxError {
    #[error("WAL outcome indeterminate; repository must be reopened")]
    WalRequiresReopen,
    #[error("commit {commit_version} published but WAL durability failed")]
    WalDurability {
        commit_version: u64,
        materialization: MaterializationState,
    },
    #[error("table '{table}' is frozen for migration")]
    MigrationFrozen { table: String },
    #[error("storage: {0}")]
    Storage(#[from] DbError),
    #[error("ssi conflict on key {key:?}")]
    SsiConflict { key: bytes::Bytes },
    #[error("unique constraint violated on key {key:?}")]
    UniqueViolation { key: bytes::Bytes },
    #[error("tx expired: elapsed {elapsed:?} > max {max:?}")]
    Expired {
        elapsed: std::time::Duration,
        max: std::time::Duration,
    },
    /// Phase C. A concurrent committer wrote a key matching one of
    /// this tx's recorded predicate dependencies (a phantom). Surfaced
    /// to the wire path as `"tx_conflict"` so existing client retry
    /// covers it.
    #[error("phantom conflict on predicate {dep}")]
    PhantomConflict { dep: String },
    /// Level-3 wound-wait abort: an older (higher-priority) tx wounded
    /// this one during `lock_key`, or this tx observed its own wound flag
    /// at commit entry. Surfaced to the wire path as `"tx_conflict"` so
    /// existing client retry covers it.
    #[error("tx {} wounded (wound-wait abort)", tx_version)]
    Wounded { tx_version: u64 },
    /// FG-7: a CAS (`expected_version`) check failed at commit time — a
    /// concurrent committer changed the key's version between the
    /// immediate `check_expected_version` staging-time check and this tx's
    /// commit. Surfaced to the wire path as `"version_conflict"` (NOT
    /// `"tx_conflict"`) so client retry logic keyed on `version_conflict`
    /// (the immediate-check error code) handles BOTH failure timings
    /// identically — see `OPTIMISTIC_CONCURRENCY.md`.
    #[error("cas conflict on key {key:?}: expected version {expected} but found {found}")]
    CasConflict {
        key: bytes::Bytes,
        expected: u64,
        found: u64,
    },
}

impl TxError {
    /// Map commit errors without turning a published version into an abort.
    pub fn into_transaction_info(
        self,
        tx_id: u64,
        snapshot_version: u64,
    ) -> shamir_query_types::batch::TransactionInfo {
        let reason = match self {
            Self::WalDurability {
                commit_version,
                materialization,
            } => {
                return TransactionInfo::committed_with_error(
                    tx_id,
                    snapshot_version,
                    commit_version,
                    materialization == MaterializationState::Complete,
                    "wal_durability",
                );
            }
            Self::MigrationFrozen { .. } => "table_migrating".to_string(),
            Self::SsiConflict { .. } | Self::PhantomConflict { .. } | Self::Wounded { .. } => {
                "tx_conflict".to_string()
            }
            Self::UniqueViolation { .. } => "unique_violation".to_string(),
            // FG-7: commit-time CAS failures retain the staging-time code.
            Self::CasConflict { .. } => "version_conflict".to_string(),
            Self::WalRequiresReopen => "wal_requires_reopen".to_string(),
            Self::Storage(e) => format!("storage: {}", e),
            Self::Expired { elapsed, max } => {
                format!("tx expired: elapsed {:?} > max {:?}", elapsed, max)
            }
        };
        TransactionInfo::aborted(tx_id, reason)
    }
}

impl From<TxError> for DbError {
    fn from(error: TxError) -> Self {
        match error {
            error @ (TxError::WalRequiresReopen | TxError::WalDurability { .. }) => {
                DbError::Storage(error.to_string())
            }
            TxError::Storage(error) => error,
            TxError::MigrationFrozen { table } => DbError::MigrationFrozen { table },
            error @ (TxError::SsiConflict { .. }
            | TxError::PhantomConflict { .. }
            | TxError::Wounded { .. }) => DbError::Conflict(error.to_string()),
            error @ TxError::UniqueViolation { .. } => DbError::DuplicateKey(error.to_string()),
            error @ TxError::CasConflict { .. } => DbError::VersionConflict(error.to_string()),
            // DbError has no expiry category; retain the diagnostic without
            // misclassifying expiry as a retryable transaction conflict.
            error @ TxError::Expired { .. } => DbError::Internal(error.to_string()),
        }
    }
}

#[cfg(test)]
pub(crate) static TEST_SYNCED_WAL_REPO: std::sync::OnceLock<String> = std::sync::OnceLock::new();

pub(super) fn commit_wal_durability(repo: &RepoInstance) -> shamir_wal::WalDurability {
    #[cfg(test)]
    if TEST_SYNCED_WAL_REPO
        .get()
        .is_some_and(|name| name == repo.name())
    {
        return shamir_wal::WalDurability::Synced;
    }
    let _ = repo;
    shamir_wal::WalDurability::Buffered
}

pub(super) fn wal_completion(
    gate: &shamir_tx::RepoTxGate,
    result: Result<(), WalAppendError>,
) -> Result<bool, TxError> {
    match result {
        Ok(()) => Ok(false),
        Err(WalAppendError::WrittenNotDurable) => Ok(true),
        Err(WalAppendError::NotWritten) => Err(TxError::Storage(WalAppendError::NotWritten.into())),
        Err(WalAppendError::Unknown | WalAppendError::Indeterminate) => {
            gate.set_requires_reopen();
            Err(TxError::WalRequiresReopen)
        }
        Err(_) => {
            gate.set_requires_reopen();
            Err(TxError::WalRequiresReopen)
        }
    }
}

/// cancel-safe: yes — read-only over `&TxContext`. The only `.await`
/// is `interner_overlay.scan_async` which is itself cancel-safe (the
/// borrowed map is untouched on drop); the rest is in-memory iteration.
/// No external state mutation happens here.
///
/// Build WalOpV2 ops from a TxContext for inclusion in the V2 WAL entry.
///
/// Emitted ops in order:
/// - CounterDelta per table.
/// - InternerOverlayMerge (if overlay non-empty).
/// - IndexPut / IndexDel from index_write_set (table_id_interned from
///   per-op table_token stamped at write time; idx_id=0 placeholder).
/// - Put / Delete from write_set snapshot (carry table_id_interned
///   so recovery can resolve target data_store).
/// - BumpFtsStats is in-memory only and not serialised.
pub async fn wal_ops_from_tx(tx: &TxContext) -> Vec<WalOpV2> {
    let mut ops =
        Vec::with_capacity(tx.counter_deltas.len() + tx.index_write_set.len() + tx.write_set.len());

    for (table_id, delta) in &tx.counter_deltas {
        ops.push(WalOpV2::CounterDelta {
            table_id_interned: *table_id,
            delta: *delta,
        });
    }

    // L10(c): skip the async scan when the overlay is empty. `is_empty()`
    // on `scc::HashMap` is NOT an atomic length read (scc exposes no atomic
    // length — `len()` is banned by `clippy.toml`'s `disallowed-methods`
    // precisely because it is O(N)); `is_empty` walks the bucket array
    // until it finds the first entry or exhausts it. That bucket-walk is
    // sync and short-circuits on the first non-empty bucket, so it stays
    // cheap on the hot path and avoids a needless `.await` point. Race
    // with a concurrent `insert` is benign: the overlay is tx-scoped and
    // only the owning tx mutates it, so the emptiness check is stable.
    if !tx.interner_overlay.is_empty() {
        // O(N) ack: per-tx interner delta sizing, bounded by this tx's touches.
        #[allow(clippy::disallowed_methods)]
        let mut entries: Vec<(u64, String)> = Vec::with_capacity(tx.interner_overlay.len());
        tx.interner_overlay
            .iter_async(|k, v| {
                entries.push((*v, k.clone()));
                true
            })
            .await;
        if !entries.is_empty() {
            ops.push(WalOpV2::InternerOverlayMerge { entries });
        }
    }

    for (table_token, op) in &tx.index_write_set {
        match op {
            shamir_tx::IndexWriteOp::SetPosting { key, value, .. } => {
                ops.push(WalOpV2::IndexPut {
                    table_id_interned: *table_token,
                    idx_id: 0,
                    key: key.clone(),
                    value: value.clone(),
                });
            }
            shamir_tx::IndexWriteOp::RemovePosting { key, .. } => {
                ops.push(WalOpV2::IndexDel {
                    table_id_interned: *table_token,
                    idx_id: 0,
                    key: key.clone(),
                });
            }
            shamir_tx::IndexWriteOp::BumpFtsStats { .. } => {}
        }
    }

    // Phase 4 data ops: snapshot each per-table StagingStore (no
    // consume — drain happens in Phase 5). This makes the WAL entry
    // self-contained: recovery can replay tx data writes without
    // needing the (still-staged) StagingStore around.
    for (table_id, staging) in &tx.write_set {
        for kv_op in staging.iter_ops() {
            match kv_op {
                shamir_storage::types::KvOp::Set(k, v) => {
                    if let Some(rid) = shamir_types::types::record_id::RecordId::try_from_bytes(&k)
                    {
                        ops.push(WalOpV2::Put {
                            table_id_interned: *table_id,
                            rid,
                            body: v,
                        });
                    }
                }
                shamir_storage::types::KvOp::Remove(k) => {
                    if let Some(rid) = shamir_types::types::record_id::RecordId::try_from_bytes(&k)
                    {
                        ops.push(WalOpV2::Delete {
                            table_id_interned: *table_id,
                            rid,
                        });
                    }
                }
            }
        }
    }

    ops
}

/// cancel-safe: partial — cancellation during prelock (including unique-lock
/// waits) still aborts. AFTER prelock, cancellation only stops waiting: the
/// exact suspended commit future runs to completion on the captured runtime,
/// even before WAL acceptance. Do not blindly retry an unobserved outcome.
/// Inline panics propagate without restarting the future; detached panics
/// are logged and existing VersionGuard orphan recovery remains the fallback.
/// Shutdown must stop admission and quiesce commits before stopping Tokio.
/// AsyncIndex retains its separately spawned materialization tail, counted
/// through completion by the same quiescence barrier.
///
/// Thin metrics-only wrapper around [`commit_tx_inner`]: it records the
/// `on_tx_aborted_storage` metric on a PRE-commit storage abort and is
/// otherwise transparent.
///
/// Metric semantics shift (Vector I.3 / MED-A): post-Phase-4 storage
/// failures no longer surface as `Err(TxError::Storage)` aborts — they
/// become `Ok(TxOutcome { materialization: Deferred, .. })`. So every
/// `Err(TxError::Storage)` reaching this wrapper is by construction a
/// PRE-commit abort, and `on_tx_aborted_storage` correctly counts only
/// those. Deferred materialization is observable three ways:
/// `TxOutcome::materialization`, the `log::warn!` emitted in `materialize`,
/// and the `TxMetrics::txs_materialization_deferred` counter
/// (`on_tx_materialization_deferred`, fired in `commit_tx_inner` when
/// `materialize` returns `Deferred`).
///
/// There is NOTHING to do for HNSW staging on a pre-commit abort —
/// staged vectors live inside the `TxContext` (`staged_vectors`) and
/// vanish when the tx is dropped on the `Err` return below. That is the
/// RAII win: no per-tx buffer outlives the `TxContext`, so no broadcast
/// drain is needed (HIGH-6).
pub async fn commit_tx(tx: TxContext, repo: &RepoInstance) -> Result<TxOutcome, TxError> {
    match commit_tx_inner(tx, repo).await {
        Ok(mut outcome) => {
            // The inner driver has settled its own version and released all locks.
            // Cancelling this read-only wait cannot undo or interrupt the commit.
            // A commit that allocated a version is strictly above its snapshot
            // (snapshots read at W <= the allocation counter); the read-only and
            // empty-footprint fast paths return commit_version == snapshot_version
            // and allocated nothing, so there is nothing to wait for (and waiting
            // on a never-allocated version would never resolve).
            if outcome.commit_version > outcome.snapshot_version {
                let gate = repo.tx_gate().await?;
                if gate.await_visible(outcome.commit_version).await
                    == shamir_tx::repo_tx_gate::VisibleOutcome::ReopenRequired
                {
                    outcome.materialization = MaterializationState::Deferred;
                }
            }
            // D2 P1e — soft backpressure. The ack-path published only the
            // in-memory overlay; the value is durable in `history` only after
            // the background drainer replays its WAL entry. Under sustained
            // write pressure faster than the disk drains, the overlay + inflight
            // WAL tail grow unbounded. Park the committer here, AFTER the commit
            // is published (the tx is COMMITTED and observable on its return),
            // until the undrained gap falls back under the low-watermark. This
            // is the single choke point: every live commit path funnels through
            // this wrapper, and it is on the SUCCESS path only — aborts never
            // grew the overlay, so they never brake.
            apply_backpressure(repo, MAX_UNDRAINED_VERSIONS).await;
            Ok(outcome)
        }
        Err(mut e) => {
            if let TxError::WalDurability {
                commit_version,
                materialization,
                ..
            } = &mut e
            {
                let gate = repo.tx_gate().await?;
                if gate.await_visible(*commit_version).await
                    == shamir_tx::repo_tx_gate::VisibleOutcome::ReopenRequired
                {
                    *materialization = MaterializationState::Deferred;
                }
                apply_backpressure(repo, MAX_UNDRAINED_VERSIONS).await;
            }
            if let TxError::Storage(_) = &e {
                repo.tx_metrics().on_tx_aborted_storage();
            }
            Err(e)
        }
    }
}

/// D2 P1e — soft async backpressure on the undrained version gap.
///
/// The gap is `gate.last_committed() - gate.durable_watermark()`: the number
/// of versions that are visible (published to readers via the overlay) but not
/// yet durable in `history` (the background [`Drainer`](crate::tx::drainer::Drainer)
/// has not replayed their WAL entries). Each undrained version pins an overlay
/// entry + an inflight WAL marker, so an unbounded gap is an unbounded RAM /
/// WAL-tail leak.
///
/// ## Contract
/// * **Fast path (zero overhead):** if `gap <= high` it returns immediately —
///   one pair of atomic loads, no allocation, no wake, no await. This is the
///   common case under any non-pathological load, so a steady committer pays
///   nothing.
/// * **Brake (only under pressure):** once `gap > high`, the committer wakes
///   the drainer and parks on the gate's durable-progress [`Notify`], re-checking
///   the gap each time the drain makes progress. It resumes the instant the gap
///   drops below the LOW watermark `high / 2` (hysteresis: braking to exactly
///   `high` would re-trigger on the very next commit and thrash; draining down
///   to `high/2` amortizes the brake over many commits).
///
/// ## Lost-wakeup safety
/// Each loop iteration registers `gate.durable_notified()` (a `Notified` future)
/// BEFORE re-reading the gap, then awaits it. A concurrent `mark_durable`
/// between the gap read and the await cannot be missed: `notify_waiters` wakes
/// every already-registered waiter, and the future was registered first. We
/// also re-wake the drainer each iteration so a drainer that went idle (its own
/// `notify_one` permit consumed) is nudged back to work.
///
/// ## Deadlock safety (the load-bearing guard)
/// The drainer can get STUCK — a durable-write error (disk full, backend fault)
/// leaves `durable_watermark` frozen, so the gap never closes and the parking
/// loop would wait forever. To make a stuck drain a DEGRADATION (more RAM) and
/// never a HANG, the loop is bounded by a wall-clock budget
/// ([`BACKPRESSURE_MAX_WAIT`]). When the budget is exhausted it logs a warning
/// and RETURNS (proceeds without backpressure). Correctness is untouched — the
/// data is already committed and observable; only the overlay-bounding promise
/// is relaxed under a faulted drain, which is the right trade (never wedge a
/// committer on a broken disk). Each individual park is itself capped by a short
/// `sleep` in the `select!` so a single lost signal cannot stall a whole budget.
///
/// Takes `high` as a parameter so tests can drive the state machine with a tiny
/// artificial gap; production callers pass [`MAX_UNDRAINED_VERSIONS`].
pub(crate) async fn apply_backpressure(repo: &RepoInstance, high: u64) {
    let Ok(gate) = repo.tx_gate().await else {
        // No gate (repo teardown / construction race) — nothing to brake on.
        return;
    };

    // Fast path: cheap atomic loads, the overwhelmingly common case.
    let gap = gate
        .last_committed()
        .saturating_sub(gate.durable_watermark());
    if gap <= high {
        return;
    }

    // Hysteresis low-watermark: resume only once the drain has caught up to
    // half the threshold, so the brake amortizes over many commits instead of
    // re-firing on every subsequent commit (thrash).
    let low = high / 2;

    log::warn!(
        "tx backpressure ENGAGED: undrained gap {gap} > {high} (low-watermark {low}); \
         parking committer on durable progress"
    );

    let start = std::time::Instant::now();
    loop {
        // Nudge the drainer every iteration: it may have gone idle (its
        // `notify_one` permit already consumed by a prior pass).
        repo.drainer().wake();

        // Register the durable-progress future BEFORE re-reading the gap so a
        // concurrent `mark_durable` cannot slip a wakeup between the check and
        // the park (no lost wakeup).
        let notified = gate.durable_notified();
        tokio::pin!(notified);

        let gap = gate
            .last_committed()
            .saturating_sub(gate.durable_watermark());
        if gap <= low {
            log::debug!(
                "tx backpressure RELEASED: gap {gap} <= low-watermark {low} after {:?}",
                start.elapsed()
            );
            return;
        }

        // Deadlock guard: if the drain has not closed the gap within the
        // wall-clock budget it is stuck (durable write faulted). Proceed without
        // backpressure rather than hang the committer forever — RAM grows, data
        // is unaffected (already committed + observable).
        if start.elapsed() >= BACKPRESSURE_MAX_WAIT {
            log::warn!(
                "tx backpressure ABANDONED after {:?}: undrained gap still {gap} > low {low} — \
                 drain appears stuck; proceeding (overlay bound relaxed, data unaffected)",
                start.elapsed()
            );
            return;
        }

        // Park on durable progress, but cap each park with a short sleep so a
        // single missed signal (or a drainer that needs re-nudging) cannot
        // stall the whole budget — we loop, re-wake, re-check.
        tokio::select! {
            _ = &mut notified => {}
            _ = tokio::time::sleep(BACKPRESSURE_PARK_SLICE) => {}
        }
    }
}

/// Wall-clock ceiling on a single [`apply_backpressure`] call. Past this the
/// brake is abandoned (drain presumed stuck) and the committer proceeds — a
/// RAM-vs-hang trade that always favours liveness. Generous enough that a
/// healthy-but-slow disk drains within it under normal bursts.
const BACKPRESSURE_MAX_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

/// Per-park cap inside the backpressure loop. Bounds how long one `select!`
/// waits on the durable-progress signal before re-checking the gap and
/// re-nudging the drainer, so a single lost wakeup degrades to a short poll
/// rather than stalling the whole [`BACKPRESSURE_MAX_WAIT`] budget.
const BACKPRESSURE_PARK_SLICE: std::time::Duration = std::time::Duration::from_millis(50);

/// Orchestrates the commit pipeline around the WAL commit point.
///
/// The boundary is explicit: [`pre_commit`] runs Phases 1–4 and may
/// return a real abort `Err` (nothing durable yet). A *successful*
/// `pre_commit` means Phase 4 `wal.begin` landed — that durable WAL
/// entry IS the commit. From there [`materialize`] runs Phases 5–7
/// best-effort and NEVER aborts: a projection failure is logged and
/// deferred to recovery, the version is still published, and the tx is
/// reported COMMITTED (with `MaterializationState::Deferred`).
///
/// C6 empty-tx fast-path: when `pre_commit` returns `Ok(None)` (the tx
/// staged nothing durable, after SSI validation passed) this function
/// short-circuits — no version assigned, no WAL write, no publish — and
/// returns `Ok(TxOutcome { commit_version: snapshot_version, materialization:
/// Complete, .. })`.
async fn commit_tx_inner(mut tx: TxContext, repo: &RepoInstance) -> Result<TxOutcome, TxError> {
    // Level-3 wound-wait: a tx wounded by an older (higher-priority) tx must
    // abort even if it finished all its statements. The check runs at commit
    // entry so a wounded tx can never commit. No-op for Snapshot / Serializable
    // (their wounded flag stays false — they never take locks).
    if let Err(tx_version) = tx.ensure_not_wounded() {
        release_pessimistic_locks(&tx, repo).await;
        return Err(TxError::Wounded { tx_version });
    }

    let max_tx_lifetime = effective_max_tx_lifetime();
    if tx.is_expired(max_tx_lifetime) {
        release_pessimistic_locks(&tx, repo).await;
        repo.tx_metrics().on_tx_aborted_expired();
        return Err(TxError::Expired {
            elapsed: tx.elapsed(),
            max: max_tx_lifetime,
        });
    }

    // Group 15 (2026-08-14 review, SUMMARY.md 6.2): both resolutions can
    // hit REAL I/O on first touch (`repo_wal()` does `create_dir_all` +
    // `SegmentSet::open`) — mirror every other early-exit path in this
    // function and release Level-3 locks before propagating the error, so
    // a Pessimistic tx never leaks `locked_keys` when its commit happens
    // to be the repo's first WAL touch.
    let gate = match repo.tx_gate().await {
        Ok(g) => g,
        Err(e) => {
            release_pessimistic_locks(&tx, repo).await;
            return Err(e.into());
        }
    };
    if gate.requires_reopen() {
        release_pessimistic_locks(&tx, repo).await;
        return Err(TxError::WalRequiresReopen);
    }
    let wal = match repo.repo_wal().await {
        Ok(w) => w,
        Err(e) => {
            release_pessimistic_locks(&tx, repo).await;
            return Err(e.into());
        }
    };

    if wal.requires_reopen() {
        gate.set_requires_reopen();
        release_pessimistic_locks(&tx, repo).await;
        return Err(TxError::WalRequiresReopen);
    }

    // === PRE-LOCK SECTION (concurrent across committers) ===
    //
    // Phase 1 (interner overlay merge + remap) and Phase 2.5 + 2.6
    // (uwl_guards acquisition + unique re-validation) run OUTSIDE
    // `commit_lock`. Phase 1 is CAS-safe on DashMap (concurrent merges
    // converge). Phase 2.5/2.6 serialize on per-table uwl_guards in
    // sorted token order — no global serialization needed. Two committers
    // touching disjoint unique tables proceed fully in parallel here.
    let PreLockResult {
        uwl_guards,
        drain_guards,
    } = match pre_commit_prelock(&mut tx, repo).await {
        Ok(r) => r,
        Err(e) => {
            release_pessimistic_locks(&tx, repo).await;
            return Err(e);
        }
    };

    // Own the foreground clone, tx and prelock guards before either downstream
    // body takes commit_lock / the serializable guard. No spawn or yield on
    // the happy path; backpressure remains in the caller-only wrapper.
    crate::tx::CommitDriver::new(tx, repo, move |tx, repo| async move {
        let _tail = repo.commit_tail().enter();
        // AsyncIndex txs bypass group-commit: their post-commit tail is spawned
        // as a background task, which is incompatible with the leader processing
        // another tx's materialization. They always wait for the lock.
        if tx.visibility == shamir_tx::CommitVisibility::AsyncIndex {
            commit_tx_inner_legacy_async(
                tx,
                uwl_guards,
                drain_guards,
                &repo,
                gate.as_ref(),
                wal.as_ref(),
            )
            .await
        } else {
            // === P2c: LOCKFREE COMMIT PATH ===
            //
            // Disjoint-table commits run fully in parallel — no global
            // `commit_mutex`. Correctness relies on per-table uwl_guards:
            // same-table committers serialize at uwl acquisition (pre-lock);
            // disjoint-table committers never conflict in SSI predicates.
            //
            // Sequence (all lock-free or per-table-locked):
            //   1. validate (atomic version + lock-free TreeIndex scan)
            //   2. WAL begin (unique key per txn_id — concurrent-safe)
            //   3. record_commit_writes (lock-free TreeIndex insert)
            //   4. materialize (gated by uwl_guards, not commit_mutex)
            //
            // Group-commit WAL batching is sacrificed for parallelism; the
            // WAL backend's internal batching (if any) still amortizes fsync.
            commit_tx_lockfree(
                tx,
                uwl_guards,
                drain_guards,
                &repo,
                gate.as_ref(),
                wal.as_ref(),
            )
            .await
        }
    })
    .await
}

/// AsyncIndex commit path: uses the traditional sequential commit_lock.
/// Group-commit does not apply because the background-task tail is
/// incompatible with leader-processed materialization.
async fn commit_tx_inner_legacy_async(
    mut tx: TxContext,
    uwl_guards: Vec<tokio::sync::OwnedMutexGuard<()>>,
    drain_guards: Vec<crate::table::writer_drain_barrier::WriterDrainGuard>,
    repo: &RepoInstance,
    gate: &shamir_tx::RepoTxGate,
    wal: &shamir_tx::RepoWalManager,
) -> Result<TxOutcome, TxError> {
    let commit_guard = gate.commit_lock().await;

    let PreCommit {
        commit_version,
        uwl_guards,
        drain_guards,
        version_guard,
        cell_guards,
        wal_entry_arc,
        durability_failed,
    } = match pre_commit_locked(&mut tx, repo, gate, wal, uwl_guards, drain_guards).await {
        Ok(Some(pc)) => pc,
        Ok(None) => {
            repo.tx_metrics().on_tx_committed();
            release_pessimistic_locks(&tx, repo).await;
            drop(commit_guard);
            return Ok(TxOutcome {
                tx_id: tx.tx_id.0,
                snapshot_version: tx.snapshot_version,
                commit_version: tx.snapshot_version,
                materialization: MaterializationState::Complete,
                background: None,
            });
        }
        Err(e) => {
            release_pessimistic_locks(&tx, repo).await;
            return Err(e);
        }
    };
    // Op #2 Stage 2: offer the persisted WAL entry to the drainer window.
    repo.drainer().offer(wal_entry_arc);
    // SSI fix S2 — still-armed cell-reservation guards from pre_commit_locked
    // (WAL begin already succeeded inside it). Disarmed after the inline Phase 5a
    // (`apply_data_phase`) finalizes the cells below.
    let mut cell_guards = cell_guards;

    let snapshot_version = tx.snapshot_version;
    let tx_id_u64 = tx.tx_id.0;
    let changefeed_event = shamir_tx::project_event(&tx, repo.name(), commit_version);
    let prepared_journal = repo.prepare_journal_event(changefeed_event).await;

    apply_data_phase(&mut tx, repo, commit_version).await;
    let data_visible = !tx.async_prefix_failed;
    #[cfg(test)]
    fire_post_wal_pre_publish_test_hook(repo, false).await;
    // SSI fix S2 — Phase 5a (`apply_data_phase` → `apply_committed_visible`)
    // finalized every claimed cell (version published, `reserved_by` cleared).
    // Disarm so the guards' `Drop` is a no-op.
    for g in &mut cell_guards {
        g.disarm();
    }
    drop(cell_guards);
    super::materialize::apply_tracked_counter_phase(&tx, repo, commit_version).await;
    // #426 (VR-4 variant A): append the durable vector delta-log chunk(s)
    // BEFORE `version_guard.commit()` (pre-publish). The delta chunk is the
    // bridge `restore_on_open::replay_delta` re-materializes vectors from
    // on the next open; appending it pre-publish closes the W-2 window. The
    // GRAPH half (in-RAM HNSW mutation) stays in `promote_vectors` inside
    // the spawned tail (post-lock, III.5). A delta-append failure sets
    // `tx.async_prefix_failed` → the ack-path reports `Deferred` (NOT a
    // silent `Complete`) so the client can detect the missing durable echo.
    apply_vector_delta_phase(&mut tx, repo, commit_version).await;
    let delta_deferred = tx.async_prefix_failed;

    // Crash seam (test-only): fires BEFORE publish, mirroring the sync
    // path's `phase5d_delta` seam in `materialize.rs` — the durable delta
    // chunk is on disk but the version is not yet published and Phase 7
    // has not run. Placed here (before `version_guard.commit()`), not
    // after, so it actually proves what its name claims: a crash between
    // the pre-publish delta-append and publish still leaves the chunk
    // durable for `restore_on_open::replay_delta` to pick up.
    maybe_crash("phase5d_delta_async", repo).await;

    // The SSI footprint was recorded immediately after WAL acceptance,
    // before any cancellable materialization. Both normal publish and orphan
    // replay therefore expose a version whose footprint is already recorded.

    // Settle Phase 5a success, or transfer failed data visibility to orphan
    // replay. WAL acceptance alone must not let the snapshot watermark pass.
    let changefeed_event =
        super::materialize::settle_visible(repo, version_guard, prepared_journal, data_visible);

    // A4 (fix a): release Level-3 pessimistic locks AFTER the publish step
    // (`apply_data_phase` + `version_guard.commit()` above) is complete,
    // never before. 2PL requires every lock to stay held until the tx's
    // outcome is fully visible to others: releasing the Exclusive lock on a
    // key BEFORE its new value is published lets a second tx acquire the
    // lock and read/write the key while this tx's write is still in-flight
    // (WAL-durable but not yet cell-published) — a lost-update protocol
    // violation. Abort / early-exit paths above (lines ~435/440/462/515/526)
    // release immediately because no write was ever published there.
    release_pessimistic_locks(&tx, repo).await;

    maybe_crash("phase6_async", repo).await;
    drop(commit_guard);

    #[cfg(test)]
    repo.fire_post_publish_pre_broadcast_test_hook().await;
    repo.broadcast_changefeed_event(changefeed_event);

    let repo_clone = repo.clone();
    let metrics = repo.tx_metrics().clone();
    // Enter before spawn while the inline driver is still counted: no zero-count gap.
    let tail_guard = repo.commit_tail().enter();
    let join = tokio::spawn(async move {
        let _tail_guard = tail_guard;
        fire_async_index_tail_test_hook(&repo_clone).await;
        let state = materialize_async_tail(
            &mut tx,
            &repo_clone,
            commit_version,
            uwl_guards,
            drain_guards,
        )
        .await;
        if state == MaterializationState::Deferred {
            metrics.on_tx_materialization_deferred();
        }
        // D2 P1d-2b CUTOVER: the inline `gate.mark_durable(commit_version)` is
        // GONE here too. The ack-path wrote only the overlay (visible half);
        // the value becomes durable in `history` only after the background
        // drainer replays the WAL entry. The drainer owns `mark_durable` + WAL
        // truncation. Wake it after the version is published so it drains the
        // freshly-committed tail promptly.
        repo_clone.drainer().wake();
        promote_vectors(&tx, &repo_clone, commit_version).await;
        state
    });

    repo.tx_metrics().on_tx_committed();

    // #426 (VR-4 variant A): a pre-publish delta-append failure
    // (`tx.async_prefix_failed` set by `apply_vector_delta_phase`) surfaces
    // as `Deferred` in the ack-path TxOutcome so the client detects the
    // missing durable vector echo and can retry — NOT a silent `Complete`.
    let materialization = if delta_deferred {
        repo.tx_metrics().on_tx_materialization_deferred();
        MaterializationState::Deferred
    } else {
        MaterializationState::Complete
    };

    if durability_failed {
        let tail = join.await.unwrap_or(MaterializationState::Deferred);
        let materialization = if delta_deferred || tail == MaterializationState::Deferred {
            MaterializationState::Deferred
        } else {
            MaterializationState::Complete
        };
        return Err(TxError::WalDurability {
            commit_version,
            materialization,
        });
    }
    Ok(TxOutcome {
        tx_id: tx_id_u64,
        snapshot_version,
        commit_version,
        materialization,
        background: Some(BackgroundCommitHandle { join }),
    })
}

/// P2c lockfree commit path: no global `commit_mutex`.
///
/// Per-table uwl_guards (acquired in pre_commit_prelock in sorted token
/// order) provide the serialization invariant: same-table committers
/// serialize at uwl acquisition; disjoint-table committers proceed fully
/// in parallel. The SSI footprint (`record_commit_writes`) is inserted
/// into the lock-free `scc::TreeIndex` BEFORE `publish_committed_max`
/// advances the reader-visible floor, so future SSI validators see it.
async fn commit_tx_lockfree(
    mut tx: TxContext,
    uwl_guards: Vec<tokio::sync::OwnedMutexGuard<()>>,
    drain_guards: Vec<crate::table::writer_drain_barrier::WriterDrainGuard>,
    repo: &RepoInstance,
    gate: &shamir_tx::RepoTxGate,
    wal: &shamir_tx::RepoWalManager,
) -> Result<TxOutcome, TxError> {
    // CRIT-4 (#438): Serializable txs must serialize the validate→publish
    // window. The lock-free path relies on per-table `uwl_guards` for
    // serialization, but those are acquired ONLY for tables with unique
    // constraints (`pre_commit_prelock`). A Serializable tx with NO unique
    // constraints is not serialized anywhere in this path, so two such txs
    // with disjoint write-sets but a rw-antidependency cycle (classic
    // write-skew: A reads x / writes y, B reads y / writes x) can BOTH pass
    // `pre_commit_locked_validate` and then BOTH publish — violating
    // Serializable.
    //
    // Fix: take `gate.commit_lock()` for the validate→publish window when
    // the tx is Serializable, mirroring exactly the section the legacy
    // (`commit_tx_inner_legacy_async`) path already holds under the same
    // mutex. Snapshot txs do NOT need SSI validation and keep the full
    // lock-free parallelism — the branch below is a pure `if` with no lock
    // taken on the Snapshot path (zero overhead).
    //
    // FG-7: ALSO take the lock when `cas_set` is non-empty, regardless of
    // isolation. A CAS tx (even under Snapshot) needs the SAME
    // validate→publish atomicity Serializable already relies on — otherwise
    // two Snapshot CAS writers could both pass the new Phase CAS check and
    // then both publish, reproducing the exact race this task closes. A
    // Snapshot tx with NO CAS keys is completely unaffected (empty-set
    // check, no lock taken).
    //
    // No deadlock / self-block risk: `commit_tx_lockfree` has a single call
    // site (`commit_tx_inner` line ~490), which itself never holds
    // `commit_lock`. The AsyncIndex path (`commit_tx_inner_legacy_async`)
    // acquires `commit_lock` itself but RETURNS at line ~471 before the
    // control flow reaches this function, so no caller arrives here already
    // holding the mutex. `tokio::sync::Mutex` is non-reentrant, so this
    // invariant is load-bearing.
    // F-40b (RI barrier): ALSO take the lock when `ri_barrier_tokens` is
    // non-empty — a Snapshot tx that recorded an FK-child scan needs the
    // SAME validate→publish window serialization Serializable already
    // relies on, so the barrier's `predicate_conflicts_batch` scan sees a
    // stable commit window (mirrors the Serializable branch's rationale
    // above). A Snapshot tx with no barrier tokens is completely unaffected
    // (the `is_empty` check short-circuits before any lock work).
    //
    // F-46 (#857, P0 — 2026-07-28 readonly review's P0-1): ALSO take the
    // lock when `footprint_tokens` is non-empty. Without this, the RI
    // barrier above is one-sided: the PARENT-side RI-barrier check takes
    // `commit_lock` and re-validates against the commit window, but a
    // concurrent Snapshot WRITER into the same FK-child table only ever
    // gets `footprint_tokens` populated (via `require_footprint_for`,
    // called unconditionally by `require_footprint_if_fk_child` at every
    // insert/update/set into an FK-child table) — never
    // `ri_barrier_tokens` (that field is written only by the reverse-FK
    // *scan* sites in `fk_restrict.rs`/`fk_actions.rs`/`fk_on_update.rs`,
    // which the plain writer never runs). Before this widening, such a
    // writer could publish (WAL write + `record_commit_writes` +
    // materialize) WHILE the parent held `commit_lock` and had already
    // passed its own Phase 2-bis `predicate_conflicts_batch` check —
    // slipping in behind the barrier's back and defeating its
    // mutual-exclusion intent (proven by
    // `fk_ri_barrier_tests.rs`'s
    // `concurrent_child_publish_during_parent_commit_window_is_serialized_on_delete_{restrict,cascade,set_null}`
    // / `..._on_update`, using the `TEST_POST_VALIDATE_PRE_PUBLISH_HOOK`
    // seam above to park a commit-under-test strictly inside this exact
    // window). Widening this condition makes every FK-child writer
    // participate in the SAME validate→publish serialization the RI
    // barrier already relies on, closing the gap symmetrically: whichever
    // side reaches `commit_lock` first now genuinely excludes the other
    // for the whole window, not just for the barrier's own read. A
    // Snapshot tx that never touches an FK-child table has empty
    // `footprint_tokens` and pays only the pre-existing single `is_empty`
    // check — zero additional cost on that path.
    let _serializable_guard = if tx.isolation == IsolationLevel::Serializable
        || !tx.cas_set.is_empty()
        || !tx.ri_barrier_tokens_is_empty()
        || !tx.footprint_tokens.is_empty()
    {
        Some(gate.commit_lock().await)
    } else {
        None
    };

    // Phase 3 + 2 + 2-bis + WAL entry build (no lock needed).
    let validated =
        match pre_commit_locked_validate(&mut tx, repo, gate, uwl_guards, drain_guards).await {
            Ok(Some(v)) => v,
            Ok(None) => {
                // C6 empty-tx fast-path.
                repo.tx_metrics().on_tx_committed();
                release_pessimistic_locks(&tx, repo).await;
                return Ok(TxOutcome {
                    tx_id: tx.tx_id.0,
                    snapshot_version: tx.snapshot_version,
                    commit_version: tx.snapshot_version,
                    materialization: MaterializationState::Complete,
                    background: None,
                });
            }
            Err(e) => {
                release_pessimistic_locks(&tx, repo).await;
                return Err(e);
            }
        };

    let commit_version = validated.commit_version;
    let mut version_guard = validated.version_guard;
    version_guard.attach_metrics(repo.tx_metrics().clone());
    // SSI fix S2 — the still-armed cell-reservation guards. Held across Phase 4
    // and `materialize`; `disarm`ed after publish (Phase 5a `finalize_reservation`
    // already cleared each `reserved_by`). On the WAL-begin abort below they drop
    // → release every claimed cell (I-PreWAL: a loser never strands a claim).
    let mut cell_guards = validated.cell_guards;

    // F-46 (#857) test-only seam: fires strictly after Phase 2-bis's
    // `predicate_conflicts_batch` (the RI barrier's commit-time re-check) has
    // already passed, and strictly before Phase 4's WAL begin / publish. See
    // `fire_post_validate_pre_publish_test_hook`'s doc. No-op unless a test
    // installed `TEST_POST_VALIDATE_PRE_PUBLISH_HOOK`.
    fire_post_validate_pre_publish_test_hook().await;
    fire_cancel_test_hook(repo, true).await;

    // Phase 4: WAL begin — THE COMMIT POINT.
    // Concurrent-safe: each tx writes a unique WAL key (txn_id).
    // Record before queueing: an unwritten abort may cause spurious SSI
    // conflicts, but a written orphan must never expose a missed conflict.
    gate.record_commit_writes(shamir_tx::build_footprint_from_tx(&tx, commit_version));
    let queued = wal.begin_grouped_ticketed(&validated.wal_entry_arc, commit_wal_durability(repo));
    let (ticket, completion) = match queued {
        Ok(queued) => queued,
        Err(e) => {
            release_pessimistic_locks(&tx, repo).await;
            return Err(e.into());
        }
    };
    version_guard.wal_queued(ticket, validated.wal_entry_arc.clone());
    let durability_failed = match wal_completion(gate, completion.await) {
        Ok(failed) => failed,
        Err(e) => {
            drop(version_guard);
            drop(cell_guards);
            release_pessimistic_locks(&tx, repo).await;
            return Err(e);
        }
    };

    // Op #2 Stage 2: offer the persisted WAL entry to the drainer window.
    repo.drainer().offer(validated.wal_entry_arc);

    maybe_crash("phase4", repo).await;

    // SSI footprint already recorded at WAL acceptance above, before replay
    // could take ownership of a cancelled commit's version.

    repo.tx_metrics().on_tx_committed();

    // Phases 5–7: materialize OUTSIDE any global lock, gated by uwl_guards.
    let snapshot_version = tx.snapshot_version;
    let tx_id_u64 = tx.tx_id.0;
    let changefeed_event = shamir_tx::project_event(&tx, repo.name(), commit_version);
    let prepared_journal = repo.prepare_journal_event(changefeed_event).await;

    let (post_publish, changefeed_event) = materialize(
        &mut tx,
        repo,
        version_guard,
        prepared_journal,
        validated.uwl_guards,
        validated.drain_guards,
    )
    .await;
    // SSI fix S2 — publish (Phase 5a `apply_committed_visible`) ran inside
    // `materialize` and `finalize_reservation`'d every claimed cell (version
    // published, `reserved_by` cleared). Disarm the guards so their `Drop` does
    // NOT redundantly release: the claims are gone and the version is the cell's
    // published state now. (A deferred Phase 5a still cleared the reservation for
    // the keys it published; any not-yet-finalized key is released by drop —
    // harmless, the cell version is the source of truth either way.)
    for g in &mut cell_guards {
        g.disarm();
    }
    drop(cell_guards);
    // CRIT-4 (#438): release the Serializable serialization window now that
    // publish (inside `materialize` above) is complete. Holding past this
    // point would needlessly block the next Serializable committer through
    // `finalize_sync_post_publish`, which is post-publish tail work.
    drop(_serializable_guard);

    // A4 (fix a): release Level-3 pessimistic locks AFTER the publish step
    // (`materialize` above, which performs Phase 5a `apply_committed_visible`
    // + `finalize_reservation` + `version_guard.commit()`) is complete, never
    // before. 2PL requires every lock to stay held until the tx's outcome is
    // fully visible to others: releasing the Exclusive lock on a key BEFORE
    // its new value is published lets a second tx acquire the lock and
    // read/write the key while this tx's write is still in-flight
    // (WAL-durable but not yet cell-published) — a lost-update protocol
    // violation. Abort / early-exit paths above (lines ~679/689/716) release
    // immediately because no write was ever published there.
    release_pessimistic_locks(&tx, repo).await;

    let materialization = finalize_sync_post_publish(
        &tx,
        post_publish,
        changefeed_event,
        repo,
        gate,
        commit_version,
    )
    .await;

    if durability_failed {
        return Err(TxError::WalDurability {
            commit_version,
            materialization,
        });
    }
    Ok(TxOutcome {
        tx_id: tx_id_u64,
        snapshot_version,
        commit_version,
        materialization,
        background: None,
    })
}

/// Release every Level-3 pessimistic lock the tx holds, grouped by table.
///
/// Called on BOTH the commit-success path and every abort path of
/// [`commit_tx_inner`]. `locked_keys` is empty for Snapshot / Serializable
/// txs (they never acquire locks), so this is a no-op for them — zero
/// overhead on the non-Pessimistic commit paths. Each `(table_token, key)`
/// is routed to the corresponding table's `MvccStore::release_locks`.
pub(crate) async fn release_pessimistic_locks(tx: &TxContext, repo: &RepoInstance) {
    if tx.locked_keys.is_empty() {
        return;
    }
    // Group keys by table_token so each MvccStore is hit once. `locked_keys`
    // is an `scc::HashMap<(u64, RecordKey), ()>` (task #532); scan into a std
    // map (the synchronous visitor cannot await inside). Keys are `RecordKey`
    // throughout — `release_locks` is `RecordKey`-keyed, no `Bytes` round-trip.
    // O(N) ack: per-tx locked-keys sizing, bounded by this tx's footprint.
    #[allow(clippy::disallowed_methods)]
    let mut by_table: TFxMap<u64, Vec<shamir_storage::types::RecordKey>> =
        TFxMap::with_capacity_and_hasher(tx.locked_keys.len(), THasher::default());
    tx.locked_keys.iter_sync(|(token, key), _| {
        by_table.entry(*token).or_default().push(key.clone());
        true
    });
    let mvcc_map = repo.per_table_mvcc();
    for (token, keys) in by_table {
        // Clone the Arc out: a `get_sync` entry guard would hold the bucket
        // lock across the `.await` (#589 class).
        let mvcc = mvcc_map.read_sync(&token, |_, mvcc| std::sync::Arc::clone(mvcc));
        if let Some(mvcc) = mvcc {
            mvcc.release_locks(tx.tx_id.0, &keys).await;
        }
    }
}
