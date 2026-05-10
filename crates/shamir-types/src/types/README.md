# Type Definitions

Core data types used throughout S.H.A.M.I.R. database.

## Modules

- **`value.rs`** - `Value<T>` enum (the main data representation)
- **`record_id.rs`** - `RecordId` (unique 128-bit record identifier)
- **`base.rs`** - Base traits and common types
- **`common.rs`** - Collection types and utilities (TMap, TSet, TDashMap)
- **`repo_record.rs`** - Repository-level records

## Collection Types (`common.rs`)

S.H.A.M.I.R. uses FxHash-based ordered collections for predictable iteration:

```rust
pub type THasher = BuildHasherDefault<FxHasher>;

/// Ordered map (IndexMap with FxHash)
pub type TMap<K, V> = IndexMap<K, V, THasher>;

/// Ordered set (IndexSet with FxHash)
pub type TSet<T> = IndexSet<T, THasher>;

/// Concurrent map (DashMap with FxHash)
pub type TDashMap<K, V> = DashMap<K, V, THasher>;
```

### Factory Functions

| Function | Description |
|----------|-------------|
| `new_map()` | Empty TMap |
| `new_map_wc(cap)` | TMap with capacity |
| `new_set()` | Empty TSet |
| `new_set_wc(cap)` | TSet with capacity |
| `new_dash_map()` | Empty TDashMap |
| `new_dash_map_wc(cap)` | TDashMap with capacity |

## Value<Key> Enum

The primary data representation in S.H.A.M.I.R.

### Definition

```rust
pub enum Value<Key: Eq + Hash + Ord + Clone + Serialize + Debug> {
    Null,
    Bool(bool),
    Int(i64),
    F64(f64),
    Dec(Decimal),
    Big(BigInt),
    Str(String),
    Bin(Vec<u8>),
    List(Vec<Value<Key>>),
    Set(TSet<Value<Key>>),
    Map(TMap<Key, Value<Key>>),
}
```

### Type Aliases

```rust
// User-facing: uses readable strings (DEPRECATED - for tests only)
#[deprecated]
pub type UserValue = Value<String>;

// Query values (for JSON query parsing)
pub type QueryValue = Value<String>;

// Internal: uses compact InternerKey IDs
pub type InnerValue = Value<InternerKey>;
```

**Important:** `InnerValue` uses `InternerKey` (not `u64`). InternerKey is a compact binary key (1-8 bytes) from the interner module.

### Examples

```rust
use shamir_types::types::value::Value;

// Simple values
let null = Value::<String>::Null;
let boolean = Value::Bool(true);
let int = Value::Int(42);
let float = Value::F64(3.14);
let str = Value::Str("hello".to_string());
let binary = Value::Bin(vec![1, 2, 3]);

// Collection values
let list = Value::List(vec![Value::Int(1), Value::Int(2)]);
let set: Value<String> = Value::Set(new_set());  // Ordered set (no duplicates)
let map: Value<String> = Value::Map(new_map());   // Ordered map

// Precision types
let decimal = Value::Dec("123.456".parse().unwrap());
let bigint = Value::Big(BigInt::from(1000i64));
```

### Serialization

Values serialize to/from MessagePack:

```rust
use shamir_types::types::value::InnerValue;

let value = InnerValue::Int(42);
let bytes = value.to_bytes();  // Bytes (MessagePack format)
let restored = InnerValue::from_bytes(&bytes).unwrap();
```

### Memory Layout

**Without Interning (UserValue):**
```
Value::Map(map![
    ("name", "Alice"),           // "name" = heap, "Alice" = heap
    ("email", "alice@test.com")  // "email" = heap, "alice@test.com" = heap
])
```
Each string = separate heap allocation.

**With Interning (InnerValue):**
```
Value::Map(map![
    (InternerKey(1), InternerKey(2)),  // 1-byte keys
    (InternerKey(3), InternerKey(4))
])
```
Compact binary keys, no heap allocations for small ID values.

---

## RecordId

Unique 128-bit identifier for each record.

### Structure

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RecordId([u8; 16]);
```

### Features

- **128-bit**: Practically collision-free
- **UUID-based**: Generated from UUID v4 random bytes
- **Cryptographically random**: Uses `getrandom` syscall
- **Copy**: Cheap to clone (just 16 bytes)
- **Hash + Eq**: Can be used as HashMap key

### Creation

```rust
use shamir_types::types::record_id::RecordId;

// Generate new random ID
let id = RecordId::new();

// System records (metadata)
let sys_id = RecordId::system("internals");

// Convert to/from bytes
let bytes = id.as_bytes(); // &[u8; 16]
```

### System Records

Special IDs for metadata (stored in `__info__` store):

```rust
RecordId::system("internals")  // Interning state
RecordId::system("inter_max")  // Next interned ID
RecordId::system("indexes")    // Index metadata
```

System IDs start with `0xFFFF` prefix to avoid collision with user records.

---

## Base Types (`base.rs`)

Foundational traits and types.

```rust
pub trait RepoKey: Sealed {}
impl RepoKey for String {}
impl RepoKey for u64 {}
```

---

## Design Decisions

### Why Value<Key> Generic?

Allows same enum for both user-facing and internal representations:
- **UserValue**: `Value<String>` - readable strings (deprecated)
- **InnerValue**: `Value<InternerKey>` - compact binary IDs

Same operations, different key types.

### Why InternerKey instead of u64?

InternerKey uses variable-size bytes (1-8 bytes) based on the ID value, providing:
- **Memory efficiency**: Most datasets need < 256 keys (1 byte each)
- **Compact serialization**: MessagePack bin8 format
- **Order-independent equality**: Hash/Eq based on u64 ID value

### Why TMap/TSet (IndexMap/IndexSet)?

- **Ordered**: Insertion order preserved for predictable iteration
- **FxHash**: Fast hashing for integer and string keys
- **Deterministic**: Same data always iterates in same order

### Why 128-bit RecordId?

- **Collision resistant**: 2^64 IDs per database
- **UUID compatible**: Can store/transport UUIDs
- **Not sequential**: No information leakage

---

## Performance Characteristics

### Value<Key>

| Operation | Complexity | Notes |
|-----------|------------|-------|
| Clone | O(n) | Deep copy (recursive) |
| Serialize | O(n) | Visit all nodes |
| Deserialize | O(n) | Reconstruct tree |
| Access | O(1) | Direct field access |

### RecordId

| Operation | Complexity | Notes |
|-----------|------------|-------|
| new() | O(1) | Syscall to getrandom |
| Clone | O(1) | Copy 16 bytes |
| Hash | O(1) | Precomputed |
| as_bytes() | O(1) | Reinterpret cast |

---

## Future Enhancements

- [ ] Value::Timestamp for datetime
- [ ] Value::Geometry for spatial data
- [ ] Value::Uuid type
- [ ] RecordId v7 (time-ordered)
- [ ] Custom serialization formats
