//! Cross-field comparison for Phase B.
//!
//! [`CrossFieldCompare`] describes a binary relation between two field paths
//! in the *same* record (e.g. `start <= end`).  The check operates entirely
//! by-name via [`RecordFields`](crate::validator::record_fields::RecordFields)
//! — no interning ids leak into user-facing code.

use shamir_types::record_view::ScalarRef;

use crate::validator::record_fields::RecordFields;

/// Binary comparison operator between two scalar fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompareOp {
    /// `a < b`
    Lt,
    /// `a <= b`
    Le,
    /// `a == b`
    Eq,
    /// `a != b`
    Ne,
    /// `a >= b`
    Ge,
    /// `a > b`
    Gt,
}

impl CompareOp {
    /// Evaluate `op(lhs, rhs)` for two borrowed scalars.
    ///
    /// Returns `None` when the two scalars are not orderable together
    /// (different incomparable type families, or either is a container).
    /// A `None` result means the check cannot decide and the caller should
    /// record a `compare_type_mismatch` error rather than silently accept.
    pub fn eval_scalar(self, lhs: ScalarRef<'_>, rhs: ScalarRef<'_>) -> Option<bool> {
        use CompareOp::*;
        // Ordering via partial_cmp on ScalarRef is not provided; we compute
        // the ordering inline for the comparable families (Null/Bool/Int/F64
        // cross Int/F64/Str/Bin).  Cross-family (e.g. Str vs Int) → None.
        let ord = scalar_ordering(lhs, rhs)?;
        Some(match self {
            Lt => ord == std::cmp::Ordering::Less,
            Le => ord != std::cmp::Ordering::Greater,
            Eq => ord == std::cmp::Ordering::Equal,
            Ne => ord != std::cmp::Ordering::Equal,
            Ge => ord != std::cmp::Ordering::Less,
            Gt => ord == std::cmp::Ordering::Greater,
        })
    }
}

/// Cross-field comparison: `self.path  OP  other.path`.
///
/// Both paths are resolved by name from the same `RecordFields`.  If either
/// path is absent the check is silently skipped (a `required` rule on that
/// field is the right way to catch absence; cross-field comparison is about
/// the *relation* between two present values).
#[derive(Debug, Clone, PartialEq)]
pub struct CrossFieldCompare {
    /// The other field path to compare against (the left operand is the
    /// rule's own `path`).
    pub other: Vec<String>,
    /// The comparison operator.
    pub op: CompareOp,
}

impl CrossFieldCompare {
    /// Construct a cross-field rule: `self-path  op  other-path`.
    pub fn new(other: Vec<String>, op: CompareOp) -> Self {
        Self { other, op }
    }

    /// Run the comparison against `fields`, using `self_path` as the left
    /// operand.  Returns `Some(true)` if the relation holds, `Some(false)`
    /// if it is violated, and `None` if the check could not be evaluated
    /// (either path absent, or the values are not mutually orderable).  The
    /// caller decides how to surface a `None`.
    pub fn check(&self, fields: &dyn RecordFields, self_path: &[&str]) -> CrossFieldResult {
        let other_refs: Vec<&str> = self.other.iter().map(String::as_str).collect();

        let lhs = match fields.scalar(self_path) {
            Some(s) => s,
            None => return CrossFieldResult::Skipped,
        };
        let rhs = match fields.scalar(&other_refs) {
            Some(s) => s,
            None => return CrossFieldResult::Skipped,
        };

        match self.op.eval_scalar(lhs, rhs) {
            Some(true) => CrossFieldResult::Ok,
            Some(false) => CrossFieldResult::Violated,
            None => CrossFieldResult::TypeMismatch,
        }
    }
}

/// Outcome of a cross-field comparison.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrossFieldResult {
    /// The relation holds.
    Ok,
    /// The relation is violated (`field_error("compare_violation")`).
    Violated,
    /// The two values are not mutually orderable (`field_error("compare_type_mismatch")`).
    TypeMismatch,
    /// Either path is absent — the check is silently skipped.
    Skipped,
}

/// Compute a total [`Ordering`] for two comparable scalars.
///
/// Mirrors the comparison families in `scalar_ref::scalar_ref_cmp`:
/// Null==Null, Bool<Bool, Int/Int (and cross Int/F64), F64/F64, Str/Str.
/// Returns `None` for any other pair (Bin, cross-type Str vs Int, etc.).
fn scalar_ordering(a: ScalarRef<'_>, b: ScalarRef<'_>) -> Option<std::cmp::Ordering> {
    use std::cmp::Ordering;
    match (a, b) {
        (ScalarRef::Null, ScalarRef::Null) => Some(Ordering::Equal),
        (ScalarRef::Bool(x), ScalarRef::Bool(y)) => Some(x.cmp(&y)),
        (ScalarRef::Int(x), ScalarRef::Int(y)) => Some(x.cmp(&y)),
        (ScalarRef::Int(x), ScalarRef::F64(y)) => (x as f64).partial_cmp(&y),
        (ScalarRef::F64(x), ScalarRef::Int(y)) => x.partial_cmp(&(y as f64)),
        (ScalarRef::F64(x), ScalarRef::F64(y)) => x.partial_cmp(&y),
        (ScalarRef::Str(x), ScalarRef::Str(y)) => Some(x.cmp(y)),
        _ => None,
    }
}
