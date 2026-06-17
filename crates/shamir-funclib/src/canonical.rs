//! `/crypto` canonical record hashing — deterministic, order-independent
//! content hash of a [`Value`], used by the sequenced-write / optimistic-CAS
//! protocol ("blockchain ordering of writes", #160).
//!
//! # Why a bespoke serializer
//!
//! The existing MessagePack codec preserves *insertion order* of map keys
//! ([`TMap`](shamir_types::types::common::TMap) is an `IndexMap`), so two maps
//! that differ only in key order serialise to different bytes. That is exactly
//! what a canonical hash must NOT do. This module instead walks the value tree
//! and emits an explicit, type-tagged byte stream in which **map entries are
//! sorted by their serialised key bytes** at every level. Arrays/sets keep
//! their element order; scalars are emitted verbatim. The result is fed to
//! BLAKE3 and returned as a lowercase hex string.
//!
//! # Determinism contract
//!
//! - Two values differing ONLY in map-key order produce the SAME hash.
//! - Any change to data (a value, a key name, an element) changes the hash.
//! - The reserved top-level key [`PREV_HASH_FIELD`] (`_prev_hash`) is
//!   EXCLUDED from the hash. This breaks the self-reference of the CAS
//!   protocol: a record carries the expected hash of the *previous* version
//!   in `_prev_hash`, and that field must not feed back into the record's own
//!   hash (which would otherwise be a fixed-point recursion).
//!
//! # Key ordering and portability
//!
//! Sorting is by the **serialised key bytes**. For a string-keyed
//! [`QueryValue`](shamir_types::types::value::QueryValue) those bytes are the
//! UTF-8 key name, so the ordering — and therefore the hash — is portable and
//! depends only on the logical content. For an interned
//! [`InnerValue`](shamir_types::types::value::InnerValue) the key bytes are the
//! interner id, so the hash is deterministic only *within a single interner
//! instance*. The CAS protocol and the `canonical_hash(record)` SELECT-field
//! both run inside one engine/interner, so this is sufficient there; for a
//! portable, content-only hash callers should canonicalise a string-keyed
//! `QueryValue` (this is what the CAS validator does).

use crate::registry::{FnEntry, ScalarError, ScalarRegistry};
use serde::Serialize;
use shamir_types::types::value::{QueryValue, Value};
use std::hash::Hash;

/// Reserved top-level field carrying the expected hash of the previous version
/// of a record in the sequenced-write / optimistic-CAS protocol. It is
/// EXCLUDED from [`canonical_hash`] / [`canonical_bytes`] at the top level so
/// the record's hash does not depend on the prev-hash it asserts.
pub const PREV_HASH_FIELD: &str = "_prev_hash";

// Type tags. Distinct leading bytes keep two values of different types from
// ever colliding on their payloads (e.g. the empty string vs. an empty list).
//
// NOTE: 0x04 (formerly T_DEC) and 0x05 (formerly T_BIG) are intentionally
// absent. Dec and Big are both wire-serialised as plain strings (Serialize
// emits `d.to_string()` / `b.to_string()`), so they are hashed with T_STR.
// This preserves round-trip invariance: after a msgpack round-trip,
// Value::Dec(d) becomes Value::Str(d.to_string()), and the hash is unchanged.
// The 0x04/0x05 values are reserved and must NOT be reused.
const T_NULL: u8 = 0x00;
const T_BOOL: u8 = 0x01;
const T_INT: u8 = 0x02;
const T_F64: u8 = 0x03;
const T_STR: u8 = 0x06;
const T_BIN: u8 = 0x07;
const T_LIST: u8 = 0x08;
const T_SET: u8 = 0x09;
const T_MAP: u8 = 0x0a;

/// Emit the canonical byte encoding of `value` into `out`.
///
/// `top_level` selects whether the [`PREV_HASH_FIELD`] key is stripped from a
/// map (only at the top level). Lengths are written as fixed 8-byte
/// big-endian so a value's framing cannot be confused with its payload.
fn encode<K>(value: &Value<K>, out: &mut Vec<u8>, top_level: bool)
where
    K: Eq + Hash + Ord + Clone + Serialize + std::fmt::Debug,
{
    match value {
        Value::Null => out.push(T_NULL),
        Value::Bool(b) => {
            out.push(T_BOOL);
            out.push(*b as u8);
        }
        Value::Int(i) => {
            out.push(T_INT);
            out.extend_from_slice(&i.to_be_bytes());
        }
        Value::F64(f) => {
            out.push(T_F64);
            // Canonicalise the bit pattern: collapse all NaNs to one
            // payload and normalise -0.0 to +0.0 so logically-equal floats
            // hash identically.
            let bits = if f.is_nan() {
                f64::NAN.to_bits()
            } else if *f == 0.0 {
                0.0_f64.to_bits()
            } else {
                f.to_bits()
            };
            out.extend_from_slice(&bits.to_be_bytes());
        }
        Value::Dec(d) => {
            // Round-trip invariant: Dec is serialised to the wire as a plain
            // string (Value::Serialize uses `d.to_string()`), so on read-back
            // the value is Value::Str(d.to_string()).  We must hash Dec
            // identically to that Str — same tag (T_STR) and same bytes —
            // otherwise canonical_hash(in-memory Dec) != canonical_hash(same
            // record after a storage/wire round-trip), breaking CAS.
            out.push(T_STR);
            write_bytes(out, d.to_string().as_bytes());
        }
        Value::Big(b) => {
            // Same invariant as Dec: BigInt survives the wire as Value::Str.
            out.push(T_STR);
            write_bytes(out, b.to_string().as_bytes());
        }
        Value::Str(s) => {
            out.push(T_STR);
            write_bytes(out, s.as_bytes());
        }
        Value::Bin(b) => {
            out.push(T_BIN);
            write_bytes(out, b);
        }
        Value::List(l) => {
            out.push(T_LIST);
            out.extend_from_slice(&(l.len() as u64).to_be_bytes());
            for el in l {
                encode(el, out, false);
            }
        }
        Value::Set(s) => {
            // A set is unordered; sort the elements' canonical encodings so the
            // hash is independent of iteration order.
            out.push(T_SET);
            let mut elems: Vec<Vec<u8>> = Vec::with_capacity(s.len());
            for el in s {
                let mut buf = Vec::new();
                encode(el, &mut buf, false);
                elems.push(buf);
            }
            elems.sort_unstable();
            out.extend_from_slice(&(elems.len() as u64).to_be_bytes());
            for e in elems {
                out.extend_from_slice(&e);
            }
        }
        Value::Map(m) => {
            out.push(T_MAP);
            // Build (serialised-key-bytes, value-ref) pairs, dropping the
            // reserved prev-hash field at the top level, then sort by key
            // bytes so key order does not affect the hash.
            let mut entries: Vec<(Vec<u8>, &Value<K>)> = Vec::with_capacity(m.len());
            for (k, v) in m {
                let key_bytes = serialise_key(k);
                if top_level && key_is_prev_hash(&key_bytes) {
                    continue;
                }
                entries.push((key_bytes, v));
            }
            entries.sort_unstable_by(|a, b| a.0.cmp(&b.0));
            out.extend_from_slice(&(entries.len() as u64).to_be_bytes());
            for (key_bytes, v) in entries {
                write_bytes(out, &key_bytes);
                encode(v, out, false);
            }
        }
    }
}

/// Length-prefixed (8-byte BE) raw bytes.
fn write_bytes(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&(bytes.len() as u64).to_be_bytes());
    out.extend_from_slice(bytes);
}

/// Serialise a map key to its canonical bytes. A string key (`QueryValue`)
/// serialises to its UTF-8 name; any other key type falls back to its
/// MessagePack encoding (deterministic per interner instance).
fn serialise_key<K>(key: &K) -> Vec<u8>
where
    K: Serialize,
{
    // rmp_serde of a bare `String` is a length-tagged copy of its UTF-8 bytes,
    // so string keys order exactly as their names do; non-string keys (interned
    // ids) order by their encoded form.
    rmp_serde::to_vec(key).unwrap_or_default()
}

/// Whether `key_bytes` is the MessagePack encoding of the reserved
/// [`PREV_HASH_FIELD`] string (so the check works on string-keyed values).
fn key_is_prev_hash(key_bytes: &[u8]) -> bool {
    rmp_serde::to_vec(PREV_HASH_FIELD)
        .map(|b| b == key_bytes)
        .unwrap_or(false)
}

/// Canonical byte encoding of `value` (top level: `_prev_hash` excluded).
pub fn canonical_bytes<K>(value: &Value<K>) -> Vec<u8>
where
    K: Eq + Hash + Ord + Clone + Serialize + std::fmt::Debug,
{
    let mut out = Vec::new();
    encode(value, &mut out, true);
    out
}

/// Deterministic, order-independent BLAKE3 content hash of `value`, as a
/// lowercase hex string. The top-level [`PREV_HASH_FIELD`] key is excluded.
///
/// See the module docs for the determinism contract and the string-key vs.
/// interned-key portability note.
pub fn canonical_hash<K>(value: &Value<K>) -> String
where
    K: Eq + Hash + Ord + Clone + Serialize + std::fmt::Debug,
{
    let bytes = canonical_bytes(value);
    blake3::hash(&bytes).to_hex().to_string()
}

/// `canonical_hash(value) -> Str` scalar.
///
/// Hashes its single argument with [`canonical_hash`]. Registered under
/// `crypto/canonical_hash`. Under the `QueryValue` ABI the argument is
/// string-keyed, so the hash is portable and content-only.
fn canonical_hash_fn(a: &[QueryValue]) -> Result<QueryValue, ScalarError> {
    let v = a.first().ok_or_else(|| ScalarError::new("missing_arg"))?;
    Ok(QueryValue::Str(canonical_hash(v)))
}

/// Register the canonical-hash scalar into the `/crypto` folder.
pub fn register(reg: &mut ScalarRegistry) {
    reg.register(
        "canonical_hash",
        FnEntry::pure(canonical_hash_fn, 1, Some(1)),
    );
}

#[cfg(test)]
mod tests;
