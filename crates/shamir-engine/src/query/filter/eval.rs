//! Filter evaluation — compile Filter AST into a callback network.
//!
//! Each node in the compiled tree implements `FilterCallback::matches()`,
//! which takes a record and context and returns a boolean.

use std::cmp::Ordering;

use regex::Regex;

use shamir_types::core::interner::{Interner, InternerKey};
use crate::query::filter::{Filter, FilterValue};
use crate::query::read::QueryResult;
use shamir_types::types::value::InnerValue;

use super::eval_context::FilterContext;

// ============================================================================
// Trait
// ============================================================================

/// Trait for compiled filter nodes. Each node evaluates a record against its predicate.
pub trait FilterCallback: Send + Sync {
    fn matches(&self, record: &InnerValue, ctx: &FilterContext) -> bool;
}

// ============================================================================
// Utility functions
// ============================================================================

/// Extract a value from an InnerValue by a path of interned keys.
///
/// Borrowing variant: walks the record in-place without cloning the
/// resolved leaf. All filter callbacks below use this — the old
/// owned variant survives only for callers outside the eval module
/// that still rely on `Option<InnerValue>`.
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
            // Strip the optional `@` prefix — the spec/diagrams show
            // `{ "$query": "@user", "path": "[0].id" }` while queries
            // map keys are bare (`{ user: ... }`).
            let key = alias.strip_prefix('@').unwrap_or(alias.as_str());
            let qr = ctx.resolved_refs.get(key)?;
            resolve_query_ref_value(qr, path.as_deref())
        }
        // FnCall, Expr, Cond — not yet supported in eval
        _ => None,
    }
}

/// Extract a value from a QueryResult by a simple path like "[0].id".
fn resolve_query_ref_value(
    qr: &QueryResult,
    path: Option<&str>,
) -> Option<InnerValue> {
    let path = path?;
    // Parse "[N].field" pattern
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

    // Strip leading dot
    let rest = rest.strip_prefix('.')?;
    // Navigate into the JSON value
    let field_val = record.get(rest)?;
    json_to_inner_value(field_val)
}

/// Extract a column of values from all records in a QueryResult.
///
/// Supports `[].field` pattern — iterates all records, extracts `field` from each.
fn resolve_query_ref_column(
    qr: &QueryResult,
    path: Option<&str>,
) -> Vec<InnerValue> {
    let path = match path {
        Some(p) => p,
        None => return Vec::new(),
    };
    // Parse "[].field" pattern
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

/// Check if a QueryRef path uses the column selector pattern `[]`.
fn is_column_query_ref(fv: &FilterValue) -> bool {
    matches!(fv, FilterValue::QueryRef { path: Some(p), .. } if p.starts_with("[]"))
}

/// Convert a serde_json::Value into an InnerValue (simple scalar conversion).
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
// Callback structs
// ============================================================================

/// Comparison callback (Eq, Ne, Gt, Gte, Lt, Lte)
struct CompareCallback {
    field_path: Vec<u64>,
    value: FilterValue,
    /// Pre-resolved at compile time when `value` is a literal
    /// (Null/Bool/Int/Float/String/Binary). Allocations for the RHS
    /// were the second source of per-record cost on the filter hot
    /// loop (the first being the leaf clone closed in 655ba4c).
    /// FieldRef/QueryRef leave this `None` — those still need
    /// per-record resolution against the record/ctx.
    pre_resolved: Option<InnerValue>,
    op: CompareOp,
}

#[derive(Debug, Clone, Copy)]
enum CompareOp {
    Eq,
    Ne,
    Gt,
    Gte,
    Lt,
    Lte,
}

impl FilterCallback for CompareCallback {
    fn matches(&self, record: &InnerValue, ctx: &FilterContext) -> bool {
        let field_val = resolve_field_ref(record, &self.field_path);
        let owned_rhs;
        let filter_val: Option<&InnerValue> = if let Some(pre) = &self.pre_resolved {
            Some(pre)
        } else {
            owned_rhs = resolve_filter_value(&self.value, record, ctx);
            owned_rhs.as_ref()
        };

        match (field_val, filter_val) {
            (Some(a), Some(b)) => match self.op {
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
            // If either side is None, comparison fails (except Ne — missing != something is true)
            (None, _) | (_, None) => matches!(self.op, CompareOp::Ne),
        }
    }
}

/// And — all children must match
struct AndCallback {
    children: Vec<Box<dyn FilterCallback>>,
}

impl FilterCallback for AndCallback {
    fn matches(&self, record: &InnerValue, ctx: &FilterContext) -> bool {
        self.children.iter().all(|c| c.matches(record, ctx))
    }
}

/// Or — at least one child must match
struct OrCallback {
    children: Vec<Box<dyn FilterCallback>>,
}

impl FilterCallback for OrCallback {
    fn matches(&self, record: &InnerValue, ctx: &FilterContext) -> bool {
        self.children.iter().any(|c| c.matches(record, ctx))
    }
}

/// Not — invert inner
struct NotCallback {
    inner: Box<dyn FilterCallback>,
}

impl FilterCallback for NotCallback {
    fn matches(&self, record: &InnerValue, ctx: &FilterContext) -> bool {
        !self.inner.matches(record, ctx)
    }
}

/// IsNull — field is missing or Null
struct IsNullCallback {
    field_path: Vec<u64>,
}

impl FilterCallback for IsNullCallback {
    fn matches(&self, record: &InnerValue, _ctx: &FilterContext) -> bool {
        matches!(
            resolve_field_ref(record, &self.field_path),
            None | Some(InnerValue::Null)
        )
    }
}

/// IsNotNull — field exists and is not Null
struct IsNotNullCallback {
    field_path: Vec<u64>,
}

impl FilterCallback for IsNotNullCallback {
    fn matches(&self, record: &InnerValue, _ctx: &FilterContext) -> bool {
        !matches!(
            resolve_field_ref(record, &self.field_path),
            None | Some(InnerValue::Null)
        )
    }
}

/// In — field value must be in the resolved values list
struct InCallback {
    field_path: Vec<u64>,
    values: Vec<FilterValue>,
    negate: bool,
}

impl FilterCallback for InCallback {
    fn matches(&self, record: &InnerValue, ctx: &FilterContext) -> bool {
        let field_val = match resolve_field_ref(record, &self.field_path) {
            Some(v) => v,
            None => return self.negate, // missing field: not in any set
        };

        let found = self.values.iter().any(|fv| {
            // Column QueryRef: expand to all values from the query result
            if is_column_query_ref(fv) {
                if let FilterValue::QueryRef { alias, path } = fv {
                    // Strip optional `@` — same convention as scalar refs.
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
            // Single value
            match resolve_filter_value(fv, record, ctx) {
                Some(resolved) => compare_values(field_val, &resolved) == Some(Ordering::Equal),
                None => false,
            }
        });

        if self.negate { !found } else { found }
    }
}

/// Always-true callback
struct TrueCallback;

impl FilterCallback for TrueCallback {
    fn matches(&self, _record: &InnerValue, _ctx: &FilterContext) -> bool {
        true
    }
}

/// Always-false callback for unresolvable field paths
struct FalseCallback;

impl FilterCallback for FalseCallback {
    fn matches(&self, _record: &InnerValue, _ctx: &FilterContext) -> bool {
        false
    }
}

/// Like — SQL-like pattern matching (% = any chars, _ = single char)
struct LikeCallback {
    field_path: Vec<u64>,
    regex: Regex,
}

impl FilterCallback for LikeCallback {
    fn matches(&self, record: &InnerValue, _ctx: &FilterContext) -> bool {
        match resolve_field_ref(record, &self.field_path) {
            Some(InnerValue::Str(s)) => self.regex.is_match(s),
            _ => false,
        }
    }
}

/// Regex — regex pattern matching on string fields
struct RegexCallback {
    field_path: Vec<u64>,
    regex: Regex,
}

impl FilterCallback for RegexCallback {
    fn matches(&self, record: &InnerValue, _ctx: &FilterContext) -> bool {
        match resolve_field_ref(record, &self.field_path) {
            Some(InnerValue::Str(s)) => self.regex.is_match(s),
            _ => false,
        }
    }
}

/// Contains — for List/Set: check if collection contains the value.
/// For Str: check if string contains substring.
struct ContainsCallback {
    field_path: Vec<u64>,
    value: FilterValue,
    pre_resolved: Option<InnerValue>,
}

impl FilterCallback for ContainsCallback {
    fn matches(&self, record: &InnerValue, ctx: &FilterContext) -> bool {
        let field_val = match resolve_field_ref(record, &self.field_path) {
            Some(v) => v,
            None => return false,
        };
        let owned_rhs;
        let filter_val: &InnerValue = if let Some(pre) = &self.pre_resolved {
            pre
        } else {
            owned_rhs = match resolve_filter_value(&self.value, record, ctx) {
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
}

/// ContainsAny — collection contains at least one of the values
struct ContainsAnyCallback {
    field_path: Vec<u64>,
    values: Vec<FilterValue>,
}

impl FilterCallback for ContainsAnyCallback {
    fn matches(&self, record: &InnerValue, ctx: &FilterContext) -> bool {
        let field_val = match resolve_field_ref(record, &self.field_path) {
            Some(v) => v,
            None => return false,
        };
        self.values.iter().any(|fv| {
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
}

/// ContainsAll — collection contains all of the values
struct ContainsAllCallback {
    field_path: Vec<u64>,
    values: Vec<FilterValue>,
}

impl FilterCallback for ContainsAllCallback {
    fn matches(&self, record: &InnerValue, ctx: &FilterContext) -> bool {
        let field_val = match resolve_field_ref(record, &self.field_path) {
            Some(v) => v,
            None => return false,
        };
        self.values.iter().all(|fv| {
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
}

/// Between — field >= from AND field <= to
struct BetweenCallback {
    field_path: Vec<u64>,
    from: FilterValue,
    to: FilterValue,
    pre_from: Option<InnerValue>,
    pre_to: Option<InnerValue>,
}

impl FilterCallback for BetweenCallback {
    fn matches(&self, record: &InnerValue, ctx: &FilterContext) -> bool {
        let field_val = match resolve_field_ref(record, &self.field_path) {
            Some(v) => v,
            None => return false,
        };
        let owned_from;
        let from_val: &InnerValue = if let Some(pre) = &self.pre_from {
            pre
        } else {
            owned_from = match resolve_filter_value(&self.from, record, ctx) {
                Some(v) => v,
                None => return false,
            };
            &owned_from
        };
        let owned_to;
        let to_val: &InnerValue = if let Some(pre) = &self.pre_to {
            pre
        } else {
            owned_to = match resolve_filter_value(&self.to, record, ctx) {
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
}

/// Exists — field is present in the record (resolve_field returns Some)
struct ExistsCallback {
    field_path: Vec<u64>,
}

impl FilterCallback for ExistsCallback {
    fn matches(&self, record: &InnerValue, _ctx: &FilterContext) -> bool {
        resolve_field_ref(record, &self.field_path).is_some()
    }
}

/// NotExists — field is not present in the record
struct NotExistsCallback {
    field_path: Vec<u64>,
}

impl FilterCallback for NotExistsCallback {
    fn matches(&self, record: &InnerValue, _ctx: &FilterContext) -> bool {
        resolve_field_ref(record, &self.field_path).is_none()
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
            // Escape regex meta-characters
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

/// Compile a Filter AST into a tree of callbacks.
///
/// Field paths are resolved via the interner at compile time.
/// If a field path cannot be interned (field doesn't exist), the callback
/// will always return false for comparisons.
pub fn compile_filter(filter: &Filter, interner: &Interner) -> Box<dyn FilterCallback> {
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

        Filter::And { filters } => {
            let children = filters
                .iter()
                .map(|f| compile_filter(f, interner))
                .collect();
            Box::new(AndCallback { children })
        }
        Filter::Or { filters } => {
            let children = filters
                .iter()
                .map(|f| compile_filter(f, interner))
                .collect();
            Box::new(OrCallback { children })
        }
        Filter::Not { filter } => {
            let inner = compile_filter(filter, interner);
            Box::new(NotCallback { inner })
        }

        Filter::IsNull { field } => match intern_field_path(field, interner) {
            Some(path) => Box::new(IsNullCallback { field_path: path }),
            // If field path can't be interned, the field doesn't exist => always null
            None => Box::new(TrueCallback),
        },
        Filter::IsNotNull { field } => match intern_field_path(field, interner) {
            Some(path) => Box::new(IsNotNullCallback { field_path: path }),
            None => Box::new(FalseCallback),
        },

        Filter::In { field, values } => match intern_field_path(field, interner) {
            Some(path) => Box::new(InCallback {
                field_path: path,
                values: values.clone(),
                negate: false,
            }),
            None => Box::new(FalseCallback),
        },
        Filter::NotIn { field, values } => match intern_field_path(field, interner) {
            Some(path) => Box::new(InCallback {
                field_path: path,
                values: values.clone(),
                negate: true,
            }),
            None => Box::new(TrueCallback),
        },

        Filter::Like { field, pattern } => match intern_field_path(field, interner) {
            Some(path) => match like_pattern_to_regex(pattern, false) {
                Some(regex) => Box::new(LikeCallback {
                    field_path: path,
                    regex,
                }),
                None => Box::new(FalseCallback),
            },
            None => Box::new(FalseCallback),
        },
        Filter::ILike { field, pattern } => match intern_field_path(field, interner) {
            Some(path) => match like_pattern_to_regex(pattern, true) {
                Some(regex) => Box::new(LikeCallback {
                    field_path: path,
                    regex,
                }),
                None => Box::new(FalseCallback),
            },
            None => Box::new(FalseCallback),
        },
        Filter::Regex { field, pattern } => match intern_field_path(field, interner) {
            Some(path) => match Regex::new(pattern) {
                Ok(regex) => Box::new(RegexCallback {
                    field_path: path,
                    regex,
                }),
                Err(_) => Box::new(FalseCallback),
            },
            None => Box::new(FalseCallback),
        },
        Filter::Contains { field, value } => match intern_field_path(field, interner) {
            Some(path) => Box::new(ContainsCallback {
                field_path: path,
                pre_resolved: filter_value_to_inner(value),
                value: value.clone(),
            }),
            None => Box::new(FalseCallback),
        },
        Filter::ContainsAny { field, values } => match intern_field_path(field, interner) {
            Some(path) => Box::new(ContainsAnyCallback {
                field_path: path,
                values: values.clone(),
            }),
            None => Box::new(FalseCallback),
        },
        Filter::ContainsAll { field, values } => match intern_field_path(field, interner) {
            Some(path) => Box::new(ContainsAllCallback {
                field_path: path,
                values: values.clone(),
            }),
            None => Box::new(FalseCallback),
        },
        Filter::Between { field, from, to } => match intern_field_path(field, interner) {
            Some(path) => Box::new(BetweenCallback {
                field_path: path,
                pre_from: filter_value_to_inner(from),
                pre_to: filter_value_to_inner(to),
                from: from.clone(),
                to: to.clone(),
            }),
            None => Box::new(FalseCallback),
        },
        Filter::Exists { field } => match intern_field_path(field, interner) {
            Some(path) => Box::new(ExistsCallback { field_path: path }),
            // If the field path can't be interned, the field doesn't exist => always false
            None => Box::new(FalseCallback),
        },
        Filter::NotExists { field } => match intern_field_path(field, interner) {
            Some(path) => Box::new(NotExistsCallback { field_path: path }),
            // If the field path can't be interned, the field doesn't exist => always true
            None => Box::new(TrueCallback),
        },
    }
}

fn compile_compare(
    field: &[String],
    value: &FilterValue,
    op: CompareOp,
    interner: &Interner,
) -> Box<dyn FilterCallback> {
    match intern_field_path(field, interner) {
        Some(path) => Box::new(CompareCallback {
            field_path: path,
            pre_resolved: filter_value_to_inner(value),
            value: value.clone(),
            op,
        }),
        None => Box::new(FalseCallback),
    }
}
