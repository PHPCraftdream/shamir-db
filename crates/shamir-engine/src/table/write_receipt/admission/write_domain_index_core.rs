use super::TableManager;
use crate::repo::RepoWritePermit;
use futures::StreamExt;
use shamir_storage::error::DbResult;
use shamir_types::time::unix_millis;
use shamir_types::types::record_id::RecordId;

impl TableManager {
    pub async fn create_index_admitted(
        &self,
        name: &str,
        paths: &[&str],
        permit: &RepoWritePermit,
    ) -> DbResult<()> {
        self.write_domain.debug_check_permit(permit);
        let mut index_def = self.build_index_definition(name, paths).await?;
        // #1003: mark THIS index's name as in-flight for the rest of the
        // method body (RAII guard, dropped on every exit path including an
        // early `?` return or a panic) so `degraded_index_count()` excludes
        // the `Building` state this create is about to publish below — see
        // `in_flight_create_guard`'s module doc for the false-positive this
        // closes (and why it's a per-identity set, not a scalar count).
        let _in_flight = self.in_flight_creates.enter(index_def.name_interned);
        // F-72 (#899, P0): register at `Building` — `IndexManager::
        // create_index_from_records` persists this marker BEFORE the
        // backfill runs and flips it to `Ready` (with its own durable
        // persist) only once the backfill fully completes. See that
        // method's doc for the full publish/backfill/flip sequence this
        // closes (a concurrent planner read must never observe a
        // half-populated index).
        index_def.state = crate::index2::state::IndexState::Building;
        // F-42 (#850): persist the interner's newly-touched ids BEFORE the
        // index goes live — a persist failure must abort BEFORE publish, not
        // after. `build_index_definition` already interned the index NAME and
        // every field-path segment in-memory (`intern_string`/`intern_path` →
        // `touch_ind`); the ONLY thing deferred to this point is the DURABLE
        // flush of those already-assigned ids. Pre-F-42 a persist failure here
        // returned `Err` but left the just-registered index LIVE in
        // `index_manager` — a live index whose interner ids may not survive a
        // restart, the exact F-33 corruption class reopened at the failure
        // path. Reordering means a persist failure aborts before any publish,
        // so no rollback is needed.
        self.interner.persist().await?;

        // #1089: Try the online-build path first (RFC v3 Phase B+A → Phase C+D).
        // This path is barrier-free for the long-running scan, allowing concurrent
        // writers to proceed. If the table has no changefeed wired (e.g. a system
        // table or a directly-constructed test table), fall back to the whole-barrier
        // path below.
        match self
            .phase_b_a_backfill(name, index_def.clone(), 1000, permit)
            .await?
        {
            Some(phase_ba) => {
                // Online build available: run Phase C+D (catch-up and publish).
                self.phase_c_d_catchup_and_publish(index_def.name_interned, phase_ba, permit)
                    .await
            }
            None => {
                // Online build unavailable (no changefeed): fall back to the
                // whole-barrier path used before RFC v3.
                //
                // F-70 (#897, P0): canonical drain-then-lock acquisition — raise
                // `REGULAR_INDEX_CREATE`, drain in-flight fast-path writers, THEN
                // take `unique_write_lock`. F-57 (#883) originally acquired the lock
                // FIRST here, which this task found deadlocks against
                // `pre_commit_prelock`'s drain-guard-then-lock shape on a second
                // table. See `TableManager::begin_write_barrier` and
                // `writer_drain_barrier`'s "F-70" doc section.
                let (_barrier, _uwl_guard) = self
                    .begin_write_barrier_admitted(
                        crate::index::write_barrier_flags::REGULAR_INDEX_CREATE,
                        permit,
                    )
                    .await;
                // R0-C (#1010): cross-family name-uniqueness preflight, done WHILE
                // holding `ddl_admission` (via the barrier above) — see
                // `any_index_exists`'s doc and `create_index_v2`'s matching check for
                // why the admission-guarded window (not a handler-layer check before
                // admission) is what closes the TOCTOU gap. `index_exists` (this
                // family's own occupancy) was already implicitly enforced by
                // `IndexManager::create_index_from_stream`'s own registration; this
                // additionally rejects a name already used by ANY OTHER family.
                if self.any_index_exists(name).await {
                    return Err(shamir_storage::error::DbError::KeyExists(format!(
                        "index '{name}' already exists on this table (possibly in a different \
                         index family — names are unique per table across all families)"
                    )));
                }
                // F-78 (#905): stream the backfill instead of materializing the WHOLE
                // table into a `Vec<(RecordId, InnerValue)>`. `list_stream` already
                // yields batches in O(batch) memory; we adapt each `RecordCow` to a
                // decoded `InnerValue` here (the same decode `collect_all_current_records`
                // did, but per-batch instead of for the whole table) and hand the
                // stream to `IndexManager::create_index_from_stream`, which batch-
                // writes postings via `set_many` per batch (one transactional commit
                // per batch) instead of one giant `set_many` at the end. Peak memory
                // drops from O(table) to O(batch).
                //
                // `collect_all_current_records` is intentionally LEFT in place — it
                // still has other callers (`doctor::repair()` and two parity tests).
                //
                // Write-delta catch-up is already FREE: the barrier+lock above
                // (`begin_write_barrier(REGULAR_INDEX_CREATE)`) serializes every
                // concurrent writer for the whole create (in both the old materialize
                // shape and this streaming shape), and Phase 1's register-at-Building
                // (inside `create_index_from_stream`) activates the live write-hook so
                // a row written at the registration boundary gets its posting
                // maintained by that hook — the SAME mechanism the old path relied on.
                // Streaming adds no new lost-write window and needs no new mechanism.
                let stream = self.list_stream(1000).map(|batch| {
                    batch.and_then(|rows| {
                        rows.into_iter()
                            .map(|(id, cow)| cow.into_inner().map(|v| (id, v)))
                            .collect()
                    })
                });
                self.index_manager
                    .create_index_from_stream(index_def, stream)
                    .await
            }
        }
    }

    /// S5c-X admitted twin — caller already holds repo admission.
    pub async fn create_unique_index_admitted(
        &self,
        name: &str,
        paths: &[&str],
        permit: &RepoWritePermit,
    ) -> DbResult<()> {
        self.write_domain.debug_check_permit(permit);
        self.write_domain.debug_check_permit(permit);
        // #1003: the in-flight guard is installed inside `create_unique_index_body`
        // (right after the name is interned), not here — see that method's
        // matching comment. This wrapper only acquires the barrier+lock.
        // F-70 (#897, P0): canonical drain-then-lock acquisition — raise
        // `UNIQUE_INDEX_CREATE`, drain in-flight fast-path writers, THEN take
        // `unique_write_lock`. F-57 (#883) originally took the lock FIRST
        // (here, in the caller), with the flag+drain inside
        // `create_unique_index_locked` — this task found that order
        // deadlocks against `pre_commit_prelock`'s drain-guard-then-lock
        // shape on a second table. See `TableManager::begin_write_barrier`
        // and `writer_drain_barrier`'s "F-70" doc section. This is the SAME
        // lock non-tx writers (`insert`) and the tx commit pipeline
        // (Phase 2.5) acquire, so it unifies DDL against all writer classes.
        // Tables without unique indexes pay this harmlessly (no contention);
        // this is a low-frequency DDL operation.
        let (_barrier, _uwl_guard) = self
            .begin_write_barrier_admitted(
                crate::index::write_barrier_flags::UNIQUE_INDEX_CREATE,
                permit,
            )
            .await;
        self.create_unique_index_body(name, paths, permit).await
    }

    /// Inner unique-index create body: snapshot→backfill→register, with NO
    /// flag/lock acquisition of its own. ASSUMES the caller already holds
    /// BOTH `unique_write_lock` AND the `UNIQUE_INDEX_CREATE` barrier bit
    /// (via [`begin_write_barrier`](Self::begin_write_barrier)) for the
    /// caller's own required span. Used by:
    ///   - [`create_unique_index`](Self::create_unique_index) (acquires
    ///     barrier+lock itself, immediately above, then calls this body), and
    ///   - [`rename_index`](Self::rename_index)'s unique-index branch, which
    ///     acquires the SAME barrier+lock ONCE and holds it across the entire
    ///     drop→create span (a DIFFERENT requirement — uniqueness-gap
    ///     atomicity across drop+create, audit A9 — not drain ordering; see
    ///     that call site).
    ///
    /// # Concurrency (F-57, #883; reordered by F-70, #897)
    ///
    /// The FIRST unique index on a table is the critical case:
    /// `has_unique_indexes()` (and therefore `UNIQUE_INDEX_EXISTS`) is
    /// `false` until this create registers it, so without the caller's
    /// `UNIQUE_INDEX_CREATE` bit + drain, every concurrent fast-path writer
    /// would bypass `unique_write_lock` entirely — the exact race the review
    /// flagged. The bit + drain (done by the caller, BEFORE the lock — F-70)
    /// make the FIRST unique-index create as safe as the second-and-later.
    pub(in crate::table) async fn create_unique_index_body(
        &self,
        name: &str,
        paths: &[&str],
        permit: &RepoWritePermit,
    ) -> DbResult<()> {
        self.write_domain.debug_check_permit(permit);
        // R0-C (#1010): cross-family name-uniqueness preflight, done WHILE
        // holding `ddl_admission` — both callers (`create_unique_index` and
        // `rename_index`'s unique branch) already acquired the barrier
        // before reaching this body (see this method's doc). See
        // `any_index_exists`'s doc and `create_index_v2`'s matching check
        // for why the admission-guarded window is what closes the TOCTOU
        // gap. `unique_index_exists` (this family's own occupancy) is
        // enforced separately by `IndexManager`'s own registration; this
        // additionally rejects a name already used by ANY OTHER family.
        if self.any_index_exists(name).await {
            return Err(shamir_storage::error::DbError::KeyExists(format!(
                "index '{name}' already exists on this table (possibly in a different \
                 index family — names are unique per table across all families)"
            )));
        }
        let index_def = self.build_index_definition(name, paths).await?;
        // #1003: mark THIS index's name as in-flight for the rest of the
        // method body — see `create_index`'s matching guard +
        // `in_flight_create_guard`'s module doc. The unique family's real-path
        // definition is never observably `Building` today (see
        // `create_unique_index_from_records`'s doc) — since it's never
        // iterated over as non-`Ready` during its own backfill in the first
        // place, this guard is a no-op for THIS family's own count, but
        // (being per-identity, not a scalar) it can never bleed into an
        // unrelated index's count either. Installed uniformly across all four
        // families, and covers a future/crash-recovery path that DOES
        // register at `Building`. Also covers `rename_index`'s unique branch,
        // which calls this body directly.
        let _in_flight = self.in_flight_creates.enter(index_def.name_interned);
        // F-42 (#850) — see `create_index`'s matching comment: durably
        // persist the index-name/field-path ids BEFORE registering the
        // unique index. A persist failure aborts before publish, so no
        // rollback is needed (nothing was published yet). Signature check:
        // `create_unique_index_from_records(index_def, records)` consumes
        // the already-built `IndexDefinition` and a records Vec collected
        // independently from `list_stream` — neither depends on the
        // interner having been durably persisted.
        self.interner.persist().await?;
        // Always use the seam: collect_all_current_records routes
        // attached→log / unattached→data_store, so it is correct for
        // both cases. Collected UNDER the lock (held by caller) so no
        // writer can interpose.
        //
        // F-78 (#905) — DEFERRED for the unique family: unlike the regular
        // `create_index` path (which now streams via
        // `create_index_from_stream`), the unique path still materializes the
        // whole table here because duplicate detection needs global knowledge.
        // See `create_unique_index_from_records`'s F-78 doc for the full
        // rationale + the bounded-memory approaches tracked as follow-up. The
        // regular-hash streaming fix landed fully tested + benchmarked; this
        // unique-family gap is the documented escape-hatch deferral.
        //
        // P1-4 (#969): the unique family has no per-batch progress point (it
        // materializes-then-writes in one shot), so at minimum log start and
        // completion of the table scan so an operator can see the DDL is
        // progressing.
        log::info!(
            "CREATE UNIQUE INDEX '{}': starting backfill (scanning whole table — \
             unique family materializes all rows, no streaming)",
            index_def.name_interned
        );
        let backfill_start = std::time::Instant::now();
        let records = self.collect_all_current_records().await?;
        log::info!(
            "CREATE UNIQUE INDEX '{}': scanned {} records in {:.1}s, \
             writing unique index...",
            index_def.name_interned,
            records.len(),
            backfill_start.elapsed().as_secs_f64()
        );
        self.index_manager
            .create_unique_index_from_records(index_def, records)
            .await
    }

    /// S5c-X admitted twin — caller already holds repo admission.
    pub async fn drop_index_admitted(
        &self,
        name: &str,
        op_id: Option<RecordId>,
        permit: &RepoWritePermit,
    ) -> DbResult<bool> {
        self.write_domain.debug_check_permit(permit);
        let name_id = self.intern_string(name).await?;
        // P0-3 (#959): drain-then-lock — raise the REGULAR_INDEX_CREATE bit,
        // drain in-flight fast-path writers, then take `unique_write_lock`.
        // See `create_index`'s matching acquisition for the F-70 ordering
        // rationale.
        let (_barrier, _uwl_guard) = self
            .begin_write_barrier_admitted(
                crate::index::write_barrier_flags::REGULAR_INDEX_CREATE,
                permit,
            )
            .await;
        let op_id_str = op_id.map(|id| id.to_string());
        // #1069: pass index_name so IndexManager can write terminal status BEFORE tombstone clear
        self.index_manager
            .drop_index(name_id, op_id_str, Some(name))
            .await
    }

    /// S5c-X admitted twin — caller already holds repo admission.
    pub async fn drop_unique_index_admitted(
        &self,
        name: &str,
        op_id: Option<RecordId>,
        permit: &RepoWritePermit,
    ) -> DbResult<bool> {
        self.write_domain.debug_check_permit(permit);
        let name_id = self.intern_string(name).await?;
        // P0-3 (#959): drain-then-lock — see `drop_index`'s doc.
        let (_barrier, _uwl_guard) = self
            .begin_write_barrier_admitted(
                crate::index::write_barrier_flags::UNIQUE_INDEX_CREATE,
                permit,
            )
            .await;
        let op_id_str = op_id.map(|id| id.to_string());
        // #1069: pass index_name so IndexManager can write terminal status BEFORE tombstone clear
        self.index_manager
            .drop_unique_index(name_id, op_id_str, Some(name))
            .await
    }

    /// S5c-X admitted twin — caller already holds repo admission.
    pub async fn drop_index2_admitted(
        &self,
        name: &str,
        op_id: Option<RecordId>,
        permit: &RepoWritePermit,
    ) -> DbResult<bool> {
        self.write_domain.debug_check_permit(permit);
        let interner = self.interner.get().await?;
        // `get_ind` is a pure lookup (does NOT mint a new id), so dropping a
        // name that was never interned cannot pollute the interner.
        let Some(name_key) = interner.get_ind(name) else {
            return Ok(false);
        };
        let (_barrier, _uwl_guard) = self
            .begin_write_barrier_admitted(crate::index::write_barrier_flags::INDEX2_CREATE, permit)
            .await;
        let Some(backend) = self.index2_registry.get_by_name(name_key.id()).await else {
            return Ok(false);
        };
        let drop_id = backend.descriptor().id;
        let drop_name = backend.descriptor().name.clone();
        let op_id_str = op_id.map(|id| id.to_string());

        // P0-3b (#988 / #1051): write a durable tombstone BEFORE retiring the backend
        // or sweeping postings. If the process crashes after the sweep but
        // before the reduced metadata is persisted, the on-disk metadata
        // still lists the index — but the tombstone tells
        // `recover_index2_drops` to finish the drop rather than resurrecting
        // a broken "Ready but no postings" index. MUST succeed before
        // proceeding; if the persist fails the on-disk tombstone is unchanged
        // and we propagate `Err` without touching the registry or postings.
        crate::index2::persistence::add_to_dropping_index2(
            drop_id,
            drop_name,
            op_id_str,
            &self.info_store,
        )
        .await
        .map_err(|e| {
            shamir_storage::error::DbError::Internal(format!(
                "DROP INDEX '{name}': failed to persist the durable drop tombstone: {e}. \
                     The backend was NOT retired and postings were NOT swept — the \
                     index is still fully intact. Retry the DROP."
            ))
        })?;

        // P0-3a (#1038) step 2.5: raise the reader-drain gate's intent flag
        // (SeqCst) BEFORE the retire below. From this point every NEW
        // index2 reader acquires a lease that observes the flag and backs off
        // (Err(IndexDrainInProgress) → caller falls back to a full scan).
        // The flag stays up until `drain_guard` is dropped (step 4.5 + RAII
        // safety net on early `?`). See `reader_drain_gate`'s module doc for
        // the memory-model proof and the deadlock-exclusion placement invariant.
        let drain_guard = self.index2_registry.reader_gate().begin_drop();

        // F-76 (#903): retire the backend from the planner-visible registry
        // BEFORE sweeping its postings. See the method doc for the full
        // rationale. `backend` is held locally, so `drop_all` below still
        // runs after this removal.
        self.index2_registry.remove_by_id(drop_id).await;
        // F-76 test seam: park here (backend already retired from the
        // registry, postings not yet swept) if a test installed a pause hook.
        // With the fix, a concurrent read issued while parked here must fall
        // back to a full scan (the backend is gone from the planner). Zero
        // cost on the real path (`None`), compiled out of non-test builds.
        #[cfg(test)]
        if let Some(hook) = self.drop_index2_pause_hook.load_full() {
            hook.wait_at_window().await;
        }

        // P0-3a (#1038) step 3.5: drain every reader that entered
        // `lease_by_field_and_kind` BEFORE the flag went up (the gate's
        // memory model guarantees such a reader is counted in `in_flight`,
        // so this wait terminates — see the livelock-freedom argument in the
        // gate's module doc). AFTER the retire and the pause-hook park, BEFORE
        // the sweep, so the physical sweep cannot start until no reader is
        // holding a lease against the backend.
        drain_guard.wait_for_drain().await;

        // Sweep the (now orphan, planner-invisible) posting entries.
        backend.drop_all().await.map_err(|e| {
            shamir_storage::error::DbError::Internal(format!(
                "DROP INDEX '{name}': a durable drop tombstone was persisted and \
                     the backend was retired from the planner-visible registry, but \
                     the posting sweep failed: {e}. On restart, recovery will resume \
                     the sweep idempotently and finish the drop. Call \
                     TableManager::verify() to confirm state."
            ))
        })?;

        // P0-3a (#1038) step 4.5: the sweep finished — explicitly release the
        // drain guard now so the (unrelated) persist / tombstone-clear steps
        // below run with the gate already clear. RAII (`drain_guard`'s `Drop`
        // clears the flag) remains the safety net for every early `?` return
        // above, but an explicit early drop here keeps the drain window as
        // tight as possible.
        drop(drain_guard);

        // P0-3b (#988) test seam — park here (sweep complete, reduced metadata
        // NOT yet persisted) if a test installed the post-sweep hook. This is
        // the exact crash window sub-bug 3c exercises: a "crash" here (dropping
        // the manager) leaves the tombstone on disk and the old metadata, and
        // the recovery path in `TableManager::create` must finish the drop.
        // NOT `#[cfg(test)]`-gated cost-wise — zero cost (`None`) on real path.
        #[cfg(test)]
        if let Some(hook) = self.drop_index2_post_sweep_hook.load_full() {
            hook.wait_at_window().await;
        }

        // Persist the reduced metadata (backend removed from registry).
        crate::index2::persistence::save_index2_metadata(&self.index2_registry, &self.info_store)
            .await
            .map_err(|e| {
                shamir_storage::error::DbError::Internal(format!(
                    "DROP INDEX '{name}': a durable drop tombstone was persisted, \
                     the backend was retired, and the posting sweep completed, but \
                     persisting the reduced index metadata failed: {e}. On restart, \
                     recovery will finish the drop idempotently. Call \
                     TableManager::verify() to confirm state."
                ))
            })?;

        // #1067: Write terminal Succeeded status BEFORE clearing the
        // tombstone — this DROP INDEX2 path never wrote ANY terminal status
        // before this fix (not even a misclassified one — the `DdlOpKind::DropIndex2`
        // classification at the `admin_table_index.rs` dispatch site only
        // covered the `InProgress` write; nothing here ever wrote `Succeeded`,
        // so polling after a successful index2 drop returned `Unknown`
        // forever). Mirrors #1069's write-order fix (write BEFORE the
        // tombstone clear) for the base_index/sorted families.
        if let Some(ref id) = op_id {
            let status = shamir_query_types::read::DdlOpStatus {
                op_id: *id,
                kind: shamir_query_types::read::DdlOpKind::DropIndex2 {
                    index_name: name.to_string(),
                },
                state: shamir_query_types::read::DdlOpState::Succeeded {
                    completed_at: unix_millis(),
                },
            };
            // Do not swallow status-write errors — log loudly.
            if let Err(e) =
                crate::table::ddl_op_log::write_op_status(&self.info_store, &status).await
            {
                log::error!(
                    "#1067: DROP INDEX (index2) '{name}' succeeded, but failed to write \
                     Succeeded status: {e}. The drop completed successfully, but polling by \
                     op_id {id:?} will return Unknown. Call TableManager::verify() to confirm \
                     the index was dropped."
                );
            }
        }

        // P0-3b (#988): clear the tombstone AFTER the reduced metadata is
        // durably persisted. If this fails, the tombstone remains — recovery
        // will just clear it (a no-op on the already-finished drop).
        crate::index2::persistence::clear_from_dropping_index2(drop_id, &self.info_store)
            .await
            .map_err(|e| {
                shamir_storage::error::DbError::Internal(format!(
                    "DROP INDEX '{name}': the drop is essentially complete (tombstone \
                     persisted, backend retired, sweep done, reduced metadata persisted), \
                     but clearing the drop tombstone failed: {e}. On restart, recovery \
                     will clear the tombstone as a no-op. Call TableManager::verify() \
                     to confirm state."
                ))
            })?;
        Ok(true)
    }
}
