# Database Layer

S.H.A.M.I.R. database abstraction layer, providing table management, indexing system, and storage abstraction.

## Architecture

```
db/
├── engine/           # Table engine (high-level API)
│   ├── dispatcher/   # ✅ NEW (2025-02-08) - Multi-repo dispatcher
│   │   ├── config.rs          # YAML configuration loader
│   │   ├── dispatcher.rs      # Dispatcher (manages multiple repos)
│   │   ├── types.rs           # Config types (DbConfig, RepoConfig, etc.)
│   │   └── tests/             # Dispatcher tests
│   │       ├── config_loader_tests.rs
│   │       ├── config_validation_tests.rs
│   │       └── dispatcher_tests.rs
│   ├── index/        # Index management system
│   │   ├── index_definition.rs   # Index definition (simple/composite)
│   │   ├── index_info.rs         # Index metadata & status
│   │   ├── index_info_item.rs    # Single index path item
│   │   ├── index_record.rs       # Index record representation
│   │   ├── index_manager.rs      # Index manager for tables (renamed from table_index_manager)
│   │   └── tests/              # ✅ REORGANIZED (2025-02-08)
│   │       ├── mod.rs
│   │       ├── index_definition_tests.rs
│   │       ├── index_info_item_tests.rs
│   │       ├── index_info_tests.rs
│   │       ├── index_record_tests.rs
│   │       └── index_manager_tests.rs
│   ├── table/        # ✅ MODULARIZED (2025-02-08)
│   │   ├── counter.rs           # RecordCounter service
│   │   ├── interner.rs          # InternerManager service
│   │   ├── table.rs             # Main Table facade
│   │   ├── mod.rs               # Public API exports
│   │   └── tests/              # Organized test suites
│   │       ├── mod.rs
│   │       ├── crud_tests.rs
│   │       ├── concurrent_tests.rs
│   │       └── persistence_tests.rs
│   ├── repo/         # ✅ NEW (2025-02-08) - Repo management
│   │   ├── repo_config.rs       # Repo configuration types
│   │   ├── repo_manager.rs     # RepoManager (manages repos)
│   │   ├── repo_manager_instance.rs # RepoManagerInstance
│   │   ├── repo_types.rs       # Repo types
│   │   └── tests/              # Repo tests
│   │       ├── mod.rs
│   │       ├── repo_config_tests.rs
│   │       └── repo_manager_tests.rs
│   ├── README.md     # Engine documentation
│   └── table.md     # Table refactoring documentation
├── storage/          # Storage abstraction (low-level)
│   ├── types.rs      # Store and Repo traits
│   ├── storage_in_memory.rs  # In-memory store (for testing)
│   ├── storage_cached.rs     # Cached store wrapper (sync/async modes)
│   ├── storage_sled.rs
│   ├── storage_redb.rs
│   ├── storage_fjall.rs
│   ├── storage_nebari.rs
│   ├── storage_persy.rs
│   ├── storage_canopy.rs
│   └── README.md     # Storage documentation
├── mod.rs
└── error.rs          # DbError, DbResult types
```

## Components

### Engine (`db/engine/`)
**High-level table API** with automatic interning and index management:
- `Dispatcher` - ✅ NEW: Multi-repo management with YAML configuration (2025-02-08)
- `RepoManager` - ✅ NEW: Repository and table management (2025-02-08)
- `Table<R>` - Main table abstraction (modularized 2025-02-08)
- `IndexManager` (formerly TableIndexManager) - Index management system
- `RecordCounter` - Counter service (separate module)
- `InternerManager` - Interning service (separate module)
- Manages key interning transparently
- Transforms UserValue ↔ InnerValue
- Provides memory-efficient async streaming
- **56 index tests**, UniqueIndexCreationFailed error

**New Modular Structure (2025-02-08):**
```
engine/
├── dispatcher/           # Multi-repo dispatcher
│   ├── config.rs         # YAML configuration loader
│   ├── dispatcher.rs     # Dispatcher implementation
│   ├── types.rs          # Config types
│   └── tests/            # 8 config tests + dispatcher tests
├── repo/                 # Repo management
│   ├── repo_config.rs    # Repo config types
│   ├── repo_manager.rs  # RepoManager
│   └── tests/            # 14 repo tests
├── table/                # Table implementation
│   ├── counter.rs        # RecordCounter (5 tests)
│   ├── interner.rs       # InternerManager (5 tests)
│   ├── table.rs          # Table facade
│   └── tests/            # 25 table tests (CRUD, concurrent, persistence)
└── index/                # Index system
    ├── index_definition.rs
    ├── index_info.rs
    ├── index_record.rs
    ├── table_index_manager.rs
    └── tests/            # 56 index tests
```

See `engine/README.md` for details.

### Index System (`db/engine/index/`)
**Index management** for tables:
- `IndexManager` (renamed from TableIndexManager) - Index operations and validation
- `IndexDefinition` - Simple and composite index definitions (name_interned: u64)
- `IndexInfo` - Index metadata with sync status tracking
- Atomic flags for fast path optimization (O(1) existence check)
- Unique indexes: validation BEFORE write, update AFTER write
- UniqueIndexCreationFailed(name, count, sample) error for duplicates
- Three indexing modes: Disabled, All, Selective
- **56 tests** organized by entity in `tests/` folder

### Dispatcher (`db/engine/dispatcher/`)
**Multi-repo management** with YAML configuration:
- `Dispatcher` - Manages multiple RepoManagers
- `ConfigLoader` (from `core::config`) - Load/save YAML configuration files
- `DbConfig`, `RepoConfig`, `TableConfig`, `IndexConfig` - Configuration types
- Atomic file writes (temp + rename) for safe updates
- Validation on load (ensures config correctness)
- **Hot-reload ready** - web interface can update config atomically

### Repo Manager (`db/engine/repo/`)
**Repository and table management**:
- `RepoManager` - Manages repositories (collections of tables)
- `RepoManagerInstance` - Holds Arc<Repo> with lazy table initialization
- `RepoConfig` - Repository configuration
- Default repo support
- CRUD operations for repos
- **14 tests** for repo management

### Storage (`db/storage/`)
**Low-level storage abstraction** over 8 embedded databases:
- Pluggable backends (InMemory, Cached, Sled, Redb, Fjall, Nebari, Persy, Canopy)
- Unified `Store` trait for key-value operations
- Unified `Repo` trait for multi-store management
- Async streaming with batch generators
- Prefix scan operations for composite keys

**New Storage Options:**
- **InMemoryStore** - Pure in-memory storage for testing/caching
- **CachedStore** - Wrapper with write-through or write-behind modes

See `storage/README.md` for details.

## Error Handling

All database operations return `DbResult<T>`:

```rust
pub type DbResult<T> = Result<T, DbError>;

pub enum DbError {
    Storage(String),           // Backend-specific error
    NotFound(String),          // Key/table doesn't exist
    Codec(String),             // Serialization error
    Internal(String),          // Internal logic error
    KeyExists(String),         // Primary key collision
    DuplicateKey(String),      // Unique index violation
    UniqueIndexCreationFailed(String, usize, String), // (name, count, sample)
}
```

## Usage Flow

### 1. Open Repository

```rust
use shamir_db::db::storage::storage_sled::SledRepo;
use shamir_db::db::storage::types::Repo;

let repo = SledRepo::new("./my_db")?;
```

### 2. Get Table

```rust
use shamir_db::db::engine::Table;

let table = repo.table_get("users")?;
```

**What happens:**
1. Opens `__data__users` store for records
2. Opens `__info__users` store for metadata
3. Creates or loads interner from system records
4. Returns `Table` handle

### 3. Use Table

```rust
use shamir_db::types::value::Value;

// Insert (interns strings automatically)
let user = Value::Object(map![
    ("name".into(), Value::Str("Alice".into())),
    ("email".into(), Value::Str("alice@example.com".into()))
]);
let id = table.insert(user).await?;

// Read (reverse interning)
let retrieved = table.get(id).await?;

// Stream (memory-efficient!)
let mut stream = table.list_stream(100);
while let Some(batch) = stream.next().await {
    for (id, record) in batch? {
        println!("{}: {:?}", id, record);
    }
}

// Index operations
table.add_index(&["email"]).await?;
table.add_unique_index(&["username"]).await?;

// Query with prefix scan stream (for index lookups)
let mut stream = store.scan_prefix_stream(b"idx:email:".to_vec().into(), 100);
while let Some(batch_result) = stream.next().await {
    let batch = batch_result?;
    for (key, value) in batch {
        // Process matching records
    }
}
```

## Storage Layout

Each table creates 2 underlying stores:

```
my_db/
├── __data__users          # User records (InnerValue<u64>)
├── __info__users          # Metadata (interning state)
├── __data__posts
├── __info__posts
└── ...
```

### Data Store (`__data__{table}`)
Contains actual user records:
- Key: `RecordId` (16 bytes)
- Value: `InnerValue<u64>` (interned)
- Example: `a1b2... → Object{1: 2, 3: 4}`

### Info Store (`__info__{table}`)
Contains metadata:
- `RecordId::system("internals")` → `Map<String, u64>`
- `RecordId::system("inter_max")` → `u64`
- `RecordId::system("indexes")` → `IndexInfo` (index definitions)
- `RecordId::system("indexes_unique")` → `IndexInfo` (unique constraints)
- Future: statistics, etc.

## Async Flow

### Insert Operation

```rust
table.insert(user_value).await?;
```

**Flow:**
1. Acquire interner (lazy load if needed)
2. Transform `UserValue<String>` → `InnerValue<u64>`
   - Extract all strings
   - Intern them (assign IDs or reuse)
3. Serialize `InnerValue` to bytes
4. Call `store.insert(bytes)`
5. Update interner in `__info__` store
6. Return `RecordId`

### Read Operation

```rust
table.get(id).await?;
```

**Flow:**
1. Acquire interner (lazy load if needed)
2. Call `store.get(id)` → bytes
3. Deserialize bytes → `InnerValue<u64>`
4. Transform `InnerValue<u64>` → `UserValue<String>`
   - Reverse lookup all u64 IDs
   - Convert back to strings
5. Return `UserValue`

### Stream Operation

```rust
table.list_stream(batch_size)
```

**Flow:**
1. Acquire interner once
2. Get stream from storage: `store.iter_stream(batch_size)`
3. For each batch:
   - Deserialize bytes → `InnerValue<u64>`
   - Transform → `UserValue<String>`
   - Yield batch to consumer
4. Consumer processes batches lazily

## Concurrency Model

### Thread Safety

All components are thread-safe:

```rust
// Clone table (cheap - Arc-based)
let t1 = table.clone();
let t2 = table.clone();

// Concurrent operations
tokio::join!(
    async {
        t1.insert(value1).await
    },
    async {
        t2.insert(value2).await
    }
);
```

**Guarantees:**
- ✅ Multiple concurrent reads
- ✅ Multiple concurrent writes
- ✅ Safe interned ID assignment (DashMap)
- ✅ Lazy loading with OnceCell (single init)

### Interning Synchronization

- **DashMap**: Lock-free reads, fine-grained write locks
- **OnceCell**: Ensures single initialization
- **Atomic**: Next ID assignment

## Performance Considerations

### When to Use Tables vs Raw Stores

**Use Tables (`Table<R>`) when:**
- Working with structured data
- Need string interning
- Want automatic transformations
- Building application features

**Use Raw Stores (`Store`) when:**
- Building custom indexes
- Maximum performance needed
- Don't need interning overhead
- Building internal components

**Storage Backend Options:**
- **InMemoryStore** - For testing/caching (zero latency)
- **CachedStore** - Wrapper with sync/async write modes
- **Persistent stores** - Sled, Redb, Fjall, Nebari, Persy, Canopy

### Memory Usage

| Operation | Memory |
|-----------|--------|
| Open table | Minimal (Arc pointers) |
| First access | + interner size |
| Streaming | Constant (batch_size) |
| Full scan (iter()) | O(dataset) - beware! |

### Interning Overhead

**Cost:**
- Lookup in DashMap for each string
- System record updates
- Extra deserialization pass

**Benefit:**
- ~70% memory reduction
- Faster comparisons (u64 vs String)
- Smaller storage footprint

**Verdict:** Worth it for string-heavy data!

## Error Recovery

### Storage Errors

```rust
match table.insert(value).await {
    Ok(id) => println!("Inserted: {}", id),
    Err(DbError::Storage(msg)) => {
        eprintln!("Backend error: {}", msg);
        // Retry? Fail gracefully?
    }
    Err(e) => eprintln!("Other error: {:?}", e),
}
```

### NotFound Errors

```rust
match table.get(id).await {
    Ok(record) => println!("Found: {:?}", record),
    Err(DbError::NotFound(_)) => {
        println!("Record doesn't exist");
        // Handle missing record
    }
}
```


Data transform:
// Write: [api: MessagePack] → [DB: InnerValue  → Bytes → Store]
let inner_bytes = transform.inner_value.to_bytes();  // rmp_serde

// Read: [DB: Store → Bytes → InnerValue] → [api: MessagePack]
let inner_value = InnerValue::from_bytes(bytes)?;

## Best Practices

### ✅ DO
- Use streaming for large datasets
- Set appropriate batch sizes (100-1000)
- Clone tables for concurrent use
- Handle errors gracefully

### ❌ DON'T
- Call `iter()` on large tables (OOM!)
- Ignore errors
- Forget to await async operations
- Assume RecordId is sequential

## Future Enhancements

- [x] ✅ **Multi-repo dispatcher** (2025-02-08)
  - Dispatcher manages multiple RepoManagers
  - YAML configuration support
  - ConfigLoader for atomic file operations
- [x] ✅ **Modular table architecture** (2025-02-08)
  - Extracted RecordCounter to separate module
  - Extracted InternerManager to separate module
  - Organized tests by type (CRUD, concurrent, persistence)
  - **280 tests passing**
- [x] ✅ **Test reorganization** (2025-02-08)
  - Tests moved to separate `tests/` folders
  - One entity per test file
  - Names match content
- [x] ✅ **Repo management** (2025-02-08)
  - RepoManager for repository operations
  - Lazy table initialization
  - Default repo support
- [x] ✅ **Index system** (2026-02-22)
  - Simple and composite indexes
  - Unique constraint validation
  - Atomic flags for O(1) existence check
  - **56 index tests**
  - UniqueIndexCreationFailed error with duplicate count
- [ ] Query planner integration
- [ ] Transaction support across tables
- [ ] Migration system
- [ ] Backup/restore utilities
- [ ] Garbage collection for unused interned strings
