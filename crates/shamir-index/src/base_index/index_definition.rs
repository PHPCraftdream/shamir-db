use super::index_info_item::IndexInfoItem;
use crate::state::IndexState;
use serde::{Deserialize, Serialize};

/// Defines a single index, which can be simple (one path) or composite (multiple paths).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexDefinition {
    /// Interned ID of the index name (used in IndexRecordKey)
    pub name_interned: u64,

    /// The list of paths that make up this index.
    /// A single path creates a simple index. Multiple paths create a composite index.
    pub paths: Vec<IndexInfoItem>,

    /// F-72 (#899, P0): planner-visibility lifecycle state, reusing
    /// `shamir_index::state::IndexState` (the same two-variant type index2's
    /// registry uses) rather than a parallel enum. A freshly-registered
    /// definition starts `Building` (set explicitly by the CREATE INDEX call
    /// site) and is flipped to `Ready` once its backfill fully completes —
    /// see `IndexManager::create_index_from_records`. `Ready` is the
    /// `#[default]`/`#[serde(default)]` value so every freshly-constructed
    /// definition (and, via `IndexInfo`'s own decode fallback — see
    /// `index_info.rs`'s module doc — every PRE-`state` on-disk definition)
    /// is `Ready` unless a CREATE explicitly marks it `Building`.
    ///
    /// bincode forward-compat NOTE: `#[serde(default)]` on this NEW trailing
    /// field does NOT by itself rescue a read of pre-`state` on-disk bytes
    /// (see `shamir_index::state`'s module doc for the proven bincode
    /// landmine) — `IndexInfo`'s custom `Deserialize` impl provides the real
    /// fallback (try current shape, then a pre-`state` shadow shape lifted to
    /// `Ready`), mirroring `persistence::load_index2_metadata`.
    #[serde(default)]
    pub state: IndexState,
}

impl IndexDefinition {
    pub fn new(name_interned: u64, paths: Vec<IndexInfoItem>) -> Self {
        Self {
            name_interned,
            paths,
            state: IndexState::default(),
        }
    }
}
