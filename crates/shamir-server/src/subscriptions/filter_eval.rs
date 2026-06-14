use std::cmp::Ordering;

use shamir_db::core::interner::Interner;
use shamir_db::types::value::InnerValue;
use shamir_query_types::filter::{Filter, FilterValue};
use tokio::sync::OnceCell;

/// Evaluate a `Filter` directly against an `InnerValue` record.
///
/// `interner_cell` must already be populated (guaranteed when the cell came
/// from `ShamirDb::decode_record_value_inner`); panics in debug builds if
/// the cell is empty (indicates a programming error).
///
/// Field paths are resolved by interning each segment via `interner.get_ind`
/// — a synchronous, lock-free DashMap lookup. Missing keys (field absent or
/// interner miss) are treated as absent (fail-closed on Put-filtered events).
///
/// This replaces the previous `filter_matches_value(&Filter, &serde_json::Value)`
/// path, eliminating the `inner_to_json_value` allocation on the filter-only
/// path. JSON conversion now happens lazily only when the event passes the
/// filter and must be delivered.
pub fn filter_matches_inner(
    filter: &Filter,
    value: &InnerValue,
    interner_cell: &OnceCell<Interner>,
) -> bool {
    // SAFETY: the cell is always populated before being stored in the
    // decode cache (see ShamirDb::decode_record_value_inner).
    let interner = match interner_cell.get() {
        Some(i) => i,
        None => {
            debug_assert!(false, "interner_cell must be populated before filter eval");
            return false;
        }
    };
    filter_matches_inner_with(filter, value, interner)
}

fn filter_matches_inner_with(filter: &Filter, value: &InnerValue, interner: &Interner) -> bool {
    match filter {
        Filter::Eq { field, value: fv } => inner_eq_fv(resolve_inner(value, field, interner), fv),
        Filter::Ne { field, value: fv } => !inner_eq_fv(resolve_inner(value, field, interner), fv),
        Filter::Gt { field, value: fv } => {
            cmp_inner_fv(resolve_inner(value, field, interner), fv) == Some(Ordering::Greater)
        }
        Filter::Gte { field, value: fv } => matches!(
            cmp_inner_fv(resolve_inner(value, field, interner), fv),
            Some(Ordering::Greater | Ordering::Equal)
        ),
        Filter::Lt { field, value: fv } => {
            cmp_inner_fv(resolve_inner(value, field, interner), fv) == Some(Ordering::Less)
        }
        Filter::Lte { field, value: fv } => matches!(
            cmp_inner_fv(resolve_inner(value, field, interner), fv),
            Some(Ordering::Less | Ordering::Equal)
        ),
        Filter::In { field, values } => {
            let resolved = resolve_inner(value, field, interner);
            values.iter().any(|v| inner_eq_fv(resolved, v))
        }
        Filter::NotIn { field, values } => {
            let resolved = resolve_inner(value, field, interner);
            !values.iter().any(|v| inner_eq_fv(resolved, v))
        }
        Filter::IsNull { field } => {
            matches!(
                resolve_inner(value, field, interner),
                None | Some(InnerValue::Null)
            )
        }
        Filter::IsNotNull { field } => !matches!(
            resolve_inner(value, field, interner),
            None | Some(InnerValue::Null)
        ),
        Filter::Exists { field } => resolve_inner(value, field, interner).is_some(),
        Filter::NotExists { field } => resolve_inner(value, field, interner).is_none(),
        Filter::And { filters } => filters
            .iter()
            .all(|f| filter_matches_inner_with(f, value, interner)),
        Filter::Or { filters } => filters
            .iter()
            .any(|f| filter_matches_inner_with(f, value, interner)),
        Filter::Not { filter: f } => !filter_matches_inner_with(f, value, interner),
        // Unsupported variants (FieldRef, QueryRef, FnCall, Expr, Cond, Param)
        // require engine context not available here — fail-closed.
        _ => false,
    }
}

/// Walk a field path in an `InnerValue::Map`, interning each segment
/// synchronously.  Returns `None` when any segment is absent or not interned.
#[inline]
fn resolve_inner<'v>(
    value: &'v InnerValue,
    path: &[String],
    interner: &Interner,
) -> Option<&'v InnerValue> {
    let mut current = value;
    for segment in path {
        let key = interner.get_ind(segment.as_str())?;
        match current {
            InnerValue::Map(map) => {
                current = map.get(&key)?;
            }
            _ => return None,
        }
    }
    Some(current)
}

/// Compare a resolved `InnerValue` against a `FilterValue` literal for equality.
#[inline]
fn inner_eq_fv(inner: Option<&InnerValue>, fv: &FilterValue) -> bool {
    match (inner, fv) {
        (None, FilterValue::Null) => true,
        (None, _) => false,
        (Some(InnerValue::Null), FilterValue::Null) => true,
        (Some(InnerValue::Bool(a)), FilterValue::Bool(b)) => a == b,
        (Some(InnerValue::Int(a)), FilterValue::Int(b)) => a == b,
        (Some(InnerValue::Int(a)), FilterValue::Float(b)) => (*a as f64) == *b,
        (Some(InnerValue::F64(a)), FilterValue::Float(b)) => a == b,
        (Some(InnerValue::F64(a)), FilterValue::Int(b)) => *a == (*b as f64),
        (Some(InnerValue::Str(a)), FilterValue::String(b)) => a == b,
        (Some(InnerValue::Bin(a)), FilterValue::Binary(b)) => a == b,
        _ => false,
    }
}

/// Order a resolved `InnerValue` against a `FilterValue` literal.
fn cmp_inner_fv(inner: Option<&InnerValue>, fv: &FilterValue) -> Option<Ordering> {
    match (inner?, fv) {
        (InnerValue::Int(a), FilterValue::Int(b)) => a.partial_cmp(b),
        (InnerValue::Int(a), FilterValue::Float(b)) => (*a as f64).partial_cmp(b),
        (InnerValue::F64(a), FilterValue::Float(b)) => a.partial_cmp(b),
        (InnerValue::F64(a), FilterValue::Int(b)) => a.partial_cmp(&(*b as f64)),
        (InnerValue::Str(a), FilterValue::String(b)) => Some(a.as_str().cmp(b.as_str())),
        _ => None,
    }
}
