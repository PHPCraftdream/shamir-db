//! [`Doc`] — record-value builder.

use crate::write::builder_error::BuilderError;
use shamir_query_types::filter::{
    check_filter_depth_at, filter_value_to_query_value, FilterValue, MAX_FILTER_DEPTH,
};
use shamir_types::types::common::{new_map, TMap};
use shamir_types::types::value::QueryValue;

/// A record-value builder that produces a [`QueryValue::Map`].
///
/// Field values are either literals or computed expressions — both go
/// through [`Doc::set`], which accepts any `impl Into<FilterValue>`.
/// Literals (`i32`, `&str`, `bool`, etc.) and expressions (`col(...)`,
/// `func(...)`, `qref(...)`) all implement `Into<FilterValue>`.
///
/// For a nested map that you have already assembled as a `QueryValue`, use
/// [`Doc::set_value`].
///
/// Insertion order is preserved (backed by an `IndexMap`).
#[derive(Debug, Clone, Default)]
pub struct Doc {
    fields: TMap<String, QueryValue>,
}

/// Create an empty [`Doc`].
pub fn doc() -> Doc {
    Doc::new()
}

impl Doc {
    /// Create an empty document.
    pub fn new() -> Self {
        Self { fields: new_map() }
    }

    /// Set a field to a literal value or a computed expression.
    ///
    /// Accepts anything that converts `Into<FilterValue>`:
    /// - Literals: `i8`..`i64`, `u8`..`u32`, `f32`, `f64`, `bool`,
    ///   `&str`, `String`.
    /// - Expressions: `col(...)`, `func(...)`, `qref(...)`, `qref_all(...)`.
    ///
    /// Literal values (scalars, binary, arrays) are converted directly via
    /// [`filter_value_to_query_value`] — no serialization round-trip on the
    /// per-row hot path (RV-QUERY-BUILDER-7B79D840). Expression markers
    /// (`col(...)`, `func(...)`, ...) still go through the msgpack
    /// round-trip, which is fallible and mapped to [`BuilderError`]
    /// (RV-QUERY-BUILDER-20D9732A) instead of panicking.
    ///
    /// # Errors
    ///
    /// Returns [`BuilderError::FieldValueCodec`] when a value nested deeper
    /// than `MAX_FILTER_DEPTH` is supplied, or when the expression-marker
    /// fallback round-trip fails to decode — both previously `.expect()`ed
    /// panics.
    pub fn set(
        mut self,
        key: impl Into<String>,
        value: impl Into<FilterValue>,
    ) -> Result<Self, BuilderError> {
        let fv: FilterValue = value.into();
        // Local depth guard shared by both paths: the converter recurses
        // unboundedly, and the msgpack fallback round-trip has no decoder
        // depth budget either — cap both at MAX_FILTER_DEPTH so pathological
        // nesting surfaces as a typed error, not a stack overflow or panic.
        // Bounded early-cutoff traversal: descends into expression markers
        // (`$fn`/`$expr`/`$cond`) too, so deep trees are rejected here and
        // never reach the recursive msgpack serialization.
        if !filter_value_is_shallow_enough(&fv) {
            return Err(BuilderError::FieldValueCodec(format!(
                "field {:?}: value nesting exceeds MAX_FILTER_DEPTH ({MAX_FILTER_DEPTH})",
                key.into()
            )));
        }
        // Fast path (RV-QUERY-BUILDER-7B79D840): literals convert directly.
        let qv = match filter_value_to_query_value(&fv) {
            Some(qv) => qv,
            None => {
                // Expression markers etc.: FilterValue and QueryValue share
                // the same serde wire encoding; fall back to the msgpack
                // round-trip, mapping codec failures to a typed error
                // (RV-QUERY-BUILDER-20D9732A) instead of `.expect()`-panicking.
                let bytes = rmp_serde::to_vec_named(&fv).map_err(|e| {
                    BuilderError::FieldValueCodec(format!("msgpack encode failed: {e}"))
                })?;
                rmp_serde::from_slice(&bytes).map_err(|e| {
                    BuilderError::FieldValueCodec(format!(
                        "FilterValue→QueryValue round-trip failed: {e}"
                    ))
                })?
            }
        };
        self.fields.insert(key.into(), qv);
        Ok(self)
    }

    /// Set a field to a [`QueryValue`] directly.
    ///
    /// Use this for nested maps or lists that you have already assembled as
    /// a `QueryValue` (e.g. from `mpack!({...})`).  For scalar literals and
    /// expressions, prefer [`Doc::set`].
    pub fn set_value(mut self, key: impl Into<String>, value: impl Into<QueryValue>) -> Self {
        self.fields.insert(key.into(), value.into());
        self
    }

    /// Consume the builder and return the record as a `QueryValue::Map`.
    pub fn build(self) -> QueryValue {
        QueryValue::Map(self.fields)
    }
}

impl From<Doc> for QueryValue {
    fn from(doc: Doc) -> Self {
        doc.build()
    }
}

/// Bounded check that a [`FilterValue`] tree nests no deeper than
/// `MAX_FILTER_DEPTH`, with early cutoff: the walk aborts as soon as any
/// node is found past the limit, so the checker itself cannot stack- or
/// overflow on the very trees it rejects. Iterative worklist, no recursion.
///
/// Unlike a leaf-treating walk, this descends into expression markers —
/// `FnCall` args, `Expr` args, and `Cond` (`condition`/`then`/`or_else`) —
/// because the msgpack fallback serializes those recursively and a deeply
/// nested `$fn`/`$expr`/`$cond` chain would otherwise overflow the stack
/// below this guard.
///
/// For `Cond`'s embedded `Filter` (`condition`), the public
/// [`check_filter_depth_at`] from `shamir-query-types` is reused rather than
/// reimplemented: it is an iterative walker over the full `Filter` enum
/// (all variants, including nested values), already tested, and keeps the
/// builder's depth semantics tied to the shared limit's canonical checker.
/// It is called with the ACCUMULATED depth (`depth + 1`), not from 1, so an
/// outer chain near the limit plus a legal-looking condition subtree cannot
/// jointly exceed `MAX_FILTER_DEPTH`.
fn filter_value_is_shallow_enough(fv: &FilterValue) -> bool {
    let mut stack: Vec<(&FilterValue, usize)> = vec![(fv, 1)];
    while let Some((current, depth)) = stack.pop() {
        if depth > MAX_FILTER_DEPTH {
            return false;
        }
        match current {
            FilterValue::Array(items) => {
                for item in items {
                    stack.push((item, depth + 1));
                }
            }
            FilterValue::FnCall { call } => {
                for arg in call.args() {
                    stack.push((arg, depth + 1));
                }
            }
            FilterValue::Expr { expr } => {
                for arg in &expr.args {
                    stack.push((arg, depth + 1));
                }
            }
            FilterValue::Cond { cond } => {
                // Reuse the shared iterative Filter-depth checker for the
                // embedded condition tree, seeded with the accumulated depth
                // so the condition counts toward the total limit.
                if check_filter_depth_at(cond.condition.as_ref(), depth + 1).is_err() {
                    return false;
                }
                stack.push((&cond.then, depth + 1));
                stack.push((&cond.or_else, depth + 1));
            }
            FilterValue::Null
            | FilterValue::Bool(_)
            | FilterValue::Int(_)
            | FilterValue::Float(_)
            | FilterValue::String(_)
            | FilterValue::Binary(_)
            | FilterValue::FieldRef { .. }
            | FilterValue::QueryRef { .. }
            | FilterValue::Param { .. } => {}
        }
    }
    true
}
