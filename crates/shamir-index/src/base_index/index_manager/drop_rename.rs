use bytes::Bytes;
use shamir_storage::error::DbResult;
use shamir_types::types::record_id::RecordId;

use crate::base_index::write_barrier_flags::REGULAR_INDEX_EXISTS;

use super::inner::IndexManager;

impl IndexManager {
    /// Удаляет индекс по его имени.
    ///
    /// Процесс удаления (F-76 / #903 — definition retired BEFORE the posting
    /// sweep, the mirror image of F-72's CREATE gate; P0-3 / #959 — durable
    /// tombstone closes the crash-resurrection gap):
    /// 1. Проверяет существование индекса
    /// 2. **P0-3**: persist a durable tombstone (`system:idx_drop`)
    ///    recording that `name_interned` is being dropped — MUST succeed
    ///    before the sweep, so a crash at any later point is recoverable
    ///
    /// 2.5. **P0-3a (#1011)**: raise the `reader_gate` intent flag
    ///    (`begin_drop`) — new readers back off to `Ok(None)`
    /// 3. Retires the definition from the in-memory Vec (planner-invisible
    ///    via RCU swap)
    ///
    /// 3.5. **P0-3a (#1011)**: drain in-flight readers
    ///    (`wait_for_drain`) BEFORE the sweep
    /// 4. Sweeps all posting entries from `info_store`
    ///
    /// 4.5. **P0-3a (#1011)**: release the drain gate (`drop(drain_guard)`)
    /// 5. Persists the reduced `IndexInfo` (def removed)
    /// 6. **P0-3**: clears the tombstone
    ///
    /// F-76 (#903): the OLD order was sweep → `remove_index`, which left a
    /// window in which a concurrent reader could still select the index via
    /// `iter_indexes_ready` (it was still `Ready` in the Vec) while its
    /// postings were mid-sweep — silently wrong/incomplete results, the
    /// mirror image of the F-72 CREATE bug. The RCU `remove_index` swap
    /// publishes a Vec without this definition atomically; every NEW reader
    /// resolves the NEW Vec and falls back to a full scan. The OLD readers
    /// that already resolved the OLD definition continue to read the OLD
    /// postings — the drop does NOT start sweeping until they finish (step
    /// 3.5).
    /// after this point no longer sees the index and falls back to a full
    /// scan. A reader that already snapshotted the old Vec keeps working
    /// against its own consistent view.
    ///
    /// P0-3 (#959) sub-bug 3c — crash-resurrection: the OLD code had a gap
    /// between the sweep (step 4) and the IndexInfo persist (step 5). If the
    /// process crashed in that gap, the on-disk `IndexInfo` still listed the
    /// index as `Ready`, but all its postings were gone — `IndexManager::new`
    /// would load the stale `Ready` definition and the planner would route
    /// queries to an index with zero postings, silently returning wrong
    /// (empty / missing) results with no error or crash signal. The durable
    /// tombstone (step 2) closes this: on restart, `recover_in_progress_drops`
    /// sees the tombstone, resumes the sweep (idempotent), removes the stale
    /// definition, persists, and clears the tombstone.
    ///
    /// P0-3a (#1011) sub-bug 3a — in-flight reader consistency (CLOSED for
    /// the REGULAR family; still OPEN for the sorted family and index2 —
    /// tracked separately as #1037 / #1038):
    /// A reader that resolved the definition BEFORE `drop_index` retired it
    /// holds an `Arc`-snapshot of the Vec and will route a lookup through
    /// `info_store`'s shared keyspace DURING the sweep. WITHOUT
    /// synchronization such a reader CAN observe a PARTIALLY-swept keyspace —
    /// some posting keys already removed, others not yet — returning an
    /// incomplete (but never wrong/corrupted) result set. `remove_many` only
    /// removes; it never corrupts existing postings.
    ///
    /// REGULAR family — CLOSED by the `reader_gate` drain gate (steps 2.5 /
    /// 3.5 / 4.5 below): `lookup_by_index` acquires a [`reader_gate`](Self::reader_gate)
    /// `ReadGuard` for the duration of its `info_store` scan, and `drop_index`
    /// raises the gate's intent flag (step 2.5) before the RCU retire and
    /// drains every in-flight reader (step 3.5) before the physical sweep — so
    /// a reader either completes against the FULL pre-sweep keyspace or backs
    /// off to `Err(IndexDrainInProgress)` (caller falls back to a full scan).
    /// The flag is cleared after the sweep (step 4.5). See `crate::reader_drain_gate`'s
    /// module doc for the memory-model proof and the placement invariant.
    ///
    /// SORTED family (#1037) — CLOSED by the same `ReaderDrainGate` pattern:
    /// all 8 sorted-index read chokepoints (`lookup_range`, `lookup_min`,
    /// `lookup_max`, `lookup_last_k`, `lookup_range_first_k_page`,
    /// `lookup_first_k`, `lookup_range_with_values`, `entry_count`) acquire
    /// the gate's `ReadGuard`, and `drop_index` raises the intent flag before
    /// the retire, drains before the sweep. The back-off signal is
    /// `DbError::IndexDrainInProgress` (distinguishable from "no matches").
    ///
    /// index2 family (#1038) — CLOSED by the LEASE variant of the same pattern:
    /// `lease_by_field_and_kind` returns a `BackendLease` that bundles the
    /// backend `Arc` with a `ReadGuard`, and the caller holds that lease for
    /// the duration of the read. `drop_index2` raises the intent flag before
    /// the retire, drains before the sweep. This is the correct shape for
    /// index2 because resolve and read are already connected through an
    /// `Arc<dyn IndexBackend>` handle — gating the resolve (which produces the
    /// `Arc`) is sufficient, unlike slices 1/2 which gate each individual scan
    /// method. The back-off signal is `DbError::IndexDrainInProgress`.
    ///
    ///
    /// The engine-side write serialization (`TableManager::drop_index`
    /// wrapping the call in `begin_write_barrier(REGULAR_INDEX_CREATE)`)
    /// serializes DROP against concurrent WRITERS (unchanged); the reader gate
    /// adds the missing reader-vs-DROP serialization for the regular family.
    ///
    /// # Возвращает
    ///
    /// `true` — индекс существовал и был удалён
    /// `false` — индекс не найден
    ///
    /// #1051: accepts `op_id` minted at dispatch time for crash recovery status writes.
    ///
    /// #1069: if `op_id` and `index_name` are both provided, writes terminal
    /// `Succeeded` status BEFORE clearing the tombstone (ensuring crash-safety for
    /// the inline path). The recovery paths write their own status, so this is
    /// only needed for the synchronous success path.
    pub async fn drop_index(
        &self,
        name_interned: u64,
        op_id: Option<String>,
        index_name: Option<&str>,
    ) -> DbResult<bool> {
        // Быстрая проверка существования индекса
        if !self.indexes.contains(name_interned) {
            return Ok(false);
        }

        // P0-3 (#959 / #1051): write a durable tombstone BEFORE retiring the
        // definition or sweeping postings. If the process crashes after
        // the sweep but before the reduced IndexInfo is persisted, the
        // on-disk metadata still lists the index as `Ready` — but the
        // tombstone tells `recover_in_progress_drops` to finish the drop
        // rather than resurrecting a broken "Ready but no postings" index.
        // MUST succeed before proceeding; `add_to_dropping` rolls back the
        // in-memory map on persist failure.
        // Clone op_id so we still have it for status write later.
        let op_id_for_status = op_id.clone();
        self.add_to_dropping(false, name_interned, op_id).await?;

        // P0-3a (#1011) step 2.5: raise the reader-drain gate's intent flag
        // (SeqCst) BEFORE the RCU retire below. From this point every NEW
        // `lookup_by_index` reader observes the flag and backs off (`Ok(None)`
        // → caller falls back to a full scan). The flag stays up until
        // `drain_guard` is dropped (step 4.5 + RAII safety net on early `?`).
        // See `crate::reader_drain_gate`'s module doc for the memory-model
        // proof and the deadlock-exclusion placement invariant.
        let drain_guard = self.reader_gate.begin_drop();

        // F-76 (#903): retire the definition from the planner-visible Vec
        // FIRST (see the method doc). The RCU swap publishes a Vec without
        // this definition atomically.
        let was_removed = self.indexes.remove_index(name_interned);
        // #1102: update REGULAR_INDEX_EXISTS to match the new IndexInfo state
        // (may be true if other indexes remain, false if this was the last one).
        self.write_barrier_flags
            .set_to(REGULAR_INDEX_EXISTS, self.indexes.is_enabled());
        self.bump_generation(); // P0-2 (#958): gen gate for commit-time rederive

        // F-76 test seam: park here (definition already retired, postings not
        // yet swept) if a test installed a pause hook. With the fix, a
        // concurrent read issued while parked here must fall back to a full
        // scan. NOT `#[cfg(test)]`-gated — see `drop_index_pause_hook`'s
        // field doc (cross-crate test consumer).
        if let Some(hook) = self.drop_index_pause_hook.load_full() {
            hook.wait_at_window().await;
        }

        // P0-3a (#1011) step 3.5: drain every reader that entered
        // `lookup_by_index` BEFORE the flag went up (the gate's memory model
        // guarantees such a reader is counted in `in_flight`, so this wait
        // terminates — see the livelock-freedom argument in the gate's module
        // doc). AFTER the RCU retire and the pause-hook park, BEFORE the sweep,
        // so the physical `remove_many` cannot start until no reader is
        // mid-scan of the shared keyspace. Bounded by the gate's escalating
        // log; this wait sits inside the already-held
        // `ddl_admission`+`unique_write_lock` critical section exactly as the
        // mirror-image `WriterDrainBarrier::drain` does.
        drain_guard.wait_for_drain().await;

        // Sweep the (now orphan, planner-invisible) posting entries.
        // P0-3 (#959): extracted to `sweep_index_postings` so the recovery
        // path can re-run it idempotently.
        // P1-2 (#967): a durable tombstone is already persisted — if this
        // sweep fails, enrich the error with the partial-state context.
        // NOTE: Cannot write DdlOpState::Failed here even though `op_id` is
        // in scope (a parameter of this function, #1051) — this crate
        // (`shamir-index`) has no access to `shamir-engine`'s `ddl_op_log`,
        // which sits one layer up. The enriched error message is the only
        // signal to the caller (`TableManager::rename_index` already writes
        // Failed status for rename-specific failures).
        self.sweep_index_postings(false, name_interned)
            .await
            .map_err(|e| {
                let detail = format!(
                    "DROP INDEX '{name_interned}': a durable drop tombstone was \
                 persisted and the definition was retired from the planner, but \
                 the posting sweep failed: {e}. On restart, recovery will resume \
                 the sweep idempotently and finish the drop. Call \
                 TableManager::verify() to confirm state."
                );
                log::error!("{}", detail);
                crate::backend::contextualize_db_error(e, detail)
            })?;

        // P0-3a (#1011) step 4.5: the sweep finished — explicitly release the
        // drain gate now so the (unrelated) persist / tombstone-clear steps
        // below run with the gate already clear. RAII (`drain_guard`'s `Drop`
        // clears the flag) remains the safety net for every early `?` return
        // above, but an explicit early drop here mirrors `doctor::repair()`'s
        // own early-barrier-release style and keeps the drain window as tight
        // as possible (the flag is up from step 2.5 until HERE — the minimum
        // span that still covers retire + drain + sweep).
        drop(drain_guard);

        // P0-3 (#959) test seam — park here (sweep complete, reduced
        // IndexInfo NOT yet persisted) if a test installed the post-sweep
        // hook. This is the exact crash window sub-bug 3c exercises: a
        // "crash" here (dropping the manager) leaves the tombstone on disk
        // but the old IndexInfo, and the recovery path in `new()` must
        // finish the drop. NOT `#[cfg(test)]`-gated — cross-crate test
        // consumer, same reason as `drop_index_pause_hook`.
        if let Some(hook) = self.drop_index_post_sweep_hook.load_full() {
            hook.wait_at_window().await;
        }

        // Persist the reduced IndexInfo (definition removed).
        // P1-2 (#967): the tombstone is still in place — if this persist
        // fails, recovery will see the tombstone and finish the drop.
        // NOTE: Cannot write DdlOpState::Failed here even though `op_id` is
        // in scope (a parameter of this function, #1051) — this crate
        // (`shamir-index`) has no access to `shamir-engine`'s `ddl_op_log`,
        // which sits one layer up.
        if was_removed {
            self.save_index_info().await.map_err(|e| {
                let detail = format!(
                    "DROP INDEX '{name_interned}': a durable drop tombstone was \
                     persisted, the definition was retired, and the posting sweep \
                     completed, but persisting the reduced index metadata failed: \
                     {e}. On restart, recovery will finish the drop idempotently. \
                     Call TableManager::verify() to confirm state."
                );
                log::error!("{}", detail);
                crate::backend::contextualize_db_error(e, detail)
            })?;
        }

        // #1069: Write terminal Succeeded status BEFORE clearing tombstone.
        // This ensures crash-safety for the inline path: if we crash after this
        // write but before tombstone clear, the status is durable and recovery
        // will find it (tombstone clear is idempotent, so recovery re-clearing
        // is safe).
        if was_removed {
            if let (Some(op_id_str), Some(index_name_str)) =
                (op_id_for_status.as_deref(), index_name)
            {
                let op_id_parsed =
                    <RecordId as std::str::FromStr>::from_str(op_id_str).map_err(|e| {
                        shamir_storage::error::DbError::Codec(format!("Invalid op_id: {e}"))
                    })?;
                let status = shamir_query_types::read::DdlOpStatus {
                    op_id: op_id_parsed,
                    kind: shamir_query_types::read::DdlOpKind::DropHashIndex {
                        index_name: index_name_str.to_string(),
                    },
                    state: shamir_query_types::read::DdlOpState::Succeeded {
                        completed_at: crate::base_index::ddl_op_log::timestamp_millis(
                            std::time::SystemTime::now(),
                        ),
                    },
                };
                // Do not swallow status-write errors — log loudly.
                if let Err(e) =
                    crate::base_index::ddl_op_log::write_op_status(&self.info_store, &status).await
                {
                    log::error!(
                        "#1069: DROP INDEX (regular) '{}' succeeded, but failed to write \
                         Succeeded status: {}. The drop completed successfully, but polling by \
                         op_id {:?} will return Unknown. Call TableManager::verify() to confirm \
                         the index was dropped.",
                        index_name_str,
                        e,
                        op_id_parsed
                    );
                }
            }
        }

        // #1069 test seam — park here (terminal Succeeded status already
        // durably written, tombstone NOT yet cleared) if a test installed
        // the status pause hook. This is the exact crash window the
        // write-order fix closes; see `drop_index_status_pause_hook`'s
        // field doc. NOT `#[cfg(test)]`-gated — cross-crate test consumer,
        // same reason as `drop_index_pause_hook`. `None` on every real path
        // — one uncontended `ArcSwapOption::load_full()` Acquire load.
        if let Some(hook) = self.drop_index_status_pause_hook.load_full() {
            hook.wait_at_window().await;
        }

        // P0-3 (#959): clear the tombstone AFTER the reduced IndexInfo is
        // durably persisted. `clear_from_dropping` persists first, then
        // updates in-memory — if the process crashes between persist and
        // in-memory update, the on-disk tombstone is already cleared and
        // the stale in-memory entry dies with the process. If the process
        // crashes BEFORE this call (between IndexInfo persist and tombstone
        // clear), recovery sees the tombstone but the def is already gone
        // from IndexInfo — it just clears the tombstone (a no-op sweep).
        // P1-2 (#967): if this fails, the tombstone remains — recovery
        // will just clear it (a no-op on the already-finished drop).
        self.clear_from_dropping(false, name_interned)
            .await
            .map_err(|e| {
                let detail = format!(
                    "DROP INDEX '{name_interned}': the drop is essentially complete \
                 (tombstone persisted, definition retired, sweep done, reduced \
                 metadata persisted), but clearing the drop tombstone failed: {e}. \
                 On restart, recovery will clear the tombstone as a no-op. Call \
                 TableManager::verify() to confirm state."
                );
                log::error!("{}", detail);
                crate::backend::contextualize_db_error(e, detail)
            })?;

        Ok(was_removed)
    }

    /// Сохраняет метаданные индексов в служебное хранилище.
    ///
    /// Сериализует IndexInfo через bincode и сохраняет под системным ключом.
    /// Сериализует напрямую без клонирования — IndexInfo::serialize конвертирует
    /// DashMap в BTreeMap внутри себя.
    pub(in crate::base_index) async fn save_index_info(&self) -> DbResult<()> {
        let indexes_key = RecordId::system("indexes").to_bytes();
        let bytes = bincode::serialize(&*self.indexes)
            .map_err(|e| shamir_storage::error::DbError::Codec(e.to_string()))?;
        self.info_store
            .set(indexes_key.into(), Bytes::from(bytes))
            .await?;
        Ok(())
    }
}
