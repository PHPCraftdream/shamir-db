use std::cmp::Ordering;

use shamir_types::core::interner::{Interner, InternerKey};
use shamir_types::types::value::InnerValue;
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

/// Owned variant — kept for external callers (tests, query/read/exec.rs).
/// Hot filter paths use `resolve_field_ref` and never call this.
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

/// Compare two InnerValue instances. Returns an Ordering if comparable.
#[inline]
pub fn compare_values(a: &InnerValue, b: &InnerValue) -> Option<Ordering> {
    match (a, b) {
        (InnerValue::Null, InnerValue::Null) => Some(Ordering::Equal),
        (InnerValue::Bool(a), InnerValue::Bool(b)) => Some(a.cmp(b)),
        (InnerValue::Int(a), InnerValue::Int(b)) => Some(a.cmp(b)),
        (InnerValue::Int(a), InnerValue::F64(b)) => (*a as f64).partial_cmp(b),
        (InnerValue::F64(a), InnerValue::Int(b)) => a.partial_cmp(&(*b as f64)),
        (InnerValue::F64(a), InnerValue::F64(b)) => a.partial_cmp(b),
        (InnerValue::Str(a), InnerValue::Str(b)) => Some(a.cmp(b)),
        _ => None,
    }
}

/// Convert a literal FilterValue to InnerValue without record/context.
///
/// Returns `None` for non-literal variants (FieldRef, QueryRef, FnCall, Expr, Cond).
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

/// Resolve a FilterValue into an InnerValue for comparison.
///
/// Public so the SELECT projection (`query::read::exec`) can evaluate
/// scalar-function select items against a record with the same semantics as
/// filter values (`$ref` / literals / `$fn`).
pub fn resolve_filter_value(
    fv: &FilterValue,
    record: &InnerValue,
    ctx: &FilterContext,
) -> Option<InnerValue> {
    match fv {
        FilterValue::Null => Some(InnerValue::Null),
        FilterValue::Bool(b) => Some(InnerValue::Bool(*b)),
        FilterValue::Int(i) => Some(InnerValue::Int(*i)),
        FilterValue::Float(f) => Some(InnerValue::F64(*f)),
        FilterValue::String(s) => Some(InnerValue::Str(s.clone())),
        FilterValue::Binary(b) => Some(InnerValue::Bin(b.clone())),
        FilterValue::FieldRef { path } => {
            let keys = intern_field_path(path, ctx.interner)?;
            resolve_field(record, &keys)
        }
        FilterValue::QueryRef { alias, path } => {
            let key = alias.strip_prefix('@').unwrap_or(alias.as_str());
            let qr = ctx.resolved_refs.get(key)?;
            resolve_query_ref_value(qr, path.as_deref())
        }
        FilterValue::FnCall { call } => {
            // Resolve each argument (literal / FieldRef / nested FnCall) into an
            // InnerValue, then dispatch by folder-qualified name through the
            // scalar registry. Any failure (unresolvable arg, unknown function,
            // arity / type error) collapses to `None` so the comparison treats
            // the value as absent rather than panicking.
            let mut args = Vec::with_capacity(call.args().len());
            for a in call.args() {
                args.push(resolve_filter_value(a, record, ctx)?);
            }
            ctx.scalars.call(call.name(), &args).ok()
        }
        FilterValue::Param { name } => {
            // Injected sub-batch parameter. Populated by the recursive
            // sub-batch executor (P3); empty at the top level.
            ctx.params.get(name.as_str()).cloned()
        }
        _ => None,
    }
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
pub(super) fn resolve_query_ref_value(qr: &QueryResult, path: Option<&str>) -> Option<InnerValue> {
    // Call-result path: source is `QueryResult.value`.
    if let Some(value) = &qr.value {
        return resolve_json_path(value, path).and_then(json_to_inner_value);
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
    let record_json = record.as_json();
    if rest.is_empty() {
        return json_to_inner_value(&record_json);
    }
    let rest = rest.strip_prefix('.')?;
    let field_val = record_json.get(rest)?;
    json_to_inner_value(field_val)
}

/// Extract a column of values from all records in a QueryResult.
///
/// Supports `[].field` pattern — iterates all records, extracts `field` from each.
pub(super) fn resolve_query_ref_column(qr: &QueryResult, path: Option<&str>) -> Vec<InnerValue> {
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
            let record_json = record.as_json();
            let val = record_json.get(field)?;
            json_to_inner_value(val)
        })
        .collect()
}

/// Walk a path like `.field`, `[0]`, `[0].name`, or `None` (root) through a
/// `serde_json::Value`. Used by [`resolve_query_ref_value`] when the source
/// is a Call result (`QueryResult.value`).
///
/// Supported segments:
/// - `.field`     → object field access.
/// - `[n]`        → array index.
/// - `[n].field`  → chained.
///
/// The path is intentionally a subset of the full `QueryReference` grammar —
/// it is what the `QueryRef.path` string carries in practice for Call refs.
pub(super) fn resolve_json_path<'a>(
    mut cur: &'a serde_json::Value,
    path: Option<&str>,
) -> Option<&'a serde_json::Value> {
    let Some(path) = path else {
        return Some(cur);
    };
    let mut rest = path;
    while !rest.is_empty() {
        if let Some(after_dot) = rest.strip_prefix('.') {
            let end = after_dot.find(['.', '[']).unwrap_or(after_dot.len());
            let field = &after_dot[..end];
            cur = cur.get(field)?;
            rest = &after_dot[end..];
        } else if rest.starts_with('[') {
            let bracket_end = rest.find(']')?;
            let idx: usize = rest[1..bracket_end].parse().ok()?;
            cur = cur.get(idx)?;
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

pub(super) fn json_to_inner_value(v: &serde_json::Value) -> Option<InnerValue> {
    match v {
        serde_json::Value::Null => Some(InnerValue::Null),
        serde_json::Value::Bool(b) => Some(InnerValue::Bool(*b)),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Some(InnerValue::Int(i))
            } else {
                n.as_f64().map(InnerValue::F64)
            }
        }
        serde_json::Value::String(s) => Some(InnerValue::Str(s.clone())),
        _ => None,
    }
}
