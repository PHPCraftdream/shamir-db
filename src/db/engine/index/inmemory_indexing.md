# In-Memory Indexing Architecture

## Overview

**Hybrid approach:** In-memory indexes with background persistence thread

- **Primary thread (Table)**: In-memory indexes for fast constraint validation
- **Background thread (Indexer)**: Mirrored indexes + disk persistence
- **Communication**: Async messages via mpsc channel
- **Storage**: Incremental diffs on disk

---

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                        Main Thread (Table)                       │
│                                                                  │
│  insert() ──► validate_unique() ──► in_memory_index.check()     │
│     │                 │                    │                    │
│     │                 │                    └─► O(1) lookup!     │
│     │                 │                                         │
│     │                 └─► If OK: update in-memory index        │
│     │
│     └─► Send IndexMessage to indexer ──────────────────────┐     │
│                                                              │     │
└──────────────────────────────────────────────────────────────┼─────┘
                                                               │
                                                               │ mpsc
                                                               │
                                                               ▼
┌─────────────────────────────────────────────────────────────────┐
│                   Background Indexer Thread                      │
│                                                                  │
│  Receive IndexMessage                                            │
│     │
│     ├─► Compare with mirrored index                              │
│     ├─► Build diff (added/removed entries)                       │
│     ├─► Save diff to disk (incremental)                          │
│     └─► Update mirrored index                                    │
│                                                                  │
│  Storage:                                                        │
│    __idx_diffs__{table} ──► Vec<IndexDiff> (append-only log)    │
│    __idx_snapshot__{table} ──► Full index snapshot (periodic)   │
│                                                                  │
└─────────────────────────────────────────────────────────────────┘
```

---

## Data Structures

### In-Memory Index (Primary)

```rust
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

/// In-memory index for fast lookups
pub struct InMemoryIndex {
    /// Map: (path, hash) → Set of RecordIds
    // Using HashSet for deduplication
    index: HashMap<(Vec<u64>, u64), HashSet<RecordId>>,

    /// For unique indexes: value → RecordId (O(1) uniqueness check)
    unique_index: HashMap<(Vec<u64>, UserValue), RecordId>,

    /// Index definition
    def: IndexDef,
}

impl InMemoryIndex {
    /// Check if value exists at path (for unique constraints)
    pub fn contains(&self, path: &[u64], value: &UserValue) -> bool {
        self.unique_index.contains_key(&(path.to_vec(), value.clone()))
    }

    /// Insert into index
    pub fn insert(&mut self, path: &[u64], value: &UserValue, id: RecordId) {
        // Update unique index
        self.unique_index.insert((path.to_vec(), value.clone()), id);

        // Update regular index (hash-based for future queries)
        let hash = hash_value(value);
        self.index.entry((path.to_vec(), hash))
            .or_insert_with(HashSet::new)
            .insert(id);
    }

    /// Remove from index
    pub fn remove(&mut self, path: &[u64], value: &UserValue, id: RecordId) {
        // Remove from unique index
        self.unique_index.remove(&(path.to_vec(), value.clone()));

        // Remove from regular index
        let hash = hash_value(value);
        if let Some(entries) = self.index.get_mut(&(path.to_vec(), hash)) {
            entries.remove(&id);
            if entries.is_empty() {
                self.index.remove(&(path.to_vec(), hash));
            }
        }
    }
}
```

### Thread-Safe Index Manager

```rust
/// Thread-safe index manager for Table
pub struct IndexManager {
    /// All in-memory indexes for this table
    indexes: Arc<RwLock<HashMap<Vec<u64>, Arc<RwLock<InMemoryIndex>>>>>,

    /// Unique indexes only (for fast iteration)
    unique_indexes: Arc<RwLock<Vec<IndexDef>>>,

    /// Sender for background indexer
    indexer_tx: mpsc::UnboundedSender<IndexMessage>,
}

impl IndexManager {
    /// Check unique constraints (O(1) per index!)
    pub fn check_unique(
        &self,
        path: &[u64],
        value: &UserValue,
    ) -> Result<(), DbError> {
        let unique_indexes = self.unique_indexes.read()
            .map_err(|_| DbError::Internal("Lock poisoned".to_string()))?;

        for index_def in unique_indexes.iter() {
            if index_def.path == path {
                let indexes = self.indexes.read()
                    .map_err(|_| DbError::Internal("Lock poisoned".to_string()))?;

                if let Some(index) = indexes.get(path) {
                    let index = index.read()
                        .map_err(|_| DbError::Internal("Lock poisoned".to_string()))?;

                    if index.contains(path, value) {
                        return Err(DbError::DuplicateKey(path.clone()));
                    }
                }
            }
        }

        Ok(())
    }

    /// Insert into indexes
    pub fn insert(&self, path: &[u64], value: &UserValue, id: RecordId) {
        let indexes = self.indexes.read()
            .map_err(|_| DbError::Internal("Lock poisoned".to_string())).ok();

        if let Some(indexes) = indexes {
            if let Some(index) = indexes.get(path) {
                let mut index = index.write()
                    .map_err(|_| DbError::Internal("Lock poisoned".to_string())).ok();

                if let Some(mut index) = index {
                    index.insert(path, value, id);
                }
            }
        }

        // Send to background indexer
        let _ = self.indexer_tx.send(IndexMessage::insert(
            self.table_name.clone(),
            path.to_vec(),
            value.clone(),
            id,
        ));
    }
}
```

---

## Background Indexer

### Diff Format

```rust
/// Index diff for persistence
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum IndexDiff {
    Add {
        path: Vec<u64>,
        value_hash: u64,
        record_id: RecordId,
    },
    Remove {
        path: Vec<u64>,
        value_hash: u64,
        record_id: RecordId,
    },
    Update {
        path: Vec<u64>,
        old_hash: u64,
        new_hash: u64,
        record_id: RecordId,
    },
}
```

### Indexer Implementation

```rust
/// Background indexer with mirrored indexes
pub struct BackgroundIndexer {
    /// Mirrored in-memory indexes
    indexes: HashMap<Vec<u64>, InMemoryIndex>,

    /// Diff log (append-only)
    diff_log: Arc<dyn Store>,

    /// Snapshot interval
    snapshot_interval: Duration,

    /// Last snapshot time
    last_snapshot: Instant,
}

impl BackgroundIndexer {
    pub async fn run(mut self, mut rx: mpsc::UnboundedReceiver<IndexMessage>) {
        loop {
            tokio::select! {
                // Process next message
                msg = rx.recv() => {
                    match msg {
                        Some(msg) => self.process_message(msg).await,
                        None => break, // Channel closed
                    }
                }

                // Periodic snapshot
                _ = tokio::time::sleep(self.snapshot_interval) => {
                    if self.last_snapshot.elapsed() >= self.snapshot_interval {
                        self.save_snapshot().await;
                        self.last_snapshot = Instant::now();
                    }
                }
            }
        }
    }

    async fn process_message(&mut self, msg: IndexMessage) {
        match msg.op_type {
            OpType::Insert => {
                // Build diff
                let diff = IndexDiff::Add {
                    path: msg.path.clone(),
                    value_hash: hash_value(&msg.value),
                    record_id: msg.record_id,
                };

                // Save diff to disk
                self.save_diff(&diff).await;

                // Update mirrored index
                self.update_index(&msg.path, &msg.value, msg.record_id);
            }

            OpType::Delete => {
                // Similar for delete
            }

            OpType::Update => {
                // Build update diff
                let diff = IndexDiff::Update {
                    path: msg.path.clone(),
                    old_hash: msg.old_hash,
                    new_hash: hash_value(&msg.value),
                    record_id: msg.record_id,
                };

                self.save_diff(&diff).await;
                self.update_index(&msg.path, &msg.value, msg.record_id);
            }
        }
    }

    async fn save_diff(&self, diff: &IndexDiff) {
        // Append to diff log
        let key = format!("__idx_diffs__{}::{}", self.table_name, self.seq_no);
        let value = bincode::serialize(diff).unwrap();

        self.diff_log.set(key.into(), value.into()).await.ok();
        self.seq_no += 1;
    }

    async fn save_snapshot(&self) {
        // Save full index snapshot
        let snapshot = IndexSnapshot {
            indexes: self.indexes.clone(),
            seq_no: self.seq_no,
        };

        let key = format!("__idx_snapshot__{}", self.table_name);
        let value = bincode::serialize(&snapshot).unwrap();

        self.diff_log.set(key.into(), value.into()).await.ok();

        // Optionally: compact diff log (remove diffs before snapshot)
    }
}
```

---

## Integration with Table

### Table Structure

```rust
pub struct Table<R: Repo> {
    // ... existing fields ...

    /// In-memory index manager (fast!)
    index_manager: Arc<IndexManager>,

    /// NOT NEEDED: unique_indexes RwLock (replaced by index_manager)
    /// NOT NEEDED: atomic flags (index manager has its own)
}
```

### Optimized Insert

```rust
impl<R: Repo> Table<R> {
    pub async fn insert(&self, value: &UserValue) -> DbResult<RecordId> {
        let interner = self.get_interner().await?;

        // ✅ O(1) unique constraint check per index!
        // Instead of O(N) table scan
        for index_def in self.index_manager.get_unique_indexes() {
            if let Some(extracted) = extract_value(value, &index_def.path, interner)? {
                // FAST: O(1) lookup in HashMap
                self.index_manager.check_unique(&index_def.path, &extracted)?;
            }
        }

        // ... rest of insert logic ...

        // ✅ Update in-memory indexes
        for index_def in self.index_manager.get_all_indexes() {
            if let Some(extracted) = extract_value(value, &index_def.path, interner)? {
                self.index_manager.insert(&index_def.path, &extracted, id);
            }
        }

        Ok(id)
    }
}
```

---

## Unique Index Creation

### Validate Before Creating Index

```rust
impl<R: Repo> Table<R> {
    pub async fn add_unique_index(&self, path: &[&str]) -> DbResult<()> {
        let interner = self.get_interner().await?;
        let interned_path: Vec<u64> = path.iter()
            .map(|&s| interner.touch_ind(s).val())
            .collect();

        // ✅ Step 1: Validate existing data (ONE TIME SCAN)
        let mut seen_values = HashMap::new();
        let stream = self.list_stream(100);
        pin_mut!(stream);

        while let Some(batch_result) = stream.next().await {
            let batch = batch_result?;
            for (id, existing_value) in batch {
                if let Some(extracted) = extract_value(&existing_value, &interned_path, interner)? {
                    if let Some(existing_id) = seen_values.get(&extracted) {
                        return Err(DbError::DuplicateKey(
                            format!("Duplicate value at {:?}: records {:?} and {:?}", path, existing_id, id)
                        ));
                    }
                    seen_values.insert(extracted, id);
                }
            }
        }

        // ✅ Step 2: Build in-memory index from existing data
        let mut index = InMemoryIndex::new(IndexDef::unique(interned_path.clone()));
        for (value, id) in seen_values {
            index.insert(&interned_path, &value, id);
        }

        // ✅ Step 3: Register index manager
        self.index_manager.add_unique_index(interned_path, index).await?;

        // ✅ Step 4: Notify background indexer
        self.index_manager.notify_index_created(interned_path).await?;

        Ok(())
    }
}
```

---

## Performance Comparison

### BEFORE (2025-02-04): Atomic Flags + Table Scan

```
insert() with unique index:
├─ check_unique_constraints(): ~80% of total time
│  ├─ has_unique_indexes.load(): ~0.1% (O(1) check)
│  ├─ unique_indexes.read(): ~1% (RwLock)
│  └─ check_value_unique(): ~78.9% (STREAMS ENTIRE TABLE!)
│     └─ O(N) where N = total records
├─ user_to_inner(): ~2%
├─ data_store.insert(): ~10%
└─ increment_record_count(): ~6%
```

**Problem:** Still O(N) table scan on every insert!

### AFTER (Proposed): In-Memory Indexes

```
insert() with unique index:
├─ check_unique_constraints(): ~5% of total time ✅
│  ├─ Get unique indexes: ~1%
│  └─ index_manager.check_unique(): ~4% (O(1) HashMap lookup!) ✅
├─ user_to_inner(): ~5%
├─ data_store.insert(): ~50%
├─ index_manager.insert(): ~5% (update in-memory)
├─ send IndexMessage: ~1% (async, non-blocking)
└─ increment_record_count(): ~29%
```

**Improvement:**
- Unique constraint check: **~94% faster** (80% → 5%)
- No table scans ever!
- O(1) HashMap lookup instead of O(N) stream
- Background indexer doesn't block inserts

---

## Crash Recovery

### On Restart

```rust
impl BackgroundIndexer {
    pub async fn load_from_disk(&mut self) -> DbResult<()> {
        // 1. Load latest snapshot
        let snapshot_key = format!("__idx_snapshot__{}", self.table_name);
        if let Ok(bytes) = self.diff_log.get(snapshot_key.into()).await {
            let snapshot: IndexSnapshot = bincode::deserialize(&bytes)?;
            self.indexes = snapshot.indexes;
            self.seq_no = snapshot.seq_no;
        }

        // 2. Replay diffs after snapshot
        loop {
            let diff_key = format!("__idx_diffs__{}::{}", self.table_name, self.seq_no);
            match self.diff_log.get(diff_key.into()).await {
                Ok(bytes) => {
                    let diff: IndexDiff = bincode::deserialize(&bytes)?;
                    self.apply_diff(&diff);
                    self.seq_no += 1;
                }
                Err(DbError::NotFound(_)) => break, // No more diffs
                Err(e) => return Err(e),
            }
        }

        Ok(())
    }
}
```

---

## Implementation Steps

### Phase 1: Core In-Memory Indexing (4-6 hours)

1. **Create InMemoryIndex struct** (1 hour)
   - HashMap-based storage
   - Unique index tracking
   - Insert/remove/contains methods

2. **Create IndexManager** (2 hours)
   - Thread-safe wrapper
   - check_unique() method
   - insert/remove methods
   - Integration with Table

3. **Update Table operations** (2 hours)
   - Replace check_unique_constraints() with IndexManager
   - Update insert/update/delete to maintain indexes
   - Add index creation with one-time scan

4. **Testing** (1 hour)
   - Unit tests for InMemoryIndex
   - Integration tests for Table + IndexManager
   - Performance benchmarks

### Phase 2: Background Indexer (6-8 hours)

5. **Create IndexDiff types** (30 min)
   - Add/Remove/Update variants
   - Serialization

6. **Create BackgroundIndexer** (3 hours)
   - Message receiving loop
   - Diff building
   - Mirrored index updates

7. **Add persistence** (2 hours)
   - Diff log storage
   - Snapshot mechanism
   - Crash recovery

8. **Integration & testing** (2 hours)
   - Connect Table → Indexer via mpsc
   - Test async updates
   - Test crash recovery

### Phase 3: Optimization (2-3 hours)

9. **Performance tuning** (2 hours)
   - Benchmark different HashMap implementations
   - Test lock contention
   - Optimize hot paths

10. **Stress testing** (1 hour)
    - Concurrent inserts
    - Large datasets
    - Index churn

---

## Total Effort: 12-17 hours

**High Priority (Phase 1):** 4-6 hours
- Immediate performance gain
- No background thread complexity
- Easy to test

**Medium Priority (Phase 2):** 6-8 hours
- Adds persistence
- Crash recovery
- Production-ready

**Low Priority (Phase 3):** 2-3 hours
- Performance tuning
- Optimization

---

## Open Questions

1. **Memory footprint:**
   - How large can indexes grow?
   - Should we cap index size?
   - What to do when memory is full?

2. **Index rebuild:**
   - How to rebuild corrupted index?
   - Can we rebuild from diff log?
   - Full table scan as fallback?

3. **Background indexer priority:**
   - Should indexer be high priority or low priority thread?
   - How to handle slow disk I/O?
   - Bounded vs unbounded message channel?

4. **Multiple tables:**
   - One global indexer or per-table indexer?
   - How to handle table drops?
   - Cross-table indexes?

5. **Query API:**
   - How to query by index?
   - Return iterator or Vec?
   - How to handle missing indexes?

---

## Summary

**Hybrid In-Memory Indexing:**

✅ **Pros:**
- O(1) unique constraint checking (vs O(N) table scan)
- Non-blocking writes (async background indexer)
- Fast query-by-index (when implemented)
- Simple architecture (in-memory HashMap + persistence)
- Crash recovery via diff log

⚠️ **Cons:**
- Higher memory usage (indexes in RAM)
- Complexity (two copies of indexes)
- Background thread management
- Index rebuild on corruption

**Recommendation:**
Start with **Phase 1** (in-memory only) for immediate performance gain.
Add **Phase 2** (background indexer) when persistence is needed.
Optimize in **Phase 3** based on real-world usage.
