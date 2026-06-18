# Codecs Module

Serialization/deserialization support for various data formats in S.H.A.M.I.R. database.

## Architecture

```
src/codecs/
├── mod.rs              # Re-exports: Codec, CodecError, basic/interned codecs
├── codec.rs            # Codec<T> trait definition
├── error.rs            # CodecError enum (Encode/Decode)
├── basic/              # Generic codecs (no dependencies)
│   ├── mod.rs        # MsgPackCodec, MessagePackCodec, bincode functions
│   ├── legacy_text.rs # Legacy text-encoding stub (orphaned — not compiled)
│   ├── messagepack.rs # MessagePack via rmp_serde
│   └── bincode.rs   # Binary via bincode (functions, not trait)
├── interned/           # Interning-aware codecs
│   ├── mod.rs        # InternedCodec trait, CodecFormat enum
│   ├── codec.rs      # InternedCodec trait definition
│   ├── legacy_text.rs # text_to_inner, inner_to_text (legacy text encoding)
│   ├── messagepack.rs # msgpack_to_inner, inner_to_msgpack
│   └── common.rs     # intern_string_key, deintern_key
├── legacy/            # Deprecated API
│   └── tools.rs      # UserValue ↔ InnerValue (deprecated)
└── tests/            # Shared test utilities
```

## Module Responsibilities

### Basic Codecs (`basic/`)
- Generic over any serializable type `T`
- No dependencies on Interner
- Used for API requests/responses
- Implement `Codec<T>` trait (except bincode - functions only)

### Interned Codecs (`interned/`)
- Specialized for database storage
- Require Interner reference
- Intern keys during decode
- De-intern keys during encode
- Implement `InternedCodec` trait

### Legacy Module (`legacy/`)
- ⚠️ Deprecated since 0.1.0
- Old transform API for manual UserValue ↔ InnerValue conversion
- Will be removed in future version

## Core Traits

### Codec Trait (`codec.rs`)
```rust
pub trait Codec<T: Serialize + DeserializeOwned> {
    fn encode(&self, value: &T) -> Result<Vec<u8>, CodecError>;
    fn decode(&self, bytes: &[u8]) -> Result<T, CodecError>;
}
```

**Design:**
- Generic over `T` - works with any serializable type
- `DeserializeOwned` - type owns deserialized data
- Error handling via `CodecError` from `error.rs` (Encode/Decode variants)

### InternedCodec Trait (`interned/codec.rs`)

All conversion paths go through `QueryValue` (string-keyed) ↔ `InnerValue` (interned keys). The old text-value typed functions have been removed.

```rust
pub trait InternedCodec: Send + Sync {
    fn decode_with_interner(
        &self,
        bytes: &[u8],
        interner: &Interner,
    ) -> Result<InnerValue, CodecError>;

    fn encode_with_interner(
        &self,
        value: &InnerValue,
        interner: &Interner,
    ) -> Result<Vec<u8>, CodecError>;

    fn format_name(&self) -> &'static str;
}
```

**Purpose:**
- Converts external format (MessagePack/QueryValue) to InnerValue with interned keys
- Automatically interns string keys during decode
- De-interns keys during encode back to strings
- Used by TableContext for efficient client data handling
- **Direct InnerValue conversion** - no UserValue (deprecated)

### CodecFormat Enum (`interned/codec.rs`)

```rust
pub enum CodecFormat {
    LegacyText,
    MessagePack,
}
```

**Note:** Only `LegacyText` (the old human-readable encoding) and `MessagePack` are supported. Bincode is NOT included.

**Methods:**
- `codec()` → `Box<dyn InternedCodec>` for this format
- `name()` → format name as string

## Basic Codecs

### LegacyTextCodec (`basic/legacy_text.rs`)

```rust
pub struct LegacyTextCodec;

impl<T: Serialize + DeserializeOwned> Codec<T> for LegacyTextCodec {
    fn encode(&self, value: &T) -> Result<Vec<u8>, CodecError> {
        // Legacy text encoding — not active; file is orphaned from module tree.
        Err(CodecError::Encode("LegacyTextCodec is not active".to_string()))
    }

    fn decode(&self, bytes: &[u8]) -> Result<T, CodecError> {
        // Legacy text encoding — not active; file is orphaned from module tree.
        Err(CodecError::Decode("LegacyTextCodec is not active".to_string()))
    }
}
```

**Features:**
- Human-readable, UTF-8 encoded
- Orphaned reference implementation (not compiled into the crate)
- Active wire format is MessagePack (`MessagePackCodec`)
- No special type handling (relies on serde)

### MessagePackCodec (`basic/messagepack.rs`)

```rust
pub struct MessagePackCodec;

impl<T: Serialize + DeserializeOwned> Codec<T> for MessagePackCodec {
    fn encode(&self, value: &T) -> Result<Vec<u8>, CodecError> {
        rmp_serde::to_vec_named(value).map_err(|e| CodecError::Encode(e.to_string()))
    }

    fn decode(&self, bytes: &[u8]) -> Result<T, CodecError> {
        rmp_serde::from_slice(bytes).map_err(|e| CodecError::Decode(e.to_string()))
    }
}
```

**Features:**
- Binary format, ~60% smaller than the legacy text encoding
- Uses `rmp_serde` library
- `to_vec_named()` for field names in output
- Faster than text codecs, not human-readable

### Bincode Functions (`basic/bincode.rs`)

**Important:** Bincode does NOT implement `Codec<T>` trait. It provides standalone functions.

```rust
pub fn to_bytes<T>(value: &T) -> Result<Bytes, CodecError>
where
    T: serde::Serialize,
{
    bincode::serialize(value).map(Bytes::from)
        .map_err(|e| CodecError::Serialize(e.to_string()))
}

pub fn from_bytes<T>(bytes: &[u8]) -> Result<T, CodecError>
where
    T: serde::de::DeserializeOwned,
{
    bincode::deserialize(bytes).map_err(|e| CodecError::Deserialize(e.to_string()))
}
```

**Features:**
- **Fastest option** (~2x faster than MessagePack)
- **Most compact** (~50% of MessagePack size)
- Uses `bincode` library
- Returns `Bytes` (zero-copy wrapper)
- Has its own `CodecError` enum (not shared with trait)

## Interned Codecs

### InternedCodec Implementations (`interned/codec.rs`)

**LegacyTextInternedCodec:**
```rust
impl InternedCodec for LegacyTextInternedCodec {
    fn decode_with_interner(&self, bytes: &[u8], interner: &Interner)
        -> Result<InnerValue, CodecError> {
        crate::codecs::interned::legacy_text::text_to_inner(interner, bytes)
    }

    fn encode_with_interner(&self, value: &InnerValue, interner: &Interner)
        -> Result<Vec<u8>, CodecError> {
        crate::codecs::interned::legacy_text::inner_to_text(interner, value)
    }

    fn format_name(&self) -> &'static str {
        "LegacyText"
    }
}
```

**MsgPackInternedCodec:**
```rust
impl InternedCodec for MsgPackInternedCodec {
    fn decode_with_interner(&self, bytes: &[u8], interner: &Interner)
        -> Result<InnerValue, CodecError> {
        crate::codecs::interned::messagepack::msgpack_to_inner(interner, bytes)
    }

    fn encode_with_interner(&self, value: &InnerValue, interner: &Interner)
        -> Result<Vec<u8>, CodecError> {
        crate::codecs::interned::messagepack::inner_to_msgpack(interner, value)
    }

    fn format_name(&self) -> &'static str {
        "MessagePack"
    }
}
```

### Legacy Text Interning Functions (`interned/legacy_text.rs`)

**Important:** These functions work directly with InnerValue. UserValue is deprecated and only for tests.

```rust
pub fn text_to_inner(interner: &Interner, bytes: &[u8]) -> Result<InnerValue, CodecError>
pub fn inner_to_text(interner: &Interner, value: &InnerValue) -> Result<Vec<u8>, CodecError>
```

**Type Handling:**

| Rust Type | Legacy Text Handling |
|-----------|----------------------|
| Null | `null` |
| Bool | `true`/`false` |
| Int(i64) | number |
| UInt(u64) | number (if > i64::MAX, store as string) |
| Float(f64) | number (if not finite, store as string) |
| Str | string |
| Bin | array of numbers `[1, 2, 3]` |
| List | array |
| Set | array (no Set type in the legacy text format) |
| Map | object (keys interned) |

**Special Cases:**
- **Large u64**: If `u <= i64::MAX`, store as Int. Otherwise as string.
- **Non-finite Float**: `Infinity`, `NaN` stored as string.
- **Binary**: Stored as array of numbers for simplicity (not base64).
- **Sets**: No native Set type in the legacy text format, stored as arrays.

### MessagePack Interning Functions (`interned/messagepack.rs`)

```rust
pub fn msgpack_to_inner(interner: &Interner, bytes: &[u8]) -> Result<InnerValue, CodecError>
pub fn inner_to_msgpack(interner: &Interner, value: &InnerValue) -> Result<Vec<u8>, CodecError>
```

**Type Handling:**

| Rust Type | MessagePack Handling |
|-----------|---------------------|
| Nil | Nil |
| Bool | Boolean |
| Int(i64) | Integer |
| F64 | F64 / F32 |
| Str | String |
| Bin | Binary |
| List | Array |
| Set | Array (same as legacy text format) |
| Map | Map (keys must be strings, then interned) |

**Note:** Uses `rmpv` crate for value representation, then converts to InnerValue.

**Special Cases:**
- **Large u64**: Same as legacy text format - store as Int if fits, else as string.
- **Very large integers**: Stored as strings.
- **Extension types**: Stored as `Bin` for now.
- **Map keys**: Must be strings in MessagePack, error otherwise.

### Common Interning Functions (`interned/common.rs`)

```rust
pub fn intern_string_key(interner: &Interner, key_str: &str) -> Result<InternerKey, CodecError>
pub fn deintern_key(interner: &Interner, interned_key: &InternerKey) -> String
```

**intern_string_key:**
- Calls `interner.touch_ind(key_str)`
- Returns `InternerKey`
- Used during decode (external format → InnerValue)

**deintern_key:**
- Calls `interner.get_str(interned_key)`
- Returns `String`
- Panics if key not found (data corruption)
- Used during encode (InnerValue → external format)

## Legacy Module (`legacy/tools.rs`)

**Status:** ⚠️ Deprecated since 0.1.0

```rust
#[deprecated(since = "0.1.0", note = "Use newer codec-based approach instead")]
pub struct TransformResult {
    pub inner_value: InnerValue,
    pub new_keys: Option<Vec<(InternerKey, UserKey)>>,
}

#[deprecated(since = "0.1.0", note = "Use newer codec-based approach instead")]
pub fn user_to_inner(value: &UserValue, interner: &Interner) -> TransformResult

#[deprecated(since = "0.1.0", note = "Use newer codec-based approach instead")]
pub fn inner_to_user(value: &InnerValue, interner: &Interner) -> UserValue
```

**Legacy Features:**
- Works with `UserValue` (deprecated type)
- Returns `TransformResult` with track of newly interned keys
- Includes `Dec` and `Big` types
- Manual transformation, not codec-based

**Why Deprecated:**
- Newer `InternedCodec` approach is cleaner
- Direct InnerValue conversion (no UserValue)
- Better error handling
- More efficient (no tracking overhead)

## Type Mapping Summary

| Value Variant | Legacy Text | MessagePack |
|--------------|-------------|-------------|
| Nil | `null` | nil |
| Bool | `true`/`false` | bool |
| Int(i64) | number | int64 |
| F64 | number (or string if infinite) | float64 |
| Str | string | string |
| Bin | array `[1,2,3]` | binary |
| List | array `[...]` | array `[...]` |
| Set | array `[...]` | array `[...]` |
| Map | object `{...}` | map `{...}` |

## Performance Comparison

| Format | Size | Encode Speed | Decode Speed | Human Readable | Notes |
|---------|-------|--------------|----------------|----------------|-------|
| LegacyText | 100% | 1x | 1x | ✅ Yes | orphaned/inactive |
| MessagePack | ~60% | 1.2x | 1.3x | ❌ No | rmp_serde |
| Bincode | ~50% | 2x | 2x | ❌ No | Functions only |

## Usage Examples

### Basic Codec (MessagePack)

```rust
use shamir_types::codecs::Codec;
use shamir_types::codecs::basic::MessagePackCodec;
use shamir_types::types::value::UserValue;

let codec = MessagePackCodec;
let value = UserValue::Str("Hello".to_string());

// Serialize
let bytes = codec.encode(&value)?;

// Deserialize
let decoded: UserValue = codec.decode(&bytes)?;
```

### Interned Codec

```rust
use shamir_types::codecs::interned::InternedCodec;
use shamir_types::codecs::interned::MsgPackInternedCodec;
use shamir_types::core::interner::Interner;
use shamir_types::types::value::InnerValue;

let interner = Interner::new();
let codec: Box<dyn InternedCodec> = MsgPackInternedCodec.into();

// MessagePack → InnerValue (keys automatically interned)
let msgpack_bytes = rmp_serde::to_vec_named(&QueryValue::Map(/* {"name":"Alice"} */)).unwrap();
let inner_value = codec.decode_with_interner(&msgpack_bytes, &interner)?;

// InnerValue → MessagePack (keys automatically de-interned)
let output = codec.encode_with_interner(&inner_value, &interner)?;
```

### Bincode Functions

```rust
use shamir_types::codecs::basic::{to_bytes, from_bytes};

let value = 42i32;

// Serialize
let bytes = to_bytes(&value)?;

// Deserialize
let result: i32 = from_bytes(&bytes)?;
```

## Best Practices

### When to Use MessagePack (preferred)
- Network transmission (compact binary)
- Large datasets (size matters)
- Performance-critical paths
- Internal database format
- When InternedCodec is needed

### When to Use Bincode Functions
- Maximum speed needed (Rust-to-Rust)
- Internal storage with fixed schemas
- IPC (Inter-Process Communication)
- Not recommended for external APIs (not portable)

### When NOT to Use Legacy Module
- ⚠️ Deprecated - use `InternedCodec` instead
- Legacy only for backward compatibility
- Will be removed in future version

## Test Coverage

### Basic Codec Tests
- ✅ LegacyText roundtrip for all types (inactive — file orphaned)
- ✅ MessagePack roundtrip for all types
- ✅ Bincode roundtrip for all types

### Interned Codec Tests
- ✅ LegacyText ↔ InnerValue conversion with interning
- ✅ MessagePack ↔ InnerValue conversion with interning
- ✅ Key interning on decode
- ✅ Key de-interning on encode
- ✅ Nested structures
- ✅ Large integers (beyond i64)

## Error Handling

### CodecError (trait-based)

```rust
pub enum CodecError {
    #[error("Failed to encode data: {0}")]
    Encode(String),
    #[error("Failed to decode data: {0}")]
    Decode(String),
}
```

**Usage:**
```rust
match codec.encode(&value) {
    Ok(bytes) => bytes,
    Err(CodecError::Encode(msg)) => eprintln!("Encoding failed: {}", msg),
    Err(CodecError::Decode(msg)) => eprintln!("Decoding failed: {}", msg),
}
```

### Bincode CodecError (separate enum)

```rust
pub enum CodecError {
    Serialize(String),
    Deserialize(String),
}
```

**Note:** Bincode has its own error type, separate from trait-based `CodecError`.

## Contributing

Adding new codecs:

1. **Basic Codec (implements Codec<T>):**
   - Create file in `src/codecs/basic/{name}.rs`
   - Implement `Codec<T>` trait
   - Add to `src/codecs/basic/mod.rs`
   - Add to `src/codecs/mod.rs` re-exports

2. **Interned Codec:**
   - Create file in `src/codecs/interned/{name}.rs`
   - Implement `InternedCodec` trait
   - Add to `src/codecs/interned/mod.rs`
   - Update `CodecFormat` enum if needed

3. Add tests in respective `tests/` directories
4. Update this README

## Design Decisions

### Why Separate Basic and Interned?
- **Basic:** Generic, works with any type, no dependencies
- **Interned:** Specialized for database storage, integrates with Interner

### Why Bincode Not in CodecFormat?
- Bincode does not implement `Codec<T>` trait
- Provides standalone functions instead
- Different error type (separate enum)
- Used differently in codebase

### Why UserValue Deprecated?
- Newer approach uses direct InnerValue conversion
- More efficient (no extra transformation step)
- Cleaner API (InternedCodec directly handles interning)

### Why Different Error Types?
- Trait-based `CodecError`: Used by codecs implementing `Codec<T>`
- Bincode-specific: Separate enum for standalone functions
- Clear separation of concerns

## Import Recommendations

```rust
// For basic codecs
use shamir_db::codecs::{Codec, CodecError, MessagePackCodec};

// For interned codecs
use shamir_db::codecs::interned::{InternedCodec, CodecFormat, MsgPackInternedCodec};
use shamir_db::codecs::interned::{msgpack_to_inner, inner_to_msgpack};
use shamir_db::codecs::interned::{intern_string_key, deintern_key};

// For bincode
use shamir_types::codecs::basic::{to_bytes, from_bytes};

// For legacy (DO NOT USE - deprecated)
use shamir_db::codecs::transform; // Will emit deprecation warning
```
