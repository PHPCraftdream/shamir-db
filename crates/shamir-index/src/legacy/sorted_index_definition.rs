//! Definition of a single sorted (B-tree-by-value) index.
//!
//! The physical key layout written by `SortedIndexManager` is:
//!
//! ```text
//!   physical_key  = SORTED_TAG (1 byte)
//!                 ||  name_interned (8 bytes BE)
//!                 ||  encoded_value (variable)
//!                 ||  record_id (16 bytes)
//!   physical_value = empty Bytes  (or a versioned projection envelope
//!                     for covering indexes)
//! ```
//!
//! `SORTED_TAG` is chosen to be distinct from the hash-index tag so
//! the two indexes never collide in the same info_store. Within one
//! `name_interned`, all entries share that prefix, so a prefix scan
//! returns every record matching this index in **value order**.

use serde::{Deserialize, Serialize};

/// Distinguishes sorted-index physical keys from any other key kind
/// that lives in the same info_store. Must NOT collide with
/// `IndexRecordKey::TAG` (see index_record_key.rs) or any system
/// RecordId byte pattern. RecordId::system uses a 4-byte zero prefix
/// followed by name bytes — first byte is 0x00. Hash-index keys
/// start with the unique flag (0 or 1). So 0x80 is a safe pick.
pub(crate) const SORTED_TAG: u8 = 0x80;

/// Definition of a sorted index — minimal, since we only support
/// single-field for now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SortedIndexDefinition {
    /// Interned id of the index name.
    pub name_interned: u64,
    /// Single field path, expressed as interner keys (matches the
    /// regular `IndexInfoItem::path`).
    pub field_path: Vec<u64>,
    /// Covering index: extra field paths (as raw string segments) whose
    /// values are projected into the index entry's physical_value.
    /// Persisted so the metadata survives restarts.
    #[serde(default)]
    pub included_fields: Vec<Vec<String>>,
    /// Pre-interned form of `included_fields` — transient, not
    /// persisted. Populated at registration time (see
    /// `SortedIndexManager::intern_included_paths`) or rebuilt after
    /// load from disk. Empty means "no covering projection".
    #[serde(skip)]
    pub included_fields_interned: Vec<Vec<u64>>,
    /// F-71 (#898): durable floor for the AsOf cursor-seek gate
    /// (`SortedIndexManager::last_mutation_version`) — the MVCC version up
    /// to which this index's postings are KNOWN to mirror the table's
    /// content, set once at the end of a successful backfill (see
    /// `SortedIndexManager::mark_ready_at`) to the table's
    /// `last_committed_version` at that moment (**never** `0`, even for an
    /// index whose backfill observed no rows — an empty table is still
    /// "ready" as of the current version, not as of the dawn of time).
    ///
    /// Persisted (unlike `included_fields_interned`) so a restart restores
    /// the EXACT epoch instead of falling back to a lower, merely-safe
    /// floor: `SortedIndexManager::load()` seeds the in-memory
    /// `last_mutation_version` high-water from this field for every loaded
    /// definition, closing the F-67 regression where a fresh manager read
    /// every index's epoch as `0` after ANY restart (see that task's
    /// brief). `#[serde(default)]` keeps loading pre-F-71 persisted blobs
    /// backward-compatible: a definition written before this field existed
    /// decodes with `ready_at_version == 0`, the same (safe, if permissive)
    /// default the epoch already had before this fix.
    #[serde(default)]
    pub ready_at_version: u64,
}

impl SortedIndexDefinition {
    pub fn new(name_interned: u64, field_path: Vec<u64>) -> Self {
        Self {
            name_interned,
            field_path,
            included_fields: Vec::new(),
            included_fields_interned: Vec::new(),
            ready_at_version: 0,
        }
    }

    /// Construct with covering-index included field paths (string form only;
    /// call `SortedIndexManager::intern_included_paths` or use
    /// `with_included_interned` to populate the interned form).
    pub fn with_included(
        name_interned: u64,
        field_path: Vec<u64>,
        included_fields: Vec<Vec<String>>,
    ) -> Self {
        Self {
            name_interned,
            field_path,
            included_fields,
            included_fields_interned: Vec::new(),
            ready_at_version: 0,
        }
    }

    /// Construct with covering-index included field paths, providing
    /// both the string and pre-interned forms.
    pub fn with_included_interned(
        name_interned: u64,
        field_path: Vec<u64>,
        included_fields: Vec<Vec<String>>,
        included_fields_interned: Vec<Vec<u64>>,
    ) -> Self {
        Self {
            name_interned,
            field_path,
            included_fields,
            included_fields_interned,
            ready_at_version: 0,
        }
    }

    /// True if this is a covering index (has included fields).
    pub fn is_covering(&self) -> bool {
        !self.included_fields_interned.is_empty()
    }
}

/// Legacy on-disk layout without `included_fields`. Used only during
/// backward-compatible load of pre-covering-index persisted data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SortedIndexDefinitionV1 {
    pub(crate) name_interned: u64,
    pub(crate) field_path: Vec<u64>,
}

impl From<SortedIndexDefinitionV1> for SortedIndexDefinition {
    fn from(v1: SortedIndexDefinitionV1) -> Self {
        Self {
            name_interned: v1.name_interned,
            field_path: v1.field_path,
            included_fields: Vec::new(),
            included_fields_interned: Vec::new(),
            // Pre-F-71 on-disk layout carries no epoch — decodes as the same
            // safe-if-permissive `0` default `#[serde(default)]` gives the
            // current V2 format when the field is absent.
            ready_at_version: 0,
        }
    }
}
