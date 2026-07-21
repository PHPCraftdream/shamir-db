# Numeric Wire Semantics

This document specifies how integer values are represented across the
SHAMIR DB wire boundary (msgpack), focusing on the `u64` promotion contract.

## The `u64 > i64::MAX` contract (FG-1)

A msgpack `uint64` value (`0xcf` marker) received by the server is decoded
according to its magnitude:

| Magnitude             | Decoded as                          | Notes                           |
|-----------------------|-------------------------------------|---------------------------------|
| `u64 <= i64::MAX`     | `Value::Int(i64)` / `QueryValue::Int` | Unchanged — plain integer.    |
| `u64 > i64::MAX`      | `Value::Big(BigInt)` / `QueryValue::Big` | Lossless promotion to arbitrary-precision. |

Before FG-1, the decoder silently wrapped via `value as i64` (sign-flipping
`u64::MAX` to `-1`) or clamped to `i64::MAX`. Both were silent data
corruption; both are now replaced by lossless promotion to the existing
`Big` variant.

## Wire representation

- **`Value::Big(b)`** serialises on the wire as a plain decimal **string**
  (`serializer.serialize_str(&b.to_string())`). This is pre-existing,
  established behavior — `Big` has always been a string on the wire.

- **`QueryValue::Big(b)`** follows the same rule.

## Filter literals (`lit_u64` / `litU64`)

Because `FilterValue` has no `Big` wire variant (by design — adding one
would require a new serde tag, comparison-resolution wiring, and a matching
TS type), values above `i64::MAX` are represented as their exact decimal
**string** at the filter-literal boundary:

| Builder              | Input          | Output                           |
|----------------------|----------------|----------------------------------|
| Rust `lit_u64(v)`    | `v <= i64::MAX` | `FilterValue::Int(v as i64)`    |
| Rust `lit_u64(v)`    | `v > i64::MAX`  | `FilterValue::String(v.to_string())` |
| TS `litU64(v)`       | `v <= 9223372036854775807n` | `number`             |
| TS `litU64(v)`       | `v > 9223372036854775807n`  | `string` (exact decimal) |

This matches how `Value::Big` itself serialises on the wire. The engine's
cross-type comparison layer bridges `Big`↔`Str` for equality in the
dedup/group-by path (`canonical_eq`). Values stored via the normal write
path (as decimal strings) round-trip and match filters correctly.

## Known limitation: raw `uint64` storage + `Eq` filters

When raw `uint64` bytes (`0xcf` marker) are stored directly (e.g. by an
external non-Rust/non-TS encoder), the read-path `scalar_at` extraction
returns `None` for the field:

- **Lens path** (`RecordView`): the decimal text is `Cow::Owned` (not
  borrowed from the buffer), and `ScalarRef::Str(&'a str)` cannot borrow
  an owned `String`. `scalar_at` returns `None`.
- **Tree path** (`InnerValue::Big`): `ScalarRef` has no `Big` variant by
  design. `scalar_at` returns `None`.

Consequently, an `Eq` filter via `lit_u64` does not currently match a field
stored as raw `uint64` bytes. This is a pre-existing structural limitation
of the `scalar_at` extraction layer, not a regression — before FG-1 the
value was silently corrupted (wrapped/clamped), so a filter "matched"
against the corrupted value. This gap is tracked as a follow-up.

Values stored via the normal write path (as `msgpack str`, which is how
`Value::Big` serialises) are unaffected — they round-trip as borrowed
strings and match correctly.
