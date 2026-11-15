//! Typed GROUP BY key fragments and the canonical-string fallback.
use shamir_types::codecs::interned::{inner_value_to_query_value, inner_value_to_query_value_with};
use shamir_types::core::interner::{Interner, InternerKey};
use shamir_types::record_view::ScalarRef;
use shamir_types::types::numeric_key::decimal_exact_parts;
use shamir_types::types::numeric_key::NumericKey;
use shamir_types::types::value::{InnerValue, QueryValue};

// ============================================================================
// Group-key fragment
// ============================================================================

/// Typed hashable key fragment used to bucket records under GROUP BY.
///
/// S4: built directly from the lens leaf (`ScalarRef`) — no extra
/// round-trip per record. Composite (Map/List/Set) group fields stay
/// rare; they fall back to a `Box<str>` canonical form materialised once
/// at the boundary (one `materialize_at` per row for that field only).
///
/// Numeric leaves share the engine-wide canonical numeric key
/// (`NumericKey`, RV-FUNCLIB-5FA455F7 S2): `Int(1)`, `F64(1.0)`, `-0.0`,
/// `Big(1)` and `Dec(1.0)` all key as one group; NaN keys per its own bit
/// pattern (each NaN is its own group); Int↔Big/Dec is exact — distinct
/// integers beyond `2^53` never collapse.
#[derive(Debug, Clone, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub(in crate::query::read) enum GroupKeyItem {
    Missing,
    Null,
    Bool(bool),
    Num(NumericKey),
    /// A NaN leaf — OCCURRENCE-unique (NaN != NaN): the payload is a
    /// per-execution counter advanced once per NaN leaf, so every NaN
    /// forms its own group (GROUP BY per-occurrence NaN, S2 contract).
    NanOnce(u64),
    Str(Box<str>),
    Bin(Box<[u8]>),
    Complex(Box<str>),
}

/// Build a `GroupKeyItem` from a borrowed `ScalarRef` (the common case — no
/// allocation beyond the leaf bytes).
#[inline]
pub(in crate::query::read) fn group_key_item_scalar(s: Option<ScalarRef<'_>>) -> GroupKeyItem {
    match s {
        None => GroupKeyItem::Missing,
        Some(ScalarRef::Null) => GroupKeyItem::Null,
        Some(ScalarRef::Bool(b)) => GroupKeyItem::Bool(b),
        Some(ScalarRef::Int(i)) => GroupKeyItem::Num(NumericKey::Int(i)),
        Some(ScalarRef::F64(f)) => GroupKeyItem::Num(NumericKey::from_f64(f)),
        Some(ScalarRef::Str(s)) => GroupKeyItem::Str(s.into()),
        Some(ScalarRef::Bin(b)) => GroupKeyItem::Bin(b.into()),
    }
}

/// Build the complex-group-key fallback from an owned `InnerValue` (a
/// container / Dec / Big leaf materialised once via `materialize_at`).
///
/// Numeric leaves (Dec/Big) use the shared canonical `NumericKey` (S2):
/// an integral Dec/Big keys like the equal Int; a non-integral Dec keys
/// by exact decimal value (normalized mantissa+scale). Other composites
/// convert to a canonical string that matches the equivalence classes of
/// the old string-keyed path — preserving grouping behaviour.
///
/// NaN occurrence caveat (S2 contract): per-occurrence NaN groups are
/// guaranteed only for SCALAR group fields (`group_key_item_scalar` →
/// `GroupKeyItem::NanOnce`). A NaN nested INSIDE a container group field
/// goes through [`query_value_canonical_str`], which renders every NaN as
/// the same `"NaN"` string — such NaNs are NOT occurrence-separated (they
/// share one group). This is a documented limitation of the string-keyed
/// container fallback, left as-is until a canonical-key container path
/// exists.
#[inline]
pub(in crate::query::read) fn group_key_item_complex(
    val: &InnerValue,
    interner: &Interner,
    overlay: Option<&shamir_collections::TFxMap<u64, String>>,
) -> GroupKeyItem {
    match val {
        InnerValue::Dec(d) => {
            if let Some(key) = NumericKey::from_decimal(d) {
                return GroupKeyItem::Num(key);
            }
            let (mantissa, scale) = decimal_exact_parts(d);
            GroupKeyItem::Complex(format!("dec:{mantissa}:{scale}").into_boxed_str())
        }
        InnerValue::Big(b) => GroupKeyItem::Num(NumericKey::from_bigint(b)),
        _ => {
            let qv = match overlay {
                Some(overlay) => inner_value_to_query_value_with(val, &|id| {
                    if id >= shamir_tx::OVERLAY_ID_BASE {
                        overlay.get(&id).cloned()
                    } else {
                        interner
                            .get_str(&InternerKey::new(id))
                            .map(|name| name.to_string())
                    }
                }),
                None => inner_value_to_query_value(val, interner),
            }
            .unwrap_or(QueryValue::Null);
            GroupKeyItem::Complex(query_value_canonical_str(&qv).into_boxed_str())
        }
    }
}

/// Produce a canonical string for a `QueryValue` that matches the equivalence
/// classes of the pre-J3 code path.
///
/// This gives the same GROUP BY equivalence classes as the pre-J3 code path,
/// with no external allocations.
fn query_value_canonical_str(qv: &QueryValue) -> String {
    match qv {
        QueryValue::Null => "null".to_string(),
        QueryValue::Bool(b) => {
            if *b {
                "true".to_string()
            } else {
                "false".to_string()
            }
        }
        QueryValue::Int(i) => i.to_string(),
        QueryValue::F64(f) => {
            if f.is_finite() {
                // Finite f64: use Rust's default float formatter which
                // uses Grisu/Dragon and produces the shortest round-trip
                // representation.
                format!("{f}")
            } else {
                // Non-finite f64 becomes a quoted string in the old path.
                format!("\"{}\"", f)
            }
        }
        QueryValue::Dec(d) => format!("\"{}\"", d),
        QueryValue::Big(b) => format!("\"{}\"", b),
        QueryValue::Str(s) => {
            // Escape the string: backslash and double-quote are the only
            // characters that must be escaped for the ASCII content that
            // appears in group keys. Full unicode escaping is not needed —
            // non-ASCII chars are written as UTF-8 (no \uXXXX).
            let mut out = String::with_capacity(s.len() + 2);
            out.push('"');
            for c in s.chars() {
                match c {
                    '"' => out.push_str("\\\""),
                    '\\' => out.push_str("\\\\"),
                    '\n' => out.push_str("\\n"),
                    '\r' => out.push_str("\\r"),
                    '\t' => out.push_str("\\t"),
                    c if (c as u32) < 0x20 => {
                        // Control characters: \uXXXX
                        let _ = std::fmt::Write::write_fmt(
                            &mut out,
                            format_args!("\\u{:04x}", c as u32),
                        );
                    }
                    c => out.push(c),
                }
            }
            out.push('"');
            out
        }
        QueryValue::Bin(bytes) => {
            // Bin → canonical Array([byte as i64, ...]) representation.
            let mut out = String::with_capacity(bytes.len() * 3 + 2);
            out.push('[');
            for (i, &b) in bytes.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                let _ = std::fmt::Write::write_fmt(&mut out, format_args!("{}", b));
            }
            out.push(']');
            out
        }
        QueryValue::List(l) => {
            let mut out = String::from("[");
            for (i, item) in l.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&query_value_canonical_str(item));
            }
            out.push(']');
            out
        }
        QueryValue::Set(s) => {
            let mut out = String::from("[");
            for (i, item) in s.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&query_value_canonical_str(item));
            }
            out.push(']');
            out
        }
        QueryValue::Map(m) => {
            let mut out = String::from("{");
            for (i, (k, v)) in m.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&query_value_canonical_str(&QueryValue::Str(k.clone())));
                out.push(':');
                out.push_str(&query_value_canonical_str(v));
            }
            out.push('}');
            out
        }
    }
}
