//! Filter evaluation — compile Filter AST into an enum-dispatched tree.
//!
//! The compiled tree is a `FilterNode` enum (static dispatch via `match`)
//! rather than `Box<dyn FilterCallback>` (virtual call per node). Each
//! `matches()` call walks the tree with monomorphic compares; the
//! compiler can inline the dispatch arms.

use std::cmp::Ordering;

use bytes::Bytes;
use regex::Regex;
use shamir_tx::predicate_set::PredicateDep;
use shamir_types::core::interner::{Interner, InternerKey};
use shamir_types::core::sort_codec;
use shamir_types::types::value::InnerValue;
use smallvec::SmallVec;

use super::eval_context::FilterContext;
use super::filter_callback::FilterCallback;
use crate::index::sorted_index_manager::SortedIndexManager;
use crate::query::filter::{Filter, FilterValue};
use crate::query::read::QueryResult;

/// Compact field-path representation for `FilterNode` variants.
/// Inline up to 4 segments (typical: `"name"` → 1, `"address.city"` → 2);
/// spills to heap for deeper paths. Replaces a `Vec<u64>` per compiled
/// node — saves a heap alloc + dereference on every `matches()` walk.
type CompactPath = SmallVec<[u64; 4]>;

// ============================================================================
// Utility functions
// ============================================================================

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
fn resolve_query_ref_value(qr: &QueryResult, path: Option<&str>) -> Option<InnerValue> {
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
    if rest.is_empty() {
        return json_to_inner_value(record);
    }
    let rest = rest.strip_prefix('.')?;
    let field_val = record.get(rest)?;
    json_to_inner_value(field_val)
}

/// Extract a column of values from all records in a QueryResult.
///
/// Supports `[].field` pattern — iterates all records, extracts `field` from each.
fn resolve_query_ref_column(qr: &QueryResult, path: Option<&str>) -> Vec<InnerValue> {
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
            let val = record.get(field)?;
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
fn resolve_json_path<'a>(
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

fn is_column_query_ref(fv: &FilterValue) -> bool {
    matches!(fv, FilterValue::QueryRef { path: Some(p), .. } if p.starts_with("[]"))
}

fn json_to_inner_value(v: &serde_json::Value) -> Option<InnerValue> {
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
        pre_resolved: Option<InnerValue>,
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
        pre_resolved: Vec<Option<InnerValue>>,
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
        pre_resolved: Option<InnerValue>,
    },
    ContainsAny {
        field_path: CompactPath,
        values: Vec<FilterValue>,
    },
    ContainsAll {
        field_path: CompactPath,
        values: Vec<FilterValue>,
    },
    Between {
        field_path: CompactPath,
        from: FilterValue,
        to: FilterValue,
        pre_from: Option<InnerValue>,
        pre_to: Option<InnerValue>,
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
        pre_resolved: Option<InnerValue>,
        op: CompareOp,
    },
}

impl FilterNode {
    pub fn matches(&self, record: &InnerValue, ctx: &FilterContext) -> bool {
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
                let rhs: &InnerValue = if let Some(pre) = pre_resolved {
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
    fn matches(&self, record: &InnerValue, ctx: &FilterContext) -> bool {
        FilterNode::matches(self, record, ctx)
    }
}

/// Convert a SQL LIKE pattern to a regex pattern.
/// `%` matches any sequence of characters, `_` matches a single character.
/// All other regex meta-characters are escaped.
fn like_pattern_to_regex(pattern: &str, case_insensitive: bool) -> Option<Regex> {
    let mut regex_str = String::with_capacity(pattern.len() + 4);
    if case_insensitive {
        regex_str.push_str("(?i)");
    }
    regex_str.push('^');
    for ch in pattern.chars() {
        match ch {
            '%' => regex_str.push_str(".*"),
            '_' => regex_str.push('.'),
            '.' | '+' | '*' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '\\' | '|' | '^' | '$' => {
                regex_str.push('\\');
                regex_str.push(ch);
            }
            _ => regex_str.push(ch),
        }
    }
    regex_str.push('$');
    Regex::new(&regex_str).ok()
}

// ============================================================================
// FTS brute-force helpers — inverted-loop word probe (no per-record alloc).
// ============================================================================

/// Compare `word` (raw, possibly mixed-case) against the pre-lowercased
/// query token at `idx`. Avoids allocating a fresh `String` per word on
/// the ASCII-already-lowercase fast path (the common case for English
/// corpora). Falls back to a full Unicode `to_lowercase()` allocation
/// only when the word actually contains uppercase characters.
#[inline]
fn fts_word_eq_token(word: &str, token: &str) -> bool {
    if word.is_ascii() {
        // ASCII fast path: bytewise compare with ASCII case folding.
        word.eq_ignore_ascii_case(token)
    } else {
        // Non-ASCII: must apply full Unicode lowercasing to preserve the
        // semantics of the previous `text.to_lowercase()` whole-string
        // pass. `str::to_lowercase` is per-char, so applying it to each
        // whitespace-split word produces the same bytes as folding the
        // whole string and then splitting.
        word.to_lowercase() == token
    }
}

/// AND-mode word probe (bitmask variant, up to 64 query tokens).
/// Returns whether the word matched anything; updates `seen` in place.
#[inline]
fn fts_word_matches(word: &str, query_tokens: &[String], seen: &mut u64) -> bool {
    let mut hit = false;
    for (i, t) in query_tokens.iter().enumerate() {
        let bit = 1u64 << i;
        if (*seen & bit) == 0 && fts_word_eq_token(word, t.as_str()) {
            *seen |= bit;
            hit = true;
        }
    }
    hit
}

/// AND-mode word probe (Vec<bool> variant, > 64 query tokens — rare).
#[inline]
fn fts_word_matches_vec(
    word: &str,
    query_tokens: &[String],
    seen: &mut [bool],
    remaining: &mut usize,
) -> bool {
    let mut hit = false;
    for (i, t) in query_tokens.iter().enumerate() {
        if !seen[i] && fts_word_eq_token(word, t.as_str()) {
            seen[i] = true;
            *remaining -= 1;
            hit = true;
        }
    }
    hit
}

/// OR-mode word probe: any-hit short-circuit.
#[inline]
fn fts_word_matches_or(word: &str, query_tokens: &[String]) -> bool {
    query_tokens
        .iter()
        .any(|t| fts_word_eq_token(word, t.as_str()))
}

// ============================================================================
// Compiler
// ============================================================================

/// Compile a Filter AST into a tree of `FilterNode` (static dispatch).
///
/// Field paths are resolved via the interner at compile time. If a field path
/// cannot be interned (field doesn't exist), the node folds to True/False.
pub fn compile_filter(filter: &Filter, interner: &Interner) -> FilterNode {
    match filter {
        Filter::Eq { field, value } => compile_compare(field, value, CompareOp::Eq, interner),
        Filter::Ne { field, value } => compile_compare(field, value, CompareOp::Ne, interner),
        Filter::Gt { field, value } => compile_compare(field, value, CompareOp::Gt, interner),
        Filter::Gte { field, value } => compile_compare(field, value, CompareOp::Gte, interner),
        Filter::Lt { field, value } => compile_compare(field, value, CompareOp::Lt, interner),
        Filter::Lte { field, value } => compile_compare(field, value, CompareOp::Lte, interner),
        Filter::FieldEq { field, value } => compile_compare(field, value, CompareOp::Eq, interner),

        Filter::And { filters } => FilterNode::And(
            filters
                .iter()
                .map(|f| compile_filter(f, interner))
                .collect(),
        ),
        Filter::Or { filters } => FilterNode::Or(
            filters
                .iter()
                .map(|f| compile_filter(f, interner))
                .collect(),
        ),
        Filter::Not { filter } => FilterNode::Not(Box::new(compile_filter(filter, interner))),

        Filter::IsNull { field } => match intern_field_path(field, interner) {
            Some(path) => FilterNode::IsNull {
                field_path: SmallVec::from_vec(path),
            },
            None => FilterNode::True,
        },
        Filter::IsNotNull { field } => match intern_field_path(field, interner) {
            Some(path) => FilterNode::IsNotNull {
                field_path: SmallVec::from_vec(path),
            },
            None => FilterNode::False,
        },

        Filter::In { field, values } => match intern_field_path(field, interner) {
            Some(path) => {
                let pre_resolved: Vec<Option<InnerValue>> =
                    values.iter().map(filter_value_to_inner).collect();
                FilterNode::In {
                    field_path: SmallVec::from_vec(path),
                    values: values.clone(),
                    pre_resolved,
                    negate: false,
                }
            }
            None => FilterNode::False,
        },
        Filter::NotIn { field, values } => match intern_field_path(field, interner) {
            Some(path) => {
                let pre_resolved: Vec<Option<InnerValue>> =
                    values.iter().map(filter_value_to_inner).collect();
                FilterNode::In {
                    field_path: SmallVec::from_vec(path),
                    values: values.clone(),
                    pre_resolved,
                    negate: true,
                }
            }
            None => FilterNode::True,
        },

        Filter::Like { field, pattern } => match intern_field_path(field, interner) {
            Some(path) => match like_pattern_to_regex(pattern, false) {
                Some(regex) => FilterNode::Like {
                    field_path: SmallVec::from_vec(path),
                    regex,
                },
                None => FilterNode::False,
            },
            None => FilterNode::False,
        },
        Filter::ILike { field, pattern } => match intern_field_path(field, interner) {
            Some(path) => match like_pattern_to_regex(pattern, true) {
                Some(regex) => FilterNode::Like {
                    field_path: SmallVec::from_vec(path),
                    regex,
                },
                None => FilterNode::False,
            },
            None => FilterNode::False,
        },
        Filter::Regex { field, pattern } => match intern_field_path(field, interner) {
            Some(path) => match Regex::new(pattern) {
                Ok(regex) => FilterNode::Regex {
                    field_path: SmallVec::from_vec(path),
                    regex,
                },
                Err(_) => FilterNode::False,
            },
            None => FilterNode::False,
        },
        Filter::Contains { field, value } => match intern_field_path(field, interner) {
            Some(path) => FilterNode::Contains {
                field_path: SmallVec::from_vec(path),
                pre_resolved: filter_value_to_inner(value),
                value: value.clone(),
            },
            None => FilterNode::False,
        },
        Filter::ContainsAny { field, values } => match intern_field_path(field, interner) {
            Some(path) => FilterNode::ContainsAny {
                field_path: SmallVec::from_vec(path),
                values: values.clone(),
            },
            None => FilterNode::False,
        },
        Filter::ContainsAll { field, values } => match intern_field_path(field, interner) {
            Some(path) => FilterNode::ContainsAll {
                field_path: SmallVec::from_vec(path),
                values: values.clone(),
            },
            None => FilterNode::False,
        },
        Filter::Between { field, from, to } => match intern_field_path(field, interner) {
            Some(path) => FilterNode::Between {
                field_path: SmallVec::from_vec(path),
                pre_from: filter_value_to_inner(from),
                pre_to: filter_value_to_inner(to),
                from: from.clone(),
                to: to.clone(),
            },
            None => FilterNode::False,
        },
        Filter::Exists { field } => match intern_field_path(field, interner) {
            Some(path) => FilterNode::Exists {
                field_path: SmallVec::from_vec(path),
            },
            None => FilterNode::False,
        },
        Filter::NotExists { field } => match intern_field_path(field, interner) {
            Some(path) => FilterNode::NotExists {
                field_path: SmallVec::from_vec(path),
            },
            None => FilterNode::True,
        },

        // Vector similarity cannot be brute-forced per-record
        // (would be O(n×dim) without an index). Planner must handle.
        Filter::VectorSimilarity { .. } => FilterNode::True,

        // FTS brute-force fallback (when no FTS index exists).
        Filter::Fts { field, query, mode } => match intern_field_path(field, interner) {
            Some(path) => FilterNode::FtsMatch {
                field_path: SmallVec::from_vec(path),
                query_tokens: query.split_whitespace().map(|w| w.to_lowercase()).collect(),
                mode_and: mode != "or",
            },
            None => FilterNode::False,
        },

        // Computed expression comparison.
        Filter::Computed {
            expr_op,
            field,
            cmp,
            value,
            expr_args,
        } => {
            let op = match cmp.as_str() {
                "eq" => CompareOp::Eq,
                "ne" => CompareOp::Ne,
                "gt" => CompareOp::Gt,
                "gte" => CompareOp::Gte,
                "lt" => CompareOp::Lt,
                "lte" => CompareOp::Lte,
                _ => return FilterNode::False,
            };
            match build_index_expr(expr_op, field, expr_args.as_deref(), interner) {
                Some(expr) => FilterNode::ComputedCompare {
                    expr: Box::new(expr),
                    pre_resolved: filter_value_to_inner(value),
                    value: value.clone(),
                    op,
                },
                None => FilterNode::False,
            }
        }
    }
}

fn build_index_expr(
    expr_op: &str,
    field: &[String],
    _expr_args: Option<&[FilterValue]>,
    interner: &Interner,
) -> Option<crate::index2::expr::IndexExpr> {
    use crate::index2::expr::IndexExpr;
    let path = intern_field_path(field, interner)?;
    let base = IndexExpr::Field(path);
    Some(match expr_op {
        "lower" => IndexExpr::Lower(Box::new(base)),
        "upper" => IndexExpr::Upper(Box::new(base)),
        "trim" => IndexExpr::Trim(Box::new(base)),
        "length" => IndexExpr::Length(Box::new(base)),
        "field" => base,
        _ => return None,
    })
}

fn compile_compare(
    field: &[String],
    value: &FilterValue,
    op: CompareOp,
    interner: &Interner,
) -> FilterNode {
    match intern_field_path(field, interner) {
        Some(path) => FilterNode::Compare {
            field_path: SmallVec::from_vec(path),
            pre_resolved: filter_value_to_inner(value),
            value: value.clone(),
            op,
        },
        None => FilterNode::False,
    }
}

// ============================================================================
// Phase C — predicate-to-index-range bridge (Step 2)
// ============================================================================

/// Encode a literal `FilterValue` into sort-codec bytes.
///
/// Returns `None` for non-literal / non-sortable variants.
fn encode_filter_value(v: &FilterValue) -> Option<Vec<u8>> {
    let mut buf = Vec::new();
    match v {
        FilterValue::Bool(b) => {
            sort_codec::encode_bool(&mut buf, *b);
            Some(buf)
        }
        FilterValue::Int(i) => {
            sort_codec::encode_i64(&mut buf, *i);
            Some(buf)
        }
        FilterValue::Float(x) => {
            sort_codec::encode_f64(&mut buf, *x).ok()?;
            Some(buf)
        }
        FilterValue::String(s) => {
            sort_codec::encode_str(&mut buf, s);
            Some(buf)
        }
        FilterValue::Binary(b) => {
            sort_codec::encode_bytes(&mut buf, b);
            Some(buf)
        }
        _ => None,
    }
}

/// Build the physical lower bound key: `SORTED_TAG || name_interned || enc`.
fn predicate_bound_lower(name_interned: u64, enc: &[u8]) -> std::ops::Bound<Bytes> {
    let mut k = Vec::with_capacity(9 + enc.len());
    k.push(shamir_tx::SORTED_TAG);
    k.extend_from_slice(&name_interned.to_be_bytes());
    k.extend_from_slice(enc);
    std::ops::Bound::Included(Bytes::from(k))
}

/// Build the physical upper bound key: `SORTED_TAG || name_interned || enc || 0xFF*16`.
fn predicate_bound_upper(name_interned: u64, enc: &[u8]) -> std::ops::Bound<Bytes> {
    let mut k = Vec::with_capacity(9 + enc.len() + 16);
    k.push(shamir_tx::SORTED_TAG);
    k.extend_from_slice(&name_interned.to_be_bytes());
    k.extend_from_slice(enc);
    k.extend_from_slice(&[0xFFu8; 16]); // tiebreak pad (matches range_bounds :536-537)
    std::ops::Bound::Included(Bytes::from(k))
}

/// Build the physical prefix-only bound (start of index keyspace).
fn predicate_bound_prefix(name_interned: u64) -> std::ops::Bound<Bytes> {
    let mut k = Vec::with_capacity(9);
    k.push(shamir_tx::SORTED_TAG);
    k.extend_from_slice(&name_interned.to_be_bytes());
    std::ops::Bound::Included(Bytes::from(k))
}

/// Build the full upper bound for the entire index: `SORTED_TAG || name_interned || 0xFF*64`.
fn predicate_bound_full_upper(name_interned: u64) -> std::ops::Bound<Bytes> {
    let mut k = Vec::with_capacity(9 + 64);
    k.push(shamir_tx::SORTED_TAG);
    k.extend_from_slice(&name_interned.to_be_bytes());
    k.extend_from_slice(&[0xFFu8; 64]); // matches range_bounds :541-543
    std::ops::Bound::Included(Bytes::from(k))
}

/// Try to derive one `PredicateDep::IndexRange` from a single leaf filter.
///
/// Returns `true` if a mapping was emitted; `false` if the filter cannot be
/// mapped to a sorted-index interval (caller falls back to `TableScan`).
fn predicate_handle_one(
    f: &Filter,
    sorted: &SortedIndexManager,
    interner: &Interner,
    table_token: u64,
    out: &mut smallvec::SmallVec<[PredicateDep; 2]>,
) -> bool {
    let (field, lo, hi): (
        &Vec<String>,
        std::ops::Bound<Vec<u8>>,
        std::ops::Bound<Vec<u8>>,
    ) = match f {
        Filter::Gt { field, value } | Filter::Gte { field, value } => {
            let enc = match encode_filter_value(value) {
                Some(e) => e,
                None => return false,
            };
            (
                field,
                std::ops::Bound::Included(enc),
                std::ops::Bound::Unbounded,
            )
        }
        Filter::Lt { field, value } | Filter::Lte { field, value } => {
            let enc = match encode_filter_value(value) {
                Some(e) => e,
                None => return false,
            };
            (
                field,
                std::ops::Bound::Unbounded,
                std::ops::Bound::Included(enc),
            )
        }
        Filter::Eq { field, value } | Filter::FieldEq { field, value } => {
            let enc = match encode_filter_value(value) {
                Some(e) => e,
                None => return false,
            };
            (
                field,
                std::ops::Bound::Included(enc.clone()),
                std::ops::Bound::Included(enc),
            )
        }
        Filter::Between { field, from, to } => {
            let lo = match encode_filter_value(from) {
                Some(e) => e,
                None => return false,
            };
            let hi = match encode_filter_value(to) {
                Some(e) => e,
                None => return false,
            };
            (
                field,
                std::ops::Bound::Included(lo),
                std::ops::Bound::Included(hi),
            )
        }
        _ => return false,
    };

    let path = match intern_field_path(field, interner) {
        Some(p) => p,
        None => return false,
    };
    let def = match sorted.find_by_field(&path) {
        Some(d) => d,
        None => return false,
    };
    let name = def.name_interned;

    let lo_b = match lo {
        std::ops::Bound::Included(e) => predicate_bound_lower(name, &e),
        std::ops::Bound::Unbounded => predicate_bound_prefix(name),
        std::ops::Bound::Excluded(_) => unreachable!(),
    };
    let hi_b = match hi {
        std::ops::Bound::Included(e) => predicate_bound_upper(name, &e),
        std::ops::Bound::Unbounded => predicate_bound_full_upper(name),
        std::ops::Bound::Excluded(_) => unreachable!(),
    };
    out.push(PredicateDep::IndexRange {
        table_token,
        index_id: name,
        lo: lo_b,
        hi: hi_b,
    });
    true
}

/// Derive zero or more `PredicateDep` from a `Filter` AST node.
///
/// Uses the table's sorted indexes to build precise byte-level intervals
/// where possible; returns an empty `SmallVec` when the filter cannot be
/// mapped (caller must fall back to a coarse `TableScan`).
///
/// For `And`: emits per-conjunct ranges for those that map; if ANY
/// conjunct fails to map, clears all precise ranges (safe over-lock:
/// the caller emits a single `TableScan` instead).
pub fn predicate_to_index_range(
    f: &Filter,
    sorted: &SortedIndexManager,
    interner: &Interner,
    table_token: u64,
) -> smallvec::SmallVec<[PredicateDep; 2]> {
    let mut out: smallvec::SmallVec<[PredicateDep; 2]> = smallvec::SmallVec::new();

    match f {
        Filter::And { filters } => {
            let mut all_mapped = true;
            for child in filters {
                if !predicate_handle_one(child, sorted, interner, table_token, &mut out) {
                    all_mapped = false;
                }
            }
            if !all_mapped {
                // Safe over-lock: drop precise parts and let caller emit TableScan.
                out.clear();
            }
        }
        // Coarse: cannot map to a precise index range.
        Filter::Or { .. }
        | Filter::Not { .. }
        | Filter::Regex { .. }
        | Filter::Like { .. }
        | Filter::ILike { .. }
        | Filter::Computed { .. }
        | Filter::Fts { .. }
        | Filter::VectorSimilarity { .. }
        | Filter::In { .. }
        | Filter::NotIn { .. }
        | Filter::Contains { .. }
        | Filter::ContainsAny { .. }
        | Filter::ContainsAll { .. }
        | Filter::IsNull { .. }
        | Filter::IsNotNull { .. }
        | Filter::Exists { .. }
        | Filter::NotExists { .. }
        | Filter::Ne { .. } => {
            // Return empty → caller records TableScan.
        }
        // Single leaf filter.
        other => {
            predicate_handle_one(other, sorted, interner, table_token, &mut out);
        }
    }
    out
}
