# Core Abstractions

This module contains fundamental abstractions used throughout S.H.A.M.I.R.:

- **Interning** (`interner/`) - String to compact binary ID mapping with dynamic sizing

## Interning System

### Purpose
The interner reduces memory usage by mapping frequently used strings to compact variable-size binary IDs.
Instead of always storing 8-byte `u64`, the system adapts key size based on the ID value:
- **1 byte** for IDs <= 255
- **2 bytes** for IDs <= 65,535
- **4 bytes** for IDs <= 4 billion
- **8 bytes** for larger IDs

This adaptive sizing saves memory while maintaining the ability to handle massive datasets.

### Module Structure

```
core/
└── interner/
    ├── mod.rs              # Re-exports: InternerKey, Interner, TouchInd, UserKey
    ├── interner.rs         # Interner struct (main two-way map)
    ├── interned_key.rs     # InternerKey (compact binary key)
    ├── touch_ind.rs        # TouchInd enum (New/Exists)
    ├── user_key.rs         # UserKey wrapper for original strings
    └── tests/
        ├── mod.rs
        └── interner_tests.rs
```

### How It Works
```rust
// First touch: assigns new ID
"username" -> 1 (as 1-byte key)

// Subsequent touches: returns existing ID
"username" -> 1 (same 1-byte key)

// Each string gets unique ID
"email" -> 2 (still 1-byte)
// After 255 strings, new keys get 2-byte IDs
// After 65535 strings, new keys get 4-byte IDs
```

### Data Structures

- **InternerKey**: `pub struct InternerKey(pub Bytes)` - Compact binary representation
  - Stores variable-size bytes (1, 2, 4, or 8)
  - Serializable via MessagePack as compact `bin8` format
  - Converts to/from `u64` with `id()` and `new(id)`
  - Hash and Eq based on `id()`, not raw bytes (keys of different sizes can match)

- **UserKey**: `pub struct UserKey(pub String)` - Original string before interning
  - Wraps user-provided strings
  - Used for lookups in forward map

- **TouchInd**: `pub enum TouchInd { New(InternerKey), Exists(InternerKey) }`
  - Returns from `touch_ind()` indicating if key was created or reused
  - `is_new()` method checks if this was a new key

- **Interner**: Main struct containing:
  - **Forward Map**: `TDashMap<UserKey, InternerKey>` - String to Compact ID
  - **Reverse Map**: `TDashMap<InternerKey, UserKey>` - Compact ID to String
  - **Current ID**: `Mutex<u64>` counter for new assignments

### Key Methods

| Method | Description |
|--------|-------------|
| `Interner::new()` | Creates empty interner |
| `Interner::with_state(Vec<(InternerKey, UserKey)>)` | Hydrate from persistent store |
| `touch_ind(str)` -> `Result<TouchInd>` | Get or create ID for a string |
| `get_str(id)` -> `Option<UserKey>` | Reverse lookup: ID to string |
| `get_ind(str)` -> `Option<InternerKey>` | Forward lookup: string to ID (without creating) |
| `all_entries()` -> `Vec<(InternerKey, UserKey)>` | All interned pairs (for persistence) |
| `len()` | Number of interned keys |
| `is_empty()` | Whether interner is empty |
| `make_key(u64)` -> `InternerKey` | Create key from numeric ID |

### Thread Safety
- `TDashMap` (DashMap with FxHasher) enables lock-free concurrent reads
- `touch_ind()` is thread-safe - uses entry API for race condition handling
- Deterministic across threads (critical for correctness!)

### Example
```rust
use shamir_types::core::interner::{Interner, TouchInd};

let interner = Interner::new();

// First usage
let result1 = interner.touch_ind("username").unwrap();
assert!(result1.is_new()); // Key was newly created

// Reuse same string
let result2 = interner.touch_ind("username").unwrap();
assert!(!result2.is_new()); // Key already existed

// Reverse lookup
let name = interner.get_str(result1.key()).unwrap();
assert_eq!(name.as_str(), "username");

// Forward lookup (without creating)
let key = interner.get_ind("username");
assert!(key.is_some());

// Get all entries (for persistence)
let entries = interner.all_entries();
assert!(!entries.is_empty());
```

### Persistence

The interner can be serialized to/from a vector for disk storage:
```rust
// Save state to disk
let state = interner.all_entries(); // Vec<(InternerKey, UserKey)>
// ... write state to file with fsync

// Restore from disk
let state = load_from_file("interner_state.bin");
let interner = Interner::with_state(state);
```

### Performance
- **Memory**: Adaptive key sizing saves space for small datasets
- **Speed**: O(1) hash lookup for both directions
- **Concurrency**: Lock-free reads, fine-grained writes via DashMap
- **Serialization**: Compact MessagePack format saves disk space

---

## Design Decisions

### Why Interning?
1. **Memory**: String objects are expensive (heap allocation + metadata)
2. **Comparison**: Binary key comparison is faster than string comparison
3. **Storage**: Smaller records = better cache locality

### Why DashMap (TDashMap)?
- **Concurrent**: Multiple threads can intern simultaneously
- **Performance**: FxHasher for fast hashing
- **Scalability**: Sharded for parallelism

### Why InternerKey instead of u64?
- **Variable size**: 1-8 bytes depending on ID value
- **Memory efficient**: Most datasets need < 256 keys (1 byte each)
- **Compact serialization**: MessagePack bin8 format

---

## Future Enhancements
- [ ] Interning garbage collection (remove unused strings)
- [ ] Interning statistics (hit rate, memory saved)
- [ ] Custom interning strategies (LRU, size-based)
