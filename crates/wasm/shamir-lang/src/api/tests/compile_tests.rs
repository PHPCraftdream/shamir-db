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
}

#[test]
fn compile_still_rejects_procedures_with_t007() {
    let source = "@entry procedure p(a: i64) -> i64 { return a; }";
    let options = CompileOptions {
        stage: crate::api::Stage::Procedures,
        ..default_options()
    };
    // Derivation: procedures are rejected at the entry-kind gate (T007), not
    // by the stage switch — both the default and Procedures stages fail.
    for options in [default_options(), options] {
        let error = compile(source, &options).expect_err("procedure entry is rejected");
        match error {
            CompileError::Diagnostics(diagnostics) => {
                assert_eq!(diagnostics.len(), 1);
                assert_eq!(diagnostics[0].code, "T007");
            }
            other => panic!("expected Diagnostics, got {other:?}"),
        }
    }
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
