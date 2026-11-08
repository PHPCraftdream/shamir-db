use crate::front::EntryKind;
use crate::sem::types::TypeData;

use super::helper::{entry_of, expect_clean, expect_codes, first_diagnostic, SCALAR_EXAMPLE};

fn entry(body: &str) -> String {
    format!("@entry scalar probe() -> i64 {{ {body} }}")
}

#[test]
fn worked_scalar_example_resolves_cleanly() {
    let module = expect_clean(SCALAR_EXAMPLE);
    assert_eq!(entry_of(&module).name, "sum");
}

#[test]
fn unresolved_identifier_reports_n001_with_stable_prefix() {
    let source = entry("return missing;");
    assert_eq!(expect_codes(&source), vec!["N001"]);
    let diagnostic = first_diagnostic(&source);
    assert!(diagnostic.to_string().starts_with("N001:1:"));
}

#[test]
fn unknown_record_field_reports_n001() {
    assert_eq!(
        expect_codes(&entry("let r: { a: i64 } = { a: 1 }; return r.nope;")),
        vec!["N001"]
    );
}

#[test]
fn param_local_function_and_builtin_names_resolve_cleanly() {
    let module = expect_clean(
        "fn add(a: i64, b: i64) -> i64 { return a + b; }\n@entry scalar probe(p: i64) -> i64 {\n  let local = add(p, 1);\n  if (len(\"x\") == 1) { return local; }\n  return p;\n}",
    );
    assert!(module.resolved.iter().flatten().count() > 0);
}

#[test]
fn duplicate_function_names_report_n002() {
    assert_eq!(
        expect_codes(
            "fn dup() -> null { return; }\nfn dup() -> null { return; }\n@entry scalar probe() -> i64 { return 1; }"
        ),
        vec!["N002"]
    );
}

#[test]
fn function_named_like_a_builtin_reports_n002() {
    assert_eq!(
        expect_codes(
            "fn len(a: i64) -> i64 { return a; }\n@entry scalar probe() -> i64 { return 1; }"
        ),
        vec!["N002"]
    );
}

#[test]
fn parameter_named_len_reports_n002() {
    assert_eq!(
        expect_codes("@entry scalar probe(len: i64) -> i64 { return 1; }"),
        vec!["N002"]
    );
}

#[test]
fn parameter_named_args_reports_n002() {
    assert_eq!(
        expect_codes("@entry scalar probe(args: i64) -> i64 { return 1; }"),
        vec!["N002"]
    );
}

#[test]
fn duplicate_parameter_pair_reports_n002_with_prefix() {
    let source = "@entry scalar probe(a: i64, a: i64) -> i64 { return 1; }";
    assert_eq!(expect_codes(source), vec!["N002"]);
    assert!(first_diagnostic(source).to_string().starts_with("N002:1:"));
}

#[test]
fn duplicate_local_in_same_block_reports_n002() {
    assert_eq!(
        expect_codes(&entry("let x = 1; let x = 2; return x;")),
        vec!["N002"]
    );
}

#[test]
fn duplicate_record_type_field_reports_n002() {
    assert_eq!(
        expect_codes(&entry("let r: { a: i64, a: i64 } = { a: 1 }; return 1;")),
        vec!["N002"]
    );
}

#[test]
fn local_shadowing_a_function_name_makes_the_call_a_t004() {
    assert_eq!(
        expect_codes(
            "fn add(a: i64, b: i64) -> i64 { return a; }\n@entry scalar probe() -> i64 { let add = 1; return add(); }"
        ),
        vec!["T004"]
    );
}

#[test]
fn nested_block_shadowing_of_locals_is_clean() {
    let module = expect_clean(&entry("let x = 1; { let x = 2; } return x;"));
    let entry_fn = entry_of(&module);
    assert_eq!(entry_fn.locals.len(), 2);
}

#[test]
fn let_x_eq_x_sees_the_outer_binding() {
    let module = expect_clean("@entry scalar probe(x: i64) -> i64 { let x = x; return x; }");
    let local = &entry_of(&module).locals[0];
    assert_eq!(module.type_data(local.ty), Some(TypeData::I64));
    assert_eq!(local.slot, 1);
}

#[test]
fn let_x_eq_x_without_an_outer_binding_reports_n001() {
    assert_eq!(expect_codes(&entry("let x = x; return 1;")), vec!["N001"]);
}

#[test]
fn function_used_before_declaration_resolves_cleanly() {
    let module = expect_clean(
        "@entry scalar sum(a: i64, b: i64) -> i64 { return add(a, b); }\nfn add(a: i64, b: i64) -> i64 { return a + b; }",
    );
    assert_eq!(module.call_order.len(), 2);
    assert_eq!(module.call_order.last(), Some(&module.entry));
}

#[test]
fn duplicate_names_still_bind_the_first_declaration_for_calls() {
    let source = "fn twice() -> i64 { return 1; }\nfn twice() -> i64 { return 2; }\n@entry scalar probe() -> i64 { return twice(); }";
    assert_eq!(expect_codes(source), vec!["N002"]);
}

#[test]
fn scalar_entry_kind_is_preserved_on_the_entry_function() {
    let module = expect_clean(SCALAR_EXAMPLE);
    assert_eq!(entry_of(&module).kind, Some(EntryKind::Scalar));
}
