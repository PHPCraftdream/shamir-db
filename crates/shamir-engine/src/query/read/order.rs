//! ORDER BY execution: QueryValue-native path.

use std::collections::BinaryHeap;

use num_bigint::BigInt;
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;
use smallvec::SmallVec;

use crate::query::filter::numeric_cmp::cmp_i64_f64;
use crate::query::read::{NullsOrder, OrderBy, OrderByItem, OrderDirection};
use shamir_types::types::record_id::RecordId;
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
    if let Some(idx) = qv_sort_permutation(records, order_by) {
        apply_permutation(records, &idx);
    }
}

/// Like [`apply_order_by_qv`] but applies the IDENTICAL permutation to a
/// companion `RecordId` vector in lockstep, so `ids[i]` stays aligned with
/// `records[i]` after the sort.
///
/// **Invariant:** `records.len() == ids.len()` — the caller (which built both
/// vectors in lockstep during the scan loop) guarantees this; a
/// `debug_assert!` guards it.
///
/// Used by the plain-ORDER BY + `with_version` read path: a plain sort
/// reorders rows but does not collapse row identity (unlike GROUP BY /
/// aggregates / DISTINCT), so each surviving output row still maps 1:1 to
/// exactly one source row whose `RecordId` — and therefore whose MVCC version
/// — is well-defined. Carrying the ids through the sort lets the per-record
/// `versions` array be rebuilt from the repositioned ids.
pub fn apply_order_by_qv_with_ids(
    records: &mut Vec<QueryValue>,
    ids: &mut Vec<RecordId>,
    order_by: &OrderBy,
) {
    debug_assert_eq!(
        records.len(),
        ids.len(),
        "apply_order_by_qv_with_ids: records and ids must have the same length (caller invariant)"
    );
    if let Some(idx) = qv_sort_permutation(records, order_by) {
        apply_permutation(records, &idx);
        apply_permutation(ids, &idx);
    }
}

/// Compute the ORDER BY sort permutation for `records` as an index array
/// (Phase 1: pre-resolve keys, Phase 2: sort the index array by those keys).
///
/// Returns `None` when no sort is needed (empty ORDER BY or ≤1 record) so
/// callers can treat it as a no-op without re-checking those conditions. Both
/// `apply_order_by_qv` and `apply_order_by_qv_with_ids` share this so the key
/// resolution / comparison logic is written once.
fn qv_sort_permutation(records: &[QueryValue], order_by: &OrderBy) -> Option<Vec<usize>> {
    if order_by.items.is_empty() || records.len() <= 1 {
        return None;
    }

    // Phase 1: pre-resolve canonical sort keys per record.
    let keys: Vec<QvPreResolvedKeys> = records
        .iter()
        .map(|r| resolve_qv_order_keys(r, &order_by.items))
        .collect();

    // Phase 2: sort index array by pre-resolved keys.
    let mut idx: Vec<usize> = (0..records.len()).collect();
    idx.sort_by(|&a, &b| compare_qv_preresolved(&keys[a], &keys[b], &order_by.items));
    Some(idx)
}

/// Apply an index permutation in place (Phase 3) — reorders `v` so that the
/// new `v[i]` is the old `v[idx[i]]`. Drains into a temp `Option<T>` vec and
/// picks by index (no `Default` bound needed, so this works for `QueryValue`,
/// which has none). The permutation must be a valid reordering of `0..v.len()`;
/// each index is taken exactly once.
fn apply_permutation<T>(v: &mut Vec<T>, idx: &[usize]) {
    let mut tmp: Vec<Option<T>> = v.drain(..).map(Some).collect();
    for &i in idx {
        v.push(tmp[i].take().expect("permutation index used twice"));
    }
}

/// Bounded top-K ORDER BY: returns the first `skip + take` records in order,
/// using O(skip + take) memory via a `BinaryHeap` capped at `skip + take`.
///
/// Thin wrapper over [`TopKHeap`] — kept as a standalone entry point so the
/// post-materialization shape (consume a ready `Vec<QueryValue>`) stays
/// available for tests and any caller that already has the full projection in
/// hand. The inline scan-loop paths (`read_collecting`, `read_as_of`) call
/// [`TopKHeap`] directly so they never build that intermediate `Vec` (F-53a).
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
    let mut heap = TopKHeap::new(order_by, skip, take, false);
    for value in records {
        heap.push(value, None);
    }
    heap.into_sorted().0
}

// ============================================================================
// TopKHeap — the shared, reusable bounded top-K max-heap (F-53a, #874)
// ============================================================================

/// One entry in the bounded top-K heap. Carries pre-resolved sort keys, the
/// insertion index (for stable tie-breaking, mirroring `sort_by` stability),
/// the projected value, and — opt-in via [`TopKHeap::new`]`(..., with_ids =
/// true)` — the source `RecordId` the `with_version` read path threads through
/// the sort so the per-record `versions` array can be rebuilt from the
/// surviving heap rows alone.
///
/// Borrows `&'ob [OrderByItem]` instead of the raw `*const [OrderByItem]` the
/// pre-F-53a inlined version used: the borrow gives `HeapItem: Send` for free
/// (no `unsafe impl Send`) so a `TopKHeap` can be held across `.await` points
/// in the async scan loop. `OrderByItem: Sync` ⇒ `&[OrderByItem]: Send`.
struct HeapItem<'ob> {
    keys: QvPreResolvedKeys,
    idx: usize,
    value: QueryValue,
    id: Option<RecordId>,
    items: &'ob [OrderByItem],
}

impl<'ob> HeapItem<'ob> {
    /// ORDER BY-direction comparison with ascending insertion-index tie-break.
    /// This is the single source of truth for top-K ordering: both the old
    /// full-sort path (`apply_order_by_qv`'s `compare_qv_preresolved`) and the
    /// heap path route through it, so the two are byte-identical by
    /// construction.
    #[inline]
    fn cmp_order(&self, other: &Self) -> std::cmp::Ordering {
        let ord = compare_qv_preresolved(&self.keys, &other.keys, self.items);
        ord.then_with(|| self.idx.cmp(&other.idx))
    }
}

impl<'ob> PartialEq for HeapItem<'ob> {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == std::cmp::Ordering::Equal
    }
}
impl<'ob> Eq for HeapItem<'ob> {}
impl<'ob> PartialOrd for HeapItem<'ob> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl<'ob> Ord for HeapItem<'ob> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // BinaryHeap is a max-heap; root = worst candidate (sorts last).
        self.cmp_order(other)
    }
}

/// Bounded top-K max-heap over `QueryValue` rows, keyed by an ORDER BY spec.
///
/// (F-53a, #874) Encapsulates the comparator, sort-key extraction, and
/// heap-eviction logic so the SAME comparison semantics serve both the
/// post-materialization [`apply_order_by_topk`] and the inline scan loops in
/// `read_collecting` / `read_as_of` — which feed rows directly into the heap
/// DURING the scan, avoiding the O(N) `rec_acc` / `matched` accumulation the
/// old shape paid BEFORE the heap trim ever ran (the pre-F-53a code comment
/// claiming "O(K) memory" described only the heap's own internals, not the
/// full projected `Vec` feeding it).
///
/// The heap is a max-heap ordered by the ORDER BY comparator: the root is the
/// WORST candidate in the current top-K window. While the heap holds fewer
/// than `k = skip + take` rows every incoming row is admitted; once full, a
/// row that sorts strictly before the root evicts it, all others are dropped.
/// After all rows are pushed, [`into_sorted`](Self::into_sorted) drains the
/// heap, sorts by ORDER BY direction + insertion index (stable tie-break), and
/// applies the `skip`/`take` window — byte-identical to `apply_order_by_qv` +
/// truncation.
///
/// `with_ids = true` makes each item carry its `RecordId` (mirroring the
/// `id_acc` pairing the plain-ORDER BY + `with_version` read path needs); the
/// ids come out of `into_sorted` aligned with the surviving values, so the
/// per-record `versions` array can be rebuilt from the heap's final survivors
/// alone — never the full scan's ids.
pub struct TopKHeap<'ob> {
    heap: BinaryHeap<HeapItem<'ob>>,
    items: &'ob [OrderByItem],
    k: usize,
    skip: usize,
    take: usize,
    /// Monotonic insertion counter shared by every push — increments on EVERY
    /// call regardless of whether the row entered the heap, so the tie-break
    /// index reflects the order rows were SEEN (matching `enumerate()` on the
    /// old full-`Vec` input), not the order they were admitted.
    next_idx: usize,
    with_ids: bool,
}

impl<'ob> TopKHeap<'ob> {
    /// Build a bounded heap capped at `k = skip.saturating_add(take)`.
    ///
    /// `with_ids` selects whether each pushed row carries its `RecordId` (the
    /// `with_version` read path sets this so the per-record versions array can
    /// be rebuilt from the surviving heap rows alone).
    pub fn new(order_by: &'ob OrderBy, skip: usize, take: usize, with_ids: bool) -> Self {
        let k = skip.saturating_add(take);
        Self {
            heap: BinaryHeap::with_capacity(k + 1),
            items: &order_by.items[..],
            k,
            skip,
            take,
            next_idx: 0,
            with_ids,
        }
    }

    /// Push one row into the heap. The heap self-bounds to `k` rows: while it
    /// has fewer than `k` items the row is always admitted; once full, a row
    /// that sorts strictly before the current root (worst) evicts it, all
    /// others are dropped. `id` is stored iff `with_ids` was set at
    /// construction; callers that did not enable it pass `None`.
    #[inline]
    pub fn push(&mut self, value: QueryValue, id: Option<RecordId>) {
        let keys = resolve_qv_order_keys(&value, self.items);
        let idx = self.next_idx;
        self.next_idx += 1;
        let stored_id = if self.with_ids { id } else { None };
        if self.heap.len() < self.k {
            self.heap.push(HeapItem {
                keys,
                idx,
                value,
                id: stored_id,
                items: self.items,
            });
        } else if let Some(worst) = self.heap.peek() {
            // If new element sorts BEFORE the worst in the heap, swap.
            let new_item = HeapItem {
                keys,
                idx,
                value,
                id: stored_id,
                items: self.items,
            };
            if new_item.cmp_order(worst) == std::cmp::Ordering::Less {
                self.heap.pop();
                self.heap.push(new_item);
            }
        }
    }

    /// Drain the heap, sort by ORDER BY direction + insertion index, apply the
    /// `skip`/`take` window, and return `(values, ids)`. The `ids` vec is
    /// populated only when `with_ids` was set at construction; in that case it
    /// is exactly `values.len()` entries long and index-aligned with it.
    pub fn into_sorted(self) -> (Vec<QueryValue>, Vec<RecordId>) {
        // Drain and sort the top-K by ORDER BY direction + insertion order.
        let mut top_k: Vec<HeapItem<'ob>> = self.heap.into_vec();
        top_k.sort_by(|a, b| a.cmp_order(b));

        let n = top_k.len().saturating_sub(self.skip).min(self.take);
        let mut values = Vec::with_capacity(n);
        let mut ids = if self.with_ids {
            Vec::with_capacity(n)
        } else {
            Vec::new()
        };
        for item in top_k.into_iter().skip(self.skip).take(self.take) {
            values.push(item.value);
            if self.with_ids {
                // Alignment invariant: with_ids ⇒ every push carried Some(id).
                ids.push(item.id.expect("with_ids heap item must carry a RecordId"));
            }
        }
        (values, ids)
    }

    /// Number of rows currently held (always ≤ `k`).
    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.heap.len()
    }

    /// Whether the heap currently holds no rows.
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.heap.is_empty()
    }
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
        // involved. Now exact via the shared `numeric_cmp::cmp_i64_f64`
        // (F-2, #791 — consolidated out of a byte-for-byte duplicate that
        // used to live here), keeping the EXISTING `.unwrap_or(Equal)` NaN
        // fallback convention this function established for every other
        // cross-type arm.
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
