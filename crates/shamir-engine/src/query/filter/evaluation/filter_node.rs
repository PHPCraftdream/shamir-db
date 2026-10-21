//! Filter evaluation — compile Filter AST into an enum-dispatched tree.
//!
//! The compiled tree is a `FilterNode` enum (static dispatch via `match`)
//! rather than `Box<dyn FilterCallback>` (virtual call per node). Each
//! `matches()` call walks the tree with monomorphic compares; the
//! compiler can inline the dispatch arms.
//!
//! C6 (#80): the comparison layer is `QueryValue`-native (name-keyed).
//! Pre-resolved literals, the `InSet`/`ContainsAnySet`/`ContainsAllSet`
//! hash-sets, and every resolved operand are `QueryValue`. The only
//! `InnerValue` crossings that remain are the `RecordRef::materialize_at`
//! boundary (which still yields `InnerValue` today — narrowing that is a
//! LATER stage) and the index-crate `IndexExpr::eval` boundary; each is
//! converted **once** to `QueryValue` and never round-tripped back.

use std::cmp::Ordering;
use std::sync::{Arc, OnceLock};

use regex::Regex;
use shamir_collections::TSet;
use shamir_types::codecs::interned::inner_value_to_query_value;
use shamir_types::core::interner::InternerKey;
use shamir_types::record_view::{scalar_ref_cmp_qv, Kind, RecordRef};
use shamir_types::types::value::QueryValue;
use smallvec::SmallVec;

use crate::query::filter::evaluation::runtime::core::eval_context::FilterContext;
use crate::query::filter::evaluation::semantics::fts::{
    fts_word_matches, fts_word_matches_or, fts_word_matches_vec,
};
use crate::query::filter::evaluation::semantics::resolve::{
    compare_values, is_column_query_ref, resolve_filter_query, resolve_query_ref_column,
};
mod membership;

use self::membership::{contains_all_set_match, set_contains_coercing, set_contains_coercing_qv};

use crate::query::filter::FilterValue;

/// Compact field-path representation for `FilterNode` variants.
/// Inline up to 4 segments (typical: `"name"` → 1, `"address.city"` → 2);
/// spills to heap for deeper paths. Replaces a `Vec<u64>` per compiled
/// node — saves a heap alloc + dereference on every `matches()` walk.
///
/// F10: stores `InternerKey` directly (not raw `u64`) so each `matches()`
/// arm can pass `field_path` straight to `RecordRef` methods without
/// re-wrapping every segment per row. `InternerKey` is a `u64` newtype
/// (`pub struct InternerKey(u64)`), so `SmallVec<[InternerKey; 4]>` has
/// identical size/layout to the previous `SmallVec<[u64; 4]>`.
pub(in crate::query::filter) type CompactPath = SmallVec<[InternerKey; 4]>;

// ============================================================================
// CompareOp — comparison operator enum used by FilterNode and compile_filter.
// ============================================================================

#[derive(Debug, Clone, Copy)]
pub enum CompareOp {
    Eq,
    Ne,
    Gt,
    Gte,
    Lt,
    Lte,
}

// ============================================================================
// FilterNode — enum-dispatched compiled filter
// ============================================================================

/// Compiled filter tree node. One enum variant per filter shape;
/// `matches()` is a single `match` so the compiler can inline each
/// arm. Previously this was `Box<dyn FilterCallback>` per node —
/// every internal recursive call paid a virtual dispatch (vtable
/// indirect call + cache miss potential).
///
/// C6 (#80): all literal/pre-resolved operands and membership sets are
/// `QueryValue` (name-keyed). The hot comparison path never crosses to
/// `InnerValue`.
pub enum FilterNode {
    /// Always true. Produced when a clause cancels out (e.g.
    /// `NotIn` on a non-existent field).
    True,
    /// Always false. Produced when a field path cannot be interned.
    False,
    Compare {
        field_path: CompactPath,
        value: FilterValue,
        /// Pre-resolved at compile time when `value` is a literal (QueryValue).
        pre_resolved: Option<QueryValue>,
        op: CompareOp,
    },
    /// Value-vs-value comparison (`Filter::ValueCompare`) — no field path,
    /// no record dependency. BOTH `left` and `right` are resolved via
    /// `resolve_filter_query` at MATCH time (never compile time — unlike
    /// `Compare` above, this can't be constant-folded since `$query` refs
    /// resolve from `ctx.resolved_refs`, which varies per call). Meaningful
    /// in ANY filter-evaluation context; the primary motivating use is
    /// `when` guards, which have no record to compare a field against.
    ///
    /// ## Null / "nothing to compare" semantics (#667)
    ///
    /// `matches()` resolves `left`/`right` independently and then dispatches
    /// on `(Option<QueryValue>, Option<QueryValue>)`. Reading that dispatch
    /// together with [`compare_values`](crate::query::filter::evaluation::semantics::resolve::compare_values)
    /// (`resolve.rs:81`) yields **three distinct "nothing to compare"
    /// shapes**, only one of which behaves the way a reader might naively
    /// guess:
    ///
    /// 1. **Genuinely unresolvable operand** (either side) — `resolve_filter_query`
    ///    itself returns `None` (unbound `$query` alias, errored `$fn` call,
    ///    unbound `$param`, ...): an ABSENCE, not a value. Hits the outer
    ///    `(None, _) | (_, None)` arm — this does NOT distinguish
    ///    left-absent from right-absent from both-absent. Result: only `Ne`
    ///    is `true`; `Eq`/`Gt`/`Gte`/`Lt`/`Lte` are all `false`.
    /// 2. **Both operands resolve to the LITERAL value `null`** (e.g. both
    ///    sides are `FilterValue::Null`, or a `$query` ref whose target
    ///    field is genuinely `null`) — this is `Some(QueryValue::Null)` on
    ///    BOTH sides, so it reaches the inner `(Some(a), Some(b))` arm and
    ///    calls `compare_values(&Null, &Null)`, which deliberately returns
    ///    `Some(Ordering::Equal)`. Result: `Eq`/`Gte`/`Lte` are `true`;
    ///    `Ne`/`Gt`/`Lt` are `false` — the OPPOSITE of case 1. An explicit,
    ///    resolved `null` on both sides is treated as a genuinely
    ///    COMPARABLE, EQUAL value — closer to JS's `null === null` than to
    ///    SQL's three-valued `NULL = NULL` (which is `UNKNOWN`, not `TRUE`).
    /// 3. **One operand resolves to the literal value `null`, the other to
    ///    a non-null value of a different type** — both sides ARE
    ///    `Some(..)`, so this also reaches the inner arm, but
    ///    `compare_values(&Null, &Int(_))` (etc.) falls through to
    ///    `compare_values`'s `_ => None` catch-all (no same-type arm
    ///    matches). Result: only `Ne` is `true` — outwardly IDENTICAL to
    ///    case 1's boolean shape, but reached via a resolved type MISMATCH,
    ///    not an absent operand. Worth keeping distinct in the mental model:
    ///    a reader auditing `compare_values` alone needs to know this is
    ///    intentional, not an oversight.
    ///
    /// This 3-way distinction is intentional and covered by tests in
    /// `crates/shamir-engine/src/query/filter/tests/eval_tests/value_compare_null_tests.rs`
    /// and `crates/shamir-engine/src/query/batch/tests/executor_tests/when_skip_tests.rs`.
    /// It is NOT a bug — case 1 and case 3 happening to produce the same
    /// boolean outcome as each other, while case 2 produces the opposite,
    /// is the documented, adopted contract.
    ValueCompare {
        left: FilterValue,
        op: CompareOp,
        right: FilterValue,
    },
    And(Vec<FilterNode>),
    Or(Vec<FilterNode>),
    Not(Box<FilterNode>),
    IsNull {
        field_path: CompactPath,
    },
    IsNotNull {
        field_path: CompactPath,
    },
    In {
        field_path: CompactPath,
        values: Vec<FilterValue>,
        /// Parallel slice of pre-resolved literals (Null/Bool/Int/Float/String/Binary).
        /// `None` entries are non-literal variants (FieldRef/QueryRef/...) that
        /// still need per-record dynamic resolution via `resolve_filter_query`.
        /// Hoisting literal materialisation off the per-row path eliminates
        /// O(records × |list|) `String::clone` / `Vec::clone` allocations.
        pre_resolved: Vec<Option<QueryValue>>,
        /// Lazily pre-resolved column-query-ref membership sets, parallel to
        /// `values`. `Some(Arc<TSet>)` for column-query-ref entries (built
        /// once on the first `matches()` call, then cached), `None` for all
        /// other entry types. `OnceLock` provides lock-free `Sync` interior
        /// mutability — the init runs once per scan (uncontended), all
        /// subsequent rows read the cached `Vec` with zero lock / zero alloc.
        /// Mirrors how `InSet` carries its set inline.
        ///
        /// **Contention model**: filter evaluation is single-threaded per
        /// scan. `OnceLock::get_or_init` is infallible after the first call.
        ref_column_sets: OnceLock<Vec<Option<Arc<TSet<QueryValue>>>>>,
        negate: bool,
    },
    /// Fast-path for `$in`/`$nin` when ALL values are literals.
    /// Membership check is O(1) via `TSet<QueryValue>` (IndexSet + FxHasher)
    /// instead of the O(N) linear scan in `In`.
    InSet {
        field_path: CompactPath,
        values: TSet<QueryValue>,
        negate: bool,
    },
    Like {
        field_path: CompactPath,
        regex: Regex,
    },
    Regex {
        field_path: CompactPath,
        regex: Regex,
    },
    Contains {
        field_path: CompactPath,
        value: FilterValue,
        pre_resolved: Option<QueryValue>,
    },
    ContainsAny {
        field_path: CompactPath,
        values: Vec<FilterValue>,
    },
    /// Fast-path for `$contains_any` when ALL values are literals.
    /// Each element of the field array is checked via O(1) `TSet::contains`
    /// instead of the O(N×M) nested scan in `ContainsAny`.
    ContainsAnySet {
        field_path: CompactPath,
        values: TSet<QueryValue>,
    },
    ContainsAll {
        field_path: CompactPath,
        values: Vec<FilterValue>,
    },
    /// Fast-path for `$contains_all` when ALL values are literals.
    /// Counts how many set members appear in the field array; passes when
    /// the count equals `values.len()` — O(field_len) instead of O(N×M).
    ContainsAllSet {
        field_path: CompactPath,
        values: TSet<QueryValue>,
    },
    Between {
        field_path: CompactPath,
        from: FilterValue,
        to: FilterValue,
        pre_from: Option<QueryValue>,
        pre_to: Option<QueryValue>,
    },
    Exists {
        field_path: CompactPath,
    },
    NotExists {
        field_path: CompactPath,
    },

    /// FTS brute-force per-record fallback (no FTS index available).
    FtsMatch {
        field_path: CompactPath,
        query_tokens: Vec<String>,
        mode_and: bool,
    },
    /// Computed expression comparison (for functional index fallback).
    ComputedCompare {
        expr: Box<crate::index2::expr::IndexExpr>,
        value: FilterValue,
        pre_resolved: Option<QueryValue>,
        op: CompareOp,
    },
}

impl FilterNode {
    pub fn matches(&self, record: &(impl RecordRef + ?Sized), ctx: &FilterContext) -> bool {
        match self {
            FilterNode::True => true,
            FilterNode::False => false,

            FilterNode::Compare {
                field_path,
                value,
                pre_resolved,
                op,
            } => {
                let field_val = record.scalar_at(field_path);
                let owned_rhs;
                let filter_val: Option<&QueryValue> = if let Some(pre) = pre_resolved {
                    Some(pre)
                } else {
                    owned_rhs = resolve_filter_query(value, record, ctx);
                    owned_rhs.as_ref()
                };

                // FG-6: `scalar_at` returns `None` in two structurally
                // different situations that must NOT be conflated:
                //  1. the field is genuinely ABSENT (or descends through a
                //     non-map) — `present_kind_at` also returns `None`.
                //  2. the field IS present but is a non-comparable-as-scalar
                //     leaf — `Dec`/`Big` in the tree, or a promoted
                //     `u64 > i64::MAX` in the lens (`RecordValue::Str(Cow::Owned)`,
                //     which `present_kind_at` reports as `Scalar` since the
                //     lens cannot distinguish it from an ordinary string, but
                //     `scalar_at` still can't borrow it). Case 2 falls back to
                //     `materialize_at` (one owned leaf, off the common hot
                //     path — every ordinary Bool/Int/F64/Str/Bin field still
                //     resolves via the zero-copy `scalar_at` above) + a single
                //     `inner_value_to_query_value` conversion, mirroring the
                //     identical `FieldRef` resolution boundary in
                //     `resolve_filter_query` and the `AggAccum` Min/Max/Sum/Avg
                //     Dec/Big fallback in `aggregate.rs`.
                if field_val.is_none() && record.present_kind_at(field_path).is_some() {
                    let owned_field = record
                        .materialize_at(field_path)
                        .and_then(|iv| inner_value_to_query_value(&iv, ctx.interner).ok());
                    return match (owned_field.as_ref(), filter_val) {
                        (Some(a), Some(b)) => match op {
                            CompareOp::Eq => compare_values(a, b) == Some(Ordering::Equal),
                            CompareOp::Ne => compare_values(a, b) != Some(Ordering::Equal),
                            CompareOp::Gt => compare_values(a, b) == Some(Ordering::Greater),
                            CompareOp::Gte => matches!(
                                compare_values(a, b),
                                Some(Ordering::Greater | Ordering::Equal)
                            ),
                            CompareOp::Lt => compare_values(a, b) == Some(Ordering::Less),
                            CompareOp::Lte => {
                                matches!(
                                    compare_values(a, b),
                                    Some(Ordering::Less | Ordering::Equal)
                                )
                            }
                        },
                        (None, _) | (_, None) => matches!(op, CompareOp::Ne),
                    };
                }

                match (field_val, filter_val) {
                    (Some(a), Some(b)) => match op {
                        CompareOp::Eq => scalar_ref_cmp_qv(a, b) == Some(Ordering::Equal),
                        CompareOp::Ne => scalar_ref_cmp_qv(a, b) != Some(Ordering::Equal),
                        CompareOp::Gt => scalar_ref_cmp_qv(a, b) == Some(Ordering::Greater),
                        CompareOp::Gte => matches!(
                            scalar_ref_cmp_qv(a, b),
                            Some(Ordering::Greater | Ordering::Equal)
                        ),
                        CompareOp::Lt => scalar_ref_cmp_qv(a, b) == Some(Ordering::Less),
                        CompareOp::Lte => {
                            matches!(
                                scalar_ref_cmp_qv(a, b),
                                Some(Ordering::Less | Ordering::Equal)
                            )
                        }
                    },
                    (None, _) | (_, None) => matches!(op, CompareOp::Ne),
                }
            }

            FilterNode::ValueCompare { left, op, right } => {
                let lhs = resolve_filter_query(left, record, ctx);
                let rhs = resolve_filter_query(right, record, ctx);
                match (&lhs, &rhs) {
                    (Some(a), Some(b)) => match op {
                        CompareOp::Eq => compare_values(a, b) == Some(Ordering::Equal),
                        CompareOp::Ne => compare_values(a, b) != Some(Ordering::Equal),
                        CompareOp::Gt => compare_values(a, b) == Some(Ordering::Greater),
                        CompareOp::Gte => matches!(
                            compare_values(a, b),
                            Some(Ordering::Greater | Ordering::Equal)
                        ),
                        CompareOp::Lt => compare_values(a, b) == Some(Ordering::Less),
                        CompareOp::Lte => {
                            matches!(compare_values(a, b), Some(Ordering::Less | Ordering::Equal))
                        }
                    },
                    (None, _) | (_, None) => matches!(op, CompareOp::Ne),
                }
            }

            FilterNode::And(children) => children.iter().all(|c| c.matches(record, ctx)),
            FilterNode::Or(children) => children.iter().any(|c| c.matches(record, ctx)),
            FilterNode::Not(inner) => !inner.matches(record, ctx),

            FilterNode::IsNull { field_path } => record.is_null_at(field_path),
            FilterNode::IsNotNull { field_path } => !record.is_null_at(field_path),

            FilterNode::InSet {
                field_path,
                values,
                negate,
            } => {
                // F6: probe via the borrow-based `scalar_at` → `ScalarRef` (zero
                // clone for scalar fields, the common case) + `set_contains_coercing`
                // — the SAME `ScalarRef`-based coercing probe the sibling
                // `FilterNode::In`'s dynamic branch uses. Previously this arm
                // called `materialize_at` (an OWNED `InnerValue` clone of the
                // field — expensive for container fields) + `inner_value_to_query_value`
                // (a second conversion pass) + `set_contains_coercing_qv`.
                //
                // Semantics for a NON-scalar field (Map/List/Set/Bin) now match
                // `FilterNode::In` exactly: `scalar_at` returns `None`, so the
                // field is treated as ABSENT (`$in` → false, `$nin` → true).
                // The previous `materialize_at`-based form would have walked
                // INTO a container and converted it to a `QueryValue::Map/List/`
                // `Set` for an exact-match probe — but a literal `$in` set never
                // realistically contains an entire container, so the practical
                // outcome (no match) is unchanged; the documented contract is
                // now the consistent one shared with `In`. See
                // `inset_against_container_field_*` regression tests.
                let field_val = match record.scalar_at(field_path) {
                    Some(v) => v,
                    None => return *negate,
                };
                let found = set_contains_coercing(values, field_val);
                if *negate {
                    !found
                } else {
                    found
                }
            }

            FilterNode::In {
                field_path,
                values,
                pre_resolved,
                ref_column_sets,
                negate,
            } => {
                let field_val = match record.scalar_at(field_path) {
                    Some(v) => v,
                    None => return *negate,
                };

                // O(N²)→O(N): pre-resolve column-query-ref sets ONCE per
                // scan (first row), cache in `ref_column_sets`. Subsequent
                // rows read the cached `Vec` lock-free + alloc-free —
                // mirroring how `InSet` carries its set inline. The
                // `OnceLock::get_or_init` runs exactly once (single-threaded
                // per scan; uncontended).
                let col_sets = ref_column_sets.get_or_init(|| {
                    values
                        .iter()
                        .map(|fv| {
                            if is_column_query_ref(fv) {
                                if let FilterValue::QueryRef { alias, path } = fv {
                                    let key = alias.strip_prefix('@').unwrap_or(alias.as_str());
                                    let path_str = path.as_deref().unwrap_or("");
                                    if let Some(qr) = ctx.resolved_refs.get(key) {
                                        let column = resolve_query_ref_column(qr, Some(path_str));
                                        return Some(Arc::new(column.into_iter().collect()));
                                    }
                                }
                            }
                            None
                        })
                        .collect()
                });

                // Walk literals and non-literals in the same order as `values`
                // to preserve any short-circuit semantics; `pre_resolved[i]` is
                // `Some` exactly when `values[i]` is a literal (no per-record
                // alloc), `None` otherwise (FieldRef / QueryRef / ... — fall
                // back to dynamic resolution).
                let mut found = false;
                for (i, fv) in values.iter().enumerate() {
                    if let Some(pre) = &pre_resolved[i] {
                        if scalar_ref_cmp_qv(field_val, pre) == Some(Ordering::Equal) {
                            found = true;
                            break;
                        }
                        continue;
                    }
                    if is_column_query_ref(fv) {
                        // O(1) coercing set probe — preserves the EXACT
                        // equality semantics of the old `scalar_ref_cmp_qv`
                        // linear scan (Int↔F64 cross-type coercion).
                        //
                        // `InSet`/`ContainsAnySet`/`ContainsAllSet` (all-
                        // literals fast-paths) now use the same coercion
                        // via `set_contains_coercing_qv` /
                        // `index_of_coercing_qv`, closing the gap this
                        // comment previously acknowledged as a known
                        // pre-existing difference.
                        if let Some(set) = &col_sets[i] {
                            if set_contains_coercing(set, field_val) {
                                found = true;
                                break;
                            }
                        }
                        continue;
                    }
                    if let Some(resolved) = resolve_filter_query(fv, record, ctx) {
                        if scalar_ref_cmp_qv(field_val, &resolved) == Some(Ordering::Equal) {
                            found = true;
                            break;
                        }
                    }
                }
                if *negate {
                    !found
                } else {
                    found
                }
            }

            FilterNode::Like { field_path, regex } | FilterNode::Regex { field_path, regex } => {
                match record.str_at(field_path) {
                    Some(s) => regex.is_match(s),
                    None => false,
                }
            }

            FilterNode::Contains {
                field_path,
                value,
                pre_resolved,
            } => {
                let owned_rhs;
                let filter_val: &QueryValue = if let Some(pre) = pre_resolved {
                    pre
                } else {
                    owned_rhs = match resolve_filter_query(value, record, ctx) {
                        Some(v) => v,
                        None => return false,
                    };
                    &owned_rhs
                };

                // F-21 group: fast path — the field is a bare string leaf
                // (the common `$contains` shape). Borrow via `str_at` (the
                // same zero-copy path `Like`/`Regex` already use) instead of
                // `materialize_at` (an owned `InnerValue` clone) +
                // `inner_value_to_query_value` (a second conversion pass).
                if let Some(s) = record.str_at(field_path) {
                    return match filter_val {
                        QueryValue::Str(sub) => s.contains(sub.as_str()),
                        _ => false,
                    };
                }

                // Slow path: only a List/Set CONTAINER can still match
                // (`$contains` on a container checks membership). Everything
                // else at this point — absent, Null, a non-string scalar,
                // Dec/Big — is a definite non-match, so skip the
                // materialize entirely rather than pay for it just to fall
                // through to `_ => false` below.
                if !matches!(record.present_kind_at(field_path), Some(Kind::Container)) {
                    return false;
                }
                let field_owned = match record.materialize_at(field_path) {
                    Some(v) => v,
                    None => return false,
                };
                let field_qv = match inner_value_to_query_value(&field_owned, ctx.interner) {
                    Ok(qv) => qv,
                    Err(_) => return false,
                };
                match &field_qv {
                    QueryValue::List(list) => list
                        .iter()
                        .any(|item| compare_values(item, filter_val) == Some(Ordering::Equal)),
                    QueryValue::Set(set) => set
                        .iter()
                        .any(|item| compare_values(item, filter_val) == Some(Ordering::Equal)),
                    _ => false,
                }
            }

            FilterNode::ContainsAny { field_path, values } => {
                let field_owned = match record.materialize_at(field_path) {
                    Some(v) => v,
                    None => return false,
                };
                let field_qv = match inner_value_to_query_value(&field_owned, ctx.interner) {
                    Ok(qv) => qv,
                    Err(_) => return false,
                };
                values.iter().any(|fv| {
                    let resolved = match resolve_filter_query(fv, record, ctx) {
                        Some(v) => v,
                        None => return false,
                    };
                    match &field_qv {
                        QueryValue::List(list) => list
                            .iter()
                            .any(|item| compare_values(item, &resolved) == Some(Ordering::Equal)),
                        QueryValue::Set(set) => set
                            .iter()
                            .any(|item| compare_values(item, &resolved) == Some(Ordering::Equal)),
                        _ => false,
                    }
                })
            }

            FilterNode::ContainsAnySet { field_path, values } => {
                let field_owned = match record.materialize_at(field_path) {
                    Some(v) => v,
                    None => return false,
                };
                let field_qv = match inner_value_to_query_value(&field_owned, ctx.interner) {
                    Ok(qv) => qv,
                    Err(_) => return false,
                };
                match &field_qv {
                    QueryValue::List(list) => list
                        .iter()
                        .any(|item| set_contains_coercing_qv(values, item)),
                    QueryValue::Set(set) => set
                        .iter()
                        .any(|item| set_contains_coercing_qv(values, item)),
                    _ => false,
                }
            }

            FilterNode::ContainsAll { field_path, values } => {
                let field_owned = match record.materialize_at(field_path) {
                    Some(v) => v,
                    None => return false,
                };
                let field_qv = match inner_value_to_query_value(&field_owned, ctx.interner) {
                    Ok(qv) => qv,
                    Err(_) => return false,
                };
                values.iter().all(|fv| {
                    let resolved = match resolve_filter_query(fv, record, ctx) {
                        Some(v) => v,
                        None => return false,
                    };
                    match &field_qv {
                        QueryValue::List(list) => list
                            .iter()
                            .any(|item| compare_values(item, &resolved) == Some(Ordering::Equal)),
                        QueryValue::Set(set) => set
                            .iter()
                            .any(|item| compare_values(item, &resolved) == Some(Ordering::Equal)),
                        _ => false,
                    }
                })
            }

            FilterNode::ContainsAllSet { field_path, values } => {
                let field_owned = match record.materialize_at(field_path) {
                    Some(v) => v,
                    None => return false,
                };
                let field_qv = match inner_value_to_query_value(&field_owned, ctx.interner) {
                    Ok(qv) => qv,
                    Err(_) => return false,
                };
                // Pass only when EVERY required value is genuinely present in
                // the field's array/set. Counting raw element hits is wrong
                // here: a field like `["a", "a"]` would let two copies of one
                // required value numerically stand in for a second, absent
                // required value (the `$contains_all` slow path `ContainsAll`
                // already gets this right via per-value membership, so the two
                // must agree).
                //
                // F-21 group: `contains_all_set_match` tracks "found" required
                // values via an INDEX-based bitmask (see `contains_all_scan`)
                // instead of the old per-record `values.clone()` + swap_remove
                // — cloning re-allocated and re-copied every owned
                // `String`/`Vec<u8>` in the required-values set on EVERY row.
                contains_all_set_match(&field_qv, values)
            }

            FilterNode::Between {
                field_path,
                from,
                to,
                pre_from,
                pre_to,
            } => {
                let field_val = match record.scalar_at(field_path) {
                    Some(v) => v,
                    None => return false,
                };
                let owned_from;
                let from_val: &QueryValue = if let Some(pre) = pre_from {
                    pre
                } else {
                    owned_from = match resolve_filter_query(from, record, ctx) {
                        Some(v) => v,
                        None => return false,
                    };
                    &owned_from
                };
                let owned_to;
                let to_val: &QueryValue = if let Some(pre) = pre_to {
                    pre
                } else {
                    owned_to = match resolve_filter_query(to, record, ctx) {
                        Some(v) => v,
                        None => return false,
                    };
                    &owned_to
                };
                matches!(
                    scalar_ref_cmp_qv(field_val, from_val),
                    Some(Ordering::Greater | Ordering::Equal)
                ) && matches!(
                    scalar_ref_cmp_qv(field_val, to_val),
                    Some(Ordering::Less | Ordering::Equal)
                )
            }

            FilterNode::Exists { field_path } => record.exists_at(field_path),
            FilterNode::NotExists { field_path } => !record.exists_at(field_path),

            FilterNode::FtsMatch {
                field_path,
                query_tokens,
                mode_and,
            } => {
                let text = match record.str_at(field_path) {
                    Some(s) => s,
                    None => return false,
                };
                if query_tokens.is_empty() {
                    // AND over empty set = true; OR over empty set = false.
                    return *mode_and;
                }
                // Invert the loop: iterate field words once and probe the
                // small (1..=N) pre-lowercased query-token slice. Saves a
                // full-string `to_lowercase` alloc + a `HashSet<&str>` build
                // per record. Semantics preserved bit-for-bit: full Unicode
                // lowercasing applied per word (matches whole-string
                // `to_lowercase` exactly under whitespace tokenisation).
                //
                // AND mode uses a bitmask over `query_tokens` (capped at 64
                // tokens — beyond that we fall back to a Vec<bool>).
                if *mode_and && query_tokens.len() <= 64 {
                    let target: u64 = if query_tokens.len() == 64 {
                        u64::MAX
                    } else {
                        (1u64 << query_tokens.len()) - 1
                    };
                    let mut seen: u64 = 0;
                    for word in text.split_whitespace() {
                        if fts_word_matches(word, query_tokens, &mut seen) && seen == target {
                            return true;
                        }
                    }
                    seen == target
                } else if *mode_and {
                    let mut seen = vec![false; query_tokens.len()];
                    let mut remaining = query_tokens.len();
                    for word in text.split_whitespace() {
                        if fts_word_matches_vec(word, query_tokens, &mut seen, &mut remaining)
                            && remaining == 0
                        {
                            return true;
                        }
                    }
                    remaining == 0
                } else {
                    // OR mode — early-return on first hit.
                    for word in text.split_whitespace() {
                        if fts_word_matches_or(word, query_tokens) {
                            return true;
                        }
                    }
                    false
                }
            }

            FilterNode::ComputedCompare {
                expr,
                value,
                pre_resolved,
                op,
            } => {
                // IndexExpr::eval_with_scalars returns InnerValue. Convert
                // once to QueryValue; the comparison itself is then
                // QueryValue-to-QueryValue. The ScalarResolver from ctx is
                // threaded so IndexExpr::Scalar variants (user-registered
                // trusted_pure scalars) resolve on the brute-force path too.
                let resolver = &ctx.scalars;
                let computed_iv = match expr.eval_with_scalars(record, Some(resolver)) {
                    Ok(v) => v,
                    Err(_) => return false,
                };
                let computed = match inner_value_to_query_value(&computed_iv, ctx.interner) {
                    Ok(qv) => qv,
                    Err(_) => return false,
                };
                let owned_rhs;
                let rhs: &QueryValue = if let Some(pre) = pre_resolved {
                    pre
                } else {
                    owned_rhs = resolve_filter_query(value, record, ctx);
                    match owned_rhs.as_ref() {
                        Some(v) => v,
                        None => return false,
                    }
                };
                match op {
                    CompareOp::Eq => compare_values(&computed, rhs) == Some(Ordering::Equal),
                    CompareOp::Ne => compare_values(&computed, rhs) != Some(Ordering::Equal),
                    CompareOp::Gt => compare_values(&computed, rhs) == Some(Ordering::Greater),
                    CompareOp::Gte => matches!(
                        compare_values(&computed, rhs),
                        Some(Ordering::Greater | Ordering::Equal)
                    ),
                    CompareOp::Lt => compare_values(&computed, rhs) == Some(Ordering::Less),
                    CompareOp::Lte => matches!(
                        compare_values(&computed, rhs),
                        Some(Ordering::Less | Ordering::Equal)
                    ),
                }
            }
        }
    }
}
