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
4. Reliability: WAL, Checksums, Crash safety

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
- Nil: null value
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

**Table Context (High-level):**
- Manages table name, interner, record counter
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
- Codec: Serialization errors
- Internal: Internal logic errors

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
