//! Filter evaluation — compile Filter AST into a callback network.
//!
//! Each node in the compiled tree implements `FilterCallback::matches()`,
//! which takes a record and context and returns a boolean.

use std::cmp::Ordering;

use crate::core::interner::{Interner, InternerKey};
use crate::db::query::filter::{Filter, FilterValue};
use crate::db::query::read::QueryResult;
use crate::types::value::InnerValue;

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
/// Similar to `IndexManager::extract_value_by_path` but standalone.
pub fn resolve_field(record: &InnerValue, path: &[u64]) -> Option<InnerValue> {
    if path.is_empty() {
        return Some(record.clone());
    }

    match record {
        InnerValue::Map(map) => {
            let key = InternerKey::new(path[0]);
            let next_value = map.get(&key)?;
            if path.len() == 1 {
                Some(next_value.clone())
            } else {
                resolve_field(next_value, &path[1..])
            }
        }
        _ => None,
    }
}

/// Resolve a field path string (e.g. "user.email") into interned u64 keys.
pub fn intern_field_path(field: &str, interner: &Interner) -> Option<Vec<u64>> {
    let mut keys = Vec::new();
    for part in field.split('.') {
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
            let qr = ctx.resolved_refs.get(alias.as_str())?;
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
        let field_val = resolve_field(record, &self.field_path);
        let filter_val = resolve_filter_value(&self.value, record, ctx);

        match (field_val, filter_val) {
            (Some(a), Some(b)) => match self.op {
                CompareOp::Eq => compare_values(&a, &b) == Some(Ordering::Equal),
                CompareOp::Ne => compare_values(&a, &b) != Some(Ordering::Equal),
                CompareOp::Gt => compare_values(&a, &b) == Some(Ordering::Greater),
                CompareOp::Gte => matches!(
                    compare_values(&a, &b),
                    Some(Ordering::Greater | Ordering::Equal)
                ),
                CompareOp::Lt => compare_values(&a, &b) == Some(Ordering::Less),
                CompareOp::Lte => matches!(
                    compare_values(&a, &b),
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
            resolve_field(record, &self.field_path),
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
            resolve_field(record, &self.field_path),
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
        let field_val = match resolve_field(record, &self.field_path) {
            Some(v) => v,
            None => return self.negate, // missing field: not in any set
        };

        let found = self.values.iter().any(|fv| {
            // Column QueryRef: expand to all values from the query result
            if is_column_query_ref(fv) {
                if let FilterValue::QueryRef { alias, path } = fv {
                    if let Some(qr) = ctx.resolved_refs.get(alias.as_str()) {
                        let column = resolve_query_ref_column(qr, path.as_deref());
                        return column
                            .iter()
                            .any(|cv| compare_values(&field_val, cv) == Some(Ordering::Equal));
                    }
                }
                return false;
            }
            // Single value
            match resolve_filter_value(fv, record, ctx) {
                Some(resolved) => compare_values(&field_val, &resolved) == Some(Ordering::Equal),
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

        // Not yet implemented — always false
        Filter::Like { .. }
        | Filter::ILike { .. }
        | Filter::Regex { .. }
        | Filter::Contains { .. }
        | Filter::ContainsAny { .. }
        | Filter::ContainsAll { .. }
        | Filter::Between { .. }
        | Filter::Exists { .. }
        | Filter::NotExists { .. } => Box::new(FalseCallback),
    }
}

fn compile_compare(
    field: &str,
    value: &FilterValue,
    op: CompareOp,
    interner: &Interner,
) -> Box<dyn FilterCallback> {
    match intern_field_path(field, interner) {
        Some(path) => Box::new(CompareCallback {
            field_path: path,
            value: value.clone(),
            op,
        }),
        None => Box::new(FalseCallback),
    }
}
