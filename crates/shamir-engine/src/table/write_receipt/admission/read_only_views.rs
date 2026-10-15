//! Public READ-ONLY surface over `TableManager` internals.
//!
//! S5c-X boundary: the raw component handles (`info_store`, `data_store`,
//! `interner`, index managers) are `pub(crate)` — invisible to cross-crate
//! callers. Production code in `shamir-db` / `shamir-server` that only READS
//! (DDL op-status polling, catalogue interner lookups, durability flushes,
//! index introspection for DESCRIBE/LIST/export/ORDER-BY planning) goes
//! through this file instead. Nothing here can mutate table data, the
//! interner dictionary, or registry state:
//!
//! * every method either reads, or performs an idempotent durability flush
//!   of already-committed state;
//! * the interner dictionary itself NEVER escapes — no method returns
//!   `&Interner`, `&InternerManager`, or `Arc<OnceCell<Interner>>`. The two
//!   types that wrap it ([`InternerReadView`] and [`FilterContext`], via
//!   [`TableInfoStoreReadView::filter_context`]) expose read accessors only
//!   (Round 8: `resolve_filter_value`, the one public converter reachable
//!   through a context-borrowed interner, now resolves names with
//!   `get_ind` and fails closed — it never interns);
//! * [`ValidatorRegistryReadView`] wraps the validator registry PRIVATELY —
//!   its `register`/`replace`/`rename`/`remove` mutators are unreachable
//!   from it.
//!
//! The interner WRITE path (persisting appended dictionary chunks) is a
//! rare DDL/catalogue op and lives on the admitted side:
//! `TableManager::persist_interner_admitted` (see `admitted_ops.rs`).
//!
//! Test fixtures that need the RAW stores use the `*_for_test` getters
//! (gated by `cfg(any(test, feature = "test-util"))`) instead — never these
//! views' raw handles. Mutation goes through the admitted ops in
//! `admitted_ops.rs`.
//!
//! [`FilterContext`]: crate::query::filter::FilterContext

use std::sync::Arc;

use shamir_index::base_index::ddl_op_log;
use shamir_index::base_index::index_definition::IndexDefinition;
use shamir_index::base_index::sorted_index_definition::SortedIndexDefinition;
use shamir_index::descriptor::IndexDescriptor;
use shamir_query_types::filter::Filter;
use shamir_query_types::read::DdlOpStatus;
use shamir_storage::error::DbResult;
use shamir_types::codecs::interned::inner_value_to_query_value;
use shamir_types::core::interner::{Interner, InternerKey};
use shamir_types::record_view::{RecordRef, RecordView};
use shamir_types::types::common::TMap;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::{InnerValue, QueryValue};
use tokio::sync::OnceCell;

use super::TableManager;
use crate::query::filter::{compile_filter, FilterContext, FilterNode};

/// Read-only view over the table's sidecar metadata (`info`) store.
///
/// Obtain via [`TableManager::info_store_read_view`]. Exposes exactly the
/// read / durability methods production callers use:
///
/// * [`read_op_status`](Self::read_op_status) — DDL op-status polling
///   (#1069), the read half of `ddl_op_log`.
/// * [`flush`](Self::flush) — durability flush of already-written sidecar
///   state (no new bytes are produced by this call).
/// * [`filter_context`](Self::filter_context) / [`interner_read_view`](Self::interner_read_view)
///   — query-planning reads over the interner dictionary WITHOUT handing
///   out `&Interner` (S5c-X F1, Round 6).
/// * [`inner_to_query_value`](Self::inner_to_query_value) — pure-read codec
///   twin that keeps the dictionary encapsulated. (The old `to_inner` twin
///   was removed in S5c-X Round 7: it was a WRITE-prep interning step that
///   mutated through the read view; that interning now lives behind
///   admission in `TableManager::intern_query_value_admitted`, chained with
///   the dictionary-delta persist by
///   `SystemStore::persist_table_interner` (called with `Some(record)`).)
/// * [`load_migration_state`](Self::load_migration_state) — read a persisted
///   migration state record for COMMIT MIGRATION retry.
pub struct TableInfoStoreReadView<'a> {
    pub(super) manager: &'a TableManager,
}

impl<'a> TableInfoStoreReadView<'a> {
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

    /// Build the engine's filter evaluation context so the caller never
    /// holds `&Interner` (S5c-X F1, Round 6 — replaces the old
    /// `interner_get` + `FilterContext::new(interner, refs)` pair).
    ///
    /// The returned borrow is tied to the MANAGER lifetime `'a` — not to
    /// this view, which callers typically create as a temporary — so the
    /// `FilterContext` composes with the subsequent `table.read(...)` call
    /// and with a caller-owned `resolved_refs` map.
    pub async fn filter_context(
        &self,
        resolved_refs: &'a TMap<String, crate::query::read::QueryResult>,
    ) -> DbResult<FilterContext<'a>> {
        let interner = self.manager.interner.get().await?;
        Ok(FilterContext::new(interner, resolved_refs))
    }

    /// Shared, POPULATED read view over the interner dictionary (S5c-X F1,
    /// Round 6 — replaces the old `interner_cell` + `interner_get` pair for
    /// callers that must hold the dictionary across an `.await` or in a
    /// cache: the subscription decode cache, the changelog decoder, doctor
    /// name resolution). The cell is initialized here; see
    /// [`InternerReadView`] for why mutation is unreachable through it.
    pub async fn interner_read_view(&self) -> DbResult<Arc<InternerReadView>> {
        // Loading here both proves the cell is populated (a `get()` on the
        // view below can then never be empty) and fails loudly on a
        // corrupted dictionary exactly like the old `interner_get` did.
        let _loaded = self.manager.interner.get().await?;
        Ok(Arc::new(InternerReadView {
            cell: self.manager.interner.interner_cell(),
        }))
    }

    /// Interned field id for `name` (`Interner::get_ind`), named for its
    /// use: unique/FK rule → single-field index existence checks in the
    /// shamir-db schema DDL handler. `None` when the name was never interned.
    pub async fn interned_field_id(&self, name: &str) -> DbResult<Option<u64>> {
        let interner = self.manager.interner.get().await?;
        Ok(interner.get_ind(name).map(|key| key.id()))
    }

    /// Resolve a `(String)` field path into interned `u64` ids — engine-side
    /// twin of `query::filter::intern_field_path` (`get_ind` lookups), used
    /// by the cursor keyset IndexSeek eligibility probe. `None` when any
    /// segment was never interned.
    pub async fn intern_field_path(&self, field: &[String]) -> DbResult<Option<Vec<u64>>> {
        let interner = self.manager.interner.get().await?;
        Ok(crate::query::filter::intern_field_path(field, interner))
    }

    /// Reverse lookup: string for an interned id (`Interner::get_str`), used
    /// for DESCRIBE/LIST index-name resolution and doctor index-name
    /// recovery. `None` when the id has no mapped name.
    pub async fn interner_str(&self, id: u64) -> DbResult<Option<String>> {
        let interner = self.manager.interner.get().await?;
        Ok(interner
            .get_str(&InternerKey::new(id))
            .map(|arc| arc.to_string()))
    }

    /// Decode an `InnerValue` (changelog payload) back into a string-keyed
    /// `QueryValue` using this table's dictionary. Faithful wrapper of
    /// `shamir_types::codecs::interned::inner_value_to_query_value`.
    pub async fn inner_to_query_value(&self, value: &InnerValue) -> DbResult<QueryValue> {
        let interner = self.manager.interner.get().await?;
        inner_value_to_query_value(value, interner)
            .map_err(|e| shamir_storage::error::DbError::Codec(e.to_string()))
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

/// Read-only handle to a table's interner dictionary (S5c-X F1, Round 6).
///
/// Wraps the interner's shared loader cell PRIVATELY. The cell is populated
/// at construction time (`TableInfoStoreReadView::interner_read_view`) and a
/// `tokio::sync::OnceCell` can never be re-initialized, so the only
/// operations reachable from this type are the pure-read accessors below —
/// `Interner`'s `touch_ind`/`touch_with_id` mutators require the raw
/// `&Interner`, which never escapes. Cheap to clone (`Arc`); safe to hold
/// across `.await` points or in a cache without keeping the `TableManager`
/// alive.
pub struct InternerReadView {
    /// PRIVATE. Guaranteed populated (see type doc).
    cell: Arc<OnceCell<Interner>>,
}

impl InternerReadView {
    /// Test-only constructor: wrap an already-built raw interner (unit /
    /// integration tests that don't go through a `TableManager`). The cell
    /// is populated here, so the read accessors' invariants hold.
    #[cfg(any(test, feature = "test-util"))]
    pub fn from_interner_for_test(interner: Interner) -> Self {
        let cell = Arc::new(OnceCell::new());
        let _ = cell.set(interner);
        Self { cell }
    }

    /// Populated-cell accessor; only ever called by the read accessors.
    fn interner(&self) -> &Interner {
        self.cell
            .get()
            .expect("InternerReadView is constructed only after the cell is populated")
    }

    /// Monotonic dictionary generation (bumped on every new interning).
    /// Read-only; used by the subscription filter compile cache to detect
    /// growth.
    pub fn generation(&self) -> u64 {
        self.interner().generation()
    }

    /// Reverse lookup: string for an interned id (`Interner::get_str`).
    pub fn get_str(&self, id: &InternerKey) -> Option<Arc<str>> {
        self.interner().get_str(id)
    }

    /// Forward lookup: interned id for a string (`Interner::get_ind`).
    /// `None` when never interned; this never mints a new id.
    pub fn get_ind(&self, name: &str) -> Option<InternerKey> {
        self.interner().get_ind(name)
    }

    /// Decode an `InnerValue` into a string-keyed `QueryValue` using this
    /// dictionary (engine-side wrapper of
    /// `shamir_types::codecs::interned::inner_value_to_query_value`, which
    /// reads the reverse snapshot only — no mutation).
    pub fn inner_to_query_value(&self, value: &InnerValue) -> DbResult<QueryValue> {
        inner_value_to_query_value(value, self.interner())
            .map_err(|e| shamir_storage::error::DbError::Codec(e.to_string()))
    }

    /// Compile a subscription filter against this dictionary (engine-side
    /// delegation to `query::filter::compile_filter`, which only performs
    /// `get_ind` lookups — read-only).
    pub fn compile_filter(&self, filter: &Filter) -> FilterNode {
        compile_filter(filter, self.interner())
    }

    /// Evaluate a compiled filter node against raw msgpack record bytes:
    /// zero-copy `RecordView` lens first, full `InnerValue` decode fallback
    /// (identical semantics to the engine's query-path evaluation; the
    /// `FilterContext` is built engine-side from an empty shared refs map,
    /// exactly like the previous server-side `make_filter_context`).
    /// Undecodable bytes fail closed with `false`.
    pub fn matches_bytes(&self, node: &FilterNode, bytes: &[u8]) -> bool {
        let ctx = self.eval_context();
        match RecordView::new(bytes) {
            Ok(view) => node.matches(&view, &ctx),
            Err(_) => match InnerValue::from_bytes(bytes) {
                Ok(inner) => node.matches(&inner, &ctx),
                Err(_) => {
                    log::warn!(
                        "subscription filter: failed to decode record bytes \
                         (both RecordView and InnerValue), skipping (fail-closed)"
                    );
                    false
                }
            },
        }
    }

    /// Evaluate a compiled filter node against a pre-decoded `InnerValue`
    /// (journal-backfill path that already decoded the value).
    pub fn matches_inner(&self, node: &FilterNode, value: &InnerValue) -> bool {
        node.matches(value, &self.eval_context())
    }

    /// Decode raw msgpack record bytes into a string-keyed `QueryValue` for
    /// delivery: zero-copy `RecordView` lens first (`QueryValue::Null` on a
    /// de-intern error inside a valid map), full `InnerValue` decode
    /// fallback for bare-scalar / non-map records. `None` when the bytes
    /// decode as neither shape.
    pub fn record_bytes_to_query_value(&self, bytes: &[u8]) -> Option<QueryValue> {
        if let Ok(view) = RecordView::new(bytes) {
            return Some(view.to_query_value(self.interner()));
        }
        let inner = InnerValue::from_bytes(bytes).ok()?;
        Some(inner.to_query_value(self.interner()))
    }

    /// Minimal evaluation context (empty shared refs map — subscriptions
    /// never use QueryRef/Param/FnCall), built engine-side so the raw
    /// dictionary stays encapsulated.
    fn eval_context(&self) -> FilterContext<'_> {
        static EMPTY_REFS: std::sync::OnceLock<TMap<String, crate::query::read::QueryResult>> =
            std::sync::OnceLock::new();
        let refs = EMPTY_REFS.get_or_init(shamir_types::types::common::new_map);
        FilterContext::new(self.interner(), refs)
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
    /// Read-only view over the validator registry bound to this table, if
    /// any (S5c-X F2, Round 6 — replaces `validator_registry()`, which
    /// handed out `&Arc<ValidatorRegistry>` and with it the registry's
    /// `register`/`replace`/`rename`/`remove` mutators). Keyset-safety
    /// planning only looks validators up by id — that is all
    /// [`ValidatorRegistryReadView`] exposes.
    pub fn validator_registry_read_view(&self) -> Option<ValidatorRegistryReadView> {
        self.validator_registry
            .as_ref()
            .map(|registry| ValidatorRegistryReadView {
                registry: Arc::clone(registry),
            })
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

/// Read-only view over a table's [`crate::validator::ValidatorRegistry`]
/// (S5c-X F2, Round 6). The registry `Arc` is PRIVATE; the registry's
/// `register`/`replace`/`rename`/`remove`/`replace_artifact` mutators are
/// unreachable from this type — only id lookups are exposed.
pub struct ValidatorRegistryReadView {
    /// PRIVATE. Mutation goes through the admitted engine DDL paths only.
    registry: Arc<crate::validator::ValidatorRegistry>,
}

impl ValidatorRegistryReadView {
    /// Look up a compiled validator by record id (read-only twin of
    /// `ValidatorRegistry::get_by_id`).
    pub fn get_by_id(&self, id: &RecordId) -> Option<Arc<dyn crate::validator::RecordValidator>> {
        self.registry.get_by_id(id)
    }
}
