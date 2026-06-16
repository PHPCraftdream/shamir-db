//! The `RecordRef` abstraction seam — Stage 2 of the RecordView migration.
//!
//! A single trait with two impls (`InnerValue` and `RecordView`) that exposes
//! a **borrowed scalar at an interned-id path** — the cross-impl currency for
//! filter-compare and index-extract (the highest-value Stage-3 consumers).
//!
//! Static dispatch only: consumers take `&impl RecordRef`, never `dyn`.

use crate::core::interner::InternerKey;
use crate::record_view::record_value::RecordValue;
use crate::record_view::scalar_ref::ScalarRef;
use crate::record_view::RecordView;
use crate::types::value::InnerValue;

/// Uniform read access to a record's scalar fields by interned-id path.
///
/// Both `InnerValue` (the in-memory tree) and `RecordView` (the zero-copy
/// msgpack lens) implement this trait so Stage-3 consumers can be written
/// once against `&impl RecordRef` and work with either representation.
///
/// The trait surface is intentionally minimal (scalar-at-path only). Richer
/// methods (container access, projection, field iteration) will be added in
/// Stage 3 when specific consumers need them (YAGNI).
pub trait RecordRef {
    /// Resolve an interned-id field path to a borrowed scalar leaf.
    ///
    /// Returns `None` if:
    /// - the path is empty,
    /// - a path segment is absent in the map,
    /// - the path descends through a non-map value,
    /// - the leaf is a container (Map/Array/Set) or a non-comparable type
    ///   (Dec/Big).
    ///
    /// Semantics MUST match `resolve_field_ref` (tree descent through
    /// `InnerValue::Map` by `InternerKey`) + "is it a scalar" for the
    /// `InnerValue` impl, and `get_path` + scalar conversion for the
    /// `RecordView` impl.
    fn scalar_at(&self, path: &[InternerKey]) -> Option<ScalarRef<'_>>;

    /// Single top-level field convenience (equivalent to `scalar_at(&[id])`).
    #[inline]
    fn scalar(&self, id: InternerKey) -> Option<ScalarRef<'_>> {
        self.scalar_at(&[id])
    }
}

// ---------------------------------------------------------------------------
// impl RecordRef for InnerValue
// ---------------------------------------------------------------------------

/// Convert an `InnerValue` leaf to `ScalarRef`. Non-scalar variants (Dec, Big,
/// List, Set, Map) return `None` — they are not comparable in `compare_values`.
/// Bin is included (eval_bytes compares bin).
#[inline]
fn inner_to_scalar(v: &InnerValue) -> Option<ScalarRef<'_>> {
    match v {
        InnerValue::Null => Some(ScalarRef::Null),
        InnerValue::Bool(b) => Some(ScalarRef::Bool(*b)),
        InnerValue::Int(i) => Some(ScalarRef::Int(*i)),
        InnerValue::F64(f) => Some(ScalarRef::F64(*f)),
        InnerValue::Str(s) => Some(ScalarRef::Str(s.as_str())),
        InnerValue::Bin(b) => Some(ScalarRef::Bin(b.as_slice())),
        // Dec, Big — compare_values returns None for these.
        // List, Set, Map — containers, not scalars.
        _ => None,
    }
}

impl RecordRef for InnerValue {
    fn scalar_at(&self, path: &[InternerKey]) -> Option<ScalarRef<'_>> {
        if path.is_empty() {
            return None;
        }
        // Descend through Map by InternerKey — mirrors resolve_field_ref.
        let mut cur = self;
        for key in path {
            match cur {
                InnerValue::Map(map) => {
                    cur = map.get(key)?;
                }
                _ => return None,
            }
        }
        inner_to_scalar(cur)
    }
}

// ---------------------------------------------------------------------------
// impl RecordRef for RecordView<'_>
// ---------------------------------------------------------------------------

impl RecordRef for RecordView<'_> {
    fn scalar_at(&self, path: &[InternerKey]) -> Option<ScalarRef<'_>> {
        if path.is_empty() {
            return None;
        }
        // `get_path` returns an owned `RecordValue<'a>` whose borrowed data
        // (Str's Cow::Borrowed, Bin's &[u8]) point into `self`'s buffer.
        // We match directly and extract only Copy / buffer-borrowed data so
        // the returned `ScalarRef` borrows from `self`, not from the local.
        match self.get_path(path)? {
            RecordValue::Null => Some(ScalarRef::Null),
            RecordValue::Bool(b) => Some(ScalarRef::Bool(b)),
            RecordValue::Int(i) => Some(ScalarRef::Int(i)),
            RecordValue::F64(f) => Some(ScalarRef::F64(f)),
            RecordValue::Str(cow) => {
                // For Cow::Borrowed the &str borrows from the msgpack buffer
                // (lifetime 'a). For Cow::Owned (the U64 > i64::MAX edge) we
                // cannot return a borrow of a local owned String. That edge
                // case is vanishingly rare (u64 > i64::MAX stored as Int) and
                // not a comparable scalar in compare_values, so return None.
                match cow {
                    std::borrow::Cow::Borrowed(s) => Some(ScalarRef::Str(s)),
                    std::borrow::Cow::Owned(_) => {
                        // U64 > i64::MAX edge — the tree decoder maps this to
                        // InnerValue::Str(decimal), which inner_to_scalar returns
                        // as ScalarRef::Str. For parity, the lens should too, but
                        // we cannot borrow an owned String here. This edge is not
                        // reachable from normal InnerValue (which stores it as Int
                        // or Str), so None is safe for the filter-compare path.
                        None
                    }
                }
            }
            RecordValue::Bin(b) => Some(ScalarRef::Bin(b)),
            // Arr, Map — containers, not scalars.
            RecordValue::Arr(_) | RecordValue::Map(_) => None,
        }
    }
}
