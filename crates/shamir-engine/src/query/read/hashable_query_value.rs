//! `HashableQueryValue` — `Hash + Eq` wrapper for `QueryValue`.
//!
//! Provides deduplication equivalence classes that are **identical** to those
//! produced by the old lossy coercion path:
//!
//! | Variant | Canonical form |
//! |---------|---------------|
//! | `Null`  | Null           |
//! | `Bool`  | Bool           |
//! | `Int`   | Number(i64)    |
//! | `F64(finite)` | Number(f64 bits) |
//! | `F64(non-finite)` | String(f.to_string()) |
//! | `Dec(d)` | String(d.to_string()) — **same class as `Str(d.to_string())`** |
//! | `Big(b)` | String(b.to_string()) — **same class as `Str(b.to_string())`** |
//! | `Str(s)` | String(s) |
//! | `Bin(b)` | Array([Number(byte as i64), ...]) |
//! | `List(l)` | Array([...]) recursively |
//! | `Set(s)` | Array([...]) in iteration order |
//! | `Map(m)` | Object({...}) in insertion order |
//!
//! Everything is a structural walk — no external allocations.

use shamir_types::types::value::QueryValue;

/// Wrapper that gives `QueryValue` a `Hash + Eq` implementation whose
/// equivalence classes exactly match those of the old coercion-based
/// canonical form.
pub(super) struct HashableQueryValue<'a>(pub(super) &'a QueryValue);

impl PartialEq for HashableQueryValue<'_> {
    fn eq(&self, other: &Self) -> bool {
        canonical_eq(self.0, other.0)
    }
}
impl Eq for HashableQueryValue<'_> {}

impl std::hash::Hash for HashableQueryValue<'_> {
    fn hash<H: std::hash::Hasher>(&self, h: &mut H) {
        hash_qv(self.0, h);
    }
}

// ── Tag constants (mirror canonical-form discriminants used by hash_qv) ──

const TAG_NULL: u8 = 0;
const TAG_BOOL: u8 = 1;
const TAG_NUMBER: u8 = 2;
const TAG_STRING: u8 = 3;
const TAG_ARRAY: u8 = 4;
const TAG_OBJECT: u8 = 5;

// Number sub-tags (mirror hash_qv's branches)
const NUM_I64: u8 = 0;
const NUM_F64: u8 = 2;

// ── Hash ─────────────────────────────────────────────────────────────────────

/// Hash `qv` using the canonical form that preserves equivalence classes.
pub(super) fn hash_qv<H: std::hash::Hasher>(qv: &QueryValue, h: &mut H) {
    match qv {
        // Null → canonical Null
        QueryValue::Null => h.write_u8(TAG_NULL),

        // Bool → canonical Bool
        QueryValue::Bool(b) => {
            h.write_u8(TAG_BOOL);
            h.write_u8(*b as u8);
        }

        // Int(i) → canonical Number(i64) → sub-tag 0
        QueryValue::Int(i) => {
            h.write_u8(TAG_NUMBER);
            h.write_u8(NUM_I64);
            h.write_i64(*i);
        }

        // F64(f) → canonical Number (finite) or String (non-finite).
        // A finite f64 uses sub-tag 2 with raw bits.
        QueryValue::F64(f) => {
            if f.is_finite() {
                h.write_u8(TAG_NUMBER);
                h.write_u8(NUM_F64);
                h.write_u64(f.to_bits());
            } else {
                // Non-finite: canonical String fallback.
                hash_str_value(h, &f.to_string());
            }
        }

        // Dec(d) → canonical String(d.to_string()) — same class as Str(d.to_string())
        QueryValue::Dec(d) => hash_str_value(h, &d.to_string()),

        // Big(b) → canonical String(b.to_string()) — same class as Str(b.to_string())
        QueryValue::Big(b) => hash_str_value(h, &b.to_string()),

        // Str(s) → canonical String(s)
        QueryValue::Str(s) => hash_str_value(h, s),

        // Bin(bytes) → canonical Array([Number(byte as i64), ...])
        // Each byte b fits in i64 → sub-tag 0, value = b as i64.
        QueryValue::Bin(bytes) => {
            h.write_u8(TAG_ARRAY);
            h.write_u64(bytes.len() as u64);
            for &byte in bytes {
                h.write_u8(TAG_NUMBER);
                h.write_u8(NUM_I64);
                h.write_i64(byte as i64);
            }
        }

        // List(l) → canonical Array([...]) recursively
        QueryValue::List(l) => {
            h.write_u8(TAG_ARRAY);
            h.write_u64(l.len() as u64);
            for item in l {
                hash_qv(item, h);
            }
        }

        // Set(s) → canonical Array([...]) in TSet iteration order
        QueryValue::Set(s) => {
            h.write_u8(TAG_ARRAY);
            h.write_u64(s.len() as u64);
            for item in s {
                hash_qv(item, h);
            }
        }

        // Map(m) → canonical Object in IndexMap insertion order.
        // Our TMap<String, _> is IndexMap-backed so iteration order is stable.
        QueryValue::Map(m) => {
            h.write_u8(TAG_OBJECT);
            h.write_u64(m.len() as u64);
            for (k, v) in m {
                h.write(k.as_bytes());
                h.write_u8(0);
                hash_qv(v, h);
            }
        }
    }
}

/// Emit the bytes for a canonical String value.
#[inline]
fn hash_str_value<H: std::hash::Hasher>(h: &mut H, s: &str) {
    h.write_u8(TAG_STRING);
    h.write(s.as_bytes());
    h.write_u8(0xff);
}

// ── Eq ───────────────────────────────────────────────────────────────────────

/// Structural equality that mirrors the canonical coercion-based equality.
///
/// Key cross-type equalities:
/// - `Dec(a) == Str(b)`  iff  `a.to_string() == b`
/// - `Big(a) == Str(b)`  iff  `a.to_string() == b`
/// - `Dec(a) == Big(b)`  iff  `a.to_string() == b.to_string()`
/// - `F64(non-finite) == Str(b)` iff `f.to_string() == b`
fn canonical_eq(a: &QueryValue, b: &QueryValue) -> bool {
    // Fast path: both are the same variant.
    match (a, b) {
        (QueryValue::Null, QueryValue::Null) => true,
        (QueryValue::Bool(x), QueryValue::Bool(y)) => x == y,
        (QueryValue::Int(x), QueryValue::Int(y)) => x == y,

        // F64: finite → Number(bits); non-finite → String
        (QueryValue::F64(x), QueryValue::F64(y)) => match (x.is_finite(), y.is_finite()) {
            (true, true) => x.to_bits() == y.to_bits(),
            (false, false) => x.to_string() == y.to_string(),
            _ => false,
        },

        // All String-canonical variants: Dec, Big, Str, and non-finite F64.
        // They're all equal when their string representations match.
        (QueryValue::Dec(x), QueryValue::Dec(y)) => x.to_string() == y.to_string(),
        (QueryValue::Dec(x), QueryValue::Str(y)) | (QueryValue::Str(y), QueryValue::Dec(x)) => {
            x.to_string() == *y
        }
        (QueryValue::Dec(x), QueryValue::Big(y)) | (QueryValue::Big(y), QueryValue::Dec(x)) => {
            x.to_string() == y.to_string()
        }
        (QueryValue::Big(x), QueryValue::Big(y)) => x.to_string() == y.to_string(),
        (QueryValue::Big(x), QueryValue::Str(y)) | (QueryValue::Str(y), QueryValue::Big(x)) => {
            x.to_string() == *y
        }
        (QueryValue::Str(x), QueryValue::Str(y)) => x == y,

        // Non-finite F64 → String canonical form
        (QueryValue::F64(x), QueryValue::Str(y)) | (QueryValue::Str(y), QueryValue::F64(x))
            if !x.is_finite() =>
        {
            x.to_string() == *y
        }
        (QueryValue::F64(x), QueryValue::Dec(y)) | (QueryValue::Dec(y), QueryValue::F64(x))
            if !x.is_finite() =>
        {
            x.to_string() == y.to_string()
        }
        (QueryValue::F64(x), QueryValue::Big(y)) | (QueryValue::Big(y), QueryValue::F64(x))
            if !x.is_finite() =>
        {
            x.to_string() == y.to_string()
        }

        // Bin([b0, b1, ...]) → Array([Number(b0), ...])
        // Two Bins are equal iff their bytes are equal (Array comparison is element-wise).
        (QueryValue::Bin(x), QueryValue::Bin(y)) => x == y,
        // Bin vs List: a Bin[b0,b1,...] becomes Array of Numbers; a List would need
        // to consist of Int(b) values to be equal. This is technically possible but
        // extremely unlikely; we must replicate: Bin(bytes) == List(items) iff items
        // are exactly [Int(bytes[0] as i64), Int(bytes[1] as i64), ...].
        (QueryValue::Bin(bytes), QueryValue::List(items))
        | (QueryValue::List(items), QueryValue::Bin(bytes)) => {
            if bytes.len() != items.len() {
                return false;
            }
            bytes
                .iter()
                .zip(items.iter())
                .all(|(&b, item)| matches!(item, QueryValue::Int(i) if *i == b as i64))
        }

        // List vs List, Set vs Set, List vs Set (all map to canonical Array)
        (QueryValue::List(x), QueryValue::List(y)) => {
            x.len() == y.len() && x.iter().zip(y.iter()).all(|(a, b)| canonical_eq(a, b))
        }
        (QueryValue::Set(x), QueryValue::Set(y)) => {
            x.len() == y.len() && x.iter().zip(y.iter()).all(|(a, b)| canonical_eq(a, b))
        }
        (QueryValue::List(x), QueryValue::Set(y)) | (QueryValue::Set(y), QueryValue::List(x)) => {
            x.len() == y.len() && x.iter().zip(y.iter()).all(|(a, b)| canonical_eq(a, b))
        }

        // Map vs Map: structural equality on key-value pairs (insertion order)
        (QueryValue::Map(x), QueryValue::Map(y)) => {
            if x.len() != y.len() {
                return false;
            }
            x.iter()
                .zip(y.iter())
                .all(|((kx, vx), (ky, vy))| kx == ky && canonical_eq(vx, vy))
        }

        // Everything else: different canonical forms → not equal.
        _ => false,
    }
}
