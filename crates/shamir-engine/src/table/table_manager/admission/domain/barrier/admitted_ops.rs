//! Engine-owned ADMITTED operations on `TableManager`.
//!
//! S5c-X boundary: cross-crate callers (`shamir-db` facade) must not reach
//! the raw `pub(crate)` component handles to perform mutations. Every method
//! here takes `&RepoWritePermit`, verifies it against THIS table's
//! repository write domain (`debug_check_permit`), and then drives the same
//! raw primitives the engine itself uses. No admission is acquired twice:
//! the caller holds the permit for the whole surrounding critical section
//! and only borrows it here.
//!
//! The read-only counterpart of this file is `read_only_views.rs`.

use std::sync::Arc;

use futures::StreamExt;
use shamir_index::base_index::ddl_op_log;
use shamir_index::build_backend::build_index2_backend_with_resolver;
use shamir_query_types::read::DdlOpStatus;
use shamir_storage::error::{DbError, DbResult};
use shamir_types::codecs::interned::query_value_to_inner;
use shamir_types::types::value::{InnerValue, QueryValue};

use super::TableManager;
use crate::index2::state::IndexState;
use crate::migration::{MigrationCoordinator, MigrationShadowLog, MigrationState};
use crate::repo::RepoWritePermit;
use crate::table::MigrationFreezeGuard;

impl TableManager {
    /// Admitted twin of `ddl_op_log::write_op_status`: write the InProgress
    /// crash-safety record for a DDL op into this table's info store. The
    /// caller (shamir-db DROP/RENAME INDEX facade) holds repo admission for
    /// the op; the subsequent mutation itself re-admits through the public
    /// `drop_index` / `rename_index` entry points as before.
    pub async fn write_ddl_op_status_admitted(
        &self,
        status: &DdlOpStatus,
        permit: &RepoWritePermit,
    ) -> DbResult<()> {
        self.write_domain.debug_check_permit(permit);
        ddl_op_log::write_op_status(&self.info_store, status).await
    }

    /// Admitted "drop every index of every family" sweep used by the DROP
    /// TABLE degraded-cleanup path (shamir-db `handle_drop_table`). Acquires
    /// THIS table's write barrier (bit `REGULAR_INDEX_CREATE` — the same
    /// single `ddl_admission` mutex backs every bit) exactly like the
    /// previous facade code, then drops the raw primitives' contents.
    /// Individual drop failures are logged and otherwise tolerated
    /// (degraded-cleanup semantics unchanged): the table is about to be
    /// removed entirely.
    pub async fn drop_all_indexes_for_table_removal_admitted(
        &self,
        permit: &RepoWritePermit,
    ) -> DbResult<()> {
        self.write_domain.debug_check_permit(permit);
        let (_barrier, _uwl_guard) = self
            .begin_write_barrier_admitted(
                crate::index::write_barrier_flags::REGULAR_INDEX_CREATE,
                permit,
            )
            .await;
        // base_index regular indexes.
        let regular_ids: Vec<u64> = self
            .index_manager_ref()
            .iter_indexes()
            .map(|d| d.name_interned)
            .collect();
        for id in regular_ids {
            if let Err(e) = self.index_manager_ref().drop_index(id, None, None).await {
                log::warn!(
                    "drop_all_indexes_for_table_removal: failed to drop regular index {id}: {e}"
                );
            }
        }
        // base_index unique indexes.
        let unique_ids: Vec<u64> = self
            .index_manager_ref()
            .iter_unique_indexes()
            .map(|d| d.name_interned)
            .collect();
        for id in unique_ids {
            if let Err(e) = self
                .index_manager_ref()
                .drop_unique_index(id, None, None)
                .await
            {
                log::warn!(
                    "drop_all_indexes_for_table_removal: failed to drop unique index {id}: {e}"
                );
            }
        }
        // Sorted indexes.
        let sorted_ids: Vec<u64> = self
            .sorted_indexes()
            .iter_indexes()
            .iter()
            .map(|d| d.name_interned)
            .collect();
        for id in sorted_ids {
            if let Err(e) = self.sorted_indexes().drop_index(id, None, None).await {
                log::warn!(
                    "drop_all_indexes_for_table_removal: failed to drop sorted index {id}: {e}"
                );
            }
        }
        // index2 registry — remove all backends.
        let backends = self.index2_registry().all_backends().await;
        for b in &backends {
            if self
                .index2_registry()
                .remove_by_id(b.descriptor().id)
                .await
                .is_none()
            {
                log::warn!(
                    "drop_all_indexes_for_table_removal: failed to drop index2 backend {} (not found)",
                    b.descriptor().id
                );
            }
        }
        Ok(())
    }

    /// Admitted MVCC current-version seeding used by COMMIT MIGRATION
    /// finalisation step 1: stream the destination data store and seed every
    /// row that has no current version yet. Returns `true` when the MVCC
    /// seeding path ran, `false` when this table has no `MvccStore` (the
    /// caller then falls back to
    /// [`seed_log_from_data_store_admitted`](Self::seed_log_from_data_store_admitted),
    /// exactly like the previous facade `if let Some(mvcc) = ...` branch).
    pub async fn seed_current_versions_admitted(&self, permit: &RepoWritePermit) -> DbResult<bool> {
        self.write_domain.debug_check_permit(permit);
        let Some(mvcc) = self.mvcc_store() else {
            return Ok(false);
        };
        let mut stream = self.table.data_store().iter_stream(256);
        while let Some(batch) = stream.next().await {
            for (key, value) in batch? {
                if mvcc.get_current(key.clone()).await?.is_none() {
                    // commit_migration retains destination barrier/UWL
                    // through index finalisation. No wait: ack -> lower
                    // orphan -> drainer Phase B posting replay -> this
                    // barrier -> ack would deadlock.
                    mvcc.set_versioned_many_no_wait(vec![(key, value)]).await?;
                }
            }
        }
        mvcc.flush_history().await?;
        Ok(true)
    }

    /// Admitted COMMIT MIGRATION finalisation step 2 backend rebuild:
    /// reset every index2 backend on this table (fresh, empty constructors —
    /// rebuilds add counts and would NOT reset a populated instance), bulk
    /// populate it from the migrated rows, and mark the family Ready. On a
    /// populate failure every rebuilt backend is marked `Failed` (recovered
    /// Failed entries become usable only after a complete replay) and the
    /// error is propagated.
    pub async fn reset_and_populate_index2_admitted(
        &self,
        permit: &RepoWritePermit,
        resolver: &shamir_funclib::scalar_resolver::ScalarResolver,
    ) -> DbResult<()> {
        self.write_domain.debug_check_permit(permit);
        let registry = self.index2_registry();
        let readers = registry.reader_gate().begin_drop();
        readers.wait_for_drain().await;
        for mut descriptor in registry.all_descriptors().await {
            // Cancellation releases the reader gate via RAII. Keep an
            // incomplete replacement planner-invisible until full replay.
            descriptor.state = IndexState::Building;
            let backend = build_index2_backend_with_resolver(
                descriptor.clone(),
                &self.info_store,
                Some(resolver.clone()),
            );
            registry.remove_by_id(descriptor.id).await;
            registry
                .insert(backend)
                .await
                .map_err(|e| DbError::Internal(format!("index2 backend insert failed: {e}")))?;
        }
        if let Err(e) = self.bulk_populate_index2_admitted(permit).await {
            for backend in registry.all_backends().await {
                registry
                    .set_failed(backend.descriptor().id, e.to_string())
                    .await;
            }
            return Err(e);
        }
        // Recovered Failed entries become usable only after complete replay.
        for backend in registry.all_backends().await {
            registry
                .set_state(backend.descriptor().id, IndexState::Ready)
                .await;
        }
        Ok(())
    }

    /// Admitted interner-dictionary durability flush (S5c-X F1): the WRITE
    /// tail of a catalogue/DDL op, replacing the old read-view
    /// `interner_persist` escape hatch. The caller (shamir-db
    /// system-store / replication-catalogue paths) holds repo admission for
    /// this rare write.
    pub async fn persist_interner_admitted(&self, permit: &RepoWritePermit) -> DbResult<()> {
        self.write_domain.debug_check_permit(permit);
        self.interner.persist().await
    }

    /// Admitted WRITE-prep interning of a `QueryValue` (S5c-X Round 7):
    /// replaces the read-view `TableInfoStoreReadView::to_inner`, whose only
    /// real effect was interning the record's map keys through the
    /// interior-mutable interner — a mutation that must sit behind
    /// admission. Interns `value`'s field names into THIS table's
    /// dictionary and returns the encoded `InnerValue` (callers that don't
    /// need it discard it). The caller holds repo admission; the usual
    /// companion is [`Self::persist_interner_admitted`] with the SAME
    /// permit (see `SystemStore::persist_table_interner` with
    /// `Some(record)`, which
    /// chains both under a single admission).
    pub async fn intern_query_value_admitted(
        &self,
        permit: &RepoWritePermit,
        value: &QueryValue,
    ) -> DbResult<InnerValue> {
        self.write_domain.debug_check_permit(permit);
        let interner = self.interner.get().await?;
        query_value_to_inner(value, interner).map_err(|e| DbError::Codec(e.to_string()))
    }

    /// Admitted MIGRATION coordinator factory (fresh START path). Builds the
    /// shadow log on THIS (source) table's info store, captures the source
    /// and destination data-store handles and the source's MVCC log seam,
    /// and takes over the caller-held source migration freeze — all raw work
    /// that used to be done by the facade with extracted `pub(crate)`
    /// handles. Both permits are identity-checked against their own
    /// repositories, and both write domains are captured on the coordinator
    /// so its admitted twins can re-verify every later mutation.
    ///
    /// F4 (Round 6): the freeze guard is a PARAMETER, acquired by the facade
    /// immediately after source-table resolution (master ordering — BEFORE
    /// the interner dictionary copy). There is no window where the guard is
    /// unowned.
    pub async fn migration_coordinator_admitted(
        &self,
        state: MigrationState,
        migration_id: String,
        dst: &TableManager,
        freeze: MigrationFreezeGuard,
        src_permit: &RepoWritePermit,
        dst_permit: &RepoWritePermit,
    ) -> DbResult<Arc<MigrationCoordinator>> {
        self.write_domain.debug_check_permit(src_permit);
        dst.write_domain.debug_check_permit(dst_permit);
        let shadow = Arc::new(MigrationShadowLog::new(
            migration_id,
            Arc::clone(&self.info_store),
        ));
        let coord = Arc::new(
            MigrationCoordinator::new(
                state,
                shadow,
                Arc::clone(self.data_store()),
                Arc::clone(dst.data_store()),
                self.mvcc_store(),
            )
            .with_migration_freeze(freeze)
            .with_domains(
                Arc::clone(&self.write_domain),
                Arc::clone(&dst.write_domain),
            ),
        );
        Ok(coord)
    }

    /// Admitted MIGRATION coordinator factory (restart-recovery path). The
    /// state was already loaded via the info-store read view's
    /// `load_migration_state`; this rebuilds the
    /// shadow log from the source info store and re-binds the source /
    /// destination data-store handles. A process-local freeze cannot be
    /// reconstructed from metadata (the caller rejects uncommitted states
    /// before calling), so no freeze is taken here — identical to the
    /// previous facade behaviour.
    pub async fn migration_coordinator_recovered_admitted(
        &self,
        state: MigrationState,
        dst: &TableManager,
        src_permit: &RepoWritePermit,
        dst_permit: &RepoWritePermit,
    ) -> DbResult<Arc<MigrationCoordinator>> {
        self.write_domain.debug_check_permit(src_permit);
        dst.write_domain.debug_check_permit(dst_permit);
        let shadow = Arc::new(
            MigrationShadowLog::recover(state.id.clone(), Arc::clone(&self.info_store)).await?,
        );
        Ok(Arc::new(
            MigrationCoordinator::new(
                state,
                shadow,
                Arc::clone(self.data_store()),
                Arc::clone(dst.data_store()),
                self.mvcc_store(),
            )
            // Recovery path stays freeze-free (a process-local freeze cannot
            // be reconstructed from metadata), but the domains are still
            // captured so the admitted twins re-verify every later mutation.
            .with_domains(
                Arc::clone(&self.write_domain),
                Arc::clone(&dst.write_domain),
            ),
        ))
    }
}
