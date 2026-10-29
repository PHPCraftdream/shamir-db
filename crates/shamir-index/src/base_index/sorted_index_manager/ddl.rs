use bytes::Bytes;
use futures::StreamExt;
use std::sync::atomic::Ordering;

use super::SortedIndexDefinition;
use shamir_storage::error::DbResult;
use shamir_storage::types::{KvOp, RecordKey};
use shamir_tunables::store_defaults::{FULL_SCAN_BATCH, MAINT_SCAN_BATCH};
use shamir_types::types::record_id::RecordId;

impl super::inner::SortedIndexManager {
    // ========================================================================
    // P0-3b (#972) — DROP INDEX durable tombstone + crash recovery (sorted)
    // ========================================================================
    //
    // Mirrors #959's base_index-family fix for the sorted family.
    //
    // Sub-bug 3c (crash-resurrection): the OLD `drop_index` had retire →
    // sweep → `persist_defs` ordering. A crash between the sweep and the
    // `persist_defs` call left the on-disk defs listing the index as present
    // while its postings were gone — on restart `load()` would restore the
    // stale `Ready` definition and the planner would route range/order/min
    // queries to an index with zero postings, silently returning wrong
    // (empty / missing) results with no error. The durable tombstone (written
    // BEFORE the sweep, cleared AFTER the persist) closes this: on restart,
    // `recover_in_progress_drops` sees the tombstone, resumes the sweep
    // (idempotent), removes the stale definition, persists, and clears the
    // tombstone.
    //
    // Sub-bug 3b (name-reuse ghost postings): a name in the tombstone set is
    // rejected by `register` (the sorted CREATE entry point) until the
    // tombstone clears, preventing ghost postings from namespace reuse.
    //
    // Tombstone shape: `Vec<u64>` serialized via bincode under
    // `system:sidx_drop`. A separate key was chosen (mirroring #959's
    // `idx_drop`/`uidx_drop`) to avoid touching `persist_defs`/`load`'s
    // three-tier bincode forward-compat fallback chain (current-shape →
    // pre-`state` → V1). An absent key or empty vec means "no in-progress
    // drops".
    //
    // **CRITICAL** (the hard-won #959 collision lesson): the key name
    // `"sidx_drop"` is short by design — `RecordId::system(name)` truncates
    // `name` to 12 bytes (see `shamir-types::types::record_id::system`). The
    // only other sorted system key is `"sorted_indexes"` (truncates to
    // `"sorted_index"`), which differs from `"sidx_drop"` at byte index 1
    // (`o` vs `i`) — NO collision. Verified against every
    // `RecordId::system(...)` call site in the workspace (the #959 review
    // caught a real `"indexes_unique_dropping"` ↔ `"indexes_unique"` clash
    // that both truncate to `"indexes_uniq"`).

    /// P0-3b (#972): persist the current in-memory `dropping_sorted` set to
    /// `info_store` under `system:sidx_drop`. Serializes the set as a
    /// `Vec<u64>` (deterministic order via `BTreeSet`). An empty set writes
    /// an empty `Vec<u64>` (NOT a key deletion) so the load path handles
    /// both `NotFound` and empty-vec uniformly. Mirrors #959's
    /// `save_dropping_regular`.
    pub(super) async fn save_dropping_sorted(&self) -> DbResult<()> {
        let snapshot: Vec<u64> = {
            let set = self.dropping_sorted.lock().unwrap();
            set.iter().copied().collect()
        };
        let key = RecordId::system("sidx_drop").to_bytes();
        let bytes = bincode::serialize(&snapshot)
            .map_err(|e| shamir_storage::error::DbError::Codec(e.to_string()))?;
        self.info_store.set(key.into(), Bytes::from(bytes)).await?;
        Ok(())
    }

    /// P0-3b (#972): load a persisted dropping set from `info_store`. Returns
    /// an empty `Vec` if the key is absent (`NotFound`) or contains an empty
    /// vec — both mean "no in-progress drops". Mirrors #959's
    /// `load_dropping_set`.
    pub(super) async fn load_dropping_sorted(&self) -> DbResult<Vec<u64>> {
        let key = RecordId::system("sidx_drop").to_bytes();
        match self.info_store.get(key.into()).await {
            Ok(bytes) => {
                if bytes.is_empty() {
                    return Ok(Vec::new());
                }
                bincode::deserialize::<Vec<u64>>(&bytes).map_err(|e| {
                    shamir_storage::error::DbError::Codec(format!(
                        "system:sidx_drop decode failed: {e}"
                    ))
                })
            }
            Err(shamir_storage::error::DbError::NotFound(_)) => Ok(Vec::new()),
            Err(e) => Err(e),
        }
    }

    /// P0-3b (#972): add `name_interned` to the in-memory dropping set, then
    /// persist the updated set durably. MUST be called BEFORE the sweep in
    /// `drop_index` so a crash at any subsequent point is recoverable.
    ///
    /// In-memory update happens FIRST (so a concurrent `register`
    /// immediately sees the name as guarded), then the persist follows. If
    /// the persist fails, the in-memory update is rolled back and the caller
    /// receives `Err` — the drop MUST NOT proceed without a durable
    /// tombstone. A crash between the in-memory update and the persist is
    /// safe: nothing has been swept or persisted yet, so the on-disk state
    /// is unchanged (old defs, postings intact). Mirrors #959's
    /// `add_to_dropping`.
    pub(super) async fn add_to_dropping_sorted(&self, name_interned: u64) -> DbResult<()> {
        {
            let mut set = self.dropping_sorted.lock().unwrap();
            set.insert(name_interned);
        }
        let result = self.save_dropping_sorted().await;
        if result.is_err() {
            // Roll back the in-memory set so the guard stays consistent.
            let mut set = self.dropping_sorted.lock().unwrap();
            set.remove(&name_interned);
        }
        result
    }

    /// P0-3b (#972): clear `name_interned` from the persisted tombstone, then
    /// from the in-memory set. Persist-first ordering ensures the on-disk
    /// state is always at least as advanced as the in-memory state: a crash
    /// between persist and in-memory update leaves a stale in-memory entry
    /// that dies with the process, while the on-disk tombstone is already
    /// correct.
    ///
    /// MUST be called AFTER `persist_defs` (the reduced defs must be durable
    /// first). If the process crashes between the defs persist and this
    /// clear, recovery sees the tombstone but the def is already gone from
    /// the persisted defs — it just clears the tombstone (a no-op sweep).
    /// Mirrors #959's `clear_from_dropping`.
    pub(super) async fn clear_from_dropping_sorted(&self, name_interned: u64) -> DbResult<()> {
        // Compute the snapshot without the entry (do NOT modify in-memory yet).
        let snapshot: Vec<u64> = {
            let set = self.dropping_sorted.lock().unwrap();
            set.iter()
                .filter(|&&k| k != name_interned)
                .copied()
                .collect()
        };
        // Persist first.
        let key = RecordId::system("sidx_drop").to_bytes();
        let bytes = bincode::serialize(&snapshot)
            .map_err(|e| shamir_storage::error::DbError::Codec(e.to_string()))?;
        self.info_store.set(key.into(), Bytes::from(bytes)).await?;
        // Only now update in-memory.
        {
            let mut set = self.dropping_sorted.lock().unwrap();
            set.remove(&name_interned);
        }
        Ok(())
    }

    /// P0-3b (#972): sweep all posting entries for the given index prefix.
    /// Extracted from `drop_index` so the recovery path can re-run it
    /// idempotently. Removing already-removed keys is a no-op on every
    /// `Store` backend. Mirrors #959's `sweep_index_postings`.
    pub(super) async fn sweep_sorted_postings(&self, name_interned: u64) -> DbResult<()> {
        let prefix = self.entry_prefix(name_interned);
        let stream = self.info_store.scan_prefix_stream(prefix, MAINT_SCAN_BATCH);
        futures::pin_mut!(stream);
        // `RecordKey` (scan yields store keys, consumed by `remove_many`).
        let mut to_drop: Vec<RecordKey> = Vec::new();
        while let Some(batch) = stream.next().await {
            for (k, _) in batch? {
                to_drop.push(k);
            }
        }
        if !to_drop.is_empty() {
            // Ok-value (removed entries) intentionally discarded; ? propagates errors.
            let _ = self.info_store.remove_many(to_drop).await?;
        }
        Ok(())
    }

    /// Drop a sorted index definition AND every entry written under
    /// it. O(I) where I is the size of the index.
    ///
    /// Process (F-76 / #903 — definition retired BEFORE the posting sweep,
    /// the mirror image of F-72's CREATE gate; P0-3b / #972 — durable
    /// tombstone closes the crash-resurrection gap):
    /// 1. Fast existence check.
    /// 2. **P0-3b**: persist a durable tombstone (`system:sidx_drop`)
    ///    recording that `name_interned` is being dropped — MUST succeed
    ///    before the sweep, so a crash at any later point is recoverable.
    /// 3. Retires the definition from the in-memory Vec (planner-invisible
    ///    from this moment — RCU swap publishes a Vec without this def).
    /// 4. Sweeps all posting entries from `info_store` (now orphan,
    ///    planner-invisible).
    /// 5. Persists the reduced defs (def removed).
    /// 6. **P0-3b**: clears the tombstone.
    ///
    /// F-76 (#903): the RCU `indexes` swap (definition retirement) runs
    /// BEFORE the posting sweep, so a concurrent reader can never observe a
    /// registered-but-emptied sorted index (the in-flight-reader race this
    /// closes is documented on #959's `IndexManager::drop_index` doc — the
    /// same 3a residual applies here).
    ///
    /// P0-3b (#972) sub-bug 3c — crash-resurrection: the OLD code had a gap
    /// between the sweep (step 4) and the defs persist (step 5). If the
    /// process crashed in that gap, the on-disk defs still listed the index
    /// as `Ready`, but all its postings were gone — `SortedIndexManager::new`
    /// would `load()` the stale `Ready` definition and the planner would
    /// route queries to an index with zero postings, silently returning
    /// wrong (empty / missing) results. The durable tombstone (step 2)
    /// closes this: on restart, `recover_in_progress_drops` sees the
    /// tombstone, resumes the sweep (idempotent), removes the stale
    /// definition, persists, and clears the tombstone.
    ///
    /// #1067: accepts `op_id`/`index_name` so the terminal `Succeeded`
    /// status can be written BEFORE the tombstone clear (mirrors #1069's
    /// write-order fix for the base_index `IndexManager::drop_index`/
    /// `drop_unique_index` — see this same function's step 6 below).
    pub async fn drop_index(
        &self,
        name_interned: u64,
        op_id: Option<String>,
        index_name: Option<&str>,
    ) -> DbResult<bool> {
        // Fast existence check. TOCTOU-safe under the engine's write barrier
        // (drop_sorted_index is serialized via begin_write_barrier); mirrors
        // #959's base_index `if !self.indexes.contains(...)`.
        if self.find_by_name_interned(name_interned).is_none() {
            return Ok(false);
        }

        // P0-3b (#972): write a durable tombstone BEFORE retiring the
        // definition or sweeping postings. If the process crashes after the
        // sweep but before the reduced defs are persisted, the on-disk
        // metadata still lists the index — but the tombstone tells
        // `recover_in_progress_drops` to finish the drop rather than
        // resurrecting a broken "Ready but no postings" index. MUST succeed
        // before proceeding; `add_to_dropping_sorted` rolls back the
        // in-memory set on persist failure.
        self.add_to_dropping_sorted(name_interned).await?;

        // P0-3a (#1037) step 2.5: raise the reader-drain gate's intent flag
        // (SeqCst) BEFORE the RCU retire below. From this point every NEW
        // sorted-index reader observes the flag and backs off (empty result
        // → caller falls back to a full scan). The flag stays up until
        // `drain_guard` is dropped (step 4.5 + RAII safety net on early `?`).
        // See `crate::reader_drain_gate`'s module doc for the memory-model
        // proof and the deadlock-exclusion placement invariant.
        let drain_guard = self.reader_gate.begin_drop();

        // F-76 (#903): retire the definition from the planner-visible Vec
        // FIRST (RCU swap publishes a Vec without this definition atomically).
        let mut existed = false;
        self.indexes.rcu(|cur| {
            let initial_len = cur.len();
            let new_vec: Vec<SortedIndexDefinition> = cur
                .iter()
                .filter(|d| d.name_interned != name_interned)
                .cloned()
                .collect();
            existed = new_vec.len() != initial_len;
            new_vec
        });
        if !existed {
            // Race: the def vanished between our pre-check and the rcu (only
            // possible if a concurrent un-serialized caller violated the
            // engine's write-barrier contract). Undo the tombstone we just
            // wrote and report not-found — nothing was swept or persisted.
            // The RAII guard is dropped here on early return (step 4.5's safety
            // net), clearing the flag.
            self.clear_from_dropping_sorted(name_interned).await?;
            return Ok(false);
        }
        // F-50 Step 2 (#870): the queryable def set changed — advance the
        // generation so the commit pipeline's re-derivation gate fires for
        // any tx that staged before this drop.
        self.generation.fetch_add(1, Ordering::AcqRel);

        // P0-3b (#972) test seam — park here (definition already retired,
        // postings not yet swept) if a test installed the pre-sweep hook.
        // This is the window sub-bug 3b's namespace-reuse test parks in
        // (tombstone written + def retired, sweep pending). NOT
        // `#[cfg(test)]`-gated — cross-crate test consumer.
        if let Some(hook) = self.drop_index_pause_hook.load_full() {
            hook.wait_at_window().await;
        }

        // P0-3a (#1037) step 3.5: drain every reader that entered one of the
        // 8 sorted-index read chokepoints BEFORE the flag went up (the gate's
        // memory model guarantees such a reader is counted in `in_flight`,
        // so this wait terminates — see the livelock-freedom argument in the
        // gate's module doc). AFTER the RCU retire and the pause-hook park,
        // BEFORE the sweep, so the physical sweep cannot start until no
        // reader is mid-scan of the shared keyspace. Bounded by the gate's
        // escalating log; this wait sits inside the already-held
        // `ddl_admission` critical section exactly as the mirror-image
        // `WriterDrainBarrier::drain` does.
        drain_guard.wait_for_drain().await;

        // Sweep the (now orphan, planner-invisible) posting entries.
        // P0-3b (#972): extracted to `sweep_sorted_postings` so the recovery
        // path can re-run it idempotently.
        // P1-2 (#967): a durable tombstone is already persisted — enrich
        // the error if this sweep fails.
        // NOTE: Cannot write DdlOpState::Failed here because this layer
        // (SortedIndexManager) does not have op_id in scope.
        self.sweep_sorted_postings(name_interned)
            .await
            .map_err(|e| {
                let detail = format!(
                    "DROP SORTED INDEX '{name_interned}': a durable drop tombstone \
                 was persisted and the definition was retired from the planner, \
                 but the posting sweep failed: {e}. On restart, recovery will \
                 resume the sweep idempotently and finish the drop. Call \
                 TableManager::verify() to confirm state."
                );
                log::error!("{}", detail);
                crate::backend::contextualize_db_error(e, detail)
            })?;

        // P0-3a (#1037) step 4.5: the sweep finished — explicitly release the
        // drain guard now so the (unrelated) persist / tombstone-clear steps
        // below run with the gate already clear. RAII (`drain_guard`'s `Drop`
        // clears the flag) remains the safety net for every early `?` return
        // above (including the `!existed` rollback branch), but an explicit
        // early drop here keeps the drain window as tight as possible.
        drop(drain_guard);

        // P0-3b (#972) test seam — park here (sweep complete, reduced defs
        // NOT yet persisted) if a test installed the post-sweep hook. This is
        // the exact crash window sub-bug 3c exercises: a "crash" here
        // (dropping the manager) leaves the tombstone on disk but the old
        // defs, and the recovery path in `new()` must finish the drop. NOT
        // `#[cfg(test)]`-gated — cross-crate test consumer.
        if let Some(hook) = self.drop_index_post_sweep_hook.load_full() {
            hook.wait_at_window().await;
        }

        // Persist the reduced defs (definition removed).
        // P1-2 (#967): the tombstone is still in place — if this persist
        // fails, recovery will see the tombstone and finish the drop.
        // NOTE: Cannot write DdlOpState::Failed here because this layer
        // (SortedIndexManager) does not have op_id in scope.
        self.persist_defs().await.map_err(|e| {
            let detail = format!(
                "DROP SORTED INDEX '{name_interned}': a durable drop tombstone \
                 was persisted, the definition was retired, and the posting sweep \
                 completed, but persisting the reduced definitions failed: {e}. \
                 On restart, recovery will finish the drop idempotently. Call \
                 TableManager::verify() to confirm state."
            );
            log::error!("{}", detail);
            crate::backend::contextualize_db_error(e, detail)
        })?;

        // #1067: Write terminal Succeeded status BEFORE clearing the
        // tombstone. This is the exact same crash-safety write-order fix
        // #1069 applied to the base_index `IndexManager::drop_index`/
        // `drop_unique_index`: if we crash after this write but before the
        // tombstone clear, the status is durable and recovery will find it
        // (the tombstone clear is idempotent, so recovery re-clearing is
        // safe).
        if let (Some(op_id_str), Some(index_name_str)) = (op_id.as_deref(), index_name) {
            let op_id_parsed =
                <RecordId as std::str::FromStr>::from_str(op_id_str).map_err(|e| {
                    shamir_storage::error::DbError::Codec(format!("Invalid op_id: {e}"))
                })?;
            let status = shamir_query_types::read::DdlOpStatus {
                op_id: op_id_parsed,
                kind: shamir_query_types::read::DdlOpKind::DropSortedIndex {
                    index_name: index_name_str.to_string(),
                },
                state: shamir_query_types::read::DdlOpState::Succeeded {
                    completed_at: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_millis() as u64,
                },
            };
            // Do not swallow status-write errors — log loudly.
            if let Err(e) =
                crate::base_index::ddl_op_log::write_op_status(&self.info_store, &status).await
            {
                log::error!(
                    "#1067: DROP INDEX (sorted) '{}' succeeded, but failed to write \
                     Succeeded status: {}. The drop completed successfully, but polling by \
                     op_id {:?} will return Unknown. Call TableManager::verify() to confirm \
                     the index was dropped.",
                    index_name_str,
                    e,
                    op_id_parsed
                );
            }
        }

        // P0-3b (#972): clear the tombstone AFTER the reduced defs are
        // durably persisted. `clear_from_dropping_sorted` persists first,
        // then updates in-memory — see that method's doc for the ordering
        // rationale.
        // P1-2 (#967): if this fails, the tombstone remains — recovery
        // will just clear it (a no-op on the already-finished drop).
        // NOTE: Cannot write DdlOpState::Failed here because this layer
        // (SortedIndexManager) does not have op_id in scope.
        self.clear_from_dropping_sorted(name_interned)
            .await
            .map_err(|e| {
                let detail = format!(
                    "DROP SORTED INDEX '{name_interned}': the drop is essentially \
                 complete (tombstone persisted, definition retired, sweep done, \
                 reduced definitions persisted), but clearing the drop tombstone \
                 failed: {e}. On restart, recovery will clear the tombstone as \
                 a no-op. Call TableManager::verify() to confirm state."
                );
                log::error!("{}", detail);
                crate::backend::contextualize_db_error(e, detail)
            })?;

        Ok(true)
    }

    /// Re-key an in-memory sorted-index definition from `old_id` to `new_id`
    /// and persist the updated metadata.
    ///
    /// This is the metadata half of RENAME INDEX for sorted indexes — the
    /// physical posting entries are re-keyed by [`rekey_postings`](Self::rekey_postings)
    /// (driven by [`rename_index_sorted`](Self::rename_index_sorted)). Here we
    /// only swap the in-memory entry and re-save the definitions blob.
    ///
    /// Note: `drop_index` would delete the physical entries we just moved, so
    /// we bypass it and manipulate the `indexes` snapshot directly via `rcu`.
    ///
    /// F-71 (#898): fixes vector 3 of the F-67 regression. The definition's
    /// `ready_at_version` travels for free (the `rcu` below mutates
    /// `name_interned` IN PLACE on the same struct, so its `ready_at_version`
    /// field is untouched and is persisted under the new key by the
    /// `persist_defs` call below). But the in-memory
    /// [`last_mutation_version`](Self::last_mutation_version) high-water is a
    /// SEPARATE map keyed by `name_interned` — without an explicit carry, the
    /// rename would silently leave the OLD key's entry behind (never read
    /// again under `new_id`) and `new_id` would read as epoch `0`, resetting
    /// the AsOf gate to wide-open for the renamed index for the remainder of
    /// this process's lifetime (a restart would then re-seed correctly from
    /// the persisted `ready_at_version`, but that's not good enough — the
    /// gate must not go wrong even without a restart in between). We `remove`
    /// the old entry and re-insert its value under `new_id` (last-write-wins
    /// `max` against any value already sitting under `new_id`, though that
    /// should be impossible since the caller already checked no index is
    /// registered under `new_id`).
    pub async fn rename_definition(&self, old_id: u64, new_id: u64) -> DbResult<()> {
        let mut not_found = false;
        self.indexes.rcu(|cur| {
            let mut new_vec: Vec<SortedIndexDefinition> = (*cur).clone();
            match new_vec.iter().position(|d| d.name_interned == old_id) {
                Some(pos) => {
                    new_vec[pos].name_interned = new_id;
                    // R0-B (#1008): bump the in-memory instance epoch on
                    // RENAME — the SAME semantic Part 1's `generation` bump
                    // establishes at manager granularity, applied here at
                    // per-definition granularity. This is what lets
                    // `pre_commit.rs`'s reconcile distinguish "a staged op
                    // planned against this definition BEFORE the rename"
                    // (stale — the op still carries the OLD name_interned
                    // AND the OLD epoch) from "a freshly re-derived op
                    // planned against the renamed definition" (carries the
                    // NEW name_interned and the NEW epoch). See
                    // `SortedIndexDefinition::instance_epoch`'s doc.
                    new_vec[pos].instance_epoch =
                        crate::base_index::sorted_index_definition::next_instance_epoch();
                    not_found = false;
                }
                None => {
                    not_found = true;
                }
            }
            new_vec
        });
        if not_found {
            return Err(shamir_storage::error::DbError::Internal(
                "sorted index definition disappeared mid-rename".to_string(),
            ));
        }
        // F-50 Step 2 (#870, Part D) / #1007: a RENAME changes the queryable
        // def set's identity (a tx staged against the OLD name must be
        // treated as stale after this point — see the class doc above), so
        // it must advance `generation` exactly like `register`/`drop_index`
        // already do. Before this fix, `rename_definition` performed the RCU
        // swap + epoch-carry + persist but never bumped `generation`, so
        // `pre_commit.rs`'s sorted rederive gate (`sorted_mgr.generation() ==
        // stage_gen`) never fired for a tx that staged before a rename —
        // the exact NP-2 gap #1007 closes. Placed AFTER the RCU swap
        // succeeds, mirroring `register`/`drop_index`'s existing call-site
        // placement (immediately after their own mutating RCU/removal).
        self.generation.fetch_add(1, Ordering::AcqRel);
        // Carry the in-memory mutation-epoch entry from old_id to new_id —
        // see the doc above. `remove_sync` returns the removed AtomicU64
        // (if any); an index that was never mutated has no entry, which is
        // fine — `new_id` correctly starts at the freshly-carried
        // `ready_at_version` floor `load()` would seed on the next restart,
        // and at `0` in-memory until the next mutation or `mark_ready_at`
        // call, matching a never-mutated index's normal semantics.
        if let Some((_, old_epoch)) = self.last_mutation_version.remove_sync(&old_id) {
            self.note_mutation_at_version(new_id, old_epoch.load(Ordering::Acquire));
        }
        self.persist_defs().await
    }

    // ========================================================================
    // P0-5b (#962) — RENAME INDEX durable tombstone + crash recovery (sorted)
    // ========================================================================
    //
    // Mirrors #972's DROP-INDEX durable-tombstone pattern, adapted for rename.
    //
    // The REAL gap this closes (re-read against CURRENT source): the per-batch
    // atomicity of the physical rekey is ALREADY handled — `rekey_postings`
    // (below) is a settle re-scan loop that applies each batch as one
    // `Store::transact` call, the accepted non-atomic-backend tolerance pattern
    // (F-85/#913; a partial-batch visibility is fine because the NEXT loop
    // iteration picks up whatever wasn't yet moved). What was NOT handled is
    // RESUMABILITY: the engine's `rename_index` previously swapped the
    // definition `old_id → new_id` FIRST (`rename_definition`) and THEN ran the
    // rekey. If the rekey returned `Err` (a genuine backend failure), the error
    // propagated to the client while the catalog already believed the rename
    // had succeeded — a client RETRY of the same command then failed
    // immediately (the source name no longer resolved), leaving postings
    // permanently orphaned under the `SORTED_TAG || old_id` prefix (silent
    // storage leak + ghost-posting collision if `old_id` is ever reused). The
    // durable "Renaming" tombstone (written BEFORE `rename_definition`, cleared
    // AFTER the rekey completes) makes the interrupted rekey RESUMABLE on
    // restart: `recover_in_progress_renames` re-runs the (idempotent) settle
    // loop for each recorded `(old_id, new_id)` pair.
    //
    // Tombstone shape: `Vec<(u64, u64)>` (the `old_id → new_id` pairs),
    // serialized via bincode under `system:sidx_ren`. A separate key was chosen
    // (mirroring #972's `system:sidx_drop`) to avoid touching `persist_defs`/
    // `load`'s three-tier bincode forward-compat fallback chain. An absent key
    // or empty vec means "no in-progress renames".
    //
    // **CRITICAL** (the hard-won #959 collision lesson): `RecordId::system(name)`
    // truncates `name` to 12 bytes (see `shamir_types::types::record_id::system`).
    // `"sidx_ren"` (8 bytes) was verified collision-free against every other
    // sorted system key: it differs from `"sorted_indexes"` (truncates to
    // `"sorted_index"`) at byte index 1 (`i` vs `o`), and from `"sidx_drop"` at
    // byte index 5 (`r` vs `d`) — NO collision.

    /// P0-5b (#962): persist the current in-memory `renaming_sorted` map to
    /// `info_store` under `system:sidx_ren`. Serializes the map as a
    /// `Vec<(u64, u64)>` (deterministic order via `BTreeMap`). An empty map
    /// writes an empty `Vec` (NOT a key deletion) so the load path handles
    /// both `NotFound` and empty-vec uniformly. Mirrors #972's
    /// `save_dropping_sorted`.
    pub(super) async fn save_renaming_sorted(&self) -> DbResult<()> {
        let snapshot: Vec<(u64, u64)> = {
            let map = self.renaming_sorted.lock().unwrap();
            map.iter().map(|(&o, &n)| (o, n)).collect()
        };
        let key = RecordId::system("sidx_ren").to_bytes();
        let bytes = bincode::serialize(&snapshot)
            .map_err(|e| shamir_storage::error::DbError::Codec(e.to_string()))?;
        self.info_store.set(key.into(), Bytes::from(bytes)).await?;
        Ok(())
    }

    /// P0-5b (#962): load a persisted renaming map from `info_store`. Returns
    /// an empty `Vec` if the key is absent (`NotFound`) or contains an empty
    /// vec — both mean "no in-progress renames". Mirrors #972's
    /// `load_dropping_sorted`.
    pub(super) async fn load_renaming_sorted(&self) -> DbResult<Vec<(u64, u64)>> {
        let key = RecordId::system("sidx_ren").to_bytes();
        match self.info_store.get(key.into()).await {
            Ok(bytes) => {
                if bytes.is_empty() {
                    return Ok(Vec::new());
                }
                bincode::deserialize::<Vec<(u64, u64)>>(&bytes).map_err(|e| {
                    shamir_storage::error::DbError::Codec(format!(
                        "system:sidx_ren decode failed: {e}"
                    ))
                })
            }
            Err(shamir_storage::error::DbError::NotFound(_)) => Ok(Vec::new()),
            Err(e) => Err(e),
        }
    }

    /// P0-5b (#962): record `(old_id → new_id)` in the in-memory renaming map,
    /// then persist the updated map durably. MUST be called BEFORE
    /// `rename_definition` so a crash at any subsequent point is recoverable.
    ///
    /// In-memory update happens FIRST (so a concurrent observer immediately
    /// sees the pair as in-flight), then the persist follows. If the persist
    /// fails, the in-memory update is rolled back and the caller receives
    /// `Err` — the rename MUST NOT proceed without a durable tombstone.
    /// Mirrors #972's `add_to_dropping_sorted`.
    pub(super) async fn add_to_renaming_sorted(&self, old_id: u64, new_id: u64) -> DbResult<()> {
        {
            let mut map = self.renaming_sorted.lock().unwrap();
            map.insert(old_id, new_id);
        }
        let result = self.save_renaming_sorted().await;
        if result.is_err() {
            // Roll back the in-memory map so the state stays consistent.
            let mut map = self.renaming_sorted.lock().unwrap();
            map.remove(&old_id);
        }
        result
    }

    /// P0-5b (#962): clear `old_id` from the persisted tombstone, then from
    /// the in-memory map. Persist-first ordering ensures the on-disk state is
    /// always at least as advanced as the in-memory state: a crash between
    /// persist and in-memory update leaves a stale in-memory entry that dies
    /// with the process, while the on-disk tombstone is already correct.
    ///
    /// MUST be called AFTER the rekey completes (the moved postings must be
    /// durable under `new_id` first). If the process crashes between the rekey
    /// and this clear, recovery sees the tombstone and re-runs the (idempotent,
    /// no-op-if-finished) rekey, then clears the tombstone. Mirrors #972's
    /// `clear_from_dropping_sorted`.
    pub(super) async fn clear_from_renaming_sorted(&self, old_id: u64) -> DbResult<()> {
        // Compute the snapshot without the entry (do NOT modify in-memory yet).
        let snapshot: Vec<(u64, u64)> = {
            let map = self.renaming_sorted.lock().unwrap();
            map.iter()
                .filter(|(&o, _)| o != old_id)
                .map(|(&o, &n)| (o, n))
                .collect()
        };
        // Persist first.
        let key = RecordId::system("sidx_ren").to_bytes();
        let bytes = bincode::serialize(&snapshot)
            .map_err(|e| shamir_storage::error::DbError::Codec(e.to_string()))?;
        self.info_store.set(key.into(), Bytes::from(bytes)).await?;
        // Only now update in-memory.
        {
            let mut map = self.renaming_sorted.lock().unwrap();
            map.remove(&old_id);
        }
        Ok(())
    }

    /// P0-5b (#962): rekey all posting entries of a sorted index from
    /// `old_id` to `new_id`. MOVED here from the engine crate's free function
    /// `rekey_sorted_prefix` so the open-time recovery path can re-run it
    /// idempotently (it now lives next to its `sweep_sorted_postings` sibling).
    ///
    /// Sorted-index physical key layout:
    ///   `[SORTED_TAG: 1][name_interned BE: 8][encoded_value: var][record_id: 16]`
    /// For each key found under the old prefix, bytes [1..9] are replaced with
    /// `new_id.to_be_bytes()` (big-endian).
    ///
    /// # Concurrency / atomicity
    ///
    /// Settle re-scan loop: after each pass applies one `Store::transact` batch
    /// (one mixed-op atomic unit — a crash/failure between a `set_many` +
    /// `remove_many` pair was the pre-#616 bug), we re-scan the old-id prefix
    /// and loop until a pass finds nothing to move. This is the accepted,
    /// F-85/#913-documented way production callers tolerate
    /// `supports_atomic_transact() == false`: a non-atomic backend's
    /// partial-batch visibility is fine because the NEXT iteration picks up
    /// whatever wasn't yet moved. A concurrent writer that lands an entry
    /// under `old_id` mid-sweep (only possible in the brief window before the
    /// `rename_definition` RCU swap publishes `new_id` to the write-hook) is
    /// caught by the next settle pass. Idempotent: calling it again after the
    /// rekey finished just finds nothing to move and returns `Ok(())`.
    pub(super) async fn rekey_postings(&self, old_id: u64, new_id: u64) -> DbResult<()> {
        let old_prefix = self.entry_prefix(old_id);
        let new_id_bytes = new_id.to_be_bytes();

        // Loop until a full scan of the old-id prefix finds no entries to move.
        loop {
            let stream = self
                .info_store
                .scan_prefix_stream(old_prefix.clone(), FULL_SCAN_BATCH);
            futures::pin_mut!(stream);

            // One mixed-op batch per pass — `Store::transact` commits the whole
            // batch as one atomic unit (the pre-#616 two-call `set_many` +
            // `remove_many` shape was NOT atomic).
            let mut ops: Vec<KvOp> = Vec::new();
            let mut moved_any = false;

            while let Some(batch) = stream.next().await {
                for (key, value) in batch? {
                    if key.len() < 9 {
                        continue; // malformed; skip
                    }
                    let mut new_key = key.to_vec();
                    new_key[1..9].copy_from_slice(&new_id_bytes);
                    ops.push(KvOp::Set(Bytes::from(new_key).into(), value));
                    ops.push(KvOp::Remove(key));
                    moved_any = true;
                }
            }

            if !moved_any {
                // No old-id entries found this pass — settle complete.
                break;
            }

            self.info_store.transact(ops).await?;
        }

        Ok(())
    }

    /// P0-5b (#962): open-time recovery for RENAME INDEX operations
    /// interrupted by a crash. Called from `SortedIndexManager::new` AFTER
    /// definitions are loaded and AFTER `recover_in_progress_drops` runs (a
    /// drop and a rename for the same name cannot both be in flight — the
    /// engine's write barrier serializes DDL).
    ///
    /// # Crash-state matrix
    ///
    /// | crash point                                  | def under? | tombstone? | recovery action                         |
    /// |----------------------------------------------|------------|------------|-----------------------------------------|
    /// | after tombstone, before rename_definition    | old_id     | yes        | rename def, rekey (moves postings)      |
    /// | after rename_definition, before/during rekey | new_id     | yes        | rekey (finishes moving remainder)       |
    /// | after rekey, before tombstone clear          | new_id     | yes        | rekey (no-op), clear tombstone          |
    ///
    /// In every case recovery leaves the manager consistent: the definition is
    /// under `new_id`, ALL postings are under `new_id`, and the tombstone is
    /// cleared. The rekey is idempotent, so calling recovery twice (two
    /// restart attempts) is a clean no-op on the second call.
    pub(super) async fn recover_in_progress_renames(&self) -> DbResult<()> {
        let renaming = self.load_renaming_sorted().await?;
        if renaming.is_empty() {
            return Ok(());
        }

        log::info!(
            "P0-5b (#962): recovering {} in-progress sorted RENAME(s)",
            renaming.len()
        );

        for &(old_id, new_id) in &renaming {
            // If the definition is still under old_id, the crash happened
            // before rename_definition's persist finalized — finish the def
            // rename now. If it is already under new_id (or absent under
            // old_id), `find_by_name_interned(old_id).is_none()` short-
            // circuits and we skip straight to the (idempotent) rekey.
            if self.find_by_name_interned(old_id).is_some() {
                // rename_definition carries the in-memory mutation epoch and
                // persists; if the def raced away between the check and the
                // call (only under a violated write-barrier contract), treat
                // its Err as non-fatal and let the rekey run regardless.
                let _ = self.rename_definition(old_id, new_id).await;
            }
            // Always run the rekey (idempotent). Covers both the "rekey never
            // ran" and "rekey ran partially" cases.
            self.rekey_postings(old_id, new_id).await?;
        }

        // Clear the tombstone (write empty Vec; the load path treats empty-vec
        // and NotFound identically — cheaper than a remove).
        let empty = bincode::serialize(&Vec::<(u64, u64)>::new())
            .map_err(|e| shamir_storage::error::DbError::Codec(e.to_string()))?;
        let key = RecordId::system("sidx_ren").to_bytes();
        self.info_store.set(key.into(), Bytes::from(empty)).await?;

        log::info!(
            "P0-5b (#962): recovery complete — {} sorted RENAME(s) finalized",
            renaming.len()
        );

        Ok(())
    }

    /// P0-5b (#962): the FULL sorted-RENAME orchestration — definition swap +
    /// physical posting rekey, wrapped in a durable "Renaming" tombstone so an
    /// interrupted rekey is resumable on restart. The engine's `rename_index`
    /// delegates its sorted branch to this method (mirroring how
    /// `drop_sorted_index` delegates to `drop_index`).
    ///
    /// Sequence:
    /// 1. Write a durable "Renaming" tombstone `(old_id → new_id)` — MUST
    ///    succeed before anything mutates, so a crash at any later point is
    ///    recoverable by `recover_in_progress_renames`.
    /// 2. `rename_definition(old_id, new_id)` — swap the in-memory definition
    ///    (RCU) + carry the mutations epoch + persist.
    /// 3. `rekey_postings(old_id, new_id)` — settle-loop move of the physical
    ///    postings to the new prefix.
    /// 4. Clear the tombstone (persist-first).
    ///
    /// # Error / resume semantics
    ///
    /// If step 2 or 3 returns `Err`, the tombstone is LEFT IN PLACE and the
    /// error propagates to the caller. The catalog may already believe the
    /// rename succeeded (step 2 swapped + persisted), so a client RETRY of the
    /// same command is expected to fail — but a restart runs
    /// `recover_in_progress_renames`, which finishes the rekey (the whole
    /// point of the tombstone). This matches #972's model: a crashed DDL op is
    /// finished by restart-recovery, not by re-issuing the command.
    ///
    /// #1067: accepts `op_id`/`old_name`/`new_name` (resolved by the caller,
    /// which already has the string names in scope — this layer only ever
    /// dealt in interned ids before) so the terminal `Succeeded` status can
    /// be written AFTER step 3 (`rekey_postings`) but BEFORE step 4 (the
    /// tombstone clear) — the same before-the-clear write-order discipline
    /// as `drop_index` above and the base_index `IndexManager` family.
    pub async fn rename_index_sorted(
        &self,
        old_id: u64,
        new_id: u64,
        op_id: Option<String>,
        old_name: Option<&str>,
        new_name: Option<&str>,
    ) -> DbResult<()> {
        // 1. Durable tombstone BEFORE the definition swap.
        self.add_to_renaming_sorted(old_id, new_id).await?;

        // 2. Swap + persist the definition (carries the mutations epoch too).
        // If this fails AFTER the tombstone was written, the tombstone is left
        // in place — recovery will reconcile on restart (it checks which id the
        // definition is actually under before deciding to rename vs. just rekey).
        // P1-2 (#967): enrich the error with partial-state context.
        // NOTE: Cannot write DdlOpState::Failed here because this layer
        // (SortedIndexManager) does not have op_id in scope.
        self.rename_definition(old_id, new_id).await.map_err(|e| {
            let detail = format!(
                "RENAME SORTED INDEX ({old_id} → {new_id}): a durable rename \
                 tombstone was persisted, but the definition swap + persist \
                 failed: {e}. The tombstone is left in place — on restart, \
                 recovery will reconcile the rename. Call TableManager::verify() \
                 to confirm state."
            );
            log::error!("{}", detail);
            crate::backend::contextualize_db_error(e, detail)
        })?;

        // P0-5b (#962) test seam — park here (definition already swapped +
        // persisted under new_id, postings NOT yet rekeyed) if a test
        // installed the pause hook. This is the EXACT crash window the bug
        // describes. NOT `#[cfg(test)]`-gated — cross-crate test consumer.
        if let Some(hook) = self.rename_rekey_pause_hook.load_full() {
            hook.wait_at_window().await;
        }

        // 3. Rekey the physical postings (settle loop).
        // P1-2 (#967): enrich — the definition swap already succeeded, so
        // the rename is partially done (definition under new_id, postings
        // still under old_id).
        // NOTE: Cannot write DdlOpState::Failed here because this layer
        // (SortedIndexManager) does not have op_id in scope.
        self.rekey_postings(old_id, new_id).await.map_err(|e| {
            let detail = format!(
                "RENAME SORTED INDEX ({old_id} → {new_id}): the definition was \
                 swapped and persisted under {new_id}, but rekeying the physical \
                 postings from {old_id} failed: {e}. The tombstone is left in \
                 place — on restart, recovery will finish the rekey. Call \
                 TableManager::verify() to confirm state."
            );
            log::error!("{}", detail);
            crate::backend::contextualize_db_error(e, detail)
        })?;

        // #1067: Write terminal Succeeded status AFTER the rekey but BEFORE
        // clearing the tombstone — same before-the-clear write-order
        // discipline as `drop_index` above and the base_index `IndexManager`
        // family's #1069 fix. `op_id`/`old_name`/`new_name` are now
        // parameters (previously this layer had no op_id in scope at all).
        if let (Some(op_id_str), Some(old_name_str), Some(new_name_str)) =
            (op_id.as_deref(), old_name, new_name)
        {
            let op_id_parsed =
                <RecordId as std::str::FromStr>::from_str(op_id_str).map_err(|e| {
                    shamir_storage::error::DbError::Codec(format!("Invalid op_id: {e}"))
                })?;
            let status = shamir_query_types::read::DdlOpStatus {
                op_id: op_id_parsed,
                kind: shamir_query_types::read::DdlOpKind::RenameSortedIndex {
                    old_name: old_name_str.to_string(),
                    new_name: new_name_str.to_string(),
                },
                state: shamir_query_types::read::DdlOpState::Succeeded {
                    completed_at: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_millis() as u64,
                },
            };
            // Do not swallow status-write errors — log loudly.
            if let Err(e) =
                crate::base_index::ddl_op_log::write_op_status(&self.info_store, &status).await
            {
                log::error!(
                    "#1067: RENAME INDEX (sorted) '{}' -> '{}' succeeded, but failed to \
                     write Succeeded status: {}. The rename completed successfully, but \
                     polling by op_id {:?} will return Unknown. Call \
                     TableManager::verify() to confirm the index was renamed.",
                    old_name_str,
                    new_name_str,
                    e,
                    op_id_parsed
                );
            }
        }

        // 4. Clear the tombstone now that the rekey is durable.
        // P1-2 (#967): if this fails, the tombstone remains — recovery
        // will just clear it (the rename is fully done).
        // NOTE: Cannot write DdlOpState::Failed here because this layer
        // (SortedIndexManager) does not have op_id in scope for a FAILURE —
        // by the time op_id/old_name/new_name are available (as of #1067),
        // every fallible step above already returned via `?` before this
        // point, so there is no reachable failure site left in this
        // function to attach a Failed status to.
        self.clear_from_renaming_sorted(old_id).await.map_err(|e| {
            let detail = format!(
                "RENAME SORTED INDEX ({old_id} → {new_id}): the rename is fully \
                 complete (definition swapped, postings rekeyed), but clearing \
                 the rename tombstone failed: {e}. On restart, recovery will \
                 clear the tombstone as a no-op. Call TableManager::verify() \
                 to confirm state."
            );
            log::error!("{}", detail);
            crate::backend::contextualize_db_error(e, detail)
        })?;

        Ok(())
    }
}
