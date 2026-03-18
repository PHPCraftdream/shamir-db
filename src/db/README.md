# Database Layer

S.H.A.M.I.R. database abstraction layer, providing table management, indexing system, query execution, and storage abstraction.

## Architecture

```
db/
├── shamir_db/           # Top-level database manager
│   ├── mod.rs
│   ├── shamir_db.rs     # ShamirDb — multi-database manager
│   ├── system_store.rs  # SystemStore — persistent metadata (databases, repos, settings, users, roles)
│   ├── execute.rs       # Batch execution entry point (TableResolver, AdminExecutor)
│   └── tests/
│       ├── shamir_db_tests.rs
│       ├── system_metadata_tests.rs
│       └── execute_tests.rs
├── engine/              # Table engine (high-level API)
│   ├── db_instance/     # Database instance management
│   │   ├── db_instance.rs  # DbInstance (manages repos within a database)
│   │   └── tests/
│   ├── repo/            # Repository management
│   │   ├── repo_config.rs       # RepoConfig, BoxRepoFactory
│   │   ├── repo_manager.rs      # RepoManager (manages repos)
│   │   ├── repo_manager_instance.rs
│   │   ├── repo_types.rs
│   │   └── tests/
│   ├── table/           # Table implementation
│   │   ├── table_manager.rs     # TableManager — main table facade
│   │   ├── table_config.rs      # TableConfig
│   │   ├── table_context.rs     # TableContext (with index integration)
│   │   ├── counter.rs           # RecordCounter service
│   │   ├── interner_manager.rs  # InternerManager service
│   │   └── tests/
│   ├── index/           # Index management system
│   │   ├── index_definition.rs
│   │   ├── index_info.rs
│   │   ├── index_info_item.rs
│   │   ├── index_record_key.rs
│   │   ├── index_status.rs
│   │   ├── index_manager.rs
│   │   └── tests/
│   └── README.md
├── storage/             # Storage abstraction (low-level)
│   ├── types.rs         # Store and Repo traits
│   ├── error.rs         # DbError, DbResult types
│   ├── storage_in_memory.rs
│   ├── storage_cached.rs
│   ├── storage_sled.rs
│   ├── storage_redb.rs
│   ├── storage_fjall.rs
│   ├── storage_nebari.rs
│   ├── storage_persy.rs
│   ├── storage_canopy.rs
│   └── README.md
├── query/               # Query system
│   ├── table_ref.rs     # TableRef { repo, table }
│   ├── batch/           # Batch query API
│   │   ├── types.rs     # BatchRequest (id mandatory), BatchResponse, BatchOp, QueryEntry
│   │   ├── planner.rs   # BatchPlanner (topological sort, dependency resolution)
│   │   ├── executor.rs  # execute_batch, TableResolver, AdminExecutor traits
│   │   ├── reference.rs # QueryReference, QueryPath ($query parsing)
│   │   └── README.md
│   ├── read/            # Read operations (SELECT)
│   │   ├── read_query.rs # ReadQuery (from: TableRef, select, where, group_by, order_by, pagination)
│   │   ├── select.rs    # Select, SelectItem
│   │   ├── limit.rs     # Pagination enum (LimitOffset / Page / None), PaginationInfo
│   │   ├── order_by.rs  # OrderBy, OrderByItem, OrderDirection, NullsOrder
│   │   ├── group_by.rs  # GroupBy
│   │   ├── agg.rs       # AggFunc, AggregateField
│   │   ├── query_result.rs # QueryResult, QueryStats
│   │   ├── exec.rs      # Query execution engine
│   │   └── README.md
│   ├── write/           # Write operations
│   │   ├── types.rs     # InsertOp, UpdateOp, SetOp, DeleteOp, UpdateSelect, UpdateReturnMode
│   │   ├── write_result.rs # WriteResult
│   │   └── README.md
│   ├── filter/          # Filter (WHERE clause)
│   │   ├── filter_enum.rs  # Filter enum (all operators)
│   │   ├── filter_value.rs # FilterValue
│   │   ├── filter_expr.rs  # FilterExpr, FilterExprOp
│   │   ├── fn_call.rs      # FnCall ($fn)
│   │   ├── cond.rs         # Cond ($cond)
│   │   ├── eval.rs         # compile_filter, compare_values, resolve_field
│   │   ├── eval_context.rs # FilterContext
│   │   └── mod.rs       # FieldPath = Vec<String>
│   ├── admin/           # Admin (DDL) operations
│   │   ├── types.rs     # Create/Drop Db/Repo/Table/Index ops, ListOp
│   │   └── mod.rs
│   ├── common/
│   │   └── mod.rs
│   └── examples/
│       ├── filter.md
│       ├── select.md
│       ├── aggregate.md
│       └── write.md
├── mod.rs               # Re-exports: ShamirDb, SystemStoreConfig, DbError, DbResult
└── error.rs             # (legacy location, actual error in storage/error.rs)
```

## Top-Level Entry Point

### ShamirDb

The primary entry point for the entire database system.

```rust
use shamir_db::db::{ShamirDb, SystemStoreConfig};

// Initialize with persistent storage
let db = ShamirDb::init(SystemStoreConfig::Redb("./data".into())).await?;

// Or in-memory for tests
let db = ShamirDb::init_memory().await?;

// Create and use databases
db.create_db("myapp").await;
let response = db.execute("myapp", &batch_request).await?;
```

**Hierarchy:**
```
ShamirDb
  +-- SystemStore (persistent metadata: databases, repos, settings, users, roles)
  +-- production (DbInstance)
  |     +-- main (RepoInstance)
  |           +-- users (TableManager)
  +-- analytics (DbInstance)
        +-- archive (RepoInstance)
              +-- logs (TableManager)
```

### SystemStore

Persistent metadata store using a dedicated DbInstance with system tables:
- `databases` - registered databases
- `repositories` - registered repositories (with engine type and path)
- `settings` - key-value settings
- `users` - user accounts (for auth/RBAC)
- `roles` - role definitions (for auth/RBAC)

```rust
// SystemStoreConfig determines persistence
pub enum SystemStoreConfig {
    InMemory,                    // For tests
    Redb(std::path::PathBuf),    // For production
}
```

## Components

### Engine (`db/engine/`)
**High-level table API** with automatic interning and index management:
- `DbInstance` - Database instance managing multiple repos
- `RepoManager` - Repository and table management
- `TableManager` - Main table abstraction with index integration
- `IndexManager` - Index management system
- `RecordCounter` - Counter service
- `InternerManager` - Interning service

### Storage (`db/storage/`)
**Low-level storage abstraction** over 7 embedded databases + cached wrapper:
- Pluggable backends: InMemory, Sled, Redb, Fjall, Nebari, Persy, Canopy
- CachedStore wrapper with sync/async write modes
- Unified `Store` trait for key-value operations
- Unified `Repo` trait for multi-store management
- Async streaming with batch generators
- Prefix scan operations for composite keys

See `storage/README.md` for details.

### Query System (`db/query/`)
**Unified query interface** for read, write, and admin operations:

- **TableRef** `{ repo, table }` - Table reference with optional repo qualifier
- **FieldPath** `Vec<String>` - Array-based field paths (`["user", "address", "city"]`)
- **BatchRequest** - Batch API with mandatory `id` field
- **BatchOp** - Key-based dispatch (explicit, not serde untagged)
- **ReadQuery** - SELECT with filters, ordering, pagination (Pagination enum: LimitOffset / Page / None)
- **Filter** - Full set of operators including Like, ILike, Regex, Contains, ContainsAny, ContainsAll, Between, Exists, NotExists
- **Write Operations**: InsertOp, UpdateOp, SetOp (upsert, fully working), DeleteOp
- **Admin Operations**: Create/Drop Db/Repo/Table/Index, List
- **AdminExecutor** trait for DDL execution
- **TableResolver** trait for resolving TableRef to TableManager

See `query/batch/README.md` for details.

## Error Handling

All database operations return `DbResult<T>`:

```rust
pub type DbResult<T> = Result<T, DbError>;

pub enum DbError {
    NotFound(String),                              // Key/table doesn't exist
    KeyExists(String),                             // Primary key collision
    DuplicateKey(String),                          // Unique index violation
    UniqueIndexCreationFailed(String, usize, String), // (name, count, sample)
    Storage(String),                               // Backend-specific error
    Config(String),                                // Configuration error
    Codec(String),                                 // Serialization error
    Io(std::io::Error),                            // I/O error
    Internal(String),                              // Internal logic error
    Validation(String),                            // Validation error
}
```

## Usage Flow

### 1. Initialize ShamirDb

```rust
use shamir_db::db::{ShamirDb, SystemStoreConfig};

let db = ShamirDb::init(SystemStoreConfig::InMemory).await?;
```

### 2. Create Database and Repository

```rust
db.create_db("myapp").await;

use shamir_db::db::engine::repo::{RepoConfig, BoxRepoFactory};
let config = RepoConfig::new("main", BoxRepoFactory::in_memory());
db.add_repo("myapp", config).await?;
```

### 3. Execute Batch Queries

```rust
use shamir_db::db::query::BatchRequest;

let request: BatchRequest = serde_json::from_value(serde_json::json!({
    "id": 1,
    "queries": {
        "users": {
            "from": "users",
            "where": { "op": "eq", "field": ["status"], "value": "active" }
        }
    }
}))?;

let response = db.execute("myapp", &request).await?;
```

## Concurrency Model

### Thread Safety

All components are thread-safe:
- `ShamirDb` is `Clone` (Arc-based)
- `DbInstance` is `Clone` (Arc-based)
- `TableManager` is `Clone` (Arc-based)
- DashMap for concurrent interning
- OnceCell for lazy initialization

## Key Type Changes

| Concept | Old | Current |
|---------|-----|---------|
| Field paths | `String` (dot-separated) | `Vec<String>` (array segments) |
| Table reference | `String` | `TableRef { repo, table }` |
| Batch request ID | not present | mandatory `id: serde_json::Value` |
| BatchOp dispatch | `#[serde(untagged)]` | explicit key-based dispatch |
| Pagination | `LimitOffset` struct | `Pagination` enum (LimitOffset / Page / None) |
| Initialization | `ShamirDb::new().init()` | `ShamirDb::init(SystemStoreConfig)` |
| InnerValue key | `Value<u64>` | `Value<InternerKey>` |

## Future Enhancements

- [x] Multi-repo dispatcher
- [x] Modular table architecture
- [x] Index system (simple, composite, unique)
- [x] Query system with read/write/admin ops
- [x] SystemStore for persistent metadata
- [x] ShamirDb::init(SystemStoreConfig)
- [x] Filter evaluation (all operators implemented)
- [x] SetOp (upsert) fully working
- [ ] Auth/RBAC (designed, see auth/README.md)
- [ ] $user reference for role-based row filtering
- [ ] Query planner integration
- [ ] Transaction support across tables
- [ ] Migration system
- [ ] Backup/restore utilities
