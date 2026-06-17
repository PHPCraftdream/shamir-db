//! The `RecordRef` abstraction seam — Stage 2 of the RecordView migration.
//!
//! A single trait with two impls (`InnerValue` and `RecordView`) that exposes
//! **borrowed scalar at an interned-id path** plus full read-path coverage
//! (kind, existence, sequence visitor, single-value materialise, field
//! iteration) — the cross-impl currency for filter-compare, index-extract,
//! projection, and computed-field evaluation (the Stage-3 consumers).
//!
//! Static dispatch only: consumers take `&impl RecordRef`, never `dyn`.

use crate::codecs::interned::{
    inner_to_json_value, inner_value_to_query_value, record_view_to_json_value,
    record_view_to_query_value,
};
use crate::core::interner::{Interner, InternerKey};
use crate::record_view::kind::Kind;
use crate::record_view::record_value::RecordValue;
use crate::record_view::scalar_ref::ScalarRef;
use crate::record_view::RecordView;
use crate::types::value::{InnerValue, QueryValue};

/// Uniform read access to a record's fields by interned-id path.
///
/// Both `InnerValue` (the in-memory tree) and `RecordView` (the zero-copy
/// msgpack lens) implement this trait so Stage-3 consumers can be written
/// once against `&impl RecordRef` and work with either representation.
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

    /// Classify the value at `path` without extracting it. Returns `None` if
    /// the path is absent or descends through a non-map.
    fn present_kind_at(&self, path: &[InternerKey]) -> Option<Kind>;

    /// Borrowed `&str` at `path` (Str leaf only). Returns `None` for
    /// non-string values or absent paths.
    #[inline]
    fn str_at(&self, path: &[InternerKey]) -> Option<&str> {
        match self.scalar_at(path) {
            Some(ScalarRef::Str(s)) => Some(s),
            _ => None,
        }
    }

    /// `true` iff a value exists at `path` (any kind, including Null).
    #[inline]
    fn exists_at(&self, path: &[InternerKey]) -> bool {
        self.present_kind_at(path).is_some()
    }

    /// `true` iff the value at `path` is absent OR is `Null`.
    #[inline]
    fn is_null_at(&self, path: &[InternerKey]) -> bool {
        matches!(self.present_kind_at(path), None | Some(Kind::Null))
    }

    /// If the value at `path` is a List or Set, visit each **scalar** element
    /// via `f`, short-circuiting when `f` returns `true`. Returns
    /// `Some(any-true)`. Non-scalar elements (nested map/array/Dec/Big) are
    /// **skipped** (they cannot equal a scalar RHS — mirrors `compare_values`
    /// returning `None`).
    ///
    /// Returns `None` iff the value at `path` is NOT a List/Set (caller
    /// treats as no-match).
    fn any_seq_elem(
        &self,
        path: &[InternerKey],
        f: &mut dyn FnMut(ScalarRef<'_>) -> bool,
    ) -> Option<bool>;

    /// Build an OWNED `InnerValue` for the value at `path` ONLY (one field /
    /// subtree, NEVER the whole record). The lens locates the value's msgpack
    /// sub-slice and calls `InnerValue::from_bytes(subslice)`; the tree
    /// descends and clones the subtree.
    fn materialize_at(&self, path: &[InternerKey]) -> Option<InnerValue>;

    /// Visit each top-level `(InternerKey, materialised InnerValue)` pair.
    /// Used for `SELECT *` / full-record projection. The lens iterates
    /// `fields()` and materialises each value; the tree iterates its map and
    /// clones each value.
    fn for_each_field(&self, f: &mut dyn FnMut(InternerKey, InnerValue));

    /// De-intern and convert the whole record to a [`QueryValue::Map`] with
    /// string keys. `InnerValue` delegates to the existing tree de-intern;
    /// `RecordView` uses the new O(N) lens walker.
    ///
    /// Returns `QueryValue::Null` on a de-intern error (missing key in interner).
    fn to_query_value(&self, interner: &Interner) -> QueryValue;

    /// De-intern and convert the whole record to a [`serde_json::Value::Object`].
    /// Same routing as `to_query_value` (tree vs lens path).
    ///
    /// Returns `serde_json::Value::Null` on a de-intern error.
    fn to_json_value(&self, interner: &Interner) -> serde_json::Value;
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

/// Classify an `InnerValue` variant into [`Kind`].
#[inline]
fn inner_to_kind(v: &InnerValue) -> Kind {
    match v {
        InnerValue::Null => Kind::Null,
        InnerValue::Bool(_)
        | InnerValue::Int(_)
        | InnerValue::F64(_)
        | InnerValue::Str(_)
        | InnerValue::Bin(_) => Kind::Scalar,
        InnerValue::Dec(_) | InnerValue::Big(_) => Kind::NonComparable,
        InnerValue::List(_) | InnerValue::Set(_) | InnerValue::Map(_) => Kind::Container,
    }
}

/// Navigate a path through `InnerValue::Map` descents and return a reference
/// to the leaf. Shared by `scalar_at`, `present_kind_at`, `any_seq_elem`,
/// `materialize_at`.
#[inline]
fn descend_path<'a>(root: &'a InnerValue, path: &[InternerKey]) -> Option<&'a InnerValue> {
    if path.is_empty() {
        return None;
    }
    let mut cur = root;
    for key in path {
        match cur {
            InnerValue::Map(map) => {
                cur = map.get(key)?;
            }
            _ => return None,
        }
    }
    Some(cur)
}

impl RecordRef for InnerValue {
    fn scalar_at(&self, path: &[InternerKey]) -> Option<ScalarRef<'_>> {
        descend_path(self, path).and_then(inner_to_scalar)
    }

    fn present_kind_at(&self, path: &[InternerKey]) -> Option<Kind> {
        descend_path(self, path).map(inner_to_kind)
    }

    fn any_seq_elem(
        &self,
        path: &[InternerKey],
        f: &mut dyn FnMut(ScalarRef<'_>) -> bool,
    ) -> Option<bool> {
        let val = descend_path(self, path)?;
        match val {
            InnerValue::List(items) => {
                for item in items {
                    if let Some(sr) = inner_to_scalar(item) {
                        if f(sr) {
                            return Some(true);
                        }
                    }
                    // Non-scalar elements (containers, Dec, Big) are skipped.
                }
                Some(false)
            }
            InnerValue::Set(items) => {
                for item in items {
                    if let Some(sr) = inner_to_scalar(item) {
                        if f(sr) {
                            return Some(true);
                        }
                    }
                }
                Some(false)
            }
            // Not a List/Set.
            _ => None,
        }
    }

    fn materialize_at(&self, path: &[InternerKey]) -> Option<InnerValue> {
        descend_path(self, path).cloned()
    }

    fn for_each_field(&self, f: &mut dyn FnMut(InternerKey, InnerValue)) {
        if let InnerValue::Map(map) = self {
            for (k, v) in map {
                f(k.clone(), v.clone());
            }
        }
    }

    fn to_query_value(&self, interner: &Interner) -> QueryValue {
        inner_value_to_query_value(self, interner).unwrap_or(QueryValue::Null)
    }

    fn to_json_value(&self, interner: &Interner) -> serde_json::Value {
        inner_to_json_value(self, interner).unwrap_or(serde_json::Value::Null)
    }
}

// ---------------------------------------------------------------------------
// impl RecordRef for RecordView<'_>
// ---------------------------------------------------------------------------

/// Classify a `RecordValue` variant into [`Kind`]. The lens cannot
/// distinguish Dec/Big from Str (they share the same msgpack str marker),
/// so all string-marker values map to `Scalar`.
#[inline]
fn record_value_to_kind(v: &RecordValue<'_>) -> Kind {
    match v {
        RecordValue::Null => Kind::Null,
        RecordValue::Bool(_)
        | RecordValue::Int(_)
        | RecordValue::F64(_)
        | RecordValue::Str(_)
        | RecordValue::Bin(_) => Kind::Scalar,
        RecordValue::Arr(_) | RecordValue::Map(_) => Kind::Container,
    }
}

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

    fn present_kind_at(&self, path: &[InternerKey]) -> Option<Kind> {
        if path.is_empty() {
            return None;
        }
        self.get_path(path).map(|v| record_value_to_kind(&v))
    }

    fn any_seq_elem(
        &self,
        path: &[InternerKey],
        f: &mut dyn FnMut(ScalarRef<'_>) -> bool,
    ) -> Option<bool> {
        if path.is_empty() {
            return None;
        }
        let val = self.get_path(path)?;
        match val {
            RecordValue::Arr(raw_seq) => {
                for elem in raw_seq.iter() {
                    let sr = match &elem {
                        RecordValue::Null => Some(ScalarRef::Null),
                        RecordValue::Bool(b) => Some(ScalarRef::Bool(*b)),
                        RecordValue::Int(i) => Some(ScalarRef::Int(*i)),
                        RecordValue::F64(fv) => Some(ScalarRef::F64(*fv)),
                        RecordValue::Str(cow) => match cow {
                            std::borrow::Cow::Borrowed(s) => Some(ScalarRef::Str(s)),
                            std::borrow::Cow::Owned(_) => None,
                        },
                        RecordValue::Bin(b) => Some(ScalarRef::Bin(b)),
                        // Containers skipped — cannot match a scalar RHS.
                        RecordValue::Arr(_) | RecordValue::Map(_) => None,
                    };
                    if let Some(sr) = sr {
                        if f(sr) {
                            return Some(true);
                        }
                    }
                }
                Some(false)
            }
            // The lens sees Set as Arr (msgpack arrays); both List and Set
            // are serialised as arrays. This arm handles both.
            // Non-list/set values.
            _ => None,
        }
    }

    fn materialize_at(&self, path: &[InternerKey]) -> Option<InnerValue> {
        let bytes = self.value_bytes_at(path)?;
        InnerValue::from_bytes(bytes).ok()
    }

    fn for_each_field(&self, f: &mut dyn FnMut(InternerKey, InnerValue)) {
        for (key, _val) in self.fields() {
            // Materialize each value via value_bytes_at with a single-segment path.
            let k = key.clone();
            if let Some(iv) = self.materialize_at(&[key]) {
                f(k, iv);
            }
        }
    }

    fn to_query_value(&self, interner: &Interner) -> QueryValue {
        record_view_to_query_value(self, interner).unwrap_or(QueryValue::Null)
    }

    fn to_json_value(&self, interner: &Interner) -> serde_json::Value {
        record_view_to_json_value(self, interner).unwrap_or(serde_json::Value::Null)
    }
}
