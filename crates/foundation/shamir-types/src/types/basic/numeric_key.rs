//! Canonical numeric key (RV-FUNCLIB-5FA455F7, D1-S2) — THE ONE EQUALITY RULE.
//!
//! **Two numeric values are equal iff their canonical [`NumericKey`]s are
//! equal, and neither is NaN. NaN is never equal to anything (itself
//! included).** This single rule drives every equality decision in the
//! engine (filter Eq/Ne, dynamic IN, `$contains`/`ContainsAny`/
//! `ContainsAll`, DISTINCT, GROUP BY, FK matching) and the shared hash
//! key for the index posting hashes. Consequences:
//!
//! - `Int(1) == Big(1) == Dec(1) == F64(1.0)`;
//!   `F64(-0.0) == F64(0.0) == Int(0) == Dec(0)`;
//! - `F64(2^53) != Big(2^53+1)`; `F64(2^63) == Big(2^63)` — an integral
//!   finite `f64` of ANY magnitude canonicalises to the same integer key
//!   as the equal `Int`/`Big`/`Dec` (integral ≥ 2^63 lands in `BigBits`,
//!   via exact bit decomposition, never `f64` arithmetic);
//! - `F64(0.5) != Dec(0.5)` — a non-integral `f64` and a non-integral
//!   `Dec` stay in their OWN key classes; the lossy `F64` ↔ `Dec`/`Big`
//!   comparison is ORDERING-only territory;
//! - ordering (`<`, `>`, sort) keeps the lossy `f64` path, untouched.
//!
//! Stored-data note (documented v1 projection, RV-TYPES-F28FAFBF): records
//! on disk serialise `Dec`/`Big` as their decimal STRING, so a GROUP BY /
//! DISTINCT over STORED bytes sees them as `Str` and groups them as strings.
//! A filter / FK comparison of a stored string against a TYPED `Dec`/`Big`
//! literal parses the string exactly and applies this rule. The exact
//! numeric classes above apply to the in-memory (type-preserving) path; see
//! the codecs README for the projection and the future typed encoding.

use std::hash::Hash;

use crate::types::value::Value;

use num_bigint::BigInt;
use num_bigint::Sign;
use rust_decimal::Decimal;

/// Hash/equality key for every numeric value that is equal to some `i64`
/// integer exactly, plus the raw-bit fallback for everything else.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum NumericKey {
    /// Every exact integer equal to some `i64`: `Int(n)`, an integral
    /// `f64` in `[-2^63, 2^63)` (`-0.0` → `0`), a `Big` fitting `i64`,
    /// a `Dec` with zero fractional part fitting `i64`.
    Int(i64),
    /// An exact integer OUTSIDE `i64` range (`Big` or integral `Dec`).
    /// Keyed by sign + big-endian magnitude so equal integers hash equal.
    BigBits {
        /// Sign of the integer.
        negative: bool,
        /// Big-endian magnitude bytes (minimal, no leading zeros).
        magnitude_be: Box<[u8]>,
    },
    /// Non-integral finite `f64`, ±inf, and NaN — raw bit pattern.
    F64Bits(u64),
}

impl NumericKey {
    /// Canonical key of an `f64` leaf. A finite INTEGRAL `f64` of ANY
    /// magnitude canonicalises to the integer key of its exact value
    /// (`|f| < 2^63` → `Int`, including `-0.0` → `0`; `|f| >= 2^63` → the
    /// `BigBits` key of the exact integer, via bit decomposition — no `f64`
    /// arithmetic). Non-integral finite, ±inf and NaN keep their raw bit
    /// pattern (`NaN` keys per its own bits; a NaN never EQUALS anything,
    /// including itself — callers pair the key with a NaN guard).
    #[inline]
    pub fn from_f64(f: f64) -> Self {
        if f.is_finite() && f.fract() == 0.0 {
            const I64_MIN_AS_F64: f64 = -9223372036854775808.0; // -2^63, exact
            const I64_MAX_EXCLUSIVE: f64 = 9223372036854775808.0; // 2^63, exact
            if (I64_MIN_AS_F64..I64_MAX_EXCLUSIVE).contains(&f) {
                return Self::Int(f as i64);
            }
            // Integral with |f| >= 2^63: exact integer via bit
            // decomposition (mantissa << exp, no f64 arithmetic), fed
            // through `from_bigint` so it lands in the SAME `BigBits`
            // variant as the equal `Big`/`Dec`.
            let bits = f.to_bits();
            let negative = bits >> 63 == 1;
            let exp_field = ((bits >> 52) & 0x7ff) as i32;
            let frac = bits & 0x000f_ffff_ffff_ffff;
            let (mantissa, exp) = if exp_field == 0 {
                (frac, -1074i32) // subnormal (only 0.0 is integral here)
            } else {
                (frac | (1u64 << 52), exp_field - 1075)
            };
            // |f| >= 2^63 and integral implies exp >= 11 >= 0; fall back to
            // the bit key rather than panic if that ever fails to hold.
            if exp >= 0 {
                let mut v = BigInt::from(mantissa);
                v <<= exp as usize;
                if negative {
                    v = -v;
                }
                return Self::from_bigint(&v);
            }
        }
        Self::F64Bits(f.to_bits())
    }

    /// Canonical key of a `Big` leaf — exact; never through `f64`.
    #[inline]
    pub fn from_bigint(b: &BigInt) -> Self {
        let (sign, mag_le) = b.to_bytes_le();
        if mag_le.len() <= 8 {
            let mut buf = [0u8; 8];
            buf[..mag_le.len()].copy_from_slice(&mag_le);
            let mag = u64::from_le_bytes(buf);
            match sign {
                Sign::NoSign | Sign::Plus if mag <= i64::MAX as u64 => {
                    return Self::Int(mag as i64);
                }
                Sign::Minus => {
                    if mag <= i64::MAX as u64 {
                        return Self::Int(-(mag as i64));
                    }
                    if mag == (i64::MAX as u64) + 1 {
                        return Self::Int(i64::MIN);
                    }
                }
                _ => {}
            }
        }
        let mut be = mag_le;
        be.reverse();
        Self::BigBits {
            negative: sign == Sign::Minus,
            magnitude_be: be.into(),
        }
    }

    /// Canonical key of a `Dec` leaf when it equals some exact integer
    /// (`Int`, `Big` or integral-`F64` equal). Returns `None` for a `Dec`
    /// with a nonzero fractional part — such a value is exactly equal to
    /// NO integer and keeps its own decimal representation as key.
    /// NOTE: for an integral `Dec` OUTSIDE `i64` (e.g. `2^63`) this returns
    /// `BigBits`, not `Int` — every caller must handle all variants.
    #[inline]
    pub fn from_decimal(d: &Decimal) -> Option<Self> {
        if d.is_zero() {
            return Some(Self::Int(0));
        }
        let n = d.normalize();
        if n.scale() != 0 {
            return None;
        }
        Some(Self::from_bigint(&BigInt::from(n.mantissa())))
    }

    /// Hash under the caller's stable u8 type tags. An `Int`, an integral
    /// `f64`, a `Big` fitting `i64` and an integral `Dec` all land in the
    /// `int_tag` bucket with the same `i64` payload — i.e. equal integers
    /// across those types hash IDENTICALLY.
    #[inline]
    pub fn hash_tagged<H: std::hash::Hasher>(
        &self,
        int_tag: u8,
        big_tag: u8,
        f64_tag: u8,
        state: &mut H,
    ) {
        match self {
            Self::Int(i) => {
                int_tag.hash(state);
                i.hash(state);
            }
            Self::BigBits {
                negative,
                magnitude_be,
            } => {
                big_tag.hash(state);
                negative.hash(state);
                magnitude_be.hash(state);
            }
            Self::F64Bits(bits) => {
                f64_tag.hash(state);
                bits.hash(state);
            }
        }
    }
}

/// Exact-value key parts of a `Dec` with no equal integer (`Dec(0.5)`):
/// the normalized `(mantissa, scale)`. Equal `Decimals` normalise to the
/// same pair (`0.50` → `0.5`), so hashing these parts gives exact decimal
/// equality/hash parity.
#[inline]
pub fn decimal_exact_parts(d: &Decimal) -> (i128, u32) {
    let n = d.normalize();
    (n.mantissa(), n.scale())
}

// Stored-data note: see the module docs (stored `Big`/`Dec` group/match as
// strings until D2).

/// The ONE numeric equality rule over `Value<K>` scalars — the predicate
/// the engine uses wherever it decides `==` (filter Eq/Ne, dynamic IN,
/// `$contains`/`ContainsAny`/`ContainsAll`, DISTINCT/GROUP BY classes, FK
/// matching): two numeric values are equal iff their canonical
/// [`NumericKey`]s are equal and neither is NaN. NaN is never equal to
/// anything, itself included. `Big` ↔ `Str` keeps the FG-6 convention:
/// the string must parse to the exact same `BigInt`.
///
/// ORDERING is a different contract: the engine-side `compare_values`
/// keeps the lossy `F64` ↔ `Dec`/`Big` path (S3 — ordering only); this
/// function deliberately does NOT mirror it (so `F64(0.5)` orders equal to
/// `Dec(0.5)` but is NOT equal to it).
pub fn numeric_eq_values<K>(a: &Value<K>, b: &Value<K>) -> bool
where
    K: Eq + std::hash::Hash + Ord + Clone + serde::Serialize + std::fmt::Debug,
{
    /// Canonical equality class of a value leaf: the shared [`NumericKey`]
    /// for every numeric leaf, plus the exact-decimal class for a
    /// non-integral `Dec` (which has NO integer key but still equals the
    /// identical decimal).
    enum Class {
        Num(NumericKey),
        DecParts(i128, u32),
    }
    impl PartialEq for Class {
        fn eq(&self, other: &Self) -> bool {
            match (self, other) {
                (Class::Num(x), Class::Num(y)) => x == y,
                (Class::DecParts(m, s), Class::DecParts(m2, s2)) => m == m2 && s == s2,
                _ => false,
            }
        }
    }
    impl Eq for Class {}

    fn class<K>(v: &Value<K>) -> Option<Class>
    where
        K: Eq + std::hash::Hash + Ord + Clone + serde::Serialize + std::fmt::Debug,
    {
        Some(match v {
            Value::Int(i) => Class::Num(NumericKey::Int(*i)),
            Value::F64(f) => Class::Num(NumericKey::from_f64(*f)),
            Value::Big(b) => Class::Num(NumericKey::from_bigint(b)),
            Value::Dec(d) => match NumericKey::from_decimal(d) {
                Some(key) => Class::Num(key),
                None => {
                    let (m, s) = decimal_exact_parts(d);
                    Class::DecParts(m, s)
                }
            },
            _ => return None,
        })
    }

    let is_nan = |v: &Value<K>| matches!(v, Value::F64(f) if f.is_nan());
    if let (Some(ca), Some(cb)) = (class(a), class(b)) {
        // The ONE rule: equal canonical keys AND neither side NaN.
        return ca == cb && !is_nan(a) && !is_nan(b);
    }
    match (a, b) {
        (Value::Null, Value::Null) => true,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::Str(x), Value::Str(y)) => x == y,
        (Value::Bin(x), Value::Bin(y)) => x == y,
        // FG-6 convention: a `Big` equals a `Str` iff the string parses to
        // the exact same integer.
        (Value::Big(x), Value::Str(y)) | (Value::Str(y), Value::Big(x)) => {
            y.parse::<BigInt>().map(|v| v == *x).unwrap_or(false)
        }
        _ => false,
    }
}
