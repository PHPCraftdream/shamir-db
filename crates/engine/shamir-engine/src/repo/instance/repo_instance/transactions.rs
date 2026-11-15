use crate::query::batch::BatchError;
use crate::repo::version_provider::repo_token;
use shamir_engine_query::query::write::WriteResult;
use shamir_query_types::batch::VisibilityWait;
use shamir_storage::error::DbResult;
use shamir_types::access::Actor;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use super::RepoInstance;

mod follower_durability;
mod receipt_worker_impl;
mod write_domain_membership;

impl RepoInstance {
    /// Per-repo transaction gate; the body lives on `RepoCore::tx_gate`.
    pub async fn tx_gate(&self) -> DbResult<Arc<shamir_tx::RepoTxGate>> {
        self.core.tx_gate().await
    }

    /// Returns the eager latest-state publication floor for this repo.
    ///
    /// Lower allocated versions may still be unsettled. Keep this value for
    /// latest-state/replication diagnostics, not snapshot positions or seeds;
    /// use [`consistent_version`](Self::consistent_version) for those.
    ///
    /// cancel-safe: yes — delegates to cancellation-safe gate initialization.
    pub async fn current_commit_version(&self) -> DbResult<u64> {
        let gate = self.tx_gate().await?;
        Ok(gate.last_committed())
    }

    /// Highest contiguous settled version, suitable for snapshot positions.
    /// Every version through this position is materialized or aborted.
    ///
    /// cancel-safe: yes — delegates to cancellation-safe gate initialization.
    pub async fn consistent_version(&self) -> DbResult<u64> {
        let gate = self.tx_gate().await?;
        Ok(gate.visible_floor())
    }

    /// cancel-safe: yes — `OnceCell::get_or_try_init` semantics. If
    /// cancellation occurs during init, the cell remains uninitialised
    /// and the next caller retries. Returned Arc clone is in-memory.
    ///
    /// Returns the per-repo WAL manager, lazily initialising it on first
    /// call. Shares the `"__tx__"` info store with `tx_gate`.
    ///
    /// The `next_txn_id` counter is seeded from
    /// `max(persisted_NextTxId, max_inflight_txn_id + 1)`. The floor is
    /// computed from `recover()` (segment/Mem replay) — empty on a fresh
    /// in-memory instance (safeguard inert), the durable entries on a disk
    /// repo. The append-only segment keys entries by frame position and
    /// recovery replays by the monotonically-floored `commit_version`, so
    /// even a repeated txn_id after a crash carries no ordering ambiguity
    /// or data loss; the floor is a belt-and-braces anti-collision guard.
    pub async fn repo_wal(&self) -> DbResult<Arc<shamir_tx::RepoWalManager>> {
        self.core.repo_wal().await
    }

    /// cancel-safe: yes — bumps a tx-start counter (atomic), opens a
    /// snapshot (single scc insert under the hood) and returns. If the
    /// caller is dropped mid-await before receiving the guard, the
    /// stack-local guard is dropped which removes the snapshot from the
    /// active set. The only persistent footprint is an atomic counter
    /// increment in `tx_metrics`; tx-ids/version-counters drift safely
    /// (monotonic, no reuse).
    ///
    /// Open a fresh transaction on this repo.
    ///
    /// Returns a `(TxContext, SnapshotGuard)` pair. The guard's lifetime
    /// must extend at least until commit (or drop = rollback) — drop
    /// removes the snapshot from the active set so GC can reclaim
    /// versions older than `min_alive`.
    ///
    /// `repo_id` in the TxContext is populated via [`repo_token`].
    pub async fn begin_tx(
        &self,
        isolation: shamir_tx::IsolationLevel,
    ) -> DbResult<(shamir_tx::TxContext, shamir_tx::SnapshotGuard)> {
        self.core.horizon.tx_metrics().on_tx_start();
        let gate = self.tx_gate().await?;
        // Open the snapshot BEFORE constructing TxContext so that any
        // concurrent non-tx write sees active_serializable_count > 0 for
        // the full lifetime of this tx's predicate window.
        let guard = if isolation == shamir_tx::IsolationLevel::Serializable {
            gate.open_snapshot_serializable().await
        } else {
            gate.open_snapshot().await
        };
        let snapshot_version = guard.version();
        let tx_id = gate.fresh_tx_id();
        let mut tx =
            shamir_tx::TxContext::new(tx_id, repo_token(&self.name), snapshot_version, isolation);

        // FG-7: attach the version provider UNCONDITIONALLY (every isolation
        // level), not just Serializable. `pre_commit_locked_validate`'s new
        // Phase CAS needs `version_of` for `expected_version` re-validation
        // at commit regardless of isolation — a Snapshot tx with no CAS
        // checks pays only a thin `Arc` construction + clone, no behavior
        // change to Phase 2 (still gated on `isolation == Serializable`).
        let provider = std::sync::Arc::new(crate::repo::RepoVersionProvider {
            per_table_mvcc: Arc::clone(&self.core.catalog.per_table_mvcc),
        });
        tx.set_version_provider(provider);

        Ok((tx, guard))
    }

    /// cancel-safe: partial — cancellation during prelock (including unique-lock
    /// waits) is still a clean abort. AFTER prelock, cancellation only stops the
    /// caller's wait: the exact suspended commit continues on the runtime, even
    /// before WAL acceptance. Do not blindly retry an unobserved outcome.
    /// Inline panics propagate; detached panics are logged, with VersionGuard
    /// orphan recovery as the fallback. Stop admission and await
    /// [`quiesce_commits`](Self::quiesce_commits) before stopping the runtime.
    /// See [`crate::tx::commit_tx`] for the full contract.
    ///
    /// Commit a transaction via the 7-phase commit pipeline.
    ///
    /// Wrapper around [`crate::tx::commit_tx`]. The free function is
    /// the canonical implementation; this method exposes it on the
    /// natural semantic owner.
    pub async fn commit_tx(
        &self,
        tx: shamir_tx::TxContext,
    ) -> Result<crate::tx::TxOutcome, crate::tx::CommitError> {
        self.commit_tx_with_options(tx, self.commit_visibility_wait())
            .await
    }

    /// Select the default for receipt-bearing transaction commits at construction.
    /// Configure before sharing this handle; subsequent clones copy this policy.
    /// This is handle-local configuration, not shared mutable repository state:
    /// changing one clone leaves existing clones (including registered handles)
    /// unchanged even though they share the same gate and storage.
    /// Value-only implicit commits remain Strict until VIS-4 provides receipts.
    pub fn with_commit_visibility_wait(mut self, wait: VisibilityWait) -> Self {
        self.core.horizon.set_commit_visibility_wait(wait);
        self
    }

    pub fn commit_visibility_wait(&self) -> VisibilityWait {
        self.core.commit_visibility_wait()
    }

    /// Commit with an explicit acknowledgement policy, resolved before the pipeline.
    /// The bounded budget excludes inner execution and starts at its known outcome.
    /// cancel-safe: partial — same inner commit ownership as commit_tx; dropping
    /// only acknowledgement cannot abort materialization or discharge recovery.
    pub async fn commit_tx_with_options(
        &self,
        tx: shamir_tx::TxContext,
        wait: VisibilityWait,
    ) -> Result<crate::tx::TxOutcome, crate::tx::CommitError> {
        crate::tx::commit::commit_tx_with_options(tx, &self.core, wait).await
    }

    /// Borrow rather than clone the retained gate on the acknowledgement fast path.
    pub(crate) fn initialized_tx_gate(&self) -> Option<&shamir_tx::RepoTxGate> {
        self.core.initialized_tx_gate()
    }

    /// R1-b — read the durable per-(db,repo) follower replication bookmark.
    ///
    /// Returns the highest LEADER `commit_version` this follower has
    /// durably recorded as applied on this repo. Returns `0` on a fresh
    /// repo (the marker has never been written). Receipt checkpointing
    /// advances the durable bookmark; the value is used for O(1)
    /// idempotent re-delivery gating.
    ///
    /// Stored in the repo's `__tx__` info-store under
    /// [`MetaKey::ReplicationBookmark`](crate::meta::MetaKey::ReplicationBookmark)
    /// via the same envelope codec as `LastCommittedVersion` / `NextTxId`.
    /// See `crate::meta::repl_bookmark` for the codec rationale and the
    /// separate-marker rationale (leader-version space ≠ local-version
    /// space).
    pub async fn replication_bookmark(&self) -> DbResult<u64> {
        let info_store = self.tx_info_store().await?;
        crate::meta::repl_bookmark::load_replication_bookmark(&info_store).await
    }

    /// Run a single non-tx write as an implicit single-op BATCH transaction.
    ///
    /// The body lives in `query/batch/control/implicit.rs`.
    pub async fn run_implicit_batch_tx<F>(
        &self,
        actor: Actor,
        alias: &str,
        stage: F,
    ) -> Result<WriteResult, BatchError>
    where
        F: for<'t> FnOnce(
            &'t mut shamir_tx::TxContext,
        )
            -> Pin<Box<dyn Future<Output = DbResult<WriteResult>> + Send + 't>>,
    {
        crate::query::batch::run_implicit_batch_tx(self, actor, alias, stage).await
    }

    /// Open the implicit Snapshot batch tx; the body lives in
    /// `query/batch/control/implicit.rs`.
    pub async fn begin_implicit_batch_tx(
        &self,
        actor: Actor,
        alias: &str,
    ) -> Result<(shamir_tx::TxContext, shamir_tx::SnapshotGuard), BatchError> {
        crate::query::batch::begin_implicit_batch_tx(self, actor, alias).await
    }

    /// Open an implicit batch tx with a caller-chosen isolation level; the
    /// body lives in `query/batch/control/implicit.rs`.
    pub async fn begin_implicit_batch_tx_with_isolation(
        &self,
        actor: Actor,
        alias: &str,
        isolation: shamir_tx::IsolationLevel,
    ) -> Result<(shamir_tx::TxContext, shamir_tx::SnapshotGuard), BatchError> {
        crate::query::batch::begin_implicit_batch_tx_with_isolation(self, actor, alias, isolation)
            .await
    }

    /// Commit an implicit batch tx; the body lives in
    /// `query/batch/control/implicit.rs`.
    pub async fn commit_implicit_batch_tx(
        &self,
        tx: shamir_tx::TxContext,
        alias: &str,
    ) -> Result<shamir_query_types::batch::TransactionInfo, BatchError> {
        crate::query::batch::commit_implicit_batch_tx(self, tx, alias).await
    }
}
