# Table Engine

High-level table abstraction with automatic key interning, value transformations, and index management.

## Architecture

```
engine/
├── db_instance/                 # Database instance
│   ├── db_instance.rs           # DbInstance (manages repos within a database)
│   ├── mod.rs
│   └── tests/
├── repo/                        # Repository management
│   ├── repo_config.rs           # RepoConfig, BoxRepoFactory
│   ├── repo_manager.rs          # RepoManager
│   ├── repo_manager_instance.rs # RepoManagerInstance
│   ├── repo_types.rs            # BoxRepoFactory enum (7 engine types)
│   ├── mod.rs
│   └── tests/
├── table/                       # Table implementation
│   ├── table_manager.rs         # TableManager — main table facade
│   ├── table_config.rs          # TableConfig
│   ├── table_context.rs         # TableContext (with index integration)
│   ├── counter.rs               # RecordCounter service
│   ├── interner_manager.rs      # InternerManager service
│   ├── mod.rs
│   └── tests/
│       ├── crud_tests.rs
│       ├── concurrent_tests.rs
│       └── persistence_tests.rs
├── index/                       # Index management system
│   ├── index_definition.rs      # Index definition (simple/composite)
│   ├── index_info.rs            # Index metadata with sync status
│   ├── index_info_item.rs       # Single index path item
│   ├── index_record_key.rs      # Index record key for B-Tree storage
│   ├── index_status.rs          # Index sync status enum
│   ├── index_manager.rs         # IndexManager
│   ├── mod.rs
│   └── tests/
├── README.md
└── table.md
```

## Purpose

Bridges the gap between raw storage (`Store`) and user-friendly API:
- **ShamirDb** -> **DbInstance** -> **RepoManager** -> **TableManager**
- Manages key interning transparently
- Provides async streaming for large datasets
- Handles table metadata via system records
- Index management with atomic flags for O(1) existence check

## DbInstance

A database instance manages multiple repositories:

```rust
let db = DbInstance::new();
db.add_repo(repo_config).await?;
let table = db.get_table("main", "users").await?;
```

## RepoManager and BoxRepoFactory

Repository management with factory-based backend selection:

```rust
// Supported storage backends
pub enum BoxRepoFactory {
    InMemory(_),
    Sled(_),
    Redb(_),
    Fjall(_),
    Nebari(_),
    Persy(_),
    Canopy(_),
}

let config = RepoConfig::new("main", BoxRepoFactory::in_memory())
    .add_table(TableConfig::new("users"))
    .add_table(TableConfig::new("orders"));
```

## TableManager

The main table abstraction (formerly `Table<R>`). Each table consists of two underlying stores:

```
__data__{table_name}  -> actual data with InnerValue (InternerKey keys)
__info__{table_name}  -> interning state + indexes
```

### System Records

The interner is stored as system records in `__info__` store:

```rust
RecordId::system("internals")  -> bincoded map<String, u64>
RecordId::system("inter_max")  -> bincoded u64
RecordId::system("indexes")    -> Index metadata
```

### Query Execution

TableManager executes queries via the Batch API:

```rust
// Read
table.read(&read_query, &filter_context).await?;

// Write operations
table.execute_insert(&insert_op).await?;
table.execute_update(&update_op, &filter_context).await?;
table.execute_set(&set_op).await?;        // Upsert (fully working)
table.execute_delete(&delete_op, &filter_context).await?;
```

### InternerManager

Manages the interner lifecycle:

```rust
// Get interner (lazy load from __info__ store)
let interner = table.interner().get().await?;

// Persist interner state to storage
table.interner().persist().await?;
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
// Simple index
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

- **Atomic flags** for O(1) existence check (no locks!)
- **Two index types**: regular and unique
- **Unique indexes**: validation BEFORE write, update AFTER write
- **Status tracking**: Actual, Pending, Saving

### Index Operations

```rust
// Create indexes
table.create_index("email_idx", &["email"]).await?;
table.create_unique_index("username_idx", &["username"]).await?;

// Drop indexes
table.drop_index("email_idx").await?;
table.drop_unique_index("username_idx").await?;

// Check if has indexes (O(1) - no locks!)
if table.has_indexes() {
    // Use indexes for query
}
```

## Concurrency

TableManager is fully thread-safe:

```rust
// Clone table (cheap - Arc-based)
let table1 = table.clone();
let table2 = table.clone();

// Concurrent operations
let t1 = tokio::spawn(async move {
    table1.execute_insert(&insert_op).await
});
let t2 = tokio::spawn(async move {
    table2.read(&query, &ctx).await
});
tokio::join!(t1, t2);
```

**Thread Safety Guarantees:**
- Multiple concurrent reads
- Multiple concurrent writes
- Interner synchronized via DashMap
- OnceCell ensures single init

## Error Handling

```rust
use shamir_db::DbError;

match table.read(&query, &ctx).await {
    Ok(result) => println!("Found {} records", result.records.len()),
    Err(DbError::NotFound(msg)) => eprintln!("Not found: {}", msg),
    Err(DbError::DuplicateKey(msg)) => eprintln!("Duplicate: {}", msg),
    Err(DbError::Storage(msg)) => eprintln!("Storage error: {}", msg),
    Err(e) => eprintln!("Error: {:?}", e),
}
```

## Future Enhancements

- [x] Multi-repo dispatcher (DbInstance)
- [x] Repo management with BoxRepoFactory
- [x] Modular table architecture (TableManager)
- [x] Index system (simple, composite, unique)
- [x] Query execution (read, insert, update, set, delete)
- [x] Admin operations via Batch API
- [ ] Garbage collection for unused interned strings
- [ ] Automatic batch size tuning
- [ ] Statistics (record count, interned strings count)
- [ ] Transaction support across tables
