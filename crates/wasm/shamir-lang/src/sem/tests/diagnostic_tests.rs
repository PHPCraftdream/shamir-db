use crate::api::{CompileLimits, Stage};
use crate::sem::types::TypeData;

use super::helper::{check_with_stage, expect_clean, first_diagnostic};

#[test]
fn query_ref_field_arity_is_checked_before_site_validation() {
    let diagnostic = first_diagnostic("@entry scalar probe() -> i64 { return query_ref_field(); }");
    assert_eq!(diagnostic.code, "T004");
}

#[test]
fn n001_renders_code_line_column_and_message() {
    let source = "@entry scalar probe() -> i64 {\n  return missing; }";
    assert_eq!(
        first_diagnostic(source).to_string(),
        "N001:2:10: unresolved name missing"
    );
}

#[test]
fn n002_renders_code_line_column_and_message() {
    let source = "@entry scalar probe() -> i64 {\n  let x = 1;\n  let x = 2; return 1; }";
    assert_eq!(
        first_diagnostic(source).to_string(),
        "N002:3:3: duplicate local x"
    );
}

#[test]
fn t001_renders_code_line_column_and_message() {
    let source = "@entry scalar probe() -> i64 {\n  let a: i32 = \"x\"; return 1; }";
    assert_eq!(
        first_diagnostic(source).to_string(),
        "T001:2:16: expected i32, found string (initializer)"
    );
}

#[test]
fn t002_renders_code_line_column_and_message() {
    let source = "@entry scalar probe(p: i64) -> i64 {\n  let a: i32 = p; return 1; }";
    assert_eq!(
        first_diagnostic(source).to_string(),
        "T002:2:16: no implicit conversion from i64 to i32 (initializer) (i32 range -2147483648..=2147483647)"
    );
}

#[test]
fn t003_renders_code_line_column_and_message() {
    let source = "@entry scalar probe() -> i64 {\n  return \"a\" + \"b\"; return 1; }";
    assert_eq!(
        first_diagnostic(source).to_string(),
        "T003:2:10: operator + requires numeric operands"
    );
}

#[test]
fn t004_renders_code_line_column_and_message() {
    let source =
        "fn add(a: i64, b: i64) -> i64 { return a; }\n@entry scalar probe() -> i64 {\n  return add; }";
    assert_eq!(
        first_diagnostic(source).to_string(),
        "T004:3:10: function and builtin names are not values"
    );
}

#[test]
fn t005_renders_code_line_column_and_message() {
    let source = "@entry scalar probe() -> i64 {\n  return; return 1; }";
    assert_eq!(
        first_diagnostic(source).to_string(),
        "T005:2:3: bare return requires a -> null function"
    );
}

#[test]
fn t006_renders_code_line_column_and_message() {
    let source = "@entry scalar probe(p: i64) -> i64 {\n  p = 1; return 1; }";
    assert_eq!(
        first_diagnostic(source).to_string(),
        "T006:2:3: cannot assign to a parameter"
    );
}

#[test]
fn t007_renders_code_line_column_and_message() {
    let source = "@entry procedure p(x: i64) -> i64 { return x; }";
    assert_eq!(
        first_diagnostic(source).to_string(),
        "T007:1:1: procedure entries are not available in the MVP stage"
    );
}

#[test]
fn t008_renders_primary_and_related_lines() {
    let source =
        "fn f(n: i64) -> i64 { return f(n); }\n@entry scalar probe() -> i64 { return f(1); }";
    assert_eq!(
        first_diagnostic(source).to_string(),
        "T008:1:30: recursive call cycle detected\n  related 1:1: first function of the recursion cycle"
    );
}

#[test]
fn t009_renders_code_line_column_and_message() {
    let source = "@entry scalar probe(p: Validation) -> i64 { return 1; }";
    assert_eq!(
        first_diagnostic(source).to_string(),
        "T009:1:24: Validation is only permitted for validator results and internal helpers"
    );
}

#[test]
fn b001_renders_code_line_column_and_message() {
    let source = "@entry scalar probe() -> i64 { return 1; }";
    let limits = CompileLimits {
        compile_fuel: 0,
        ..CompileLimits::default()
    };
    assert_eq!(
        check_with_stage(source, &limits, Stage::Mvp)
            .err()
            .and_then(|diags| diags.first().map(|diag| diag.to_string())),
        Some("B001:1:1: compile fuel exceeded (limit 0)".to_string())
    );
}

#[test]
fn duplicate_function_names_report_the_second_declaration() {
    let source = "fn twice() -> i64 { return 1; }\nfn twice() -> i64 { return 2; }\n@entry scalar probe() -> i64 { return twice(); }";
    let diagnostic = first_diagnostic(source);
    assert_eq!(diagnostic.code, "N002");
    assert_eq!(diagnostic.primary.start.line, 2);
    assert_eq!(diagnostic.primary.start.column, 1);
}

#[test]
fn field_access_expression_is_typed_in_the_table() {
    let source =
        "@entry scalar probe() -> string { let r: { name: string } = { name: \"x\" }; return r.name; }";
    let module = expect_clean(source);
    let string_typed = module
        .expr_types
        .iter()
        .flatten()
        .filter(|ty| module.type_data(**ty) == Some(TypeData::String))
        .count();
    assert!(
        string_typed >= 2,
        "the string literal and the field-access expression must both be typed"
    );
}
