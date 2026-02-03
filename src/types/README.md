# Type Definitions

Core data types used throughout S.H.A.M.I.R. database.

## Modules

- **`value.rs`** - `Value<T>` enum (the main data representation)
- **`record_id.rs`** - `RecordId` (unique 128-bit record identifier)
- **`base.rs`** - Base traits and common types
- **`common.rs`** - Shared utilities
- **`repo_record.rs`** - Repository-level records

## Value<T> Enum

The primary data representation in S.H.A.M.I.R.

### Definition

```rust
pub enum Value<T> {
    Null,
    Bool(bool),
    Int(i64),
    UInt(u64),
    Float(f64),
    String(T),          // T = String for UserValue
    Str(T),             // T = String for UserValue
    Binary(Vec<u8>),
    Array(Vec<Value<T>>),
    Object(HashMap<T, Value<T>>),
    Decimal(Decimal),
    BigInt(Box<BigInt>),
}
```

### Type Aliases

```rust
// User-facing: uses readable strings
pub type UserValue = Value<String>;

// Internal: uses compact u64 IDs
pub type InnerValue = Value<u64>;
```

### Examples

```rust
use shamir_db::types::value::Value;

// Simple values
let null = Value::<String>::Null;
let bool = Value::Bool(true);
let int = Value::Int(42);
let float = Value::Float(3.14);
let str = Value::Str("hello");
let binary = Value::Binary(vec![1, 2, 3]);

// Complex nested structures
let obj = Value::Object(map![
    ("name".into(), Value::Str("Alice".into())),
    ("age".into(), Value::Int(30)),
    ("tags".into(), Value::Array(vec![
        Value::Str("rust".into()),
        Value::Str("database".into())
    ]))
];

// Decimal and BigInt for precision
let decimal = Value::Decimal("123.456".parse().unwrap());
let bigint = Value::BigInt(Box::new(BigInt::from(1000i64)));
```

### Memory Layout

**Without Interning:**
```
Value::Object(map![
    ("name", "Alice"),           // "name" = heap, "Alice" = heap
    ("email", "alice@test.com")   // "email" = heap, "alice@test.com" = heap
])
```
Each string = separate heap allocation.

**With Interning:**
```
Value::Object(map![
    (1, 2),  // name → 1, Alice → 2 (both u64)
    (3, 4)   // email → 3, alice@test.com → 4
])
```
All u64 - stored inline, no heap allocations!

### Serialization

All values support serialization/deserialization:

```rust
use shamir_db::types::value::InnerValue;

let value = InnerValue::Int(42);
let bytes = value.to_bytes();  // Vec<u8>
let restored = InnerValue::from_bytes(bytes).unwrap();
```

Format: `bincode` for compact binary representation.

---

## RecordId

Unique 128-bit identifier for each record.

### Structure

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RecordId([u8; 16]);
```

### Features

- **128-bit**: Practically collision-free (2^64 possible IDs per database)
- **UUID-based**: Generated from UUID v4 random bytes
- **Cryptographically random**: Uses `getrandom` syscall
- **Copy**: Cheap to clone (just 16 bytes)
- **Hash + Eq**: Can be used as HashMap key

### Creation

```rust
use shamir_db::types::record_id::RecordId;

// Generate new random ID
let id = RecordId::new();

// Parse from bytes
let bytes = [1u8; 16];
let id = RecordId(bytes);

// System records (metadata)
let sys_id = RecordId::system("internals");

// Convert to/from bytes
let bytes = id.as_bytes(); // &[u8; 16]
let arr: [u8; 16] = id.into();
```

### System Records

Special IDs for metadata (stored in `__info__` store):

```rust
RecordId::system("internals")  // Interning state
RecordId::system("inter_max")   // Next interned ID
RecordId::system("indexes")     // Index metadata (future)
```

System IDs start with `0xFFFF` prefix to avoid collision with user records.

### Display

```rust
let id = RecordId::new();
println!("{}", id);  // e.g., "a1b2c3d4-e5f6-7890-abcd-ef1234567890"
```

---

## Base Types (`base.rs`)

Foundational traits and types.

### Key Traits

```rust
pub trait RepoKey: Sealed {}
impl RepoKey for String {}
impl RepoKey for u64 {}
```

Restricts repository key types to `String` or `u64` (for interned).

---

## Common Utilities (`common.rs`)

Shared helper functions.

---

## Design Decisions

### Why Value<T> Generic?

Allows same enum for both user-facing and internal representations:
- **UserValue**: `Value<String>` - readable strings
- **InnerValue**: `Value<u64>` - compact IDs

Same operations, different key types!

### Why 128-bit RecordId?

- **Collision resistant**: 2^64 IDs per database
- **UUID compatible**: Can store/transport UUIDs
- **Sort stable**: Can lexicographically sort
- **Not sequential**: No information leakage

### Why bincode?

- **Compact**: Smaller than JSON/msgpack
- **Fast**: Zero-copy deserialization
- **Type-safe**: Compile-time type checking

---

## Performance Characteristics

### Value<T>

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

## Usage Examples

### Creating Values

```rust
use shamir_db::types::value::Value;
use shamir_db::types::record_id::RecordId;

// User value (with strings)
let user_val: Value<String> = Value::Object(map![
    ("name".into(), Value::Str("Alice".into()))
]);

// Internal value (with IDs)
let inner_val: Value<u64> = Value::Object(map![
    (1, 2)  // name→1, Alice→2 (interned)
]);

// Convert
let id = RecordId::new();
```

### Working with RecordId

```rust
use shamir_db::types::record_id::RecordId;

// Generate ID
let id = RecordId::new();

// Use as map key
let mut map = HashMap::new();
map.insert(id, "data");

// System records
let meta_id = RecordId::system("internals");

// Display
println!("Record: {}", id);
```

---

## Future Enhancements

- [ ] Value::Timestamp for datetime
- [ ] Value::Geometry for spatial data
- [ ] Value::Uuid type
- [ ] RecordId v7 (time-ordered)
- [ ] Custom serialization formats
