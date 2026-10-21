use std::collections::BTreeSet;
use std::str::FromStr;
use std::sync::Arc;

use futures::StreamExt;
use shamir_query_types::read::{DdlOpKind, DdlOpState, DdlOpStatus};
use shamir_storage::error::DbResult;
use shamir_types::core::interner::TouchInd;
use shamir_types::time::unix_millis;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;

use crate::table::table_manager::TableManager;
use crate::index::index_definition::IndexDefinition;
use crate::index::index_info_item::IndexInfoItem;

/// Result structure for #1088 Phase C/D: the SnapshotGuard and pin version
/// must be returned from Phase B+A and kept alive through Phase C/D.
pub(crate) struct PhaseBAResult {
    pub guard: shamir_tx::SnapshotGuard,
    pub pin: u64,
}

impl TableManager {
    pub(crate) const CATCHUP_ITERATION_CAP: usize = 10; // RFC v3 §2.4/§6.2 — conservative
                                                        // fixed cap, no tunables precedent
                                                        // for this yet; local const is fine.

    /// Create a regular or specialized (fts/vector/functional) index.
    ///
    /// Routes `btree` + `unique` variants through the base_index `IndexManager`
    /// path (`create_index` / `create_unique_index`). All other index types
    /// go through the `index2` backend pipeline.
    pub async fn create_index_v2(
        &self,
        op: &shamir_query_types::admin::CreateIndexOp,
    ) -> DbResult<()> {
        // P0-3b (#988) sub-bug 3b: the name-reuse fast-reject MUST run BEFORE
        // repository admission. `drop_index2` holds the repo permit across its
        // ENTIRE critical section — including while parked at the F-76
        // pre-sweep test hook (`drop_index2_pause_hook`, in
        // `write_domain_index_core.rs`) and across its reader-drain wait — so
        // a `create_index_v2` that admitted FIRST would block on the
        // non-reentrant domain mutex until the parked drop completes. Under
        // `p03b_index2_name_reuse_rejected_during_drop` the only task that can
        // release the hook is the SAME task issuing the create → deadlock
        // (test timeout). The cheap persisted-tombstone read below rejects
        // without touching the domain; the same check re-runs inside the
        // admitted body (closing the admit→check race for admitted callers).
        let index_type = op.index_type.as_deref().unwrap_or("btree");
        if index_type != "btree" {
            self.reject_create_while_drop_in_flight(&op.create_index)
                .await?;
        }
        let permit = self.admit().await?;
        self.create_index_v2_admitted(op, &permit).await
    }

    /// P0-3b (#988): reject a CREATE whose name's DROP is still in flight,
    /// by reading the persisted drop tombstone. Pure reads — safe to run
    /// WITHOUT repo admission (used pre-admission by
    /// [`TableManager::create_index_v2`] and under a held permit inside
    /// [`TableManager::create_index_v2_admitted`]).
    pub(in crate::table) async fn reject_create_while_drop_in_flight(
        &self,
        index_name: &str,
    ) -> DbResult<()> {
        let dropping_ids =
            crate::index2::persistence::load_dropping_index2(&self.info_store).await?;
        if dropping_ids.is_empty() {
            return Ok(());
        }
        if let Some(persisted) =
            crate::index2::persistence::load_index2_metadata(&self.info_store).await?
        {
            for d in &persisted.descriptors {
                if dropping_ids.iter().any(|(id, _, _)| *id == d.id) && d.name == index_name {
                    return Err(shamir_storage::error::DbError::Internal(format!(
                        "Cannot create index '{index_name}': a DROP INDEX for this name is \
                         still in progress. Retry after the drop completes."
                    )));
                }
            }
        }
        Ok(())
    }

    /// Populate a single freshly-created index2 backend from the records that
    /// already exist in this table, by streaming the current record set and
    /// running `plan_insert` + `apply_index_ops` for each row.
    ///
    /// This is the index2 analogue of the base_index btree
    /// `create_index_from_records` backfill. It is scoped to ONE backend (the
    /// one just created) so a CREATE INDEX on a table that already carries
    /// other index2 backends does not needlessly re-touch them.
    ///
    /// Called from BOTH `create_index_v2` (the normal CREATE INDEX path) AND
    /// the F-50 Step 3b table-open self-healing restart-from-scratch
    /// (`TableManager::create`'s `Building`-descriptor recovery loop).
    ///
    /// Runs BEFORE the backend is registered, which avoids a double-write
    /// (the live `index2_on_insert` hook can't yet route to an unregistered
    /// backend).
    ///
    /// # Lost-write race — CLOSED for the non-tx write path (#534), PARTIALLY
    /// # closed on the tx-commit path (#538 Part A), Part B still OPEN
    ///
    /// The caller (`create_index_v2`) holds `unique_write_lock` AND has set
    /// the `INDEX2_CREATE` bit across this backfill → the subsequent
    /// `index2_registry.insert`. While that bit is up, every **non-tx**
    /// writer path (`TableManager::insert`/`insert_many_returning_version`/
    /// `delete_returning_version`/`set`, gated by `needs_write_barrier()`)
    /// also takes `unique_write_lock`, so no row reaching the store through
    /// those methods can land in the window between this backfill's stream
    /// cursor passing a key and the backend becoming live-registered.
    ///
    /// **#534 `@fl` adversarial pass** found the tx-commit path (the primary
    /// production DML route) was NOT covered at all. **#538** closes part of
    /// that gap and leaves the harder part honestly open:
    ///
    /// 1. **Part A — CLOSED.** Every client INSERT/UPDATE/DELETE/SET runs
    ///    through an implicit or interactive tx (`execute_insert_tx` et al.
    ///    in `write_exec.rs` → `insert_tx_many_bytes`/`update_tx_bytes`/
    ///    `delete_tx` in `table_manager_tx_ops.rs` for STAGING → the commit
    ///    pipeline's Phase 2.5 prelock / Phase 5a-5c for MATERIALIZE). The
    ///    commit-time prelock (`pre_commit.rs`'s `pre_commit_prelock`, Phase
    ///    2.5) originally took `unique_write_lock` only for tables in
    ///    `tx.unique_guards` (tables with a base_index UNIQUE index) — an
    ///    index2-only table under an in-flight `create_index_v2` contributed
    ///    nothing, so that tx's commit could freely interleave with this
    ///    backfill. #538 Part A extends Phase 2.5 to ALSO acquire
    ///    `unique_write_lock` for every table the tx wrote to
    ///    (`tx.write_set` keys) whose `needs_write_barrier()` is `true` — so
    ///    a tx's COMMIT now serializes against this backfill exactly like
    ///    the non-tx writers do, via the same lock held for this whole
    ///    backfill → register sequence.
    /// 2. **Part B — CLOSED for the functional/INSERT case by F-50 (#869
    ///    spike); full closure (vector/fts/update/delete/sorted-index) is
    ///    Step 2.** Closing Part A only fixed the commit's TIMING; it does
    ///    not fix WHAT gets committed. The index2 ops-PLAN
    ///    (`tx.index_write_set`) is built at STAGE time (inside
    ///    `insert_tx_many_bytes` etc.) against an `all_backends()` snapshot
    ///    taken well before Phase 2.5 ever runs — in the worst case, before
    ///    this `create_index_v2` even started. If a tx stages before this
    ///    backend exists (in any form) and then commits after this backfill
    ///    has already completed and registered it, that tx's row IS correctly
    ///    serialized into the store by Part A's lock — but its ops-plan was
    ///    built with zero ops for this backend, so Phase 5c has nothing to
    ///    write for it. This was a GUARANTEED miss (not a rare race),
    ///    independent of Part A. F-50 closes it by re-deriving the ops-plan
    ///    against the LIVE backend set at commit time (in `pre_commit_prelock`
    ///    Phase 2.7, gated by `IndexRegistry`'s generation counter, BEFORE the
    ///    WAL entry is built so recovery replays the fresh plan) — see the
    ///    decision memo `docs/dev-artifacts/research/f50-index-lifecycle-spike.md`
    ///    for the full design + the Step 2 scope (the remaining backend kinds
    ///    and the sorted-index residual).
    /// 3. **Check-then-act, not a drain (pre-existing #534 residual,
    ///    unaffected by #538).** A writer/committer that observes
    ///    `needs_write_barrier() == false` (before `create_index_v2` has set
    ///    the flag) proceeds fully lock-free; nothing here waits for such an
    ///    already-in-flight writer to finish before the backfill takes its
    ///    snapshot. This narrows the original whole-backfill-duration race
    ///    down to the duration of one writer's already-in-flight
    ///    validate+write step — a real improvement, not a full close.
    ///
    /// Net effect: #534 was strictly better than the pre-#534 state, and
    /// fully correct for the non-tx/replication-apply write path. #538 Part A
    /// closes the commit-time serialization gap on the tx-commit path. F-50
    /// (#869 spike) closes Part B's stage-time ops-plan staleness for the
    /// functional/INSERT case by re-deriving ops in `pre_commit_prelock`
    /// Phase 2.7 — see
    /// `crate::table::tests::index2_create_barrier_tests::stage_and_commit_inside_window_now_indexes_new_index_part_b_closed`
    /// for the regression test confirming the row is now indexed. The full
    /// closure (vector/fts/update/delete + sorted-index + crash/restart
    /// continuation) is Step 2 / Step 3 per the F-50 memo — do not read this
    /// comment as "the tx-commit-path lost-write race is fully closed for all
    /// backend kinds" without that qualification.
    pub(super) async fn backfill_index2_backend(
        &self,
        backend: &dyn crate::index2::backend::IndexBackend,
    ) -> DbResult<()> {
        let stream = self.list_stream(1000);
        futures::pin_mut!(stream);
        while let Some(batch_result) = stream.next().await {
            let batch = batch_result?;
            for (rid, cow) in batch {
                let val = cow.into_inner()?;
                let ops = backend
                    .plan_insert(rid, &val)
                    .await
                    .map_err(|e| shamir_storage::error::DbError::Internal(e.to_string()))?;
                crate::index2::apply_index_ops(&ops, &self.info_store, backend)
                    .await
                    .map_err(|e| shamir_storage::error::DbError::Internal(e.to_string()))?;
            }
        }
        Ok(())
    }

    /// Create a regular index on specified paths.
    ///
    /// # Concurrency (F-57, #883)
    ///
    /// Holds `unique_write_lock` across the ENTIRE snapshot→backfill→register
    /// sequence and raises the `REGULAR_INDEX_CREATE` bit so
    /// `needs_write_barrier` returns `true` for
    /// every writer path — closing the lost-write race where a row written
    /// between the backfill snapshot and registration is seen by NEITHER the
    /// backfill NOR the live `index_manager` write hook. Pre-F-57 this path had
    /// ZERO protection: `needs_write_barrier()` never considered regular
    /// indexes at all. The drain (via
    /// [`begin_write_barrier`](Self::begin_write_barrier), BEFORE the lock —
    /// F-70, #897) closes the check-then-act gap for a writer that read
    /// `false` a moment before the flag went up — same pattern as
    /// `create_index_v2`.
    pub async fn create_index(&self, name: &str, paths: &[&str]) -> DbResult<()> {
        let permit = self.admit().await?;
        self.create_index_admitted(name, paths, &permit).await
    }

    /// Create a unique index on specified paths.
    ///
    /// # Concurrency (audit A9 — write-barrier for unique CREATE)
    ///
    /// Holds the table-wide `unique_write_lock` across the ENTIRE
    /// snapshot→backfill→register sequence. This closes the
    /// duplicate-slip-through window at its root: while the unique index
    /// is between "not yet registered" and "registered", no writer can
    /// insert ANY row (let alone a duplicate). Unique-index uniqueness
    /// validation during backfill is NOT safely idempotent-double-writable
    /// (a duplicate is a correctness violation, not a harmless double-write),
    /// so the write-barrier (Option B) is used instead of the register-first
    /// approach (Option A) applied to regular indexes.
    ///
    /// # Errors
    /// Returns `DbError::UniqueIndexCreationFailed` if duplicate values exist.
    pub async fn create_unique_index(&self, name: &str, paths: &[&str]) -> DbResult<()> {
        let permit = self.admit().await?;
        self.create_unique_index_admitted(name, paths, &permit)
            .await
    }

    /// Drop a regular index by name.
    ///
    /// P0-3 (#957/#959): wrapped in `begin_write_barrier(REGULAR_INDEX_CREATE)`
    /// — same drain-then-lock pattern as `create_index`. This serializes DROP
    /// against concurrent WRITERS (an in-flight fast-path writer that read
    /// `needs_write_barrier() == false` before the bit went up is drained
    /// before the sweep begins). It does NOT fully close the in-flight
    /// READER race (sub-bug 3a — a reader holding an `Arc` snapshot of the
    /// old definition can still observe a partially-swept keyspace); that
    /// residual is documented on `IndexManager::drop_index`'s method doc.
    ///
    /// # Returns
    /// `true` if index existed and was removed, `false` if not found.
    ///
    /// #1051: accepts `op_id` minted at dispatch time for crash recovery status writes.
    pub async fn drop_index(&self, name: &str, op_id: Option<RecordId>) -> DbResult<bool> {
        let permit = self.admit().await?;
        self.drop_index_admitted(name, op_id, &permit).await
    }

    /// Drop a unique index by name.
    ///
    /// P0-3 (#957/#959): wrapped in `begin_write_barrier(UNIQUE_INDEX_CREATE)`
    /// — same drain-then-lock pattern as `create_unique_index`. See
    /// `drop_index`'s doc for what this does and does not close.
    ///
    /// # Returns
    /// `true` if index existed and was removed, `false` if not found.
    ///
    /// #1051: accepts `op_id` minted at dispatch time for crash recovery status writes.
    pub async fn drop_unique_index(&self, name: &str, op_id: Option<RecordId>) -> DbResult<bool> {
        let permit = self.admit().await?;
        self.drop_unique_index_admitted(name, op_id, &permit).await
    }

    ///
    /// This is the standalone `DROP INDEX` counterpart to
    /// [`create_index_v2`](Self::create_index_v2). Unlike the
    /// `DROP TABLE ... CASCADE` path (which can skip persistence + posting
    /// cleanup because the whole table dies with the index), a standalone
    /// drop must do both so the surviving table never observes a stale
    /// descriptor or orphan postings.
    ///
    /// # Crash-safety sequence (P0-3b / #988 — durable tombstone)
    ///
    /// The order tombstone → retire → sweep → persist → clear-tombstone
    /// closes the crash-resurrection gap that the retire → sweep → persist
    /// order had (a crash between sweep and persist left the on-disk
    /// `PersistedIndexes` still listing the index as `Ready` with zero
    /// postings — the planner would route queries to a dead index, silently
    /// returning empty/missing results). This mirrors the durable-tombstone
    /// pattern from the base_index regular/unique family (#959) and the
    /// sorted family (#972):
    ///   1. resolve the backend by interned name (`Ok(false)` if absent);
    ///   2. **persist a durable tombstone** (`add_to_dropping_index2`) recording
    ///      that this descriptor id is being dropped — MUST succeed before
    ///      the sweep, so a crash at any later point is recoverable;
    ///   3. `registry.remove_by_id(id)` to retire it from the planner-visible
    ///      live set FIRST (F-76 / #903 — mirror image of F-72's `Building`
    ///      gate for CREATE). Also advances the F-50 generation counter;
    ///   4. `backend.drop_all()` to sweep its (now orphan, planner-invisible)
    ///      posting entries — `backend` is held locally so it survives the
    ///      registry removal;
    ///   5. `save_index2_metadata` to persist the reduced registry — it
    ///      re-derives `PersistedIndexes` from the LIVE registry's
    ///      `all_descriptors()`, so calling it AFTER `remove_by_id`
    ///      naturally persists the removal;
    ///   6. **clear the tombstone** (`clear_from_dropping_index2`) — the
    ///      reduced metadata is now durable, so the tombstone is no longer
    ///      needed.
    ///
    /// On restart, `recover_index2_drops` in
    /// `TableManager::create` sees any tombstone left by an interrupted drop
    /// and finishes it idempotently (remove backend, sweep postings, persist,
    /// clear tombstone). See that method's crash-state matrix for the full
    /// recovery reasoning.
    ///
    /// F-76 (#903): the OLD order was `drop_all` → `remove_by_id`, which left
    /// a window in which a concurrent reader could still resolve the backend
    /// via `find_by_field_and_kind` (it was still `Ready` in the registry)
    /// while its postings were mid-sweep — silently wrong/incomplete results,
    /// the mirror image of the F-72 CREATE bug. Retiring the registry entry
    /// FIRST closes that window: every NEW reader after the removal resolves
    /// the backend as absent and falls back to a full scan. A reader that
    /// already holds an `Arc<dyn IndexBackend>` snapshot keeps working
    /// against its own consistent view (RCU — the `Arc` keeps the backend and
    /// its already-read postings alive).
    ///
    /// # Returns
    /// `true` if a backend existed and was removed, `false` if no index2
    /// backend is registered under `name`.
    ///
    /// R0-A (#1012): wrapped in `begin_write_barrier(INDEX2_CREATE)` — reuses
    /// the family's existing CREATE bit (mirrors `drop_sorted_index`'s
    /// reasoning: no legitimate case needs a concurrent CREATE/DROP/RENAME
    /// pair racing the SAME index2 backend, and a dedicated DROP bit would
    /// only let an unrelated CREATE proceed unserialized against this drop's
    /// registry mutation). Held across the ENTIRE tombstone → registry
    /// `remove_by_id` → posting-sweep → metadata-persist → tombstone-clear
    /// sequence below — before this fix, NOTHING serialized this method
    /// against a concurrent CREATE/DROP/RENAME on the same table, so two
    /// registry-mutating DDL ops could race `IndexRegistry`'s ticket/
    /// generation bookkeeping (see `IndexRegistry`'s doc for the scenario
    /// this closes).
    ///
    /// #1051: accepts `op_id` minted at dispatch time for crash recovery status writes.
    pub async fn drop_index2(&self, name: &str, op_id: Option<RecordId>) -> DbResult<bool> {
        let permit = self.admit().await?;
        self.drop_index2_admitted(name, op_id, &permit).await
    }

    // #1048: write SucceededViaCrashRecovery status for recovered hash DROP operations.
    // This is a public helper called from TableManager::create.
    pub async fn write_hash_drop_recovery_status(
        dropping_regular: &[(u64, Option<String>)],
        dropping_unique: &[(u64, Option<String>)],
        interner: &crate::table::interner_manager::InternerManager,
        info_store: Arc<dyn shamir_storage::types::Store>,
    ) -> Result<(), shamir_storage::error::DbError> {
        // Resolve name_interned back to string names using the interner
        // and write SucceededViaCrashRecovery for each recovered operation.
        let interner_guard = interner.get().await.map_err(|e| {
            shamir_storage::error::DbError::Internal(format!(
                "#1048: failed to get interner for hash DROP recovery status: {e}"
            ))
        })?;

        // Regular family
        for &(name_interned, ref op_id_str) in dropping_regular {
            if let Some(name) = interner_guard.with_str(
                &shamir_types::core::interner::InternerKey::new(name_interned),
                |s| s.to_string(),
            ) {
                // Skip status write if op_id is None (pre-#1051 tombstone or no-op caller)
                if let Some(op_id_str) = op_id_str {
                    // A corrupt op_id string is a best-effort miss on the
                    // status log, not a reason to fail the whole table open
                    // — the actual index recovery (this function's caller)
                    // has already succeeded by the time this runs.
                    let op_id = match std::str::FromStr::from_str(op_id_str) {
                        Ok(id) => id,
                        Err(e) => {
                            log::error!(
                                "#1051: failed to parse op_id '{op_id_str}' for recovered \
                                 hash DROP (regular) '{name}': {e} — skipping status write"
                            );
                            continue;
                        }
                    };
                    let status = DdlOpStatus {
                        op_id,
                        kind: DdlOpKind::DropHashIndex {
                            index_name: name.clone(),
                        },
                        state: DdlOpState::SucceededViaCrashRecovery {
                            completed_at_restart: unix_millis(),
                        },
                    };
                    if let Err(e) =
                        crate::table::ddl_op_log::write_op_status(&info_store, &status).await
                    {
                        log::error!(
                            "#1048 / #1069: failed to write SucceededViaCrashRecovery status for \
                             recovered hash DROP (regular) '{}': {}. The drop completed successfully, \
                             but polling by op_id {:?} will return Unknown. Call TableManager::verify() \
                             to confirm the index was dropped.",
                            name, e, op_id
                        );
                    }
                }
            }
        }

        // Unique family
        for &(name_interned, ref op_id_str) in dropping_unique {
            if let Some(name) = interner_guard.with_str(
                &shamir_types::core::interner::InternerKey::new(name_interned),
                |s| s.to_string(),
            ) {
                // Skip status write if op_id is None (pre-#1051 tombstone or no-op caller)
                if let Some(op_id_str) = op_id_str {
                    // See the regular-family arm above for why this is
                    // best-effort (log + skip), not a hard failure.
                    let op_id = match std::str::FromStr::from_str(op_id_str) {
                        Ok(id) => id,
                        Err(e) => {
                            log::error!(
                                "#1051: failed to parse op_id '{op_id_str}' for recovered \
                                 hash DROP (unique) '{name}': {e} — skipping status write"
                            );
                            continue;
                        }
                    };
                    let status = DdlOpStatus {
                        op_id,
                        kind: DdlOpKind::DropUniqueHashIndex {
                            index_name: name.clone(),
                        },
                        state: DdlOpState::SucceededViaCrashRecovery {
                            completed_at_restart: unix_millis(),
                        },
                    };
                    if let Err(e) =
                        crate::table::ddl_op_log::write_op_status(&info_store, &status).await
                    {
                        log::error!(
                            "#1048 / #1069: failed to write SucceededViaCrashRecovery status for \
                             recovered hash DROP (unique) '{}': {}. The drop completed successfully, \
                             but polling by op_id {:?} will return Unknown. Call TableManager::verify() \
                             to confirm the index was dropped.",
                            name, e, op_id
                        );
                    }
                }
            }
        }

        Ok(())
    }

    /// P0-3b (#988): open-time recovery for index2 DROP INDEX operations
    /// interrupted by a crash. Called from `TableManager::create` AFTER the
    /// F-50 Step 3b self-heal block (which loaded persisted metadata and
    /// inserted backends into the registry) and BEFORE the `restore_on_open`
    /// loop (so we don't waste time rebuilding a backend recovery removes).
    ///
    /// # Crash-state matrix (mirrors #972's `recover_in_progress_drops`)
    ///
    /// The drop sequence is: tombstone → `remove_by_id` → sweep → persist →
    /// clear-tombstone. On restart, the F-50 Step 3b block has already loaded
    /// persisted metadata and inserted backends, so the registry reflects
    /// whatever was on disk.
    ///
    /// | crash point                              | registry has backend? | postings? | tombstone? | recovery action                       |
    /// |------------------------------------------|----------------------|-----------|------------|---------------------------------------|
    /// | after tombstone-write, before sweep      | yes (from persisted)  | present   | yes        | remove_by_id, sweep, persist, clear   |
    /// | after sweep, before persist              | yes (from persisted)  | gone      | yes        | remove_by_id, sweep (no-op), persist, clear |
    /// | after persist, before clear              | no (not in persisted) | gone      | yes        | sweep (no-op), clear                  |
    ///
    /// In every case the recovery leaves the manager in a consistent state:
    /// the backend is gone from the registry, its postings are swept, the
    /// reduced metadata is persisted, and the tombstone is cleared. The sweep
    /// is idempotent (prefix-scan + remove_many on already-removed keys is a
    /// no-op), so calling recovery twice (two restart attempts) is a clean
    /// no-op on the second call. Mirrors #972's `recover_in_progress_drops`.
    pub(crate) async fn recover_index2_drops(&self) -> DbResult<()> {
        let dropping = crate::index2::persistence::load_dropping_index2(&self.info_store).await?;
        if dropping.is_empty() {
            return Ok(());
        }

        log::info!(
            "P0-3b (#988): recovering {} in-progress index2 DROP(s)",
            dropping.len()
        );

        let mut changed = false;
        for (id, _name, _op_id) in &dropping {
            // If the backend is still in the registry (crash happened before
            // `save_index2_metadata` finalized the removal), retire it now.
            // If it's already gone (crash after persist), this is a no-op.
            if self.index2_registry.remove_by_id(*id).await.is_some() {
                changed = true;
            }
            // Always run the sweep (idempotent). Covers both the "sweep never
            // ran" and "sweep ran but persist failed" cases. The sweep is a
            // 4-byte prefix scan on `id.to_le_bytes()` — no backend Arc needed.
            crate::index2::persistence::sweep_index2_postings_by_id(*id, &self.info_store).await?;
        }
        if changed {
            crate::index2::persistence::save_index2_metadata(
                &self.index2_registry,
                &self.info_store,
            )
            .await?;
        }

        // Defect 2 (#1069): Write SucceededViaCrashRecovery status BEFORE clearing the tombstone.
        // This ensures the terminal status is durable before the tombstone is cleared.
        // If we crash after this write but before tombstone clear, recovery will see the
        // tombstone and finish idempotently (tombstone clear is idempotent).
        for &(_id, ref name, ref op_id_str) in &dropping {
            if let Some(op_id_str) = op_id_str {
                let op_id = match RecordId::from_str(op_id_str) {
                    Ok(id) => id,
                    Err(e) => {
                        log::error!(
                            "#1051 / #1069: failed to parse op_id '{op_id_str}' for recovered \
                             index2 DROP '{name}': {e} — skipping status write"
                        );
                        continue;
                    }
                };
                let status = DdlOpStatus {
                    op_id,
                    kind: DdlOpKind::DropIndex2 {
                        index_name: name.clone(),
                    },
                    state: DdlOpState::SucceededViaCrashRecovery {
                        completed_at_restart: unix_millis(),
                    },
                };
                // Defect 2 (#1069): Do not swallow status-write errors — log loudly.
                if let Err(e) =
                    crate::table::ddl_op_log::write_op_status(&self.info_store, &status).await
                {
                    log::error!(
                        "#1048 / #1051 / #1069: failed to write SucceededViaCrashRecovery status for \
                         recovered index2 DROP '{}' (id={}): {}. The drop completed successfully, \
                         but polling by op_id {:?} will return Unknown. Call TableManager::verify() to \
                         confirm the index was dropped.",
                        name, _id, e, op_id
                    );
                }
            }
        }

        // #1204: clear every recovered entry through the shared
        // `clear_from_dropping_index2` fn instead of hand-rolling a raw
        // `bincode::serialize` write of an empty Vec. The two write paths
        // (this recovery-clear and the normal `drop_index2` clear) must
        // agree on the wire format — see `persistence.rs`'s
        // `INDEX2_DROP_TOMBSTONE_VERSION` doc for why a second, bespoke
        // encoder here would silently diverge from it.
        for &(id, _, _) in &dropping {
            crate::index2::persistence::clear_from_dropping_index2(id, &self.info_store).await?;
        }

        log::info!(
            "P0-3b (#988): recovery complete — {} index2 DROP(s) finalized",
            dropping.len()
        );

        Ok(())
    }

    // #1048 / #1051: DEPRECATED — moved into recover_index2_drops to fix #1069.
    // This function is kept for API compatibility but is now a no-op.
    // The status write now happens BEFORE tombstone clear in recover_index2_drops.
    #[deprecated(since = "0.0.0", note = "Moved into recover_index2_drops for #1069")]
    pub async fn write_index2_drop_recovery_status(
        dropping_entries: &[(u32, String, Option<String>)],
        info_store: Arc<dyn shamir_storage::types::Store>,
    ) -> Result<(), shamir_storage::error::DbError> {
        let _ = dropping_entries;
        let _ = info_store;
        Ok(())
    }

    /// #1066: open-time recovery for RENAME INDEX operations on the index2
    /// (FTS / functional / vector) family interrupted by a crash. Called
    /// from `TableManager::create` immediately after `recover_index2_drops`
    /// — both touch the SAME `index2_registry` and
    /// `save_index2_metadata`/`load_index2_metadata` persistence path, so
    /// keeping them adjacent respects the ordering these two recovery
    /// passes share (drop recovery must resolve the registry's shape before
    /// rename recovery reasons about the SAME ids). This runs BEFORE
    /// `recover_hash_renames`, which touches a completely different
    /// subsystem (`IndexManager`, base_index hash family).
    ///
    /// Unlike the index2 DROP tombstone (which always needs a sweep) or the
    /// hash RENAME tombstone (a drop+rebuild that needs the record stream),
    /// an index2 rename is a pure in-memory registry rekey — physical
    /// posting entries are keyed by the numeric `id`, not by name. So
    /// recovery never needs to touch postings; it only needs to make sure
    /// the registry (and thus the persisted metadata) ends up reflecting
    /// `new_name` for the tombstoned id, then clear the tombstone.
    ///
    /// Recovery semantics per `(id, old_name, new_name, op_id)` entry from
    /// `load_renaming_index2`:
    /// - Backend not found by `id` at all — nothing to recover (shouldn't
    ///   normally happen since index2 ids are never reused/dropped mid-rename
    ///   by this specific bug). Log a warning and just clear the tombstone so
    ///   it doesn't strand forever.
    /// - Backend found and already resolvable under `new_name`'s interned id
    ///   (the common case: crash happened between `save_index2_metadata` and
    ///   `clear_from_renaming_index2`, or the crash landed even later) — the
    ///   rename already completed. Just clear the tombstone (idempotent
    ///   finish).
    /// - Backend found but still resolvable under `old_name`'s interned id
    ///   (crash happened before `rename_entry` ran, or before
    ///   `save_index2_metadata` persisted it) — re-run `rename_entry` +
    ///   `save_index2_metadata` to completion, THEN clear the tombstone.
    ///
    /// Per-entry tombstone clearing (via `clear_from_renaming_index2` inside
    /// the loop, NOT a single batched "clear the whole list at the end")
    /// mirrors `recover_hash_renames`'s OPPOSITE choice deliberately: hash
    /// rename's batched clear is safe there because `IndexManager`'s
    /// in-memory `renaming_*` maps are always empty at open time, so a
    /// per-entry `clear_from_renaming` (which persists from that in-memory
    /// snapshot) would silently discard sibling entries (the #1000 bug).
    /// `clear_from_renaming_index2` here has NO such trap — it loads the
    /// CURRENT on-disk list, removes just `id`, and writes back, so calling
    /// it once per entry is safe against a partial-recovery-of-recovery
    /// interruption (a later entry's tombstone survives untouched) and is
    /// strictly safer than batching.
    ///
    /// #1067: writes `SucceededViaCrashRecovery` for the resolved `op_id`
    /// BEFORE clearing each entry's tombstone (mirrors `recover_index2_drops`'s
    /// own write-before-clear ordering, and #1069's write-order fix for the
    /// base_index hash/unique families). If a tombstone entry carries no
    /// `op_id` (pre-#1067 tombstone, or a non-DDL caller), the status write
    /// is skipped silently — same convention `recover_index2_drops` already
    /// uses for a `None` op_id.
    pub(crate) async fn recover_index2_renames(&self) -> DbResult<()> {
        let renaming = crate::index2::persistence::load_renaming_index2(&self.info_store).await?;
        if renaming.is_empty() {
            return Ok(());
        }

        log::info!(
            "#1066: recovering {} in-progress index2 RENAME(s)",
            renaming.len()
        );

        for (id, old_name, new_name, op_id_str) in &renaming {
            match self.index2_registry.get_by_id(*id).await {
                None => {
                    log::warn!(
                        "#1066: index2 RENAME recovery found a tombstone for id={id} \
                         ('{old_name}' -> '{new_name}') but no backend with that id exists — \
                         nothing to recover; clearing the stale tombstone"
                    );
                }
                Some(_backend) => {
                    let new_name_interned = self.intern_string(new_name).await?;
                    // #1066: persist the (possibly freshly-minted) interned
                    // id for `new_name` BEFORE using it below. Mirrors the
                    // `rename_index` dispatch path's own persist — without
                    // this, a second crash right after recovery could again
                    // strand an undurable interned id.
                    self.interner.persist().await?;
                    let already_renamed = self
                        .index2_registry
                        .get_by_name(new_name_interned)
                        .await
                        .map(|b| b.descriptor().id == *id)
                        .unwrap_or(false);

                    if already_renamed {
                        log::info!(
                            "#1066: index2 RENAME '{old_name}' -> '{new_name}' (id={id}) was \
                             already durable — tombstone-only reconciliation"
                        );
                    } else {
                        let old_name_interned = self.intern_string(old_name).await?;
                        let ok = self
                            .index2_registry
                            .rename_entry(old_name_interned, new_name.clone(), new_name_interned)
                            .await;
                        if !ok {
                            return Err(shamir_storage::error::DbError::Internal(format!(
                                "#1066: index2 RENAME recovery failed to rename id={id} \
                                 ('{old_name}' -> '{new_name}') — rename_entry reported no \
                                 matching old-name mapping (concurrent conflict during \
                                 recovery, which should be impossible before the table is \
                                 open to writers)"
                            )));
                        }
                        crate::index2::persistence::save_index2_metadata(
                            &self.index2_registry,
                            &self.info_store,
                        )
                        .await?;
                        log::info!(
                            "#1066: index2 RENAME '{old_name}' -> '{new_name}' (id={id}) \
                             replayed by recovery"
                        );
                    }
                }
            }

            // #1067: Write SucceededViaCrashRecovery BEFORE clearing this
            // entry's tombstone — if we crash after this write but before
            // the clear, recovery will see the tombstone and finish
            // idempotently (the tombstone clear is idempotent). Skip
            // silently if the tombstone entry carries no op_id.
            if let Some(op_id_str) = op_id_str {
                let op_id = match RecordId::from_str(op_id_str) {
                    Ok(id) => id,
                    Err(e) => {
                        log::error!(
                            "#1067: failed to parse op_id '{op_id_str}' for recovered \
                             index2 RENAME '{old_name}' -> '{new_name}' (id={id}): {e} — \
                             skipping status write"
                        );
                        crate::index2::persistence::clear_from_renaming_index2(
                            *id,
                            &self.info_store,
                        )
                        .await?;
                        continue;
                    }
                };
                let status = DdlOpStatus {
                    op_id,
                    kind: DdlOpKind::RenameIndex2 {
                        old_name: old_name.clone(),
                        new_name: new_name.clone(),
                    },
                    state: DdlOpState::SucceededViaCrashRecovery {
                        completed_at_restart: unix_millis(),
                    },
                };
                // Do not swallow status-write errors — log loudly.
                if let Err(e) =
                    crate::table::ddl_op_log::write_op_status(&self.info_store, &status).await
                {
                    log::error!(
                        "#1067: failed to write SucceededViaCrashRecovery status for recovered \
                         index2 RENAME '{}' -> '{}' (id={}): {}. The rename completed \
                         successfully, but polling by op_id {:?} will return Unknown. Call \
                         TableManager::verify() to confirm the index was renamed.",
                        old_name,
                        new_name,
                        id,
                        e,
                        op_id
                    );
                }
            }

            // Clear this entry's tombstone immediately — safe against a
            // partial-recovery-of-recovery interruption (see doc comment
            // above for why this differs from recover_hash_renames' batched
            // clear).
            crate::index2::persistence::clear_from_renaming_index2(*id, &self.info_store).await?;
        }

        log::info!(
            "#1066: recovery complete — {} index2 RENAME(s) finalized",
            renaming.len()
        );

        Ok(())
    }

    /// #997: open-time recovery for RENAME INDEX operations on the
    /// Check if a sorted index exists by name.
    ///
    /// Mirrors [`index_exists`](Self::index_exists) /
    /// [`unique_index_exists`](Self::unique_index_exists): a pure lookup that
    /// does NOT mint a new interned id when the name is absent.
    pub async fn sorted_index_exists(&self, name: &str) -> bool {
        if let Ok(interner) = self.interner.get().await {
            if let Some(key) = interner.get_ind(name) {
                return self
                    .sorted_indexes
                    .find_by_name_interned(key.id())
                    .is_some();
            }
        }
        false
    }

    /// Check if an index2 backend (`fts` / `functional` / `vector`) exists
    /// by name.
    ///
    /// Mirrors [`index_exists`](Self::index_exists): a pure lookup that does
    /// NOT mint a new interned id when the name is absent.
    pub async fn index2_exists(&self, name: &str) -> bool {
        if let Ok(interner) = self.interner.get().await {
            if let Some(key) = interner.get_ind(name) {
                return self.index2_registry.get_by_name(key.id()).await.is_some();
            }
        }
        false
    }

    /// Look up records by index value.
    pub async fn lookup_by_index(
        &self,
        name: &str,
        values: &[InnerValue],
    ) -> DbResult<BTreeSet<RecordId>> {
        let name_id = self.intern_string(name).await?;
        // Audit 1.5/3.2: the internal `IndexManager::lookup_by_index` now
        // returns `Arc<[RecordId]>` (sorted slice, O(1) cache-hit). This
        // public wrapper keeps the legacy `BTreeSet<RecordId>` signature
        // for API stability; the one-time collect here is at the boundary,
        // NOT on the internal hot path (engine-internal callers go through
        // `index_manager_ref().lookup_by_index` directly and consume the
        // `Arc<[RecordId]>` slice without collecting).
        //
        // ACCEPTED TRADE-OFF (task #488 review): this boundary clone
        // reintroduces the audit's O(|postings|) cost for whoever reaches
        // this specific public method — but as of this fix, NOTHING does:
        // `DbInstance::lookup_by_index`/`RepoInstance::lookup_by_index`
        // (the only callers of THIS function) have zero callers anywhere
        // in `shamir-server`/`shamir-client`/`shamir-sdk`/`shamir-db`/
        // `shamir-connect` (verified by grep across those crates). The
        // audit's actual dominant cost centers — SELECT execution
        // (`read_exec.rs`/`read_index_scan.rs`), write-path uniqueness/FK
        // checks (`write_helpers.rs`, `fk_restrict.rs`, `fk_on_update.rs`),
        // and validator dedup (`validator_db.rs`) — all bypass this
        // wrapper and call `index_manager_ref().lookup_by_index(...)`
        // directly, getting the true O(1) Arc-clone benefit. If a future
        // caller (SDK, server) starts using this public method on a hot
        // path, propagate `Arc<BTreeSet<RecordId>>` out through this
        // wrapper (and `RepoInstance`/`DbInstance`) instead of adding a
        // second clone-avoidance layer here.
        // P0-3a (#1011): the internal `IndexManager::lookup_by_index` now
        // returns `Option<Arc<[RecordId]>>`: `None` means a DROP of this index
        // is currently in its drain→sweep window, so the result is
        // deliberately withheld. This public introspection-by-name API is NOT a
        // query-planning path — a silent empty result here would be
        // indistinguishable from "the index legitimately has no matches", which
        // is strictly worse than an honest error. Surface it as `NotFound`
        // naming the index so the caller can re-plan / retry explicitly.
        let ids = self.index_manager.lookup_by_index(name_id, values).await?;
        let ids = ids.ok_or_else(|| {
            shamir_storage::error::DbError::NotFound(format!(
                "index '{name}' is currently being dropped (reader-drain window); \
                 the lookup result is unavailable — retry or re-plan without this index"
            ))
        })?;
        Ok(ids.iter().copied().collect())
    }

    /// Check if a regular index exists.
    ///
    /// Note: This method is async because it may need to load the interner.
    pub async fn index_exists(&self, name: &str) -> bool {
        // Try to get interned ID; if not interned, index doesn't exist
        if let Ok(interner) = self.interner.get().await {
            if let Some(key) = interner.get_ind(name) {
                return self.index_manager.index_exists(key.id());
            }
        }
        false
    }

    /// Check if a unique index exists.
    ///
    /// Note: This method is async because it may need to load the interner.
    pub async fn unique_index_exists(&self, name: &str) -> bool {
        if let Ok(interner) = self.interner.get().await {
            if let Some(key) = interner.get_ind(name) {
                return self.index_manager.unique_index_exists(key.id());
            }
        }
        false
    }

    /// R0-C (#1010): does `name` exist in ANY of the four index families
    /// (regular, unique, sorted, index2) on this table?
    ///
    /// Single shared helper combining `index_exists` / `unique_index_exists`
    /// / `sorted_index_exists` / `index2_exists` so every CREATE path uses
    /// the SAME cross-family check instead of drifting independent call
    /// sites. Callers that need admission-time atomicity (i.e. every real
    /// CREATE entry point) MUST call this AFTER acquiring
    /// `begin_write_barrier` — see each `create_*` method's call site for
    /// why: the admission-guarded window is what rules out another family's
    /// CREATE interleaving between this check and the eventual registration
    /// (the same TOCTOU class `create_index_v2`'s tombstone pre-check
    /// comment documents for the drop-in-flight case).
    pub async fn any_index_exists(&self, name: &str) -> bool {
        self.index_exists(name).await
            || self.unique_index_exists(name).await
            || self.sorted_index_exists(name).await
            || self.index2_exists(name).await
    }

    // ============================================================================
    // Internal helpers
    // ============================================================================

    /// Intern a single string, returning its u64 ID.
    pub(crate) async fn intern_string(&self, s: &str) -> DbResult<u64> {
        let interner = self.interner.get().await?;
        match interner.touch_ind(s) {
            Ok(TouchInd::New(key)) | Ok(TouchInd::Exists(key)) => Ok(key.id()),
            Err(e) => Err(shamir_storage::error::DbError::Codec(e.to_string())),
        }
    }

    /// Intern a path string like "user.address.city" into Vec<u64>.
    async fn intern_path(&self, path: &str) -> DbResult<Vec<u64>> {
        let interner = self.interner.get().await?;
        let mut result = Vec::new();

        for component in path.split('.') {
            let id = match interner.touch_ind(component) {
                Ok(TouchInd::New(key)) | Ok(TouchInd::Exists(key)) => key.id(),
                Err(e) => return Err(shamir_storage::error::DbError::Codec(e.to_string())),
            };
            result.push(id);
        }

        Ok(result)
    }

    /// Build IndexDefinition from string name and paths.
    pub(in crate::table) async fn build_index_definition(
        &self,
        name: &str,
        paths: &[&str],
    ) -> DbResult<IndexDefinition> {
        let name_id = self.intern_string(name).await?;

        let mut interned_paths = Vec::with_capacity(paths.len());
        for path in paths {
            let path_components = self.intern_path(path).await?;
            interned_paths.push(IndexInfoItem::new(path_components));
        }

        Ok(IndexDefinition::new(name_id, interned_paths))
    }

    // ============================================================================
    // Rename index (rekey in place, preserve all posting data)
    // ============================================================================

    /// Rename an index from `old_name` to `new_name` on this table.
    ///
    /// Handles all four index kinds:
    ///   - **regular** (hash, `is_unique=0`): drop+rebuild — the hash-index
    ///     physical key embeds `name_interned` into the dual hash
    ///     (`compute_leaf_hashes` / `compute_lookup_hashes` both mix
    ///     `name_interned` into h1+h2), so a raw key-rewrite would leave
    ///     orphaned entries that the lookup path (which recomputes hashes
    ///     with the NEW name_interned) cannot find. The index is derived
    ///     data, so drop+rebuild from the live record stream is safe.
    ///   - **unique** (hash, `is_unique=1`): same as regular — drop+rebuild.
    ///   - **sorted** (B-tree-by-value, `SORTED_TAG` prefix): rekeys physical
    ///     entries from old to new name_interned (big-endian 8 bytes after
    ///     the tag byte). No hash mixing — sorted keys embed the raw
    ///     value bytes, not a hash of (name, value), so the rewrite is exact.
    ///   - **index2** (FTS / functional / vector): posting entries are keyed
    ///     by the compact `u32` `index_id`, not by name_interned, so no
    ///     physical move is needed — only the `by_name` lookup table AND the
    ///     authoritative name slots in the registry's `by_id` entry are updated
    ///     (P0-5a / #961), then the persisted metadata is re-saved.
    ///
    /// RENAME INDEX (hash regular + unique, sorted, index2) by name.
    ///
    /// `old_name` must exist; `new_name` must be free. Returns `Err` when the
    /// source does not exist or the destination name is already occupied by any
    /// index on this table.
    ///
    /// `op_id` is the operation ID minted at dispatch time (#1015); threaded
    /// through to tombstones so recovery can write `SucceededViaCrashRecovery`
    /// status under the same ID.
    pub async fn rename_index(
        &self,
        old_name: &str,
        new_name: &str,
        op_id: Option<shamir_types::types::record_id::RecordId>,
    ) -> DbResult<()> {
        let permit = self.admit().await?;
        self.rename_index_admitted(old_name, new_name, op_id, &permit)
            .await
    }
}
