use super::index_info_item::IndexInfoItem;
use crate::state::IndexState;
use serde::{Deserialize, Serialize};
use shamir_tx::{IndexFamily, Provenance};

/// Defines a single index, which can be simple (one path) or composite (multiple paths).
///
/// `PartialEq`/`Eq` are hand-written (NOT derived) so that `instance_epoch`
/// (in-memory-only identity, see that field's doc) does NOT participate in
/// equality — two definitions that are otherwise identical but were minted
/// at different times (e.g. a bincode round-trip, which mints a fresh epoch
/// on decode) must still compare equal, matching every pre-existing
/// round-trip equality test (`index_definition_tests.rs`) and the general
/// expectation that equality reflects PERSISTED content, not process-local
/// bookkeeping.
#[derive(Debug, Clone, Serialize, Deserialize)]
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

    /// R0-B (#1008): in-memory-only instance identity, minted fresh on every
    /// CREATE and bumped on every RENAME (mirrors
    /// `SortedIndexDefinition::instance_epoch` — see that field's doc for the
    /// full rationale, and `index_write_op.rs::Provenance` for how the
    /// commit-time reconcile in `shamir-engine::tx::pre_commit` uses it to
    /// distinguish a DROP+CREATE-same-name (ABA) from the SAME live
    /// instance). `#[serde(skip)]` — mirrors the existing
    /// `SortedIndexDefinition::included_fields_interned` precedent: never
    /// persisted, only needs to survive within one process's uptime.
    /// `default = "next_instance_epoch"` mints a FRESH value on every
    /// deserialize path too (derive-based `Deserialize`, including the
    /// `IndexDefinitionNoState`/legacy-shape `From` conversions in
    /// `index_info.rs`), so a definition reloaded from disk on restart never
    /// collides with a stale in-memory epoch from before the restart.
    #[serde(skip, default = "next_instance_epoch")]
    pub instance_epoch: u64,
}

/// R0-B (#1008): process-wide monotonic counter minting fresh
/// [`IndexDefinition::instance_epoch`] values. A single global counter
/// (rather than per-manager) guarantees no two definitions — regular or
/// unique, on the same table or a different one — ever collide on epoch,
/// which is the only property [`Provenance`](crate::write_ops::IndexWriteOp)
/// matching needs. Starts at 1 so `0` stays free as an unambiguous
/// "no epoch assigned yet" sentinel (never actually observed in practice,
/// since every construction path — `new()` and every `Deserialize` path —
/// mints a fresh value immediately).
static NEXT_INSTANCE_EPOCH: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// Mint a fresh, process-wide-unique instance epoch. See
/// [`NEXT_INSTANCE_EPOCH`]'s doc.
pub(crate) fn next_instance_epoch() -> u64 {
    NEXT_INSTANCE_EPOCH.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

impl IndexDefinition {
    pub fn new(name_interned: u64, paths: Vec<IndexInfoItem>) -> Self {
        Self {
            name_interned,
            paths,
            state: IndexState::default(),
            instance_epoch: next_instance_epoch(),
        }
    }

    /// R0-B (#1008): build the [`Provenance`] every `IndexWriteOp` planned
    /// against this definition must carry, tagged with the caller's family
    /// (`IndexDefinition` backs BOTH the regular and unique base_index
    /// families — the caller, which already knows which `IndexManager`
    /// collection it iterated (`self.indexes` vs `self.indexes_unique`),
    /// supplies the tag).
    pub fn provenance(&self, family: IndexFamily) -> Provenance {
        Provenance {
            family,
            name_interned: self.name_interned,
            instance_epoch: self.instance_epoch,
        }
    }
}

impl PartialEq for IndexDefinition {
    fn eq(&self, other: &Self) -> bool {
        // Deliberately excludes `instance_epoch` — see the struct doc.
        self.name_interned == other.name_interned
            && self.paths == other.paths
            && self.state == other.state
    }
}

impl Eq for IndexDefinition {}
