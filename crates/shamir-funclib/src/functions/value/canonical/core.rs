//! `/crypto` canonical record hashing — deterministic, order-independent
//! content hash of a [`Value`], used by the sequenced-write / optimistic-CAS
//! protocol ("blockchain ordering of writes", #160).
//!
//! # Frozen v1 format
//!
//! The canonical stream is format v1 and has no version prefix. Its bytes are
//! frozen because clients and stored `_prev_hash` values depend on them. A v1
//! stream starts with one of the type tags `0x00`, `0x01`, `0x02`, `0x03`,
//! `0x06`, `0x07`, `0x08`, `0x09`, or `0x0a`; leading bytes `0x0b..=0xff` are
//! reserved for future version markers. Any future byte change must introduce
//! a marker and retain a v1 verifier.
//!
//! Values use explicit type tags; lengths are fixed 8-byte big-endian. Maps
//! sort entries by the serialized key bytes at every level. The key bytes in
//! the stream include their MessagePack encoding header. For String keys this
//! means UTF-8 byte length first, then UTF-8 bytes (for example, `"b"` sorts
//! before `"aa"`). Interned numeric keys sort by MessagePack uint encoding, and
//! the hash is stable only within one interner instance. `_prev_hash` is
//! excluded only at the top level and only if the key serializes as the
//! MessagePack string `"_prev_hash"`; nested instances are ordinary data.
//!
//! # Determinism contract
//!
//! - Two values differing ONLY in map-key order produce the SAME hash.
//! - Any change to data (a value, a key name, an element) changes the hash.
//! - The reserved top-level key [`PREV_HASH_FIELD`] is EXCLUDED, which breaks
//!   the CAS self-reference: a record's hash must not depend on the prev-hash
//!   it asserts.

use crate::registry::{FnEntry, ScalarError, ScalarRegistry};
use serde::Serialize;
use shamir_types::types::value::{QueryValue, Value};
use std::hash::Hash;

/// Reserved top-level field carrying the expected hash of the previous version
/// of a record in the sequenced-write / optimistic-CAS protocol. It is
/// EXCLUDED from [`canonical_hash`] / [`canonical_bytes`] at the top level so
/// the record's hash does not depend on the prev-hash it asserts.
pub const PREV_HASH_FIELD: &str = "_prev_hash";
/// Leading bytes available to future canonical format markers; v1 has no prefix.
pub const CANONICAL_V1_RESERVED_MARKERS: std::ops::RangeInclusive<u8> = 0x0b..=0xff;
/// Precomputed MessagePack string encoding of `PREV_HASH_FIELD` (`0xaa` + UTF-8).
pub const PREV_HASH_KEY_BYTES: &[u8] = &[
    0xaa, b'_', b'p', b'r', b'e', b'v', b'_', b'h', b'a', b's', b'h',
];

/// Key serialization failure while building the canonical stream.
#[derive(Debug)]
pub struct CanonicalError(rmp_serde::encode::Error);

impl From<rmp_serde::encode::Error> for CanonicalError {
    fn from(error: rmp_serde::encode::Error) -> Self {
        Self(error)
    }
}

impl std::fmt::Display for CanonicalError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "canonical serialization failed: {}", self.0)
    }
}

impl std::error::Error for CanonicalError {}

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
/// map (only at the top level).
fn encode<K>(value: &Value<K>, out: &mut Vec<u8>, top_level: bool) -> Result<(), CanonicalError>
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
            // Round-trip invariant: Dec reads back as Value::Str(d.to_string()),
            // so it must hash identically to that Str or CAS breaks.
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
                encode(el, out, false)?;
            }
        }
        Value::Set(s) => {
            // A set is unordered; sort the elements' canonical encodings so the
            // hash is independent of iteration order.
            out.push(T_SET);
            let mut elems = Vec::with_capacity(s.len());
            for el in s {
                let mut buf = Vec::new();
                encode(el, &mut buf, false)?;
                elems.push(buf);
            }
            elems.sort_unstable();
            out.extend_from_slice(&(elems.len() as u64).to_be_bytes());
            for e in elems {
                out.extend_from_slice(&e);
            }
        }
        Value::Map(m) => {
            let mut entries: Vec<(Vec<u8>, &Value<K>)> = Vec::with_capacity(m.len());
            for (k, v) in m {
                let key_bytes = serialise_key(k)?;
                if top_level && key_is_prev_hash(&key_bytes) {
                    continue;
                }
                entries.push((key_bytes, v));
            }
            entries.sort_unstable_by(|a, b| a.0.cmp(&b.0));
            out.push(T_MAP);
            out.extend_from_slice(&(entries.len() as u64).to_be_bytes());
            for (key_bytes, v) in entries {
                write_bytes(out, &key_bytes);
                encode(v, out, false)?;
            }
        }
    }
    Ok(())
}

/// Length-prefixed (8-byte BE) raw bytes.
fn write_bytes(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&(bytes.len() as u64).to_be_bytes());
    out.extend_from_slice(bytes);
}

fn serialise_key<K: Serialize>(key: &K) -> Result<Vec<u8>, CanonicalError> {
    Ok(rmp_serde::to_vec(key)?)
}

fn key_is_prev_hash(key_bytes: &[u8]) -> bool {
    key_bytes == PREV_HASH_KEY_BYTES
}

/// Canonical v1 bytes (unprefixed); excludes only a top-level string key whose
/// serialization is exactly the MessagePack encoding of `_prev_hash`.
pub fn canonical_bytes<K>(value: &Value<K>) -> Result<Vec<u8>, CanonicalError>
where
    K: Eq + Hash + Ord + Clone + Serialize + std::fmt::Debug,
{
    let mut out = Vec::with_capacity(128);
    encode(value, &mut out, true)?;
    Ok(out)
}

/// BLAKE3 hash of canonical v1 bytes, as lowercase hex.
pub fn canonical_hash<K>(value: &Value<K>) -> Result<String, CanonicalError>
where
    K: Eq + Hash + Ord + Clone + Serialize + std::fmt::Debug,
{
    let bytes = canonical_bytes(value)?;
    Ok(blake3::hash(&bytes).to_hex().to_string())
}

/// `canonical_hash(value) -> Str` scalar, registered under
/// `crypto/canonical_hash`.
fn canonical_hash_fn(a: &[QueryValue]) -> Result<QueryValue, ScalarError> {
    let v = a
        .first()
        .ok_or_else(|| ScalarError::builtin("missing_arg"))?;
    canonical_hash(v)
        .map(QueryValue::Str)
        .map_err(|_| ScalarError::builtin("encode_failed"))
}

/// Register the canonical-hash scalar into the `/crypto` folder.
pub fn register(reg: &mut ScalarRegistry) {
    reg.register(
        "canonical_hash",
        FnEntry::pure(canonical_hash_fn, 1, Some(1)),
    );
}
