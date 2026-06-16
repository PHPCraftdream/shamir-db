//! The [`Kind`] enum — classifies a value's type for `RecordRef::present_kind_at`.
//!
//! One primary export: [`Kind`]. Used by the `RecordRef` trait to report what
//! kind of value lives at a given path without extracting the value itself.

/// Classifies the type of a value at a path within a record.
///
/// Returned by [`RecordRef::present_kind_at`](super::RecordRef::present_kind_at)
/// to let callers branch on the value's category without extracting or
/// materialising it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// The value is `Null` / nil.
    Null,
    /// The value is a comparable scalar (Bool, Int, F64, Str, Bin).
    Scalar,
    /// The value is a container (Map, List/Array, Set).
    Container,
    /// The value is a non-comparable type (Dec, Big) — serialised as Str on
    /// the wire, but semantically distinct in the original tree. The lens
    /// cannot distinguish Dec/Big from Str (they share the same msgpack
    /// marker), so the lens maps all string-marker values to `Scalar`. This
    /// variant is only produced by the `InnerValue` impl.
    NonComparable,
}
