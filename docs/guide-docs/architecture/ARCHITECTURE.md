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
- Storage backends: Fjall (durable production backend) + InMemory (tests), feature-gated
- Error handling: thiserror, anyhow

## arch-002-storage

Storage Backend Abstraction Layer

S.H.A.M.I.R. provides a unified interface over the storage layer: Fjall (the
durable production backend) plus an always-available InMemory backend used
by tests. Earlier prototypes explored several other embedded engines (Sled,
Redb, Nebari, Persy, Canopy) before the workspace settled on Fjall; see
`docs/dev-artifacts/perf/backend-decision-2026-06-19.md` for that decision record.

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

**Type Hints (for UserValue / legacy text-encoding deserialization only):**
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
1. MessagePack: Binary, rmp-serde (primary format — records and wire)
2. Legacy text encoding: Human-readable (legacy/v1 wire; kept for tooling interop only)

**Type Mapping:**

| Rust Type | MessagePack | Legacy text encoding |
|-----------|-------------|----------------------|
| Null | nil | null |
| Bool | bool | true/false |
| Int(i64) | int64 | number |
| UInt(u64) | uint64 | number |
| Float(f64) | float64 | number |
| String | str | string |
| Binary | bin | base64 string |
| Array | array | array |
| Object | map | object |
| Decimal | str (decimal) | string (decimal) |
| BigInt | str (bigint) | string (bigint) |

**Performance:**
| Format | Size | Encode | Decode |
|--------|------|-------|--------|
| MessagePack | ~60% | 1.2x | 1.3x |
| Legacy text encoding | 100% | 1x | 1x |

**Key Differences:**
- MessagePack: Primary storage and wire format; no type-hint prefixes needed
- Legacy text encoding: Type hint support (i:, u:, float:, dec:, big:, arr:, set:) via UserValue; being phased out of the wire
- BigInt/Decimal: Serialized as strings in both formats for precision preservation

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
- In tests: MessagePack/QueryValue literals must be formatted, multi-line
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

Query Language (OQL — Object Query Language)

**Design Philosophy:**
- Object-native: Queries are typed DTOs carried as MessagePack on the wire
- Familiar: MongoDB-style find/update syntax
- Index-aware: Query planner uses indexes automatically
- Pipeline-based: Composable operations

**Principle: OQL (Object Query Language) — no text language, ever.**
SDBQL is an *object* query language: a query is a typed data structure
(`Filter` / `ReadQuery` / `BatchRequest` DTO) carried as MessagePack and
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

**Client-side query construction — use the TS builder:**

Reads:

```ts
import { Query, filter, select } from '@shamir/client';

// eq filter
const db = client.db('my_app');
const rows = await db.query('users').where(filter.eq('status', 'active')).rows();

// Nested field path
const rows2 = await db.query('users').where(filter.eq(['address', 'city'], 'NYC')).rows();

// Aggregation
const qr = await db.query('orders')
  .select([select.countAll('n'), select.sum('amount', { alias: 'total' })])
  .ex();
```

Writes:

```ts
import { write } from '@shamir/client';

// insert
await db.run(write.insert('users', [{ id: 'A1', name: 'Alice' }]));

// update
await db.run(write.update('users').where(filter.eq('id', 'A1')).set({ name: 'Alice V2' }));

// upsert
await db.run(write.upsert('users', { id: 'A1' }, { id: 'A1', name: 'Alice V3' }));

// delete
await db.run(write.del('users', filter.eq('id', 'A1')));
```

### Special Value References

SDBQL поддерживает специальные ссылки в значениях:

| Ссылка | Описание | Builder |
|--------|----------|---------|
| `$query` | Ссылка на результат другого запроса | `filter.queryRef('@alias', '[0].id')` |
| `$ref` | Ссылка на другое поле записи | `filter.ref(['address', 'city'])` |
| `$fn` | Системная функция | `filter.fn('NOW')` / `filter.fn('COALESCE', [null, 'x'])` |
| `$expr` | Выражение (арифметика, строки) | `filter.expr('add', [10, 20])` |
| `$cond` | Условный оператор | `filter.cond(filter.eq('active', true), 'yes', 'no')` |

```ts
filter.fn('NOW')                        // { "$fn": "NOW" }
filter.fn('COALESCE', [null, 'default']) // { "$fn": { "name": "COALESCE", "args": [null, "default"] } }
filter.expr('mul', [filter.ref('price'), 1.1])  // { "$expr": { "op": "mul", "args": [{ "$ref": ["price"] }, 1.1] } }
filter.cond(filter.gte('score', 100), 'vip', 'regular')
// { "$cond": { "if": { "op": "gte", "field": ["score"], "value": 100 }, "then": "vip", "else": "regular" } }
```

### Системные функции ($fn)

**Категории:**
- Дата/время: `NOW`, `TODAY`, `UNIX_TIMESTAMP`
- Генерация: `UUID`, `RANDOM`, `RANDOM_INT`
- Строки: `LENGTH`, `UPPER`, `LOWER`, `TRIM`, `SUBSTRING`
- Логические: `COALESCE`, `IFNULL`, `NULLIF`
- Хеширование: `MD5`, `SHA256`
- Математика: `ABS`, `ROUND`, `FLOOR`, `CEIL`

### Выражения ($expr)

Арифметические и строковые операции.

**Операторы:**
- Математика: `add`, `sub`, `mul`, `div`, `mod`, `neg`
- Строки: `concat`, `lower`, `upper`, `trim`, `length`
- Логика: `and`, `or`, `not`
- Сравнение: `eq`, `ne`, `gt`, `gte`, `lt`, `lte`

**Builder:**
```ts
filter.expr('add', [10, 20])
filter.expr('mul', [filter.ref('price'), 1.1])
filter.expr('concat', [filter.ref('first'), ' ', filter.ref('last')])
```

### Условия ($cond)

Условный оператор (тернарный).

**Builder:**
```ts
filter.cond(filter.eq('active', true), 'yes', 'no')
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

**Index Selection Strategy (wire form; clients build this via the query builder):**

```
Query: filter.eq('email', 'alice@example.com')
Available Indexes: ["by_email" (unique), "by_name" (regular)]

Plan:
  - Use index: "by_email" (unique)
  - Estimated cost: O(1)
  - Execution: Index lookup -> Single record
```

**Composite Index Usage:**

```
Query: filter.and([filter.eq('status', 'active'), filter.eq('department', 'engineering')])
Available Indexes: ["by_status_dept" (composite: status, department)]

Plan:
  - Use index: "by_status_dept"
  - Estimated cost: O(log n)
  - Execution: Index range scan
```

**Full Scan Fallback:**

```
Query: filter.eq('temp_field', 'value')
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

### 1. SCRAM-Argon2id (primary — used by TS/Rust clients)

The `connect()` call performs the full SCRAM-Argon2id handshake:

```ts
import { connect } from '@shamir/client';

const client = await connect({
  host: '127.0.0.1',
  port: 13760,
  username: 'admin',
  password: 'correct horse battery staple',
  tls: { rejectUnauthorized: false },
  origin: 'https://127.0.0.1',
});
```

The raw wire shapes below document the handshake protocol (wire form; clients build this via `connect()`):

```msgpack
{ "op": "auth_init", "username": "admin", "client_nonce": "<bytes>" }
```

```msgpack
{ "op": "auth_finish", "client_proof": "<bytes>" }
```

### 2. User provisioning

```ts
// Create a new login-capable user
const created = await client.createScramUser('bob', 'password', ['reader']);
created.name;    // 'bob'
created.user_id; // Uint8Array(16)
```

**Password Storage:**
- Algorithm: Argon2id (memory-hard)
- Salt: 16 bytes random
- Hash: 32 bytes

**System Users:**
| Username | Role | Description |
|----------|------|-------------|
| `admin` | superuser | Created on bootstrap |
| `system` | system | Internal operations |

## arch-015-authorization

Authorization (POSIX-style DAC + RBAC)

**Permission Model:**

```
User -> Role -> Permission -> Resource
```

**Resource Types:**
- `database` - Database-level operations
- `store` (repo) - Repo-level operations
- `table` - Table-level operations
- `function` - Function-level operations

**Permission Actions:**
| Action | Description |
|--------|-------------|
| `read` | Read/query data |
| `insert` | Insert data |
| `update` | Modify data |
| `delete` | Delete data |
| `create` | Create new resource |
| `drop` | Drop resource |
| `alter` | Alter resource |
| `manage_users` | Manage users |
| `manage_roles` | Manage roles |
| `all` | All of the above |

**RBAC via the builder:**

```ts
import { admin, Batch } from '@shamir/client';

// Create role
await Batch.create('mk-role')
  .add('r', admin.createRole('analyst', [
    admin.permission('allow', ['read'], admin.scopeDatabase('analytics')),
  ]))
  .execute(client, 'default');

// Grant role to user
await Batch.create('grant')
  .add('g', admin.grantRole('analyst', 'alice'))
  .execute(client, 'default');

// Revoke role from user
await Batch.create('revoke')
  .add('r', admin.revokeRole('analyst', 'alice'))
  .execute(client, 'default');
```

**ACL (chmod/chown/chgrp) via the builder:**

```ts
// chmod
await Batch.create('chmod')
  .add('cm', admin.chmod(admin.refTable('mydb', 'main', 'secret'), 0o700))
  .execute(client, 'mydb');

// chown
await Batch.create('chown')
  .add('co', admin.chown(admin.refDatabase('mydb'), userId))
  .execute(client, 'default');
```

**Permission Resolution:**
1. Check user's direct permissions
2. Check user's role permissions
3. Check inherited roles
4. Default: deny

## arch-016-admin-commands

Administration Commands

### Database Management

```ts
import { ddl, Batch } from '@shamir/client';

// Create database
await Batch.create('create-db')
  .add('mk', ddl.createDb('mydb'))
  .execute(client, 'default');

// Drop database (HMAC-gated)
await Batch.create('drop-db')
  .add('d', ddl.dropDb(client, 'mydb', { cascade: true }))
  .execute(client, 'default');

// List databases
const resp = await Batch.create('list-dbs')
  .add('l', ddl.listDatabases())
  .execute(client, 'default');
resp.results.l.records[0].databases; // string[]
```

### Repo Management

```ts
const db = client.db('mydb');

// Create repo
await db.run(ddl.createRepo('cold'));

// Drop repo (HMAC-gated, via handle)
await db.dropRepo('cold', { cascade: true });

// List repos
const resp = await Batch.create('list-repos')
  .add('l', ddl.listRepos())
  .execute(client, 'mydb');
resp.results.l.records[0].repos; // string[]
```

### Table Management

```ts
// Create table
await db.run(ddl.createTable('users', { repo: 'main' }));

// Drop table (HMAC-gated, via handle)
const qr = await db.dropTable('main', 'users');
qr.records[0]; // { dropped_table: 'users', existed: true }

// List tables
const resp = await Batch.create('list-tables')
  .add('l', ddl.listTables({ repo: 'main' }))
  .execute(client, 'mydb');
resp.results.l.records[0].tables; // string[]
```

### Index Management

```ts
// Create index
await Batch.create('mk-idx')
  .add('i', ddl.createIndex('by_email', 'users', [['email']], { unique: true }))
  .execute(client, 'mydb');

// List indexes
const resp = await Batch.create('list-idx')
  .add('l', ddl.listIndexes('users', { repo: 'main' }))
  .execute(client, 'mydb');
resp.results.l.records[0].indexes; // Array<{ name: string }>

// Drop index (HMAC-gated)
await Batch.create('drop-idx')
  .add('d', ddl.dropIndex(client, 'mydb', 'main', 'users', 'by_email'))
  .execute(client, 'mydb');
```

### User Management

```ts
// Create user
const user = await client.createScramUser('john', 'password', ['read_write']);

// Drop user (HMAC-gated)
await Batch.create('drop-user')
  .add('d', admin.dropUser(client, 'john'))
  .execute(client, 'default');

// List users
const resp = await Batch.create('list-users')
  .add('u', ddl.listUsers())
  .execute(client, 'default');
```

## arch-017-transaction-manager

Transaction Manager

**Purpose:** Atomic multi-operation transactions.

**ACID Guarantees:**
- Atomicity: All or nothing
- Consistency: Data integrity maintained
- Isolation: Concurrent transactions don't interfere
- Durability: Committed transactions survive crashes

**Single-batch transactions via the builder:**

```ts
import { write, Batch } from '@shamir/client';

const db = client.db('my_app');

// Auto-commit batch transaction (Snapshot Isolation)
const resp = await db.batch()
  .add('ins', write.insert('items', [{ name: 'widget' }]))
  .transactional()
  .run();

resp.transaction?.status;         // 'committed'
resp.transaction?.tx_id;          // number
resp.transaction?.commit_version; // number

// Serializable
const resp2 = await db.batch()
  .add('ins', write.insert('items', [{ name: 'ssi-item' }]))
  .transactional('serializable')
  .run();
```

**Auto-managed multi-op transactions:**

```ts
await db.tx(async (t) => {
  await t.run(write.insert('acct', [{ id: 'a', bal: 100 }]));
  await t.run(write.update('acct').where(filter.eq('id', 'a')).set({ bal: 90 }));
  const rows = await t.query('acct').rows();   // reads inside the tx
});
// committed automatically; on any throw → rolled back + error rethrown
```

**Interactive (multi-call) transactions:**

```ts
// Begin transaction
const opened = await client.txBegin('my_app', 'main');
// opened.tx_handle, opened.snapshot_version, opened.isolation

// Execute a batch inside the open transaction
await client.txExecute(
  'my_app',
  opened.tx_handle,
  Batch.create('ins').add('i', write.insert('items', [{ id: 'a', bal: 100 }])).build(),
);

// Commit
const info = await client.txCommit('my_app', opened.tx_handle);
// info.status === 'committed', info.commit_version

// — or abort:
// await client.txRollback('my_app', opened.tx_handle);
```

Wire form for raw transaction protocol (wire form; clients build this via the builder):

```msgpack
{ "op": "tx_begin",   "db": "my_app", "repo": "main" }
{ "op": "tx_execute", "tx_handle": 1, "batch": { "…": "…" } }
{ "op": "tx_commit",  "tx_handle": 1 }
{ "op": "tx_rollback","tx_handle": 1 }
```

**Isolation Levels:**
| Level | Description |
|-------|-------------|
| `snapshot` | Consistent reads within transaction (default) |
| `serializable` | Full isolation — aborts on read-write conflict |

## arch-018-network-layer

Network Layer

**Supported Protocols:**

### 1. TCP (Binary Protocol)
- MessagePack-framed messages
- TLS 1.3 (SCRAM-Argon2id auth)

### 2. WebSocket
- MessagePack frames over WSS
- Browser-compatible (native WebSocket)
- TLS 1.3 (SCRAM-Argon2id auth)

**Connection via TS client:**

```ts
import { connect } from '@shamir/client';

// Node.js (TCP or WS)
const client = await connect({
  host: '127.0.0.1',
  port: 13760,
  username: 'admin',
  password: 'correct horse battery staple',
  tls: { rejectUnauthorized: false },
  origin: 'https://127.0.0.1',
});

// Browser
import { connect } from '@shamir/client/browser';
const client = await connect({
  host: 'db.example.com',
  port: 443,
  username: 'reader',
  password: 's3cret',
});
```

Wire envelope shapes (wire form; clients build this via `connect()` + builder):

```
Request:  { "sid": bytes(32), "rid": Optional<u32>, "req": <opaque> }
Response: { "rid": Optional<u32>, "res": <opaque> }
Error:    { "rid": Optional<u32>, "error": String }
```
