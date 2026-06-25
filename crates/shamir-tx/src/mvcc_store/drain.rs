//! Synchronous overlay→history drain for one-shot admin operations.
//!
//! [`MvccStore::drain_to_history`] forces every committed overlay entry into the
//! durable `history` version-log and advances the durable watermark, so a
//! subsequent `copy_store` of `__history__<table>` captures all live data.
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
    /// After every version is durable, advances the durable watermark via
    /// [`mark_durable`](crate::RepoTxGate::mark_durable) and reclaims the
    /// overlay via [`gc_overlay_to`](Self::gc_overlay_to).
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
        // Snapshot the visibility watermark ONCE. New writes landing after this
        // point are NOT drained (they belong to the next drain pass / drainer).
        let visibility = self.gate.last_committed();
        if visibility == 0 {
            // No commits ever — overlay is empty by construction.
            return Ok(());
        }

        // Collect every (key, version, value) with version <= visibility.
        // Unlike `snapshot_le` (which collapses to per-key winners), this
        // returns ALL version entries — each distinct version is a distinct
        // row in the timeline and must land in history independently.
        let entries = self.overlay.iter_all_le(visibility);
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
        for (commit_version, ops) in &by_version {
            self.write_committed_to_history(ops, *commit_version)
                .await?;
        }

        // Advance the durable watermark to visibility. `mark_durable` is
        // idempotent — if the non-tx path already marked each version durable
        // inline, this is a no-op. For tx-path versions that were only in the
        // overlay, this is the first time they become durable.
        self.gate.mark_durable(visibility);

        // Reclaim overlay memory: drop every entry <= the (now-advanced)
        // durable watermark. `gc_overlay_to` is a lock-free sweep.
        self.gc_overlay_to(visibility);

        Ok(())
    }
}
