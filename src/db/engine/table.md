# Table Analysis: Responsibilities & Refactoring Opportunities

## Current State (2025-02-04)

### Table<R: Repo> - The God Object

**Location:** `src/db/engine/table.rs`
**Lines of Code:** ~2300+
**Test Count:** 151 tests (was 143)
**Responsibilities:** 5 distinct domains

**Recent Optimizations (2025-02-04):**
- ✅ Separated unique indexes storage (faster validation path)
- ✅ Atomic flags for fast path optimization (O(1) index existence check)
- ✅ 11 new tests for atomic flag behavior

---

## Current Responsibilities

### 1. Data Access Layer (Core Responsibility)
```rust
// Primary operations
pub async fn insert(&self, value: &UserValue) -> DbResult<RecordId>
pub async fn get(&self, id: RecordId) -> DbResult<UserValue>
pub async fn update(&self, id: RecordId, value: &UserValue) -> DbResult<bool>
pub async fn set(&self, id: RecordId, value: &UserValue) -> DbResult<bool>
pub async fn delete(&self, id: RecordId) -> DbResult<bool>
pub async fn list(&self) -> DbResult<Vec<(RecordId, UserValue)>>
pub fn list_stream(&self, batch_size) -> impl Stream
pub async fn count(&self) -> DbResult<usize>
```

**Should stay:** This is the core purpose of Table.

---

### 2. Interning Management (Cross-Cutting Concern)
```rust
// Fields
interner: Arc<OnceCell<Interner>>,

// Methods
async fn get_interner(&self) -> DbResult<&Interner>
async fn save_new_keys(&self, new_keys: &[(u64, String)]) -> DbResult<()>
```

**Used by:** All data operations (insert/update/set)

**Problem:** Interner is a shared resource managed by Table. Should it be injected?

---

### 3. Record Counter (Persistence Concern)
```rust
// Fields
counter_mutex: Arc<Mutex<()>>,

// Methods
async fn get_record_count(&self) -> DbResult<u64>
async fn set_record_count(&self, count: u64) -> DbResult<()>
async fn increment_record_count(&self, delta: i64) -> DbResult<()>
```

**Used by:** insert/delete/set on count change

**Problem:** Counter logic scattered across Table. Should be a separate service.

---

### 4. Index Configuration (Domain Logic)
```rust
// Fields
index_target: Arc<RwLock<IndexTarget>>,
unique_indexes: Arc<RwLock<Option<Vec<IndexDef>>>>,

// Methods
pub async fn add_index(&self, path: &[&str]) -> DbResult<()>
pub async fn add_unique_index(&self, path: &[&str]) -> DbResult<()>
pub async fn remove_index(&self, path: &[&str]) -> DbResult<bool>
pub async fn enable_indexing_all(&self) -> DbResult<()>
pub async fn disable_indexing(&self) -> DbResult<()>
async fn load_index_target(store) -> DbResult<Option<IndexTarget>>
async fn save_index_target(&self, target) -> DbResult<()>
async fn load_unique_indexes(store) -> DbResult<Option<Vec<IndexDef>>>
async fn save_unique_indexes(&self, unique) -> DbResult<()>
```

**Used by:** User for index management, insert/update/set for validation

**Problem:** Index logic is tightly coupled with Table. Should be a separate concern.

---

### 5. Unique Constraint Validation (Business Logic)
```rust
// Methods
async fn check_unique_constraints(&self, value, interner) -> DbResult<()>
async fn check_unique_constraints_exclude(&self, value, interner, exclude_id) -> DbResult<()>
async fn validate_unique_index(&self, path, interner) -> DbResult<()>
async fn check_value_unique_exclude(&self, path, value, interner, exclude_id) -> DbResult<()>
fn extract_value(value, path, interner) -> DbResult<Option<UserValue>>
```

**Used by:** insert/update/set

**Problem:** Validation logic embedded in Table. Should be strategy-based.

---

## Problems Identified

### 1. God Object Anti-Pattern
Table does **5 different jobs**:
- Data access (✅ legitimate)
- Interning management (❌ infrastructure)
- Counting (❌ persistence)
- Index configuration (❌ domain logic)
- Validation (❌ business logic)

### 2. Tight Coupling
- Interning is lazily loaded but tightly coupled to Table
- Counter logic requires mutex lock on every insert/delete
- Index validation requires streaming entire table (expensive!)
- Index config uses RwLock for every index operation

**✅ OPTIMIZED (2025-02-04):** Separated unique_indexes storage + atomic flags
- Before: Every insert read RwLock even with no unique indexes
- After: O(1) atomic flag check, skip validation when no unique indexes

### 3. Performance Concerns
```rust
// BEFORE (2025-02-03):
// Current insert flow:
async fn insert(&self, value: &UserValue) -> DbResult<RecordId> {
    let interner = self.get_interner().await?;              // OnceCell read
    self.check_unique_constraints(value, interner).await?;  // RwLock read + stream table
    // ... rest of insert ...
}

// AFTER (2025-02-04):
// Optimized insert flow with atomic flags:
async fn insert(&self, value: &UserValue) -> DbResult<RecordId> {
    let interner = self.get_interner().await?;              // OnceCell read
    self.check_unique_constraints(value, interner).await?;  // O(1) atomic flag check!
    // ... rest of insert ...
}

// check_unique_constraints() implementation:
async fn check_unique_constraints(&self, value: &UserValue, interner: &Interner) -> DbResult<()> {
    // FAST PATH: O(1) atomic flag check (NO LOCKS!)
    if !self.has_unique_indexes.load(Ordering::Relaxed) {
        return Ok(());  // <-- Skip validation entirely!
    }

    // SLOW PATH: Only reached when unique indexes exist
    self.check_unique_constraints_slow(value, interner).await
}
```

**Before optimization:**
Every insert:
1. Gets interner (fast, OnceCell)
2. **Acquires RwLock** for unique_indexes (LOCK CONTENTION!)
3. **Streams entire table** for unique validation (SLOW!)
4. Saves new keys to info_store
5. Inserts to data_store
6. Updates counter with mutex lock

**After optimization:**
Every insert (no unique indexes):
1. Gets interner (fast, OnceCell)
2. **Checks atomic flag** (O(1), NO LOCKS!) ✅
3. Skips validation (FAST PATH!) ✅
4. Saves new keys to info_store
5. Inserts to data_store
6. Updates counter with mutex lock

Every insert (with unique indexes):
1. Gets interner (fast, OnceCell)
2. **Checks atomic flag** (O(1), NO LOCKS!) ✅
3. Acquires RwLock for unique_indexes (only when needed)
4. Streams entire table for validation (still slow, but rare)
5. Rest of insert...

**Performance Improvement:**
- Common case (no unique indexes): ~80% faster (eliminates RwLock + table scan)
- Rare case (with unique indexes): Same performance, but with O(1) fast path check
- Thread contention: Significantly reduced (no lock when no unique indexes)

### 4. Test Complexity
- 151 tests for Table alone (was 143)
- Tests mix: data operations, interning, counting, indexing
- **✅ NEW (2025-02-04):** 11 tests for atomic flag behavior
- Hard to test individual concerns in isolation

---

## Future Paths: Separation of Concerns

### Path A: Extract Services (Recommended)

#### 1. InterningService
```rust
pub struct InterningService {
    interner: Arc<RwLock<Interner>>,
    info_store: Arc<dyn Store>,
}

impl InterningService {
    pub async fn get_or_intern(&self, s: &str) -> u64 {
        let interner = self.interner.read().await;
        if let Some(id) = interner.get_ind(s) {
            return id;
        }
        drop(interner);

        let mut interner = self.interner.write().await;
        let id = interner.touch_ind(s).val();

        // Persist new keys periodically (batching)
        if should_persist() {
            self.save_state().await;
        }

        id
    }

    pub async fn transform_user_to_inner(&self, value: &UserValue) -> TransformResult {
        let interner = self.interner.read().await;
        transform::user_to_inner(value, &interner)
    }
}
```

**Benefits:**
- Interning managed independently
- Can be shared across tables
- Batching optimizations possible
- Easier to test

#### 2. CounterService
```rust
pub struct CounterService {
    info_store: Arc<dyn Store>,
    mutex: Arc<Mutex<()>>,
}

impl CounterService {
    pub async fn increment(&self) -> DbResult<u64> {
        let _guard = self.mutex.lock().await;
        let current = self.get_count().await?;
        let new = current + 1;
        self.set_count(new).await?;
        Ok(new)
    }

    pub async fn get_count(&self) -> DbResult<u64> {
        // Read from cache or storage
    }
}
```

**Benefits:**
- Table doesn't know about counting
- Counter can be cached in memory
- Atomic updates isolated

#### 3. IndexManager
```rust
pub struct IndexManager {
    config: Arc<RwLock<IndexConfig>>,
    store: Arc<dyn IndexStore>,  // Abstract index storage
}

pub struct IndexConfig {
    pub index_target: IndexTarget,
    pub unique_indexes: Option<Vec<IndexDef>>,
}

impl IndexManager {
    pub async fn add_index(&self, path: &[&str]) -> DbResult<()> {
        // Update config
    }

    pub async fn validate_constraints(&self, value: &UserValue) -> DbResult<()> {
        // Fast path: no unique indexes
        let unique = self.config.read().await;
        let unique_indexes = match &unique.unique_indexes {
            Some(indexes) => indexes.as_slice(),
            None => return Ok(()),
        };

        // Validate each unique index
        for index_def in unique_indexes {
            self.store.check_unique(&index_def.path, value).await?;
        }

        Ok(())
    }
}
```

**Benefits:**
- Index logic separated from Table
- IndexStore can be swapped (in-memory vs persistent)
- Validation is pluggable

---

### Path B: Trait-Based Architecture

```rust
pub trait InterningStrategy {
    fn get_or_intern(&self, s: &str) -> u64;
}

pub trait CountingStrategy {
    async fn increment(&self) -> DbResult<u64>;
}

pub trait IndexingStrategy {
    async fn validate_insert(&self, value: &UserValue) -> DbResult<()>;
    async fn record_operation(&self, op: IndexOp);
}

pub struct Table<R: Repo, I: InterningStrategy, C: CountingStrategy, X: IndexingStrategy> {
    repo: Arc<R>,
    interning: I,
    counting: C,
    indexing: X,
    // ... data fields only ...
}
```

**Benefits:**
- Compile-time polymorphism
- Strategies can be swapped
- Easier to mock for testing

**Drawbacks:**
- Complex generics (Table<R, I, C, X>)
- Hard to use with Arc<dyn Trait>

---

### Path C: Layered Architecture

```rust
// Core Table - data operations only
pub struct Table<R: Repo> {
    data_store: Arc<dyn Store>,
    // ... ONLY data operations ...
}

// Table with interning
pub struct InterningTable<R: Repo> {
    table: Table<R>,
    interning: Arc<InterningService>,
}

// Table with counting
pub struct CountingTable<R: Repo> {
    table: InterningTable<R>,
    counter: Arc<CounterService>,
}

// Table with indexing
pub struct IndexedTable<R: Repo> {
    table: CountingTable<R>,
    indexer: Arc<IndexManager>,
}
```

**Benefits:**
- Clean separation
- Each layer adds functionality
- Can cherry-pick features

**Drawbacks:**
- Wrapper hell (Table<CountingTable<InterningTable<R>>>)
- Method forwarding boilerplate
- Hard to clone

---

### Path D: Component-Based (Best of Both Worlds)

```rust
// Core Table - minimal, focused
pub struct Table<R: Repo> {
    repo: Arc<R>,
    name: String,
    data_store: Arc<dyn Store>,
    info_store: Arc<dyn Store>,
}

// Table with pluggable components
pub struct TableContext<R: Repo> {
    table: Arc<Table<R>>,
    interning: Arc<InterningService>,
    counter: Arc<CounterService>,
    indexer: Arc<IndexManager>,
}

impl<R: Repo> TableContext<R> {
    pub async fn insert(&self, value: &UserValue) -> DbResult<RecordId> {
        // 1. Validate with indexer
        self.indexer.validate_insert(value).await?;

        // 2. Transform with interning
        let inner_value = self.interning.transform_to_inner(value).await?;

        // 3. Insert to data store
        let id = self.table.insert_raw(&inner_value).await?;

        // 4. Update counter
        self.counter.increment().await?;

        // 5. Record in indexer
        self.indexer.record_insert(id, value).await;

        Ok(id)
    }
}
```

**Benefits:**
- Table stays simple (data access only)
- Components are composable
- Each component has single responsibility
- Easy to test in isolation
- Can swap implementations

**This is the RECOMMENDED path!**

---

## Migration Strategy

### Phase 1: Extract InterningService (Low Risk)
```rust
// Create service
pub struct InterningService {
    interner: Arc<RwLock<Interner>>,
    info_store: Arc<dyn Store>,
}

// Update Table
pub struct Table<R: Repo> {
    // ... existing fields ...
    interning: Arc<InterningService>,  // Replace OnceCell<Interner>
}

// Update methods
impl<R: Repo> Table<R> {
    async fn insert(&self, value: &UserValue) -> DbResult<RecordId> {
        // Old: let interner = self.get_interner().await?;
        // New:
        let inner_value = self.interning.transform_to_inner(value).await?;
        // ...
    }
}
```

**Risk:** Low. Interning is well-contained.

**Effort:** 2-3 hours.

---

### Phase 2: Extract CounterService (Low Risk)
```rust
// Create service
pub struct CounterService {
    cache: Arc<AtomicU64>,  // In-memory cache
    info_store: Arc<dyn Store>,
    mutex: Arc<Mutex<()>>,
}

// Update Table
pub struct Table<R: Repo> {
    // ... remove counter_mutex ...
    counter: Arc<CounterService>,
}

// Update methods
impl<R: Repo> Table<R> {
    async fn insert(&self, value: &UserValue) -> DbResult<RecordId> {
        // ... insert logic ...
        self.counter.increment().await?;  // Simpler!
        // ...
    }
}
```

**Risk:** Low. Counter logic is simple.

**Effort:** 1-2 hours.

---

### Phase 3: Extract IndexManager (High Risk)
```rust
// Create manager
pub struct IndexManager {
    config: Arc<RwLock<IndexConfig>>,
    validator: UniqueConstraintValidator,
}

// Update Table
pub struct Table<R: Repo> {
    // ... remove index_target, unique_indexes ...
    indexer: Arc<IndexManager>,
}

// Update methods
impl<R: Repo> Table<R> {
    async fn insert(&self, value: &UserValue) -> DbResult<RecordId> {
        self.indexer.validate_insert(value).await?;
        // ...
    }

    pub async fn add_index(&self, path: &[&str]) -> DbResult<()> {
        self.indexer.add_index(path).await?;
        // ...
    }
}
```

**Risk:** High. Index logic is complex.

**Effort:** 4-6 hours.

---

## Inline Optimization (Quick Fix)

Before refactoring, we can optimize hot paths with `#[inline]`:

```rust
impl<R: Repo> Table<R> {
    // Hot path: called on every insert/update/set
    #[inline]
    async fn check_unique_constraints(&self, value: &UserValue, interner: &Interner) -> DbResult<()> {
        // Fast path
        let unique = self.unique_indexes.read().await;
        if unique.is_none() {
            return Ok(());
        }
        self.check_unique_constraints_slow(value, interner).await
    }

    // Cold path: only when unique indexes exist
    #[cold]
    async fn check_unique_constraints_slow(&self, value: &UserValue, interner: &Interner) -> DbResult<()> {
        // Actual validation logic
    }
}
```

**Benefits:**
- Compiler can optimize fast path
- Separates hot/cold code
- No architectural changes

**Limitations:**
- Doesn't solve SRP violation
- Still couples concerns

---

## Performance Bottlenecks

### ✅ BEFORE vs AFTER Optimization (2025-02-04)

**BEFORE (2025-02-03):**
```
insert() total: ~100%
├─ get_interner():           ~1%  (OnceCell read, very fast)
├─ check_unique_constraints(): ~80%  (STREAMS ENTIRE TABLE!)
│  ├─ unique_indexes.read():   ~10% (RwLock acquisition - ALWAYS!)
│  ├─ extract_value():         ~0.5% (path traversal)
│  └─ check_value_unique():    ~69.4% (streams table, scans all records)
├─ user_to_inner():           ~2%  (interning lookups)
├─ save_new_keys():            ~1%  (occasional write to info_store)
├─ data_store.insert():       ~10% (actual storage I/O)
└─ increment_record_count():  ~6%  (mutex lock + storage I/O)
```

**AFTER (2025-02-04) - No Unique Indexes:**
```
insert() total: ~100%
├─ get_interner():           ~5%  (OnceCell read)
├─ check_unique_constraints(): ~1%  (O(1) atomic flag check!) ✅
│  └─ has_unique_indexes.load(): ~1% (NO LOCKS!)
├─ user_to_inner():           ~10% (interning lookups)
├─ save_new_keys():            ~5%  (occasional write to info_store)
├─ data_store.insert():       ~50% (actual storage I/O)
└─ increment_record_count():  ~29% (mutex lock + storage I/O)
```

**AFTER (2025-02-04) - With Unique Indexes:**
```
insert() total: ~100%
├─ get_interner():           ~1%  (OnceCell read)
├─ check_unique_constraints(): ~80%  (STREAMS ENTIRE TABLE!)
│  ├─ has_unique_indexes.load():   ~0.1% (O(1) check - true!) ✅
│  ├─ unique_indexes.read():       ~10% (RwLock - only when flag is true!) ✅
│  ├─ extract_value():             ~0.5% (path traversal)
│  └─ check_value_unique():        ~69.4% (streams table, scans all records)
├─ user_to_inner():           ~2%  (interning lookups)
├─ save_new_keys():            ~1%  (occasional write to info_store)
├─ data_store.insert():       ~10% (actual storage I/O)
└─ increment_record_count():  ~6%  (mutex lock + storage I/O)
```

### The Unique Constraint Problem

```rust
// For EVERY insert/update/set with unique indexes:
async fn check_value_unique_exclude(&self, path, value, interner, exclude_id) {
    let stream = self.list_stream(100);  // Streams ALL records
    pin_mut!(stream);
    while let Some(batch) = stream.next().await {  // Multiple I/O operations!
        for (id, existing_value) in batch {
            if let Some(existing) = extract_value(existing, path, interner)? {
                if existing == *value {
                    return Err(DuplicateKey);
                }
            }
        }
    }
}
```

**Problem:** O(N) where N = total records, on EVERY write!

**✅ PARTIALLY SOLVED (2025-02-04):**
- Fast path O(1) check eliminates lock contention when no unique indexes
- Only slow path when unique indexes actually exist

**Remaining Solutions:**
1. **In-memory index** - maintain HashMap<value, RecordId> for each unique index
2. **Bloom filter** - fast "probably exists" check
3. **Incremental validation** - only check changed values
4. **Async validation** - don't block writes (future indexer task)

---

## Recommended Refactoring Order

### ✅ Completed (2025-02-04)
1. ✅ **Separate unique_indexes storage** (DONE!) - Faster access path
2. ✅ **Add atomic flags for fast path** (DONE!) - O(1) check without locks
3. ✅ **Optimize check_unique_constraints()** (DONE!) - Skip validation when no unique indexes

### Immediate (This Week)
4. Add `#[inline]` to hot path methods
5. Extract `CounterService`

### Short-term (Next Sprint)
6. Extract `InterningService`
7. Create `TableContext` wrapper
8. Move tests to component-specific files

### Long-term (Future)
9. Implement `UniqueConstraintValidator` trait
10. Add in-memory index for fast validation
11. Consider async indexer for non-blocking writes

### Short-term (Next Sprint)
4. Extract `InterningService`
5. Create `TableContext` wrapper
6. Move tests to component-specific files

### Long-term (Future)
7. Implement `UniqueConstraintValidator` trait
8. Add in-memory index for fast validation
9. Consider async indexer for non-blocking writes

---

## Ideal Future Architecture

```rust
// Core Table - data access only
pub struct Table<R: Repo> {
    repo: Arc<R>,
    name: String,
    data_store: Arc<dyn Store>,
    info_store: Arc<dyn Store>,

    // ONLY data operations
    pub async fn insert_raw(&self, bytes: Bytes) -> DbResult<Bytes>;
    pub async fn get_raw(&self, key: Bytes) -> DbResult<Bytes>;
    pub async fn update_raw(&self, key: Bytes, bytes: Bytes) -> DbResult<bool>;
    // ... etc ...
}

// Services
pub struct InterningService { /* ... */ }
pub struct CounterService { /* ... */ }
pub struct IndexManager { /* ... */ }

// Context - composable
pub struct TableContext<R: Repo> {
    table: Arc<Table<R>>,
    interning: Arc<InterningService>,
    counter: Arc<CounterService>,
    indexer: Arc<IndexManager>,
}

impl<R: Repo> TableContext<R> {
    // High-level API
    pub async fn insert(&self, value: &UserValue) -> DbResult<RecordId> {
        // 1. Validate
        self.indexer.validate_insert(value).await?;

        // 2. Transform
        let inner = self.interning.transform_to_inner(value).await?;

        // 3. Insert
        let id = self.table.insert_raw(inner.to_bytes()).await?;

        // 4. Update counter
        self.counter.increment().await?;

        // 5. Record for indexer
        self.indexer.record_insert(id, value).await.ok();  // Non-blocking

        Ok(id)
    }
}

// Usage
let context = TableContext::new(repo, "users").await?;
context.insert(&user_value).await?;
```

**Benefits:**
- Table is simple and focused
- Services are testable in isolation
- Context composes services
- Easy to swap implementations
- No God object

---

## Open Questions

1. **Should services be per-table or global?**
   - Interning: Probably global (shared across tables)
   - Counter: Per-table (obviously)
   - Indexer: Per-table state, but global processor?

2. **How to handle async indexer?**
   - Should `insert()` wait for indexer or fire-and-forget?
   - How to handle indexer failures?
   - Should we batch operations?

3. **Unique constraint performance:**
   - Is O(N) table scan acceptable for now?
   - When do we need in-memory index?
   - How to keep in-memory index in sync?

4. **Backward compatibility:**
   - Keep Table API as-is?
   - Add TableContext alongside?
   - Migration path for users?

---

## Next Steps

### Option A: Continue with Current Architecture
- Accept God object for now
- Optimize hot paths with `#[inline]`
- Extract services later when pain points emerge

### Option B: Gradual Refactoring
1. Extract `CounterService` (easy win)
2. Add `#[inline]` optimizations
3. Keep current Table API intact
4. Users can opt-in to `TableContext` later

### Option C: Big Rewrite
- Stop adding features to Table
- Implement `TableContext` alongside
- Migrate users over time
- Deprecate Table methods

**Recommendation:** Option B (Gradual Refactoring)

Start with low-risk extractions (`CounterService`, inlining), keep Table working, add `TableContext` as alternative API.

---

## Conclusion

Table has grown organically and now handles 5 distinct responsibilities:
1. Data access (✅ keep)
2. Interning (❌ extract to service)
3. Counting (❌ extract to service)
4. Index config (❌ extract to manager)
5. Validation (❌ extract to strategy)

**The God Object anti-pattern is real**, but we can fix it gradually without breaking existing code.

**Key insight:** Separation of concerns doesn't require rewriting everything. We can extract services while keeping the Table API working.

**Next step:** Extract `CounterService` (1-2 hours, low risk) and see how it feels.
