//! ORDER BY execution: QueryValue-native path.

use smallvec::SmallVec;

use crate::query::read::{NullsOrder, OrderBy, OrderByItem, OrderDirection};
use shamir_types::types::value::QueryValue;

// ============================================================================
// QueryValue-based ORDER BY
// ============================================================================

/// Sort `QueryValue` rows by ORDER BY items.
///
/// Uses the canonical-key approach: sort keys are extracted to match the
/// semantics of the pre-J1 ORDER BY exactly — in particular
/// `Dec`/`Big` values are compared as their `to_string()` form
/// (lexicographic, preserving prior coercion semantics), and `Bin`/`Set` map to
/// `Other` (unsortable, preserving insertion order via stable sort).
pub fn apply_order_by_qv(records: &mut Vec<QueryValue>, order_by: &OrderBy) {
    if order_by.items.is_empty() || records.len() <= 1 {
        return;
    }

    // Phase 1: pre-resolve canonical sort keys per record.
    let keys: Vec<QvPreResolvedKeys> = records
        .iter()
        .map(|r| resolve_qv_order_keys(r, &order_by.items))
        .collect();

    // Phase 2: sort index array by pre-resolved keys.
    let mut idx: Vec<usize> = (0..records.len()).collect();
    idx.sort_by(|&a, &b| compare_qv_preresolved(&keys[a], &keys[b], &order_by.items));

    // Phase 3: apply permutation — swap each element into position.
    // We drain the records into a temp vec and pick by index (no Default needed).
    let mut tmp: Vec<Option<QueryValue>> = records.drain(..).map(Some).collect();
    let sorted: Vec<QueryValue> = idx
        .into_iter()
        .map(|i| tmp[i].take().expect("permutation index used twice"))
        .collect();
    *records = sorted;
}

/// Owned sort key for QueryValue fields. Unlike the legacy `SortKey<'a>` this
/// does not borrow from the source records, because `Dec`/`Big` require an
/// owned `String` (the `to_string()` canonical form). Comparison semantics are
/// identical to the former `compare_sort_keys`.
#[derive(Clone)]
enum QvSortKey {
    Null,
    Bool(bool),
    I64(i64),
    F64(f64),
    Str(String),
    Other,
}

impl QvSortKey {
    /// Extract a canonical sort key from a `QueryValue` field reference.
    /// - `Int` -> I64, `F64` -> F64, `Bool` -> Bool, `Str` -> Str (cloned)
    /// - `Dec(d)` -> Str(d.to_string()) -- canonical string form
    /// - `Big(b)` -> Str(b.to_string()) -- canonical string form
    /// - `Null` / missing -> Null
    /// - `Bin`, `Set`, `List`, `Map` -> Other (unsortable)
    fn from_query_value(v: &QueryValue) -> Self {
        match v {
            QueryValue::Null => QvSortKey::Null,
            QueryValue::Bool(b) => QvSortKey::Bool(*b),
            QueryValue::Int(i) => QvSortKey::I64(*i),
            QueryValue::F64(f) => QvSortKey::F64(*f),
            QueryValue::Str(s) => QvSortKey::Str(s.clone()),
            QueryValue::Dec(d) => QvSortKey::Str(d.to_string()),
            QueryValue::Big(b) => QvSortKey::Str(b.to_string()),
            QueryValue::Bin(_) | QueryValue::Set(_) | QueryValue::List(_) | QueryValue::Map(_) => {
                QvSortKey::Other
            }
        }
    }

    #[inline]
    fn is_null(&self) -> bool {
        matches!(self, QvSortKey::Null)
    }
}

type QvPreResolvedKeys = SmallVec<[QvSortKey; 4]>;

/// Get a field from a `QueryValue::Map` by path segments.
fn get_query_value_field<'a>(value: &'a QueryValue, path: &[String]) -> Option<&'a QueryValue> {
    let mut current = value;
    for part in path {
        match current {
            QueryValue::Map(m) => {
                current = m.get(part.as_str())?;
            }
            _ => return None,
        }
    }
    Some(current)
}

/// Pre-resolve all ORDER BY field values from a single QueryValue record.
fn resolve_qv_order_keys(record: &QueryValue, items: &[OrderByItem]) -> QvPreResolvedKeys {
    items
        .iter()
        .map(|item| {
            let field = get_query_value_field(record, &item.field);
            match field {
                Some(v) => QvSortKey::from_query_value(v),
                None => QvSortKey::Null,
            }
        })
        .collect()
}

/// Compare two pre-resolved QvSortKey vectors.
fn compare_qv_preresolved(
    a: &QvPreResolvedKeys,
    b: &QvPreResolvedKeys,
    items: &[OrderByItem],
) -> std::cmp::Ordering {
    for (i, item) in items.iter().enumerate() {
        let ord = compare_qv_sort_keys(&a[i], &b[i], &item.direction, &item.nulls);
        if ord != std::cmp::Ordering::Equal {
            return ord;
        }
    }
    std::cmp::Ordering::Equal
}

/// Compare two QvSortKey values with direction + nulls handling.
#[inline]
fn compare_qv_sort_keys(
    a: &QvSortKey,
    b: &QvSortKey,
    direction: &OrderDirection,
    nulls: &Option<NullsOrder>,
) -> std::cmp::Ordering {
    let is_null_a = a.is_null();
    let is_null_b = b.is_null();
    if is_null_a && is_null_b {
        return std::cmp::Ordering::Equal;
    }
    if is_null_a || is_null_b {
        let nulls_order = nulls.unwrap_or(match direction {
            OrderDirection::Asc => NullsOrder::Last,
            OrderDirection::Desc => NullsOrder::First,
        });
        let null_first = matches!(nulls_order, NullsOrder::First);
        return if is_null_a == null_first {
            std::cmp::Ordering::Less
        } else {
            std::cmp::Ordering::Greater
        };
    }

    let base = match (a, b) {
        (QvSortKey::I64(x), QvSortKey::I64(y)) => x.cmp(y),
        (QvSortKey::F64(x), QvSortKey::F64(y)) => {
            x.partial_cmp(y).unwrap_or(std::cmp::Ordering::Equal)
        }
        (QvSortKey::I64(x), QvSortKey::F64(y)) => (*x as f64)
            .partial_cmp(y)
            .unwrap_or(std::cmp::Ordering::Equal),
        (QvSortKey::F64(x), QvSortKey::I64(y)) => x
            .partial_cmp(&(*y as f64))
            .unwrap_or(std::cmp::Ordering::Equal),
        (QvSortKey::Str(x), QvSortKey::Str(y)) => x.cmp(y),
        (QvSortKey::Bool(x), QvSortKey::Bool(y)) => x.cmp(y),
        _ => std::cmp::Ordering::Equal,
    };

    match direction {
        OrderDirection::Asc => base,
        OrderDirection::Desc => base.reverse(),
    }
}
