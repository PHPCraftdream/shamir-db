# Core Abstractions

This module contains fundamental abstractions used throughout S.H.A.M.I.R.:

- **Interning** (`interner.rs`) - String → Compact binary ID mapping with dynamic sizing
- **Transformations** (`transform.rs`) - UserValue ↔ InnerValue conversions
- **Configuration** (`config.rs`) - YAML configuration loader/saver

## Interning System

### Purpose
The interner reduces memory usage by mapping frequently used strings to compact variable-size binary IDs.
Instead of always storing 8-byte `u64`, the system adapts key size based on the number of interned strings:
- **1 byte** (u8) for < 256 keys
- **2 bytes** (u16) for < 65,536 keys
- **4 bytes** (u32) for < 4 billion keys
- **8 bytes** (u64) for larger datasets

This adaptive sizing saves memory while maintaining the ability to handle massive datasets.

### How It Works
```rust
// First touch: assigns new ID
"username" → 1 (as 1-byte key)

// Subsequent touches: returns existing ID
"username" → 1 (same 1-byte key)

// Each string gets unique ID
"email" → 2 (still 1-byte)
// ... after 255 strings, migrates to 2-byte keys
// ... after 65535 strings, migrates to 4-byte keys
// ... after 4B strings, migrates to 8-byte keys
```

### Data Structures

- **InternedKey**: `pub struct InternedKey(pub Bytes)` - Compact binary representation
  - Stores variable-size bytes (1, 2, 4, or 8)
  - Serializable via MessagePack as compact `bin8` format
  - Converts to/from `u64` with `id()` and `new(id, size)`

- **UserKey**: `pub struct UserKey(pub String)` - Original string before interning
  - Wraps user-provided strings
  - Used for lookups in forward map

- **TouchInd**: `pub enum TouchInd { New(InternedKey), Exists(InternedKey) }`
  - Returns from `touch_ind()` indicating if key was created or reused
  - `is_new()` method checks if this was a new key

- **Interner**: Main struct containing:
  - **Forward Map**: `DashMap<UserKey, InternedKey>` - String → Compact ID
  - **Reverse Map**: `DashMap<InternedKey, UserKey>` - Compact ID → String
  - **Next ID**: `AtomicU64` counter for new assignments
  - **Key Size**: `Mutex<u8>` - Current byte size (1/2/4/8)
  - **Migration Lock**: Ensures only one thread migrates at a time

### Automatic Key Size Migration

The interner automatically migrates to larger key sizes when thresholds are crossed:

| Current Size | Key Count | Migrates To | Reason |
|-------------|------------|---------------|---------|
| 1 byte (u8) | ≥ 256 | 2 bytes (u16) | Exceeds u8::MAX (255) |
| 2 bytes (u16) | ≥ 65,536 | 4 bytes (u32) | Exceeds u16::MAX (65535) |
| 4 bytes (u32) | ≥ 4,000,000,001 | 8 bytes (u64) | Practical limit for u32 |

Migration happens **atomically** during `touch_ind()`:
1. Acquires migration lock
2. Rebuilds all keys with new byte size
3. Updates key size atomically
4. Continues with new key size

### Compact Serialization

`InternedKey` serializes efficiently using MessagePack's `bin8` format:
- **Format**: `0xC4` (marker) + length byte + data bytes
- **1-byte ID**: 3 bytes total (`[0xC4, 0x01, 0x2A]`)
- **2-byte ID**: 4 bytes total
- **4-byte ID**: 6 bytes total
- **8-byte ID**: 10 bytes total

Compare to full u64 serialization:
- **u64 as int64**: 9-10 bytes (MessagePack int64 format)
- **InternedKey**: 3-10 bytes (bin8 format)

**Result**: Small datasets save ~70% per key, large datasets save minimal overhead.

### Thread Safety
- `DashMap` enables lock-free concurrent reads
- `touch_ind()` is thread-safe - same string always gets same ID
- Migration lock prevents concurrent size changes
- Deterministic across threads (critical for correctness!)

### Example
```rust
use shamir_db::core::interner::{Interner, TouchInd};

let interner = Interner::new();

// First usage
let result1 = interner.touch_ind("username").unwrap();
assert!(result1.is_new()); // Key was newly created
assert_eq!(result1.key().id(), 1);
assert_eq!(interner.key_size(), 1); // Using 1-byte keys

// Reuse same string
let result2 = interner.touch_ind("username").unwrap();
assert!(!result2.is_new()); // Key already existed
assert_eq!(result2.key().id(), 1); // Same ID!

// Reverse lookup
let name = interner.get_str(result1.key()).unwrap();
assert_eq!(name.as_str(), "username");

// Add many keys to trigger migration
for i in 0..300 {
    interner.touch_ind(&format!("key_{}", i)).unwrap();
}
assert_eq!(interner.key_size(), 2); // Now using 2-byte keys!
```

### Persistence

The interner can be serialized to/from a vector for disk storage:
```rust
// Save state to disk
let state = interner.to_state(); // Vec<(InternedKey, UserKey)>
// ... write state to file with fsync

// Restore from disk
let state = load_from_file("interner_state.bin");
let interner = Interner::with_state(state);
```

### Performance
- **Memory**: ~70% reduction for small datasets, adapts for large datasets
- **Speed**: O(1) hash lookup for both directions
- **Concurrency**: Lock-free reads, fine-grained writes
- **Migration Cost**: One-time rebuild when threshold crossed, amortized over all subsequent operations
- **Serialization**: Compact MessagePack format saves disk space

---

## Transformations

### Purpose
Convert between user-facing and internal representations:
- **UserValue**: `Value<String>` - what users work with
- **InnerValue**: `Value<u64>` - what's stored in database

### Why?
Users see readable field names, database stores compact IDs.

### Example
```rust
use shamir_db::core::transform;
use shamir_db::types::value::{Value, InnerValue, UserValue};
use shamir_db::core::interner::Interner;

let interner = Interner::new();
let user_value: UserValue = Value::Object(map![
    ("name".into(), Value::Str("Alice".into())),
    ("age".into(), Value::Int(30))
]);

// Transform to internal (with interning)
let inner_value: InnerValue = transform::user_to_inner(&user_value, &interner);

// Transform back to user (reverse lookup)
let user_value2: UserValue = transform::inner_to_user(&inner_value, &interner);
```

### Transformation Rules
- **String**: Interned → u64 ID, Reverse lookup on restore
- **Object Keys**: Interned → u64 IDs
- **Primitives**: Passed through unchanged (Int, Bool, Float, etc.)
- **Arrays/Objects**: Recursively transformed

### Important Notes
- **Interner Required**: Both directions require interner reference
- **Deterministic**: Same string always gets same ID
- **Lossless**: Round-trip preserves original values

---

## Configuration

### Purpose
Load and save YAML configuration files for the database.

### Features
- **Atomic writes**: Uses temp file + rename for safe updates
- **Validation**: Ensures config correctness on load
- **Error handling**: Context-aware errors with `anyhow`

### Example
```rust
use shamir_db::core::config::ConfigLoader;

// Load from file
let config = ConfigLoader::load_from_file("config/database.yaml").unwrap();

// Save to file (atomic write)
ConfigLoader::save_to_file("config/database.yaml", &config).unwrap();

// Validate config
ConfigLoader::validate_config(&config).unwrap();
```

---

## Design Decisions

### Why Interning?
1. **Memory**: String objects are expensive (heap allocation + metadata)
2. **Comparison**: u64 comparison is faster than string comparison
3. **Storage**: Smaller records = better cache locality

### Why DashMap?
- **Concurrent**: Multiple threads can intern simultaneously
- **Performance**: Lock-free reads (flurry algorithm)
- **Scalability**: Sharded for parallelism

### Why Store Interner in Database?
- **Persistence**: Interning survives restarts
- **Shared**: Multiple threads see same IDs
- **System Records**: Stored in `__info__{table}` with RecordId

---

## Future Enhancements
- [ ] Interning garbage collection (remove unused strings)
- [ ] Interning statistics (hit rate, memory saved)
- [ ] Custom interning strategies (LRU, size-based)
