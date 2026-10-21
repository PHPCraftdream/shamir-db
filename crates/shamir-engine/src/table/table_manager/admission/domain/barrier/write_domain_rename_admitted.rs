use std::str::FromStr;

use super::TableManager;
use crate::index::index_info_item::IndexInfoItem;
use crate::index::index_manager::HashRenameTombstone;
use crate::repo::RepoWritePermit;
use shamir_query_types::read::{DdlOpKind, DdlOpState, DdlOpStatus};
use shamir_storage::error::DbResult;
use shamir_types::core::interner::InternerKey;
use shamir_types::time::unix_millis;
use shamir_types::types::record_id::RecordId;

impl TableManager {
    pub async fn recover_hash_renames_admitted(&self, permit: &RepoWritePermit) -> DbResult<()> {
        self.write_domain.debug_check_permit(permit);
        let regular_renames = self.index_manager.load_renaming_list(false).await?;
        let unique_renames = self.index_manager.load_renaming_list(true).await?;

        if regular_renames.is_empty() && unique_renames.is_empty() {
            return Ok(());
        }

        log::info!(
            "#997: recovering {} regular + {} unique in-progress hash RENAME(s)",
            regular_renames.len(),
            unique_renames.len()
        );

        // ── Regular family ──────────────────────────────────────────────
        for entry in &regular_renames {
            let path_refs: Vec<&str> = entry.paths.iter().map(|s| s.as_str()).collect();

            // Step 1: ensure new index is in a clean, Ready state.
            if self.index_exists(&entry.new_name).await {
                let new_id = self.intern_string(&entry.new_name).await?;
                if let Some(def) = self.index_manager.get_index_definition(new_id) {
                    if def.state == crate::index2::state::IndexState::Building {
                        log::warn!(
                            "#997: regular rename target '{}' was left in Building \
                             state by a crashed create — dropping partial and re-creating",
                            entry.new_name
                        );
                        self.drop_index_admitted(&entry.new_name, None, permit)
                            .await?;
                        self.create_index_admitted(&entry.new_name, &path_refs, permit)
                            .await?;
                    }
                    // else Ready — already done, leave as-is
                }
            } else {
                // New doesn't exist — crash before/during create. Re-run.
                self.create_index_admitted(&entry.new_name, &path_refs, permit)
                    .await?;
            }

            // Step 2: drop old if still present.
            if self.index_exists(&entry.old_name).await {
                self.drop_index_admitted(&entry.old_name, None, permit)
                    .await?;
            }

            // #1000 test seam — park here (this entry fully reconciled, the
            // tombstone list not yet cleared) if a test installed the
            // between-entries pause hook. This is the exact window a
            // regression test uses to prove a not-yet-processed sibling
            // entry's tombstone survives an interruption here.
            self.index_manager
                .maybe_pause_recover_renames_between_entries()
                .await;
        }

        // Step 3: clear the WHOLE regular tombstone list once, after every
        // loaded entry has been reconciled — NOT per-entry via
        // `clear_from_renaming` (see `IndexManager::clear_all_renaming`'s doc
        // comment for why a per-entry clear during recovery would silently
        // discard not-yet-processed entries: `renaming_regular` is empty at
        // open time, so `clear_from_renaming`'s snapshot-from-in-memory-map
        // would persist `[]` after the FIRST entry, stranding any later
        // entry's tombstone if recovery then failed or crashed again).
        if !regular_renames.is_empty() {
            // Defect 2 (#1069): Write SucceededViaCrashRecovery status BEFORE clearing tombstone.
            // This ensures the terminal status is durable before the tombstone is cleared.
            for entry in &regular_renames {
                if let Some(ref op_id_str) = entry.op_id {
                    if let Ok(op_id) = RecordId::from_str(op_id_str) {
                        let status = DdlOpStatus {
                            op_id,
                            kind: DdlOpKind::RenameHashIndex {
                                old_name: entry.old_name.clone(),
                                new_name: entry.new_name.clone(),
                            },
                            state: DdlOpState::SucceededViaCrashRecovery {
                                completed_at_restart: unix_millis(),
                            },
                        };
                        // Defect 2 (#1069): Do not swallow status-write errors — log loudly.
                        if let Err(e) =
                            crate::table::ddl_op_log::write_op_status(&self.info_store, &status)
                                .await
                        {
                            log::error!(
                                "#1048 / #1069: failed to write SucceededViaCrashRecovery status for \
                                 recovered regular rename '{} → '{}': {}. The rename completed \
                                 successfully, but polling by op_id {:?} will return Unknown. Call \
                                 TableManager::verify() to confirm the index was renamed.",
                                entry.old_name, entry.new_name, e, op_id
                            );
                        }
                    }
                }
            }

            self.index_manager.clear_all_renaming(false).await?;
        }

        // ── Unique family ───────────────────────────────────────────────
        for entry in &unique_renames {
            let path_refs: Vec<&str> = entry.paths.iter().map(|s| s.as_str()).collect();

            // Step 1: ensure new unique index is in a clean, Ready state.
            if self.unique_index_exists(&entry.new_name).await {
                let new_id = self.intern_string(&entry.new_name).await?;
                if let Some(def) = self.index_manager.get_unique_index_definition(new_id) {
                    if def.state == crate::index2::state::IndexState::Building {
                        log::warn!(
                            "#997: unique rename target '{}' was left in Building \
                             state by a crashed create — dropping partial and re-creating",
                            entry.new_name
                        );
                        // Drop + re-create under the barrier (mirrors the
                        // rename path's barrier+lock span).
                        let (_barrier, _uwl_guard) = self
                            .begin_write_barrier_admitted(
                                crate::index::write_barrier_flags::UNIQUE_INDEX_CREATE,
                                permit,
                            )
                            .await;
                        self.index_manager
                            .drop_unique_index(new_id, None, Some(entry.new_name.as_str()))
                            .await?;
                        self.create_unique_index_body(&entry.new_name, &path_refs, permit)
                            .await?;
                    }
                    // else Ready — already done
                }
            } else {
                // New doesn't exist. Either crash before drop (old still
                // present) or crash after drop (SEVERE: both absent).
                // Either way: create new. The backfill checks uniqueness —
                // see the doc comment above for why this is safe.
                self.create_unique_index_admitted(&entry.new_name, &path_refs, permit)
                    .await?;
            }

            // Step 2: drop old if still present.
            if self.unique_index_exists(&entry.old_name).await {
                self.drop_unique_index_admitted(&entry.old_name, None, permit)
                    .await?;
            }

            // #1000 test seam — same between-entries pause point as the
            // regular family above.
            self.index_manager
                .maybe_pause_recover_renames_between_entries()
                .await;
        }

        // Step 3: clear the WHOLE unique tombstone list once, after every
        // loaded entry has been reconciled — same reasoning as the regular
        // family above.
        if !unique_renames.is_empty() {
            // Defect 2 (#1069): Write SucceededViaCrashRecovery status BEFORE clearing tombstone.
            // This ensures the terminal status is durable before the tombstone is cleared.
            for entry in &unique_renames {
                if let Some(ref op_id_str) = entry.op_id {
                    if let Ok(op_id) = RecordId::from_str(op_id_str) {
                        let status = DdlOpStatus {
                            op_id,
                            kind: DdlOpKind::RenameUniqueHashIndex {
                                old_name: entry.old_name.clone(),
                                new_name: entry.new_name.clone(),
                            },
                            state: DdlOpState::SucceededViaCrashRecovery {
                                completed_at_restart: unix_millis(),
                            },
                        };
                        // Defect 2 (#1069): Do not swallow status-write errors — log loudly.
                        if let Err(e) =
                            crate::table::ddl_op_log::write_op_status(&self.info_store, &status)
                                .await
                        {
                            log::error!(
                                "#1048 / #1069: failed to write SucceededViaCrashRecovery status for \
                                 recovered unique rename '{} → '{}': {}. The rename completed \
                                 successfully, but polling by op_id {:?} will return Unknown. Call \
                                 TableManager::verify() to confirm the index was renamed.",
                                entry.old_name, entry.new_name, e, op_id
                            );
                        }
                    }
                }
            }

            self.index_manager.clear_all_renaming(true).await?;
        }

        log::info!(
            "#997: recovery complete — {} regular + {} unique hash RENAME(s) finalized",
            regular_renames.len(),
            unique_renames.len()
        );

        Ok(())
    }

    /// S5c-X admitted twin — caller already holds repo admission.
    pub async fn rename_index_admitted(
        &self,
        old_name: &str,
        new_name: &str,
        op_id: Option<shamir_types::types::record_id::RecordId>,
        permit: &RepoWritePermit,
    ) -> DbResult<()> {
        self.write_domain.debug_check_permit(permit);
        let old_id = self.intern_string(old_name).await?;
        let new_id = self.intern_string(new_name).await?;

        if old_id == new_id {
            // Nothing to do — same interned id means same name.
            return Ok(());
        }

        // ── Classify what kind(s) of index exist under old_name ──────────────
        let is_regular = self.index_manager.index_exists(old_id);
        let is_unique = self.index_manager.unique_index_exists(old_id);
        let is_sorted = self.sorted_indexes.find_by_name_interned(old_id).is_some();
        let is_index2 = self.index2_registry.get_by_name(old_id).await.is_some();

        if !is_regular && !is_unique && !is_sorted && !is_index2 {
            return Err(shamir_storage::error::DbError::Internal(format!(
                "index '{}' not found on this table",
                old_name
            )));
        }

        // R0-C (#1010): refuse instead of silently resolving when `old_name`
        // is a PRE-EXISTING cross-family collision (a name present in more
        // than one of the four families — only reachable on a table that
        // acquired the collision before the #1010 CREATE-time preflight
        // landed, since CREATE now refuses to introduce a new one). Without
        // this guard, the family-specific blocks below (`is_regular`,
        // `is_unique`, `is_sorted`, `is_index2`) each act independently on
        // EVERY family that matches `old_name` — not just one — silently
        // renaming every colliding sibling instead of the single index the
        // caller meant. A full redesign (explicit per-family disambiguator)
        // is out of scope here (tracked as #1025); this is the low-risk
        // "detect and refuse" the R0-C brief permits.
        let matching_families = [is_regular, is_unique, is_sorted, is_index2]
            .iter()
            .filter(|&&m| m)
            .count();
        if matching_families > 1 {
            return Err(shamir_storage::error::DbError::Internal(format!(
                "index '{old_name}' exists in {matching_families} different index families \
                 on this table (a pre-existing cross-family name collision) — RENAME INDEX \
                 cannot safely resolve which one to rename. Run TableManager::verify() to \
                 see the affected families, then drop or rename the colliding sibling(s) \
                 individually before renaming '{old_name}'."
            )));
        }

        // ── Guard: destination name must not be occupied ──────────────────────
        let dst_regular = self.index_manager.index_exists(new_id);
        let dst_unique = self.index_manager.unique_index_exists(new_id);
        let dst_sorted = self.sorted_indexes.find_by_name_interned(new_id).is_some();
        let dst_index2 = self.index2_registry.get_by_name(new_id).await.is_some();

        if dst_regular || dst_unique || dst_sorted || dst_index2 {
            return Err(shamir_storage::error::DbError::Internal(format!(
                "index '{}' already exists on this table; cannot rename '{}' to it",
                new_name, old_name
            )));
        }

        // ── Regular (hash): create-new-first, drop-old-second ───────────────
        // Hash-index keys embed name_interned into hash1/hash2; a raw key
        // rewrite breaks lookup. Rebuild from the live record stream instead.
        //
        // Order matters for concurrency (audit A9): `create_index` now
        // registers the new definition FIRST (Option A), so the live
        // write-hook immediately starts maintaining postings for the NEW
        // index. By creating the new index BEFORE dropping the old one,
        // there is NEVER a window where a concurrent write is invisible to
        // BOTH the old and new indexes — during the brief overlap, a write
        // goes to both (postings are idempotent: same (index_key, record_id)
        // → same physical key, empty value). Dropping the old afterward
        // only removes old-id-prefixed postings, leaving the new index intact.
        if is_regular {
            let old_def = self
                .index_manager
                .get_index_definition(old_id)
                .ok_or_else(|| {
                    shamir_storage::error::DbError::Internal(
                        "index definition disappeared mid-rename".to_string(),
                    )
                })?;
            let interner = self.interner.get().await?;
            let paths = resolve_index_paths(interner, &old_def.paths);
            let path_refs: Vec<&str> = paths.iter().map(|s| s.as_str()).collect();

            // #997: write a durable rename tombstone BEFORE the first mutating
            // step (create_index). The tombstone carries the resolved string
            // names + paths so recovery can rebuild from nothing if the crash
            // strands it (unlike sorted's id-pair tombstone, a hash rename is
            // drop+rebuild — the old def is gone by the time the tombstone
            // matters). Cleared AFTER the last step succeeds. Mirrors #962's
            // durable "Renaming" tombstone (sorted) and #959's `idx_drop`
            // (same IndexManager owner).
            self.index_manager
                .add_to_renaming(
                    false,
                    old_id,
                    HashRenameTombstone {
                        old_name: old_name.to_string(),
                        new_name: new_name.to_string(),
                        paths: paths.clone(),
                        op_id: op_id.as_ref().map(|id| id.to_string()),
                    },
                )
                .await?;

            // Create the new index FIRST (registers + backfills under new_id).
            self.create_index_admitted(new_name, &path_refs, permit)
                .await?;

            // #997 test seam — park here (tombstone written, new index created,
            // old index NOT yet dropped) if a test installed the pause hook.
            // This is the EXACT crash window recovery tests simulate.
            self.index_manager.maybe_pause_rename_mid().await;

            // Then drop the old index (removes old-id postings only).
            // P1-2 (#967): if this fails after create_index succeeded, both
            // old and new indexes exist — enrich the error with context AND
            // write a structured `Failed { detail }` to the op-status log.
            match self
                .index_manager
                .drop_index(old_id, None, Some(old_name))
                .await
            {
                Ok(_) => {}
                Err(e) => {
                    let detail = format!(
                        "RENAME INDEX '{old_name}' → '{new_name}': the new index \
                         '{new_name}' was created successfully, but dropping the old \
                         index '{old_name}' failed: {e}. Both indexes now exist — \
                         call TableManager::verify() to confirm state."
                    );
                    // Write structured failure status if op_id is available.
                    if let Some(ref id) = op_id {
                        let status = shamir_query_types::read::DdlOpStatus {
                            op_id: *id,
                            kind: shamir_query_types::read::DdlOpKind::RenameHashIndex {
                                old_name: old_name.to_string(),
                                new_name: new_name.to_string(),
                            },
                            state: shamir_query_types::read::DdlOpState::Failed {
                                detail: detail.clone(),
                            },
                        };
                        if let Err(write_err) =
                            crate::table::ddl_op_log::write_op_status(&self.info_store, &status)
                                .await
                        {
                            log::error!(
                                "#967: failed to write Failed status for RENAME '{old_name}' → '{new_name}': {write_err}",
                            );
                        }
                    }
                    return Err(shamir_storage::error::DbError::Internal(detail));
                }
            }

            // #1069: Write terminal Succeeded status BEFORE clearing tombstone.
            // This ensures crash-safety for the inline path: if we crash after this
            // write but before tombstone clear, the status is durable.
            if let Some(ref id) = op_id {
                let status = shamir_query_types::read::DdlOpStatus {
                    op_id: *id,
                    kind: shamir_query_types::read::DdlOpKind::RenameHashIndex {
                        old_name: old_name.to_string(),
                        new_name: new_name.to_string(),
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
                        "#1069: RENAME INDEX (regular) '{}' → '{}' succeeded, but failed to write \
                         Succeeded status: {}. The rename completed successfully, but polling by \
                         op_id {:?} will return Unknown. Call TableManager::verify() to confirm \
                         the index was renamed.",
                        old_name,
                        new_name,
                        e,
                        id
                    );
                }
            }

            // #997: clear the rename tombstone now that the rename is durable.
            // If this fails, the tombstone remains — recovery will reconcile.
            self.index_manager
                .clear_from_renaming(false, old_id)
                .await
                .map_err(|e| {
                    shamir_storage::error::DbError::Internal(format!(
                        "RENAME INDEX '{old_name}' → '{new_name}': the rename is \
                         complete (new index created, old dropped), but clearing \
                         the rename tombstone failed: {e}. On restart, recovery \
                         will reconcile. Call TableManager::verify() to confirm state."
                    ))
                })?;
        }

        // ── Unique (hash): write-barrier across drop→backfill→register ──────
        // Unique-index uniqueness validation during backfill is NOT safely
        // idempotent — a duplicate slipping through the gap is a correctness
        // violation of the uniqueness guarantee, not a harmless double-write.
        // So we hold the table-wide `unique_write_lock` across the ENTIRE
        // drop→backfill→register sequence (Option B), preventing ANY writer
        // (non-tx insert, tx commit Phase 2.5) from inserting a duplicate
        // while the unique index is between its old and new registered states.
        // No gap = no duplicates possible. Acceptable cost for a low-frequency
        // DDL operation.
        if is_unique {
            let old_def = self
                .index_manager
                .get_unique_index_definition(old_id)
                .ok_or_else(|| {
                    shamir_storage::error::DbError::Internal(
                        "unique index definition disappeared mid-rename".to_string(),
                    )
                })?;
            let interner = self.interner.get().await?;
            let paths = resolve_index_paths(interner, &old_def.paths);
            let path_refs: Vec<&str> = paths.iter().map(|s| s.as_str()).collect();

            // #997: write the durable rename tombstone BEFORE the barrier (the
            // tombstone write is an I/O op; holding the barrier during it is a
            // real cost). A crash after the tombstone write but before the
            // barrier/drop is safe: nothing was mutated yet, and recovery sees
            // the tombstone + old still present → re-runs the rename.
            self.index_manager
                .add_to_renaming(
                    true,
                    old_id,
                    HashRenameTombstone {
                        old_name: old_name.to_string(),
                        new_name: new_name.to_string(),
                        paths: paths.clone(),
                        op_id: op_id.as_ref().map(|id| id.to_string()),
                    },
                )
                .await?;

            // Hold the barrier bit + unique_write_lock across drop + create.
            // This is the SAME lock non-tx writers acquire
            // (table_manager_crud.rs) and the tx commit pipeline acquires
            // (Phase 2.5), so it blocks all writer classes for the rename's
            // duration.
            //
            // F-70 (#897, P0): acquired via the canonical drain-then-lock
            // path (`begin_write_barrier`) — raise `UNIQUE_INDEX_CREATE`,
            // drain in-flight fast-path writers, THEN take
            // `unique_write_lock` — instead of lock-then-drain (F-57, #883's
            // original order, which deadlocks against `pre_commit_prelock`'s
            // drain-guard-then-lock shape on a second table). Holding the
            // guards across BOTH `drop_unique_index` AND the create body is a
            // SEPARATE, still-valid requirement (uniqueness-gap atomicity,
            // audit A9) — unrelated to drain ordering: the drain always
            // completes strictly before this span begins, regardless of how
            // long the span itself then holds the lock for.
            let (_barrier, _uwl_guard) = self
                .begin_write_barrier_admitted(
                    crate::index::write_barrier_flags::UNIQUE_INDEX_CREATE,
                    permit,
                )
                .await;

            self.index_manager
                .drop_unique_index(old_id, None, Some(old_name))
                .await?;

            // #997 test seam — park here (tombstone written, old dropped,
            // new NOT yet created) if a test installed the pause hook. This
            // is the EXACT SEVERE crash window: neither index exists.
            self.index_manager.maybe_pause_rename_mid().await;

            // Use the body variant: the barrier + lock are already held
            // above, and `create_unique_index` would re-acquire the lock →
            // deadlock (`tokio::sync::Mutex` is NOT reentrant).
            // P1-2 (#967): if this fails after drop_unique_index succeeded,
            // the old index is gone but the new one doesn't exist — enrich the
            // error with context AND write a structured `Failed { detail }`
            // to the op-status log.
            match self
                .create_unique_index_body(new_name, &path_refs, permit)
                .await
            {
                Ok(_) => {}
                Err(e) => {
                    let detail = format!(
                        "RENAME INDEX '{old_name}' → '{new_name}': the old unique \
                         index '{old_name}' was dropped, but creating the new \
                         unique index '{new_name}' failed: {e}. The old index is \
                         gone — call TableManager::verify() to confirm state, or \
                         re-create the index manually."
                    );
                    // Write structured failure status if op_id is available.
                    if let Some(ref id) = op_id {
                        let status = shamir_query_types::read::DdlOpStatus {
                            op_id: *id,
                            kind: shamir_query_types::read::DdlOpKind::RenameUniqueHashIndex {
                                old_name: old_name.to_string(),
                                new_name: new_name.to_string(),
                            },
                            state: shamir_query_types::read::DdlOpState::Failed {
                                detail: detail.clone(),
                            },
                        };
                        if let Err(write_err) =
                            crate::table::ddl_op_log::write_op_status(&self.info_store, &status)
                                .await
                        {
                            log::error!(
                                "#967: failed to write Failed status for unique RENAME '{old_name}' → '{new_name}': {write_err}",
                            );
                        }
                    }
                    return Err(shamir_storage::error::DbError::Internal(detail));
                }
            }

            // #1069: Write terminal Succeeded status BEFORE clearing tombstone.
            // This ensures crash-safety for the inline path.
            if let Some(ref id) = op_id {
                let status = shamir_query_types::read::DdlOpStatus {
                    op_id: *id,
                    kind: shamir_query_types::read::DdlOpKind::RenameUniqueHashIndex {
                        old_name: old_name.to_string(),
                        new_name: new_name.to_string(),
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
                        "#1069: RENAME INDEX (unique) '{}' → '{}' succeeded, but failed to write \
                         Succeeded status: {}. The rename completed successfully, but polling by \
                         op_id {:?} will return Unknown. Call TableManager::verify() to confirm \
                         the index was renamed.",
                        old_name,
                        new_name,
                        e,
                        id
                    );
                }
            }

            // #997: clear the rename tombstone now that the rename is durable.
            // The barrier + lock are still held here (they release when the
            // guards drop at end of block), but the clear is just a Store::set
            // under a separate key — no contention concern.
            self.index_manager
                .clear_from_renaming(true, old_id)
                .await
                .map_err(|e| {
                    shamir_storage::error::DbError::Internal(format!(
                        "RENAME INDEX '{old_name}' → '{new_name}': the unique rename \
                         is complete (old dropped, new created), but clearing the \
                         rename tombstone failed: {e}. On restart, recovery will \
                         reconcile. Call TableManager::verify() to confirm state."
                    ))
                })?;
        }

        // ── Rekey sorted index posting entries ────────────────────────────────
        // P0-5b (#962): the full sorted-rename orchestration (durable "Renaming"
        // tombstone → definition swap → settle-loop rekey → tombstone clear)
        // now lives in `SortedIndexManager::rename_index_sorted`, mirroring how
        // `drop_sorted_index` delegates to `drop_index`. The durable tombstone
        // makes an interrupted rekey resumable on restart via
        // `recover_in_progress_renames` (previously a rekey `Err` after the
        // definition swap orphaned postings permanently). Concurrency (audit
        // A9): the definition is renamed FIRST (atomic RCU swap), so the live
        // write-hook starts writing under new_id immediately; the rekey's
        // settle re-scan then catches any old-id entry a concurrent writer
        // landed in the brief rename→rekey window.
        //
        // R0-A (#1012): wrapped in `begin_write_barrier(SORTED_INDEX_CREATE)` —
        // reuses the family's CREATE bit (same reasoning as `drop_sorted_index`),
        // held across the ENTIRE tombstone → definition-swap → rekey →
        // tombstone-clear sequence inside `rename_index_sorted`. Before this
        // fix, nothing serialized this rename against a concurrent CREATE/
        // DROP/RENAME on the same table's sorted family.
        if is_sorted {
            let (_barrier, _uwl_guard) = self
                .begin_write_barrier_admitted(
                    crate::index::write_barrier_flags::SORTED_INDEX_CREATE,
                    permit,
                )
                .await;
            // #1067: thread op_id + the resolved string names down so the
            // terminal DdlOpStatus write (inside rename_index_sorted, before
            // its own tombstone clear) carries the caller's correlation id —
            // mirrors how the is_unique/regular branches above already pass
            // their own op_id into their respective drop/create calls.
            self.sorted_indexes
                .rename_index_sorted(
                    old_id,
                    new_id,
                    op_id.as_ref().map(|id| id.to_string()),
                    Some(old_name),
                    Some(new_name),
                )
                .await?;
        }

        // ── Rekey index2 (FTS / functional / vector) ──────────────────────────
        // Physical posting entries are keyed by `index_id` (u32), not by
        // name_interned — no data movement needed. Only the by_name lookup
        // table in the registry changes, plus the persisted metadata.
        //
        // R0-A (#1012): wrapped in `begin_write_barrier(INDEX2_CREATE)` —
        // reuses the family's CREATE bit (same reasoning as `drop_index2`),
        // held across the `rename_entry` registry mutation AND the metadata
        // persist below. Before this fix, nothing serialized this rename
        // against a concurrent CREATE/DROP/RENAME on the same table's index2
        // family — two registry-mutating ops could race `IndexRegistry`'s
        // ticket/generation bookkeeping.
        //
        // #1066: `rename_entry` (mutates the live registry) and
        // `save_index2_metadata` (persists it) are two sequential,
        // non-atomic steps with no rollback between them. A durable RENAME
        // tombstone (mirroring index2 DROP's `add_to_dropping_index2` /
        // `clear_from_dropping_index2`) is written BEFORE `rename_entry` and
        // cleared AFTER `save_index2_metadata` succeeds, so a storage error
        // or task cancellation anywhere in this window is recoverable via
        // `recover_index2_renames` on the next table open.
        if is_index2 {
            let (_barrier, _uwl_guard) = self
                .begin_write_barrier_admitted(
                    crate::index::write_barrier_flags::INDEX2_CREATE,
                    permit,
                )
                .await;

            // #1066: `new_id` (interned above via `self.intern_string(new_name)`
            // at the top of this function) MUST be durable BEFORE anything
            // below references it — otherwise a crash before the interner's
            // next incidental persist leaves `new_name`'s interned id
            // unrecoverable, and a FRESH interner minted on restart can
            // assign that SAME numeric id to a completely different string
            // (the interner's id counter resumes from the last persisted
            // entry). `recover_index2_renames` would then silently compare
            // against the wrong string. Mirrors `create_index`/
            // `create_unique_index_body`'s own `self.interner.persist()`
            // call before publishing a newly-interned name.
            self.interner.persist().await?;

            // Re-resolve the backend to get its u32 descriptor id — reuse
            // the SAME lookup `is_index2` was computed from above (by
            // old_id), not a different derivation (e.g. via new_id or a
            // fresh lookup by new_name).
            let backend = self
                .index2_registry
                .get_by_name(old_id)
                .await
                .ok_or_else(|| {
                    shamir_storage::error::DbError::Internal(
                        "index2 backend disappeared mid-rename".to_string(),
                    )
                })?;
            let index2_id = backend.descriptor().id;

            // #1066: write the durable rename tombstone BEFORE rename_entry
            // mutates the live registry, so a crash/error at any subsequent
            // point is recoverable. Mirrors add_to_dropping_index2's
            // ordering guarantee. Nothing has moved yet at this point, so
            // propagating a raw error on failure is safe.
            crate::index2::persistence::add_to_renaming_index2(
                index2_id,
                old_name.to_string(),
                new_name.to_string(),
                op_id.as_ref().map(|id| id.to_string()),
                &self.info_store,
            )
            .await?;

            // #1066 test seam — park here (tombstone written, registry NOT
            // yet mutated) if a test installed the early pause hook. This is
            // the crash window where both registry and disk still show the
            // OLD name but the tombstone is already durable.
            #[cfg(test)]
            if let Some(hook) = self.rename_index2_early_pause_hook.load_full() {
                hook.wait_at_window().await;
            }

            // rename_entry moves the by_name mapping old_id → new_id AND
            // updates the authoritative name slots in the by_id entry so the
            // rename survives `save_index2_metadata` (P0-5a / #961: without
            // the by_id update, the persisted descriptor carried the stale
            // original name and the rename was silently reverted on reopen).
            let ok = self
                .index2_registry
                .rename_entry(old_id, new_name.to_string(), new_id)
                .await;
            if !ok {
                return Err(shamir_storage::error::DbError::Internal(
                    "index2 rename_entry failed (concurrent conflict?)".to_string(),
                ));
            }

            // #1066 test seam — park here (registry mutated to the NEW
            // name, metadata NOT yet persisted) if a test installed the mid
            // pause hook. This is the EXACT crash window the tombstone
            // closes: registry shows new_name, disk still shows old_name.
            #[cfg(test)]
            if let Some(hook) = self.rename_index2_mid_pause_hook.load_full() {
                hook.wait_at_window().await;
            }

            crate::index2::persistence::save_index2_metadata(
                &self.index2_registry,
                &self.info_store,
            )
            .await?;

            // #1067: Write terminal Succeeded status AFTER save_index2_metadata
            // succeeds but BEFORE clearing the rename tombstone — the same
            // before-the-clear write-order discipline #1069 established for
            // the is_unique/regular branches above (and #1066 itself already
            // used for the tombstone clear position). Deferred by #1066,
            // which built the tombstone/recovery mechanics but explicitly
            // left the DdlOpStatus integration to this task.
            if let Some(ref id) = op_id {
                let status = shamir_query_types::read::DdlOpStatus {
                    op_id: *id,
                    kind: shamir_query_types::read::DdlOpKind::RenameIndex2 {
                        old_name: old_name.to_string(),
                        new_name: new_name.to_string(),
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
                        "#1067: RENAME INDEX (index2) '{}' → '{}' succeeded, but failed to write \
                         Succeeded status: {}. The rename completed successfully, but polling by \
                         op_id {:?} will return Unknown. Call TableManager::verify() to confirm \
                         the index was renamed.",
                        old_name,
                        new_name,
                        e,
                        id
                    );
                }
            }

            // #1066: clear the tombstone now that the rename is durable. If
            // this fails, log loudly but do NOT fail the rename — it
            // already succeeded; recovery reconciles the stale-but-harmless
            // tombstone on next restart.
            if let Err(e) =
                crate::index2::persistence::clear_from_renaming_index2(index2_id, &self.info_store)
                    .await
            {
                log::error!(
                    "#1066: RENAME INDEX (index2) '{old_name}' -> '{new_name}': the rename is \
                     complete, but clearing the rename tombstone failed: {e}. On restart, \
                     recovery will reconcile. Call TableManager::verify() to confirm state."
                );
            }
        }

        Ok(())
    }
}

/// Resolve interned path ids back to dot-separated string paths.
///
/// Used by `rename_index` to capture the field paths of a hash index
/// before drop+rebuild: the `IndexDefinition.paths` are `Vec<IndexInfoItem>`
/// whose segments are interned u64 ids. We resolve each segment through the
/// interner to recover the original string path (e.g. `"user.email"`).
fn resolve_index_paths(
    interner: &shamir_types::core::interner::Interner,
    paths: &[IndexInfoItem],
) -> Vec<String> {
    paths
        .iter()
        .map(|item| {
            item.path
                .iter()
                .map(|&id| {
                    interner
                        .get_str(&InternerKey::new(id))
                        .map(|s| (*s).to_string())
                        .unwrap_or_default()
                })
                .collect::<Vec<_>>()
                .join(".")
        })
        .collect()
}
