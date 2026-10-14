//! Synchronous body of [`MvccStore::publish_cell`], split out of
//! `mvcc_store/mod.rs` (see `write_receipt::cell_publish`).

use shamir_storage::types::RecordKey;

use super::super::{MvccStore, RecordCell};

impl MvccStore {
    /// Synchronous body of [`publish_cell`]. Same max-monotonic cell
    /// semantics; no `.await`, no I/O — callable from the synchronous
    /// PUBLISH half of the publication split.
    /// `pub(in crate::mvcc_store)` so sibling modules of `write_receipt`
    /// (e.g. `mvcc_history`) can call it, matching the old scope.
    pub(in crate::mvcc_store) fn publish_cell_sync(&self, key: RecordKey, version: u64) {
        match self.cells.entry_sync(key) {
            scc::hash_map::Entry::Occupied(mut e) => {
                if version > e.get().version {
                    e.get_mut().version = version;
                }
            }
            scc::hash_map::Entry::Vacant(e) => {
                e.insert_entry(RecordCell {
                    version,
                    vacuum_anchor: 0,
                    reserved_by: 0,
                });
            }
        }
    }
}
