# Index Data Structures Design

## Overview

Separation of concerns for index data:

1. **IndexEntry** - The actual indexed data (record_id, value, state)
2. **IndexUsageTracker** - Usage metadata (when used, how often, hot/cold)
3. **IndexStore** - Container holding both with LRU eviction logic

---

## Core Structures

### 1. IndexEntry - The Data

```rust
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Instant;
use std::sync::Arc;

/// State of an index entry (1 byte = AtomicU8 for lock-free reads)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum EntryState {
    /// Entry is up-to-date (in memory matches disk)
    Actual = 0,

    /// Entry was modified by Table, needs to be saved
    Update = 1,

    /// Indexer is currently saving this entry to disk
    Saving = 2,
}

impl EntryState {
    /// Convert to/from u8 for atomic storage
    pub fn as_u8(self) -> u8 {
        self as u8
    }

    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(EntryState::Actual),
            1 => Some(EntryState::Update),
            2 => Some(EntryState::Saving),
            _ => None,
        }
    }

    /// Check if entry can be evicted
    pub fn can_evict(&self) -> bool {
        matches!(self, EntryState::Actual)
    }
}

/// Single entry in unique index
#[derive(Debug, Clone)]
pub struct IndexEntry {
    /// The record ID
    pub record_id: RecordId,

    /// The indexed value
    pub value: UserValue,

    /// Hash of the value (for quick comparison)
    pub value_hash: u64,

    /// Current state (atomic for lock-free reads)
    state: Arc<AtomicU8>,

    /// Size in bytes
    pub size_bytes: usize,
}

impl IndexEntry {
    /// Create new entry (initially marked as UPDATE)
    pub fn new(record_id: RecordId, value: UserValue) -> Self {
        let size_bytes = 16 + value.estimate_size() + 8 + 1; // record_id + value + hash + state

        Self {
            record_id,
            value,
            value_hash: hash_value(&value),
            state: Arc::new(AtomicU8::new(EntryState::Update.as_u8())),
            size_bytes,
        }
    }

    /// Get current state (lock-free read)
    pub fn get_state(&self) -> EntryState {
        EntryState::from_u8(self.state.load(Ordering::Relaxed))
            .unwrap_or(EntryState::Actual)
    }

    /// Mark as needing update
    pub fn mark_update(&self) {
        self.state.store(EntryState::Update.as_u8(), Ordering::Relaxed);
    }

    /// Mark as currently being saved
    pub fn mark_saving(&self) {
        self.state.store(EntryState::Saving.as_u8(), Ordering::Relaxed);
    }

    /// Mark as actual (saved)
    pub fn mark_actual(&self) {
        self.state.store(EntryState::Actual.as_u8(), Ordering::Relaxed);
    }

    /// Check if this entry can be evicted
    pub fn can_evict(&self) -> bool {
        self.get_state().can_evict()
    }

    /// Calculate memory size
    pub fn calculate_size(&self) -> usize {
        self.size_bytes
    }
}

// For unique index: (path, value) -> IndexEntry
pub type UniqueIndex = HashMap<(Vec<u64>, UserValue), IndexEntry>;

// For non-unique index: (path, hash) -> Vec<IndexEntry>
pub type NonUniqueIndex = HashMap<(Vec<u64>, u64), Vec<IndexEntry>>;
```

---

### 2. IndexUsageTracker - Usage Metadata

```rust
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

/// Access pattern statistics
#[derive(Debug, Clone)]
pub struct AccessStats {
    /// Total number of accesses (reads)
    total_accesses: Arc<AtomicU64>,

    /// Number of accesses since last checkpoint
    recent_accesses: Arc<AtomicU64>,

    /// Last checkpoint time
    last_checkpoint: Arc<std::sync::RwLock<Instant>>,
}

impl AccessStats {
    pub fn new() -> Self {
        Self {
            total_accesses: Arc::new(AtomicU64::new(0)),
            recent_accesses: Arc::new(AtomicU64::new(0)),
            last_checkpoint: Arc::new(std::sync::RwLock::new(Instant::now())),
        }
    }

    /// Record an access (e.g., when checking uniqueness)
    pub fn record_access(&self) {
        self.total_accesses.fetch_add(1, Ordering::Relaxed);
        self.recent_accesses.fetch_add(1, Ordering::Relaxed);
    }

    /// Get total accesses
    pub fn total_accesses(&self) -> u64 {
        self.total_accesses.load(Ordering::Relaxed)
    }

    /// Get recent accesses since checkpoint
    pub fn recent_accesses(&self) -> u64 {
        self.recent_accesses.load(Ordering::Relaxed)
    }

    /// Get access rate (accesses per second)
    pub fn access_rate(&self) -> f64 {
        let total = self.total_accesses();
        let elapsed = {
            let checkpoint = self.last_checkpoint.read().unwrap();
            checkpoint.elapsed()
        };
        let secs = elapsed.as_secs_f64();
        if secs > 0.0 {
            total as f64 / secs
        } else {
            0.0
        }
    }

    /// Reset recent counter (call periodically)
    pub fn checkpoint(&self) {
        self.recent_accesses.store(0, Ordering::Relaxed);
        *self.last_checkpoint.write().unwrap() = Instant::now();
    }
}

/// Usage tracking for a single index entry
#[derive(Debug, Clone)]
pub struct IndexUsageTracker {
    /// When this entry was last accessed (for LRU)
    last_access: Arc<std::sync::RwLock<Instant>>,

    /// When this entry was created
    created_at: Instant,

    /// Access statistics
    stats: AccessStats,

    /// Access pattern (for optimization)
    pattern: Arc<AtomicU8>, // Stored as AccessPattern enum
}

/// Access pattern classification
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum AccessPattern {
    /// Unknown (new entry)
    Unknown = 0,

    /// Hot (frequently accessed - >1 access/sec)
    Hot = 1,

    /// Warm (occasionally accessed - >0.1 access/sec)
    Warm = 2,

    /// Cold (rarely accessed - <0.1 access/sec)
    Cold = 3,

    /// Dead (never accessed after creation)
    Dead = 4,
}

impl AccessPattern {
    fn as_u8(self) -> u8 {
        self as u8
    }

    fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(AccessPattern::Unknown),
            1 => Some(AccessPattern::Hot),
            2 => Some(AccessPattern::Warm),
            3 => Some(AccessPattern::Cold),
            4 => Some(AccessPattern::Dead),
            _ => None,
        }
    }
}

impl IndexUsageTracker {
    /// Create new usage tracker
    pub fn new() -> Self {
        Self {
            last_access: Arc::new(std::sync::RwLock::new(Instant::now())),
            created_at: Instant::now(),
            stats: AccessStats::new(),
            pattern: Arc::new(AtomicU8::new(AccessPattern::Unknown.as_u8())),
        }
    }

    /// Record an access (call when checking uniqueness)
    pub fn record_access(&self) {
        // Update last access time
        *self.last_access.write().unwrap() = Instant::now();

        // Update statistics
        self.stats.record_access();

        // Update pattern classification
        self.update_pattern();
    }

    /// Update access pattern based on statistics
    fn update_pattern(&self) {
        let rate = self.stats.access_rate();
        let pattern = match rate {
            r if r > 1.0 => AccessPattern::Hot,
            r if r > 0.1 => AccessPattern::Warm,
            r if r > 0.0 => AccessPattern::Cold,
            _ => {
                // Never accessed
                if self.stats.total_accesses() == 0 {
                    AccessPattern::Dead
                } else {
                    AccessPattern::Cold
                }
            }
        };

        self.pattern.store(pattern.as_u8(), Ordering::Relaxed);
    }

    /// Get access pattern
    pub fn get_pattern(&self) -> AccessPattern {
        AccessPattern::from_u8(self.pattern.load(Ordering::Relaxed))
            .unwrap_or(AccessPattern::Unknown)
    }

    /// Get time since last access
    pub fn idle_time(&self) -> Duration {
        let last = *self.last_access.read().unwrap();
        last.elapsed()
    }

    /// Get age of this entry
    pub fn age(&self) -> Duration {
        self.created_at.elapsed()
    }

    /// Get total accesses
    pub fn total_accesses(&self) -> u64 {
        self.stats.total_accesses()
    }

    /// Get access rate
    pub fn access_rate(&self) -> f64 {
        self.stats.access_rate()
    }

    /// Check if entry is "hot" (frequently accessed)
    pub fn is_hot(&self) -> bool {
        self.get_pattern() == AccessPattern::Hot
    }

    /// Check if entry is "cold" (rarely accessed)
    pub fn is_cold(&self) -> bool {
        matches!(self.get_pattern(), AccessPattern::Cold | AccessPattern::Dead)
    }

    /// Check if entry should be evicted based on usage
    pub fn should_evict(&self, max_idle_time: Duration) -> bool {
        // Never evict hot entries
        if self.is_hot() {
            return false;
        }

        // Evict if idle for too long
        self.idle_time() > max_idle_time
    }
}
```

---

### 3. IndexEntryWithTracking - Combined Structure

```rust
/// Combined entry with data and usage tracking
#[derive(Debug, Clone)]
pub struct IndexEntryWithTracking {
    /// The actual index entry
    pub entry: IndexEntry,

    /// Usage tracking metadata
    pub usage: IndexUsageTracker,
}

impl IndexEntryWithTracking {
    /// Create new entry with tracking
    pub fn new(record_id: RecordId, value: UserValue) -> Self {
        Self {
            entry: IndexEntry::new(record_id, value),
            usage: IndexUsageTracker::new(),
        }
    }

    /// Check uniqueness (records access for LRU)
    pub fn check_unique(&self, other_value: &UserValue) -> bool {
        self.usage.record_access();
        self.entry.value == *other_value
    }

    /// Get total memory size (entry + tracking overhead)
    pub fn total_size(&self) -> usize {
        self.entry.size_bytes + 64 // ~64 bytes for tracking metadata
    }

    /// Check if can be evicted
    pub fn can_evict(&self, max_idle_time: Duration) -> bool {
        self.entry.can_evict() && self.usage.should_evict(max_idle_time)
    }

    /// Get eviction priority (higher = evict first)
    pub fn eviction_priority(&self, max_idle_time: Duration) -> u64 {
        let mut priority = 0u64;

        // Factor 1: Idle time (higher = more idle)
        let idle_secs = self.usage.idle_time().as_secs();
        priority = priority.saturating_add(idle_secs);

        // Factor 2: Access pattern (cold > warm > hot)
        match self.usage.get_pattern() {
            AccessPattern::Dead => priority += 1000,
            AccessPattern::Cold => priority += 500,
            AccessPattern::Warm => priority += 100,
            AccessPattern::Hot => priority += 0,
            AccessPattern::Unknown => priority += 200,
        }

        // Factor 3: Entry size (larger = higher priority to evict)
        priority = priority.saturating_add(self.entry.size_bytes as u64 / 1024);

        priority
    }
}
```

---

### 4. LRUIndexStore - The Container

```rust
use std::sync::{Arc, RwLock};
use std::collections::HashMap;

/// LRU index store with usage tracking
pub struct LRUIndexStore {
    /// Unique index: (path, value) -> EntryWithTracking
    unique_index: Arc<RwLock<HashMap<(Vec<u64>, UserValue), IndexEntryWithTracking>>>,

    /// Non-unique index: (path, hash) -> Vec<EntryWithTracking>
    non_unique_index: Arc<RwLock<HashMap<(Vec<u64>, u64), Vec<IndexEntryWithTracking>>>>,

    /// Memory limit in bytes
    memory_limit: usize,

    /// Current memory usage
    current_memory: Arc<AtomicUsize>,

    /// Disk store for loading/saving
    disk_store: Arc<dyn Store>,

    /// Table name
    table_name: String,

    /// Maximum idle time before eviction (default: 1 hour)
    max_idle_time: Duration,

    /// Eviction threshold (default: 90%)
    eviction_threshold: f64,
}

impl LRUIndexStore {
    pub fn new(
        memory_limit: usize,
        disk_store: Arc<dyn Store>,
        table_name: String,
    ) -> Self {
        Self {
            unique_index: Arc::new(RwLock::new(HashMap::new())),
            non_unique_index: Arc::new(RwLock::new(HashMap::new())),
            memory_limit,
            current_memory: Arc::new(AtomicUsize::new(0)),
            disk_store,
            table_name,
            max_idle_time: Duration::from_secs(3600), // 1 hour
            eviction_threshold: 0.9,
        }
    }

    /// Check if value exists (for unique constraint) - O(1)
    /// Records access for LRU tracking
    pub fn contains_unique(&self, path: &[u64], value: &UserValue) -> bool {
        let unique_index = self.unique_index.read().unwrap();
        if let Some(entry_with_tracking) = unique_index.get(&(path.to_vec(), value.clone())) {
            // Record access for LRU
            entry_with_tracking.usage.record_access();

            // Check value
            entry_with_tracking.check_unique(value)
        } else {
            false
        }
    }

    /// Get entry (without recording access)
    pub fn get_entry(&self, path: &[u64], value: &UserValue) -> Option<IndexEntryWithTracking> {
        let unique_index = self.unique_index.read().unwrap();
        unique_index.get(&(path.to_vec(), value.clone())).cloned()
    }

    /// Insert or update entry
    pub fn insert(&mut self, path: &[u64], value: &UserValue, record_id: RecordId) {
        // Remove old entry if exists
        let old_size = if let Some(old_entry) = self.remove_entry(path, value) {
            old_entry.total_size()
        } else {
            0
        };

        // Create new entry
        let entry_with_tracking = IndexEntryWithTracking::new(record_id, value.clone());
        let new_size = entry_with_tracking.total_size();

        // Check memory limit
        self.ensure_memory_available(new_size);

        // Insert
        let mut unique_index = self.unique_index.write().unwrap();
        unique_index.insert((path.to_vec(), value.clone()), entry_with_tracking);

        // Update memory counter
        self.current_memory.fetch_add(new_size, Ordering::Relaxed);
    }

    /// Remove entry (marks for deletion, doesn't free memory yet)
    pub fn remove_entry(&self, path: &[u64], value: &UserValue) -> Option<IndexEntryWithTracking> {
        let mut unique_index = self.unique_index.write().unwrap();
        unique_index.remove(&(path.to_vec(), value.clone()))
    }

    /// Ensure enough memory available (evict if necessary)
    fn ensure_memory_available(&self, required: usize) {
        let current = self.current_memory.load(Ordering::Relaxed);

        if current + required <= self.memory_limit {
            return; // Enough space
        }

        // Need to evict
        let need_to_free = (current + required) - ((self.memory_limit as f64 * self.eviction_threshold) as usize);
        let mut freed = 0;

        // Collect entries for eviction
        let mut eviction_candidates = Vec::new();

        {
            let unique_index = self.unique_index.read().unwrap();
            for (key, entry_with_tracking) in unique_index.iter() {
                // Only consider actual entries (not UPDATE or SAVING)
                if entry_with_tracking.entry.can_evict() {
                    let priority = entry_with_tracking.eviction_priority(self.max_idle_time);
                    eviction_candidates.push((key.clone(), priority, entry_with_tracking.total_size()));
                }
            }
        }

        // Sort by eviction priority (highest first)
        eviction_candidates.sort_by(|a, b| b.1.cmp(&a.1));

        // Evict until we have enough space
        for (key, priority, size) in eviction_candidates {
            if freed >= need_to_free {
                break;
            }

            // Save to disk before evicting
            if let Some(entry_with_tracking) = self.get_entry(&key.0, &key.1) {
                if entry_with_tracking.entry.can_evict() {
                    self.save_to_disk(&key, &entry_with_tracking).ok();
                }
            }

            // Remove from memory
            self.remove_entry(&key.0, &key.1);
            self.current_memory.fetch_sub(size, Ordering::Relaxed);
            freed += size;
        }

        if freed < need_to_free {
            log::warn!(
                "Cannot free enough memory: need {}, freed {}",
                need_to_free,
                freed
            );
        }
    }

    /// Save entry to disk
    async fn save_to_disk(&self, key: &(Vec<u64>, UserValue), entry_with_tracking: &IndexEntryWithTracking) {
        let (path, value) = key;
        let entry = &entry_with_tracking.entry;

        // Mark as SAVING
        entry.entry.mark_saving();

        // Create disk key
        let disk_key = format!(
            "__idx_entry__{}__{:?}__{}",
            self.table_name,
            path,
            entry.value_hash
        );

        // Serialize
        let serialized = bincode::serialize(&(entry.record_id, value.clone()))
            .map_err(|e| log::error!("Failed to serialize: {}", e))
            .ok();

        if let Some(bytes) = serialized {
            match self.disk_store.set(disk_key.into(), bytes.into()).await {
                Ok(_) => {
                    // Mark as ACTUAL
                    entry.entry.mark_actual();
                }
                Err(e) => {
                    log::error!("Failed to save to disk: {}", e);
                    // Keep as UPDATE so indexer retries
                    entry.entry.mark_update();
                }
            }
        }
    }

    /// Get memory statistics
    pub fn memory_stats(&self) -> MemoryStats {
        let current = self.current_memory.load(Ordering::Relaxed);
        let usage_percent = (current as f64 / self.memory_limit as f64) * 100.0;

        let mut hot_count = 0;
        let mut cold_count = 0;
        let mut dead_count = 0;

        {
            let unique_index = self.unique_index.read().unwrap();
            for entry_with_tracking in unique_index.values() {
                match entry_with_tracking.usage.get_pattern() {
                    AccessPattern::Hot => hot_count += 1,
                    AccessPattern::Warm => {}
                    AccessPattern::Cold | AccessPattern::Dead => cold_count += 1,
                    AccessPattern::Unknown => dead_count += 1,
                }
            }
        }

        MemoryStats {
            current_bytes: current,
            limit_bytes: self.memory_limit,
            usage_percent,
            entry_count: hot_count + cold_count + dead_count,
            hot_count,
            cold_count,
            dead_count,
        }
    }
}

/// Memory usage statistics
#[derive(Debug, Clone)]
pub struct MemoryStats {
    pub current_bytes: usize,
    pub limit_bytes: usize,
    pub usage_percent: f64,
    pub entry_count: usize,
    pub hot_count: usize,
    pub cold_count: usize,
    pub dead_count: usize,
}
```

---

## Usage Example

```rust
// Create store
let store = LRUIndexStore::new(
    100 * 1024 * 1024, // 100 MB
    disk_store,
    "users".to_string(),
);

// Insert record
store.insert(&[1, 2], &UserValue::Str("alice".to_string()), record_id);

// Check uniqueness (records access!)
let exists = store.contains_unique(&[1, 2], &UserValue::Str("alice".to_string()));

// Get memory stats
let stats = store.memory_stats();
println!("Memory usage: {}%", stats.usage_percent);
println!("Hot entries: {}", stats.hot_count);
println!("Cold entries: {}", stats.cold_count);
```

---

## Benefits of This Design

### 1. Separation of Concerns

- **IndexEntry**: Just the data
- **IndexUsageTracker**: Just usage metadata
- **LRUIndexStore**: Container with eviction logic

### 2. Rich Usage Tracking

- **Access frequency**: For hot/cold detection
- **Last access time**: For LRU eviction
- **Access patterns**: For optimization decisions
- **Memory size**: For accurate tracking

### 3. Smart Eviction

- **Never evict hot entries**: Even if idle
- **Prioritize cold entries**: Evict first
- **Consider entry size**: Larger entries evicted first
- **Respect state machine**: Only evict ACTUAL entries

### 4. Performance

- **Lock-free state reads**: AtomicU8 for EntryState
- **Lock-free access stats**: AtomicU64 for counters
- **Minimal locking**: Only lock for mutations

### 5. Observability

- **Memory statistics**: Know what's in memory
- **Hot/cold breakdown**: Understand access patterns
- **Access rates**: For optimization decisions

---

## Memory Overhead

Per entry:

```
IndexEntry:
  - record_id: 16 bytes
  - value: variable (10-500 bytes)
  - value_hash: 8 bytes
  - state: 1 byte (AtomicU8)
  - size_bytes: 8 bytes
  Total: ~33-523 bytes

IndexUsageTracker:
  - last_access: 8 bytes (Instant in RwLock)
  - created_at: 8 bytes
  - stats: ~32 bytes (Arc + atomics)
  - pattern: 1 byte (AtomicU8)
  Total: ~49 bytes

IndexEntryWithTracking overhead: ~64 bytes

Total per entry: ~97-587 bytes
```

**Example:**
- 10,000 entries: ~1-6 MB
- 100,000 entries: ~10-60 MB
- 1,000,000 entries: ~100-600 MB

---

## Next Steps

1. ✅ **Data structures designed** (this file)
2. ⏭️ **Implement IndexEntry** - Core data structure
3. ⏭️ **Implement IndexUsageTracker** - Usage tracking
4. ⏭️ **Implement LRUIndexStore** - Container with eviction
5. ⏭️ **Add IndexManager** - Thread-safe wrapper
6. ⏭️ **Testing** - Unit tests + benchmarks

**This is the foundation!** Once these structures are implemented, the rest is straightforward.
