//! Public READ-ONLY surface over `TableManager` internals.
//!
//! S5c-X boundary: the raw component handles (`info_store`, `data_store`,
//! `interner`, index managers) are `pub(crate)` — invisible to cross-crate
//! callers. Production code in `shamir-db` / `shamir-server` that only READS
//! (DDL op-status polling, catalogue interner lookups, durability flushes,
//! index introspection for DESCRIBE/LIST/export/ORDER-BY planning) goes
//! through this file instead. Nothing here can mutate table data or registry
//! state: every method either reads, or performs an idempotent durability
//! flush of already-committed state.
//!
//! Test fixtures that need the RAW stores use the `*_for_test` getters
//! (gated by `cfg(any(test, feature = "test-util"))`) instead — never these
//! views' raw handles. Mutation goes through the admitted ops in
//! `admitted_ops.rs`.

use std::sync::Arc;

use shamir_index::base_index::ddl_op_log;
use shamir_index::base_index::index_definition::IndexDefinition;
use shamir_index::base_index::sorted_index_definition::SortedIndexDefinition;
use shamir_index::descriptor::IndexDescriptor;
use shamir_query_types::read::DdlOpStatus;
use shamir_storage::error::DbResult;
use shamir_types::core::interner::Interner;
use shamir_types::types::record_id::RecordId;
use tokio::sync::OnceCell;

use super::TableManager;

/// Read-only view over the table's sidecar metadata (`info`) store.
///
/// Obtain via [`TableManager::info_store_read_view`]. Exposes exactly the
/// read / durability methods production callers use:
///
/// * [`read_op_status`](Self::read_op_status) — DDL op-status polling
///   (#1069), the read half of `ddl_op_log`.
/// * [`flush`](Self::flush) — durability flush of already-written sidecar
///   state (no new bytes are produced by this call).
/// * [`interner_get`](Self::interner_get) / [`interner_persist`](Self::interner_persist)
///   / [`interner_cell`](Self::interner_cell) — read the interner dictionary
///   for query planning, persist its append-only chunks (the DDL tail the
///   catalogue writers already performed), and share the loader cell with
///   the changelog writer.
/// * [`load_migration_state`](Self::load_migration_state) — read a persisted
///   migration state record for COMMIT MIGRATION retry.
pub struct TableInfoStoreReadView<'a> {
    pub(super) manager: &'a TableManager,
}

impl TableInfoStoreReadView<'_> {
    /// Read a DDL operation status record from this table's op log.
    /// Read-only twin of `ddl_op_log::read_op_status`.
    pub async fn read_op_status(&self, op_id: &RecordId) -> DbResult<Option<DdlOpStatus>> {
        ddl_op_log::read_op_status(&self.manager.info_store, op_id).await
    }

    /// Flush the info store so previously written sidecar metadata (DDL
    /// status records, interner chunks, counter blobs) is durable. Does not
    /// write anything new.
    pub async fn flush(&self) -> DbResult<()> {
        self.manager.info_store.flush().await
    }

    /// Load (or get the already loaded) interner dictionary. Read-only.
    pub async fn interner_get(&self) -> DbResult<&Interner> {
        self.manager.interner.get().await
    }

    /// Persist the interner dictionary's append-only chunks. This is the
    /// durability tail of a catalogue/DDL write the caller already made
    /// through admitted APIs; no dictionary state is mutated here.
    pub async fn interner_persist(&self) -> DbResult<()> {
        self.manager.interner.persist().await
    }

    /// Share the interner's lazy-loader cell (the changelog writer needs the
    /// SAME cell so it never races the table's own loader).
    pub fn interner_cell(&self) -> Arc<OnceCell<Interner>> {
        self.manager.interner.interner_cell()
    }

    /// Read a persisted migration state record for COMMIT MIGRATION retry.
    /// Read-only twin of `MigrationCoordinator::load_state`.
    pub async fn load_migration_state(
        &self,
        id: &str,
    ) -> DbResult<Option<crate::migration::MigrationState>> {
        crate::migration::MigrationCoordinator::load_state(&self.manager.info_store, id).await
    }
}

/// Read-only view over the table's row (`data`) store.
///
/// Obtain via [`TableManager::data_store_read_view`]. Today's production
/// readers only need the durability flush; the migration snapshot stream is
/// encapsulated engine-side in `admitted_ops.rs`.
pub struct TableDataStoreReadView<'a> {
    pub(super) manager: &'a TableManager,
}

impl TableDataStoreReadView<'_> {
    /// Flush the data store so previously committed rows are durable. Does
    /// not write any new row.
    pub async fn flush(&self) -> DbResult<()> {
        self.manager.table.data_store().flush().await
    }
}

impl TableManager {
    /// Read-only borrow of the validator registry bound to this table, if
    /// any. Read-only twin of `validator_registry_ref` (which is
    /// `pub(crate)`); keyset-safety planning only iterates the declared
    /// `SchemaValidator` rules — no registry mutation is possible here.
    pub fn validator_registry(&self) -> Option<&Arc<crate::validator::ValidatorRegistry>> {
        self.validator_registry.as_ref()
    }

    /// Read-only snapshot of the regular hash-index definitions (DESCRIBE /
    /// LIST / replication export). Read-only twin of
    /// `IndexManager::iter_indexes`.
    pub fn index_definitions(&self) -> Vec<IndexDefinition> {
        self.index_manager.iter_indexes().collect()
    }

    /// Read-only snapshot of the unique hash-index definitions.
    /// Read-only twin of `IndexManager::iter_unique_indexes`.
    pub fn unique_index_definitions(&self) -> Vec<IndexDefinition> {
        self.index_manager.iter_unique_indexes().collect()
    }

    /// Read-only snapshot of the sorted-index definitions.
    /// Read-only twin of `SortedIndexManager::iter_indexes`.
    pub fn sorted_index_definitions(&self) -> Vec<SortedIndexDefinition> {
        self.sorted_indexes.iter_indexes()
    }

    /// Read-only lookup of the sorted-index definition keyed on a field
    /// path (ORDER-BY planning). Read-only twin of
    /// `SortedIndexManager::find_by_field`.
    pub fn sorted_index_definition_by_field(
        &self,
        field_path: &[u64],
    ) -> Option<SortedIndexDefinition> {
        self.sorted_indexes.find_by_field(field_path)
    }

    /// Read-only snapshot of the index2 registry descriptors (FTS /
    /// Functional / Vector). Read-only twin of `IndexRegistry::all_descriptors`.
    pub async fn index2_descriptors(&self) -> Vec<IndexDescriptor> {
        self.index2_registry.all_descriptors().await
    }

    /// Read-only MVCC snapshot stream (seed-catalogue budget scan). Returns
    /// `None` when this table has no attached `MvccStore`. Read-only twin of
    /// `MvccStore::snapshot_stream`; no version is written.
    pub fn mvcc_snapshot_stream(
        &self,
        batch: usize,
        at_version: u64,
    ) -> Option<
        impl futures::Stream<
                Item = shamir_storage::error::DbResult<Vec<(bytes::Bytes, bytes::Bytes)>>,
            > + Send
            + '_,
    > {
        Some(self.mvcc_store_ref()?.snapshot_stream(batch, at_version))
    }
}
