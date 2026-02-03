//! Index operation types for journal-based asynchronous index updates
//!
//! # Current Status
//!
//! These types are defined for the future async journal-based indexing system.
//! Currently, only `IndexDef` and `IndexTarget` are actively used for:
//! - Index configuration management
//! - Unique constraint enforcement
//!
//! # Type Usage
//!
//! - **IndexDef**: Defines a single index with path and unique flag (✅ Active)
//! - **IndexTarget**: Three-state indexing configuration (✅ Active)
//! - **OpType**: Operation type enum (⏸️ For future journal)
//! - **IndexChange**: Single index change entry (⏸️ For future journal)
//! - **IndexOp**: Complete journal operation (⏸️ For future journal)
//!
//! See `index_engine.md` for the full architecture design.
//! See `milestones.md` for implementation status.

use crate::types::record_id::RecordId;
use serde::{Deserialize, Serialize};

/// Definition of a single index
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexDef {
    /// Path to indexed field (interned components)
    pub path: Vec<u64>,

    /// Is this a unique index?
    pub unique: bool,
}

impl IndexDef {
    /// Create a non-unique index
    pub fn new(path: Vec<u64>) -> Self {
        Self { path, unique: false }
    }

    /// Create a unique index
    pub fn unique(path: Vec<u64>) -> Self {
        Self { path, unique: true }
    }

    /// Check if this is a unique index
    pub fn is_unique(&self) -> bool {
        self.unique
    }
}

/// Indexing target specification
///
/// Defines what should be indexed for a table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum IndexTarget {
    /// Indexing disabled - no indexing, journal not written
    Disabled,

    /// Index everything - all Map fields are indexed (non-unique)
    All,

    /// Selective indexing - only specific indexes are active
    Selective(Vec<IndexDef>),
}

impl IndexTarget {
    /// Create Disabled target
    pub fn disabled() -> Self {
        IndexTarget::Disabled
    }

    /// Create All target
    pub fn all() -> Self {
        IndexTarget::All
    }

    /// Create Selective target with indexes
    pub fn selective(indexes: Vec<IndexDef>) -> Self {
        IndexTarget::Selective(indexes)
    }

    /// Check if indexing is enabled
    pub fn is_enabled(&self) -> bool {
        !matches!(self, IndexTarget::Disabled)
    }

    /// Check if all fields should be indexed
    pub fn is_all(&self) -> bool {
        matches!(self, IndexTarget::All)
    }

    /// Check if selective indexing is enabled
    pub fn is_selective(&self) -> bool {
        matches!(self, IndexTarget::Selective(_))
    }

    /// Add an index to selective mode
    /// If disabled, switches to Selective with single index
    /// If all, switches to Selective with single index
    pub fn add_index(&mut self, path: Vec<u64>, unique: bool) {
        match self {
            IndexTarget::Disabled => {
                *self = IndexTarget::Selective(vec![IndexDef { path, unique }]);
            }
            IndexTarget::All => {
                *self = IndexTarget::Selective(vec![IndexDef { path, unique }]);
            }
            IndexTarget::Selective(indexes) => {
                // Remove existing index with same path (if any)
                indexes.retain(|idx| idx.path != path);
                indexes.push(IndexDef { path, unique });
            }
        }
    }

    /// Remove an index from selective mode
    /// Returns true if index was removed, false if not found
    /// If indexes become empty, switches to Disabled
    pub fn remove_index(&mut self, path: &Vec<u64>) -> bool {
        let (should_disable, removed) = match self {
            IndexTarget::Disabled | IndexTarget::All => (false, false),
            IndexTarget::Selective(indexes) => {
                let _initial_len = indexes.len();
                let was_present = indexes.iter().any(|idx| idx.path == *path);

                indexes.retain(|idx| idx.path != *path);

                (indexes.is_empty(), was_present)
            }
        };

        if should_disable {
            *self = IndexTarget::Disabled;
        }

        removed
    }

    /// Get all indexes if selective, None otherwise
    pub fn indexes(&self) -> Option<&[IndexDef]> {
        match self {
            IndexTarget::Selective(indexes) => Some(indexes),
            _ => None,
        }
    }

    /// Get only unique indexes
    pub fn unique_indexes(&self) -> Vec<IndexDef> {
        match self {
            IndexTarget::Selective(indexes) => {
                indexes.iter().filter(|idx| idx.unique).cloned().collect()
            }
            _ => Vec::new(),
        }
    }

    /// Check if a specific path is indexed (regardless of uniqueness)
    pub fn has_index(&self, path: &Vec<u64>) -> bool {
        match self {
            IndexTarget::All => true,  // Everything is indexed
            IndexTarget::Disabled => false,
            IndexTarget::Selective(indexes) => {
                indexes.iter().any(|idx| idx.path == *path)
            }
        }
    }

    /// Check if a specific path has a unique index
    pub fn has_unique_index(&self, path: &Vec<u64>) -> bool {
        match self {
            IndexTarget::All => false,  // All is non-unique
            IndexTarget::Disabled => false,
            IndexTarget::Selective(indexes) => {
                indexes.iter().any(|idx| idx.path == *path && idx.unique)
            }
        }
    }
}

/// Operation type for index changes
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum OpType {
    /// Insert a new record
    Insert = 0,
    /// Update an existing record
    Update = 1,
    /// Delete a record
    Delete = 2,
}

/// A single index change entry
///
/// Represents one indexed path that changed during an operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexChange {
    /// Interned path components (e.g., ["user", "profile", "age"])
    pub path: Vec<u64>,

    /// Value type discriminator
    /// 0x00=Null, 0x01=Int, 0x02=UInt, 0x03=Float, 0x04=Bool, 0x05=Str,
    /// 0x06=Bin, 0x07=Array, 0x08=Map, 0x09=Set, 0x0A=Decimal, 0x0B=BigInt
    pub value_type: u8,

    /// First hash (xxhash64)
    pub hash1: u64,

    /// Second hash (fnvhash64)
    pub hash2: u64,
}

/// Complete index operation for journaling
///
/// Represents all index changes for a single table operation (insert/update/delete).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexOp {
    /// Sequence number in journal
    pub seq_no: u64,

    /// Operation timestamp (unix epoch)
    pub timestamp: u64,

    /// Operation type
    pub op_type: OpType,

    /// Record ID being operated on (stored as raw bytes)
    pub record_id: [u8; 16],

    /// All index changes for this operation
    pub changes: Vec<IndexChange>,
}

impl IndexOp {
    /// Get the RecordId from raw bytes
    pub fn get_record_id(&self) -> RecordId {
        RecordId(self.record_id)
    }

    /// Create IndexOp with RecordId
    pub fn with_record_id(mut self, id: RecordId) -> Self {
        self.record_id = id.as_bytes().clone();
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_op_type_values() {
        assert_eq!(OpType::Insert as u8, 0);
        assert_eq!(OpType::Update as u8, 1);
        assert_eq!(OpType::Delete as u8, 2);
    }

    #[test]
    fn test_index_change_creation() {
        let change = IndexChange {
            path: vec![42, 78, 156],
            value_type: 0x01,
            hash1: 0x123456789ABCDEF0,
            hash2: 0xFEDCBA9876543210,
        };

        assert_eq!(change.path.len(), 3);
        assert_eq!(change.path[0], 42);
        assert_eq!(change.value_type, 0x01);
        assert_eq!(change.hash1, 0x123456789ABCDEF0);
        assert_eq!(change.hash2, 0xFEDCBA9876543210);
    }

    #[test]
    fn test_index_op_creation() {
        let op = IndexOp {
            seq_no: 1,
            timestamp: 1234567890,
            op_type: OpType::Insert,
            record_id: [0; 16],
            changes: vec![],
        };

        assert_eq!(op.seq_no, 1);
        assert_eq!(op.timestamp, 1234567890);
        assert_eq!(op.op_type, OpType::Insert);
        assert_eq!(op.changes.len(), 0);
    }

    #[test]
    fn test_index_op_serialization() {
        let original = IndexOp {
            seq_no: 42,
            timestamp: 9876543210,
            op_type: OpType::Update,
            record_id: [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16],
            changes: vec![
                IndexChange {
                    path: vec![100, 200],
                    value_type: 0x05,
                    hash1: 111,
                    hash2: 222,
                },
            ],
        };

        // Serialize
        let serialized = bincode::serialize(&original).unwrap();

        // Deserialize
        let deserialized: IndexOp = bincode::deserialize(&serialized).unwrap();

        assert_eq!(deserialized.seq_no, 42);
        assert_eq!(deserialized.timestamp, 9876543210);
        assert_eq!(deserialized.op_type, OpType::Update);
        assert_eq!(deserialized.record_id, [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]);
        assert_eq!(deserialized.changes.len(), 1);
        assert_eq!(deserialized.changes[0].path, vec![100, 200]);
        assert_eq!(deserialized.changes[0].value_type, 0x05);
        assert_eq!(deserialized.changes[0].hash1, 111);
        assert_eq!(deserialized.changes[0].hash2, 222);
    }

    #[test]
    fn test_multiple_index_changes() {
        let op = IndexOp {
            seq_no: 1,
            timestamp: 0,
            op_type: OpType::Insert,
            record_id: [0; 16],
            changes: vec![
                IndexChange {
                    path: vec![1],
                    value_type: 0x01,
                    hash1: 100,
                    hash2: 200,
                },
                IndexChange {
                    path: vec![2, 3],
                    value_type: 0x05,
                    hash1: 300,
                    hash2: 400,
                },
            ],
        };

        assert_eq!(op.changes.len(), 2);
        assert_eq!(op.changes[0].path, vec![1]);
        assert_eq!(op.changes[1].path, vec![2, 3]);
    }

    // IndexTarget tests
    #[test]
    fn test_index_def() {
        let def1 = IndexDef::new(vec![1, 2]);
        assert_eq!(def1.path, vec![1, 2]);
        assert!(!def1.is_unique());

        let def2 = IndexDef::unique(vec![3, 4]);
        assert_eq!(def2.path, vec![3, 4]);
        assert!(def2.is_unique());
    }

    #[test]
    fn test_index_target_disabled() {
        let target = IndexTarget::Disabled;
        assert!(!target.is_enabled());
        assert!(!target.is_all());
        assert!(!target.is_selective());
        assert!(target.indexes().is_none());
        assert!(target.unique_indexes().is_empty());
        assert!(!target.has_index(&vec![1]));
        assert!(!target.has_unique_index(&vec![1]));
    }

    #[test]
    fn test_index_target_all() {
        let target = IndexTarget::All;
        assert!(target.is_enabled());
        assert!(target.is_all());
        assert!(!target.is_selective());
        assert!(target.indexes().is_none());
        assert!(target.unique_indexes().is_empty());
        assert!(target.has_index(&vec![1]));  // All means everything is indexed
        assert!(!target.has_unique_index(&vec![1]));  // But not unique
    }

    #[test]
    fn test_index_target_selective() {
        let indexes = vec![
            IndexDef::new(vec![1, 2]),
            IndexDef::unique(vec![3, 4]),
        ];
        let target = IndexTarget::selective(indexes.clone());

        assert!(target.is_enabled());
        assert!(!target.is_all());
        assert!(target.is_selective());
        assert_eq!(target.indexes(), Some(&indexes as &[IndexDef]));
        assert_eq!(target.unique_indexes().len(), 1);
        assert!(target.has_index(&vec![1, 2]));
        assert!(target.has_unique_index(&vec![3, 4]));
    }

    #[test]
    fn test_index_target_add_index_to_disabled() {
        let mut target = IndexTarget::Disabled;
        target.add_index(vec![1, 2], false);

        assert!(target.is_selective());
        let indexes = target.indexes().unwrap();
        assert_eq!(indexes.len(), 1);
        assert_eq!(indexes[0], IndexDef::new(vec![1, 2]));
    }

    #[test]
    fn test_index_target_add_unique_index_to_disabled() {
        let mut target = IndexTarget::Disabled;
        target.add_index(vec![1, 2], true);

        assert!(target.is_selective());
        assert_eq!(target.unique_indexes().len(), 1);
    }

    #[test]
    fn test_index_target_add_index_to_all() {
        let mut target = IndexTarget::All;
        target.add_index(vec![1, 2], false);

        assert!(target.is_selective());
        let indexes = target.indexes().unwrap();
        assert_eq!(indexes.len(), 1);
        assert_eq!(indexes[0], IndexDef { path: vec![1, 2], unique: false });
    }

    #[test]
    fn test_index_target_add_index_to_selective() {
        let mut target = IndexTarget::selective(vec![IndexDef::new(vec![1])]);
        target.add_index(vec![2, 3], true);

        let indexes = target.indexes().unwrap();
        assert_eq!(indexes.len(), 2);
        assert!(indexes.contains(&IndexDef::new(vec![1])));
        assert!(indexes.contains(&IndexDef::unique(vec![2, 3])));
    }

    #[test]
    fn test_index_target_remove_index_from_selective() {
        let mut target = IndexTarget::selective(vec![
            IndexDef::new(vec![1, 2]),
            IndexDef::unique(vec![3, 4]),
        ]);

        let removed = target.remove_index(&vec![1, 2]);
        assert!(removed);

        let indexes = target.indexes().unwrap();
        assert_eq!(indexes.len(), 1);
        assert_eq!(indexes[0], IndexDef::unique(vec![3, 4]));
    }

    #[test]
    fn test_index_target_remove_last_index_becomes_disabled() {
        let mut target = IndexTarget::selective(vec![IndexDef::new(vec![1, 2])]);

        let removed = target.remove_index(&vec![1, 2]);
        assert!(removed);

        assert!(matches!(target, IndexTarget::Disabled));
        assert!(!target.is_enabled());
    }

    #[test]
    fn test_index_target_remove_unique_index_from_selective() {
        let mut target = IndexTarget::selective(vec![
            IndexDef::new(vec![1, 2]),
            IndexDef::unique(vec![3, 4]),
        ]);

        let removed = target.remove_index(&vec![3, 4]);
        assert!(removed);

        let indexes = target.indexes().unwrap();
        assert_eq!(indexes.len(), 1);
        assert_eq!(indexes[0], IndexDef::new(vec![1, 2]));
        assert_eq!(target.unique_indexes().len(), 0);
    }

    #[test]
    fn test_index_target_remove_index_from_disabled() {
        let mut target = IndexTarget::Disabled;
        let removed = target.remove_index(&vec![1, 2]);

        assert!(!removed);
        assert!(matches!(target, IndexTarget::Disabled));
    }

    #[test]
    fn test_index_target_remove_index_from_all() {
        let mut target = IndexTarget::All;
        let removed = target.remove_index(&vec![1, 2]);

        assert!(!removed);
        assert!(matches!(target, IndexTarget::All));
    }

    #[test]
    fn test_index_def_serialization() {
        let def = IndexDef {
            path: vec![1, 2, 3],
            unique: true,
        };

        let serialized = bincode::serialize(&def).unwrap();
        let deserialized: IndexDef = bincode::deserialize(&serialized).unwrap();

        assert_eq!(deserialized, def);
    }

    #[test]
    fn test_index_target_serialization() {
        let targets = vec![
            IndexTarget::Disabled,
            IndexTarget::All,
            IndexTarget::selective(vec![
                IndexDef::new(vec![1, 2]),
                IndexDef::unique(vec![3]),
            ]),
        ];

        for original in targets {
            let serialized = bincode::serialize(&original).unwrap();
            let deserialized: IndexTarget = bincode::deserialize(&serialized).unwrap();
            assert_eq!(deserialized, original);
        }
    }

    #[test]
    fn test_index_target_has_unique_index() {
        let target = IndexTarget::selective(vec![
            IndexDef::new(vec![1]),
            IndexDef::unique(vec![2]),
        ]);

        assert!(!target.has_unique_index(&vec![1]));
        assert!(target.has_unique_index(&vec![2]));
        assert!(target.has_index(&vec![1]));  // Has non-unique index
        assert!(target.has_index(&vec![2]));  // Has unique index
    }
}
