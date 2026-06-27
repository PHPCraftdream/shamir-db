//! Fluent rule builder for declarative schema field rules.
//!
//! Usage:
//! ```ignore
//! use shamir_engine::validator::schema::rule;
//!
//! let rules = vec![
//!     rule(["email"]).string().max_len(255).required(),
//!     rule(["age"]).int().min(0).max(150),
//!     rule(["address", "zip"]).string().len(5),
//!     rule(["tags"]).list().array_of_string(),
//! ];
//! ```

use shamir_query_types::filter::FilterValue;
use shamir_types::types::value::QueryValue;

use super::constraints::{Constraints, Num};
use super::cross_field::{CompareOp, CrossFieldCompare};
use super::field_rule::FieldRule;
use super::format::FormatKind;
use super::type_tag::TypeTag;

/// Start building a field rule for the given path.
///
/// `path` accepts anything that can be converted into a `Vec<String>` of
/// path segments — typically a fixed-size array of `&str`.
pub fn rule<I, S>(path: I) -> RuleBuilder
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    RuleBuilder {
        path: path.into_iter().map(Into::into).collect(),
        ty: TypeTag::Any,
        constraints: Constraints::default(),
    }
}

/// Fluent builder for a single [`FieldRule`].
///
/// Type-setting methods (`string()`, `int()`, etc.) set the [`TypeTag`];
/// constraint methods (`required()`, `min()`, etc.) accumulate into
/// [`Constraints`].  The builder converts to [`FieldRule`] via `Into` /
/// `build()`.
pub struct RuleBuilder {
    path: Vec<String>,
    ty: TypeTag,
    constraints: Constraints,
}

impl RuleBuilder {
    // ── Type setters (each returns self for chaining) ───────────────────

    /// Set the type tag to `String`.
    pub fn string(mut self) -> Self {
        self.ty = TypeTag::String;
        self
    }

    /// Set the type tag to `Int`.
    pub fn int(mut self) -> Self {
        self.ty = TypeTag::Int;
        self
    }

    /// Set the type tag to `F64`.
    pub fn f64(mut self) -> Self {
        self.ty = TypeTag::F64;
        self
    }

    /// Set the type tag to `Dec`.
    pub fn dec(mut self) -> Self {
        self.ty = TypeTag::Dec;
        self
    }

    /// Set the type tag to `Bool`.
    pub fn bool(mut self) -> Self {
        self.ty = TypeTag::Bool;
        self
    }

    /// Set the type tag to `Bin`.
    pub fn bin(mut self) -> Self {
        self.ty = TypeTag::Bin;
        self
    }

    /// Set the type tag to `List`.
    pub fn list(mut self) -> Self {
        self.ty = TypeTag::List;
        self
    }

    /// Set the type tag to `Map`.
    pub fn map(mut self) -> Self {
        self.ty = TypeTag::Map;
        self
    }

    /// Set the type tag to `Set`.
    pub fn set(mut self) -> Self {
        self.ty = TypeTag::Set;
        self
    }

    // ── Constraint setters ──────────────────────────────────────────────

    /// Mark the field as required (must be present in the record).
    pub fn required(mut self) -> Self {
        self.constraints.required = true;
        self
    }

    /// Mark the field as nullable (Null is accepted when present).
    pub fn nullable(mut self) -> Self {
        self.constraints.nullable = true;
        self
    }

    /// Set the minimum numeric value (inclusive).
    pub fn min(mut self, n: i64) -> Self {
        self.constraints.min = Some(Num::Int(n));
        self
    }

    /// Set the minimum numeric value as f64 (inclusive).
    pub fn min_f64(mut self, n: f64) -> Self {
        self.constraints.min = Some(Num::F64(n));
        self
    }

    /// Set the maximum numeric value (inclusive).
    pub fn max(mut self, n: i64) -> Self {
        self.constraints.max = Some(Num::Int(n));
        self
    }

    /// Set the maximum numeric value as f64 (inclusive).
    pub fn max_f64(mut self, n: f64) -> Self {
        self.constraints.max = Some(Num::F64(n));
        self
    }

    /// Set exact length (strings: char count; collections: item count).
    pub fn len(mut self, n: u64) -> Self {
        self.constraints.len = Some(n);
        self
    }

    /// Set maximum length (strings: char count; collections: item count).
    pub fn max_len(mut self, n: u64) -> Self {
        self.constraints.max_len = Some(n);
        self
    }

    /// Set minimum length (strings: char count; collections: item count).
    pub fn min_len(mut self, n: u64) -> Self {
        self.constraints.min_len = Some(n);
        self
    }

    /// Mark the integer field as unsigned (value must be >= 0).
    pub fn unsigned(mut self) -> Self {
        self.constraints.unsigned = true;
        self
    }

    /// Set allowed values (enum / const).
    pub fn one_of(mut self, values: Vec<QueryValue>) -> Self {
        self.constraints.one_of = Some(values);
        self
    }

    /// Set the default value stamped on INSERT for an absent field (③.2c:
    /// extended from Phase ②.4b literal-only to expression).
    ///
    /// - **Literal** `FilterValue` scalars (Null/Bool/Int/Float/String/Binary)
    ///   route through the fast `apply_defaults` path (②.4c behaviour is
    ///   unchanged).
    /// - **Expression** `FilterValue` forms (`$fn` / `$ref` / etc.) route
    ///   through `apply_transforms` → `eval_write_value` → `builtin_scalars()`
    ///   at admission-time.  User scalars are NOT available here (same boundary
    ///   as inline `$fn` in `resolve_computed_record`).
    ///
    /// Legacy callers that pass a `QueryValue` directly are unaffected because
    /// `QueryValue` implements `Into<FilterValue>` via the untagged-serde
    /// round-trip (they share the same msgpack encoding).  To pass a `QueryValue`
    /// literal use `.default(FilterValue::Int(5))` or the `From` impls on
    /// `FilterValue`.
    // Inherent method named `default` — clippy's `should_implement_trait`
    // fires only when the type implements `Default`; `RuleBuilder` does not,
    // but we allow it defensively in case `Default` is ever derived here.
    #[allow(clippy::should_implement_trait)]
    pub fn default(mut self, value: impl Into<FilterValue>) -> Self {
        self.constraints.default = Some(value.into());
        self
    }

    /// Set the element type for `List` fields (`array_of` check).
    pub fn array_of_string(mut self) -> Self {
        self.constraints.array_of = Some(TypeTag::String);
        self
    }

    /// Set the element type for `List` fields (generic).
    pub fn array_of(mut self, tag: TypeTag) -> Self {
        self.constraints.array_of = Some(tag);
        self
    }

    // ── Phase B setters ────────────────────────────────────────────────

    /// Phase B — scalar-bridge: validate this field by calling the named
    /// registered scalar as a predicate.  The scalar receives the
    /// materialised field value as its single argument and must return
    /// `Bool`.
    pub fn scalar(mut self, name: impl Into<String>) -> Self {
        self.constraints.scalar = Some(name.into());
        self
    }

    /// Phase B — named format check (`email` / `url` / `uuid` / `date`).
    pub fn format(mut self, kind: FormatKind) -> Self {
        self.constraints.format = Some(kind);
        self
    }

    /// Phase B — `format("email")` convenience (parses the name).
    pub fn format_str(mut self, name: &str) -> Self {
        if let Some(k) = FormatKind::parse(name) {
            self.constraints.format = Some(k);
        }
        self
    }

    /// Phase B — cross-field comparison: `self.path  op  other_path`.
    pub fn compare(mut self, other_path: Vec<String>, op: CompareOp) -> Self {
        self.constraints.compare = Some(CrossFieldCompare::new(other_path, op));
        self
    }

    /// Consume the builder and produce a [`FieldRule`].
    pub fn build(self) -> FieldRule {
        FieldRule {
            path: self.path,
            ty: self.ty,
            constraints: self.constraints,
        }
    }
}

/// `RuleBuilder` converts to `FieldRule` via `Into`.
impl From<RuleBuilder> for FieldRule {
    fn from(b: RuleBuilder) -> Self {
        b.build()
    }
}
