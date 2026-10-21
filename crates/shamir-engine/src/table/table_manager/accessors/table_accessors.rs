#[cfg(any(test, feature = "test-util"))]
use crate::index::sorted_index_manager::SortedIndexManager;
#[cfg(any(test, feature = "test-util"))]
use crate::table::table::Table;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use futures::FutureExt;

use crate::index::index_manager::IndexManager;
use crate::table::interner_manager::InternerManager;
use crate::table::record_counter::RecordCounter;
use shamir_storage::error::DbResult;
use shamir_storage::types::Store;

use super::super::read_only_views::{TableDataStoreReadView, TableInfoStoreReadView};
use super::super::TableManager;
use super::super::{panic_payload_message, table_token_for, AUTO_VERIFY_EVERY_N_WRITES};

impl TableManager {
    /// Increment the watchdog counter for `n` writes. Every
    /// `AUTO_VERIFY_EVERY_N_WRITES`-th increment spawns a
    /// non-blocking background `verify()` and logs at WARN if it
    /// reports inconsistency. Best-effort signal — does NOT block
    /// the caller, does NOT auto-repair (user calls `repair()`
    /// when ready).
    pub fn bump_write_counter(&self, n: u64) {
        if n == 0 {
            return;
        }
        let prev = self.write_counter.fetch_add(n, Ordering::Relaxed);
        let next = prev.saturating_add(n);
        let crossed = prev / AUTO_VERIFY_EVERY_N_WRITES != next / AUTO_VERIFY_EVERY_N_WRITES;
        if !crossed {
            return;
        }
        // Single-flight: skip if another verify is in flight.
        if self
            .verify_running
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }
        let self_clone = self.clone();
        tokio::spawn(async move {
            // Panic-safety (task group 10 / audit finding 1.4, 6.6, 7.1): a
            // panic inside `verify()` must not strand `verify_running = true`
            // forever — that would permanently disable this table's
            // background consistency watchdog with no signal to anyone.
            // `catch_unwind` converts a panic into an `Err` so the
            // unconditional `verify_running.store(false, ..)` below always
            // runs, exactly like an ordinary `verify()` error. The wrapping
            // `async { .. }` block ensures both invoking `verify()` and
            // polling its future are covered by the same `catch_unwind`.
            let result: DbResult<crate::table::doctor::VerifyReport> =
                match std::panic::AssertUnwindSafe(async { self_clone.verify().await })
                    .catch_unwind()
                    .await
                {
                    Ok(inner) => inner,
                    Err(payload) => {
                        let msg = panic_payload_message(&payload);
                        log::error!(
                            "Background verify on '{}' panicked: {msg}",
                            self_clone.name(),
                        );
                        Err(shamir_storage::error::DbError::Internal(format!(
                            "background verify panicked: {msg}"
                        )))
                    }
                };
            match result {
                Ok(report) => {
                    if !report.is_healthy() {
                        log::warn!(
                            "Background verify flagged inconsistency in '{}': {:?}",
                            self_clone.name(),
                            report,
                        );
                    }
                }
                Err(e) => log::warn!("Background verify on '{}' failed: {}", self_clone.name(), e,),
            }
            self_clone.verify_running.store(false, Ordering::Release);
        });
    }

    /// Whether a background verify is currently in flight. Test-
    /// support accessor; users normally don't care.
    pub fn is_background_verify_running(&self) -> bool {
        self.verify_running.load(Ordering::Acquire)
    }

    /// Test-only: arm the deterministic panic-injection seam so the next
    /// `verify()` call panics instead of running its normal audit — see the
    /// `verify_panic_hook` field doc.
    #[cfg(test)]
    pub fn install_verify_panic_hook(&self) {
        self.verify_panic_hook
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    /// Test-only: disarm the panic-injection seam installed by
    /// [`install_verify_panic_hook`](Self::install_verify_panic_hook) so the
    /// next `verify()` call runs its normal audit again.
    #[cfg(test)]
    pub fn clear_verify_panic_hook(&self) {
        self.verify_panic_hook
            .store(false, std::sync::atomic::Ordering::SeqCst);
    }

    /// Test-only: number of times `verify()` has been entered (panicking or
    /// not) since this `TableManager` was created. See `verify_call_count`'s
    /// field doc.
    #[cfg(test)]
    pub fn verify_call_count(&self) -> usize {
        self.verify_call_count
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    #[cfg(any(test, feature = "test-util"))]
    pub fn table_for_test(&self) -> &Table {
        &self.table
    }

    /// Stable u64 identifier for this table, used as key in
    /// `TxContext.write_set` and `counter_deltas`.
    ///
    /// Stage 4 implementation: deterministic hash of `self.name`.
    /// Stage 5 will replace with real repo-level interner ID.
    pub fn table_token(&self) -> u64 {
        table_token_for(&self.name)
    }

    pub(crate) fn data_store(&self) -> &Arc<dyn Store> {
        self.table.data_store()
    }

    /// Borrow the info_store this table writes its sidecar
    /// metadata into (counter, interner dictionary, sorted-index
    /// blob, WAL, buffer config, ...). DDL uses this directly.
    pub(crate) fn info_store(&self) -> &Arc<dyn Store> {
        &self.info_store
    }

    #[cfg(any(test, feature = "test-util"))]
    pub fn data_store_for_test(&self) -> &Arc<dyn Store> {
        self.table.data_store()
    }

    /// S5c-X read-only view over the info store for PRODUCTION cross-crate
    /// callers (`shamir-db` catalogue / DDL-status readers). The raw
    /// `info_store` handle stays `pub(crate)`; the view
    /// exposes exactly the read/durability methods those callers use (see
    /// `write_receipt/admission/views/read_only_views.rs`).
    pub fn info_store_read_view(&self) -> TableInfoStoreReadView<'_> {
        TableInfoStoreReadView { manager: self }
    }

    /// S5c-X read-only view over the data store for PRODUCTION cross-crate
    /// callers (`shamir-db` migration seeding, `SystemStore` durability
    /// flushes). The raw `data_store` handle stays
    /// `pub(crate)`.
    pub fn data_store_read_view(&self) -> TableDataStoreReadView<'_> {
        TableDataStoreReadView { manager: self }
    }

    /// Test-only raw info-store handle for fixture code (same gating as
    /// [`data_store_for_test`](Self::data_store_for_test)): returns an owned
    /// `Arc` clone so tests can call store-level helpers (ddl op-log writes,
    /// validator-metadata reads) directly.
    #[cfg(any(test, feature = "test-util"))]
    pub fn info_store_for_test(&self) -> Arc<dyn Store> {
        Arc::clone(&self.info_store)
    }

    #[cfg(any(test, feature = "test-util"))]
    pub fn interner_for_test(&self) -> &InternerManager {
        &self.interner
    }

    #[cfg(any(test, feature = "test-util"))]
    pub fn counter_for_test(&self) -> &Arc<RecordCounter> {
        &self.counter
    }

    #[cfg(test)]
    pub fn index_manager(&self) -> &IndexManager {
        &self.index_manager
    }

    /// Borrow the table's `IndexManager`. Public so the `db_instance`
    /// admin path (`create_index_async`) can register / drop indices via
    /// `TableManager` from outside this module — previously `pub(crate)`
    /// when this code was a single crate, but `db_instance` and
    /// `table_manager` now live in adjacent crate modules and the
    /// boundary needs `pub`.
    #[cfg(any(test, feature = "test-util"))]
    pub fn index_manager_ref_for_test(&self) -> &IndexManager {
        &self.index_manager
    }

    #[cfg(any(test, feature = "test-util"))]
    pub fn index2_registry_for_test(&self) -> &Arc<crate::index2::IndexRegistry> {
        &self.index2_registry
    }

    #[cfg(any(test, feature = "test-util"))]
    pub fn mvcc_store_for_test(&self) -> Option<Arc<shamir_tx::MvccStore>> {
        self.mvcc_store_ref().cloned()
    }

    // -----------------------------------------------------------------------
    // Engine-internal raw component handles. NOT part of the public boundary:
    // cross-crate callers must go through the read-only views above or the
    // admitted ops in `write_receipt/admission/`. They stay reachable inside
    // `shamir-engine` (query batch, repo seed/recovery, doctor, benches)
    // because those callers are engine-trusted code.
    // -----------------------------------------------------------------------

    pub(crate) fn interner(&self) -> &InternerManager {
        &self.interner
    }

    pub(crate) fn counter(&self) -> &Arc<RecordCounter> {
        &self.counter
    }

    pub(crate) fn index_manager_ref(&self) -> &IndexManager {
        &self.index_manager
    }

    pub(crate) fn index2_registry(&self) -> &Arc<crate::index2::IndexRegistry> {
        &self.index2_registry
    }

    pub(crate) fn unique_write_lock(&self) -> Arc<tokio::sync::Mutex<()>> {
        Arc::clone(&self.unique_write_lock)
    }

    /// Test-only: install (or clear with `None`) the deterministic
    /// `create_index_v2` backfill→register pause hook (#534 finding 1).
    #[cfg(test)]
    pub(crate) fn set_create_index2_backfill_hook(
        &self,
        hook: Option<Arc<crate::table::index2_backfill_hook::BackfillPauseHook>>,
    ) {
        self.create_index2_backfill_hook.store(hook);
    }

    /// F-72 (#899, P0) test-only: install (or clear with `None`) the
    /// deterministic `create_sorted_index_with_include` backfill pause hook.
    /// See `create_sorted_index_backfill_hook`'s field doc.
    #[cfg(test)]
    pub(crate) fn set_create_sorted_index_backfill_hook(
        &self,
        hook: Option<Arc<crate::table::index2_backfill_hook::BackfillPauseHook>>,
    ) {
        self.create_sorted_index_backfill_hook.store(hook);
    }

    /// F-76 (#903) test-only: install (or clear with `None`) the deterministic
    /// `drop_index2` pause hook (fires between the registry retirement and the
    /// posting sweep). See `drop_index2_pause_hook`'s field doc.
    #[cfg(test)]
    pub(crate) fn set_drop_index2_pause_hook(
        &self,
        hook: Option<Arc<crate::table::index2_backfill_hook::BackfillPauseHook>>,
    ) {
        self.drop_index2_pause_hook.store(hook);
    }

    /// P0-3b (#988) test-only: install (or clear with `None`) the deterministic
    /// `drop_index2` post-sweep pause hook (fires after the posting sweep,
    /// before the reduced metadata persist). See
    /// `drop_index2_post_sweep_hook`'s field doc.
    #[cfg(test)]
    pub(crate) fn set_drop_index2_post_sweep_hook(
        &self,
        hook: Option<Arc<crate::table::index2_backfill_hook::BackfillPauseHook>>,
    ) {
        self.drop_index2_post_sweep_hook.store(hook);
    }

    /// #1066 test-only: install (or clear with `None`) the deterministic
    /// `rename_index` (`is_index2` branch) early pause hook (fires after the
    /// durable RENAME tombstone is written, before `rename_entry` mutates
    /// the live registry). See `rename_index2_early_pause_hook`'s field doc.
    #[cfg(test)]
    pub(crate) fn set_rename_index2_early_pause_hook(
        &self,
        hook: Option<Arc<crate::table::index2_backfill_hook::BackfillPauseHook>>,
    ) {
        self.rename_index2_early_pause_hook.store(hook);
    }

    /// #1066 test-only: install (or clear with `None`) the deterministic
    /// `rename_index` (`is_index2` branch) mid pause hook (fires after
    /// `rename_entry` mutates the live registry, before
    /// `save_index2_metadata` persists it). See
    /// `rename_index2_mid_pause_hook`'s field doc.
    #[cfg(test)]
    pub(crate) fn set_rename_index2_mid_pause_hook(
        &self,
        hook: Option<Arc<crate::table::index2_backfill_hook::BackfillPauseHook>>,
    ) {
        self.rename_index2_mid_pause_hook.store(hook);
    }

    /// #1003 test-only: install (or clear with `None`) the deterministic
    /// pause hook that fires after `create_index_v2`'s non-btree branch
    /// registers the backend in the live registry (still `Building`) but
    /// before it flips to `Ready`. See
    /// `index2_registered_before_ready_hook`'s field doc.
    #[cfg(test)]
    pub(crate) fn set_index2_registered_before_ready_hook(
        &self,
        hook: Option<Arc<crate::table::index2_backfill_hook::BackfillPauseHook>>,
    ) {
        self.index2_registered_before_ready_hook.store(hook);
    }

    /// P-1060 test-only: install (or clear with `None`) the deterministic
    /// Phase B pause hook (fires after `mark_build_in_flight` but before
    /// dropping barrier guards). See `phase_b_pause_hook`'s field doc.
    #[cfg(test)]
    pub(crate) fn set_phase_b_pause_hook(
        &self,
        hook: Option<Arc<crate::table::index2_backfill_hook::BackfillPauseHook>>,
    ) {
        self.phase_b_pause_hook.store(hook);
    }

    /// P-1060 test-only: install (or clear with `None`) the deterministic
    /// Phase C pause hook (fires at the top of the catch-up loop, first iteration).
    /// See `phase_c_pause_hook`'s field doc.
    #[cfg(test)]
    pub(crate) fn set_phase_c_pause_hook(
        &self,
        hook: Option<Arc<crate::table::index2_backfill_hook::BackfillPauseHook>>,
    ) {
        self.phase_c_pause_hook.store(hook);
    }

    /// P-1060 test-only: install (or clear with `None`) the deterministic
    /// Phase D pause hook (fires after the publish barrier is acquired but
    /// before the final residual drain). See `phase_d_pause_hook`'s field doc.
    #[cfg(test)]
    pub(crate) fn set_phase_d_pause_hook(
        &self,
        hook: Option<Arc<crate::table::index2_backfill_hook::BackfillPauseHook>>,
    ) {
        self.phase_d_pause_hook.store(hook);
    }

    /// Clone the handle to this table's unique-write serialisation lock.
    ///
    /// **Physical authority for uniqueness (HIGH-A) — defense-in-depth contract
    /// (②.3b).** This `tokio::sync::Mutex` is the serialiser that closes the
    /// non-tx ↔ tx-commit unique race; the logical fail-fast probe in
    /// `schema_validator.rs` (`Phase C3 — unique constraint`) runs ABOVE this
    /// layer as early diagnosis + a clean field-scoped error, but is NOT the
    /// atomicity authority (it has a pre-commit TOCTOU window).
    ///
    /// HIGH-A — closing the non-tx ↔ tx-commit unique race. Non-tx
    /// `insert` / `set` / `delete` take this `tokio::sync::Mutex` around their
    /// validate-then-write-then-index window (see those methods). The tx
    /// commit pipeline (`commit_tx_inner` Phase 2.6 → 5c) acquires the SAME
    /// lock for every table that has unique guards, so a tx's unique
    /// re-check and its posting write are atomic against any concurrent non-tx
    /// unique writer to that table. Returns the `Arc` (cloned) so the caller
    /// holds the exact same mutex instance the non-tx path locks.
    #[cfg(any(test, feature = "test-util"))]
    pub fn unique_write_lock_for_test(&self) -> Arc<tokio::sync::Mutex<()>> {
        Arc::clone(&self.unique_write_lock)
    }

    #[cfg(any(test, feature = "test-util"))]
    pub fn sorted_indexes_for_test(&self) -> &SortedIndexManager {
        &self.sorted_indexes
    }
}
