//! By-name field access for validators — zero interning on the call path.
//!
//! [`RecordFields`] is the universal by-name entry point for both native and
//! WASM validators. Interning ids are hidden from the author; the validator
//! writes `fields.scalar(&["age"])` and the backing resolves names → ids
//! lazily (for [`ViewFields`]) or via direct string lookup (for
//! [`OwnedFields`]).
//!
//! Two backings ship with Phase 0:
//!
//! - [`ViewFields`] — wraps a zero-copy [`RecordView`] + [`Interner`] reference.
//!   Name → id resolution is lazy and point-wise (no full de-intern).
//!   Used for the DELETE path (and will be used for INSERT/UPDATE once the
//!   write path exposes a `RecordView`).
//!
//! - [`OwnedFields`] — wraps a `&QueryValue` (the `QueryValue::Map` form
//!   already produced by INSERT/UPDATE resolved_values). Direct string key
//!   lookup; no interner needed.

use shamir_types::core::interner::Interner;
use shamir_types::record_view::{Kind, RecordRef, RecordView, ScalarRef};
use shamir_types::types::value::{InnerValue, QueryValue};

// ── trait ────────────────────────────────────────────────────────────────────

/// By-name read-only access to a record's fields.
///
/// Path segments are field names as `&str` slices — no interning ids visible
/// to the validator author.  Implementations resolve names lazily.
///
/// `Send + Sync` are required so the `&dyn RecordFields` reference can be
/// held safely across `.await` points on the tokio write path.
pub trait RecordFields: Send + Sync {
    /// Borrowed scalar at `path` (zero-copy for comparable scalars).
    ///
    /// Returns `None` if the path is absent, traverses a non-map, or the leaf
    /// is a container / non-comparable type.
    fn scalar(&self, path: &[&str]) -> Option<ScalarRef<'_>>;

    /// Borrowed `&str` at `path` (string scalar only).
    ///
    /// Convenience shorthand for `scalar(path) == Some(ScalarRef::Str(_))`.
    fn str(&self, path: &[&str]) -> Option<&str> {
        match self.scalar(path) {
            Some(ScalarRef::Str(s)) => Some(s),
            _ => None,
        }
    }

    /// Classify the value at `path` without extracting it.
    ///
    /// Returns `None` if the path is absent or traverses a non-map.
    fn present(&self, path: &[&str]) -> Option<Kind>;

    /// Materialise the value at `path` as an owned [`InnerValue`].
    ///
    /// Intended for containers (List/Map/Set) and other non-scalar types.
    /// Scalar callers should prefer [`scalar`](Self::scalar) (zero-copy).
    fn materialize(&self, path: &[&str]) -> Option<InnerValue>;

    /// Materialise the **whole record** as an owned `QueryValue::Map`.
    ///
    /// This is the §5b escape hatch used exclusively by [`WasmRecordValidator`]
    /// to build the `Params` that the WASM guest ABI requires. Native and
    /// declarative validators never call this method — they use `scalar` /
    /// `str` / `present` by name.
    ///
    /// [`WasmRecordValidator`]: super::WasmRecordValidator
    fn to_query_value(&self) -> QueryValue;
}

// ── ViewFields ────────────────────────────────────────────────────────────────

/// [`RecordFields`] backed by a zero-copy [`RecordView`] lens.
///
/// Name → interned-id resolution is **lazy and point-wise** via
/// [`Interner::get_ind`] — no full de-intern of the record.  Used on the
/// DELETE path where the record is stored as raw msgpack bytes.
pub struct ViewFields<'a> {
    /// The zero-copy msgpack lens.
    pub view: &'a RecordView<'a>,
    /// The repo interner (base-only — DELETE path, all field names in base).
    pub interner: &'a Interner,
}

impl<'a> ViewFields<'a> {
    /// Resolve a string path to an interned-id path, returning `None` if any
    /// segment is unknown to the interner.
    fn resolve_path(
        &self,
        path: &[&str],
    ) -> Option<Vec<shamir_types::core::interner::InternerKey>> {
        path.iter().map(|seg| self.interner.get_ind(*seg)).collect()
    }
}

impl<'a> RecordFields for ViewFields<'a> {
    fn scalar(&self, path: &[&str]) -> Option<ScalarRef<'_>> {
        let ids = self.resolve_path(path)?;
        self.view.scalar_at(&ids)
    }

    fn str(&self, path: &[&str]) -> Option<&str> {
        let ids = self.resolve_path(path)?;
        self.view.str_at(&ids)
    }

    fn present(&self, path: &[&str]) -> Option<Kind> {
        let ids = self.resolve_path(path)?;
        self.view.present_kind_at(&ids)
    }

    fn materialize(&self, path: &[&str]) -> Option<InnerValue> {
        let ids = self.resolve_path(path)?;
        self.view.materialize_at(&ids)
    }

    fn to_query_value(&self) -> QueryValue {
        self.view.to_query_value(self.interner)
    }
}

// ── OwnedFields ───────────────────────────────────────────────────────────────

/// [`RecordFields`] backed by a `QueryValue::Map` (string-keyed, owned form).
///
/// Used transitionally on INSERT/UPDATE paths where the record is already a
/// `QueryValue` (the `resolved_values` produced by `write_exec.rs`).  No
/// interner is needed — map lookups are direct string comparisons.
pub struct OwnedFields<'a> {
    /// The underlying `QueryValue` (should be `QueryValue::Map`).
    pub qv: &'a QueryValue,
}

/// Navigate a [`QueryValue::Map`] hierarchy by string path.
fn qv_descend<'a>(root: &'a QueryValue, path: &[&str]) -> Option<&'a QueryValue> {
    if path.is_empty() {
        return None;
    }
    let mut cur = root;
    for seg in path {
        match cur {
            QueryValue::Map(m) => {
                cur = m.get(*seg)?;
            }
            _ => return None,
        }
    }
    Some(cur)
}

/// Convert a `QueryValue` scalar leaf to a [`ScalarRef`] (mirrors
/// `inner_to_scalar` from `record_ref.rs`).
fn qv_to_scalar(qv: &QueryValue) -> Option<ScalarRef<'_>> {
    match qv {
        QueryValue::Null => Some(ScalarRef::Null),
        QueryValue::Bool(b) => Some(ScalarRef::Bool(*b)),
        QueryValue::Int(i) => Some(ScalarRef::Int(*i)),
        QueryValue::F64(f) => Some(ScalarRef::F64(*f)),
        QueryValue::Str(s) => Some(ScalarRef::Str(s.as_str())),
        QueryValue::Bin(b) => Some(ScalarRef::Bin(b.as_slice())),
        // Dec, Big, List, Set, Map — not scalars in this context.
        _ => None,
    }
}

/// Classify a `QueryValue` into [`Kind`].
fn qv_to_kind(qv: &QueryValue) -> Kind {
    match qv {
        QueryValue::Null => Kind::Null,
        QueryValue::Bool(_)
        | QueryValue::Int(_)
        | QueryValue::F64(_)
        | QueryValue::Str(_)
        | QueryValue::Bin(_) => Kind::Scalar,
        QueryValue::Dec(_) | QueryValue::Big(_) => Kind::NonComparable,
        QueryValue::List(_) | QueryValue::Set(_) | QueryValue::Map(_) => Kind::Container,
    }
}

/// Convert a `QueryValue` leaf to [`InnerValue`] (shallow, without interner —
/// containers map to `InnerValue::Null` because re-interning map keys needs an
/// interner).
fn qv_to_inner(qv: &QueryValue) -> InnerValue {
    match qv {
        QueryValue::Null => InnerValue::Null,
        QueryValue::Bool(b) => InnerValue::Bool(*b),
        QueryValue::Int(i) => InnerValue::Int(*i),
        QueryValue::F64(f) => InnerValue::F64(*f),
        QueryValue::Str(s) => InnerValue::Str(s.clone()),
        QueryValue::Bin(b) => InnerValue::Bin(b.clone()),
        QueryValue::Dec(d) => InnerValue::Dec(*d),
        QueryValue::Big(b) => InnerValue::Big(b.clone()),
        QueryValue::List(items) => InnerValue::List(items.iter().map(qv_to_inner).collect()),
        QueryValue::Set(items) => {
            // Set items are in a BTreeSet<QueryValue>.
            let mut s = shamir_types::types::common::new_set();
            for v in items {
                s.insert(qv_to_inner(v));
            }
            InnerValue::Set(s)
        }
        // Map keys would need an interner — return Null (rare, containers).
        QueryValue::Map(_) => InnerValue::Null,
    }
}

impl<'a> RecordFields for OwnedFields<'a> {
    fn scalar(&self, path: &[&str]) -> Option<ScalarRef<'_>> {
        qv_descend(self.qv, path).and_then(qv_to_scalar)
    }

    fn str(&self, path: &[&str]) -> Option<&str> {
        match qv_descend(self.qv, path) {
            Some(QueryValue::Str(s)) => Some(s.as_str()),
            _ => None,
        }
    }

    fn present(&self, path: &[&str]) -> Option<Kind> {
        qv_descend(self.qv, path).map(qv_to_kind)
    }

    fn materialize(&self, path: &[&str]) -> Option<InnerValue> {
        qv_descend(self.qv, path).map(qv_to_inner)
    }

    fn to_query_value(&self) -> QueryValue {
        self.qv.clone()
    }
}
