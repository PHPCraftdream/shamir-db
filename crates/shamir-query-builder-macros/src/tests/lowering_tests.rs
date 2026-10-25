//! Token-rendering tests for `q!` lowering (RV-QUERY-BUILDER-MACROS-36328515,
//! 2C459E8E, 30DA1AC8, 7E797992): expansions must reference only
//! `::shamir_query_builder` paths and honour the repo-qualified call form.

use crate::query_parse::QMacro;
use crate::{filter_lower, query_parse_lower};
use proc_macro2::TokenStream as TokenStream2;
use syn::parse2;

fn parse_q(src: &str) -> syn::Result<QMacro> {
    let ts: TokenStream2 = src.parse().unwrap();
    parse2::<QMacro>(ts)
}

fn q_err(src: &str) -> String {
    match parse_q(src) {
        Ok(_) => panic!("`{src}` should not parse"),
        Err(e) => e.to_string(),
    }
}

fn render_q(src: &str) -> String {
    let qm = parse_q(src).expect("parse should succeed");
    query_parse_lower::render(&qm).expect("lowering should succeed")
}

#[test]
fn rv_query_builder_macros_36328515_expansion_has_no_query_types_paths() {
    let forms = [
        "from t where x == 1 select count(*) as n order_by x asc limit 5 offset 1",
        "insert into t values { \"a\" => 1 }",
        "update t set { \"a\" => 1 } where x == 2",
        "delete from t where x == 3",
        "upsert t key { \"k\" => 1 } value { \"v\" => 2 }",
        "call p(1, \"x\")",
    ];
    for form in forms {
        let rendered = render_q(form);
        assert!(
            !rendered.contains("shamir_query_types"),
            "expansion of `{form}` references shamir_query_types: {rendered}"
        );
    }
}

#[test]
fn rv_query_builder_macros_36328515_call_uses_builder_paths() {
    let rendered = render_q("call p(1)");
    assert!(rendered.contains("shamir_query_builder :: CallOp"));
    assert!(rendered.contains("shamir_query_builder :: FilterValue"));
}

#[test]
fn rv_query_builder_macros_2c459e8e_repo_qualified_call_lowers_repo() {
    let rendered = render_q("call my_repo.daily(1)");
    // Flatten by removing whitespace; proc_macro2 renders `::` Joint but single
    // `:` spaced, so `:` + `::` join to `:::` in the flattened string.
    let flat: String = rendered.split_whitespace().collect();
    assert!(
        flat.contains("call:::std::string::String::from(\"daily\")"),
        "name field wrong in expansion: {rendered}"
    );
    assert!(
        flat.contains("repo:::std::string::String::from(\"my_repo\")"),
        "repo field wrong in expansion: {rendered}"
    );
}

/// Preservation test: also passes on the pre-fix code.
#[test]
fn rv_query_builder_macros_2c459e8e_unqualified_call_defaults_to_main() {
    let rendered = render_q("call daily(1)");
    // Whitespace removed; `:` + `::` join to `:::` in the flattened string.
    let flat: String = rendered.split_whitespace().collect();
    assert!(
        flat.contains("repo:::std::string::String::from(\"main\")"),
        "expansion: {rendered}"
    );
    assert!(
        flat.contains("call:::std::string::String::from(\"daily\")"),
        "expansion: {rendered}"
    );
}

#[test]
fn rv_query_builder_macros_30da1ac8_dotted_clause_keyword_field_where() {
    let rendered = render_q("from t where foo.select == 1");
    assert!(
        rendered.contains("\"foo\"") && rendered.contains("\"select\""),
        "where clause truncated at keyword-like segment: {rendered}"
    );
}

#[test]
fn rv_query_builder_macros_30da1ac8_clause_keyword_as_rhs_ident() {
    assert!(parse_q("from t where x == group_by").is_ok());
}

#[test]
fn rv_query_builder_macros_30da1ac8_bare_clause_keyword_still_terminates() {
    // Preservation pin: also passes on the pre-fix code.
    let err = q_err("from t where select");
    assert_eq!(
        err.to_string(),
        "q!: expected a filter expression after `where`"
    );
}

/// Pins current behaviour (30DA1AC8, accepted limitation): a field named
/// exactly like a clause keyword is rejected at the start of a where
/// expression — the grammar is not changed.
#[test]
fn rv_query_builder_macros_30da1ac8_clause_named_field_at_expr_start_rejected() {
    let err = q_err("from t where select == 1");
    assert_eq!(
        err.to_string(),
        "q!: expected a filter expression after `where`"
    );
}

#[test]
fn rv_query_builder_macros_7e797992_predicate_field_arg_wording() {
    let ts: TokenStream2 = "like(1, \"x\")".parse().unwrap();
    let expr: syn::Expr = parse2(ts).unwrap();
    let err = filter_lower::lower_expr(&expr).unwrap_err();
    assert_eq!(
        err.to_string(),
        "filter!: expected a field name (ident) or dotted field path (e.g. `address.city`)"
    );
}
