use crate::api::CompileLimits;
use crate::sem::types::TypeData;

use super::helper::{entry_of, expect_clean, expect_codes, first_diagnostic};

fn entry(body: &str) -> String {
    format!("@entry scalar probe() -> i64 {{ {body} }}")
}

fn validator_entry(body: &str) -> String {
    format!(
        "@entry validator probe(record: {{ name: string }}?, old_record: {{ name: string }}?) -> Validation {{ {body} }}"
    )
}

// ---------------------------------------------------------------------------
// Integer literal rules
// ---------------------------------------------------------------------------

#[test]
fn integer_literal_rules_follow_suffix_and_expected_type() {
    let clean_cases: [(&str, TypeData); 6] = [
        ("let a: i32 = 5;", TypeData::I32),
        ("let a = 5;", TypeData::I64),
        ("let a: i32 = 5i32;", TypeData::I32),
        ("let a: i64 = 5i64;", TypeData::I64),
        ("let a: i32 = -2147483648;", TypeData::I32),
        ("let a: i64 = -9223372036854775808;", TypeData::I64),
    ];
    for (body, expected) in clean_cases {
        let module = expect_clean(&entry(&format!("{body} return 1;")));
        let local = &entry_of(&module).locals[0];
        assert_eq!(module.type_data(local.ty), Some(expected), "case: {body}");
    }
}

#[test]
fn out_of_range_i32_literal_reports_t001() {
    // The literal no longer fits i32, so the literal itself becomes i64; the
    // out-of-range literal mismatch is classified as T001 per the brief.
    assert_eq!(
        expect_codes(&entry("let a: i32 = 5000000000; return 1;")),
        vec!["T001"]
    );
}

// ---------------------------------------------------------------------------
// Arithmetic and operator rules
// ---------------------------------------------------------------------------

#[test]
fn matching_numeric_addition_is_clean() {
    expect_clean("@entry scalar probe(p: i32) -> i32 { return p + 1; }");
    expect_clean("@entry scalar probe(q: i64) -> i64 { return q + 1; }");
    expect_clean("@entry scalar probe(x: f64) -> f64 { return x + 1.0; }");
}

#[test]
fn mixed_numeric_addition_reports_t003() {
    assert_eq!(
        expect_codes("@entry scalar probe(a: i32, b: i64) -> i64 { return a + b; }"),
        vec!["T003"]
    );
}

#[test]
fn string_concatenation_reports_t003() {
    assert_eq!(expect_codes(&entry("return \"a\" + \"b\";")), vec!["T003"]);
}

#[test]
fn remainder_rules() {
    expect_clean("@entry scalar probe(a: i32, b: i32) -> i32 { return a % b; }");
    assert_eq!(
        expect_codes("@entry scalar probe(a: f64, b: f64) -> f64 { return a % b; }"),
        vec!["T003"]
    );
    assert_eq!(
        expect_codes("@entry scalar probe(a: i32, b: i64) -> i64 { return a % b; }"),
        vec!["T003"]
    );
}

#[test]
fn logical_not_requires_bool() {
    expect_clean("@entry scalar probe(b: bool) -> bool { return !b; }");
    assert_eq!(
        expect_codes("@entry scalar probe(b: i32) -> i64 { return !b; }"),
        vec!["T003"]
    );
}

#[test]
fn unary_signs_require_numeric_operands_and_are_clean_on_numbers() {
    expect_clean("@entry scalar probe(p: i32) -> i32 { return -p; }");
    expect_clean("@entry scalar probe(x: f64) -> f64 { return -x; }");
    expect_clean("@entry scalar probe(p: i32) -> i32 { return +p; }");
}

#[test]
fn statement_conditions_must_be_bool() {
    assert_eq!(
        expect_codes(&entry("if (1) { return 1; } return 1;")),
        vec!["T003"]
    );
    assert_eq!(
        expect_codes(&entry("while (\"x\") { break; } return 1;")),
        vec!["T003"]
    );
}

#[test]
fn logical_operands_must_be_bool() {
    expect_clean("@entry scalar probe(a: bool, b: bool) -> bool { return a && b; }");
    assert_eq!(
        expect_codes("@entry scalar probe() -> bool { return 1 && true; }"),
        vec!["T003"]
    );
    assert_eq!(
        expect_codes("@entry scalar probe() -> bool { return true || 2; }"),
        vec!["T003"]
    );
}

// ---------------------------------------------------------------------------
// Equality and ordering
// ---------------------------------------------------------------------------

#[test]
fn equality_on_compatible_types_is_clean() {
    expect_clean("@entry scalar probe(a: i32, b: i32) -> bool { return a == b; }");
    expect_clean("@entry scalar probe(a: i64, b: i64) -> bool { return a != b; }");
    expect_clean("@entry scalar probe(a: f64, b: f64) -> bool { return a == b; }");
    expect_clean("@entry scalar probe(a: bool, b: bool) -> bool { return a == b; }");
    expect_clean("@entry scalar probe(a: string, b: string) -> bool { return a == b; }");
    expect_clean("@entry scalar probe() -> bool { return b\"a\" == b\"b\"; }");
}

#[test]
fn mixed_numeric_equality_reports_t003() {
    assert_eq!(
        expect_codes("@entry scalar probe(a: i32, b: i64) -> bool { return a == b; }"),
        vec!["T003"]
    );
}

#[test]
fn ordering_on_comparable_types_is_clean() {
    expect_clean("@entry scalar probe(a: i32, b: i32) -> bool { return a < b; }");
    expect_clean("@entry scalar probe(a: i64, b: i64) -> bool { return a <= b; }");
    expect_clean("@entry scalar probe(a: f64, b: f64) -> bool { return a < b; }");
    expect_clean("@entry scalar probe() -> bool { return \"a\" < \"b\"; }");
}

#[test]
fn ordering_on_non_comparable_types_reports_t003() {
    assert_eq!(
        expect_codes("@entry scalar probe(a: bool, b: bool) -> bool { return a < b; }"),
        vec!["T003"]
    );
    assert_eq!(
        expect_codes("@entry scalar probe() -> bool { return b\"a\" < b\"b\"; }"),
        vec!["T003"]
    );
    assert_eq!(
        expect_codes(&entry(
            "let xs: [i64] = [1]; let ys: [i64] = [2]; return xs < ys;"
        )),
        vec!["T003"]
    );
    assert_eq!(
        expect_codes("@entry scalar probe(a: i64?, b: i64?) -> bool { return a < b; }"),
        vec!["T003"]
    );
}

// ---------------------------------------------------------------------------
// Optional rules
// ---------------------------------------------------------------------------

#[test]
fn optional_injection_rules() {
    // A suffixed i32 literal injects cleanly into an optional.
    let module = expect_clean(&entry("let a: i32? = 5i32; return 1;"));
    let local = &entry_of(&module).locals[0];
    assert!(matches!(
        module.type_data(local.ty),
        Some(TypeData::Optional(inner)) if module.type_data(inner) == Some(TypeData::I32)
    ));
    expect_clean(&entry("let a: i32? = null; return 1;"));
    // An unsuffixed literal that fits i32 injects into i32?.
    expect_clean(&entry("let a: i32? = 5; return 1;"));
}

#[test]
fn unsuffixed_literal_injects_into_annotated_optional() {
    let module = expect_clean("@entry scalar f() -> i64 { let a: i32? = 5; return 1; }");
    let local = &entry_of(&module).locals[0];
    assert!(matches!(
        module.type_data(local.ty),
        Some(TypeData::Optional(inner)) if module.type_data(inner) == Some(TypeData::I32)
    ));
}

#[test]
fn null_and_wrong_literals_into_non_optional_report_t001() {
    assert_eq!(
        expect_codes(&entry("let a: i64 = null; return 1;")),
        vec!["T001"]
    );
    assert_eq!(
        expect_codes(&entry("let a: i32? = \"x\"; return 1;")),
        vec!["T001"]
    );
}

#[test]
fn is_null_accepts_optionals_only() {
    expect_clean("@entry scalar probe(p: i64?) -> bool { return is_null(p); }");
    assert_eq!(
        expect_codes("@entry scalar probe() -> bool { return is_null(5); }"),
        vec!["T001"]
    );
}

#[test]
fn unwrap_yields_the_inner_type() {
    expect_clean("@entry scalar probe() -> i32 { let a: i32? = 5; return unwrap(a); }");
    assert_eq!(expect_codes(&entry("return unwrap(5);")), vec!["T001"]);
}

#[test]
fn unwrap_of_null_reports_t001() {
    assert_eq!(expect_codes(&entry("return unwrap(null);")), vec!["T001"]);
}

#[test]
fn conditional_expression_rules() {
    expect_clean("@entry scalar probe(c: bool) -> i64 { let x: i64? = c ? 1 : null; return 1; }");
    expect_clean("@entry scalar probe(c: bool) -> i32 { return c ? 1 : 2; }");
    assert_eq!(
        expect_codes("@entry scalar probe(c: bool) -> i64 { return c ? 1 : \"x\"; }"),
        vec!["T003"]
    );
}

#[test]
fn contextual_typing_into_binary_and_list_literals() {
    expect_clean("@entry scalar probe() -> i32 { return 1 + 2; }");
    expect_clean(&entry("let x: i32 = 1 + 2; return 1;"));
    expect_clean(&entry("let xs: [i64?] = [1, null]; return 1;"));
}

#[test]
fn equality_against_null() {
    expect_clean("@entry scalar probe(p: i32?) -> bool { return p == null; }");
    assert_eq!(
        expect_codes("@entry scalar probe() -> bool { return 5 == null; }"),
        vec!["T003"]
    );
    expect_clean("@entry scalar probe() -> bool { return null == null; }");
}

// ---------------------------------------------------------------------------
// Cast and len builtins
// ---------------------------------------------------------------------------

#[test]
fn cast_builtins_accept_their_mono_argument_types() {
    expect_clean("@entry scalar probe(p: i32) -> i64 { return to_i64(p); }");
    expect_clean("@entry scalar probe(p: i32) -> f64 { return to_f64_i32(p); }");
    expect_clean("@entry scalar probe(q: i64) -> f64 { return to_f64_i64(q); }");
    expect_clean("@entry scalar probe(q: i64) -> i32 { return to_i32(q); }");
    expect_clean("@entry scalar probe(x: f64) -> i32 { return to_i32_f64(x); }");
    expect_clean("@entry scalar probe(x: f64) -> i64 { return to_i64_f64(x); }");
}

#[test]
fn cast_builtins_reject_wrong_argument_types_with_t001() {
    assert_eq!(expect_codes(&entry("return to_i64(5i64);")), vec!["T001"]);
    assert_eq!(
        expect_codes("@entry scalar probe(p: i32) -> i32 { return to_i32(p); }"),
        vec!["T001"]
    );
    assert_eq!(
        expect_codes("@entry scalar probe(x: f64) -> f64 { return to_f64_i32(x); }"),
        vec!["T001"]
    );
    assert_eq!(
        expect_codes("@entry scalar probe(q: i64) -> i64 { return to_i64_f64(q); }"),
        vec!["T001"]
    );
}

#[test]
fn len_accepts_string_bytes_and_lists() {
    expect_clean("@entry scalar probe() -> i32 { return len(\"abc\"); }");
    expect_clean("@entry scalar probe() -> i32 { return len(b\"x\"); }");
    expect_clean(&entry("let xs: [i64] = [1]; return 1;"));
    expect_clean("@entry scalar probe() -> i32 { let xs: [i64] = [1]; return len(xs); }");
    assert_eq!(
        expect_codes("@entry scalar probe() -> i32 { return len(5); }"),
        vec!["T001"]
    );
}

#[test]
fn builtin_arity_mismatches_report_t004() {
    assert_eq!(expect_codes(&entry("return to_i64();")), vec!["T004"]);
    assert_eq!(
        expect_codes("@entry scalar probe() -> i32 { return len(); }"),
        vec!["T004"]
    );
    assert_eq!(
        expect_codes(&validator_entry("return accept(1);")),
        vec!["T004"]
    );
}

// ---------------------------------------------------------------------------
// Record and list literals
// ---------------------------------------------------------------------------

#[test]
fn record_literal_with_all_fields_present_is_clean() {
    expect_clean(&entry("let r: { a: i64 } = { a: 1 }; return 1;"));
}

#[test]
fn record_literal_errors_report_t001() {
    // An unknown field is reported, and the (now unseen) expected field is
    // also reported as missing.
    assert_eq!(
        expect_codes(&entry("let r: { a: i64 } = { b: 1 }; return 1;")),
        vec!["T001", "T001"]
    );
    assert_eq!(
        expect_codes(&entry("let r: { a: i64 } = { a: 1, a: 2 }; return 1;")),
        vec!["T001"]
    );
    assert_eq!(
        expect_codes(&entry("let r: { a: i64 } = {}; return 1;")),
        vec!["T001"]
    );
    assert_eq!(
        expect_codes(&entry("let r: { a: string } = { a: 1 }; return 1;")),
        vec!["T001"]
    );
    assert_eq!(
        expect_codes(&entry("let r = { a: 1 }; return 1;")),
        vec!["T001"]
    );
}

#[test]
fn record_literal_injects_into_expected_optional_record() {
    expect_clean(&entry("let r: { a: i64 }? = { a: 1 }; return 1;"));
}

#[test]
fn nested_records_with_expected_types_are_clean() {
    expect_clean(&entry(
        "let r: { a: { b: i64 } } = { a: { b: 1 } }; return 1;",
    ));
}

#[test]
fn list_literal_rules() {
    expect_clean(&entry("let xs: [i32] = []; return 1;"));
    assert_eq!(expect_codes(&entry("let xs = []; return 1;")), vec!["T001"]);
    assert_eq!(
        expect_codes(&entry("let xs: [i32] = [1, \"x\"]; return 1;")),
        vec!["T001"]
    );
    // A non-empty literal without an expected type infers from its first element.
    let module = expect_clean(&entry("let xs = [1, 2]; return 1;"));
    let local = &entry_of(&module).locals[0];
    assert!(matches!(
        module.type_data(local.ty),
        Some(TypeData::List(inner)) if module.type_data(inner) == Some(TypeData::I64)
    ));
}

// ---------------------------------------------------------------------------
// Field access and indexing
// ---------------------------------------------------------------------------

#[test]
fn record_field_access_yields_the_field_type() {
    let module = expect_clean("@entry scalar probe() -> string { let r: { name: string } = { name: \"x\" }; return r.name; }");
    let entry_fn = entry_of(&module);
    let local = &entry_fn.locals[0];
    assert!(matches!(
        module.type_data(local.ty),
        Some(TypeData::Record(_))
    ));
}

#[test]
fn field_access_errors() {
    assert_eq!(
        expect_codes(&entry("let r: { a: i64 } = { a: 1 }; return r.nope;")),
        vec!["N001"]
    );
    assert_eq!(
        expect_codes("@entry scalar probe(p: i64) -> i64 { return p.name; }"),
        vec!["T003"]
    );
    assert_eq!(
        expect_codes(&validator_entry("return record_error(record.name);")),
        vec!["T003"]
    );
}

#[test]
fn list_indexing_rules() {
    expect_clean(&entry("let xs: [i64] = [1]; return xs[0];"));
    assert_eq!(
        expect_codes(&entry("let xs: [i64] = [1]; return xs[\"a\"];")),
        vec!["T001"]
    );
    assert_eq!(
        expect_codes("@entry scalar probe(p: i64) -> i64 { return p[0]; }"),
        vec!["T003"]
    );
}

// ---------------------------------------------------------------------------
// Opaque and null type positions (T009)
// ---------------------------------------------------------------------------

#[test]
fn validation_local_is_clean_only_in_validators() {
    let module = expect_clean(&validator_entry("let v: Validation = accept(); return v;"));
    assert_eq!(entry_of(&module).locals.len(), 1);
    // The checker reports the T009 on the type position; accept() itself also
    // exceeds the scalar capability, and both spans sort in source order.
    assert_eq!(
        expect_codes(&entry("let v: Validation = accept(); return 1;")),
        vec!["T009", "T007"]
    );
}

#[test]
fn scalar_wire_safe_optional_parameters_and_results_are_clean() {
    expect_clean("@entry scalar probe(p: i32?) -> i32? { return p; }");
    expect_clean("@entry scalar probe(p: { a: [i32] }) -> i64 { return 1; }");
    expect_clean("@entry scalar probe(p: { a: i64? }) -> i64 { return 1; }");
}

#[test]
fn opaque_entry_boundaries_report_t009() {
    assert_eq!(
        expect_codes("@entry scalar probe(p: Validation) -> i64 { return 1; }"),
        vec!["T009"]
    );
    assert_eq!(
        expect_codes("@entry scalar probe() -> Validation { return 1; }"),
        vec!["T009"]
    );
}

#[test]
fn null_type_positions_report_t009() {
    assert_eq!(
        expect_codes("@entry scalar probe() -> null? { return null; }"),
        vec!["T009"]
    );
    assert_eq!(
        expect_codes("@entry scalar probe(p: null) -> i64 { return 1; }"),
        vec!["T009"]
    );
    assert_eq!(
        expect_codes(&entry("let bad: null = null; return 1;")),
        vec!["T009"]
    );
}

#[test]
fn opaque_nested_inside_lists_and_records_reports_t009() {
    assert_eq!(
        expect_codes(&validator_entry(
            "let bad: [Validation] = [accept()]; return accept();"
        )),
        vec!["T009"]
    );
    assert_eq!(
        expect_codes(&validator_entry(
            "let bad: { v: Validation } = { v: accept() }; return accept();"
        )),
        vec!["T009"]
    );
}

#[test]
fn helper_functions_may_use_opaque_types_internally() {
    expect_clean(
        "fn h(b: Batch) -> Batch { return b; }\n@entry scalar probe() -> i64 { return 1; }",
    );
}

#[test]
fn validator_shape_rules_report_t009() {
    assert_eq!(
        expect_codes("@entry validator probe(record2: { name: string }?, old_record: { name: string }?) -> Validation { return accept(); }"),
        vec!["T009"]
    );
    assert_eq!(
        expect_codes(
            "@entry validator probe(record: { name: string }?) -> Validation { return accept(); }"
        ),
        vec!["T009"]
    );
    assert_eq!(
        expect_codes("@entry validator probe(record: { name: string }, old_record: { name: string }) -> Validation { return accept(); }"),
        vec!["T009", "T009"]
    );
    assert_eq!(
        expect_codes("@entry validator probe(record: { name: string }?, old_record: { name: string }?) -> i64 { return 1; }"),
        vec!["T009"]
    );
}

// ---------------------------------------------------------------------------
// Assignment targets (T006)
// ---------------------------------------------------------------------------

#[test]
fn assignment_to_params_consts_fields_and_slots_reports_t006() {
    assert_eq!(
        expect_codes("@entry scalar probe(p: i64) -> i64 { p = 1; return 1; }"),
        vec!["T006"]
    );
    assert_eq!(
        expect_codes(&entry("const c = 1; c = 2; return 1;")),
        vec!["T006"]
    );
    assert_eq!(
        expect_codes(&entry("let r: { a: i64 } = { a: 1 }; r.a = 1; return 1;")),
        vec!["T006"]
    );
    assert_eq!(
        expect_codes(&entry("let xs: [i64] = [1]; xs[0] = 1; return 1;")),
        vec!["T006"]
    );
    assert_eq!(
        expect_codes(
            "fn add(a: i64, b: i64) -> i64 { return a; }\n@entry scalar probe() -> i64 { add = 1; return 1; }"
        ),
        vec!["T006"]
    );
}

#[test]
fn every_expression_of_a_clean_module_has_a_type_entry() {
    let source = "@entry scalar f(p: i64, c: bool) -> i64 {\n  let a: i32? = 5;\n  let b = p;\n  b = b + 1;\n  if (b > 0) { return b; }\n  let r: { v: i64 } = { v: 2 };\n  let z = r.v;\n  let xs: [i64] = [1];\n  let y = xs[0];\n  let w = c ? 1 : 2;\n  return y + z + b + w;\n}";
    let module = expect_clean(source);
    let parsed = crate::front::parse(source, &CompileLimits::default()).expect("fixture parses");
    assert_eq!(module.expr_types.len(), parsed.expressions.len());
    assert!(
        module.expr_types.iter().all(Option::is_some),
        "every expression needs a type entry in a clean module"
    );
}

#[test]
fn assignment_to_a_mutable_local_is_clean() {
    expect_clean(&entry("let x = 1; x = 2; return x;"));
}

#[test]
fn assigned_value_type_is_checked() {
    assert_eq!(
        expect_codes(&entry("let x = 1; x = \"x\"; return 1;")),
        vec!["T001"]
    );
    assert_eq!(
        expect_codes("@entry scalar probe(p: i32) -> i64 { let x = 1; x = p; return 1; }"),
        vec!["T002"]
    );
}

// ---------------------------------------------------------------------------
// T001 vs T002 classification
// ---------------------------------------------------------------------------

#[test]
fn non_numeric_mismatches_are_t001() {
    assert_eq!(
        expect_codes(&entry("let a: i32 = \"x\"; return 1;")),
        vec!["T001"]
    );
    assert_eq!(
        expect_codes(&entry("let a: string = 5; return 1;")),
        vec!["T001"]
    );
    let diagnostic = first_diagnostic(&entry("let a: i32 = \"x\"; return 1;"));
    assert!(diagnostic.message.contains("expected i32"));
}

#[test]
fn numeric_mismatches_are_t002() {
    assert_eq!(
        expect_codes("@entry scalar probe(p: i64) -> i64 { let a: i32 = p; return 1; }"),
        vec!["T002"]
    );
    assert_eq!(
        expect_codes("@entry scalar probe(p: i32) -> i64 { let a: i64 = p; return 1; }"),
        vec!["T002"]
    );
    assert_eq!(
        expect_codes("@entry scalar probe(p: i64) -> i64 { let a: f64 = p; return 1; }"),
        vec!["T002"]
    );
}

// ---------------------------------------------------------------------------
// Return value rules
// ---------------------------------------------------------------------------

#[test]
fn return_value_mismatches_are_classified() {
    assert_eq!(
        expect_codes("@entry scalar probe(p: i64) -> string { return p; }"),
        vec!["T001"]
    );
    assert_eq!(
        expect_codes("@entry scalar probe(p: i64) -> i32 { return p + 1; }"),
        vec!["T002"]
    );
}

#[test]
fn null_entry_result_reports_t009() {
    // The null return type is the rejected thing (T009); the value-return
    // mismatch is suppressed because the converted result is the error type.
    assert_eq!(
        expect_codes("@entry scalar probe() -> null { return 1; }"),
        vec!["T009"]
    );
}

#[test]
fn local_slot_types_survive_parameter_offsets() {
    let source =
        "@entry scalar f(p: i64) -> i64 {\n  let a: i32? = 5;\n  let b = p;\n  return b + 1;\n}";
    let module = expect_clean(source);
    let entry = entry_of(&module);
    assert_eq!(entry.locals.len(), 2);
    assert_eq!(entry.locals[0].slot, 1);
    assert_eq!(entry.locals[1].slot, 2);
    assert!(matches!(
        module.type_data(entry.locals[0].ty),
        Some(TypeData::Optional(_))
    ));
    assert_eq!(module.type_data(entry.locals[1].ty), Some(TypeData::I64));
}
