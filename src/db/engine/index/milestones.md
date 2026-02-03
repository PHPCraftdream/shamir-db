# Index Engine Implementation Milestones

## Overview

Step-by-step implementation plan for the index engine. Each milestone should be completed and tested before moving to the next.

---

## Milestone 1: Basic Types

**File:** `src/db/engine/index/types.rs`

### Tasks
- [ ] Create module file
- [ ] Define `OpType` enum (Insert/Update/Delete)
- [ ] Define `IndexChange` struct
  - path: Vec<u64>
  - value_type: u8
  - hash1: u64
  - hash2: u64
- [ ] Define `IndexOp` struct
  - seq_no: u64
  - timestamp: u64
  - op_type: OpType
  - record_id: [u8; 16]
  - changes: Vec<IndexChange>
- [ ] Add derives: Debug, Clone, Serialize, Deserialize
- [ ] Add `#[cfg(test)]` module placeholder

### Acceptance Criteria
- ✅ Code compiles
- ✅ Types can be serialized/deserialized with bincode
- ✅ Basic unit tests for type creation

### Estimated Time: 30 minutes

---

## Milestone 2: Key Encoding/Decoding

**File:** `src/db/engine/index/encoding.rs`

### Tasks
- [ ] Implement `encode_index_key(path, type, hash1, hash2) -> Bytes`
  - Append path components (each u64 as 8 bytes big-endian)
  - Append type discriminator (1 byte)
  - Append hash1 (8 bytes big-endian)
  - Append hash2 (8 bytes big-endian)
- [ ] Implement `decode_index_key(key) -> Result<(Vec<u64>, u8, u64, u64)>`
  - Read last 17 bytes (tail)
  - Extract path from beginning
  - Validate path length % 8 == 0
  - Parse path components
- [ ] Add unit tests:
  - [ ] Encode/decode roundtrip
  - [ ] Single component path
  - [ ] Multi component path
  - [ ] All value types (0x00-0x0B)
  - [ ] Invalid key length (should error)

### Acceptance Criteria
- ✅ Encoding produces correct Bytes format
- ✅ Decoding recovers original values
- ✅ Invalid inputs produce errors
- ✅ Test coverage > 90%

### Estimated Time: 1 hour

---

## Milestone 3: Index Configuration

**File:** `src/db/engine/index/config.rs`

### Tasks
- [ ] Define `IndexDef` struct
  - path: Vec<u64>
  - path_str: String
  - unique: bool
  - created_at: u64
- [ ] Define `IndexConfig` struct
  - indexes: Vec<IndexDef>
- [ ] Implement `save_config(store, config) -> DbResult<()>`
  - Use `RecordId::system("index_config:{table}")` as key
  - Serialize with bincode
  - Write to info_store
- [ ] Implement `load_config(store) -> DbResult<IndexConfig>`
  - Read from info_store
  - Deserialize
  - Return empty config if not found
- [ ] Add tests:
  - [ ] Save and load roundtrip
  - [ ] Load non-existent config returns empty
  - [ ] Multiple indexes in config

### Acceptance Criteria
- ✅ Config persists across restarts
- ✅ Can add/remove indexes from config
- ✅ Empty config handled correctly

### Estimated Time: 45 minutes

---

## Milestone 4: Path Extraction

**File:** `src/db/engine/index/path.rs`

### Tasks
- [ ] Implement `extract_value(value: &InnerValue, path: &str) -> Option<&InnerValue>`
  - Parse path string: "a.b.c" -> ["a", "b", "c"]
  - Traverse InnerValue recursively
  - Support Map access by key
  - Support Array access by index (optional, mark as such)
- [ ] Add helper: `parse_path(path: &str) -> Vec<&str>`
- [ ] Add tests:
  - [ ] Simple Map access: "user.age"
  - [ ] Nested Map access: "user.profile.age"
  - [ ] Missing path returns None
  - [ ] Non-Map value returns None
  - [ ] Array access: "items.0" (if implemented)
  - [ ] Edge cases: empty path, "." etc

### Acceptance Criteria
- ✅ Can extract values from nested Maps
- ✅ Returns None for missing paths
- ✅ Handles edge cases gracefully
- ✅ Test coverage for all cases

### Estimated Time: 1.5 hours

---

## Milestone 5: Hash Computation

**File:** `src/db/engine/index/hash.rs`

### Tasks
- [ ] Implement `compute_hash(value: &InnerValue) -> Option<(u8, u64, u64)>`
  - For simple types (Int, UInt, Float, Bool, Str, Bin, Decimal, BigInt):
    - Determine type discriminator
    - Compute hash1 = xxhash64
    - Compute hash2 = fnvhash64
    - Return Some((type, h1, h2))
  - For Map:
    - Serialize with bincode
    - Hash serialized bytes
    - Return Some((0x08, h1, h2))
  - For Array, Set:
    - Return None (not indexed)
  - For Null:
    - Return Some((0x00, 0, 0))
- [ ] Add tests:
  - [ ] All simple types produce hashes
  - [ ] Map produces hash
  - [ ] Array/Set return None
  - [ ] Same value produces same hash (deterministic)
  - [ ] Different values produce different hashes (collision resistance)

### Acceptance Criteria
- ✅ All indexable types produce valid hashes
- ✅ Non-indexable types return None
- ✅ Hashes are deterministic
- ✅ Test coverage for all InnerValue variants

### Estimated Time: 1 hour

---

## Milestone 6: Table Journal

**File:** `src/db/engine/index/journal.rs`

### Tasks
- [ ] Define `TableJournal` struct
  - table_name: String
  - store: Arc<dyn Store>
  - seq_no: AtomicU64
- [ ] Implement `new(table_name, store) -> Self`
- [ ] Implement `append(&self, op: IndexOp) -> DbResult<u64>`
  - Fetch-add seq_no
  - Encode key as seq_no (big-endian)
  - Serialize op with bincode
  - Insert to store
  - Return seq_no
- [ ] Implement `read(&self, seq: u64) -> DbResult<Option<IndexOp>>`
  - Encode key
  - Try get from store
  - Deserialize if found
  - Return None if not found
- [ ] Implement `next_seq(&self) -> u64`
  - Return current seq_no
- [ ] Add tests:
  - [ ] Append and read roundtrip
  - [ ] Sequential seq_no generation
  - [ ] Read non-existent returns None
  - [ ] Concurrent append (seq_no uniqueness)

### Acceptance Criteria
- ✅ Operations persist correctly
- ✅ Seq_no increments atomically
- ✅ Can read back operations
- ✅ Thread-safe

### Estimated Time: 1 hour

---

## Milestone 7: Integration with Table (Basic)

**File:** `src/db/engine/table.rs`

### Tasks
- [ ] Add `journal: Arc<TableJournal>` to Table struct
- [ ] Update `Table::new()` to create journal
- [ ] Update `insert()` to write to journal
  - Extract indexed paths
  - Compute hashes
  - Create IndexOp with Insert
  - Append to journal
- [ ] Update `delete()` to write to journal
  - Extract indexed paths
  - Compute hashes
  - Create IndexOp with Delete
  - Append to journal
- [ ] Add helper: `collect_index_changes(value) -> Vec<IndexChange>`
- [ ] Add tests:
  - [ ] Insert creates journal entry
  - [ ] Delete creates journal entry
  - [ ] Journal persists across restarts

### Acceptance Criteria
- ✅ All table operations logged
- ✅ Journal entries contain correct changes
- ✅ No impact on existing functionality

### Estimated Time: 1.5 hours

---

## Milestone 8: Index Store Operations

**File:** `src/db/engine/index/store.rs`

### Tasks
- [ ] Define `IndexStore` struct
  - store: Arc<dyn Store>
- [ ] Implement `add(&self, key: Bytes, record_id: RecordId) -> DbResult<()>`
  - Get existing Vec<RecordId>
  - Push new record_id
  - Set back to store
- [ ] Implement `remove(&self, key: Bytes, record_id: RecordId) -> DbResult<bool>`
  - Get existing Vec<RecordId>
  - Remove record_id
  - Update or delete if empty
- [ ] Implement `find(&self, key: Bytes) -> DbResult<Vec<RecordId>>`
  - Get and deserialize
  - Return empty vec if not found
- [ ] Add tests:
  - [ ] Add and find roundtrip
  - [ ] Add multiple records to same key
  - [ ] Remove removes specific record
  - [ ] Remove last record deletes entry

### Acceptance Criteria
- ✅ Can add/remove/find records
- ✅ Multiple records per key work correctly
- ✅ Empty entries cleaned up

### Estimated Time: 1 hour

---

## Milestone 9: Indexer Position Tracking

**File:** `src/db/engine/index/position.rs`

### Tasks
- [ ] Implement `save_position(store, table, seq_no) -> DbResult<()>`
  - Key: `RecordId::system("indexer_pos:{table}")`
  - Value: seq_no (u64)
  - Write to info_store
- [ ] Implement `load_position(store, table) -> DbResult<u64>`
  - Read from info_store
  - Return 0 if not found
- [ ] Add tests:
  - [ ] Save and load roundtrip
  - [ ] Load non-existent returns 0
  - [ ] Concurrent updates

### Acceptance Criteria
- ✅ Position persists across restarts
- ✅ Can resume from last position

### Estimated Time: 30 minutes

---

## Milestone 10: Global Indexer (Basic)

**File:** `src/db/engine/indexer.rs`

### Tasks
- [ ] Define `GlobalIndexer` struct
  - journals: HashMap<String, Arc<TableJournal>>
  - index_stores: HashMap<String, Arc<IndexStore>>
  - positions: Arc<RwLock<HashMap<String, u64>>>
  - meta_store: Arc<dyn Store>
  - shutdown: AtomicBool
- [ ] Implement `run(&self) -> impl Future`
  - Loop until shutdown
  - Round-robin through tables
  - Read next journal entry
  - Process (add/remove from index store)
  - Save position
- [ ] Implement `process_op(&self, table, op) -> DbResult<()>`
  - For each change in op:
    - Encode key
    - Add or remove from index store
- [ ] Implement `shutdown(&self)`
  - Set shutdown flag
  - Wait for completion
- [ ] Add tests:
  - [ ] Processes single operation
  - [ ] Processes multiple tables
  - [ ] Position advances correctly
  - [ ] Shutdown works

### Acceptance Criteria
- ✅ Indexer processes operations sequentially
- ✅ Multiple tables handled correctly
- ✅ Positions tracked correctly

### Estimated Time: 2 hours

---

## Milestone 11: Index Management API

**File:** `src/db/engine/table.rs`

### Tasks
- [ ] Add `create_index(&self, path: &str) -> DbResult<()>`
  - Parse path to interned components
  - Add to IndexConfig
  - Save config
  - Build initial index (scan all records)
- [ ] Add `drop_index(&self, path: &str) -> DbResult<bool>`
  - Remove from IndexConfig
  - Save config
  - Delete all entries with this path prefix from index store
- [ ] Add `list_indexes(&self) -> DbResult<Vec<IndexDef>>`
  - Load config
  - Return index definitions
- [ ] Add tests:
  - [ ] Create and list index
  - [ ] Create index on existing table (initial build)
  - [ ] Drop index removes entries
  - [ ] Duplicate index creation fails

### Acceptance Criteria
- ✅ Can create/drop/list indexes
- ✅ Initial index build works
- ✅ Index config persists

### Estimated Time: 2 hours

---

## Milestone 12: Query API (Basic)

**File:** `src/db/engine/index/query.rs`

### Tasks
- [ ] Define `QueryBuilder` struct
  - table: Arc<Table>
  - filters: Vec<Filter>
- [ ] Define `Filter` enum
  - Eq { path: String, value: InnerValue }
- [ ] Implement `eq(&mut self, path: &str, value) -> &mut Self`
- [ ] Implement `find(&self) -> impl Future<Output = DbResult<Vec<(RecordId, UserValue)>>>`
  - For single filter:
    - Resolve path to interned
    - Compute hash
    - Lookup in index store
    - Fetch records
  - For multiple filters:
    - Intersect result sets
    - Fetch records
- [ ] Add tests:
  - [ ] Single equality query
  - [ ] Multiple filters (AND logic)
  - [ ] No results case
  - [ ] Index not found error

### Acceptance Criteria
- ✅ Can query by indexed field
- ✅ Multiple filters work correctly
- ✅ Returns correct records

### Estimated Time: 1.5 hours

---

## Future Milestones (Not Yet Scheduled)

- [ ] **Milestone 13:** Update operations in journal
- [ ] **Milestone 14:** Concurrent indexer testing
- [ ] **Milestone 15:** Query optimizations (batch fetching)
- [ ] **Milestone 16:** IN queries (multiple values)
- [ ] **Milestone 17:** Unique index enforcement
- [ ] **Milestone 18:** Index rebuild on corruption
- [ ] **Milestone 19:** Background index creation
- [ ] **Milestone 20:** Performance benchmarks

---

## Progress Tracking

| Milestone | Status | Completed Date |
|-----------|--------|----------------|
| 1. Basic Types | ⏳ Pending | - |
| 2. Key Encoding | ⏳ Pending | - |
| 3. Index Config | ⏳ Pending | - |
| 4. Path Extraction | ⏳ Pending | - |
| 5. Hash Computation | ⏳ Pending | - |
| 6. Table Journal | ⏳ Pending | - |
| 7. Table Integration | ⏳ Pending | - |
| 8. Index Store Ops | ⏳ Pending | - |
| 9. Position Tracking | ⏳ Pending | - |
| 10. Global Indexer | ⏳ Pending | - |
| 11. Index Management | ⏳ Pending | - |
| 12. Query API | ⏳ Pending | - |

---

## Notes

- Each milestone should be completed independently
- Tests are required for each milestone
- Code should compile after each milestone
- Review architecture document before starting: `index_engine.md`
