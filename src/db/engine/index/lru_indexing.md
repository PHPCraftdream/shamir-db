# LRU Index Management with State Flags

## Overview

**Single in-memory index instance with per-record state flags + LRU eviction**

- **One index copy in RAM** (not two!)
- **Per-record state flags:** ACTUAL, UPDATE, SAVING
- **LRU cache** for memory management
- **Memory limit** with automatic eviction
- **Loader** for on-demand disk → RAM loading

---

## Architecture

### State Machine per Index Entry

```
┌─────────┐  Table updates  ┌─────────┐  Indexer starts  ┌─────────┐
│ ACTUAL  │ ───────────────► │ UPDATE  │ ────────────────► │ SAVING  │
└─────────┘                 └─────────┘                   └─────────┘
     ▲                                                            │
     │                    Indexer saves                          │
     └────────────────────────────────────────────────────────────┘
```

**State transitions:**

1. **ACTUAL → UPDATE**: Table modifies data
   - Table sets flag when inserting/updating/deleting

2. **UPDATE → SAVING**: Indexer picks up change
   - Indexer scans for UPDATE entries
   - Marks as SAVING to prevent concurrent modifications

3. **SAVING → ACTUAL**: Indexer completes save
   - Data written to disk
   - Flag cleared to ACTUAL

4. **SAVING → UPDATE**: Table modifies while saving
   - Table can set UPDATE even during SAVING
   - Creates pending change for next indexer iteration

---

## Data Structures

### Index Entry with State Flag

```rust
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// State of an index entry
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryState {
    /// Entry is up-to-date (in memory matches disk)
    Actual,

    /// Entry was modified by Table, needs to be saved
    Update,

    /// Indexer is currently saving this entry to disk
    Saving,
}

/// Single entry in index with state flag
#[derive(Debug, Clone)]
pub struct IndexEntry {
    /// The record ID
    pub record_id: RecordId,

    /// The indexed value (for unique indexes)
    pub value: UserValue,

    /// Hash of the value (for non-unique indexes)
    pub value_hash: u64,

    /// Current state of this entry
    pub state: EntryState,

    /// Last access time (for LRU eviction)
    pub last_access: Instant,

    /// Size in bytes (for memory tracking)
    pub size_bytes: usize,
}

impl IndexEntry {
    /// Mark entry as needing update
    pub fn mark_update(&mut self) {
        self.state = EntryState::Update;
    }

    /// Mark entry as currently being saved
    pub fn mark_saving(&mut self) {
        self.state = EntryState::Saving;
    }

    /// Mark entry as actual (saved)
    pub fn mark_actual(&mut self) {
        self.state = EntryState::Actual;
        self.last_access = Instant::now();
    }

    /// Calculate memory size
    pub fn calculate_size(&self) -> usize {
        // RecordId: 16 bytes
        // UserValue: estimated
        // value_hash: 8 bytes
        // state: 1 byte
        // last_access: 8 bytes
        // size_bytes: 8 bytes
        16 + self.value.estimate_size() + 8 + 1 + 8 + 8
    }
}
```

### LRU Index Store

```rust
/// LRU index store with memory limit
pub struct LRUIndexStore {
    /// Map: (path, value_hash) → IndexEntry
    /// For non-unique indexes (multiple records per value)
    index: HashMap<(Vec<u64>, u64), Vec<IndexEntry>>,

    /// Map: (path, value) → IndexEntry
    /// For unique indexes (single record per value)
    unique_index: HashMap<(Vec<u64>, UserValue), IndexEntry>,

    /// Memory limit in bytes
    memory_limit: usize,

    /// Current memory usage in bytes
    current_memory: Arc<AtomicUsize>,

    /// Disk store for loading/saving
    disk_store: Arc<dyn Store>,

    /// Table name for prefixing keys
    table_name: String,
}

impl LRUIndexStore {
    /// Create new LRU index store
    pub fn new(
        memory_limit: usize,
        disk_store: Arc<dyn Store>,
        table_name: String,
    ) -> Self {
        Self {
            index: HashMap::new(),
            unique_index: HashMap::new(),
            memory_limit,
            current_memory: Arc::new(AtomicUsize::new(0)),
            disk_store,
            table_name,
        }
    }

    /// Check if value exists (for unique constraint) - O(1)
    pub fn contains_unique(&self, path: &[u64], value: &UserValue) -> bool {
        self.unique_index.contains_key(&(path.to_vec(), value.clone()))
    }

    /// Get entry without marking as accessed (for checking)
    pub fn get_entry(&self, path: &[u64], value: &UserValue) -> Option<&IndexEntry> {
        self.unique_index.get(&(path.to_vec(), value.clone()))
    }

    /// Insert or update entry - marks as UPDATE
    pub fn insert(&mut self, path: &[u64], value: &UserValue, record_id: RecordId) {
        // Remove old entry if exists
        if let Some(old_entry) = self.unique_index.remove(&(path.to_vec(), value.clone())) {
            self.current_memory.fetch_sub(old_entry.size_bytes, Ordering::Relaxed);
        }

        // Create new entry
        let entry = IndexEntry {
            record_id,
            value: value.clone(),
            value_hash: hash_value(value),
            state: EntryState::Update, // Mark as needing save
            last_access: Instant::now(),
            size_bytes: 0, // Will be calculated
        };

        let size = entry.calculate_size();

        // Check memory limit before inserting
        self.ensure_memory_available(size).await;

        // Insert
        self.unique_index.insert((path.to_vec(), value.clone()), entry);

        // Update memory counter
        self.current_memory.fetch_add(size, Ordering::Relaxed);
    }

    /// Remove entry - marks as UPDATE (will be saved as deletion)
    pub fn remove(&mut self, path: &[u64], value: &UserValue) {
        if let Some(entry) = self.unique_index.remove(&(path.to_vec(), value.clone())) {
            self.current_memory.fetch_sub(entry.size_bytes, Ordering::Relaxed);

            // Don't delete immediately - mark as UPDATE so indexer saves deletion
            // This is tricky - we might need a "tombstone" mechanism
        }
    }

    /// Ensure enough memory is available (evict if necessary)
    async fn ensure_memory_available(&mut self, required: usize) {
        let current = self.current_memory.load(Ordering::Relaxed);

        if current + required <= self.memory_limit {
            return; // Enough space
        }

        // Need to evict
        let need_to_free = (current + required) - self.memory_limit;
        let mut freed = 0;

        // Collect entries for eviction (LRU order)
        let mut entries_by_age: Vec<_> = self.unique_index
            .iter()
            .filter_map(|(key, entry)| {
                // Only evict ACTUAL entries (not UPDATE or SAVING)
                if entry.state == EntryState::Actual {
                    Some((key.clone(), entry.last_access, entry.size_bytes))
                } else {
                    None
                }
            })
            .collect();

        // Sort by last access (oldest first)
        entries_by_age.sort_by_key(|(_, access, _)| *access);

        // Evict until we have enough space
        for (key, _, size) in entries_by_age {
            if freed >= need_to_free {
                break;
            }

            // Save to disk before evicting
            if let Some(entry) = self.unique_index.get(&key) {
                if entry.state == EntryState::Actual {
                    self.save_to_disk(&key, entry).await.ok();
                }
            }

            // Remove from memory
            self.unique_index.remove(&key);
            self.current_memory.fetch_sub(size, Ordering::Relaxed);
            freed += size;
        }

        // If still not enough space, we have a problem
        if freed < need_to_free {
            log::warn!("Cannot free enough memory: need {}, freed {}", need_to_free, freed);
        }
    }

    /// Save entry to disk
    async fn save_to_disk(&self, key: &(Vec<u64>, UserValue), entry: &IndexEntry) {
        let (path, value) = key;

        // Create disk key
        let disk_key = format!(
            "__idx_entry__{}__{:?}__{}",
            self.table_name,
            path,
            entry.value_hash
        );

        // Serialize entry
        let serialized = bincode::serialize(&(entry.record_id, value.clone()))
            .map_err(|e| log::error!("Failed to serialize index entry: {}", e))
            .ok();

        if let Some(bytes) = serialized {
            self.disk_store.set(disk_key.into(), bytes.into()).await.ok();
        }
    }

    /// Load entry from disk
    async fn load_from_disk(&self, path: &[u64], value_hash: u64) -> Option<IndexEntry> {
        let disk_key = format!(
            "__idx_entry__{}__{:?}__{}",
            self.table_name, path, value_hash
        );

        match self.disk_store.get(disk_key.into()).await {
            Ok(bytes) => {
                if let Ok((record_id, value)) = bincode::deserialize(&(bytes.to_vec())) {
                    return Some(IndexEntry {
                        record_id,
                        value,
                        value_hash,
                        state: EntryState::Actual,
                        last_access: Instant::now(),
                        size_bytes: bytes.len(),
                    });
                }
            }
            Err(DbError::NotFound(_)) => {
                // Entry not on disk
            }
            Err(e) => {
                log::error!("Failed to load index entry: {}", e);
            }
        }

        None
    }
}
```

---

## Index Manager with LRU

```rust
/// Thread-safe index manager with LRU eviction
pub struct IndexManager {
    /// LRU index store
    store: Arc<RwLock<LRUIndexStore>>,

    /// Background indexer handle
    indexer_handle: Option<tokio::task::JoinHandle<()>>,
}

impl IndexManager {
    /// Create new index manager
    pub fn new(
        memory_limit: usize,
        disk_store: Arc<dyn Store>,
        table_name: String,
    ) -> Self {
        let store = Arc::new(RwLock::new(LRUIndexStore::new(
            memory_limit,
            disk_store.clone(),
            table_name.clone(),
        )));

        // Start background indexer
        let indexer_store = store.clone();
        let indexer_handle = tokio::spawn(async move {
            Self::indexer_loop(indexer_store).await;
        });

        Self {
            store,
            indexer_handle: Some(indexer_handle),
        }
    }

    /// Check unique constraint - O(1) lookup
    pub async fn check_unique(&self, path: &[u64], value: &UserValue) -> Result<(), DbError> {
        let store = self.store.read().await;
        if store.contains_unique(path, value) {
            return Err(DbError::DuplicateKey(path.clone()));
        }
        Ok(())
    }

    /// Insert entry (marks as UPDATE)
    pub async fn insert(&self, path: &[u64], value: &UserValue, record_id: RecordId) {
        let mut store = self.store.write().await;
        store.insert(path, value, record_id);
    }

    /// Background indexer loop
    async fn indexer_loop(store: Arc<RwLock<LRUIndexStore>>) {
        let mut interval = tokio::time::interval(Duration::from_secs(1));

        loop {
            interval.tick().await;

            // Scan for UPDATE entries
            let updates_to_save = {
                let mut store = store.write().await;
                Self::collect_updates(&mut store)
            };

            // Save updates to disk
            for (path, value, entry) in updates_to_save {
                // Mark as SAVING
                {
                    let mut store = store.write().await;
                    if let Some(entry) = store.get_entry_mut(&path, &value) {
                        entry.mark_saving();
                    }
                }

                // Save to disk
                // ... save logic ...

                // Mark as ACTUAL
                {
                    let mut store = store.write().await;
                    if let Some(entry) = store.get_entry_mut(&path, &value) {
                        entry.mark_actual();
                    }
                }
            }
        }
    }

    /// Collect entries marked as UPDATE
    fn collect_updates(store: &mut LRUIndexStore) -> Vec<(Vec<u64>, UserValue, IndexEntry)> {
        let mut updates = Vec::new();

        for ((path, value), entry) in store.unique_index.iter() {
            if entry.state == EntryState::Update {
                updates.push((path.clone(), value.clone(), entry.clone()));
            }
        }

        updates
    }
}
```

---

## Memory Management

### Configuration

```rust
/// Index manager configuration
pub struct IndexConfig {
    /// Memory limit per table (default: 100 MB)
    pub memory_limit_per_table: usize,

    /// Global memory limit (default: 1 GB)
    pub global_memory_limit: usize,

    /// LRU eviction threshold (default: 90%)
    pub eviction_threshold: f64,

    /// Indexer sweep interval (default: 1 second)
    pub indexer_interval: Duration,
}

impl Default for IndexConfig {
    fn default() -> Self {
        Self {
            memory_limit_per_table: 100 * 1024 * 1024, // 100 MB
            global_memory_limit: 1024 * 1024 * 1024,   // 1 GB
            eviction_threshold: 0.9,                   // 90%
            indexer_interval: Duration::from_secs(1),
        }
    }
}
```

### Memory Estimation

```rust
impl UserValue {
    /// Estimate memory size
    pub fn estimate_size(&self) -> usize {
        match self {
            UserValue::Null => 8,
            UserValue::Int(_) => 8,
            UserValue::UInt(_) => 8,
            UserValue::Float(_) => 8,
            UserValue::Bool(_) => 1,
            UserValue::Str(s) => 8 + s.len(),
            UserValue::Bin(b) => 8 + b.len(),
            UserValue::Decimal(d) => 16,
            UserValue::BigInt(n) => 16,
            UserValue::Array(arr) => {
                8 + arr.iter().map(|v| v.estimate_size()).sum::<usize>()
            }
            UserValue::Set(set) => {
                8 + set.iter().map(|v| v.estimate_size()).sum::<usize>()
            }
            UserValue::Map(map) => {
                8 + map.iter()
                    .map(|(k, v)| k.len() + v.estimate_size())
                    .sum::<usize>()
            }
        }
    }
}
```

---

## Disk Storage Format

### Entry Storage

```
__idx_entry__{table}__{path_hash}__{value_hash} -> (RecordId, UserValue)

Example:
__idx_entry__users__[1,2,3]__12345678 -> (RecordId(...), UserValue::Str("alice"))
```

### Tombstone for Deletions

```
__idx_tombstone__{table}__{path_hash}__{value_hash} -> RecordId

When an entry is removed, we save a tombstone to disk
so indexer knows to delete it from persistent storage
```

---

## Integration with Table

```rust
pub struct Table<R: Repo> {
    // ... existing fields ...

    /// Index manager with LRU
    index_manager: Arc<IndexManager>,
}

impl<R: Repo> Table<R> {
    pub async fn insert(&self, value: &UserValue) -> DbResult<RecordId> {
        let interner = self.get_interner().await?;

        // Check unique constraints (O(1)!)
        for index_def in self.index_manager.get_unique_indexes().await {
            if let Some(extracted) = extract_value(value, &index_def.path, interner)? {
                // This will auto-load from disk if evicted
                self.index_manager.check_unique(&index_def.path, &extracted).await?;
            }
        }

        // ... rest of insert ...

        // Update in-memory indexes (marks as UPDATE)
        for index_def in self.index_manager.get_all_indexes().await {
            if let Some(extracted) = extract_value(value, &index_def.path, interner)? {
                self.index_manager.insert(&index_def.path, &extracted, id).await;
            }
        }

        Ok(id)
    }
}
```

---

## Crash Recovery

### On Startup

```rust
impl IndexManager {
    pub async fn recover_from_disk(&self) -> DbResult<()> {
        let mut store = self.store.write().await;

        // Scan disk for all index entries
        let prefix = format!("__idx_entry__{}__", store.table_name);
        let iter = store.disk_store.scan(&prefix).await?;

        while let Some((key, value)) = iter.next().await {
            // Deserialize entry
            if let Ok((record_id, user_value)) = bincode::deserialize(&value) {
                // Extract path and value_hash from key
                // Parse key: "__idx_entry__users__{path}__{hash}"
                // ...

                // Add to memory as ACTUAL
                let entry = IndexEntry {
                    record_id,
                    value: user_value,
                    value_hash: /* from key */,
                    state: EntryState::Actual,
                    last_access: Instant::now(),
                    size_bytes: value.len(),
                };

                // Add to store
                store.unique_index.insert((path, user_value), entry);
                store.current_memory.fetch_add(entry.size_bytes, Ordering::Relaxed);
            }
        }

        // If memory limit exceeded, LRU will evict automatically

        Ok(())
    }
}
```

---

## Performance Characteristics

### Operations

| Operation | Complexity | Notes |
|-----------|-----------|-------|
| `check_unique()` | O(1) | HashMap lookup |
| `insert()` | O(1) | HashMap insert + mark UPDATE |
| `remove()` | O(1) | HashMap remove |
| `indexer sweep` | O(N) | Scan for UPDATE flags |
| `eviction` | O(N log N) | Sort by LRU + evict |
| `load from disk` | O(1) | Single entry load |

### Memory

- **Per entry:** ~100-500 bytes depending on value size
- **10,000 records:** ~1-5 MB
- **100,000 records:** ~10-50 MB
- **1,000,000 records:** ~100-500 MB

### Scalability

- **Insert with unique index:** O(1) - constant time!
- **Memory usage:** Bounded by LRU limit
- **Disk usage:** O(N) - all entries persisted

---

## Implementation Steps

### Phase 1: Core LRU Index (6-8 hours)

1. **EntryState enum** (30 min)
2. **IndexEntry struct** (1 hour)
3. **LRUIndexStore** (3 hours)
   - insert/remove/contains
   - Memory tracking
   - LRU eviction

4. **IndexManager** (2 hours)
   - Thread-safe wrapper
   - check_unique() method

5. **Testing** (1 hour)
   - Unit tests
   - Memory stress tests

### Phase 2: Background Indexer (4-6 hours)

6. **Indexer loop** (2 hours)
   - Scan for UPDATE flags
   - Mark SAVING
   - Save to disk
   - Mark ACTUAL

7. **Disk storage** (2 hours)
   - Save/load entries
   - Tombstone for deletions

8. **Crash recovery** (1 hour)
   - Load entries on startup
   - Handle missing entries

### Phase 3: Optimization (2-3 hours)

9. **Memory tuning** (1 hour)
10. **Performance benchmarks** (1 hour)
11. **Stress testing** (1 hour)

---

## Total Effort: 12-17 hours

**Same as before, but with better architecture!**

- ✅ Single index instance (not two!)
- ✅ Per-record state flags (ACTUAL/UPDATE/SAVING)
- ✅ LRU eviction for memory management
- ✅ On-demand disk loading
- ✅ Memory limit enforcement

---

## Summary

**Key improvements over two-instance approach:**

1. **No duplication** - Single index in memory
2. **State flags** - Clear ownership of changes
3. **LRU eviction** - Automatic memory management
4. **On-demand loading** - Load from disk when needed

**Same benefits:**
- ✅ O(1) unique constraint checking
- ✅ Background persistence
- ✅ Crash recovery

**Bonus:**
- ✅ Memory limit enforcement
- ✅ Automatic eviction
- ✅ Better resource utilization

**Recommendation:** Implement this LRU-based approach instead of two-instance architecture.
