# S.H.A.M.I.R. Architecture Knowledge Base

## arch-001-overview

S.H.A.M.I.R. Database Architecture Overview

**S.H.A.M.I.R.** is a production-level, standalone binary, decentralized database written in Rust.

**Acronym Meaning:**
- S: Secure (Rust)
- H: High-performance
- A: Asynchronous
- M: Modular (WASM)
- I: Interconnected (Chat/P2P)
- R: Repository

**Global Goals:**
1. Self-contained: Single binary file (<50MB), no external dependencies
2. Hybrid Storage: MessagePack data with interning (String -> u64) for speed and compression
3. WASM-First: Database logic as WASM modules
4. Reliability: Checksums, Crash safety (storage backends handle durability)

**Key Technologies:**
- Async: Tokio with full features
- Serialization: rmp-serde (MessagePack), serde
- Concurrency: DashMap, arc-swap, tokio::task::spawn_blocking
- Storage backends: 6 supported engines (Sled, Redb, Fjall, Nebari, Persy, Canopy)
- Error handling: thiserror, anyhow

## arch-002-storage

Storage Backend Abstraction Layer

S.H.A.M.I.R. provides a unified interface over 6 different embedded database engines.

**Core Traits:**

Store trait (low-level key-value store):
- insert(value: Bytes) -> RecordId
- set(key: RecordId, value: Bytes) -> bool
- get(key: RecordId) -> Bytes
- remove(key: RecordId) -> bool
- iter() -> Vec<(RecordId, Bytes)>
- iter_stream(batch_size) -> Stream of batches (PHP-style generators)
- scan_prefix_stream(prefix, batch_size) -> Stream of batches

Repo trait (manages multiple stores):
- store_get(name) -> Arc<Store>
- store_delete(name) -> bool
- stores_list() -> Vec<String>

**Supported Backends:**
1. InMemory: DashMap-based for testing/caching
2. Cached: Wrapper with Sync/Async write modes
3. Sled: Pure Rust B-tree, battle-tested
4. Redb: Modern MVCC B-tree with ACID transactions
5. Fjall: LSM-tree for high write throughput
6. Nebari: B-tree (BlueDB successor)
7. Persy: B-tree with indexes and ACID transactions
8. Canopy: B-tree with LZ4 compression

## arch-003-interner

Interning System - String to u64 Compression

**Purpose:** Convert frequently occurring strings to compact u64 IDs for memory efficiency and serialization speed.

**Key Types:**
- UserKey: Wrapper around String for user-facing keys
- InternedKey: Compact binary key (1/2/4/8 bytes based on count)
- TouchInd: Result enum (Exists(InternedKey) | New(InternedKey))

**Interner Structure:**
- map_user_to_interned: DashMap<UserKey, InternedKey>
- map_interned_to_user: DashMap<InternedKey, UserKey>
- current_id: Mutex<u64>
- key_size: Mutex<u8> (1, 2, 4, or 8)
- migration_lock: Mutex<()> (prevents concurrent migrations)

**Dynamic Key Sizing:**
- 0-255 keys: 1 byte (u8)
- 256-65535 keys: 2 bytes (u16)
- 65536-4B keys: 4 bytes (u32)
- 4B+ keys: 8 bytes (u64)

**Migration Process:**
1. When threshold crossed, acquire migration_lock
2. Collect all current mappings
3. Clear both maps
4. Rebuild with new InternedKey instances using updated byte size

**Benefits:**
- ~70% memory reduction for string-heavy data
- Faster serialization (fixed-size keys)
- Smaller storage footprint

## arch-004-value-types

Value Type System

**Two Value Types:**
- UserValue<Value<String>>: Deprecated, for tests only
- InnerValue<Value<InternedKey>>: Production format with interning

**Supported Types:**
- Null: null value
- Bool: boolean
- Int: i64 integer
- F64: float64
- Dec: Decimal (rust_decimal)
- Big: BigInt (num-bigint)
- Str: String
- Bin: Vec<u8> binary data
- List: Vec<Value<Key>>
- Set: TSet<Value<Key>>
- Map: TMap<Key, Value<Key>>

**Serialization:**
- to_bytes(): Serialize to MessagePack (Bytes)
- from_bytes(): Deserialize from MessagePack
- Full serde support with custom deserializer for type hints

**JSON Type Hints (for UserValue only):**
- i:prefix -> Int
- u:prefix -> UInt
- float:prefix -> Float
- dec:prefix -> Decimal (as string)
- big:prefix -> BigInt (as string)
- arr:prefix -> Array
- set:prefix -> Set

## arch-005-table-engine

Table Engine Architecture

**TableContext (High-level):**
- Manages table name, interner, record counter, index manager
- Handles UserValue <-> InnerValue transformation
- Provides convenient API: insert_user(), get_user(), list_stream_user()

**Table (Low-level):**
- InnerValue only (no interning/conversion)
- Just a data store wrapper around Store
- Methods: insert(), get(), update(), set(), delete(), list_stream()
- Doesn't know its name or manage counters

**InterningManager:**
- Manages interned keys for a table
- Provides bidirectional mapping (String <-> InternedKey)
- Used by TableContext for transformation

**RecordCounter:**
- Monotonically increasing record IDs
- Persistent storage
- Thread-safe using Mutex

**IndexManager:**
- Secondary index management (simple and unique)
- Atomic flags for O(1) index existence check
- See arch-011-indexes for details

## arch-006-async-streaming

Async Streaming Implementation

**PHP-style Generators:**
All backends implement iter_stream() using async_stream::stream! macro.

**Key Features:**
- Clean generator syntax: stream! { yield Ok(batch); }
- Memory-efficient: Constant memory regardless of dataset size
- Concurrent: Uses spawn_blocking for CPU-intensive work
- Lazy: Only fetches when consumer calls .next().await
- Batch processing: Tunable batch size (100-1000 typical)

**Cursor Management:**
Each backend manages cursors differently:
- Sled: range(cursor..) includes cursor, need skip_first
- Redb: range((Excluded(cursor), Unbounded))
- Fjall: Manual iterator skip
- Nebari: Scan with skip flag
- Persy: Collect all, then slice (no range support)
- Canopy: range(cursor..)

## arch-007-codecs

Codec System

**Available Codecs:**
1. JSON: Human-readable, serde_json
2. MessagePack: Binary, rmp-serde (primary format)

**Type Mapping:**

| Rust Type | JSON | MessagePack |
|-----------|------|-------------|
| Null | null | nil |
| Bool | true/false | bool |
| Int(i64) | number | int64 |
| UInt(u64) | number | uint64 |
| Float(f64) | number | float64 |
| String | string | str |
| Binary | base64 string | bin |
| Array | array | array |
| Object | object | map |
| Decimal | string (decimal) | str (decimal) |
| BigInt | string (bigint) | str (bigint) |

**Performance:**
| Format | Size | Encode | Decode |
|--------|------|-------|--------|
| JSON | 100% | 1x | 1x |
| MessagePack | ~60% | 1.2x | 1.3x |

**Key Differences:**
- JSON: Type hints support (i:, u:, float:, dec:, big:, arr:, set:)
- MessagePack: No type hints, more compact
- BigInt/Decimal: Both serialized as strings for precision preservation

## arch-008-concurrency

Concurrency Patterns

**DashMap Usage:**
- Used for interner maps (user_to_interned, interned_to_user)
- Lock-free concurrent operations
- Fine-grained locking (sharded)

**arc-swap Usage:**
- For atomic reference swapping
- Used where Mutex would be too slow
- Good for read-heavy scenarios

**tokio::task::spawn_blocking:**
- Used for blocking operations in async context
- Prevents blocking tokio runtime
- Applied to: storage operations, serialization, deserialization

**Mutex Usage:**
- Interner: current_id, key_size, migration_lock
- Minimal locking around critical sections
- Lock held for short durations only

**Avoid:**
- Mutex for high-contention data structures
- Blocking operations in async context (use spawn_blocking instead)
- Shared state without synchronization

## arch-009-error-handling

Error Handling Strategy

**Error Types:**
- DbError: Database errors (storage, not found, codec, internal)
- CodecError: Serialization/deserialization errors
- thiserror: For library error enums with #[from] attributes
- anyhow: For application-level error handling

**Result Pattern:**
- All functions return Result<T, E>
- Use ? operator for error propagation
- Box<dyn Error> for different error types
- Avoid panic! in production code

**DbError Variants:**
- Storage: Backend-specific errors
- NotFound: Key doesn't exist
- KeyExists: Primary key collision
- DuplicateKey: Unique index violation
- UniqueIndexCreationFailed(name, count, sample): Cannot create unique index due to duplicates
- Codec: Serialization errors
- Internal: Internal logic errors

**UniqueIndexCreationFailed Details:**
- Signature: UniqueIndexCreationFailed(String, usize, String)
- Contains: (index_name, duplicate_count, sample_value)
- Example: Cannot create unique index 'by_email': found 3 records with duplicate values (example: alice@example.com)
- Returned by: IndexManager::create_unique_index()

**Best Practices:**
- Use specific error types (DbError over anyhow)
- Provide context: DbError::Storage(format!("...: {}", e))
- Handle errors gracefully (not crash)
- Use ? for propagation
- Map lower-level errors to domain errors

## arch-010-development-protocol

Development Protocol (TDD)

**Test-Driven Development:**
1. RED: Write test that fails or doesn't compile
2. GREEN: Write minimal code to pass test
3. REFACTOR: Improve code while keeping tests green

**Code Quality (Mandatory):**
1. cargo clippy --all-targets
2. cargo fmt
3. cargo test --lib

**Project Rules:**
- Use arc_swap, dashmap, tokio::task::spawn_blocking for concurrency
- Don't change unrelated code
- Don't change unrelated comments
- Make only targeted changes
- In tests: JSON must be formatted, multi-line
- mod.rs files only contain exports
- Tests in separate tests/ folder, not in implementation files

**Test Organization:**
- Each module has tests/ folder
- Separate test files for logically related tests
- mod.rs in tests/ only contains exports
- Parent module contains #[cfg(test)] mod tests;

## arch-011-indexes

Index System Architecture

**IndexManager (formerly TableIndexManager):**
- Manages secondary indexes for tables
- Supports regular and unique indexes
- Uses atomic flags for O(1) existence check

**Index Modes:**
- Disabled: No indexing
- All: Index all Map fields (simple indexes)
- Selective: Custom index definitions

**Index Types:**
- Simple: Single field path
- Composite: Multiple field paths

**IndexDefinition:**
- name_interned: u64 (interned index name, NOT String)
- paths: Vec<IndexInfoItem> (field paths as Vec<u64>)

**Unique Index Behavior:**
1. Validation BEFORE write: Check for existing duplicates
2. Update AFTER write: Add to index on successful insert
3. Error on duplicates: UniqueIndexCreationFailed(name, count, sample)

**IndexRecordKey Format:**
- Fixed 26 bytes: [is_unique:1][index_id:8][hash1:8][hash2:8]
- is_unique: 1 byte flag
- index_id: 8 bytes (interned index name)
- hash1: 8 bytes (FxHasher of values)
- hash2: 8 bytes (collision resistance)

**⚠️ Hash Collision Safety:**
При извлечении данных по хешу (hash1, hash2) **ОБЯЗАТЕЛЬНО** проверять фактическое значение данных:

```rust
// ❌ НЕПРАВИЛЬНО: доверяем хешу без проверки
let record_id = index.lookup(hash)?;
return Ok(record_id);

// ✅ ПРАВИЛЬНО: проверяем фактическое значение
let candidates = index.lookup(hash)?;
for record_id in candidates {
    let record = table.get(record_id)?;
    if record.get(field_path) == expected_value {
        return Ok(Some(record_id));  // Точное совпадение
    }
}
return Ok(None);  // Хеш совпал, но данные разные (коллизия)
}
```

**Почему это важно:**
- Хеши имеют фиксированный размер → возможны коллизии
- FxHasher быстрый, но не криптографический
- Два разных значения могут дать одинаковый hash1 + hash2
- Без проверки можно вернуть чужую запись

**Статус:**
- hash2 добавлен для снижения вероятности коллизии (2^128 комбинаций)
- Но проверка данных всё равно обязательна для корректности

**Status Tracking:**
- Actual: Index is up-to-date
- Pending: Index needs sync
- Saving: Index is being saved

**System Records:**
- RecordId::system("indexes"): Regular index definitions
- RecordId::system("indexes_unique"): Unique index definitions

**Test Coverage:**
- 79 index manager tests
- 303 total lib tests passing

## arch-012-query-language

Query Language (SDBQL - S.H.A.M.I.R. Database Query Language)

**Design Philosophy:**
- JSON-native: Queries are JSON objects
- Familiar: MongoDB-style find/update syntax
- Index-aware: Query planner uses indexes automatically
- Pipeline-based: Composable operations

**Principle: OQL (Object Query Language) — no text language, ever.**
SDBQL is an *object* query language: a query is a typed data structure
(`Filter` / `ReadQuery` / `BatchRequest` DTO) carried as msgpack/JSON and
built by the typed builder / `q!` / `filter!`. There will **never** be a
textual / SQL frontend or a "v2" parser. This is a deliberate, permanent
decision, not a missing feature. Queries-as-text is the single root
mistake behind SQL injection, parser/grammar bugs and DoS, prepared-
statement/bind ceremony, dialect drift, and parse/plan caching. OQL does
not *mitigate* those — it makes them **structurally impossible**:

- **Injection (CWE-89): impossible.** Values live in `value` fields; they
  are never concatenated into a command string. There is no context in
  which data could be reinterpreted as code, so there is nothing to escape.
- **No parser, no parser bugs.** "Parsing" is total, deterministic msgpack
  deserialisation into typed structs — no grammar, no lexer, no
  catastrophic-backtracking DoS, no dialect drift.
- **No prepared statements.** Every query is already parameterised — code
  and data were never mixed, so there is no binding apparatus to add.
- **No parse/plan cache.** The DTO *is* the wire *is* the AST — one
  representation, not three (text → AST → plan), so there is no re-parse
  cost to cache.

OQL may *grow* (more operators, `$fn`, richer filters) — that is evolving
the same object language, never adding a textual frontend. The builder,
the wire, and guest procedures all speak the identical DTOs (one language,
one builder, three callers); a text "v2" would only parse back into these
DTOs and would fracture that symmetry.

**Query Types:**

### Special Value References

SDBQL поддерживает специальные ссылки в значениях:

| Ссылка | Описание | Пример |
|--------|----------|--------|
| `$query` | Ссылка на результат другого запроса | `{ "$query": "users[0].id" }` |
| `$ref` | Ссылка на другое поле записи | `{ "$ref": "other_field" }` |
| `$fn` | Системная функция | `{ "$fn": "NOW" }` |
| `$expr` | Выражение (арифметика, строки) | `{ "$expr": { "op": "add", "args": [1, 2] } }` |
| `$cond` | Условный оператор | `{ "$cond": { "if": {...}, "then": "a", "else": "b" } }` |

### Системные функции ($fn)

**Категории:**
- Дата/время: `NOW`, `TODAY`, `UNIX_TIMESTAMP`
- Генерация: `UUID`, `RANDOM`, `RANDOM_INT`
- Строки: `LENGTH`, `UPPER`, `LOWER`, `TRIM`, `SUBSTRING`
- Логические: `COALESCE`, `IFNULL`, `NULLIF`
- Хеширование: `MD5`, `SHA256`
- Математика: `ABS`, `ROUND`, `FLOOR`, `CEIL`

**Синтаксис:**
```json
// Без аргументов
{ "$fn": "NOW" }
{ "$fn": "UUID" }

// С аргументами
{ "$fn": { "name": "COALESCE", "args": [null, "default"] } }
```

**Использование:**
```json
// В WHERE
{ "where": { "op": "lt", "field": "expires_at", "value": { "$fn": "NOW" } } }

// В SET
{ "set": { "created_at": { "$fn": "NOW" }, "token": { "$fn": "UUID" } } }
```

### Выражения ($expr)

Арифметические и строковые операции.

**Операторы:**
- Математика: `add`, `sub`, `mul`, `div`, `mod`, `neg`
- Строки: `concat`, `lower`, `upper`, `trim`, `length`
- Логика: `and`, `or`, `not`
- Сравнение: `eq`, `ne`, `gt`, `gte`, `lt`, `lte`

**Синтаксис:**
```json
{ "$expr": { "op": "add", "args": [10, 20] } }
{ "$expr": { "op": "mul", "args": [{ "$ref": "price" }, 1.1] } }
{ "$expr": { "op": "concat", "args": [{ "$ref": "first" }, " ", { "$ref": "last" }] } }
```

**Использование:**
```json
// В SET
{ "set": { "price": { "$expr": { "op": "mul", "args": [{ "$ref": "price" }, 1.1] } } } }

// Вложенные выражения
{ "set": { "total": { "$expr": { "op": "mul", "args": [
  { "$expr": { "op": "add", "args": [{ "$ref": "a" }, { "$ref": "b" }] } },
  2
] } } } }
```

### Условия ($cond)

Условный оператор (тернарный).

**Синтаксис:**
```json
{
  "$cond": {
    "if": { "op": "eq", "field": "active", "value": true },
    "then": "yes",
    "else": "no"
  }
}
```

**Условие `if` использует существующий синтаксис Filter.**

**Использование:**
```json
// Простой выбор
{ "set": { "label": { "$cond": {
  "if": { "op": "gte", "field": "score", "value": 100 },
  "then": "vip",
  "else": "regular"
} } } }

// С $expr в then/else
{ "set": { "price": { "$cond": {
  "if": { "op": "eq", "field": "is_vip", "value": true },
  "then": { "$expr": { "op": "mul", "args": [{ "$ref": "base" }, 0.9] } },
  "else": { "$ref": "base" }
} } } }

// Вложенные $cond
{ "set": { "tier": { "$cond": {
  "if": { "op": "gte", "field": "score", "value": 1000 },
  "then": "platinum",
  "else": { "$cond": {
    "if": { "op": "gte", "field": "score", "value": 500 },
    "then": "gold",
    "else": "silver"
  } }
} } } }
```

### Data Query Commands

```json
// FIND - Query records with filter
{
  "find": {
    "from": "users",
    "where": { "name": "Alice" },
    "select": ["name", "email"],
    "limit": 10,
    "offset": 0,
    "sort": { "name": 1 }
  }
}

// FIND ONE - Single record
{
  "findOne": {
    "from": "users",
    "where": { "_id": "record_id" }
  }
}

// INSERT - Create new record
{
  "insert": {
    "into": "users",
    "data": { "name": "Alice", "email": "alice@example.com" }
  }
}

// INSERT MANY - Bulk insert
{
  "insertMany": {
    "into": "users",
    "data": [
      { "name": "Alice" },
      { "name": "Bob" }
    ]
  }
}

// UPDATE - Modify records
{
  "update": {
    "in": "users",
    "where": { "status": "active" },
    "set": { "last_login": "2024-01-15" },
    "inc": { "login_count": 1 }
  }
}

// DELETE - Remove records
{
  "delete": {
    "from": "users",
    "where": { "status": "inactive" }
  }
}

// COUNT - Count records
{
  "count": {
    "from": "users",
    "where": { "status": "active" }
  }
}
```

### Filter Operators

```json
{
  "where": {
    "age": { "$gt": 18 },
    "name": { "$like": "A%" },
    "status": { "$in": ["active", "pending"] },
    "email": { "$ne": null },
    "tags": { "$contains": "admin" },
    "$and": [
      { "age": { "$gte": 18 } },
      { "age": { "$lte": 65 } }
    ],
    "$or": [
      { "role": "admin" },
      { "role": "moderator" }
    ]
  }
}
```

**Supported Operators:**
| Operator | Description | Example |
|----------|-------------|---------|
| `$eq` | Equal | `{ "status": { "$eq": "active" } }` |
| `$ne` | Not equal | `{ "status": { "$ne": "deleted" } }` |
| `$gt` | Greater than | `{ "age": { "$gt": 18 } }` |
| `$gte` | Greater or equal | `{ "age": { "$gte": 18 } }` |
| `$lt` | Less than | `{ "age": { "$lt": 65 } }` |
| `$lte` | Less or equal | `{ "age": { "$lte": 65 } }` |
| `$in` | In array | `{ "status": { "$in": ["a", "b"] } }` |
| `$nin` | Not in array | `{ "status": { "$nin": ["a", "b"] } }` |
| `$like` | Pattern match | `{ "name": { "$like": "A%" } }` |
| `$contains` | Array contains | `{ "tags": { "$contains": "admin" } }` |
| `$exists` | Field exists | `{ "email": { "$exists": true } }` |
| `$and` | Logical AND | `{ "$and": [...] }` |
| `$or` | Logical OR | `{ "$or": [...] }` |
| `$not` | Logical NOT | `{ "$not": { "status": "deleted" } }` |

### Update Operators

```json
{
  "set": { "name": "New Name" },       // Set field
  "unset": ["temp_field"],              // Remove field
  "inc": { "count": 1 },                // Increment number
  "push": { "tags": "new_tag" },        // Add to array
  "pull": { "tags": "old_tag" },        // Remove from array
  "rename": { "old_name": "new_name" }  // Rename field
}
```

## arch-013-query-planner

Query Planner - Index-Aware Query Execution

**Purpose:** Transform queries into efficient execution plans using available indexes.

**Planning Process:**
1. Parse query AST
2. Analyze filter conditions
3. Check available indexes
4. Select optimal index (or full scan)
5. Generate execution plan

**Index Selection Strategy:**

```
Query: { "where": { "email": "alice@example.com" } }
Available Indexes: ["by_email" (unique), "by_name" (regular)]

Plan:
  - Use index: "by_email" (unique)
  - Estimated cost: O(1)
  - Execution: Index lookup -> Single record
```

**Composite Index Usage:**

```
Query: { "where": { "status": "active", "department": "engineering" } }
Available Indexes: ["by_status_dept" (composite: status, department)]

Plan:
  - Use index: "by_status_dept"
  - Estimated cost: O(log n)
  - Execution: Index range scan
```

**Full Scan Fallback:**

```
Query: { "where": { "temp_field": "value" } }
No index on "temp_field"

Plan:
  - Full table scan
  - Estimated cost: O(n)
  - Execution: Stream all records, filter in memory
```

**Query Planner API:**

```rust
pub struct QueryPlan {
    pub table: String,
    pub index_used: Option<String>,
    pub scan_type: ScanType,
    pub estimated_cost: f64,
    pub filter: Filter,
}

pub enum ScanType {
    IndexLookup { key: Value },
    IndexRangeScan { from: Value, to: Value },
    FullScan,
}

impl QueryPlanner {
    pub async fn plan(&self, query: &FindQuery) -> DbResult<QueryPlan>;
    pub async fn execute(&self, plan: &QueryPlan) -> DbResult<Vec<Record>>;
}
```

## arch-014-authentication

Authentication System

**Authentication Methods:**

### 1. Password Authentication
```json
{
  "auth": {
    "method": "password",
    "username": "admin",
    "password": "secure_hash"
  }
}
```

### 2. Token Authentication
```json
{
  "auth": {
    "method": "token",
    "token": "eyJhbGciOiJIUzI1NiIs..."
  }
}
```

### 3. API Key Authentication
```json
{
  "auth": {
    "method": "api_key",
    "key": "sk_live_xxx"
  }
}
```

**Session Management:**

```json
// Login
{ "login": { "username": "admin", "password": "***" } }
// Response
{ "token": "xxx", "expires_at": "2024-01-16T00:00:00Z" }

// Logout
{ "logout": {} }

// Refresh token
{ "refreshToken": {} }
```

**Password Storage:**
- Algorithm: Argon2id (memory-hard)
- Salt: 16 bytes random
- Hash: 32 bytes

**System Users:**
| Username | Role | Description |
|----------|------|-------------|
| `root` | superuser | Created on first init |
| `system` | system | Internal operations |
| `public` | anonymous | Unauthenticated access |

## arch-015-authorization

Authorization (RBAC - Role-Based Access Control)

**Permission Model:**

```
User -> Role -> Permission -> Resource
```

**Resource Types:**
- `server` - Server-level operations
- `database` - Database-level operations
- `store` - Store-level operations
- `table` - Table-level operations
- `index` - Index-level operations

**Permission Actions:**
| Action | Description |
|--------|-------------|
| `create` | Create new resource |
| `read` | Read/query data |
| `update` | Modify data |
| `delete` | Delete data |
| `manage` | Full control (includes all above) |
| `grant` | Grant permissions to others |
| `revoke` | Revoke permissions |

**Predefined Roles:**

| Role | Permissions |
|------|-------------|
| `superuser` | Full access to everything |
| `admin` | Manage databases, users, roles |
| `db_owner` | Full access to specific database |
| `read_write` | Read and write data |
| `read_only` | Read data only |
| `public` | Minimal access |

**Permission Commands:**

```json
// Create role
{
  "createRole": {
    "name": "analyst",
    "permissions": [
      { "resource": "database:analytics", "action": "read" },
      { "resource": "table:analytics.*", "action": "read" }
    ]
  }
}

// Grant role to user
{
  "grantRole": {
    "user": "john",
    "role": "analyst"
  }
}

// Revoke role from user
{
  "revokeRole": {
    "user": "john",
    "role": "analyst"
  }
}

// Check permission
{
  "checkPermission": {
    "user": "john",
    "resource": "table:analytics.users",
    "action": "read"
  }
}
```

**Permission Resolution:**
1. Check user's direct permissions
2. Check user's role permissions
3. Check inherited roles
4. Default: deny

## arch-016-admin-commands

Administration Commands

### Server Management

```json
// Server status
{ "serverStatus": {} }

// Server configuration
{ "serverConfig": { "get": "max_connections" } }
{ "serverConfig": { "set": { "max_connections": 100 } } }

// Shutdown server
{ "shutdown": { "delay": 5000 } }
```

### Database Management

```json
// Create database
{
  "createDatabase": {
    "name": "mydb",
    "options": {
      "backend": "sled",
      "path": "./data/mydb"
    }
  }
}

// Drop database
{
  "dropDatabase": {
    "name": "mydb",
    "confirm": true
  }
}

// Rename database
{
  "renameDatabase": {
    "from": "old_name",
    "to": "new_name"
  }
}

// List databases
{ "listDatabases": {} }

// Use database (switch context)
{ "use": "mydb" }
```

### Store Management

```json
// Create store
{
  "createStore": {
    "name": "users",
    "database": "mydb"
  }
}

// Drop store
{
  "dropStore": {
    "name": "users",
    "database": "mydb"
  }
}

// Rename store
{
  "renameStore": {
    "database": "mydb",
    "from": "old_name",
    "to": "new_name"
  }
}

// List stores
{ "listStores": { "database": "mydb" } }
```

### Table Management

```json
// Create table
{
  "createTable": {
    "name": "users",
    "store": "default",
    "schema": {
      "name": { "type": "string", "required": true },
      "email": { "type": "string", "unique": true },
      "age": { "type": "integer", "min": 0 }
    }
  }
}

// Drop table
{
  "dropTable": {
    "name": "users",
    "confirm": true
  }
}

// Rename table
{
  "renameTable": {
    "from": "old_name",
    "to": "new_name"
  }
}

// Truncate table (delete all records)
{
  "truncateTable": {
    "name": "users"
  }
}

// List tables
{ "listTables": {} }

// Table info
{ "tableInfo": { "name": "users" } }
```

### Index Management

```json
// Create index
{
  "createIndex": {
    "table": "users",
    "name": "by_email",
    "fields": ["email"],
    "unique": true
  }
}

// Create composite index
{
  "createIndex": {
    "table": "orders",
    "name": "by_user_date",
    "fields": ["user_id", "created_at"],
    "unique": false
  }
}

// Drop index
{
  "dropIndex": {
    "table": "users",
    "name": "by_email"
  }
}

// Rebuild index
{
  "rebuildIndex": {
    "table": "users",
    "name": "by_email"
  }
}

// List indexes
{ "listIndexes": { "table": "users" } }
```

### User Management

```json
// Create user
{
  "createUser": {
    "username": "john",
    "password": "***",
    "roles": ["read_write"]
  }
}

// Drop user
{
  "dropUser": {
    "username": "john"
  }
}

// Update user password
{
  "updatePassword": {
    "username": "john",
    "oldPassword": "***",
    "newPassword": "***"
  }
}

// List users
{ "listUsers": {} }

// User info
{ "userInfo": { "username": "john" } }
```

## arch-017-transaction-manager

Transaction Manager (Future)

**Purpose:** Atomic multi-operation transactions.

**ACID Guarantees:**
- Atomicity: All or nothing
- Consistency: Data integrity maintained
- Isolation: Concurrent transactions don't interfere
- Durability: Committed transactions survive crashes

**Transaction Commands:**

```json
// Begin transaction
{ "begin": {} }
// Response: { "transaction_id": "tx_123" }

// Execute in transaction
{ "execute": { "tx": "tx_123", "query": { ... } } }

// Commit transaction
{ "commit": { "tx": "tx_123" } }

// Rollback transaction
{ "rollback": { "tx": "tx_123" } }
```

**Isolation Levels:**
| Level | Description |
|-------|-------------|
| `read_uncommitted` | Can read uncommitted changes |
| `read_committed` | Only committed data visible |
| `repeatable_read` | Consistent reads within transaction |
| `serializable` | Full isolation |

**Implementation Status:** NOT IMPLEMENTED
- Storage backends have their own transaction support
- S.H.A.M.I.R. layer will provide unified API
- Priority: After Query Planner

## arch-018-network-layer

Network Layer (Future)

**Supported Protocols:**

### 1. TCP (Binary Protocol)
- MessagePack-framed messages
- Connection pooling
- TLS support

### 2. HTTP/REST
- JSON API
- WebSocket for real-time
- OpenAPI documentation

### 3. gRPC (Optional)
- Protocol Buffers
- Streaming support

**Connection Command:**

```
# TCP
shamir-cli connect --host localhost --port 7331

# HTTP
curl http://localhost:7331/api/v1/query \
  -H "Authorization: Bearer xxx" \
  -d '{"find": {"from": "users"}}'
```

**Implementation Status:** NOT IMPLEMENTED
- Priority: After Query Language
- Required for production use
