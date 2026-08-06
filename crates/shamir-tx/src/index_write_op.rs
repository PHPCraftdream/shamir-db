//! Pure-data index write operation enum.
//!
//! Moved here from `shamir-engine::index2::write_ops` so that
//! `TxContext` (which lives in `shamir-tx`) can reference it without
//! a circular dependency. `shamir-engine` re-exports it via `pub use`.

use bytes::Bytes;

/// R0-B (#1008): which index family a [`Provenance`] describes.
///
/// `shamir-tx` cannot depend on `shamir-index` (see this module's doc — the
/// dependency runs the other way), so this is a small local tag rather than
/// a re-export of `shamir_index::kind::IndexKind`. It only needs to
/// distinguish the four posting-key namespaces the commit-time reconcile
/// logic in `shamir-engine::tx::pre_commit` cares about — it is NOT a
/// general-purpose index-kind descriptor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexFamily {
    /// base_index regular (non-unique) hash index.
    Regular,
    /// base_index unique hash index.
    Unique,
    /// base_index sorted (B-tree-by-value) index.
    Sorted,
    /// index2 (FTS / functional / vector) backend.
    Index2,
}

/// R0-B (#1008): identity of the index INSTANCE a [`IndexWriteOp::SetPosting`]/
/// [`IndexWriteOp::RemovePosting`] was planned against, at the moment it was
/// planned.
///
/// This replaces the pre-R0-B byte-length/name heuristic
/// (`pre_commit.rs`'s old `(is_unique, name_interned)` 2c-retain filter),
/// which could not distinguish two DIFFERENT definitions that happen to
/// share a `name_interned` (a DROP followed by a CREATE of the same name —
/// ABA). `instance_epoch` is the tiebreaker: every definition mints a FRESH
/// epoch on CREATE and bumps it on RENAME (see
/// `shamir_index::base_index::index_definition::IndexDefinition::instance_epoch`
/// and `..::sorted_index_definition::SortedIndexDefinition::instance_epoch`
/// for base_index/sorted; index2 reuses the ALREADY-correct
/// `IndexRegistry`'s per-entry `BackendEntry.gen`, R0-A/#1006 — see that
/// field's doc). A staged op's `(name_interned, instance_epoch)` pair is
/// compared against the CURRENT live definition's pair at commit time: a
/// match means "still the same instance, keep the op"; no match means "this
/// instance no longer exists (dropped) or was replaced (ABA / renamed) —
/// retract the op".
///
/// In-memory only — never serialized (see this crate's module doc: this
/// whole enum is `Debug, Clone`, not `Serialize`/`Deserialize`, and never
/// touches the WAL wire format; `WalOpV2::IndexPut`/`IndexDel` in
/// `shamir-wal` carry their own, currently-unpopulated `idx_id: u32` field
/// for a FUTURE wire-level identity scheme — out of scope here).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Provenance {
    pub family: IndexFamily,
    pub name_interned: u64,
    pub instance_epoch: u64,
}

impl Default for Provenance {
    /// A harmless placeholder — `IndexFamily::Regular`, `name_interned: 0`,
    /// `instance_epoch: 0`. Exists mainly so tests that construct an
    /// `IndexWriteOp` purely to exercise UNRELATED behavior (byte layout,
    /// WAL conversion, footprint sizing, etc. — not the R0-B reconcile
    /// logic itself) don't need to hand-build a `Provenance` just to satisfy
    /// the type checker. Production call sites always construct a REAL
    /// `Provenance` (see `IndexDefinition::provenance` /
    /// `SortedIndexDefinition::provenance` / `index2_provenance`) — never
    /// rely on this default outside tests.
    fn default() -> Self {
        Self {
            family: IndexFamily::Regular,
            name_interned: 0,
            instance_epoch: 0,
        }
    }
}

/// A single planned index mutation. Returned by `IndexBackend::plan_*`
/// methods; applied either immediately (non-tx) or accumulated in
/// `TxContext.index_write_set` for atomic commit (tx).
#[derive(Debug, Clone)]
pub enum IndexWriteOp {
    /// Insert or overwrite a posting in the index store.
    SetPosting {
        key: Bytes,
        value: Bytes,
        /// R0-B (#1008): identity of the index instance this posting was
        /// planned against — see [`Provenance`]'s doc. Used by
        /// `shamir-engine::tx::pre_commit`'s reconcile logic to retract a
        /// staged op whose instance no longer matches the current live
        /// definition (DROP, or DROP+CREATE-same-name/ABA, or RENAME).
        provenance: Provenance,
    },
    /// Delete a posting by key from the index store.
    RemovePosting {
        key: Bytes,
        /// See [`SetPosting::provenance`].
        provenance: Provenance,
    },
    /// Bump FtsRankedBackend's in-memory BM25 stats (doc_count +
    /// sum_doc_len). `sign` is +1 for insert, -1 for delete.
    /// Never persisted as a posting — applied via `apply_in_memory`.
    BumpFtsStats { doc_len: u32, sign: i8 },
}

impl IndexWriteOp {
    /// R0-B (#1008): overwrite this op's [`Provenance`] in place. A no-op for
    /// [`IndexWriteOp::BumpFtsStats`] (it carries no provenance — it is
    /// in-memory-only and never retracted by the commit-time reconcile).
    ///
    /// index2 backends (`FunctionalBackend`/`FtsBackend`/`FtsRankedBackend`/
    /// `VectorBackend`) construct their `IndexWriteOp`s with only a
    /// CONSTRUCTION-TIME `descriptor()` snapshot available — they have no
    /// access to `IndexRegistry`'s live per-entry `gen` (the actual instance
    /// epoch for index2, R0-A/#1006's `BackendEntry.gen`). Callers that DO
    /// have registry access (`TableManager::plan_insert_ops`/
    /// `plan_update_ops`/`plan_delete_ops` at stage time, and
    /// `rederive_index2_ops_post_stage` at commit time) call this
    /// immediately after a backend's `plan_*_tx` returns, to stamp the
    /// CORRECT live `instance_epoch` onto every op the backend produced —
    /// see those call sites for the exact wiring.
    pub fn set_provenance(&mut self, new_provenance: Provenance) {
        match self {
            IndexWriteOp::SetPosting { provenance, .. }
            | IndexWriteOp::RemovePosting { provenance, .. } => {
                *provenance = new_provenance;
            }
            IndexWriteOp::BumpFtsStats { .. } => {}
        }
    }
}
