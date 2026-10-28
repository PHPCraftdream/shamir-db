# Codecs Module

The `shamir-types` crate exposes generic serde codecs through `Codec<T>` and `MessagePackCodec`. The `interned/` module provides conversion helpers between external values and interned storage values. Bincode is available through standalone helper functions.

## Module layout

```text
crates/shamir-types/src/codecs/
├── mod.rs
├── codec.rs
├── error.rs
├── basic/
│   ├── mod.rs
│   ├── messagepack.rs
│   └── bincode.rs
├── interned/
│   ├── mod.rs
│   ├── codec.rs
│   ├── messagepack.rs
│   ├── projection.rs
│   ├── validate_keys.rs
│   └── common.rs
└── tests/
```

`basic/` provides `MessagePackCodec` (which implements `Codec<T>`) and bincode's `to_bytes`/`from_bytes` helpers (which do not implement `Codec<T>`). `interned/` provides value conversion/projection helpers. Key helpers are exported from `shamir_types::codecs::interned::common`, not directly from `interned`.

## MessagePack

`MessagePackCodec` serializes named serde fields using MessagePack and maps failures to shared `CodecError::Encode` or `CodecError::Decode` variants. Its public paths are `shamir_types::codecs::Codec` and `shamir_types::codecs::basic::MessagePackCodec`; `MessagePackCodec` is also re-exported from `shamir_types::codecs`.

A value conversion example using the actual exported paths:

```rust,ignore
use shamir_types::codecs::interned::messagepack::{inner_to_msgpack, msgpack_to_inner};
use shamir_types::core::interner::Interner;

fn convert(bytes: &[u8]) -> Result<Vec<u8>, shamir_types::codecs::CodecError> {
    let interner = Interner::new();
    let value = msgpack_to_inner(&interner, bytes)?;
    inner_to_msgpack(&interner, &value)
}
```

Key interning helpers are accessed through `shamir_types::codecs::interned::common::{intern_string_key, deintern_key}`.

## Bincode helpers

The helpers are exported as `shamir_types::codecs::basic::{to_bytes, from_bytes}`. They use the shared `shamir_types::codecs::CodecError`: encoding failures are `Encode`, and decoding failures are `Decode`.

```rust,ignore
fn main() -> Result<(), shamir_types::codecs::CodecError> {
    use shamir_types::codecs::basic::{from_bytes, to_bytes};

    let value = 42i32;
    let bytes = to_bytes(&value)?;
    let decoded: i32 = from_bytes(&bytes)?;
    assert_eq!(decoded, value);
    Ok(())
}
```

## Interned conversions and wire projection

`shamir_types::codecs::interned::messagepack` exports MessagePack conversion helpers (`msgpack_to_inner`, `inner_to_msgpack`). `shamir_types::codecs::interned::common` provides `intern_string_key` and `deintern_key`. `deintern_key` returns an error when no string is registered for a key; it does not panic.

MessagePack map keys must be strings and are interned on decode. The v1 projection is intentionally lossy: `Dec` and `Big` encode as decimal strings and decode as `Str`; `Set` encodes as an array in iteration order and decodes as `List`. There is no type tag in the current format. A typed round trip would require a separately versioned wire format.

## Errors

All codec operations use the shared `thiserror` error type. Its variants are `Encode(String)`, `Decode(String)`, and `Interner(InternerError)`.

```rust,ignore
fn report(result: Result<(), shamir_types::codecs::CodecError>) {
    use shamir_types::codecs::CodecError;

    match result {
        Ok(()) => println!("Success"),
        Err(CodecError::Encode(message)) => eprintln!("Encoding failed: {message}"),
        Err(CodecError::Decode(message)) => eprintln!("Decoding failed: {message}"),
        Err(CodecError::Interner(error)) => eprintln!("Interner failed: {error}"),
    }
}
```

## Adding codecs

For a basic codec, add its implementation under `crates/shamir-types/src/codecs/basic/`, register it in that directory's `mod.rs`, and update the crate's codec exports as needed. For interned conversion helpers, add and export them from `crates/shamir-types/src/codecs/interned/mod.rs`. Add coverage in the corresponding test modules and update this document.
