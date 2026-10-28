use std::sync::Arc;

use super::inner::IndexManager;

impl IndexManager {
    /// F-72 (#899, P0) test-only: install (or clear with `None`) the
    /// deterministic `create_index_from_records` backfill pause hook. Not
    /// `#[cfg(test)]`-gated — see the field's doc for why (cross-crate test
    /// consumer).
    pub fn set_create_index_backfill_hook(
        &self,
        hook: Option<Arc<crate::base_index::backfill_pause_hook::BackfillPauseHook>>,
    ) {
        self.create_index_backfill_hook.store(hook);
    }

    /// F-76 (#903) test-only: install (or clear with `None`) the deterministic
    /// `drop_index` / `drop_unique_index` pause hook (fires between the
    /// definition retirement and the posting sweep). Not `#[cfg(test)]`-gated
    /// — cross-crate test consumer, same reason as
    /// `set_create_index_backfill_hook`. See `drop_index_pause_hook`'s field
    /// doc.
    pub fn set_drop_index_pause_hook(
        &self,
        hook: Option<Arc<crate::base_index::backfill_pause_hook::BackfillPauseHook>>,
    ) {
        self.drop_index_pause_hook.store(hook);
    }

    /// P0-3a (#1011) test-only: install (or clear with `None`) the
    /// deterministic `lookup_by_index` pause hook (fires inside the read
    /// chokepoint, after the reader-gate guard is acquired and before the
    /// `info_store` scan). NOT `#[cfg(test)]`-gated — cross-crate test
    /// consumer, same reason as `set_drop_index_pause_hook`. See
    /// `lookup_pause_hook`'s field doc.
    pub fn set_lookup_pause_hook(
        &self,
        hook: Option<Arc<crate::base_index::backfill_pause_hook::BackfillPauseHook>>,
    ) {
        self.lookup_pause_hook.store(hook);
    }

    /// D3ED367D test-only: pause after lookup scan completion and before cache publication.
    /// Not `#[cfg(test)]`-gated for cross-crate test consumers.
    pub fn set_lookup_post_scan_pause_hook(
        &self,
        hook: Option<Arc<crate::base_index::backfill_pause_hook::BackfillPauseHook>>,
    ) {
        self.lookup_post_scan_pause_hook.store(hook);
    }

    /// P0-3a (#1011) test/telemetry oracle: how many `drop_index` drains
    /// genuinely had to wait for at least one in-flight reader. Passthrough to
    /// [`reader_drain_gate::ReaderDrainGate::drain_waits`]. The drain-waits
    /// counter is the test oracle that discriminates "gate genuinely blocked a
    /// drain" from "gate was wired but never contended" — a lone `== 0`
    /// assertion passes vacuously if the counter is never incremented, so test
    /// suites always pair it with the contended `== 1` case (see
    /// `p1011_reader_drain_tests`).
    pub fn reader_drain_waits(&self) -> usize {
        self.reader_gate.drain_waits()
    }

    /// P0-3a (#1011) test-only: expose the reader gate for in-flight count
    /// checks. Used by regression tests to prove a reader is counted mid-flight
    /// (e.g., to verify `in_flight_count() == 1` while a read is parked at
    /// `lookup_pause_hook`). Not `#[cfg(test)]`-gated — cross-crate test
    /// consumer, same reason as the other hooks.
    pub fn reader_gate(&self) -> &crate::reader_drain_gate::ReaderDrainGate {
        &self.reader_gate
    }

    /// P0-3 (#959) test-only: install (or clear with `None`) the deterministic
    /// post-sweep pause hook (fires after the posting sweep, before the
    /// reduced `IndexInfo` is persisted — the exact crash window sub-bug 3c
    /// tests). Not `#[cfg(test)]`-gated — cross-crate test consumer, same
    /// reason as `set_drop_index_pause_hook`.
    pub fn set_drop_index_post_sweep_hook(
        &self,
        hook: Option<Arc<crate::base_index::backfill_pause_hook::BackfillPauseHook>>,
    ) {
        self.drop_index_post_sweep_hook.store(hook);
    }

    /// #1069 test-only: install (or clear with `None`) the deterministic
    /// pause point that fires AFTER the terminal `Succeeded` status is written
    /// but BEFORE `clear_from_dropping` runs. This is the exact window the
    /// inline-path write-order fix proves is crash-safe. NOT `#[cfg(test)]`-
    /// gated — cross-crate test consumer, same reason as the other hooks.
    pub fn set_drop_index_status_pause_hook(
        &self,
        hook: Option<Arc<crate::base_index::backfill_pause_hook::BackfillPauseHook>>,
    ) {
        self.drop_index_status_pause_hook.store(hook);
    }

    /// #997 test-only: install (or clear with `None`) the deterministic
    /// rename mid-pause hook (fires after the tombstone is written, before
    /// the second mutating step in `rename_index`). NOT `#[cfg(test)]`-gated
    /// — cross-crate test consumer, same reason as the other hooks.
    pub fn set_rename_mid_pause_hook(
        &self,
        hook: Option<Arc<crate::base_index::backfill_pause_hook::BackfillPauseHook>>,
    ) {
        self.rename_mid_pause_hook.store(hook);
    }

    /// #997 test-only: fire the rename mid-point pause hook if installed.
    /// Called by `TableManager::rename_index` between the two mutating steps
    /// of each hash-rename path. `None` on every real path — one uncontended
    /// `ArcSwapOption::load_full()` Acquire load.
    pub async fn maybe_pause_rename_mid(&self) {
        if let Some(hook) = self.rename_mid_pause_hook.load_full() {
            hook.wait_at_window().await;
        }
    }

    /// #997 post-review fix (task #1000) test-only: install (or clear with
    /// `None`) the deterministic pause point fired between entries in
    /// `TableManager::recover_hash_renames`'s per-family reconciliation
    /// loop. See the field's doc for the crash window this simulates.
    pub fn set_recover_renames_between_entries_hook(
        &self,
        hook: Option<Arc<crate::base_index::backfill_pause_hook::BackfillPauseHook>>,
    ) {
        self.recover_renames_between_entries_hook.store(hook);
    }

    /// #997 post-review fix (task #1000) test-only: fire the
    /// between-entries recovery pause hook if installed. Called by
    /// `TableManager::recover_hash_renames` after each tombstone entry is
    /// fully reconciled. `None` on every real path — one uncontended
    /// `ArcSwapOption::load_full()` Acquire load.
    pub async fn maybe_pause_recover_renames_between_entries(&self) {
        if let Some(hook) = self.recover_renames_between_entries_hook.load_full() {
            hook.wait_at_window().await;
        }
    }
}
