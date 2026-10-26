//! Synchronous overlay→history drain for one-shot admin operations.
//!
//! [`MvccStore::drain_to_history`] forces every committed overlay entry into the
//! durable `history` version-log. Repository-wide durability is advanced only
//! by the repository drainer after all stores for a version are complete.
//! This is the keystone of populated-table RENAME (Phase F.2).

use std::collections::BTreeMap;

use shamir_storage::error::DbResult;
use shamir_storage::types::KvOp;

use super::MvccStore;

impl MvccStore {
    /// Force-drain every overlay entry into the durable `history` version-log.
    ///
    /// Snapshots the overlay up to the current visibility watermark
    /// (`last_committed`), groups entries by commit version, and writes each
    /// version's ops through [`write_committed_to_history`](Self::write_committed_to_history)
    /// — exactly what the background [`Drainer`](shamir_engine::tx::drainer::Drainer)
    /// does, but synchronously and for a single table.
    ///
    /// The repository-wide durable watermark is NOT advanced here: this call
    /// owns only one table and cannot prove completion for a shared version.
    ///
    /// **Idempotent**: calling on an already-drained (empty-overlay) store is a
    /// no-op — `iter_all_le` returns an empty vec, no `history.transact` fires,
    /// and `mark_durable` / `gc_overlay_to` on a version ≤ the current watermark
    /// are themselves no-ops.
    ///
    /// **Used by**: `rename_table_stores` (Phase F.2) to ensure the source
    /// table's `__history__` store is complete before `copy_store` copies it
    /// to the destination name.
    ///
    /// Off hot path: this is a one-shot admin op (table rename / migration),
    /// so the O(N) overlay scan and per-version history writes are acceptable.
    pub async fn drain_to_history(&self) -> DbResult<()> {
        // #1032 diagnostic instrumentation (follow-up to F-68/#895 task #124):
        // the prior instrumentation round only logged the CALLER's
        // before/after around this whole call (`rename_table_stores: calling
        // drain_to_history` / `... returned after`), which is enough to show
        // THAT this function is where a hang lands, but not WHICH step
        // inside it. Adding a log per meaningful step below so the next CI
        // recurrence pinpoints the exact stuck `.await`.
        log::debug!("drain_to_history: enter");
        // #1032: exclude the background Drainer's `write_committed_batch_to_history`
        // from this table for the ENTIRE duration of this force-drain — see
        // `drain_exclusive`'s field doc (`mvcc_store/store.rs`) for why two
        // concurrent history-store writers can hang, not just race on data.
        let _drain_guard = self.drain_exclusive.lock().await;
        log::debug!("drain_to_history: acquired drain_exclusive");
        // Snapshot the visibility watermark ONCE. New writes landing after this
        // point are NOT drained (they belong to the next drain pass / drainer).
        let visibility = self.gate.last_committed();
        log::debug!("drain_to_history: visibility={visibility}");
        if visibility == 0 {
            // No commits ever — overlay is empty by construction.
            return Ok(());
        }

        // Collect every (key, version, value) with version <= visibility.
        // Unlike `snapshot_le` (which collapses to per-key winners), this
        // returns ALL version entries — each distinct version is a distinct
        // row in the timeline and must land in history independently.
        let entries = self.overlay.iter_all_le(visibility);
        log::debug!(
            "drain_to_history: iter_all_le returned {} entries",
            entries.len()
        );
        if entries.is_empty() {
            // Overlay already drained (or was never populated — e.g. non-tx
            // writes that went directly to history). Nothing to do.
            return Ok(());
        }

        // Group by version (ascending). BTreeMap<u64, Vec<KvOp>> gives us
        // deterministic ascending version order — required by the drain
        // contract (versions must be written in commit order so the
        // durable watermark advances contiguously).
        let mut by_version: BTreeMap<u64, Vec<KvOp>> = BTreeMap::new();
        for (key, version, value) in entries {
            // Overlay keys and KvOp are both `RecordKey`-keyed (task #532) —
            // move straight through, no conversion.
            let op = if value.is_empty() {
                KvOp::Remove(key)
            } else {
                KvOp::Set(key, value)
            };
            by_version.entry(version).or_default().push(op);
        }

        // Write each version's ops to history (ascending order — BTreeMap iter).
        // `write_committed_to_history` is the same method the background
        // drainer calls; it does one `history.transact` per version and is
        // idempotent (re-writing an existing version-key is a no-op overwrite).
        log::debug!(
            "drain_to_history: writing {} version(s) to history: {:?}",
            by_version.len(),
            by_version.keys().collect::<Vec<_>>()
        );
        for (commit_version, ops) in &by_version {
            log::debug!(
                "drain_to_history: write_committed_to_history version={commit_version} start"
            );
            self.write_committed_to_history(ops, *commit_version)
                .await?;
            log::debug!(
                "drain_to_history: write_committed_to_history version={commit_version} done"
            );
        }
        log::debug!("drain_to_history: table-local history writes complete");
        // Table-local draining cannot establish repository-wide completion of
        // a shared version. The repository drainer owns that watermark.

        // Reclaim only entries already globally durable. This call must not
        // discard this table's undrained overlay entries.
        self.gc_overlay_to(self.gate.durable_watermark());

        Ok(())
    }
}

impl MvccStore {
    /// F6b (I2): force the durable `history` version-log to disk.
    ///
    /// Narrow durability seam for the truncation fsync-gate: before the
    /// drainer deletes a sealed WAL segment, every record in that segment
    /// must be `fsync`'d into `history` (not merely written to the page
    /// cache), else a power-loss after the unlink but before the page-cache
    /// flush would lose data (I2, power-loss / level-3). `mark_durable` only
    /// means "written to history"; this makes that write physically durable.
    ///
    /// Flushes ONLY this store's `history` — it does NOT drain the WAL
    /// (calling `drain_all` from the drainer would recurse). The drainer
    /// gates this on `wal.has_truncatable(durable)`, so it fires only on a
    /// segment boundary, never per-commit (no `fsync`-on-commit regression).
    pub async fn flush_history(&self) -> DbResult<()> {
        self.history.flush().await
    }
}
