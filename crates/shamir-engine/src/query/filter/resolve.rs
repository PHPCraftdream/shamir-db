use std::cmp::Ordering;

use shamir_types::codecs::interned::{inner_value_to_query_value, query_value_to_inner};
use shamir_types::core::interner::{Interner, InternerKey};
use shamir_types::record_view::RecordRef;
use shamir_types::types::value::{InnerValue, QueryValue, Value};
use smallvec::SmallVec;

use super::eval_context::FilterContext;
use super::filter_node::CompactPath;
use crate::query::filter::FilterValue;
use crate::query::read::QueryResult;

/// Extract a value from an InnerValue by a path of interned keys.
///
/// Borrowing variant: walks the record in-place without cloning the
/// resolved leaf. All filter nodes below use this — the old owned
/// variant survives only for callers outside the eval module that
/// still rely on `Option<InnerValue>`.
#[inline]
pub fn resolve_field_ref<'a>(record: &'a InnerValue, path: &[u64]) -> Option<&'a InnerValue> {
    let mut cur = record;
    for &id in path {
        match cur {
            InnerValue::Map(map) => {
                let key = InternerKey::new(id);
                cur = map.get(&key)?;
            }
            _ => return None,
        }
    }
    Some(cur)
}

/// Owned variant — pinned by `table/read_exec.rs` (two field-resolution
/// call-sites) plus unit tests. Hot filter paths use `resolve_field_ref`
/// and never call this. §5b floor: eliminable only when read_exec moves
/// those sites to the lens.
pub fn resolve_field(record: &InnerValue, path: &[u64]) -> Option<InnerValue> {
    resolve_field_ref(record, path).cloned()
}

/// Resolve a field path (segments) into interned u64 keys.
pub fn intern_field_path(field: &[String], interner: &Interner) -> Option<Vec<u64>> {
    let mut keys = Vec::with_capacity(field.len());
    for part in field {
        let interned = interner.get_ind(part)?;
        keys.push(interned.id());
    }
    Some(keys)
}

/// Inline-allocated variant of [`intern_field_path`] for hot compile paths.
///
/// Returns a `CompactPath` (`SmallVec<[u64; 4]>`) so `FilterNode` callers
/// that store the result in `field_path` avoid a heap allocation for the
/// typical 1-3 segment case.
pub(super) fn intern_field_path_compact(
    field: &[String],
    interner: &Interner,
) -> Option<CompactPath> {
    let mut keys: CompactPath = SmallVec::with_capacity(field.len());
    for part in field {
        let interned = interner.get_ind(part)?;
        keys.push(interned.id());
    }
    Some(keys)
}

/// Compare two `Value<K>` scalars. Returns an `Ordering` if comparable.
///
/// C6 (#80): generic over the key type. Only the scalar arms
/// (Null/Bool/Int/F64/Str) participate, and those are key-agnostic — so this
/// serves BOTH the name-keyed filter path (`QueryValue`) and the id-keyed
/// aggregator path (`InnerValue`, until S4) with ZERO conversion at either
/// call site (anti-formal: no inner↔query bridge added). The ordering is
/// byte-identical to the previous `InnerValue`-only form.
#[inline]
pub fn compare_values<K>(a: &Value<K>, b: &Value<K>) -> Option<Ordering>
where
    K: Eq + std::hash::Hash + Ord + Clone + serde::Serialize + std::fmt::Debug,
{
    match (a, b) {
        (Value::Null, Value::Null) => Some(Ordering::Equal),
        (Value::Bool(a), Value::Bool(b)) => Some(a.cmp(b)),
        (Value::Int(a), Value::Int(b)) => Some(a.cmp(b)),
        (Value::Int(a), Value::F64(b)) => (*a as f64).partial_cmp(b),
        (Value::F64(a), Value::Int(b)) => a.partial_cmp(&(*b as f64)),
        (Value::F64(a), Value::F64(b)) => a.partial_cmp(b),
        (Value::Str(a), Value::Str(b)) => Some(a.cmp(b)),
        _ => None,
    }
}

/// Convert a literal `FilterValue` to `QueryValue` without record/context.
///
/// C6 (#80): the name-keyed analogue of the legacy `filter_value_to_inner`.
/// Returns `None` for non-literal variants (FieldRef, QueryRef, FnCall,
/// Expr, Cond, Param, Array).
#[inline]
pub fn filter_value_to_query(fv: &FilterValue) -> Option<QueryValue> {
    match fv {
        FilterValue::Null => Some(QueryValue::Null),
        FilterValue::Bool(b) => Some(QueryValue::Bool(*b)),
        FilterValue::Int(i) => Some(QueryValue::Int(*i)),
        FilterValue::Float(f) => Some(QueryValue::F64(*f)),
        FilterValue::String(s) => Some(QueryValue::Str(s.clone())),
        FilterValue::Binary(b) => Some(QueryValue::Bin(b.clone())),
        _ => None,
    }
}

/// Convert a literal `FilterValue` to `InnerValue` without record/context.
///
/// **Legacy adapter** — kept for out-of-scope callers that still bind to
/// `InnerValue`. New filter code uses [`filter_value_to_query`] (name-keyed).
///
/// Returns `None` for non-literal variants (FieldRef, QueryRef, FnCall, Expr, Cond).
///
/// §5b floor: pinned by `table/read_planner.rs`, which encodes filter
/// literals into InnerValue index-bound keys. Eliminable only via the
/// index key-encoding boundary — not in resolve.rs scope.
#[inline]
pub fn filter_value_to_inner(fv: &FilterValue) -> Option<InnerValue> {
    match fv {
        FilterValue::Null => Some(InnerValue::Null),
        FilterValue::Bool(b) => Some(InnerValue::Bool(*b)),
        FilterValue::Int(i) => Some(InnerValue::Int(*i)),
        FilterValue::Float(f) => Some(InnerValue::F64(*f)),
        FilterValue::String(s) => Some(InnerValue::Str(s.clone())),
        FilterValue::Binary(b) => Some(InnerValue::Bin(b.clone())),
        _ => None,
    }
}

/// Resolve a `FilterValue` into a **`QueryValue`** for comparison.
///
/// C6 (#80) — this is the name-keyed hot-path resolver. It retires the
/// transient `inner→query→funclib→query→inner` round-trips that the
/// funclib ABI flip (#75) created on the filter-eval path:
///
/// - **literals** (`Null`/`Bool`/`Int`/`Float`/`String`/`Binary`) — built
///   directly as `QueryValue`.
/// - **`FieldRef`** — `record.materialize_at` still yields `InnerValue`
///   today (narrowing that lens output is a LATER stage, out of scope for
///   C6). We convert **once** at this boundary via
///   `inner_value_to_query_value`. This single lens→QueryValue
///   materialization *replaces* the old lens→InnerValue one (it is not a
///   net-new round-trip); everything downstream stays `QueryValue`.
/// - **`FnCall`** — arguments are now `QueryValue`, passed straight to
///   `ctx.scalars.call` (no `inner_value_to_query_value` on the way in);
///   the funclib result is already `QueryValue` and is returned directly
///   (no `query_value_to_inner` on the way out). **The round-trip is gone.**
/// - **`QueryRef`** / **`Param`** — return `QueryValue` directly.
///
/// Any failure (unresolvable arg, unknown function, arity/type/conversion
/// error) collapses to `None` so the comparison treats the value as absent.
pub fn resolve_filter_query(
    fv: &FilterValue,
    record: &(impl RecordRef + ?Sized),
    ctx: &FilterContext,
) -> Option<QueryValue> {
    match fv {
        FilterValue::Null => Some(QueryValue::Null),
        FilterValue::Bool(b) => Some(QueryValue::Bool(*b)),
        FilterValue::Int(i) => Some(QueryValue::Int(*i)),
        FilterValue::Float(f) => Some(QueryValue::F64(*f)),
        FilterValue::String(s) => Some(QueryValue::Str(s.clone())),
        FilterValue::Binary(b) => Some(QueryValue::Bin(b.clone())),
        FilterValue::FieldRef { path } => {
            let keys = intern_field_path(path, ctx.interner)?;
            let ipath: SmallVec<[InternerKey; 4]> =
                keys.iter().map(|&id| InternerKey::new(id)).collect();
            // Single lens→QueryValue boundary (replaces the old lens→InnerValue
            // materialization). NOT a net-new round-trip — see fn doc.
            record
                .materialize_at(&ipath)
                .and_then(|iv| inner_value_to_query_value(&iv, ctx.interner).ok())
        }
        FilterValue::QueryRef { alias, path } => {
            let key = alias.strip_prefix('@').unwrap_or(alias.as_str());
            let qr = ctx.resolved_refs.get(key)?;
            resolve_query_ref_value(qr, path.as_deref())
        }
        FilterValue::FnCall { call } => {
            // Args are QueryValue → straight to funclib. Result is QueryValue
            // → returned directly. Zero InnerValue, zero round-trip.
            let mut qv_args = Vec::with_capacity(call.args().len());
            for a in call.args() {
                let qv = resolve_filter_query(a, record, ctx)?;
                qv_args.push(qv);
            }
            ctx.scalars.call(call.name(), &qv_args).ok()
        }
        FilterValue::Param { name } => {
            // Injected sub-batch parameter. Populated by the recursive
            // sub-batch executor (P3); empty at the top level. The param
            // scope is name-keyed (`QueryValue`) — returned directly, no conversion.
            ctx.params.get(name.as_str()).cloned()
        }
        _ => None,
    }
}

/// Resolve a `FilterValue` into an `InnerValue` for comparison.
///
/// **Legacy adapter (C6 #80).** After E6, the only remaining caller is the
/// cold projection twin (`SelectProjection::project` in
/// `query/read/select_projection.rs`), which feeds `inner_to_query_value`.
/// The hot QueryValue paths (aggregate scalar-fn, `project_value`) were
/// migrated to [`resolve_filter_query`] directly (E6). This entry delegates
/// to [`resolve_filter_query`] (the name-keyed hot path) and performs a
/// single trailing `query_value_to_inner` conversion at the legacy boundary
/// — a documented cold adapter, NOT a hot-path round-trip. The internal
/// filter eval tree (`FilterNode::matches`) uses `resolve_filter_query`
/// directly and never crosses this seam.
///
/// §5b floor: survives only for the legacy projection twin; dies with the
/// InnerValue axis — not reducible within resolve.rs scope.
pub fn resolve_filter_value(
    fv: &FilterValue,
    record: &(impl RecordRef + ?Sized),
    ctx: &FilterContext,
) -> Option<InnerValue> {
    let qv = resolve_filter_query(fv, record, ctx)?;
    query_value_to_inner(&qv, ctx.interner).ok()
}

/// Extract a value from a QueryResult by a simple path like "[0].id".
///
/// **Phase 2 — Call-aware**: when the `QueryResult` carries a
/// `value` (a stored-procedure / `BatchOp::Call` return — object /
/// array / scalar), the path is applied to that `value` instead of
/// to the records array. This lets later batch ops reference a Call's
/// result with the same `$query` syntax used for Read results:
///
/// - `@proc`              → entire `value` (scalar / object / array).
/// - `@proc.id`           → object field.
/// - `@proc[0]`           → array index.
/// - `@proc[0].name`      → chained.
///
/// For ordinary Read results (`value` is `None`), the records-based
/// behaviour is preserved unchanged.
///
/// C6 (#80): returns `QueryValue` (name-keyed) — the filter comparison
/// layer is now QueryValue-native.
pub(super) fn resolve_query_ref_value(qr: &QueryResult, path: Option<&str>) -> Option<QueryValue> {
    // Call-result path: source is `QueryResult.value`.
    if let Some(value) = &qr.value {
        return resolve_query_value_path(value, path).cloned();
    }

    // Read-result path: source is `QueryResult.records`.
    let path = path?;
    if !path.starts_with('[') {
        return None;
    }
    let bracket_end = path.find(']')?;
    let index: usize = path[1..bracket_end].parse().ok()?;
    let record = qr.records.get(index)?;

    let rest = &path[bracket_end + 1..];
    let record_qv = record.as_value();
    if rest.is_empty() {
        return Some(record_qv.into_owned());
    }
    let rest = rest.strip_prefix('.')?;
    Some(record_qv.get(rest)?.clone())
}

/// Extract a column of values from all records in a QueryResult.
///
/// Supports `[].field` pattern — iterates all records, extracts `field` from each.
///
/// C6 (#80): returns `Vec<QueryValue>` (name-keyed).
pub(super) fn resolve_query_ref_column(qr: &QueryResult, path: Option<&str>) -> Vec<QueryValue> {
    let path = match path {
        Some(p) => p,
        None => return Vec::new(),
    };
    if !path.starts_with("[]") {
        return Vec::new();
    }
    let rest = &path[2..];
    let field = match rest.strip_prefix('.') {
        Some(f) => f,
        None => return Vec::new(),
    };

    qr.records
        .iter()
        .filter_map(|record| {
            let record_qv = record.as_value();
            Some(record_qv.get(field)?.clone())
        })
        .collect()
}

/// Walk a path like `.field`, `[0]`, `[0].name`, or `None` (root) through a
/// `QueryValue`. Used by [`resolve_query_ref_value`] when the source is a
/// Call result (`QueryResult.value`).
///
/// Supported segments:
/// - `.field`     → Map field access.
/// - `[n]`        → List index.
/// - `[n].field`  → chained.
///
/// The path is intentionally a subset of the full `QueryReference` grammar —
/// it is what the `QueryRef.path` string carries in practice for Call refs.
pub(super) fn resolve_query_value_path<'a>(
    mut cur: &'a QueryValue,
    path: Option<&str>,
) -> Option<&'a QueryValue> {
    // Preserve the original semantics: a `None` path returns the root
    // value itself (Some(cur)), not None.
    let mut rest = match path {
        Some(p) => p,
        None => return Some(cur),
    };
    while !rest.is_empty() {
        if let Some(after_dot) = rest.strip_prefix('.') {
            let end = after_dot.find(['.', '[']).unwrap_or(after_dot.len());
            let field = &after_dot[..end];
            cur = match cur {
                QueryValue::Map(m) => m.get(field)?,
                _ => return None,
            };
            rest = &after_dot[end..];
        } else if rest.starts_with('[') {
            let bracket_end = rest.find(']')?;
            let idx: usize = rest[1..bracket_end].parse().ok()?;
            cur = match cur {
                QueryValue::List(l) => l.get(idx)?,
                _ => return None,
            };
            rest = &rest[bracket_end + 1..];
        } else {
            return None;
        }
    }
    Some(cur)
}

pub(super) fn is_column_query_ref(fv: &FilterValue) -> bool {
    matches!(fv, FilterValue::QueryRef { path: Some(p), .. } if p.starts_with("[]"))
}
