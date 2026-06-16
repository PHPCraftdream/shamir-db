//! Borrowed scalar leaf — the cross-impl currency for the [`RecordRef`] trait.
//!
//! Mirrors [`eval_bytes::RawScalar`] in shape (Null/Bool/Int/F64/Str/Bin) but
//! is a public, `PartialEq + Debug` type suitable for assertions and Stage-3
//! consumer code. Only scalar types that [`compare_values`] and
//! `eval_bytes::compare_raw_to_filter` handle are represented — containers
//! (Map/Array), `Dec`, `Big`, `Set` are NOT scalars and resolve to `None` from
//! [`RecordRef::scalar_at`].
//!
//! [`RecordRef`]: super::RecordRef
//! [`compare_values`]: (engine-internal — see `resolve.rs`)

/// A borrowed scalar leaf extracted from a record. The cross-impl currency
/// that makes `InnerValue` and `RecordView` interchangeable at the trait level.
///
/// # Why these variants and no others
///
/// * **Null, Bool, Int, F64, Str, Bin** — these are the types that
///   `compare_values` (tree path) and `compare_raw_to_filter` (bytes path)
///   can compare. They are the scalar types that filter-eval and index-extract
///   (the first Stage-3 consumers) need.
/// * **Dec / Big** — `compare_values` returns `None` for these (not comparable
///   as scalars in the current filter algebra). Mapped to `None` by both impls.
/// * **List / Set / Map** — containers, not scalars. Mapped to `None`.
///
/// # NaN caveat
///
/// `PartialEq` for `F64` compares with `==`, so `NaN != NaN`. The parity
/// tests use a helper that compares `f64` via `to_bits()` to handle this.
#[derive(Debug, Clone, Copy)]
pub enum ScalarRef<'a> {
    /// Null / nil.
    Null,
    /// Boolean.
    Bool(bool),
    /// Signed 64-bit integer (all int widths collapse here, matching `InnerValue::Int`).
    Int(i64),
    /// 64-bit float (F32 widened, matching `InnerValue::F64`).
    F64(f64),
    /// Borrowed string slice.
    Str(&'a str),
    /// Borrowed binary slice.
    Bin(&'a [u8]),
}

impl<'a> PartialEq for ScalarRef<'a> {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (ScalarRef::Null, ScalarRef::Null) => true,
            (ScalarRef::Bool(a), ScalarRef::Bool(b)) => a == b,
            (ScalarRef::Int(a), ScalarRef::Int(b)) => a == b,
            (ScalarRef::F64(a), ScalarRef::F64(b)) => a.to_bits() == b.to_bits(),
            (ScalarRef::Str(a), ScalarRef::Str(b)) => a == b,
            (ScalarRef::Bin(a), ScalarRef::Bin(b)) => a == b,
            _ => false,
        }
    }
}

impl<'a> Eq for ScalarRef<'a> {}
