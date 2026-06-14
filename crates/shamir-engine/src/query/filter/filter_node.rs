//! Filter evaluation — compile Filter AST into an enum-dispatched tree.
//!
//! The compiled tree is a `FilterNode` enum (static dispatch via `match`)
//! rather than `Box<dyn FilterCallback>` (virtual call per node). Each
//! `matches()` call walks the tree with monomorphic compares; the
//! compiler can inline the dispatch arms.

use std::cmp::Ordering;

use regex::Regex;
use shamir_collections::TSet;
use smallvec::SmallVec;

use super::eval_context::FilterContext;
use super::filter_callback::FilterCallback;
use super::fts::{fts_word_matches, fts_word_matches_or, fts_word_matches_vec};
use super::resolve::{
    compare_values, is_column_query_ref, resolve_field_ref, resolve_filter_value,
    resolve_query_ref_column,
};
use crate::query::filter::FilterValue;

/// Compact field-path representation for `FilterNode` variants.
/// Inline up to 4 segments (typical: `"name"` → 1, `"address.city"` → 2);
/// spills to heap for deeper paths. Replaces a `Vec<u64>` per compiled
/// node — saves a heap alloc + dereference on every `matches()` walk.
pub(super) type CompactPath = SmallVec<[u64; 4]>;

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
pub enum FilterNode {
    /// Always true. Produced when a clause cancels out (e.g.
    /// `NotIn` on a non-existent field).
    True,
    /// Always false. Produced when a field path cannot be interned.
    False,
    Compare {
        field_path: CompactPath,
        value: FilterValue,
        /// Pre-resolved at compile time when `value` is a literal.
        pre_resolved: Option<shamir_types::types::value::InnerValue>,
        op: CompareOp,
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
        /// still need per-record dynamic resolution via `resolve_filter_value`.
        /// Hoisting literal materialisation off the per-record path eliminates
        /// O(records × |list|) `String::clone` / `Vec::clone` allocations.
        pre_resolved: Vec<Option<shamir_types::types::value::InnerValue>>,
        negate: bool,
    },
    /// Fast-path for `$in`/`$nin` when ALL values are literals.
    /// Membership check is O(1) via `TSet<InnerValue>` (IndexSet + FxHasher)
    /// instead of the O(N) linear scan in `In`.
    InSet {
        field_path: CompactPath,
        values: TSet<shamir_types::types::value::InnerValue>,
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
        pre_resolved: Option<shamir_types::types::value::InnerValue>,
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
        values: TSet<shamir_types::types::value::InnerValue>,
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
        values: TSet<shamir_types::types::value::InnerValue>,
    },
    Between {
        field_path: CompactPath,
        from: FilterValue,
        to: FilterValue,
        pre_from: Option<shamir_types::types::value::InnerValue>,
        pre_to: Option<shamir_types::types::value::InnerValue>,
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
        pre_resolved: Option<shamir_types::types::value::InnerValue>,
        op: CompareOp,
    },
}

impl FilterNode {
    pub fn matches(
        &self,
        record: &shamir_types::types::value::InnerValue,
        ctx: &FilterContext,
    ) -> bool {
        use shamir_types::types::value::InnerValue;
        match self {
            FilterNode::True => true,
            FilterNode::False => false,

            FilterNode::Compare {
                field_path,
                value,
                pre_resolved,
                op,
            } => {
                let field_val = resolve_field_ref(record, field_path);
                let owned_rhs;
                let filter_val: Option<&InnerValue> = if let Some(pre) = pre_resolved {
                    Some(pre)
                } else {
                    owned_rhs = resolve_filter_value(value, record, ctx);
                    owned_rhs.as_ref()
                };

                match (field_val, filter_val) {
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

            FilterNode::IsNull { field_path } => matches!(
                resolve_field_ref(record, field_path),
                None | Some(InnerValue::Null)
            ),
            FilterNode::IsNotNull { field_path } => !matches!(
                resolve_field_ref(record, field_path),
                None | Some(InnerValue::Null)
            ),

            FilterNode::InSet {
                field_path,
                values,
                negate,
            } => {
                let found = match resolve_field_ref(record, field_path) {
                    Some(v) => values.contains(v),
                    None => false,
                };
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
                negate,
            } => {
                let field_val = match resolve_field_ref(record, field_path) {
                    Some(v) => v,
                    None => return *negate,
                };
                // Walk literals and non-literals in the same order as `values`
                // to preserve any short-circuit semantics; `pre_resolved[i]` is
                // `Some` exactly when `values[i]` is a literal (no per-record
                // alloc), `None` otherwise (FieldRef / QueryRef / ... — fall
                // back to dynamic resolution).
                let mut found = false;
                for (i, fv) in values.iter().enumerate() {
                    if let Some(pre) = &pre_resolved[i] {
                        if compare_values(field_val, pre) == Some(Ordering::Equal) {
                            found = true;
                            break;
                        }
                        continue;
                    }
                    if is_column_query_ref(fv) {
                        if let FilterValue::QueryRef { alias, path } = fv {
                            let key = alias.strip_prefix('@').unwrap_or(alias.as_str());
                            if let Some(qr) = ctx.resolved_refs.get(key) {
                                let column = resolve_query_ref_column(qr, path.as_deref());
                                if column.iter().any(|cv| {
                                    compare_values(field_val, cv) == Some(Ordering::Equal)
                                }) {
                                    found = true;
                                    break;
                                }
                            }
                        }
                        continue;
                    }
                    if let Some(resolved) = resolve_filter_value(fv, record, ctx) {
                        if compare_values(field_val, &resolved) == Some(Ordering::Equal) {
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
                match resolve_field_ref(record, field_path) {
                    Some(InnerValue::Str(s)) => regex.is_match(s),
                    _ => false,
                }
            }

            FilterNode::Contains {
                field_path,
                value,
                pre_resolved,
            } => {
                let field_val = match resolve_field_ref(record, field_path) {
                    Some(v) => v,
                    None => return false,
                };
                let owned_rhs;
                let filter_val: &InnerValue = if let Some(pre) = pre_resolved {
                    pre
                } else {
                    owned_rhs = match resolve_filter_value(value, record, ctx) {
                        Some(v) => v,
                        None => return false,
                    };
                    &owned_rhs
                };
                match field_val {
                    InnerValue::Str(s) => {
                        if let InnerValue::Str(sub) = filter_val {
                            s.contains(sub.as_str())
                        } else {
                            false
                        }
                    }
                    InnerValue::List(list) => list
                        .iter()
                        .any(|item| compare_values(item, filter_val) == Some(Ordering::Equal)),
                    InnerValue::Set(set) => set
                        .iter()
                        .any(|item| compare_values(item, filter_val) == Some(Ordering::Equal)),
                    _ => false,
                }
            }

            FilterNode::ContainsAny { field_path, values } => {
                let field_val = match resolve_field_ref(record, field_path) {
                    Some(v) => v,
                    None => return false,
                };
                values.iter().any(|fv| {
                    let resolved = match resolve_filter_value(fv, record, ctx) {
                        Some(v) => v,
                        None => return false,
                    };
                    match field_val {
                        InnerValue::List(list) => list
                            .iter()
                            .any(|item| compare_values(item, &resolved) == Some(Ordering::Equal)),
                        InnerValue::Set(set) => set
                            .iter()
                            .any(|item| compare_values(item, &resolved) == Some(Ordering::Equal)),
                        _ => false,
                    }
                })
            }

            FilterNode::ContainsAnySet { field_path, values } => {
                let field_val = match resolve_field_ref(record, field_path) {
                    Some(v) => v,
                    None => return false,
                };
                match field_val {
                    InnerValue::List(list) => list.iter().any(|item| values.contains(item)),
                    InnerValue::Set(set) => set.iter().any(|item| values.contains(item)),
                    _ => false,
                }
            }

            FilterNode::ContainsAll { field_path, values } => {
                let field_val = match resolve_field_ref(record, field_path) {
                    Some(v) => v,
                    None => return false,
                };
                values.iter().all(|fv| {
                    let resolved = match resolve_filter_value(fv, record, ctx) {
                        Some(v) => v,
                        None => return false,
                    };
                    match field_val {
                        InnerValue::List(list) => list
                            .iter()
                            .any(|item| compare_values(item, &resolved) == Some(Ordering::Equal)),
                        InnerValue::Set(set) => set
                            .iter()
                            .any(|item| compare_values(item, &resolved) == Some(Ordering::Equal)),
                        _ => false,
                    }
                })
            }

            FilterNode::ContainsAllSet { field_path, values } => {
                let field_val = match resolve_field_ref(record, field_path) {
                    Some(v) => v,
                    None => return false,
                };
                // Count how many required values appear in the field array/set.
                // Pass when every required value was found (count == values.len()).
                let required = values.len();
                let found = match field_val {
                    InnerValue::List(list) => {
                        list.iter().filter(|item| values.contains(*item)).count()
                    }
                    InnerValue::Set(set) => {
                        set.iter().filter(|item| values.contains(*item)).count()
                    }
                    _ => return false,
                };
                found >= required
            }

            FilterNode::Between {
                field_path,
                from,
                to,
                pre_from,
                pre_to,
            } => {
                let field_val = match resolve_field_ref(record, field_path) {
                    Some(v) => v,
                    None => return false,
                };
                let owned_from;
                let from_val: &InnerValue = if let Some(pre) = pre_from {
                    pre
                } else {
                    owned_from = match resolve_filter_value(from, record, ctx) {
                        Some(v) => v,
                        None => return false,
                    };
                    &owned_from
                };
                let owned_to;
                let to_val: &InnerValue = if let Some(pre) = pre_to {
                    pre
                } else {
                    owned_to = match resolve_filter_value(to, record, ctx) {
                        Some(v) => v,
                        None => return false,
                    };
                    &owned_to
                };
                matches!(
                    compare_values(field_val, from_val),
                    Some(Ordering::Greater | Ordering::Equal)
                ) && matches!(
                    compare_values(field_val, to_val),
                    Some(Ordering::Less | Ordering::Equal)
                )
            }

            FilterNode::Exists { field_path } => resolve_field_ref(record, field_path).is_some(),
            FilterNode::NotExists { field_path } => resolve_field_ref(record, field_path).is_none(),

            FilterNode::FtsMatch {
                field_path,
                query_tokens,
                mode_and,
            } => {
                use shamir_types::types::value::InnerValue;
                let text = match resolve_field_ref(record, field_path) {
                    Some(InnerValue::Str(s)) => s,
                    _ => return false,
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
                let computed = match expr.eval(record) {
                    Ok(v) => v,
                    Err(_) => return false,
                };
                let owned_rhs;
                let rhs: &shamir_types::types::value::InnerValue = if let Some(pre) = pre_resolved {
                    pre
                } else {
                    owned_rhs = resolve_filter_value(value, record, ctx);
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

impl FilterCallback for FilterNode {
    fn matches(
        &self,
        record: &shamir_types::types::value::InnerValue,
        ctx: &FilterContext,
    ) -> bool {
        FilterNode::matches(self, record, ctx)
    }
}
