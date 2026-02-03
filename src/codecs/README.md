# Codecs

Serialization/deserialization support for various data formats.

## Available Codecs

- **JSON** (`json.rs`) - Human-readable, widely supported
- **MessagePack** (`message_pack.rs`) - Compact binary format

## Status

🚧 **Under Development** - Codecs are implemented but not yet fully integrated with the table engine.

## Usage

### JSON Codec

```rust
use shamir_db::codecs::json;
use shamir_db::types::value::UserValue;

// Serialize
let value: UserValue = Value::Object(map![
    ("name".into(), Value::Str("Alice".into()))
]);
let json_string = json::to_string(&value)?;

// Deserialize
let restored: UserValue = json::from_str(&json_string)?;
```

### MessagePack Codec

```rust
use shamir_db::codecs::message_pack;
use shamir_db::types::value::UserValue;

// Serialize
let value: UserValue = Value::Int(42);
let bytes = message_pack::to_bytes(&value)?;

// Deserialize
let restored: UserValue = message_pack::from_bytes(&bytes)?;
```

## Implementation Details

### JSON
- **Format:** Text-based, UTF-8 encoded
- **Library:** `serde_json`
- **Pros:** Human-readable, widely supported, debuggable
- **Cons:** Larger size, slower than binary

**Example:**
```json
{
  "name": "Alice",
  "age": 30,
  "tags": ["rust", "database"]
}
```

### MessagePack
- **Format:** Binary, efficient
- **Library:** `rmp_serde`
- **Pros:** Compact, fast, schema-less
- **Cons:** Not human-readable

**Example:**
```
Binary data (typically 40-60% smaller than JSON)
```

## Type Mapping

| Rust Type | JSON | MessagePack |
|-----------|------|-------------|
| Null | `null` | nil |
| Bool | `true`/`false` | bool |
| Int(i64) | number | int64 |
| UInt(u64) | number | uint64 |
| Float(f64) | number | float64 |
| String | string | str |
| Binary | base64 string | bin |
| Array | array | array |
| Object | object | map |
| Decimal | string (decimal) | str (decimal) |
| BigInt | string (bigint) | str (bigint) |

## Future Plans

### Integration with Table Engine

```rust
// Planned API
let table = repo.table_get("users")?;

// Insert with format auto-detection
table.insert_with_format(value, Format::Json).await?;

// Export to specific format
let json_data = table.export_format(Format::Json).await?;
let msgpack_data = table.export_format(Format::MessagePack).await?;

// Import from format
table.import_format(json_data, Format::Json).await?;
```

### Additional Formats (Planned)

- [ ] **CBOR** - Concise Binary Object Representation
- [ ] **BSON** - Binary JSON (MongoDB format)
- [ ] **Protobuf** - Protocol Buffers (schema-based)
- [ ] **Smile** - Binary JSON with compression
- [ ] **UBJSON** - Universal Binary JSON

### Compression

Planned integration with compression for large datasets:
- **Snappy** - Fast compression/decompression
- **LZ4** - High-speed compression
- **Zstd** - Best compression ratio

## Performance Comparison

| Format | Size | Encode | Decode |
|--------|------|-------|--------|
| JSON | 100% | 1x | 1x |
| MessagePack | ~60% | 1.2x | 1.3x |
| CBOR (planned) | ~55% | TBD | TBD |

## Best Practices

### When to Use JSON
- Human-readable logs
- Debugging
- API responses (REST)
- Configuration files

### When to Use MessagePack
- Network transmission
- Large datasets
- Performance-critical paths
- Storage optimization

## Error Handling

```rust
use shamir_db::db::error::DbError;

match json::from_str::<UserValue>(input) {
    Ok(value) => value,
    Err(e) => Err(DbError::Codec(format!("JSON parse error: {}", e)))
}
```

## Testing

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use shamir_db::types::value::Value;

    #[test]
    fn test_json_roundtrip() {
        let original = Value::Object(map![
            ("name".into(), Value::Str("Alice".into()))
        ]);

        let json = json::to_string(&original).unwrap();
        let restored = json::from_str::<Value<String>>(&json).unwrap();

        assert_eq!(original, restored);
    }
}
```

## Implementation Notes

### Codec Trait (Planned)

Future unified codec interface:

```rust
pub trait Codec: Send + Sync {
    fn name(&self) -> &str;
    fn to_bytes<T: Serialize>(&self, value: &T) -> DbResult<Vec<u8>>;
    fn from_bytes<'de, T: Deserialize<'de>>(&self, bytes: &[u8]) -> DbResult<T>;
}
```

### Format Detection

Auto-detection based on content:
```rust
pub enum Format {
    Json,
    MessagePack,
    Auto, // Detect from content
    CBOR,  // Future
}
```

## Contributing

Adding new codecs:
1. Create file in `src/codecs/{name}.rs`
2. Implement `Codec<T>` trait
3. Add to `mod.rs`
4. Add tests
5. Update this README

---

## Implementation Details (Deep Dive)

### Codec Trait (`mod.rs`)

```rust
pub trait Codec<T: Serialize + DeserializeOwned> {
    fn encode(&self, value: &T) -> Result<Vec<u8>, CodecError>;
    fn decode(&self, bytes: &[u8]) -> Result<T, CodecError>;
}
```

**Key Design:**
- **Generic over T** - works with any serializable type
- **DeserializeOwned** - type owns deserialized data (no borrowing)
- **Zero-copy** possible for some formats

### JSON Codec Implementation

**File:** `json.rs`

```rust
pub struct JsonCodec;

impl<T: Serialize + DeserializeOwned> Codec<T> for JsonCodec {
    fn encode(&self, value: &T) -> Result<Vec<u8>, CodecError> {
        serde_json::to_vec(value)
            .map_err(|e| CodecError::Encode(e.to_string()))
    }

    fn decode(&self, bytes: &[u8]) -> Result<T, CodecError> {
        serde_json::from_slice(bytes)
            .map_err(|e| CodecError::Decode(e.to_string()))
    }
}
```

**Special Features:**

1. **Type Hints System** - prefixed keys for disambiguation:
```rust
"i:version"   → Int
"u:user_id"   → UInt
"float:pi"     → Float
"dec:price"    → Decimal
"big:balance"  → BigInt
"arr:items"    → Array
"set:tags"     → Set
```

2. **Custom Deserializer** for UserValue:
   - Detects type hints from key prefix
   - Converts to appropriate UserValue variant
   - Supports `i:`, `u:`, `float:`, `dec:`, `big:`, `arr:`, `set:` prefixes

3. **BigInt/Decimal Handling:**
   - Both serialized as **strings** in JSON
   - Preserves full precision
   - Example: `Big(12345678901234567890...)` → `"12345678901234567890..."`

### MessagePack Codec Implementation

**File:** `message_pack.rs`

```rust
pub struct MessagePackCodec;

impl<T: Serialize + DeserializeOwned> Codec<T> for MessagePackCodec {
    fn encode(&self, value: &T) -> Result<Vec<u8>, CodecError> {
        rmp_serde::to_vec_named(value)
            .map_err(|e| CodecError::Encode(e.to_string()))
    }

    fn decode(&self, bytes: &[u8]) -> Result<T, CodecError> {
        rmp_serde::from_slice(bytes)
            .map_err(|e| CodecError::Decode(e.to_string()))
    }
}
```

**Special Features:**

1. **BigInt/Decimal as Strings:**
   ```rust
   // Both encode to str in MessagePack!
   Big(12345) → "12345" (MessagePack str)
   Dec("19.99") → "19.99" (MessagePack str)
   ```

2. **Binary Support:**
   - `UserValue::Bin(vec![1,2,3])` → MessagePack `bin` format
   - Preserves raw bytes efficiently

3. **Set Handling:**
   - Serialized as Array
   - Deserialized to Set on decode (via custom logic)

### Test Coverage

**JSON Tests** (`json.rs`):
- ✅ `test_generic_json_codec` - basic roundtrip
- ✅ `test_json_roundtrip` - all UserValue types
- ✅ `test_decode_from_raw_json_string` - nested structures
- ✅ `test_decode_with_all_type_prefixes` - type hints
- ✅ `test_decode_with_truly_large_bigint` - beyond u64
- ✅ `test_serialization_to_string_for_big_types` - Decimal/BigInt
- ✅ `test_fail_on_unknown_prefix` - error handling
- ✅ `test_decode_bigint_from_number` - number → BigInt

**MessagePack Tests** (`message_pack.rs`):
- ✅ `test_generic_msgpack_codec` - basic roundtrip
- ✅ `test_messagepack_roundtrip` - all UserValue types
- ✅ `test_json_to_msgpack_conversion_with_all_hints` - format conversion
- ✅ `test_serialization_to_string_for_big_types_msgpack` - Decimal/BigInt as strings

## Key Differences Summary

| Feature | JSON | MessagePack |
|---------|------|-------------|
| **Type hints** | ✅ Yes (prefixes) | ❌ No |
| **BigInt/Decimal** | String (custom deser) | String (serde default) |
| **Binary data** | base64 string | bin format |
| **Human-readable** | ✅ Yes | ❌ No |
| **Size** | 100% (baseline) | ~60% |
| **Speed** | 1x (baseline) | 1.2-1.3x faster |