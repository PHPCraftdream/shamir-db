//! Filter evaluation — compile Filter AST into an enum-dispatched tree.
//!
//! The compiled tree is a `FilterNode` enum (static dispatch via `match`)
//! rather than `Box<dyn FilterCallback>` (virtual call per node). Each
//! `matches()` call walks the tree with monomorphic compares; the
//! compiler can inline the dispatch arms.

use std::cmp::Ordering;

use regex::Regex;

use shamir_types::core::interner::{Interner, InternerKey};
use crate::query::filter::{Filter, FilterValue};
use crate::query::read::QueryResult;
use shamir_types::types::value::InnerValue;

use super::eval_context::FilterContext;

// ============================================================================
// Trait — kept as a thin compat shim. `FilterNode` implements it so callers
// that still ask for `&dyn FilterCallback` keep working; new code uses
// `&FilterNode` directly.
// ============================================================================

pub trait FilterCallback: Send + Sync {
    fn matches(&self, record: &InnerValue, ctx: &FilterContext) -> bool;
}

// ============================================================================
// Utility functions
// ============================================================================

/// Extract a value from an InnerValue by a path of interned keys.
///
/// Borrowing variant: walks the record in-place without cloning the
/// resolved leaf. All filter nodes below use this — the old owned
/// variant survives only for callers outside the eval module that
/// still rely on `Option<InnerValue>`.
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
fn resolve_filter_value(
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
        _ => None,
    }
}

/// Extract a value from a QueryResult by a simple path like "[0].id".
fn resolve_query_ref_value(qr: &QueryResult, path: Option<&str>) -> Option<InnerValue> {
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
// FilterNode — enum-dispatched compiled filter
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
        field_path: Vec<u64>,
        value: FilterValue,
        /// Pre-resolved at compile time when `value` is a literal.
        pre_resolved: Option<InnerValue>,
        op: CompareOp,
    },
    And(Vec<FilterNode>),
    Or(Vec<FilterNode>),
    Not(Box<FilterNode>),
    IsNull {
        field_path: Vec<u64>,
    },
    IsNotNull {
        field_path: Vec<u64>,
    },
    In {
        field_path: Vec<u64>,
        values: Vec<FilterValue>,
        negate: bool,
    },
    Like {
        field_path: Vec<u64>,
        regex: Regex,
    },
    Regex {
        field_path: Vec<u64>,
        regex: Regex,
    },
    Contains {
        field_path: Vec<u64>,
        value: FilterValue,
        pre_resolved: Option<InnerValue>,
    },
    ContainsAny {
        field_path: Vec<u64>,
        values: Vec<FilterValue>,
    },
    ContainsAll {
        field_path: Vec<u64>,
        values: Vec<FilterValue>,
    },
    Between {
        field_path: Vec<u64>,
        from: FilterValue,
        to: FilterValue,
        pre_from: Option<InnerValue>,
        pre_to: Option<InnerValue>,
    },
    Exists {
        field_path: Vec<u64>,
    },
    NotExists {
        field_path: Vec<u64>,
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
                        CompareOp::Lte => matches!(
                            compare_values(a, b),
                            Some(Ordering::Less | Ordering::Equal)
                        ),
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
                negate,
            } => {
                let field_val = match resolve_field_ref(record, field_path) {
                    Some(v) => v,
                    None => return *negate,
                };
                let found = values.iter().any(|fv| {
                    if is_column_query_ref(fv) {
                        if let FilterValue::QueryRef { alias, path } = fv {
                            let key = alias.strip_prefix('@').unwrap_or(alias.as_str());
                            if let Some(qr) = ctx.resolved_refs.get(key) {
                                let column = resolve_query_ref_column(qr, path.as_deref());
                                return column
                                    .iter()
                                    .any(|cv| compare_values(field_val, cv) == Some(Ordering::Equal));
                            }
                        }
                        return false;
                    }
                    match resolve_filter_value(fv, record, ctx) {
                        Some(resolved) => compare_values(field_val, &resolved) == Some(Ordering::Equal),
                        None => false,
                    }
                });
                if *negate { !found } else { found }
            }

            FilterNode::Like { field_path, regex }
            | FilterNode::Regex { field_path, regex } => match resolve_field_ref(record, field_path) {
                Some(InnerValue::Str(s)) => regex.is_match(s),
                _ => false,
            },

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

            FilterNode::Exists { field_path } => {
                resolve_field_ref(record, field_path).is_some()
            }
            FilterNode::NotExists { field_path } => {
                resolve_field_ref(record, field_path).is_none()
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
            '.' | '+' | '*' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '\\' | '|' | '^'
            | '$' => {
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
        Filter::FieldEq { field, value } => {
            compile_compare(field, value, CompareOp::Eq, interner)
        }

        Filter::And { filters } => FilterNode::And(
            filters.iter().map(|f| compile_filter(f, interner)).collect(),
        ),
        Filter::Or { filters } => FilterNode::Or(
            filters.iter().map(|f| compile_filter(f, interner)).collect(),
        ),
        Filter::Not { filter } => FilterNode::Not(Box::new(compile_filter(filter, interner))),

        Filter::IsNull { field } => match intern_field_path(field, interner) {
            Some(path) => FilterNode::IsNull { field_path: path },
            None => FilterNode::True,
        },
        Filter::IsNotNull { field } => match intern_field_path(field, interner) {
            Some(path) => FilterNode::IsNotNull { field_path: path },
            None => FilterNode::False,
        },

        Filter::In { field, values } => match intern_field_path(field, interner) {
            Some(path) => FilterNode::In {
                field_path: path,
                values: values.clone(),
                negate: false,
            },
            None => FilterNode::False,
        },
        Filter::NotIn { field, values } => match intern_field_path(field, interner) {
            Some(path) => FilterNode::In {
                field_path: path,
                values: values.clone(),
                negate: true,
            },
            None => FilterNode::True,
        },

        Filter::Like { field, pattern } => match intern_field_path(field, interner) {
            Some(path) => match like_pattern_to_regex(pattern, false) {
                Some(regex) => FilterNode::Like {
                    field_path: path,
                    regex,
                },
                None => FilterNode::False,
            },
            None => FilterNode::False,
        },
        Filter::ILike { field, pattern } => match intern_field_path(field, interner) {
            Some(path) => match like_pattern_to_regex(pattern, true) {
                Some(regex) => FilterNode::Like {
                    field_path: path,
                    regex,
                },
                None => FilterNode::False,
            },
            None => FilterNode::False,
        },
        Filter::Regex { field, pattern } => match intern_field_path(field, interner) {
            Some(path) => match Regex::new(pattern) {
                Ok(regex) => FilterNode::Regex {
                    field_path: path,
                    regex,
                },
                Err(_) => FilterNode::False,
            },
            None => FilterNode::False,
        },
        Filter::Contains { field, value } => match intern_field_path(field, interner) {
            Some(path) => FilterNode::Contains {
                field_path: path,
                pre_resolved: filter_value_to_inner(value),
                value: value.clone(),
            },
            None => FilterNode::False,
        },
        Filter::ContainsAny { field, values } => match intern_field_path(field, interner) {
            Some(path) => FilterNode::ContainsAny {
                field_path: path,
                values: values.clone(),
            },
            None => FilterNode::False,
        },
        Filter::ContainsAll { field, values } => match intern_field_path(field, interner) {
            Some(path) => FilterNode::ContainsAll {
                field_path: path,
                values: values.clone(),
            },
            None => FilterNode::False,
        },
        Filter::Between { field, from, to } => match intern_field_path(field, interner) {
            Some(path) => FilterNode::Between {
                field_path: path,
                pre_from: filter_value_to_inner(from),
                pre_to: filter_value_to_inner(to),
                from: from.clone(),
                to: to.clone(),
            },
            None => FilterNode::False,
        },
        Filter::Exists { field } => match intern_field_path(field, interner) {
            Some(path) => FilterNode::Exists { field_path: path },
            None => FilterNode::False,
        },
        Filter::NotExists { field } => match intern_field_path(field, interner) {
            Some(path) => FilterNode::NotExists { field_path: path },
            None => FilterNode::True,
        },
    }
}

fn compile_compare(
    field: &[String],
    value: &FilterValue,
    op: CompareOp,
    interner: &Interner,
) -> FilterNode {
    match intern_field_path(field, interner) {
        Some(path) => FilterNode::Compare {
            field_path: path,
            pre_resolved: filter_value_to_inner(value),
            value: value.clone(),
            op,
        },
        None => FilterNode::False,
    }
}
