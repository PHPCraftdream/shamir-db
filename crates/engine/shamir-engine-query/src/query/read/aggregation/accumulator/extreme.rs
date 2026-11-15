//! Owned Min/Max running-extreme types (the §5b boundary #2 currency).
use shamir_types::types::value::InnerValue;

// ============================================================================
// OwnedExtreme — the running-state currency for Min/Max (§5b boundary #2).
// ============================================================================

/// The running extreme held across rows by Min/Max. Two reprs:
///
/// - `Scalar` — the overwhelming common case (Min/Max over Int/F64/Str/Bool).
///   Owns at most the leaf bytes; the incoming `ScalarRef` is compared
///   against it directly via [`OwnedScalar::cmp_scalar`] — no `Value` tree.
/// - `Tree` — the rare Min/Max-over-container case (Map/List/Set/Dec/Big).
///   Owns an `InnerValue` (the materialised container leaf from
///   `materialize_at`); comparison is via the generic `compare_values`
///   which is key-agnostic for scalars. Converted to `QueryValue` only
///   in `finish` (§5b boundary #3: output form).
///
/// This split keeps the hot scalar path allocation-light (a single i64 / f64
/// / small Box) while still preserving byte-identity for the rare container
/// case (which the pre-S4 path supported by holding `&'a InnerValue`).
/// Storing `InnerValue` in `Tree` avoids the need for `&Interner` in `step`.
pub(in crate::query::read) enum OwnedExtreme {
    Scalar(OwnedScalar),
    Tree(InnerValue),
}

/// An owned scalar leaf. Cheaper than a `Value<K>` tree: only the 6 scalar
/// arms are represented.
#[derive(Debug, Clone)]
pub(in crate::query::read) enum OwnedScalar {
    Null,
    Bool(bool),
    Int(i64),
    F64(f64),
    Str(Box<str>),
    Bin(Box<[u8]>),
}
