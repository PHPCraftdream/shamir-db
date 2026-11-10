use shamir_wasm_admission::AdmissionLimits;

use crate::api::{CompileError, CompileLimits, CompileOptions};
use crate::{compile, CompiledModule};

/// Fixture: one helper function plus one `@entry` scalar wrapper calling it.
const FIXTURE: &str = r"
fn add(a: i64, b: i64) -> i64 { return a + b; }
@entry scalar sum(a: i64, b: i64) -> i64 { return add(a, b); }
";

fn default_options() -> CompileOptions {
    CompileOptions::default()
}

#[test]
fn compiles_deterministically() {
    let first = compile(FIXTURE, &default_options()).expect("fixture compiles");
    let second = compile(FIXTURE, &default_options()).expect("fixture compiles");
    // Determinism: identical inputs must produce identical output bytes.
    assert_eq!(first.wasm, second.wasm);
    assert_eq!(first.signature, second.signature);
    // The admission report counts the emitted module, so it must equal its length.
    assert_eq!(first.report.byte_size, first.wasm.len());
}

#[test]
fn diagnostics_variant_reachable_from_source() {
    // The `{` is followed by an unexpected `->`, a parse error.
    let source = "@entry scalar x( -> i64 { }";
    let error = compile(source, &default_options()).expect_err("broken source must fail");
    match error {
        CompileError::Diagnostics(diagnostics) => {
            // At least one located diagnostic is produced for malformed source.
            assert!(!diagnostics.is_empty());
        }
        other => panic!("expected Diagnostics, got {other:?}"),
    }
}

#[test]
fn procedure_entry_gives_t007() {
    let source = "@entry procedure p(a: i64) -> i64 { return a; }";
    let error = compile(source, &default_options()).expect_err("procedure entry is rejected");
    match error {
        CompileError::Diagnostics(diagnostics) => {
            // Exactly one diagnostic: the T007 entry-kind rejection.
            assert_eq!(diagnostics.len(), 1);
            // T007 either from the checker or the pipeline entry gate.
            assert_eq!(diagnostics[0].code, "T007");
        }
        other => panic!("expected Diagnostics, got {other:?}"),
    }
}

#[test]
fn admission_variant_reachable() {
    // A 1-byte module cap makes the self-admission of our own output fail.
    let options = CompileOptions {
        admission: AdmissionLimits {
            max_module_bytes: 1,
            ..Default::default()
        },
        ..default_options()
    };
    let error = compile(FIXTURE, &options).expect_err("1-byte cap must reject the module");
    match error {
        CompileError::Admission(_) => {}
        other => panic!("expected Admission, got {other:?}"),
    }
}

#[test]
fn emitted_operator_cap_enforced_by_self_admission() {
    // Three operators per function cannot hold the fixture's bodies.
    let options = CompileOptions {
        admission: AdmissionLimits {
            max_operators_per_function: 3,
            ..Default::default()
        },
        ..default_options()
    };
    let error = compile(FIXTURE, &options).expect_err("operator cap must reject the module");
    match error {
        CompileError::Admission(_) => {}
        other => panic!("expected Admission, got {other:?}"),
    }
}

/// Compiles the fixture with a single limit set; used by the boundary tests.
fn compile_with_usize(field: fn(&mut CompileLimits) -> &mut usize, value: usize) -> Result<(), ()> {
    // The variant is intentionally erased: only Ok/Err drives the search.
    let mut limits = CompileLimits::default();
    *field(&mut limits) = value;
    let options = CompileOptions {
        limits,
        ..default_options()
    };
    compile(FIXTURE, &options).map(|_| ()).map_err(|_| ())
}

fn compile_with_u64(field: fn(&mut CompileLimits) -> &mut u64, value: u64) -> Result<(), ()> {
    // The variant is intentionally erased: only Ok/Err drives the search.
    let mut limits = CompileLimits::default();
    *field(&mut limits) = value;
    let options = CompileOptions {
        limits,
        ..default_options()
    };
    compile(FIXTURE, &options).map(|_| ()).map_err(|_| ())
}

/// Binary-searches downward for the smallest limit value that still compiles
/// the fixture. The exact boundary is derived at test time, not hand-computed.
fn minimum_passing_usize(field: fn(&mut CompileLimits) -> &mut usize) -> usize {
    let default = *field(&mut CompileLimits::default());
    // Invariant: the default limit compiles the fixture.
    assert_eq!(compile_with_usize(field, default), Ok(()));
    let mut lo = 1;
    let mut hi = default;
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if compile_with_usize(field, mid) == Ok(()) {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    lo
}

fn minimum_passing_u64(field: fn(&mut CompileLimits) -> &mut u64) -> u64 {
    let default = *field(&mut CompileLimits::default());
    // Invariant: the default limit compiles the fixture.
    assert_eq!(compile_with_u64(field, default), Ok(()));
    let mut lo = 1;
    let mut hi = default;
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if compile_with_u64(field, mid) == Ok(()) {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    lo
}

fn assert_boundary_usize(name: &str, field: fn(&mut CompileLimits) -> &mut usize) {
    // Zero is rejected in tests via clamped()'s test-aware zero gate.
    assert_eq!(
        compile_with_usize(field, 0),
        Err(()),
        "{name}: zero must be LimitExceeded"
    );
    let minimum = minimum_passing_usize(field);
    // The exact boundary is derived at test time by the search, not hand-computed.
    assert_eq!(
        compile_with_usize(field, minimum),
        Ok(()),
        "{name}: minimum passes"
    );
    assert_eq!(
        compile_with_usize(field, minimum - 1),
        Err(()),
        "{name}: minimum - 1 fails"
    );
}

#[test]
fn max_ir_nodes_boundary() {
    assert_boundary_usize("max_ir_nodes", |limits| &mut limits.max_ir_nodes);
}

#[test]
fn max_ir_nodes_per_function_boundary() {
    assert_boundary_usize("max_ir_nodes_per_function", |limits| {
        &mut limits.max_ir_nodes_per_function
    });
}

#[test]
fn max_static_bytes_boundary() {
    assert_boundary_usize("max_static_bytes", |limits| &mut limits.max_static_bytes);
}

#[test]
fn max_total_literal_bytes_boundary() {
    // The shared fixture has no literal, so the limit would never bind there:
    // this entry returns a string literal, and the exact boundary is searched.
    let source = "@entry scalar probe() -> string { return \"ab\"; }
";
    let compile_with = |limit: usize| -> Result<(), ()> {
        let options = CompileOptions {
            limits: CompileLimits {
                max_total_literal_bytes: limit,
                ..CompileLimits::default()
            },
            ..default_options()
        };
        compile(source, &options).map(|_| ()).map_err(|_| ())
    };
    assert_eq!(compile_with(0), Err(()), "zero literal bytes must fail");
    let minimum = (1..=64)
        .find(|limit| compile_with(*limit) == Ok(()))
        .expect("a small limit admits the literal");
    assert_eq!(
        compile_with(minimum - 1),
        Err(()),
        "below the minimum fails"
    );
    assert_eq!(compile_with(minimum), Ok(()), "the minimum passes");
    assert_eq!(
        compile_with(minimum + 1),
        Ok(()),
        "above the minimum passes"
    );
}

#[test]
fn compile_fuel_boundary() {
    let field: fn(&mut CompileLimits) -> &mut u64 = |limits| &mut limits.compile_fuel;
    // Zero is rejected in tests via clamped()'s test-aware zero gate.
    assert_eq!(
        compile_with_u64(field, 0),
        Err(()),
        "compile_fuel: zero fails"
    );
    let minimum = minimum_passing_u64(field);
    // The exact boundary is derived at test time by the search, not hand-computed.
    assert_eq!(
        compile_with_u64(field, minimum),
        Ok(()),
        "compile_fuel: minimum passes"
    );
    assert_eq!(
        compile_with_u64(field, minimum - 1),
        Err(()),
        "compile_fuel: minimum - 1 fails"
    );
}

#[test]
fn max_output_bytes_boundary() {
    assert_boundary_usize("max_output_bytes", |limits| &mut limits.max_output_bytes);
}

#[test]
fn max_function_body_bytes_boundary() {
    assert_boundary_usize("max_function_body_bytes", |limits| {
        &mut limits.max_function_body_bytes
    });
}

// CompileError::InternalInvariant is a compiler-bug channel: it can only be
// produced by internal invariant/verifier/emission failures, never from valid
// source. Valid source must compile to Ok, leaving the variant unreachable.
#[test]
fn internal_invariant_not_reachable() {
    let result = compile(FIXTURE, &default_options()).expect("valid fixture compiles cleanly");
    assert!(matches!(result, CompiledModule { .. }));
}

#[test]
fn one_budget_rejects_oversized_comment_source() {
    // Derivation: scan charges >= 1 fuel per source byte; a 100_000-byte
    // block comment plus a tiny entry needs > 100_000 fuel under ONE shared
    // budget, so a 90_000 fuel limit must fail with B001 before parse even
    // starts. (Pre-fix, scan+parse+check each had separate budgets.)
    let comment = format!("/*{}*/", "a".repeat(100_000));
    let source = format!("{comment}\n@entry scalar probe() -> i64 {{ return 1; }}\n");
    let options = CompileOptions {
        limits: CompileLimits {
            compile_fuel: 90_000,
            ..CompileLimits::default()
        },
        ..default_options()
    };
    match compile(&source, &options) {
        // The scanner reports fuel exhaustion as a B001 diagnostic.
        Err(CompileError::Diagnostics(diagnostics)) => {
            assert_eq!(diagnostics.first().map(|diag| diag.code), Some("B001"));
        }
        Err(CompileError::LimitExceeded { code, .. }) => assert_eq!(code, "B001"),
        other => panic!("expected B001, got {other:?}"),
    }
}

/// Fixture: the milestone-5 validator worked example.
const VALIDATOR: &str = r#"
@entry validator require_name(
  record: { name: string }?,
  old_record: { name: string }?
) -> Validation {
  if (is_null(record)) {
    return record_error("record_required");
  }
  let current: { name: string } = unwrap(record);
  if (current.name == "") {
    return reject("name", "name_empty");
  }
  return accept();
}
"#;

#[test]
fn compile_accepts_validator_entries() {
    let module = compile(VALIDATOR, &default_options()).expect("validator entry compiles");
    // The admission report counts the emitted module, so it must equal its length.
    assert_eq!(module.report.byte_size, module.wasm.len());
    // This validator's opaque result intentionally has no procedure signature.
    assert_eq!(module.signature, None);
}

#[test]
fn compile_rejects_procedures_at_the_mvp_stage_with_t007() {
    let source = "@entry procedure p(a: i64) -> i64 { return a; }";
    // Derivation: the default stage is Mvp, where the checker reports T007 for
    // a procedure entry (the only diagnostic).
    let error = compile(source, &default_options()).expect_err("procedure entry is rejected");
    match error {
        CompileError::Diagnostics(diagnostics) => {
            assert_eq!(diagnostics.len(), 1);
            assert_eq!(diagnostics[0].code, "T007");
        }
        other => panic!("expected Diagnostics, got {other:?}"),
    }
}

fn procedure_options() -> CompileOptions {
    CompileOptions {
        stage: crate::api::Stage::Procedures,
        ..default_options()
    }
}

#[test]
fn compile_accepts_a_pure_procedure_at_the_procedures_stage() {
    let source = "@entry procedure p(a: i64) -> i64 { return a; }";
    let module = compile(source, &procedure_options()).expect("procedure compiles");
    // Derivation: no database builtin is used, so the module keeps the single
    // `raise_error` import of the scalar modules.
    assert_eq!(module.report.import_count, 1);
    assert_eq!(
        module.signature.as_deref(),
        Some(b"procedure(a:i64)->i64".as_slice())
    );
}

#[test]
fn compile_procedure_signature_is_canonical() {
    let source = "@entry procedure run(second: { z: i64, a: string? }, first: [i32]?) -> { y: bool, b: bytes } { return { y: true, b: b\"x\" }; }";
    let compiled = compile(source, &procedure_options()).expect("procedure compiles");
    assert_eq!(
        compiled.signature.as_deref(),
        Some(b"procedure(second:{a:string?,z:i64},first:[i32]?)->{b:bytes,y:bool}".as_slice())
    );
}

#[test]
fn compile_procedure_signature_excludes_function_name_and_handles_null_result() {
    let first = compile(
        "@entry procedure first() -> null { return; }",
        &procedure_options(),
    )
    .expect("first compiles");
    let renamed = compile(
        "@entry procedure renamed() -> null { return; }",
        &procedure_options(),
    )
    .expect("renamed compiles");
    assert_eq!(first.signature, renamed.signature);
    assert_eq!(
        first.signature.as_deref(),
        Some(b"procedure()->null".as_slice())
    );
}

#[test]
fn compile_procedure_signature_supports_nullable_result() {
    let compiled = compile(
        "@entry procedure finish() -> i64? { return null; }",
        &procedure_options(),
    )
    .expect("nullable-result procedure compiles");
    assert_eq!(
        compiled.signature.as_deref(),
        Some(b"procedure()->i64?".as_slice())
    );
}

#[test]
fn compile_scalar_entry_has_no_callable_signature() {
    let compiled = compile(FIXTURE, &default_options()).expect("scalar compiles");
    assert_eq!(compiled.signature, None);
}

#[test]
fn compile_procedure_with_database_calls_imports_the_host_functions() {
    let source = "@entry procedure p(key: i64) -> i64 {
         let user: { id: i64, name: string }? = db_get(\"users\", key);
         let rows: [{ id: i64 }] = db_query(\"users\", null);
         let stored: { id: i64 } = db_insert(\"users\", { id: key });
         if (is_null(user)) { return 0; }
         return stored.id + to_i64(len(rows));
         }
";
    let module = compile(source, &procedure_options()).expect("procedure with db calls compiles");
    assert_eq!(
        module.signature.as_deref(),
        Some(b"procedure(key:i64)->i64".as_slice())
    );
    // Derivation: `raise_error` plus one import per used operation
    // (`db_get`, `db_insert`, `db_query`).
    assert_eq!(module.report.import_count, 4);
    // Determinism holds for procedure modules too.
    let again = compile(source, &procedure_options()).expect("second compile");
    assert_eq!(module.wasm, again.wasm);
}

#[test]
fn compile_procedure_with_the_typed_builder_imports_db_execute() {
    let source = "@entry procedure p() -> i64 {
         let b = batch_read(batch(), \"u\", \"users\", { role: \"admin\" });
         let resp = db_execute(request(b));
         let rows: [{ id: i64 }]? = response_rows(resp, \"u\");
         if (is_null(rows)) { return 0; }
         return to_i64(len(unwrap(rows)));
         }
";
    let module = compile(source, &procedure_options()).expect("builder procedure compiles");
    assert_eq!(
        module.signature.as_deref(),
        Some(b"procedure()->i64".as_slice())
    );
    // Derivation: `raise_error` plus the single `db_execute` import.
    assert_eq!(module.report.import_count, 2);
    let again = compile(source, &procedure_options()).expect("second compile");
    assert_eq!(module.wasm, again.wasm);
}

#[test]
fn compile_validator_limits_are_enforced() {
    // Derivation: parse rejects sources over max_source_bytes with B001
    // before any lowering, so a 16-byte cap rejects the worked example.
    let options = CompileOptions {
        limits: CompileLimits {
            max_source_bytes: 16,
            ..CompileLimits::default()
        },
        ..default_options()
    };
    let error = compile(VALIDATOR, &options).expect_err("oversized source must fail");
    match error {
        CompileError::Diagnostics(diagnostics) => {
            assert_eq!(diagnostics.first().map(|diag| diag.code), Some("B001"));
        }
        CompileError::LimitExceeded { code, .. } => assert_eq!(code, "B001"),
        other => panic!("expected B001, got {other:?}"),
    }
}

#[test]
fn compile_validator_is_deterministic() {
    let first = compile(VALIDATOR, &default_options()).expect("validator entry compiles");
    let second = compile(VALIDATOR, &default_options()).expect("validator entry compiles");
    // Determinism: identical inputs must produce identical output bytes.
    assert_eq!(first.wasm, second.wasm);
    assert_eq!(first.signature, None);
}

/// Fixture: a `Validation` helper parameter, an entry-only aggregate
/// projection, a conditional constructor argument (`?:`) and branching
/// `Validation` lets, all in one module.
const VALIDATOR_BRANCHING: &str = r#"
fn add(v: Validation) -> Validation {
  return with_record_error(v, "extra");
}
@entry validator v(
  record: { name: string, nick: string? }?,
  old_record: { name: string, nick: string? }?
) -> Validation {
  let base = accept();
  let left = with_error(base, "f", "left");
  let right = with_error(base, "f", "right");
  let current: { name: string, nick: string? } = unwrap(record);
  if (is_null(current.nick)) {
    return add(left);
  }
  return add(with_error(right, is_null(old_record) ? "a" : "b", "code"));
}
"#;

#[test]
fn compile_accepts_validation_helpers_branching_and_optional_fields() {
    let module = compile(VALIDATOR_BRANCHING, &default_options());
    // Derivation: Ok means the IR verifier accepted the new CellsCopy
    // instruction, the entry-only type-node policy (a `Validation` helper
    // parameter), the spilled conditional constructor argument, and the
    // branching `Validation` lets; self-admission then passed on the emitted
    // module.
    let module = module.expect("branching validator module compiles");
    // The admission report counts the emitted module, so it must equal its length.
    assert_eq!(module.report.byte_size, module.wasm.len());
}

fn assert_procedure_diagnostic_codes(source: &str, expected: &[&str]) {
    let error =
        compile(source, &procedure_options()).expect_err("invalid procedure source rejects");
    let actual: Vec<&str> = match error {
        CompileError::Diagnostics(diagnostics) => diagnostics
            .iter()
            .map(|diagnostic| diagnostic.code)
            .collect(),
        other => panic!("expected source diagnostics, got {other:?}; source: {source}"),
    };
    assert_eq!(actual, expected, "source: {source}");
}

fn update_filter_source(filter: &str, parameters: &str) -> String {
    format!(
        "@entry procedure p({parameters}) -> i64 {{ let b = batch_update(batch(), \"u\", \"users\", {filter}, {{ name: \"x\" }}); return 0; }}"
    )
}

fn delete_filter_source(filter: &str, parameters: &str) -> String {
    format!(
        "@entry procedure p({parameters}) -> i64 {{ let b = batch_delete(batch(), \"d\", \"users\", {filter}); return 0; }}"
    )
}

fn assert_invalid_filter_both_operations(filter: &str, parameters: &str, code: &str) {
    assert_procedure_diagnostic_codes(&update_filter_source(filter, parameters), &[code]);
    assert_procedure_diagnostic_codes(&delete_filter_source(filter, parameters), &[code]);
}

#[test]
fn compile_accepts_mutations_nested_sets_and_nullable_affected_results() {
    let source = r#"
@entry procedure p(key: i64) -> i64 {
  let changes: { profile: { name: string }, labels: [string], score: i64 } = {
    profile: { name: "Ada" },
    labels: ["staff", "active"],
    score: key
  };
  let updated = batch_update(batch(), "u", "users", { id: key }, changes);
  let all = batch_delete(updated, "d", "sessions", { owner: key });
  let literal = batch_delete(all, "literal", "users", { enabled: true });
  let response = db_execute(request(literal));
  let changed: i64? = response_affected(response, "u");
  let removed: i64? = response_affected(response, "d");
  let removed_literal: i64? = response_affected(response, "literal");
  let missing: i64? = response_affected(response, "absent");
  if (is_null(missing)) { return 0; }
  return unwrap(changed) + unwrap(removed) + unwrap(removed_literal);
}
"#;
    compile(source, &procedure_options()).expect("typed mutations compile");
}

#[test]
fn compile_accepts_batch_helpers_without_opaque_result_provenance() {
    let source = r#"
fn build(key: i64) -> Batch {
  return batch_update(batch(), "u", "users", { id: key }, { name: "Ada" });
}
@entry procedure p(key: i64) -> i64 {
  let response = db_execute(request(build(key)));
  return 0;
}
"#;
    compile(source, &procedure_options())
        .expect("helper mutation can be executed without projecting it");
}

#[test]
fn mutation_predicates_reject_invalid_shapes_for_update_and_delete() {
    assert_invalid_filter_both_operations("{}", "", "T001");
    assert_invalid_filter_both_operations("null", "", "T001");
    assert_invalid_filter_both_operations("key", "key: i64", "T001");
    assert_invalid_filter_both_operations("{ id: maybe }", "maybe: i64?", "T001");
    assert_invalid_filter_both_operations("filter", "filter: { id: i64 }?", "T001");
    assert_invalid_filter_both_operations("filter", "filter: { id: i64 }", "T001");
    assert_invalid_filter_both_operations("{ id: key + 1 }", "key: i64", "T001");
    assert_invalid_filter_both_operations("{ id: to_i32(key) }", "key: i64", "T001");
    // Record-field access stands for a reference-shaped expression; references
    // themselves are not a stage-1 language feature.
    assert_invalid_filter_both_operations("{ id: row.id }", "row: { id: i64 }", "T001");
}

#[test]
fn mutation_predicates_reject_record_expressions_and_null_fields() {
    let record_expression = r#"
fn selection() -> { id: i64 } { return { id: 1 }; }
@entry procedure p() -> i64 {
  let b = batch_update(batch(), "u", "users", selection(), { name: "x" });
  return 0;
}
"#;
    let delete_record_expression = r#"
fn selection() -> { id: i64 } { return { id: 1 }; }
@entry procedure p() -> i64 {
  let b = batch_delete(batch(), "d", "users", selection());
  return 0;
}
"#;
    assert_procedure_diagnostic_codes(record_expression, &["T001"]);
    assert_procedure_diagnostic_codes(delete_record_expression, &["T001"]);

    assert_invalid_filter_both_operations("{ id: null }", "", "T001");
}

#[test]
fn unresolved_reference_helper_uses_the_unknown_function_diagnostic() {
    let source = r#"
@entry procedure p() -> i64 {
  let b = batch_delete(batch(), "d", "users", { id: qref("users", "[0].id") });
  return 0;
}
"#;
    // qref is deliberately not a stage-1 builtin. The outer predicate shape
    // diagnostic precedes the unresolved function diagnostic in source order.
    assert_procedure_diagnostic_codes(source, &["T001", "N001"]);
}

#[test]
fn mutation_calls_require_the_predicate_argument() {
    assert_procedure_diagnostic_codes(
        "@entry procedure p() -> i64 { batch_update(batch(), \"u\", \"users\"); return 0; }",
        &["T004"],
    );
    assert_procedure_diagnostic_codes(
        "@entry procedure p() -> i64 { batch_delete(batch(), \"d\", \"users\"); return 0; }",
        &["T004"],
    );
}

#[test]
fn mutation_sets_reject_empty_nonrecord_and_opaque_values() {
    assert_procedure_diagnostic_codes(
        "@entry procedure p() -> i64 { batch_update(batch(), \"u\", \"users\", { id: 1 }, {}); return 0; }",
        &["T001"],
    );
    assert_procedure_diagnostic_codes(
        "@entry procedure p() -> i64 { batch_update(batch(), \"u\", \"users\", { id: 1 }, 7); return 0; }",
        &["T001"],
    );
    assert_procedure_diagnostic_codes(
        "@entry procedure p() -> i64 { batch_update(batch(), \"u\", \"users\", { id: 1 }, { nested: batch() }); return 0; }",
        &["T001"],
    );
}

#[test]
fn mutation_alias_and_table_must_be_literals_for_both_operations() {
    let update_alias = "@entry procedure p(alias: string) -> i64 { batch_update(batch(), alias, \"users\", { id: 1 }, { name: \"x\" }); return 0; }";
    let delete_alias = "@entry procedure p(alias: string) -> i64 { batch_delete(batch(), alias, \"users\", { id: 1 }); return 0; }";
    let update_table = "@entry procedure p(table: string) -> i64 { batch_update(batch(), \"u\", table, { id: 1 }, { name: \"x\" }); return 0; }";
    let delete_table = "@entry procedure p(table: string) -> i64 { batch_delete(batch(), \"d\", table, { id: 1 }); return 0; }";
    for source in [update_alias, delete_alias, update_table, delete_table] {
        assert_procedure_diagnostic_codes(source, &["T001"]);
    }
}

#[test]
fn mutation_calls_reject_extra_returning_projection_and_cas_arguments() {
    let header = "@entry procedure p(values: { name: string }, mode: string, projection: { id: i64 }, expected: i64) -> i64 { ";
    let update_base = "batch_update(batch(), \"u\", \"users\", { id: 1 }, values, ";
    for extra in ["mode", "projection", "expected"] {
        let source = format!("{header}{update_base}{extra}); return 0; }}");
        assert_procedure_diagnostic_codes(&source, &["T004"]);
    }
    let delete_header = "@entry procedure p(mode: string, projection: { id: i64 }) -> i64 { ";
    for extra in ["mode", "projection"] {
        let source = format!(
            "{delete_header}batch_delete(batch(), \"d\", \"users\", {{ id: 1 }}, {extra}); return 0; }}"
        );
        assert_procedure_diagnostic_codes(&source, &["T004"]);
    }
}

#[test]
fn mutation_aliases_must_be_unique() {
    let source = r#"
@entry procedure p() -> i64 {
  let first = batch_update(batch(), "same", "users", { id: 1 }, { name: "x" });
  let second = batch_delete(first, "same", "users", { id: 1 });
  return 0;
}
"#;
    assert_procedure_diagnostic_codes(source, &["T010"]);
}

#[test]
fn mutation_metadata_requires_provable_batch_and_response_helpers() {
    let batch_helper = r#"
fn change(b: Batch) -> Batch {
  return batch_update(b, "u", "users", { id: 1 }, { name: "x" });
}
@entry procedure p() -> i64 { return 0; }
"#;
    assert_procedure_diagnostic_codes(batch_helper, &["T010"]);

    let response_helper = r#"
fn affected(r: Response) -> i64? { return response_affected(r, "u"); }
@entry procedure p() -> i64 { return 0; }
"#;
    assert_procedure_diagnostic_codes(response_helper, &["T010"]);
}

#[test]
fn affected_projection_rejects_read_and_insert_aliases() {
    let read = r#"
@entry procedure p() -> i64 {
  let response = db_execute(request(batch_read(batch(), "r", "users", { id: 1 })));
  let count: i64? = response_affected(response, "r");
  return 0;
}
"#;
    assert_procedure_diagnostic_codes(read, &["T001"]);

    let insert = r#"
@entry procedure p() -> i64 {
  let response = db_execute(request(batch_insert(batch(), "i", "users", { id: 1 })));
  let count: i64? = response_affected(response, "i");
  return 0;
}
"#;
    assert_procedure_diagnostic_codes(insert, &["T001"]);
}

#[test]
fn mutation_builtins_require_procedure_capability() {
    let source = "@entry scalar p() -> i64 { batch_update(batch(), \"u\", \"users\", { id: 1 }, { name: \"x\" }); return 0; }";
    assert_procedure_diagnostic_codes(source, &["T007", "T007"]);
}

#[test]
fn affected_projection_requires_i64_optional_context() {
    let source = r#"
@entry procedure p() -> i64 {
  let b = batch_update(batch(), "u", "users", { id: 1 }, { name: "x" });
  let response = db_execute(request(b));
  let count: i64 = response_affected(response, "u");
  return count;
}
"#;
    assert_procedure_diagnostic_codes(source, &["T010"]);
}

#[test]
fn batch_metadata_reassignment_cannot_retain_stale_operations() {
    let source = r#"
fn replacement() -> Batch { return batch(); }
@entry procedure p() -> i64 {
  let b = batch_update(batch(), "u", "users", { id: 1 }, { name: "x" });
  b = replacement();
  let response = db_execute(request(b));
  let count: i64? = response_affected(response, "u");
  return 0;
}
"#;
    assert_procedure_diagnostic_codes(source, &["T010"]);
}

#[test]
fn batch_metadata_after_if_and_loop_is_conservative() {
    let conditional = r#"
@entry procedure p(flag: bool) -> i64 {
  let b = batch_update(batch(), "u", "users", { id: 1 }, { name: "x" });
  if (flag) { let marker = 1; } else { let other = 2; }
  let response = db_execute(request(b));
  let count: i64? = response_affected(response, "u");
  return 0;
}
"#;
    assert_procedure_diagnostic_codes(conditional, &["T010"]);

    let looped = r#"
@entry procedure p(flag: bool) -> i64 {
  let b = batch_update(batch(), "u", "users", { id: 1 }, { name: "x" });
  while (flag) { let marker = 1; }
  let response = db_execute(request(b));
  let count: i64? = response_affected(response, "u");
  return 0;
}
"#;
    assert_procedure_diagnostic_codes(looped, &["T010"]);
}

#[test]
fn batch_metadata_from_else_arm_does_not_leak_after_if() {
    let source = r#"
@entry procedure p(flag: bool) -> i64 {
  let b = batch_update(batch(), "u", "users", { id: 1 }, { name: "x" });
  if (flag) { let marker = 1; }
  else { b = batch_update(batch(), "else", "users", { id: 2 }, { name: "y" }); }
  let response = db_execute(request(b));
  let count: i64? = response_affected(response, "u");
  return 0;
}
"#;
    assert_procedure_diagnostic_codes(source, &["T010"]);
}
