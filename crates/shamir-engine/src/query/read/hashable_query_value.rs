//! `HashableQueryValue` — `Hash + Eq` wrapper for `QueryValue`.
//!
//! Provides deduplication equivalence classes that are **identical** to those
//! produced by the old `From<QueryValue> for serde_json::Value` coercion path:
//!
//! | Variant | Canonical form |
//! |---------|---------------|
//! | `Null`  | json Null      |
//! | `Bool`  | json Bool      |
//! | `Int`   | json Number(i64) |
//! | `F64(finite)` | json Number(f64 bits) |
//! | `F64(non-finite)` | json String(f.to_string()) |
//! | `Dec(d)` | json String(d.to_string()) — **same class as `Str(d.to_string())`** |
//! | `Big(b)` | json String(b.to_string()) — **same class as `Str(b.to_string())`** |
//! | `Str(s)` | json String(s) |
//! | `Bin(b)` | json Array([Number(byte as i64), ...]) |
//! | `List(l)` | json Array([...]) recursively |
//! | `Set(s)` | json Array([...]) in iteration order |
//! | `Map(m)` | json Object({...}) in insertion order |
//!
//! No serde_json allocations are performed — everything is a structural walk.

use shamir_types::types::value::QueryValue;

/// Wrapper that gives `QueryValue` a `Hash + Eq` implementation whose
/// equivalence classes exactly match those of
/// `HashableJson(serde_json::Value::from(qv))`.
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

// ── Tag constants (mirror serde_json Value discriminants used by hash_json) ──

const TAG_NULL: u8 = 0;
const TAG_BOOL: u8 = 1;
const TAG_NUMBER: u8 = 2;
const TAG_STRING: u8 = 3;
const TAG_ARRAY: u8 = 4;
const TAG_OBJECT: u8 = 5;

// Number sub-tags (mirror hash_json's branches)
const NUM_I64: u8 = 0;
const NUM_F64: u8 = 2;

// ── Hash ─────────────────────────────────────────────────────────────────────

/// Hash `qv` using the same canonical form as `hash_json(Value::from(qv), h)`.
pub(super) fn hash_qv<H: std::hash::Hasher>(qv: &QueryValue, h: &mut H) {
    match qv {
        // Null → json Null
        QueryValue::Null => h.write_u8(TAG_NULL),

        // Bool → json Bool
        QueryValue::Bool(b) => {
            h.write_u8(TAG_BOOL);
            h.write_u8(*b as u8);
        }

        // Int(i) → json Number via i64.into() → as_i64() succeeds → sub-tag 0
        QueryValue::Int(i) => {
            h.write_u8(TAG_NUMBER);
            h.write_u8(NUM_I64);
            h.write_i64(*i);
        }

        // F64(f) → json Number via Number::from_f64(f) — returns None for
        // non-finite → String(f.to_string()) for NaN/±inf, otherwise Number.
        // In hash_json a finite f64 never satisfies as_i64()/as_u64(), so it
        // falls to as_f64() → sub-tag 2, bits.
        QueryValue::F64(f) => {
            if f.is_finite() {
                h.write_u8(TAG_NUMBER);
                h.write_u8(NUM_F64);
                h.write_u64(f.to_bits());
            } else {
                // Non-finite: Number::from_f64 returns None → String fallback.
                hash_str_value(h, &f.to_string());
            }
        }

        // Dec(d) → json String(d.to_string()) — same class as Str(d.to_string())
        QueryValue::Dec(d) => hash_str_value(h, &d.to_string()),

        // Big(b) → json String(b.to_string()) — same class as Str(b.to_string())
        QueryValue::Big(b) => hash_str_value(h, &b.to_string()),

        // Str(s) → json String(s)
        QueryValue::Str(s) => hash_str_value(h, s),

        // Bin(bytes) → json Array([Number(byte as i64), ...])
        // Each byte b: byte.into() → serde_json Number from u8, as_i64() succeeds
        // (fits in i64) → sub-tag 0, value = b as i64.
        QueryValue::Bin(bytes) => {
            h.write_u8(TAG_ARRAY);
            h.write_u64(bytes.len() as u64);
            for &byte in bytes {
                h.write_u8(TAG_NUMBER);
                h.write_u8(NUM_I64);
                h.write_i64(byte as i64);
            }
        }

        // List(l) → json Array([...]) recursively
        QueryValue::List(l) => {
            h.write_u8(TAG_ARRAY);
            h.write_u64(l.len() as u64);
            for item in l {
                hash_qv(item, h);
            }
        }

        // Set(s) → json Array([...]) in TSet iteration order
        QueryValue::Set(s) => {
            h.write_u8(TAG_ARRAY);
            h.write_u64(s.len() as u64);
            for item in s {
                hash_qv(item, h);
            }
        }

        // Map(m) → json Object in IndexMap insertion order.
        // In hash_json Object iteration uses serde_json::Map which preserves
        // insertion order (also IndexMap-backed). Our TMap<String, _> is also
        // IndexMap-backed so iteration order matches.
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

/// Emit the bytes for a canonical json String value.
#[inline]
fn hash_str_value<H: std::hash::Hasher>(h: &mut H, s: &str) {
    h.write_u8(TAG_STRING);
    h.write(s.as_bytes());
    h.write_u8(0xff);
}

// ── Eq ───────────────────────────────────────────────────────────────────────

/// Structural equality that mirrors `serde_json::Value::eq` after the
/// `From<QueryValue>` coercion.
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
        // extremely unlikely; the old code compared serde_json::Value structurally, so
        // we must replicate: Bin(bytes) == List(items) iff items are exactly
        // [Int(bytes[0] as i64), Int(bytes[1] as i64), ...].
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

        // List vs List, Set vs Set, List vs Set (all become Array in json)
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

#[cfg(test)]
mod tests {
    use super::*;
    use num_bigint::BigInt;
    use rust_decimal::Decimal;
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    fn hash_of(qv: &QueryValue) -> u64 {
        let mut h = DefaultHasher::new();
        HashableQueryValue(qv).hash(&mut h);
        h.finish()
    }

    fn eq(a: &QueryValue, b: &QueryValue) -> bool {
        HashableQueryValue(a) == HashableQueryValue(b)
    }

    // ── Dec / Str same-class ─────────────────────────────────────────────────

    #[test]
    fn dec_str_same_hash_and_eq() {
        let dec = QueryValue::Dec("1.0".parse::<Decimal>().unwrap());
        let str = QueryValue::Str("1.0".to_string());
        assert_eq!(
            hash_of(&dec),
            hash_of(&str),
            "Dec and Str must hash identically"
        );
        assert!(eq(&dec, &str), "Dec and Str with same form must be equal");
        assert!(eq(&str, &dec), "symmetry");
    }

    #[test]
    fn dec_str_different_not_eq() {
        let dec = QueryValue::Dec("1.5".parse::<Decimal>().unwrap());
        let str = QueryValue::Str("2.5".to_string());
        assert!(!eq(&dec, &str));
    }

    // ── Big / Str same-class ─────────────────────────────────────────────────

    #[test]
    fn big_str_same_hash_and_eq() {
        let big = QueryValue::Big(BigInt::from(42));
        let str = QueryValue::Str("42".to_string());
        assert_eq!(
            hash_of(&big),
            hash_of(&str),
            "Big and Str must hash identically"
        );
        assert!(eq(&big, &str), "Big and Str with same form must be equal");
        assert!(eq(&str, &big), "symmetry");
    }

    // ── F64 finite hashes by bits ────────────────────────────────────────────

    #[test]
    fn f64_finite_eq_by_bits() {
        let a = QueryValue::F64(1.5);
        let b = QueryValue::F64(1.5);
        let c = QueryValue::F64(2.5);
        assert_eq!(hash_of(&a), hash_of(&b));
        assert!(eq(&a, &b));
        assert!(!eq(&a, &c));
    }

    #[test]
    fn f64_nonfinite_maps_to_string() {
        let nan = QueryValue::F64(f64::NAN);
        let nan_str = QueryValue::Str("NaN".to_string());
        assert_eq!(hash_of(&nan), hash_of(&nan_str));
        assert!(eq(&nan, &nan_str));

        let inf = QueryValue::F64(f64::INFINITY);
        let inf_str = QueryValue::Str("inf".to_string());
        assert_eq!(hash_of(&inf), hash_of(&inf_str));
        assert!(eq(&inf, &inf_str));
    }

    // ── Bin dedup ────────────────────────────────────────────────────────────

    #[test]
    fn bin_same_bytes_eq() {
        let a = QueryValue::Bin(vec![1, 2, 3]);
        let b = QueryValue::Bin(vec![1, 2, 3]);
        assert_eq!(hash_of(&a), hash_of(&b));
        assert!(eq(&a, &b));
    }

    #[test]
    fn bin_different_bytes_not_eq() {
        let a = QueryValue::Bin(vec![1, 2]);
        let b = QueryValue::Bin(vec![1, 3]);
        assert!(!eq(&a, &b));
    }

    // ── Null ─────────────────────────────────────────────────────────────────

    #[test]
    fn null_eq_null() {
        assert!(eq(&QueryValue::Null, &QueryValue::Null));
        assert!(!eq(&QueryValue::Null, &QueryValue::Int(0)));
    }

    // ── Int distinct from String ─────────────────────────────────────────────

    #[test]
    fn int_not_eq_str() {
        let int = QueryValue::Int(42);
        let str = QueryValue::Str("42".to_string());
        // Int → Number; Str → String; different canonical forms.
        assert!(!eq(&int, &str));
        assert_ne!(hash_of(&int), hash_of(&str));
    }

    // ── Map insertion-order eq ───────────────────────────────────────────────

    #[test]
    fn map_eq_same_order() {
        use shamir_types::types::common::new_map_wc;
        let mut m1 = new_map_wc(2);
        m1.insert("a".to_string(), QueryValue::Int(1));
        m1.insert("b".to_string(), QueryValue::Int(2));
        let mut m2 = new_map_wc(2);
        m2.insert("a".to_string(), QueryValue::Int(1));
        m2.insert("b".to_string(), QueryValue::Int(2));
        assert!(eq(&QueryValue::Map(m1), &QueryValue::Map(m2)));
    }
}
