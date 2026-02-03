# Index Engine Implementation Milestones

## Overview

Step-by-step implementation plan for the index engine. Each milestone should be completed and tested before moving to the next.

**🔄 Updated Approach (2025-02-03):**

We've pivoted from the original journal-based async indexing plan to a **synchronous Table API approach** for index management and unique constraints. This provides immediate value while we defer the complex async indexing infrastructure.

### What's Implemented Now ✅

- **Milestone 1**: Basic types (`IndexDef`, `IndexTarget`, `OpType`, `IndexChange`, `IndexOp`)
- **Index Management API**: `add_index()`, `add_unique_index()`, `remove_index()`, `enable_indexing_all()`, `disable_indexing()`
- **Unique Constraints**: Validation on insert/update/set with duplicate detection
- **Path Extraction**: `extract_value()` for nested Map access via interned IDs
- **Persistence**: IndexTarget persists across restarts in `__info__{table}`

### Deferred to Later ⏳

The full journal-based async indexing (Milestones 2-10, 12) is deferred until we need actual query acceleration. The current implementation focuses on:
1. Index configuration management
2. Unique constraint enforcement
3. Foundation for future query optimization

---

## Milestone 1: Basic Types ✅ Done

**File:** `src/db/engine/index/types.rs`

### Tasks
- [x] Create module file
- [x] Define `OpType` enum (Insert/Update/Delete)
- [x] Define `IndexChange` struct
  - path: Vec<u64>
  - value_type: u8
  - hash1: u64
  - hash2: u64
- [x] Define `IndexOp` struct
  - seq_no: u64
  - timestamp: u64
  - op_type: OpType
  - record_id: [u8; 16]
  - changes: Vec<IndexChange>
- [x] Define `IndexDef` struct
  - path: Vec<u64>
  - unique: bool
- [x] Define `IndexTarget` enum
  - Disabled (no indexing)
  - All (index all Map fields, non-unique)
  - Selective(Vec<IndexDef>) (specific indexes)
- [x] Add derives: Debug, Clone, Serialize, Deserialize
- [x] Add comprehensive tests (21 tests, all passing)

### Acceptance Criteria
- ✅ Code compiles
- ✅ Types can be serialized/deserialized with bincode
- ✅ Basic unit tests for type creation
- ✅ IndexTarget supports three-state indexing
- ✅ IndexDef supports unique/non-unique indexes

### Completed: 2025-02-03

---

## Milestone 2: Index Management API ✅ Done

**File:** `src/db/engine/table.rs`

### Tasks
- [x] Add `index_target: Arc<RwLock<IndexTarget>>` to Table struct
- [x] Implement `add_index(&self, path: &[&str]) -> DbResult<()>`
  - Parse path to interned components via interner
  - Add to IndexTarget
  - Persist to info_store
- [x] Implement `add_unique_index(&self, path: &[&str]) -> DbResult<()>`
  - Parse path to interned components
  - **Validate existing data** (scan all records, check for duplicates)
  - Add to IndexTarget if validation passes
  - Persist to info_store
- [x] Implement `remove_index(&self, path: &[&str]) -> DbResult<bool>`
  - Parse path to interned components
  - Remove from IndexTarget
  - Delete from storage if becomes Disabled
- [x] Implement `enable_indexing_all(&self) -> DbResult<()>`
  - Set IndexTarget::All
  - Persist to info_store
- [x] Implement `disable_indexing(&self) -> DbResult<()>`
  - Set IndexTarget::Disabled
  - Delete from storage
- [x] Add helper: `get_index_target(&self) -> IndexTarget` (for testing)
- [x] Add `DbError::DuplicateKey` variant

### Acceptance Criteria
- ✅ Can add/remove/list/disable indexes
- ✅ Unique index creation validates existing data
- ✅ Index paths persist across restarts
- ✅ Three states work correctly (Disabled/All/Selective)
- ✅ Thread-safe (RwLock protection)

### Tests Added (15 tests, all passing)
- `test_add_index` - Add simple index
- `test_add_nested_index` - Add nested path index
- `test_add_unique_index` - Add unique index on empty table
- `test_add_unique_index_with_duplicates` - Fail when duplicates exist
- `test_remove_index` - Remove existing index
- `test_remove_nonexistent_index` - Handle non-existent index
- `test_enable_indexing_all` - Enable full indexing
- `test_disable_indexing` - Disable indexing
- `test_unique_constraint_on_insert` - Enforce uniqueness on insert
- `test_unique_constraint_on_update` - Enforce uniqueness on update
- `test_unique_constraint_on_set` - Enforce uniqueness on set
- `test_unique_constraint_allows_null` - Null values allowed
- `test_multiple_indexes` - Multiple indexes on same table
- `test_index_target_persistence` - Persist across restarts
- `test_update_same_value_succeeds` - Self-update works

### Completed: 2025-02-03

---

## Milestone 3: Unique Constraint Enforcement ✅ Done

**File:** `src/db/engine/table.rs`

### Tasks
- [x] Implement `check_unique_constraints()` for insert
- [x] Implement `check_unique_constraints_exclude()` for update/set
- [x] Implement `extract_value()` for path-based value extraction
  - Supports nested Maps: `["user", "profile", "age"]`
  - Uses interned IDs for path components
  - Returns None for missing paths (null values)
- [x] Implement `validate_unique_index()` for index creation
  - Streams all records (memory-efficient)
  - Checks for duplicate values
  - Returns `DbError::DuplicateKey` if found
- [x] Pin streams with `pin_mut!` for async iteration
- [x] Add comprehensive tests for all scenarios

### Acceptance Criteria
- ✅ Insert validates all unique indexes
- ✅ Update validates excluding current record
- ✅ Set validates for both create and update
- ✅ Unique index creation validates existing data
- ✅ Null values allowed in unique indexes
- ✅ Memory-efficient (uses streaming, not full table load)

### Completed: 2025-02-03

---

## Deferred Milestones (Original Plan)

The following milestones are **deferred** until we need actual query-by-index functionality. The types and structures are already defined in `types.rs` for future use.

### Milestone 4: Key Encoding/Decoding ⏸️ Deferred

**File:** `src/db/engine/index/encoding.rs` (not yet created)

### Tasks
- [ ] Implement `encode_index_key(path, type, hash1, hash2) -> Bytes`
- [ ] Implement `decode_index_key(key) -> Result<(Vec<u64>, u8, u64, u64)>`
- [ ] Add unit tests for roundtrip encoding

**Why Deferred:** Not needed until we implement actual index storage and querying.

---

### Milestone 5: Hash Computation ⏸️ Deferred

**File:** `src/db/engine/index/hash.rs` (not yet created)

### Tasks
- [ ] Implement `compute_hash(value: &InnerValue) -> Option<(u8, u64, u64)>`
- [ ] Support all types with xxhash64 + fnvhash64
- [ ] Add tests for determinism and collision resistance

**Why Deferred:** Hash computation only needed when we build actual index data structures.

---

### Milestone 6: Table Journal ⏸️ Deferred

**File:** `src/db/engine/index/journal.rs` (not yet created)

### Tasks
- [ ] Define `TableJournal` struct
- [ ] Implement `append()`, `read()`, `next_seq()`
- [ ] Add tests for persistence and concurrency

**Why Deferred:** Journal-based async indexing is complex. Current synchronous validation is sufficient for now.

---

### Milestone 7-10: Index Store & Global Indexer ⏸️ Deferred

**Files:**
- `src/db/engine/index/store.rs`
- `src/db/engine/index/position.rs`
- `src/db/engine/indexer.rs` (in `db/engine/`)

**Tasks**
- [ ] Index store operations (add/remove/find)
- [ ] Position tracking for indexer
- [ ] Global indexer thread
- [ ] Async index updates

**Why Deferred:** Full async indexing infrastructure is not needed until we:
1. Have large datasets requiring index-based query acceleration
2. Need non-blocking writes
3. Want to separate index maintenance from write path

---

### Milestone 11: Query API ⏸️ Deferred

**File:** `src/db/engine/index/query.rs` (not yet created)

### Tasks
- [ ] Define `QueryBuilder` struct
- [ ] Define `Filter` enum (Eq, In, etc.)
- [ ] Implement `eq()`, `find()` methods
- [ ] Add intersection for multiple filters

**Why Deferred:**
- Current table scans are fast enough for small/medium datasets
- No query-by-index requirement yet
- Index management is the priority

---

## Future Enhancements

### High Priority
- [ ] **Background index creation**: Create unique indexes without blocking
- [ ] **Index rebuild on corruption**: Recovery mechanism
- [ ] **Performance benchmarks**: Measure index overhead

### Medium Priority
- [ ] **Hash computation**: For index keys
- [ ] **Index storage**: Actual index data structures
- [ ] **Query API**: Index-based lookups

### Low Priority
- [ ] **Journal-based async indexing**: Non-blocking index updates
- [ ] **Global indexer thread**: Background processing
- [ ] **Range indexes**: For inequality queries
- [ ] **Full-text search**: Inverted indexes for text fields

---

## Progress Tracking

| Milestone | Status | Completed Date | Notes |
|-----------|--------|----------------|-------|
| 1. Basic Types | ✅ Done | 2025-02-03 | All types defined, 21 tests passing |
| 2. Index Management API | ✅ Done | 2025-02-03 | Full CRUD for indexes, 15 tests |
| 3. Unique Constraints | ✅ Done | 2025-02-03 | Validation on insert/update/set |
| 4. Key Encoding | ⏸️ Deferred | - | Not needed yet |
| 5. Hash Computation | ⏸️ Deferred | - | Not needed yet |
| 6. Table Journal | ⏸️ Deferred | - | Complex, not needed yet |
| 7-10. Index Store & Indexer | ⏸️ Deferred | - | Not needed until query phase |
| 11. Query API | ⏸️ Deferred | - | Table scans sufficient for now |

---

## Architecture Notes

### Current Approach (2025-02-03)

**Synchronous Index Management:**
- Index configuration stored in `__info__{table}` as `IndexTarget`
- Path components stored as interned u64 IDs
- Unique constraints validated synchronously on write
- Thread-safe via `RwLock<IndexTarget>`
- Memory-efficient (streaming validation, not full table scans)

**Three-State Indexing:**
```rust
pub enum IndexTarget {
    Disabled,                    // No indexing, no record in storage
    All,                         // Index all Map fields (non-unique)
    Selective(Vec<IndexDef>),   // Specific indexes with unique flags
}
```

**Unique Index Validation:**
1. On `add_unique_index()`: Scan all existing data for duplicates
2. On `insert()`: Check all unique indexes before inserting
3. On `update()`: Check excluding the record being updated
4. On `set()`: Check for create, exclude for update

**Storage Format:**
```
RecordId::system("index_target") -> IndexTarget (bincode serialized)
```

### Original Async Design (Deferred)

The `index_engine.md` document describes a more complex async journal-based architecture that we're not implementing yet. Key differences:

| Original Plan | Current Implementation |
|--------------|------------------------|
| Async journal writes | Synchronous validation |
| Global indexer thread | Validation on write path |
| Index store with Vec<RecordId> | No index storage yet |
| Non-blocking writes | Blocking unique checks |
| Query acceleration | Unique constraints only |

---

## Testing Status

**Total Tests:** 136 (all passing except 1 flaky interner test)

**Index-Related Tests:**
- `types.rs`: 21 tests ✅
- Index management: 15 tests ✅
- Table tests: All passing ✅

**Test Coverage:**
- ✅ IndexDef creation and serialization
- ✅ IndexTarget state transitions
- ✅ Add/remove/enable/disable operations
- ✅ Unique constraint enforcement
- ✅ Path extraction for nested Maps
- ✅ Persistence across restarts
- ✅ Null value handling
- ✅ Self-update scenarios

---

## Next Steps

When we need query acceleration, implement in order:
1. **Hash computation** (Milestone 5) - For index keys
2. **Key encoding** (Milestone 4) - For index storage format
3. **Index store** (Milestone 8) - Actual index data structure
4. **Query API** (Milestone 12) - Index-based lookups

Only then consider:
5. **Journal** (Milestone 6) - For async updates
6. **Global indexer** (Milestone 10) - Background processing

The current implementation provides a solid foundation for unique constraints and index configuration management.
