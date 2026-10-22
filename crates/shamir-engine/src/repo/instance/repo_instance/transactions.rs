use crate::meta::repl::gate_provenance::with_follower_provenance as provenance_gate;
use crate::query::batch::BatchError;
use crate::query::write::WriteResult;
use crate::repo::version_provider::repo_token;
use shamir_query_types::batch::VisibilityWait;
use shamir_storage::error::{DbError, DbResult};
use shamir_storage::types::Repo;
use shamir_types::access::Actor;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use super::RepoInstance;

mod follower_durability;
mod write_domain_membership;

impl RepoInstance {
    /// cancel-safe: yes — `OnceCell::get_or_try_init` semantics. If
    /// cancellation occurs during init, the cell remains uninitialised
    /// and the next caller retries. Returned Arc clone is in-memory.
    ///
    /// Returns the per-repo transaction gate, lazily initialising it on
    /// first call.
    ///
    /// The gate is seeded from durable recovery markers stored under the
    /// repo's `"__tx__"` info store:
    /// - `last_committed_version` from `MetaKey::LastCommittedVersion`
    /// - `next_tx_id` from `MetaKey::NextTxId`
    ///
    /// Recovery marker reads are best-effort — if absent we start from
    /// defaults (`RepoTxGate::fresh()`).
    ///
    /// CRIT-B: the persisted `last_committed_version` marker can lag the
    /// highest `commit_version` of an *inflight* V2 WAL entry — the
    /// commit pipeline stamps `commit_version` in Phase 4 (WAL begin) but
    /// only persists the marker in Phase 6.5. A crash in that window
    /// leaves the marker stale (e.g. 7) while a durable inflight entry
    /// carries `commit_version = 10`. Seeding the gate solely from the
    /// marker would let `assign_next_version()` re-issue 8, 9, 10 —
    /// versions the crashed (and about-to-be-recovered) tx already
    /// consumed — violating version monotonicity. We therefore pre-scan
    /// the inflight entries and seed the gate's counter from
    /// `max(marker, max_inflight_commit_version)`. `RepoTxGate::new`
    /// initialises BOTH the monotonic `version_counter` and
    /// `last_committed_version` to this floor, so the next
    /// `assign_next_version()` is strictly greater than every version a
    /// recovered entry will replay at. The scan is the same cheap
    /// `recover()` replay recovery runs and happens once per gate (OnceCell).
    pub async fn tx_gate(&self) -> DbResult<Arc<shamir_tx::RepoTxGate>> {
        self.tx_gate
            .get_or_try_init(|| async {
                let info_store = self.repo.store_get("__tx__").await?;

                let marker = crate::meta::recovery_marker::load_last_committed(&info_store)
                    .await?
                    .unwrap_or(0);
                let next_tx_id =
                    crate::meta::recovery_marker::load_next_tx_id_snapshot(&info_store)
                        .await?
                        .unwrap_or(1);

                // Pre-scan inflight V2 entries so the version floor covers
                // any commit_version stamped before its marker was
                // persisted (CRIT-B). Best-effort: a WAL read error here
                // falls back to the marker rather than blocking gate
                // construction — recovery (which propagates errors) runs
                // separately on the open path.
                let max_inflight = self.max_inflight_commit_version().await.unwrap_or(0);
                let checkpoint =
                    crate::repo::durability_policy::checkpoint_allocation_floor(self).await?;
                let last_committed = marker.max(max_inflight).max(checkpoint);
                let gate = provenance_gate(last_committed, next_tx_id, &info_store).await?;
                // Best-effort: a WAL-init failure must NOT fail the gate
                // init. The old contract (see commit_wal_init_failure tests)
                // keeps the repo_wal OnceCell uninitialised and retryable —
                // the failure surfaces only at commit_tx's repo_wal() call.
                if let Ok(group) = self.wal_group().await {
                    if let Some(coverage) = group.coverage_observer() {
                        gate.completion().install_abort_coverage(coverage);
                    }
                }
                Ok::<Arc<shamir_tx::RepoTxGate>, DbError>(gate)
            })
            .await
            .cloned()
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

    /// Highest `commit_version` across all durable inflight V2 WAL
    /// entries, or `0` if there are none. Used to seed the tx gate's
    /// version floor (CRIT-B) so recovered commit versions are never
    /// re-issued.
    pub(crate) async fn max_inflight_commit_version(&self) -> DbResult<u64> {
        let wal = self.repo_wal().await?;
        let entries = wal.recover().await?;
        Ok(entries.iter().map(|e| e.commit_version).max().unwrap_or(0))
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
        self.repo_wal
            .get_or_try_init(|| async {
                let info_store = self.repo.store_get("__tx__").await?;
                let initial_txn_id =
                    crate::meta::recovery_marker::load_next_tx_id_snapshot(&info_store)
                        .await
                        .unwrap_or(None)
                        .unwrap_or(1);
                // F5e: build a `WalGroupCommit` for EVERY repo — `File` sink
                // for disk repos (sibling `<name>.shamirwal/` directory via
                // `file_name()` — NOT `OsString::push` on the full path, which
                // breaks on trailing separators), `Mem` sink (in-RAM Vec) for
                // in-memory repos. The group is always present, so the live
                // commit path is one code path: `begin_grouped(Buffered)` to
                // append, `recover()` (segment/Mem `replay()`) to recover.
                // Only disk repos get a background fsync — the Mem sink's
                // `sync` is a no-op, so a timer would be pointless.
                let group = match &self.wal_dir {
                    Some(dir) => {
                        let wal_dir = match dir.file_name() {
                            Some(name) => {
                                let mut n = name.to_os_string();
                                n.push(".shamirwal");
                                dir.with_file_name(n)
                            }
                            None => dir.join("shamirwal"),
                        };
                        let wal_dir_for_blocking = wal_dir.clone();
                        tokio::task::spawn_blocking(move || {
                            std::fs::create_dir_all(&wal_dir_for_blocking)
                        })
                        .await
                        .map_err(|e| DbError::Storage(format!("wal dir join: {e}")))?
                        .map_err(|e| DbError::Storage(format!("wal dir create: {e}")))?;
                        // F6b: the WAL is a DIRECTORY of numbered segments
                        // (`NNNNNNNN.wal`), not a single `repo.wal`. The
                        // project is pre-release with an atomic flip (no
                        // dual-recovery bridge — `wal-refactor.md` §4.4), so
                        // there is no legacy `repo.wal` migration: `SegmentSet`
                        // simply owns `<name>.shamirwal/` and rotates/truncates
                        // segments inside it. `create_dir_all` above already
                        // made the dir; `SegmentSet::open` opens the first
                        // active `00000000.wal`.
                        // F6c: the seal/rotate threshold is the
                        // `WAL_SEGMENT_MAX_BYTES` tunable (8 MiB), overridable
                        // per-process via `SHAMIR_WAL_SEGMENT_MAX_BYTES` (parsed
                        // as `u64`; a malformed value falls back to the const).
                        // This is a legitimate ops tunable (a small cap forces
                        // rotation+seal so truncation/growth-limit tests can
                        // reach segment boundaries without huge data volumes),
                        // NOT a debug-only seam — left live in every build, but
                        // Captured at construction so validation and rotation
                        // cannot observe different environment values.
                        let seg_max_bytes = self.wal_segment_bytes;
                        let segset =
                            shamir_wal::SegmentSet::open(wal_dir.clone(), seg_max_bytes).await?;
                        let sink = shamir_wal::WalSink::File(segset);
                        let group = Arc::new(shamir_wal::WalGroupCommit::new(Arc::new(sink)));
                        crate::repo::durability_policy::install_empty_coverage(&group)?;
                        // RF1: 250 ms is the nominal background fsync timer,
                        // not a hard power-loss bound; I/O errors and scheduling
                        // delays can extend the unsynced window.
                        const WAL_BG_FSYNC_MS: u64 = 250;
                        group.spawn_background_fsync(std::time::Duration::from_millis(
                            WAL_BG_FSYNC_MS,
                        ));
                        group
                    }
                    None => {
                        // Successful OnceCell initialization logs once, never per commit.
                        log::warn!(
                            "repo '{}': Mem WAL sink is not durable — wal_commit policy tier is ignored for this in-memory/hybrid backend",
                            self.name
                        );
                        let sink = shamir_wal::WalSink::mem();
                        let group = Arc::new(shamir_wal::WalGroupCommit::new(Arc::new(sink)));
                        crate::repo::durability_policy::install_empty_coverage(&group)?;
                        group
                    }
                };
                let mgr = shamir_tx::RepoWalManager::new(initial_txn_id, Arc::clone(&group));
                // Publish the group for the replication read path's R bound.
                let _ = self.wal_group.set(group);

                // Anti-collision safeguard: floor next_txn_id above any
                // inflight txn_id replayed from the WAL, so a fresh
                // `fresh_txn_id` can never reuse an id a crashed entry already
                // used. On a fresh Mem instance `recover()` is empty → inert;
                // on a disk repo it returns the durable entries.
                if let Ok(entries) = mgr.recover().await {
                    if let Some(max_txn_id) = entries.iter().map(|e| e.txn_id).max() {
                        mgr.seed_floor_at_least(max_txn_id + 1);
                    }
                }

                Ok::<Arc<shamir_tx::RepoWalManager>, DbError>(Arc::new(mgr))
            })
            .await
            .cloned()
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
        self.tx_metrics.on_tx_start();
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
            per_table_mvcc: Arc::clone(&self.per_table_mvcc),
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
        self.commit_visibility_wait = wait;
        self
    }

    pub fn commit_visibility_wait(&self) -> VisibilityWait {
        self.commit_visibility_wait
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
        crate::tx::commit::commit_tx_with_options(tx, self, wait).await
    }

    /// Borrow rather than clone the retained gate on the acknowledgement fast path.
    pub(crate) fn initialized_tx_gate(&self) -> Option<&shamir_tx::RepoTxGate> {
        self.tx_gate.get().map(|gate| gate.as_ref())
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
    /// F4b-1 keystone of "everything is a transaction": instead of taking the
    /// direct V1 write path (which emits `begin_with_delta`/`commit` WAL
    /// markers), a non-tx write opens a `IsolationLevel::Snapshot` tx on
    /// this repo, stages the write via the `_tx` execute path (`stage`), and
    /// commits — folding the data, index postings, and counter into ONE
    /// `WalEntryV2` and consuming ONE commit version.
    ///
    /// Snapshot isolation is deliberate: SSI validation is gated on
    /// `Serializable`, so the implicit tx NEVER aborts on a read/write
    /// conflict — this preserves non-tx last-writer-wins semantics.
    /// Unique-index violations still surface (the unique re-validation in
    /// pre-commit is unconditional) and are mapped to a coded
    /// `unique_violation` error.
    ///
    /// On a `stage` error or a commit error the `TxContext` / `SnapshotGuard`
    /// drop = clean RAII abort.
    ///
    /// `stage` receives the open `&mut TxContext` and returns the write's
    /// [`WriteResult`]; the returned ids / shape are identical to the direct
    /// path.
    ///
    /// F5a: lifted from the private `run_implicit_batch_tx` free fn in
    /// `query::batch::query_runner` so the `shamir-db` system-store and
    /// admin user/role direct-delete callers can route their deletes through
    /// the same implicit-tx file-WAL path (retiring the V1 DELETE marker).
    ///
    /// F-28 Step 1 (#828): reimplemented as a thin wrapper over
    /// [`begin_implicit_batch_tx`](Self::begin_implicit_batch_tx) /
    /// [`commit_implicit_batch_tx`](Self::commit_implicit_batch_tx) — the
    /// `stage` HRTB closure this method still accepts forces any captured
    /// reference to satisfy `'a: 'static` (see those methods' docs), which is
    /// why `query_runner`'s 4 implicit-arm call sites moved off this API onto
    /// the split begin/commit pair directly. This wrapper is kept, unchanged
    /// in signature and behavior, for its ~14 other existing callers.
    ///
    /// The two delegated calls are `Box::pin`'d: without it, this debug
    /// (`opt-level = 0`) build inlines `begin_implicit_batch_tx`'s and
    /// `commit_implicit_batch_tx`'s full `async fn` state machines into
    /// THIS future's state machine on top of the original inline body,
    /// growing every caller's stack frame by the sum of both — measured to
    /// tip at least one pre-existing, unrelated `#[tokio::test]` deep in a
    /// DDL admin call chain (`access_ddl::group_grant_via_ddl`) over the
    /// default Windows thread stack, observed as a `has overflowed its
    /// stack` abort with no logic change on that path at all. Boxing moves
    /// each callee's state machine to the heap, keeping this fn's own frame
    /// (and thus every one of `run_implicit_batch_tx`'s ~14 callers) at
    /// pre-refactor size — verified by reproducing the failure/fix with the
    /// delegation form isolated in a scratch worktree before landing this.
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
        let (mut tx, _guard) = Box::pin(self.begin_implicit_batch_tx(actor, alias)).await?;

        // Stage the write into the tx. On error drop tx/_guard = RAII abort.
        let wr = stage(&mut tx).await.map_err(|e| BatchError::QueryError {
            alias: alias.to_string(),
            message: e.to_string(),
            code: e.code().map(str::to_owned),
        })?;

        // Commit — folds everything into one WalEntryV2 / one commit_version.
        Box::pin(self.commit_implicit_batch_tx(tx, alias)).await?;
        Ok(wr)
    }

    /// Open the implicit Snapshot batch tx exactly as `run_implicit_batch_tx`
    /// does internally (set_actor + set_implicit(true)), returning the tx and
    /// its SnapshotGuard so the CALLER can stage with ordinary straight-line
    /// code that borrows freely (no HRTB) — see F-28 Step 1 (#828).
    ///
    /// The returned `TxContext` / `SnapshotGuard` are plain local variables:
    /// both drop cleanly (RAII abort) on any early return/error the caller
    /// hits between this call and `commit_implicit_batch_tx`, exactly as
    /// `run_implicit_batch_tx`'s internal `tx`/`_guard` did.
    pub async fn begin_implicit_batch_tx(
        &self,
        actor: Actor,
        alias: &str,
    ) -> Result<(shamir_tx::TxContext, shamir_tx::SnapshotGuard), BatchError> {
        self.begin_implicit_batch_tx_with_isolation(
            actor,
            alias,
            shamir_tx::IsolationLevel::Snapshot,
        )
        .await
    }

    /// Like [`begin_implicit_batch_tx`](Self::begin_implicit_batch_tx), but
    /// lets the caller pick the isolation level instead of hardcoding
    /// `Snapshot`.
    ///
    /// F-28 Step 5 (S3-C): the query-runner's implicit DELETE/UPDATE arms use
    /// this to upgrade to `Serializable` for a single implicit tx when the
    /// target table is flagged (`FkReverseCache::is_fk_parent_with_delete_action`
    /// / `is_fk_parent_with_update_action`) as an FK parent with a
    /// non-`NoAction` `on_delete`/`on_update` — the Serializable predicate
    /// check is what closes the cross-transaction TOCTOU race between the FK
    /// reverse-check scan and this tx's commit
    /// (see `fk_restrict.rs`/`fk_actions.rs`/`fk_on_update.rs`'s module docs).
    /// Every other implicit-tx caller keeps calling
    /// [`begin_implicit_batch_tx`](Self::begin_implicit_batch_tx), which
    /// delegates here with `Snapshot` — byte-identical behavior, unchanged.
    pub async fn begin_implicit_batch_tx_with_isolation(
        &self,
        actor: Actor,
        alias: &str,
        isolation: shamir_tx::IsolationLevel,
    ) -> Result<(shamir_tx::TxContext, shamir_tx::SnapshotGuard), BatchError> {
        let (mut tx, guard) =
            self.begin_tx(isolation)
                .await
                .map_err(|e| BatchError::QueryError {
                    alias: alias.to_string(),
                    message: format!("implicit begin_tx: {}", e),
                    code: None,
                })?;
        // Provenance: thread the actor for commit-time attribution (R2).
        tx.set_actor(actor);
        // Mark implicit so the changefeed event reports tx_id == 0 (preserving
        // the "0 = non-tx write" subscription contract). The internal tx_id
        // stays real for WAL / crash-injection seams.
        tx.set_implicit(true);
        Ok((tx, guard))
    }

    /// Commit an implicit batch tx with the canonical BatchError mapping —
    /// same error-precedence/coding as `run_implicit_batch_tx`'s existing
    /// commit-error handling (UniqueViolation -> "unique_violation",
    /// CasConflict -> "version_conflict" per FG-7). See F-28 Step 1 (#828).
    pub async fn commit_implicit_batch_tx(
        &self,
        tx: shamir_tx::TxContext,
        alias: &str,
    ) -> Result<shamir_query_types::batch::TransactionInfo, BatchError> {
        crate::query::batch::receipts::commit_implicit(self, tx, alias, VisibilityWait::Strict)
            .await
    }
}
