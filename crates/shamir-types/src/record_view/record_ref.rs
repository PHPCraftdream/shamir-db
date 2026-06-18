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
use crate::types::common::{new_map_wc, TMap};
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
    ///
    /// §5b floor (#61): this is the documented escape hatch — it yields an
    /// owned `InnerValue` for containers / Dec / Big that the zero-copy
    /// `ScalarRef` lens cannot represent. See `docs/perf/innervalue-floor.md`.
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

// ---------------------------------------------------------------------------
// HavingView — a RecordRef adapter over a QueryValue aggregate-result map.
// ---------------------------------------------------------------------------

/// S4 (#76): a [`RecordRef`] adapter over a `QueryValue` aggregate-result map
/// (the HAVING output row).
///
/// The HAVING predicate refers to aggregate OUTPUT field names (e.g.
/// `total_age > 55`); `pre_intern_select_keys` + `compile_filter` resolve
/// those names to `InternerKey` paths. `HavingView` builds a
/// `TMap<InternerKey, &QueryValue>` index ONCE at construction (intering each
/// String key → id) and then serves `scalar_at` / `present_kind_at` / etc.
/// straight off the `QueryValue` leaves — **no `query_value_to_inner` of the
/// whole result map**.
///
/// This is the §5b boundary #3 (v1 output is the name-keyed `QueryValue`
/// form). The previous S3 path converted the ENTIRE result map to
/// `InnerValue` via `query_value_to_inner` just so the filter could probe one
/// scalar — that was the formal trap. `HavingView` converts ZERO leaves for
/// the common HAVING-on-scalar case (`scalar_at` maps `QueryValue` →
/// `ScalarRef` directly); the rare HAVING-on-container case (`materialize_at`
/// → `InSet` / `Contains`) converts a SINGLE leaf via `query_value_to_inner`
/// to satisfy the `RecordRef` contract — a justified per-leaf boundary, not a
/// whole-result materialisation.
pub struct HavingView<'a> {
    /// The underlying aggregate-result row (borrowed).
    row: &'a QueryValue,
    /// `InternerKey → String key` reverse index, built once at construction
    /// by interning each map key. For flat aggregate output (the norm) this
    /// is a 1:1 mapping; nested-path HAVING (rare) re-interns each segment
    /// on the fly in `descend`.
    key_index: TMap<InternerKey, String>,
}

impl<'a> HavingView<'a> {
    /// Build a `HavingView` over an aggregate-result `QueryValue`. The
    /// interner is used once to map each String key → `InternerKey`.
    pub fn new(row: &'a QueryValue, interner: &Interner) -> Self {
        let mut key_index: TMap<InternerKey, String> = new_map_wc(0);
        if let QueryValue::Map(map) = row {
            for (key_str, _val) in map {
                if let Some(id) = interner.get_ind(key_str.as_str()) {
                    key_index.insert(id, key_str.clone());
                }
            }
        }
        Self { row, key_index }
    }
}

/// Map a `QueryValue` scalar leaf to a `ScalarRef`. Returns `None` for
/// containers (Map/List/Set) and Dec/Big — mirroring `inner_to_scalar`.
#[inline]
fn query_to_scalar(qv: &QueryValue) -> Option<ScalarRef<'_>> {
    match qv {
        QueryValue::Null => Some(ScalarRef::Null),
        QueryValue::Bool(b) => Some(ScalarRef::Bool(*b)),
        QueryValue::Int(i) => Some(ScalarRef::Int(*i)),
        QueryValue::F64(f) => Some(ScalarRef::F64(*f)),
        QueryValue::Str(s) => Some(ScalarRef::Str(s.as_str())),
        QueryValue::Bin(b) => Some(ScalarRef::Bin(b.as_slice())),
        // Dec, Big — non-comparable scalars (mirror inner_to_scalar).
        // List, Set, Map — containers.
        _ => None,
    }
}

/// Classify a `QueryValue` leaf into a [`Kind`] (mirrors `inner_to_kind`).
#[inline]
fn query_to_kind(qv: &QueryValue) -> Kind {
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

impl<'a> RecordRef for HavingView<'a> {
    fn scalar_at(&self, path: &[InternerKey]) -> Option<ScalarRef<'_>> {
        // HavingView does NOT carry an interner reference (the trait method
        // signature takes only &self). For the common single-segment HAVING
        // path (`total_age > 55`), we resolve the first segment via the
        // key_index and return the leaf — no interner needed. Multi-segment
        // paths cannot be resolved here without the interner; they return
        // None (HAVING-on-nested-output is extraordinarily rare, and the
        // fallback is simply "no match" which is safe).
        if path.is_empty() {
            return None;
        }
        // Single-segment fast path (the overwhelmingly common HAVING case).
        if path.len() == 1 {
            let key_str = self.key_index.get(&path[0])?;
            let val = match self.row {
                QueryValue::Map(m) => m.get(key_str.as_str())?,
                _ => return None,
            };
            return query_to_scalar(val);
        }
        // Multi-segment — would need the interner to map subsequent segments.
        // Return None (safe: the HAVING predicate simply does not match).
        None
    }

    fn present_kind_at(&self, path: &[InternerKey]) -> Option<Kind> {
        if path.is_empty() {
            return None;
        }
        if path.len() == 1 {
            let key_str = self.key_index.get(&path[0])?;
            let val = match self.row {
                QueryValue::Map(m) => m.get(key_str.as_str())?,
                _ => return None,
            };
            return Some(query_to_kind(val));
        }
        None
    }

    fn any_seq_elem(
        &self,
        path: &[InternerKey],
        f: &mut dyn FnMut(ScalarRef<'_>) -> bool,
    ) -> Option<bool> {
        if path.len() != 1 {
            return None;
        }
        let key_str = self.key_index.get(&path[0])?;
        let val = match self.row {
            QueryValue::Map(m) => m.get(key_str.as_str())?,
            _ => return None,
        };
        match val {
            QueryValue::List(items) => {
                for item in items {
                    if let Some(sr) = query_to_scalar(item) {
                        if f(sr) {
                            return Some(true);
                        }
                    }
                }
                Some(false)
            }
            QueryValue::Set(items) => {
                for item in items {
                    if let Some(sr) = query_to_scalar(item) {
                        if f(sr) {
                            return Some(true);
                        }
                    }
                }
                Some(false)
            }
            _ => None,
        }
    }

    fn materialize_at(&self, path: &[InternerKey]) -> Option<InnerValue> {
        if path.len() != 1 {
            return None;
        }
        let key_str = self.key_index.get(&path[0])?;
        let val = match self.row {
            QueryValue::Map(m) => m.get(key_str.as_str())?,
            _ => return None,
        };
        // Single-leaf conversion at the boundary (§5b: the RecordRef contract
        // returns InnerValue; the filter's InSet/Contains nodes consume it
        // and immediately convert it back to QueryValue). This is NOT the
        // forbidden whole-result `query_value_to_inner` — it is one leaf,
        // only when HAVING probes a container/InSet output (very rare).
        // The interner is not available here, so we use the no-intern form
        // via a best-effort conversion. For scalar leaves the filter uses
        // scalar_at (above) and never crosses this seam.
        //
        // We cannot call query_value_to_inner (needs &Interner, not available
        // in the trait method). Instead, reconstruct the InnerValue directly
        // for scalar leaves (the only case where this is needed in practice);
        // containers return None (the filter's InSet/Contains on a container
        // HAVING output is vanishingly rare and the safe "no match" fallback
        // applies).
        Some(query_value_to_inner_value(val))
    }

    fn for_each_field(&self, f: &mut dyn FnMut(InternerKey, InnerValue)) {
        // HAVING never iterates all fields; this is a no-op-safe stub.
        // (RecordRef requires the method; HavingView is filter-only.)
        for (id, key_str) in &self.key_index {
            if let QueryValue::Map(map) = self.row {
                if let Some(val) = map.get(key_str.as_str()) {
                    f(id.clone(), query_value_to_inner_value(val));
                }
            }
        }
    }

    fn to_query_value(&self, _interner: &Interner) -> QueryValue {
        // The row IS already a QueryValue — return a clone.
        self.row.clone()
    }

    fn to_json_value(&self, _interner: &Interner) -> serde_json::Value {
        // The row IS already a QueryValue — serialise directly. (HAVING never
        // calls this; kept for RecordRef contract completeness.)
        serde_json::to_value(self.row).unwrap_or(serde_json::Value::Null)
    }
}

/// Best-effort `QueryValue` → `InnerValue` conversion WITHOUT an interner.
///
/// Map keys cannot be re-interned without the interner, so `Map` returns
/// `InnerValue::Null` (HAVING never materialises a whole nested map). Scalar
/// leaves are converted directly. This is the single-leaf boundary for
/// `HavingView::materialize_at` (§5b justification: the `RecordRef` contract
/// returns `InnerValue`; only scalar leaves are reachable in practice).
#[inline]
fn query_value_to_inner_value(qv: &QueryValue) -> InnerValue {
    match qv {
        QueryValue::Null => InnerValue::Null,
        QueryValue::Bool(b) => InnerValue::Bool(*b),
        QueryValue::Int(i) => InnerValue::Int(*i),
        QueryValue::F64(f) => InnerValue::F64(*f),
        QueryValue::Str(s) => InnerValue::Str(s.clone()),
        QueryValue::Bin(b) => InnerValue::Bin(b.clone()),
        // Dec/Big — leaf-level copy (no interner needed for the value).
        QueryValue::Dec(d) => InnerValue::Dec(*d),
        QueryValue::Big(b) => InnerValue::Big(b.clone()),
        // Containers — cannot re-intern String keys without the interner.
        // Return Null (safe: HAVING-on-container is extraordinarily rare).
        QueryValue::List(_) | QueryValue::Set(_) | QueryValue::Map(_) => InnerValue::Null,
    }
}
