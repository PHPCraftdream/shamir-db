//! ORDER BY execution: columnar fast path + general enum path.

use serde_json as json;
use smallvec::SmallVec;

use crate::query::read::{NullsOrder, OrderBy, OrderByItem, OrderDirection};

// ============================================================================
// Public entry point
// ============================================================================

/// Sort JSON objects by ORDER BY items.
///
/// Pre-resolves field values once per record (O(n) linear scan), then
/// sorts an index array by those pre-resolved references.  This avoids
/// repeated `Value::get` lookups inside the comparator — the dominant
/// cost identified in bench #106 (~85% of ORDER BY time).
///
/// Fast path: for single-column ORDER BY with a homogeneous typed column,
/// `try_columnar_sort` extracts values into a typed `Vec<T>` (i64/f64/bool/
/// &str) and sorts a plain index with one native `cmp` — no enum-tag match,
/// no SmallVec.  Falls back to the enum path if the column is heterogeneous
/// or uses a non-scalar type.
pub fn apply_order_by(records: &mut Vec<json::Value>, order_by: &OrderBy) {
    if order_by.items.is_empty() || records.len() <= 1 {
        return;
    }

    // Fast path: single-column with a homogeneous typed column.
    // Falls through to the general enum path if the column is heterogeneous.
    if order_by.items.len() == 1 && try_columnar_sort(records, &order_by.items[0]) {
        return;
    }

    // Phase 1: pre-resolve field values — one linear pass.
    // String keys carry a borrowed `&str` into the source records
    // (zero-copy); the borrow lives only across phases 1-2, before the
    // mutable permutation in phase 3. This skips ~n heap allocations
    // and ~n memcpy of every string sort key vs the previous `Box<str>`
    // form (measurable on 100k-row string sorts).
    let keys: Vec<PreResolvedKeys<'_>> = records
        .iter()
        .map(|r| resolve_order_keys(r, &order_by.items))
        .collect();

    // Phase 2: sort index array by pre-resolved keys.
    let mut idx: Vec<usize> = (0..records.len()).collect();
    idx.sort_by(|&a, &b| compare_preresolved(&keys[a], &keys[b], &order_by.items));

    // Drop borrowed keys before the mutable permutation pass so the
    // compiler-enforced shared borrow of `records` ends here.
    drop(keys);

    // Phase 3: apply permutation in-place.
    let sorted: Vec<json::Value> = idx
        .into_iter()
        .map(|i| std::mem::take(&mut records[i]))
        .collect();
    *records = sorted;
}

// ============================================================================
// Columnar fast path (single-column ORDER BY)
// ============================================================================

/// Typed column buffer — one variant per scalar type we can extract.
/// The `Str` variant borrows from the source `records` slice; the borrow
/// is released before the mutable permutation in phase 3 (same pattern as
/// `PreResolvedKeys<'_>` in the general path).
pub(super) enum ColBuf<'a> {
    I64(Vec<i64>),
    F64(Vec<f64>),
    Bool(Vec<bool>),
    Str(Vec<&'a str>),
}

/// Null-placement helper: `null_first` semantics for the columnar path
/// (mirrors `compare_sort_keys` null logic).
#[inline]
pub(super) fn null_is_first(direction: OrderDirection, nulls: Option<NullsOrder>) -> bool {
    let order = nulls.unwrap_or(match direction {
        OrderDirection::Asc => NullsOrder::Last,
        OrderDirection::Desc => NullsOrder::First,
    });
    matches!(order, NullsOrder::First)
}

/// Try to sort `records` using a typed columnar buffer for `item`.
///
/// Returns `true` if the sort was performed (column is homogeneous scalar);
/// returns `false` if the column is heterogeneous or non-scalar, in which case
/// the caller must fall back to the general enum path.
///
/// Null-handling and direction semantics exactly match `compare_sort_keys`.
pub(super) fn try_columnar_sort(records: &mut Vec<json::Value>, item: &OrderByItem) -> bool {
    let n = records.len();
    let mut is_null: Vec<bool> = Vec::with_capacity(n);

    // Phase 1: probe type from the first non-null value, then extract the
    // whole column.  Any type mismatch → abort and return false.
    //
    // The shared borrow of `records` is held only during phases 1 and 2.
    // We drop it (via `drop(col_buf)`) before the mutable phase 3.
    let col_buf: ColBuf<'_> = {
        // SAFETY: we borrow `records` immutably here; the &str values live
        // inside `json::Value::String` heap-allocated strings that are not
        // moved until phase 3 (after we drop `col_buf`).  This matches the
        // existing `PreResolvedKeys<'_>` lifetime pattern in `apply_order_by`.
        let records_ref: &Vec<json::Value> = records;

        // Find first non-null to determine column type.
        let mut probed_type: Option<u8> = None; // 0=i64, 1=f64, 2=bool, 3=str
        for rec in records_ref.iter() {
            let v = get_json_field(rec, &item.field);
            match v {
                None | Some(json::Value::Null) => continue,
                Some(json::Value::Number(n)) => {
                    if n.as_i64().is_some() {
                        probed_type = Some(0);
                    } else if n.as_f64().is_some() {
                        probed_type = Some(1);
                    } else {
                        return false;
                    }
                }
                Some(json::Value::Bool(_)) => {
                    probed_type = Some(2);
                }
                Some(json::Value::String(_)) => {
                    probed_type = Some(3);
                }
                _ => return false, // Array/Object — abort
            }
            break;
        }

        let pt = match probed_type {
            Some(t) => t,
            None => {
                // All values are null — already "sorted", nothing to do.
                return true;
            }
        };

        match pt {
            0 => {
                // i64 column
                let mut col: Vec<i64> = Vec::with_capacity(n);
                for rec in records_ref.iter() {
                    let v = get_json_field(rec, &item.field);
                    match v {
                        None | Some(json::Value::Null) => {
                            is_null.push(true);
                            col.push(0);
                        }
                        Some(json::Value::Number(num)) => {
                            if let Some(i) = num.as_i64() {
                                is_null.push(false);
                                col.push(i);
                            } else {
                                return false; // mixed i64/f64 → abort
                            }
                        }
                        _ => return false,
                    }
                }
                ColBuf::I64(col)
            }
            1 => {
                // f64 column
                let mut col: Vec<f64> = Vec::with_capacity(n);
                for rec in records_ref.iter() {
                    let v = get_json_field(rec, &item.field);
                    match v {
                        None | Some(json::Value::Null) => {
                            is_null.push(true);
                            col.push(0.0);
                        }
                        Some(json::Value::Number(num)) => {
                            if let Some(f) = num.as_f64() {
                                is_null.push(false);
                                col.push(f);
                            } else {
                                return false;
                            }
                        }
                        _ => return false,
                    }
                }
                ColBuf::F64(col)
            }
            2 => {
                // bool column
                let mut col: Vec<bool> = Vec::with_capacity(n);
                for rec in records_ref.iter() {
                    let v = get_json_field(rec, &item.field);
                    match v {
                        None | Some(json::Value::Null) => {
                            is_null.push(true);
                            col.push(false);
                        }
                        Some(json::Value::Bool(b)) => {
                            is_null.push(false);
                            col.push(*b);
                        }
                        _ => return false,
                    }
                }
                ColBuf::Bool(col)
            }
            _ => {
                // str column
                let mut col: Vec<&str> = Vec::with_capacity(n);
                for rec in records_ref.iter() {
                    let v = get_json_field(rec, &item.field);
                    match v {
                        None | Some(json::Value::Null) => {
                            is_null.push(true);
                            col.push("");
                        }
                        Some(json::Value::String(s)) => {
                            is_null.push(false);
                            col.push(s.as_str());
                        }
                        _ => return false,
                    }
                }
                ColBuf::Str(col)
            }
        }
    };

    // Phase 2: sort index by typed column + null/direction handling.
    let null_first = null_is_first(item.direction, item.nulls);
    let desc = matches!(item.direction, OrderDirection::Desc);

    let mut idx: Vec<usize> = (0..n).collect();
    idx.sort_by(|&a, &b| {
        let na = is_null[a];
        let nb = is_null[b];
        if na && nb {
            return std::cmp::Ordering::Equal;
        }
        if na || nb {
            return if na == null_first {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Greater
            };
        }
        let base = match &col_buf {
            ColBuf::I64(col) => col[a].cmp(&col[b]),
            ColBuf::F64(col) => col[a]
                .partial_cmp(&col[b])
                .unwrap_or(std::cmp::Ordering::Equal),
            ColBuf::Bool(col) => col[a].cmp(&col[b]),
            ColBuf::Str(col) => col[a].cmp(col[b]),
        };
        if desc {
            base.reverse()
        } else {
            base
        }
    });

    // Release the shared borrow (col_buf holds &str refs into records).
    drop(col_buf);

    // Phase 3: apply permutation in-place (identical to the general path).
    let sorted: Vec<json::Value> = idx
        .into_iter()
        .map(|i| std::mem::take(&mut records[i]))
        .collect();
    *records = sorted;

    true
}

/// Pre-resolved field values for all ORDER BY fields of one record.
/// SmallVec<[…; 4]> avoids heap allocation for the common ≤4 field case.
type PreResolvedKeys<'a> = SmallVec<[SortKey<'a>; 4]>;

/// Typed pre-resolved ORDER BY field value. The comparator dispatches on
/// the enum variant once and then compares native types (i64::cmp,
/// str::cmp, etc) — bypassing the per-comparison `serde_json::Value`
/// match that dominated the original `apply_order_by`.
///
/// String variant borrows from the source record to avoid one heap
/// allocation + memcpy per row (the source `json::Value`s outlive the
/// `keys` vector — see `apply_order_by`).
#[derive(Clone, Copy)]
pub(super) enum SortKey<'a> {
    Null,
    Bool(bool),
    I64(i64),
    F64(f64),
    Str(&'a str),
    Other, // unsupported (Array / Object) — falls back to Equal
}

impl<'a> SortKey<'a> {
    pub(super) fn from_json(v: Option<&'a json::Value>) -> Self {
        match v {
            None | Some(json::Value::Null) => SortKey::Null,
            Some(json::Value::Bool(b)) => SortKey::Bool(*b),
            Some(json::Value::Number(n)) => {
                if let Some(i) = n.as_i64() {
                    SortKey::I64(i)
                } else if let Some(f) = n.as_f64() {
                    SortKey::F64(f)
                } else {
                    SortKey::Null
                }
            }
            Some(json::Value::String(s)) => SortKey::Str(s.as_str()),
            _ => SortKey::Other,
        }
    }

    #[inline]
    pub(super) fn is_null(&self) -> bool {
        matches!(self, SortKey::Null)
    }
}

/// Pre-resolve all ORDER BY field values from a single JSON record.
pub(super) fn resolve_order_keys<'a>(
    record: &'a json::Value,
    items: &[OrderByItem],
) -> PreResolvedKeys<'a> {
    items
        .iter()
        .map(|item| SortKey::from_json(get_json_field(record, &item.field)))
        .collect()
}

/// Compare two pre-resolved key vectors.
pub(super) fn compare_preresolved(
    a: &PreResolvedKeys<'_>,
    b: &PreResolvedKeys<'_>,
    items: &[OrderByItem],
) -> std::cmp::Ordering {
    for (i, item) in items.iter().enumerate() {
        let ord = compare_sort_keys(&a[i], &b[i], &item.direction, &item.nulls);
        if ord != std::cmp::Ordering::Equal {
            return ord;
        }
    }
    std::cmp::Ordering::Equal
}

/// Compare two pre-resolved sort keys with direction + nulls handling.
/// Mirrors `compare_json_values` semantics but dispatches on the typed
/// enum once instead of matching `serde_json::Value` on every comparison.
#[inline]
pub(super) fn compare_sort_keys(
    a: &SortKey<'_>,
    b: &SortKey<'_>,
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
        (SortKey::I64(x), SortKey::I64(y)) => x.cmp(y),
        (SortKey::F64(x), SortKey::F64(y)) => x.partial_cmp(y).unwrap_or(std::cmp::Ordering::Equal),
        (SortKey::I64(x), SortKey::F64(y)) => (*x as f64)
            .partial_cmp(y)
            .unwrap_or(std::cmp::Ordering::Equal),
        (SortKey::F64(x), SortKey::I64(y)) => x
            .partial_cmp(&(*y as f64))
            .unwrap_or(std::cmp::Ordering::Equal),
        (SortKey::Str(x), SortKey::Str(y)) => (*x).cmp(*y),
        (SortKey::Bool(x), SortKey::Bool(y)) => x.cmp(y),
        _ => std::cmp::Ordering::Equal,
    };

    match direction {
        OrderDirection::Asc => base,
        OrderDirection::Desc => base.reverse(),
    }
}

/// Get a field from a JSON value by path segments.
pub(super) fn get_json_field<'a>(
    value: &'a json::Value,
    path: &[String],
) -> Option<&'a json::Value> {
    let mut current = value;
    for part in path {
        current = current.get(part.as_str())?;
    }
    Some(current)
}
