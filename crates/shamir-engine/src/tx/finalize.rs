//! Shared finalization tail for the synchronous commit paths.
//!
//! PR3 (Fowler preparatory refactoring — "make the change easy, then make
//! the easy change"): the three commit paths (`commit_tx_lockfree`,
//! `run_single_tx`, and the `run_leader` batch loop) all share the exact
//! same post-`materialize` tail:
//!
//! `post_publish_cleanup` → deferred-metric → `drainer().wake()` →
//! `emit_changefeed_event` → `promote_vectors`
//!
//! R1 (`apply_replicated`) will invoke this same tail with the version
//! from a replicated commit event, instead of re-implementing it a fourth
//! time.
//!
//! ## Why `commit_tx_inner_legacy_async` is NOT a caller
//!
//! The AsyncIndex path diverges on three semantically load-bearing axes:
//!  1. Its SSI footprint (`record_commit_writes`) runs AFTER
//!     `version_guard.commit()` (and AFTER the inline `apply_data_phase`
//!     published the overlay), whereas the sync paths record the footprint
//!     BEFORE `materialize` publishes. Unifying would change the
//!     footprint-vs-publish ordering invariant on one of the paths.
//!  2. It emits the changefeed on the caller thread BEFORE spawning the
//!     background materialize tail; the sync paths emit AFTER `materialize`.
//!  3. Its tail (index → markers → promote) runs in a spawned background
//!     task and returns a `BackgroundCommitHandle`; the sync paths run
//!     inline and return `background: None`.
//!
//! Folding these into one function would require boolean flags + branch
//! divergence — a leaky abstraction, not a clean seam. The honest shared
//! core is the sync post-publish tail below.
//!
//! All phases here run OUTSIDE `commit_lock` (P2b) and are pure
//! post-commit bookkeeping — the version is already published (Phase 6 ran
//! inside `materialize` via `version_guard.commit()`). None of this may
//! abort the tx.

use shamir_tx::{RepoTxGate, TxContext};

use crate::repo::RepoInstance;
use crate::tx::commit_phases::promote_vectors;
use crate::tx::materialize::{post_publish_cleanup, PostPublishState};
use crate::tx::tx_outcome::MaterializationState;

/// Run the shared synchronous post-publish finalization tail.
///
/// Sequence (all outside `commit_lock`):
///  1. `post_publish_cleanup` — Phase 6.5 recovery markers + A5 interner
///     checkpoint (fire-and-forget). Returns `Complete` or `Deferred`.
///  2. Fire `on_tx_materialization_deferred` if the marker write deferred.
///  3. `drainer().wake()` — nudge the background drainer so the freshly-
///     published version's WAL entry is replayed into `history` promptly
///     (the ack-path wrote only the in-memory overlay; durability is the
///     drainer's job post-D2-cutover).
///  4. `emit_changefeed_event` — publish the tx's record-level changefeed
///     event (if any) to live subscribers.
///  5. `promote_vectors` — Phase 5d, promote staged HNSW vectors into the
///     live graph OUTSIDE the commit critical section (III.5). A failure
///     here is NOT `Deferred` (the graph reconciles via rebuild-on-open).
///
/// `tx` is borrowed (read-only) for `promote_vectors`; `post_publish_state`
/// and `changefeed_event` are consumed. Returns the final
/// [`MaterializationState`] for the caller's `TxOutcome`.
#[inline]
pub(super) async fn finalize_sync_post_publish(
    tx: &TxContext,
    post_publish_state: PostPublishState,
    changefeed_event: Option<shamir_tx::ChangelogEvent>,
    repo: &RepoInstance,
    gate: &RepoTxGate,
    commit_version: u64,
) -> MaterializationState {
    let materialization = post_publish_cleanup(post_publish_state, repo, gate).await;
    if materialization == MaterializationState::Deferred {
        repo.tx_metrics().on_tx_materialization_deferred();
    }
    // D2 P1d-2b CUTOVER: the inline `gate.mark_durable(commit_version)` is
    // GONE. The ack-path no longer writes `history` (only the overlay), so
    // the value is NOT durable at this point — it is durable only after the
    // background drainer replays the WAL entry into `history`. The DRAINER
    // now owns both `mark_durable` and the WAL truncation. We only WAKE it
    // here, after the version is published (visibility), so it drains
    // promptly.
    repo.drainer().wake();
    repo.emit_changefeed_event(changefeed_event).await;
    promote_vectors(tx, repo, commit_version).await;
    materialization
}
