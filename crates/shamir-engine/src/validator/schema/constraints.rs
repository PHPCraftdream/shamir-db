//! Constraint descriptors for declarative schema field rules.
//!
//! [`Constraints`] holds the set of checks applied to a field value after
//! the type tag passes.  Phase A covers pure (in-process, no DB) checks;
//! Phase B adds `scalar` (escape-hatch via a registered scalar),
//! `format` (email/url/uuid/date), and `compare` (cross-field).
//! Phase C will extend this struct with `foreign_key`, `unique`.

use shamir_query_types::filter::FilterValue;
use shamir_types::types::value::QueryValue;

use super::cross_field::CrossFieldCompare;
use super::foreign_key::ForeignKeyRef;
use super::format::FormatKind;
use super::type_tag::TypeTag;

/// Numeric bound for `min` / `max` constraints.
///
/// Supports both integer and floating-point bounds so the same constraint
/// struct works for `Int`, `F64`, and `Dec` fields.
#[derive(Debug, Clone, PartialEq)]
pub enum Num {
    /// Integer bound (for `Int` fields).
    Int(i64),
    /// Floating-point bound (for `F64` fields).
    F64(f64),
}

/// Constraints for a field rule.
///
/// All fields are public for construction in tests and from the fluent
/// builder.  Phase B adds the `scalar`, `format`, and `compare` fields;
/// Phase C will add `foreign_key` and `unique`.
#[derive(Debug, Clone, Default)]
pub struct Constraints {
    /// The field must be present in the record.
    pub required: bool,
    /// The field may be `Null`.  If `false` and the field is present as
    /// `Null`, validation fails with `null_not_allowed`.
    pub nullable: bool,
    /// Minimum numeric value (inclusive).  Applies to `Int` / `F64` fields.
    pub min: Option<Num>,
    /// Maximum numeric value (inclusive).  Applies to `Int` / `F64` fields.
    pub max: Option<Num>,
    /// Exact length for strings (`String` tag) or exact item count for
    /// collections (`List` / `Map` / `Set` tags).
    pub len: Option<u64>,
    /// Maximum length for strings.  Separate from `len` (which is exact).
    pub max_len: Option<u64>,
    /// Minimum length for strings.
    pub min_len: Option<u64>,
    /// `Int` field must be >= 0 (the "u64 intent" pattern: `Int + unsigned`).
    pub unsigned: bool,
    /// Enumerated allowed values.  A single-element vec is a `const` check.
    pub one_of: Option<Vec<QueryValue>>,
    /// Default value (literal or expression) stamped on INSERT for an absent
    /// field (③.2c: extended from Phase ②.4b literal-only to expression).
    ///
    /// Literal variants (Null/Bool/Int/Float/String/Binary) stay on the fast
    /// `apply_defaults` path (`collect_defaults()` → `QueryValue`); expression
    /// variants (`$fn` / `$ref` / etc.) are routed through `apply_transforms`
    /// as `ComputedDefault(expr)` (see `SchemaValidator::transforms()`).
    /// Replay-safe by construction (the stamped field is present on reload;
    /// see DDL-EVOLUTION-PLAN §②.4a variant B).
    pub default: Option<FilterValue>,
    /// Element type for `List` fields (`array_of`).  Only meaningful when
    /// the field's [`TypeTag`] is `List`.
    pub array_of: Option<TypeTag>,
    /// Phase B — scalar-bridge: validate the field by calling the named
    /// registered scalar (built-in funclib or user scalar) as a predicate.
    /// The scalar receives the materialised field value as its single
    /// argument and must return `Bool`.  When the resolver is unavailable
    /// (`ValidatorCtx::scalars() == None`) the rule is silently skipped.
    pub scalar: Option<String>,
    /// Phase B — named format check (`email` / `url` / `uuid` / `date`).
    /// Implemented as a thin in-process predicate; reuses funclib regex
    /// patterns where they exist (email/url/uuid) to avoid duplication.
    pub format: Option<FormatKind>,
    /// Phase B — cross-field comparison against another path in the same
    /// record (e.g. `start <= end`).
    pub compare: Option<CrossFieldCompare>,
    /// Phase C2 — forward-only foreign-key reference.  The field value must
    /// exist in `ref_table.ref_field`.  Enforced whenever the validator runs
    /// with a cross-table resolver wired into `ctx.db()`.  In production the
    /// server wraps every batch in a tx context with the resolver wired, so
    /// FK fires on BOTH transactional and autocommit writes (proven by the
    /// `foreign_key: autocommit also enforces FK` e2e).  The resolver is
    /// absent only on the raw-engine implicit single-op path invoked WITHOUT
    /// the server's tx wrapping (`query_runner` autocommit arm passes
    /// `resolver = None`, some unit fixtures) — there FK silently skips
    /// (fail-open).  NULL values bypass the FK check (standard SQL semantics).
    pub foreign_key: Option<ForeignKeyRef>,
    /// Phase C3 — unique constraint.  The field value must not duplicate any
    /// existing committed row (or staged write within the same tx).  The
    /// unique probe reads SELF-table state and needs NO cross-table resolver —
    /// it is enforced whenever the validator runs with a tx (`ctx.db() ==
    /// Some`).  Every write path threads a tx (the autocommit path routes
    /// through an implicit Snapshot tx), so unique fires on BOTH transactional
    /// and autocommit writes (proven by the `unique: autocommit also enforces
    /// unique` e2e).  NULL values bypass the unique check (standard SQL
    /// semantics: NULL is never equal to anything, including another NULL).
    pub unique: bool,

    /// ③.2d — server-stamping: stamp the server wall-clock nanoseconds onto
    /// this field on EVERY write (INSERT and UPDATE). The server clock is
    /// always authoritative — any caller-supplied value is overwritten.
    ///
    /// Semantic: `updated_at` pattern.  Aggregated by
    /// `SchemaValidator::transforms()` as `TransformSpec::AutoNow`.
    pub auto_now: bool,

    /// ③.2d — server-stamping: stamp the server wall-clock nanoseconds onto
    /// this field on INSERT only, and only when the field is absent.
    /// An explicitly-supplied value (including explicit `Null`) is preserved.
    ///
    /// Semantic: `created_at` pattern.  Aggregated by
    /// `SchemaValidator::transforms()` as `TransformSpec::AutoNowAdd`.
    pub auto_now_add: bool,
}
