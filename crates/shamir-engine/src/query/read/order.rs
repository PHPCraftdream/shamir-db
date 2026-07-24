//! ORDER BY execution: QueryValue-native path.

use std::collections::BinaryHeap;

use num_bigint::BigInt;
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;
use smallvec::SmallVec;

use crate::query::read::{NullsOrder, OrderBy, OrderByItem, OrderDirection};
use shamir_types::types::value::QueryValue;

// ============================================================================
// QueryValue-based ORDER BY
// ============================================================================

/// Sort `QueryValue` rows by ORDER BY items.
///
/// Uses the canonical-key approach: sort keys are extracted to match the
/// semantics of the pre-J1 ORDER BY exactly — in particular `Dec` values are
/// compared numerically (via a dedicated `Dec` sort-key variant), `Big`
/// values are compared as their `to_string()` form (lexicographic — a
/// separate, lower-priority item), and `Bin`/`Set` map to `Other` (unsortable,
/// preserving insertion order via stable sort).
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

/// Bounded top-K ORDER BY: returns the first `skip + take` records in order,
/// using O(skip + take) memory via a `BinaryHeap` capped at `skip + take`.
///
/// The heap uses *reversed* comparison so the root is the WORST element in the
/// current top-K set. When a new row compares better than the root, we pop the
/// root and push the new row. After all rows are consumed, the heap is drained
/// and sorted to produce the final ordered slice.
///
/// Insertion order (`idx`) is used as a tiebreaker for equal sort keys to
/// match the stable-sort semantics of `apply_order_by_qv`.
///
/// The result is byte-identical to `apply_order_by_qv` + truncation.
pub fn apply_order_by_topk(
    records: Vec<QueryValue>,
    order_by: &OrderBy,
    skip: usize,
    take: usize,
) -> Vec<QueryValue> {
    if order_by.items.is_empty() || records.is_empty() || take == 0 {
        return Vec::new();
    }

    let k = skip.saturating_add(take);

    // HeapItem carries pre-resolved sort keys, insertion index (for stable
    // tie-breaking), and the value. Comparison is in ORDER BY direction;
    // equal keys break by ascending insertion index (preserving insertion
    // order, matching `sort_by` stability).
    struct HeapItem {
        keys: QvPreResolvedKeys,
        idx: usize,
        value: QueryValue,
        items_ptr: *const [OrderByItem],
    }

    // SAFETY: QueryValue is Send, QvSortKey is Send, and items_ptr is only
    // dereferenced within this function's scope where order_by is alive.
    unsafe impl Send for HeapItem {}

    impl HeapItem {
        #[inline]
        fn cmp_order(&self, other: &Self) -> std::cmp::Ordering {
            let items = unsafe { &*self.items_ptr };
            let ord = compare_qv_preresolved(&self.keys, &other.keys, items);
            ord.then_with(|| self.idx.cmp(&other.idx))
        }
    }

    impl PartialEq for HeapItem {
        fn eq(&self, other: &Self) -> bool {
            self.cmp(other) == std::cmp::Ordering::Equal
        }
    }
    impl Eq for HeapItem {}
    impl PartialOrd for HeapItem {
        fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
            Some(self.cmp(other))
        }
    }
    impl Ord for HeapItem {
        fn cmp(&self, other: &Self) -> std::cmp::Ordering {
            // BinaryHeap is a max-heap; root = worst candidate (sorts last).
            self.cmp_order(other)
        }
    }

    let items_ptr: *const [OrderByItem] = &order_by.items[..];
    let mut heap: BinaryHeap<HeapItem> = BinaryHeap::with_capacity(k + 1);

    for (idx, value) in records.into_iter().enumerate() {
        let keys = resolve_qv_order_keys(&value, &order_by.items);

        if heap.len() < k {
            heap.push(HeapItem {
                keys,
                idx,
                value,
                items_ptr,
            });
        } else if let Some(worst) = heap.peek() {
            // If new element sorts BEFORE the worst in the heap, swap.
            let new_item = HeapItem {
                keys,
                idx,
                value,
                items_ptr,
            };
            if new_item.cmp_order(worst) == std::cmp::Ordering::Less {
                heap.pop();
                heap.push(new_item);
            }
        }
    }

    // Drain and sort the top-K by ORDER BY direction + insertion order.
    let mut top_k: Vec<HeapItem> = heap.into_vec();
    top_k.sort_by(|a, b| a.cmp_order(b));

    // Apply skip, then take.
    top_k
        .into_iter()
        .skip(skip)
        .take(take)
        .map(|e| e.value)
        .collect()
}

/// Owned sort key for QueryValue fields. Unlike the legacy `SortKey<'a>` this
/// does not borrow from the source records. `Dec` is preserved as a dedicated
/// numeric variant (exact `Decimal: Ord` comparison); `Big` is likewise a
/// dedicated numeric variant (exact `BigInt: Ord` for Big/Big, f64 fallback
/// for cross-type against `I64`/`F64`/`Dec` — mirrors `compare_values`'s
/// existing `Big` arms in `resolve.rs`, FG-6). Comparison semantics match the
/// former `compare_sort_keys` for every non-Dec/non-Big type, and are numeric
/// for both Dec and Big (including Int↔Big / Big↔Big cross-comparison).
#[derive(Clone)]
enum QvSortKey {
    Null,
    Bool(bool),
    I64(i64),
    F64(f64),
    Dec(Decimal),
    Big(BigInt),
    Str(String),
    Other,
}

/// Lossy `BigInt` → `f64` (NaN on overflow). Mirrors `resolve.rs`'s
/// `lossy_f64` / `scalar_ref.rs`'s `big_to_f64`.
///
/// CR-C5 (#780): this is now the DELIBERATE, accepted-approximation path for
/// `Big`↔`F64` only — see `compare_qv_sort_keys`'s `(Big, F64)`/`(F64, Big)`
/// arms for why (F64 is inherently imprecise; there is no single "correct"
/// exact answer). `Big`↔`I64` and `Big`↔`Dec` no longer use this helper —
/// they compare two exact types via `cmp_i64_big`/`cmp_big_dec` instead.
#[inline]
fn big_to_f64(b: &BigInt) -> f64 {
    b.to_f64().unwrap_or(f64::NAN)
}

/// Exact `i64` vs `BigInt` comparison — CR-C5 (#780), mirrors
/// `resolve.rs::compare_values`'s `(Int, Big)`/`(Big, Int)` arms. An `i64`
/// always converts to `BigInt` losslessly (unlike the reverse `f64`
/// conversion `big_to_f64` performs), so this is exact with no edge case.
#[inline]
fn cmp_i64_big(i: i64, b: &BigInt) -> std::cmp::Ordering {
    BigInt::from(i).cmp(b)
}

/// Exact `i64` vs `f64` comparison — CR-D3 (#784), mirrors
/// `resolve.rs::cmp_i64_f64` (see its doc comment for the full derivation).
/// `f64`'s 11-bit exponent covers every integer up to `2^63` in magnitude
/// exactly at the boundaries (`i64::MIN == -2^63`, `i64::MAX == 2^63 - 1`,
/// both exact powers of two); any finite `f` within that range has an exact,
/// losslessly-`i64`-castable `floor()`, so a bounds-check + floor/fract
/// tie-break is exact with no `BigInt` needed.
#[inline]
fn cmp_i64_f64(i: i64, f: f64) -> Option<std::cmp::Ordering> {
    if f.is_nan() {
        return None;
    }
    if f.is_infinite() {
        return Some(if f > 0.0 {
            std::cmp::Ordering::Less
        } else {
            std::cmp::Ordering::Greater
        });
    }
    const I64_MIN_AS_F64: f64 = -9223372036854775808.0; // -2^63, exact
    const I64_MAX_EXCLUSIVE_UPPER_BOUND: f64 = 9223372036854775808.0; // 2^63, exact
    if f < I64_MIN_AS_F64 {
        return Some(std::cmp::Ordering::Greater);
    }
    if f >= I64_MAX_EXCLUSIVE_UPPER_BOUND {
        return Some(std::cmp::Ordering::Less);
    }
    let f_floor = f.floor();
    let f_floor_i64 = f_floor as i64;
    match i.cmp(&f_floor_i64) {
        std::cmp::Ordering::Equal => {
            if f.fract() > 0.0 {
                Some(std::cmp::Ordering::Less)
            } else {
                Some(std::cmp::Ordering::Equal)
            }
        }
        other => Some(other),
    }
}

/// Exact `Decimal` vs `BigInt` comparison via cross-multiplication — CR-C5
/// (#780), mirrors `resolve.rs::cmp_big_dec` (see its doc comment for the
/// full derivation). `Decimal == mantissa / 10^scale`; cross-multiplying by
/// `10^scale` (arbitrary-precision, via `BigInt`) lifts both sides to
/// exact integers with no `f64` intermediate.
#[inline]
fn cmp_big_dec(big: &BigInt, dec: &Decimal) -> std::cmp::Ordering {
    let scale_factor = BigInt::from(10u32).pow(dec.scale());
    let lhs = big * scale_factor;
    let rhs = BigInt::from(dec.mantissa());
    lhs.cmp(&rhs)
}

impl QvSortKey {
    /// Extract a canonical sort key from a `QueryValue` field reference.
    /// - `Int` -> I64, `F64` -> F64, `Bool` -> Bool, `Str` -> Str (cloned)
    /// - `Dec(d)` -> Dec(*d) -- numeric comparison (exact via `Decimal: Ord`)
    /// - `Big(b)` -> Big(b.clone()) -- numeric comparison (exact `BigInt: Ord`
    ///   for Big/Big; f64 fallback cross-type against I64/F64/Dec)
    /// - `Null` / missing -> Null
    /// - `Bin`, `Set`, `List`, `Map` -> Other (unsortable)
    fn from_query_value(v: &QueryValue) -> Self {
        match v {
            QueryValue::Null => QvSortKey::Null,
            QueryValue::Bool(b) => QvSortKey::Bool(*b),
            QueryValue::Int(i) => QvSortKey::I64(*i),
            QueryValue::F64(f) => QvSortKey::F64(*f),
            QueryValue::Str(s) => QvSortKey::Str(s.clone()),
            QueryValue::Dec(d) => QvSortKey::Dec(*d),
            QueryValue::Big(b) => QvSortKey::Big(b.clone()),
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
        // I64<->F64: CR-D3 (#784), follow-up to CR-C5 (#780)'s own
        // re-verification finding (see `resolve.rs::compare_values`'s
        // matching `(Int, F64)` arm for the full writeup): the plain
        // `as f64` cast was lossy for large `i64` magnitudes with NO `Big`
        // involved. Now exact via `cmp_i64_f64`, keeping the EXISTING
        // `.unwrap_or(Equal)` NaN fallback convention this function
        // established for every other cross-type arm.
        (QvSortKey::I64(x), QvSortKey::F64(y)) => {
            cmp_i64_f64(*x, *y).unwrap_or(std::cmp::Ordering::Equal)
        }
        (QvSortKey::F64(x), QvSortKey::I64(y)) => cmp_i64_f64(*y, *x)
            .map(std::cmp::Ordering::reverse)
            .unwrap_or(std::cmp::Ordering::Equal),
        // Dec: exact for Dec/Dec and I64↔Dec (`Decimal` represents every i64
        // exactly); F64↔Dec uses the f64 fallback (mirrors I64↔F64 style).
        (QvSortKey::Dec(x), QvSortKey::Dec(y)) => x.cmp(y),
        (QvSortKey::I64(x), QvSortKey::Dec(y)) => Decimal::from(*x).cmp(y),
        (QvSortKey::Dec(x), QvSortKey::I64(y)) => x.cmp(&Decimal::from(*y)),
        (QvSortKey::F64(x), QvSortKey::Dec(y)) => x
            .partial_cmp(&y.to_f64().unwrap_or(f64::NAN))
            .unwrap_or(std::cmp::Ordering::Equal),
        (QvSortKey::Dec(x), QvSortKey::F64(y)) => x
            .to_f64()
            .unwrap_or(f64::NAN)
            .partial_cmp(y)
            .unwrap_or(std::cmp::Ordering::Equal),
        // Big: exact for Big/Big (`BigInt: Ord`, unchanged). CR-C5 (#780):
        // I64/Dec cross-type arms are now ALSO exact (`cmp_i64_big` /
        // `cmp_big_dec`, both exact-integer arithmetic); only the F64
        // cross-type arms keep the `f64` fallback, as a DELIBERATE, accepted
        // approximation — `F64` is itself an inherently imprecise IEEE-754
        // column type, so comparing an exact `BigInt` against it has no
        // single "correct" exact answer beyond "which f64 is closest". This
        // is distinct from the I64/Dec arms: those compare two EXACT types,
        // where the `f64` intermediate was a genuine comparison-code bug.
        (QvSortKey::Big(x), QvSortKey::Big(y)) => x.cmp(y),
        (QvSortKey::I64(x), QvSortKey::Big(y)) => cmp_i64_big(*x, y),
        (QvSortKey::Big(x), QvSortKey::I64(y)) => cmp_i64_big(*y, x).reverse(),
        (QvSortKey::F64(x), QvSortKey::Big(y)) => x
            .partial_cmp(&big_to_f64(y))
            .unwrap_or(std::cmp::Ordering::Equal),
        (QvSortKey::Big(x), QvSortKey::F64(y)) => big_to_f64(x)
            .partial_cmp(y)
            .unwrap_or(std::cmp::Ordering::Equal),
        (QvSortKey::Dec(x), QvSortKey::Big(y)) => cmp_big_dec(y, x).reverse(),
        (QvSortKey::Big(x), QvSortKey::Dec(y)) => cmp_big_dec(x, y),
        (QvSortKey::Str(x), QvSortKey::Str(y)) => x.cmp(y),
        (QvSortKey::Bool(x), QvSortKey::Bool(y)) => x.cmp(y),
        _ => std::cmp::Ordering::Equal,
    };

    match direction {
        OrderDirection::Asc => base,
        OrderDirection::Desc => base.reverse(),
    }
}
