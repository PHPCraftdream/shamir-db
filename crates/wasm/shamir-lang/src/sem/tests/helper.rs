use crate::api::{CompileLimits, Diagnostic, Stage};
use crate::front;
use crate::sem::{self, TypedFunction, TypedModule};

pub(super) const SCALAR_EXAMPLE: &str = "fn add(a: i64, b: i64) -> i64 { return a + b; }\n@entry scalar sum(a: i64, b: i64) -> i64 {\n  return add(a, b);\n}\n";

pub(super) const VALIDATOR_EXAMPLE: &str = "@entry validator require_name(\n  record: { name: string }?,\n  old_record: { name: string }?\n) -> Validation {\n  if (is_null(record)) {\n    return record_error(\"record_required\");\n  }\n  let current: { name: string } = unwrap(record);\n  if (current.name == \"\") {\n    return reject(\"name\", \"name_empty\");\n  }\n  return accept();\n}\n";

/// Parses with the default limits and runs the semantic check at `Stage::Mvp`.
pub(super) fn check_source(source: &str) -> Result<TypedModule, Vec<Diagnostic>> {
    let module = front::parse(source, &CompileLimits::default()).expect("fixture source parses");
    sem::check(&module, &CompileLimits::default(), Stage::Mvp)
}

/// Parses and checks with the same custom limits at `Stage::Mvp`.
pub(super) fn check_with(
    source: &str,
    limits: &CompileLimits,
) -> Result<TypedModule, Vec<Diagnostic>> {
    let module = front::parse(source, limits).expect("fixture source parses");
    sem::check(&module, limits, Stage::Mvp)
}

/// Parses with the default limits but checks with custom limits and stage.
/// Needed where a lowered limit (e.g. zero compile fuel) would otherwise
/// abort parsing before the semantic pass runs.
pub(in crate::sem) fn check_with_stage(
    source: &str,
    limits: &CompileLimits,
    stage: Stage,
) -> Result<TypedModule, Vec<Diagnostic>> {
    let module = front::parse(source, &CompileLimits::default()).expect("fixture source parses");
    sem::check(&module, limits, stage)
}

/// Asserts the source is rejected and returns the diagnostic codes in the
/// order the checker emits them.
pub(super) fn expect_codes(source: &str) -> Vec<&'static str> {
    match check_source(source) {
        Ok(_) => panic!("expected diagnostics, got a clean module for: {source}"),
        Err(diags) => diags.iter().map(|diag| diag.code).collect(),
    }
}

/// Asserts the source passes the semantic check cleanly.
pub(super) fn expect_clean(source: &str) -> TypedModule {
    match check_source(source) {
        Ok(module) => module,
        Err(diags) => panic!(
            "expected a clean module, got diagnostics: {:?}",
            diags
                .iter()
                .map(|diag| diag.to_string())
                .collect::<Vec<_>>()
        ),
    }
}

/// Returns the single/first diagnostic for message assertions.
pub(super) fn first_diagnostic(source: &str) -> Diagnostic {
    match check_source(source) {
        Ok(_) => panic!("expected diagnostics, got a clean module for: {source}"),
        Err(diags) => diags
            .into_iter()
            .next()
            .unwrap_or_else(|| panic!("expected at least one diagnostic for: {source}")),
    }
}

/// Looks up the typed entry function of a checked module.
pub(super) fn entry_of(module: &TypedModule) -> &TypedFunction {
    module
        .function(module.entry)
        .expect("entry function is present in the typed table")
}
