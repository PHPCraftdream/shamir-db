//! `HashableQueryValue` — `Hash + Eq` wrapper for `QueryValue`.
//!
//! Deduplication equivalence classes follow the S2/S3 contract of
//! RV-FUNCLIB-5FA455F7: equality is EXACT and hashable, sharing the
//! engine-wide canonical numeric key (`shamir_types NumericKey`):
//!
//! | Variant | Canonical class |
//! |---------|-----------------|
//! | `Null`  | Null            |
//! | `Bool`  | Bool            |
//! | `Int(i)` | exact integer `i` |
//! | `F64` integral (ANY magnitude) | the equal integer key (`F64(1.0)` == `Int(1)`, `-0.0` == `0`, `F64(2^63)` == `Big(2^63)`) |
//! | `F64` non-integral / ±inf / NaN | raw bits (each NaN its own class) |
//! | `Big(b)` | the equal `Int` when `b` fits `i64`, else exact sign+magnitude |
//! | `Dec(d)` | the equal `Int` when `d` is integral, else exact normalized decimal |
//! | `Str(s)` | String |
//! | `Bin(b)` | Array([Number(byte as i64), ...]) |
//! | `List(l)` | Array([...]) recursively |
//! | `Set(s)` | Array([...]) in iteration order |
//! | `Map(m)` | Object({...}) in insertion order |
//!
//! `Int` ↔ `Big`/`Dec` equality is EXACT — never through `f64`, so distinct
//! integers beyond `2^53` stay distinct. `Str` is equal only to `Str` (the
//! old `Dec("1.0") == Str("1.0")` / `Big == Str` / NaN-string classes are
//! gone). The lossy `F64` ↔ `Dec`/`Big` comparison is ORDERING-only (S3):
//! a non-integral `F64` never deduplicates against a `Dec`/`Big`, even
//! where the filter's ordering-based `Eq` happens to match.

use shamir_types::types::numeric_key::decimal_exact_parts;
use shamir_types::types::numeric_key::NumericKey;
use shamir_types::types::value::QueryValue;

/// Wrapper that gives `QueryValue` a `Hash + Eq` implementation whose
/// equivalence classes follow the exact canonical contract above.
pub(in crate::query::read) struct HashableQueryValue<'a>(pub(in crate::query::read) &'a QueryValue);

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
const TAG_BIG: u8 = 6;
const TAG_DEC: u8 = 7;

// Number sub-tags
const NUM_I64: u8 = 0;
const NUM_F64: u8 = 2;

// ── Exact numeric class ──────────────────────────────────────────────────────

/// Exact equality/hash class of a numeric `QueryValue` leaf (RV-FUNCLIB-
/// 5FA455F7 S2). `None` for non-numeric values.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum NumClass {
    /// Any value exactly equal to an `i64` integer (Int, integral F64,
    /// integral Dec, Big fitting i64).
    Int(i64),
    /// Exact integer outside i64 (Big / integral Dec), sign + magnitude.
    Big {
        negative: bool,
        magnitude_be: Box<[u8]>,
    },
    /// Non-integral finite F64, ±inf, NaN — raw bits.
    F64Bits(u64),
    /// Non-integral Dec — exact normalized (mantissa, scale).
    Dec(i128, u32),
}

/// Total mapping from a canonical numeric key to its DISTINCT class.
fn class_of_key(key: NumericKey) -> NumClass {
    match key {
        NumericKey::Int(i) => NumClass::Int(i),
        NumericKey::F64Bits(b) => NumClass::F64Bits(b),
        NumericKey::BigBits {
            negative,
            magnitude_be,
        } => NumClass::Big {
            negative,
            magnitude_be,
        },
    }
}

fn num_class(qv: &QueryValue) -> Option<NumClass> {
    match qv {
        QueryValue::Int(i) => Some(NumClass::Int(*i)),
        QueryValue::F64(f) => Some(class_of_key(NumericKey::from_f64(*f))),
        QueryValue::Big(b) => Some(class_of_key(NumericKey::from_bigint(b))),
        QueryValue::Dec(d) => Some(match NumericKey::from_decimal(d) {
            Some(key) => class_of_key(key),
            None => {
                let (m, s) = decimal_exact_parts(d);
                NumClass::Dec(m, s)
            }
        }),
        _ => None,
    }
}

fn hash_num_class<H: std::hash::Hasher>(c: &NumClass, h: &mut H) {
    match c {
        NumClass::Int(i) => {
            h.write_u8(TAG_NUMBER);
            h.write_u8(NUM_I64);
            h.write_i64(*i);
        }
        NumClass::F64Bits(b) => {
            h.write_u8(TAG_NUMBER);
            h.write_u8(NUM_F64);
            h.write_u64(*b);
        }
        NumClass::Big {
            negative,
            magnitude_be,
        } => {
            h.write_u8(TAG_BIG);
            h.write_u8(*negative as u8);
            h.write(magnitude_be);
        }
        NumClass::Dec(m, s) => {
            h.write_u8(TAG_DEC);
            h.write_i128(*m);
            h.write_u32(*s);
        }
    }
}

// ── Hash ─────────────────────────────────────────────────────────────────────

/// Hash `qv` using the canonical form that preserves equivalence classes.
pub(in crate::query::read) fn hash_qv<H: std::hash::Hasher>(qv: &QueryValue, h: &mut H) {
    // Numeric leaves first: their class IS the canonical form.
    if let Some(class) = num_class(qv) {
        hash_num_class(&class, h);
        return;
    }
    match qv {
        QueryValue::Null => h.write_u8(TAG_NULL),

        QueryValue::Bool(b) => {
            h.write_u8(TAG_BOOL);
            h.write_u8(*b as u8);
        }

        QueryValue::Str(s) => hash_str_value(h, s),

        // Bin(bytes) → canonical Array([Number(byte as i64), ...])
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
        QueryValue::Map(m) => {
            h.write_u8(TAG_OBJECT);
            h.write_u64(m.len() as u64);
            for (k, v) in m {
                h.write(k.as_bytes());
                h.write_u8(0);
                hash_qv(v, h);
            }
        }

        // Numeric leaves are fully handled by num_class above.
        QueryValue::Int(_) | QueryValue::F64(_) | QueryValue::Big(_) | QueryValue::Dec(_) => {
            unreachable!("numeric leaf not handled by num_class")
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

/// Structural equality that mirrors the canonical classes above: numeric
/// leaves compare by exact numeric class (Int/F64-integral/Big/Dec unify
/// exactly); everything else compares structurally; `Str` is equal only
/// to `Str`.
fn canonical_eq(a: &QueryValue, b: &QueryValue) -> bool {
    // Exact numeric classes first (covers Int/F64/Big/Dec in any mix).
    if let (Some(ca), Some(cb)) = (num_class(a), num_class(b)) {
        return ca == cb;
    }

    match (a, b) {
        (QueryValue::Null, QueryValue::Null) => true,
        (QueryValue::Bool(x), QueryValue::Bool(y)) => x == y,
        (QueryValue::Str(x), QueryValue::Str(y)) => x == y,

        // Bin(bytes) → Array([Number(b0), ...]); equal iff bytes equal, or
        // (legacy classes) the List is exactly [Int(bytes[0]), ...].
        (QueryValue::Bin(x), QueryValue::Bin(y)) => x == y,
        (QueryValue::Bin(bytes), QueryValue::List(items))
        | (QueryValue::List(items), QueryValue::Bin(bytes)) => {
            bytes.len() == items.len()
                && bytes
                    .iter()
                    .zip(items.iter())
                    .all(|(&b, item)| matches!(item, QueryValue::Int(i) if *i == b as i64))
        }

        // List/Set (and List↔Set) all map to canonical Array.
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
            x.len() == y.len()
                && x.iter()
                    .zip(y.iter())
                    .all(|((kx, vx), (ky, vy))| kx == ky && canonical_eq(vx, vy))
        }

        // Everything else (including numeric vs Str, Str vs containers):
        // different canonical forms → not equal.
        _ => false,
    }
}
