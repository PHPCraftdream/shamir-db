/// Declarative transform operation applied to a record BEFORE encode
/// (admission-time, NOT on WAL-replay — replay-safety is free by construction:
/// the transformed bytes are what gets stored, and replay restores them verbatim).
///
/// Transform rules are aggregated by [`super::schema::SchemaValidator`] (close
/// twin of `defaults()`) and applied on the insert path in `write_exec.rs`
/// AFTER `apply_defaults` and BEFORE encode + CHECK-validators.
///
/// # Order of operations (insert path)
/// `resolve_computed_record` → `apply_defaults` (literals) →
/// `apply_transforms` (computed-default + stamping) → encode →
/// CHECK-validators.
///
/// # Variant semantics
/// See the field-level doc-comments for per-variant stamping policy
/// (absence-guarded vs unconditional).
pub enum TransformSpec {
    /// ③.2c: computed-default expression.  Applied ONLY when the field is
    /// absent (same keystone as `apply_defaults` / DDL-EVOLUTION-PLAN §②.4a):
    /// an explicit `Null` is NOT absent and is never overwritten.
    ///
    /// The expression is evaluated through `eval_write_value` at
    /// admission-time.  On evaluation error the stamp is skipped silently
    /// (fail-open, consistent with the scalar-bridge precedent in Phase B).
    ComputedDefault(shamir_query_types::filter::FilterValue),

    /// ③.2d `created_at`: timestamp stamp on INSERT, only when the field is
    /// absent.  An explicitly-supplied `created_at` is preserved as-is.
    AutoNowAdd,

    /// ③.2d `updated_at`: timestamp stamp UNCONDITIONALLY on every write —
    /// overwrites any caller-supplied value so the server clock is always
    /// authoritative.
    AutoNow,
}
