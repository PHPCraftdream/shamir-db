# Table Engine

High-level table abstraction with automatic key interning, value transformations, and index management.

**Updated:** 2026-02-22

## Architecture

```
engine/
├── dispatcher/               # ✅ Multi-repo dispatcher
│   ├── config.rs             # YAML configuration loader
│   ├── dispatcher.rs         # Dispatcher implementation
│   ├── types.rs              # Config types (DbConfig, RepoConfig, etc.)
│   └── tests/                # Dispatcher tests
│       ├── config_loader_tests.rs      # YAML roundtrip tests
│       ├── config_validation_tests.rs  # Validation rule tests
│       └── dispatcher_tests.rs          # Dispatcher logic tests
├── index/                    # Index management system
│   ├── index_definition.rs   # Index definition (simple/composite)
│   ├── index_info.rs         # Index metadata with sync status
│   ├── index_info_item.rs    # Single index path item
│   ├── index_record_key.rs   # Index record key for B-Tree storage
│   ├── index_status.rs       # Index sync status enum
│   ├── index_manager.rs      # ✅ RENAMED from table_index_manager (~1020 lines)
│   ├── index_types.md        # Documentation for index types
│   └── tests/                # Index tests (56 tests)
│       ├── mod.rs
│       ├── index_definition_tests.rs
│       ├── index_info_item_tests.rs
│       ├── index_info_tests.rs
│       ├── index_record_key_tests.rs
│       └── index_manager_tests.rs    # 56 tests (41 regular + 15 unique)
├── repo/                     # Repo management
│   ├── repo_config.rs        # Repo configuration types
│   ├── repo_manager.rs      # RepoManager (manages repos)
│   ├── repo_manager_instance.rs # RepoManagerInstance
│   ├── repo_types.rs        # Repo types
│   └── tests/                # Repo tests
│       ├── mod.rs
│       ├── repo_config_tests.rs
│       └── repo_manager_tests.rs
├── table/                    # Modular table architecture
│   ├── counter.rs           # RecordCounter service
│   ├── interner_manager.rs  # InternerManager service
│   ├── table.rs             # Main Table facade
│   ├── table_config.rs      # Table configuration
│   ├── table_context.rs     # Table with index integration
│   ├── mod.rs               # Public API exports
│   └── tests/              # Table tests
│       ├── mod.rs
│       ├── crud_tests.rs
│       ├── concurrent_tests.rs
│       └── persistence_tests.rs
├── README.md     # Engine documentation
└── table.md      # Table refactoring documentation
```

## Purpose

Bridges the gap between raw storage (`Store`) and user-friendly API:
- **Dispatcher**: Manages multiple repositories with YAML configuration
- **RepoManager**: Manages tables within repositories with lazy initialization
- Manages key interning transparently
- Transforms UserValue ↔ InnerValue
- Provides async streaming for large datasets
- Handles table metadata via system records
- Test organization: one entity per file in `tests/` folders

## Architecture

Each table consists of two underlying stores:
```
__data__{table_name}  → actual data with InnerValue (u64 keys)
__info__{table_name}  → interning state + future indexes
```

## System Records

The interner is stored as system records in `__info__` store:

```rust
// Interning state
RecordId::system("internals") → Map<String, u64>  // String → ID mapping
RecordId::system("inter_max")   → u64               // Next available ID

// Future use:
RecordId::system("indexes")    → Index metadata
```

## Index System

The index system provides efficient data lookup with three indexing modes:

### Index Modes

```rust
pub enum IndexMode {
    Disabled,                      // No indexing
    All,                           // Index all Map fields (simple indexes)
    Selective(Vec<IndexDefinition>), // Custom indexes
}
```

### Index Definition

Indexes can be **simple** (single path) or **composite** (multiple paths):

```rust
// Simple index (name_interned is u64 from interner)
let email_idx = IndexDefinition::new(email_name_interned, vec![
    IndexInfoItem::new(vec![2])  // Path to email field
]);

// Composite index
let name_age_idx = IndexDefinition::new(name_age_name_interned, vec![
    IndexInfoItem::new(vec![1]),  // Path to name field
    IndexInfoItem::new(vec![3])   // Path to age field
]);
```

### IndexManager

The `IndexManager` handles index operations:

- **Atomic flags** for O(1) existence check (no locks!)
- **Two index types**: regular and unique
- **Unique indexes**: validation BEFORE write, update AFTER write
- **Status tracking**: Actual, Pending, Saving

```rust
pub struct IndexManager {
    data_store: Arc<dyn Store>,
    info_store: Arc<dyn Store>,
    indexes: Arc<IndexInfo>,           // Regular indexes (DashMap-based)
    indexes_unique: Arc<IndexInfo>,    // Unique indexes
    has_indexes: Arc<AtomicBool>,      // O(1) check!
    has_indexes_unique: Arc<AtomicBool>, // O(1) check!
}
```

### Index Operations

```rust
// Add index
table.add_index(&["email"]).await?;

// Add unique index
table.add_unique_index(&["username"]).await?;

// Check if has indexes (O(1) - no locks!)
if table.has_indexes() {
    // Use indexes for query
}

// Remove index
table.remove_index(&["email"]).await?;

// Enable/disable indexing
table.enable_indexing_all().await?;
table.disable_indexing().await?;
```

### Performance Optimization

The index system uses atomic flags for fast path optimization:

```rust
// Before: O(N) with locks even when no indexes
if let Some(indexes) = self.unique_indexes.read().await {
    // Validate...
}

// After: O(1) without locks when no indexes
if !self.has_indexes_unique.load(Ordering::Relaxed) {
    return Ok(()); // Skip validation!
}
// Only acquire lock when indexes actually exist
```

## Table Structure

**Updated: 2025-02-08 - Modular Architecture**

```rust
// Main Table facade (src/db/engine/table/table.rs)
pub struct Table<R: Repo> {
    repo: Arc<R>,
    table_name: String,
    data_store: Arc<dyn Store>,
    info_store: Arc<dyn Store>,

    // Service components (wrapped in Arc for shared state)
    interner: Arc<InternerManager>,
    counter: Arc<RecordCounter>,
}

// RecordCounter service (src/db/engine/table/counter.rs)
pub struct RecordCounter {
    info_store: Arc<dyn Store>,
    counter_mutex: Mutex<()>,
}

// InternerManager service (src/db/engine/table/interner.rs)
pub struct InternerManager {
    info_store: Arc<dyn Store>,
    interner: OnceCell<Interner>,
}
```

### Lazy Loading

The interner is loaded on first access:

```rust
async fn get_interner(&self) -> DbResult<&Interner> {
    self.interner.get_or_try_init(|| async {
        // Load from __info__ store
        self.load_interner().await
    }).await
}
```

**Benefits:**
- Faster startup (no interner load if not used)
- Lower memory usage
- Thread-safe via `tokio::sync::OnceCell`

## Value Transformations

### UserValue → InnerValue (Insert)

```rust
let user_value: UserValue = Value::Object(map![
    ("name".into(), Value::Str("Alice".into())),
    ("email".into(), Value::Str("alice@example.com".into()))
]);

// Intern string keys and values
let inner_value: InnerValue = user_to_inner(&user_value, interner);
// Value::Object(map![1, 2]) where 1="name", 2="alice@example.com"
```

### InnerValue → UserValue (Read)

```rust
let inner_value: InnerValue = Value::Object(map![1, 2]);

// Reverse lookup via interner
let user_value: UserValue = inner_to_user(&inner_value, interner);
// Value::Object(map![("name", "Alice"), ("email", "...")])
```

## API

### Basic Operations

```rust
use shamir_db::db::engine::Table;
use shamir_db::db::storage::storage_sled::SledRepo;
use shamir_db::types::value::Value;

// Open table
let repo = SledRepo::new("./db")?;
let table = repo.table_get("users")?;

// Insert record
let user = Value::Object(map![
    ("name".into(), Value::Str("Alice".into())),
    ("age".into(), Value::Int(30))
]);
let id = table.insert(user).await?;

// Get record
let retrieved = table.get(id).await?;

// Update record
table.set(id, new_value).await?;

// Delete record
table.remove(id).await?;
```

### Streaming (Memory-Efficient!)

```rust
// Stream all records in batches
let mut stream = table.list_stream(100); // 100 records per batch

while let Some(batch_result) = stream.next().await {
    let batch = batch_result?;

    for (id, record) in batch {
        println!("{}: {:?}", id, record);
    }
}

// No OOM! Constant memory usage regardless of dataset size.
```

### Batch Size Configuration

```rust
// Set custom batch size
let mut table = repo.table_get("users")?;
table.set_batch_size(500); // Larger batches for throughput

// Or use default (100)
let stream = table.list_stream(100);
```

## Concurrency

Table is fully thread-safe:

```rust
// Clone table (cheap - Arc-based)
let table1 = table.clone();
let table2 = table.clone();

// Concurrent inserts from multiple threads
let t1 = tokio::spawn(async move {
    table1.insert(value1).await
});

let t2 = tokio::spawn(async move {
    table2.insert(value2).await
});

let (id1, id2) = tokio::join!(t1, t2);
```

**Thread Safety Guarantees:**
- ✅ Multiple concurrent reads
- ✅ Multiple concurrent writes
- ✅ Interner is synchronized (DashMap)
- ✅ OnceCell ensures single init

## Interning Flow

### On Insert

1. User provides `UserValue<String>`
2. Extract all strings (keys, string values)
3. For each string:
   - Check if already interned
   - If no: assign new ID
   - If yes: reuse existing ID
4. Store with `InnerValue<u64>` (more compact)
5. Update system records in `__info__` store

### On Read

1. Read `InnerValue<u64>` from storage
2. For each u64:
   - Reverse lookup in interner
   - Convert back to original string
3. Return `UserValue<String>`

## Metadata

### Table Name
```rust
table.name() // Returns "users"
```

### Record Count
```rust
// Via iteration
let count = table.list_stream(1000)
    .map(|batch| {
        batch.map(|b| b.len()).sum::<usize>()
    })
    .sum::<usize>().await;
```

## System Records Format

`__info__{table}` store structure:

```
Key                                    Value
────────────────────────────────────────────────────────────────────
RecordId::system("internals")          bincoded map<String, u64>
RecordId::system("inter_max")           bincoded u64
RecordId::system("inter_created_at")   bincoded DateTime (future)
```

## Error Handling

```rust
use shamir_db::db::error::DbError;

match table.insert(value).await {
    Ok(id) => println!("Inserted: {:?}", id),
    Err(DbError::NotFound(msg)) => eprintln!("Not found: {}", msg),
    Err(DbError::Storage(msg)) => eprintln!("Storage error: {}", msg),
    Err(DbError::Codec(msg)) => eprintln!("Codec error: {}", msg),
}
```

## Performance

### Interning Benefits

| Dataset | Without Interning | With Interning | Savings |
|---------|------------------|----------------|---------|
| 1M objects with 10 string fields each | ~500 MB | ~150 MB | **70%** |

### Streaming Performance

| Dataset Size | Memory Usage (iter) | Memory Usage (list_stream) |
|--------------|---------------------|----------------------------|
| 1K records | ~1 MB | ~1 MB |
| 1M records | **OOM!** | ~1 MB ✅ |
| 1B records | **CRASH!** | ~1 MB ✅ |

## Implementation Notes

### Clone Behavior
```rust
let table2 = table.clone(); // Cheap! Only clones Arc pointers
```

All fields are `Arc` wrapped - clones are shallow copies.

### Interner Lifetime
Interned strings live as long as the table exists:
- Stored in system records
- Survives restarts
- Shared across all table instances

### Future Enhancements

- [x] ✅ **Multi-repo dispatcher** (2025-02-08)
  - Dispatcher manages multiple RepoManagers
  - YAML configuration with ConfigLoader (from core::config)
  - Atomic file writes for safe updates
- [x] ✅ **Repo management** (2025-02-08)
  - RepoManager for repository operations
  - Lazy table initialization
  - Default repo support
- [x] ✅ **Modular architecture** (2025-02-08)
  - RecordCounter, InternerManager separated
  - Tests organized by type
  - **280 tests passing**
- [x] ✅ **Test reorganization** (2025-02-08)
  - Tests in separate `tests/` folders
  - One entity per file
  - Names match content
- [x] ✅ **Index system** (2026-02-22)
  - Simple and composite indexes
  - Unique constraint validation (BEFORE write)
  - Atomic flags for O(1) existence check
  - **56 index tests**
  - `UniqueIndexCreationFailed` error with duplicate count
- [ ] Garbage collection for unused interned strings
- [ ] Automatic batch size tuning
- [ ] Statistics (record count, interned strings count)
- [ ] Query planner integration
- [ ] Transaction support across tables
