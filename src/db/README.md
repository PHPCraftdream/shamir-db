# Database Layer

S.H.A.M.I.R. database abstraction layer, providing table management, indexing system, and storage abstraction.

## Architecture

```
db/
├── engine/           # Table engine (high-level API)
│   ├── index/        # Index management system
│   │   ├── index_definition.rs   # Index definition (simple/composite)
│   │   ├── index_info.rs         # Index metadata & status
│   │   ├── index_info_item.rs    # Single index path item
│   │   ├── index_record.rs       # Index record representation
│   │   └── table_index_manager.rs # Index manager for tables
│   ├── table.rs     # Table with interning + streaming
│   └── table.md     # Table analysis & refactoring plans
├── storage/          # Storage abstraction (low-level)
│   ├── types.rs      # Store and Repo traits
│   ├── storage_in_memory.rs  # In-memory store (for testing)
│   ├── storage_cached.rs     # Cached store wrapper (sync/async modes)
│   ├── storage_sled.rs
│   ├── storage_redb.rs
│   ├── storage_fjall.rs
│   ├── storage_nebari.rs
│   ├── storage_persy.rs
│   └── storage_canopy.rs
├── mod.rs
└── error.rs          # DbError, DbResult types
```

## Components

### Engine (`db/engine/`)
**High-level table API** with automatic interning and index management:
- `Table<R>` - Main table abstraction
- `TableIndexManager` - Index management system
- Manages key interning transparently
- Transforms UserValue ↔ InnerValue
- Provides memory-efficient async streaming

See `engine/README.md` for details.

### Index System (`db/engine/index/`)
**Index management** for tables:
- `IndexDefinition` - Simple and composite index definitions
- `IndexInfo` - Index metadata with sync status tracking
- `TableIndexManager` - Index operations and validation
- Atomic flags for fast path optimization (O(1) existence check)
- Three indexing modes: Disabled, All, Selective

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

// Query with prefix scan (for index lookups)
let results = store.scan_prefix(b"idx:email:".to_vec().into()).await?;
for (key, value) in results {
    // Process matching records
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

- [x] Index system with simple/composite indexes
- [x] Unique constraint validation
- [x] Atomic flags for fast path optimization
- [ ] Query planner integration
- [ ] Transaction support across tables
- [ ] Migration system
- [ ] Backup/restore utilities
