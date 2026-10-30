//! Parse-level diagnostics for `q!` clauses (RV-QUERY-BUILDER-MACROS-7E797992,
//! FB738525, E50875AB): clause names must be reported accurately and select
//! output keys must be unique.

use crate::query_parse::QMacro;
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

#[test]
fn rv_query_builder_macros_ec1b86b3_clause_keywords_in_expressions() {
    for src in [
        "from t where foo.select == 1 select foo.select",
        "from t where x == group_by select x",
        "from t where x == call(limit) order_by x asc",
        "from t where (x == limit || y == offset) limit 3",
        "from t group_by x having x == offset select x",
        "update t set { \"x\" => 1 } where foo.select == 2",
        "delete from t where x == group_by",
    ] {
        assert!(parse_q(src).is_ok(), "{src}");
    }
}

#[test]
fn rv_query_builder_macros_002c0148_raw_identifier_normalizes_table() {
    let rendered = crate::query_parse_lower::render(&parse_q("from r#users").unwrap()).unwrap();
    assert!(rendered.contains("Query :: from (\"users\")"), "{rendered}");
}

#[test]
fn rv_query_builder_macros_3c03a7da_group_and_order_lists_accept_paths_and_literals() {
    assert!(parse_q("from t group_by a.b, \"literal.field\" order_by a.b desc").is_ok());
}

#[test]
fn rv_query_builder_macros_03ba722d_lists_accept_trailing_commas() {
    assert!(parse_q("from t group_by a, select a, order_by a asc,").is_ok());
}

#[test]
fn rv_query_builder_macros_7e797992_empty_having_names_having() {
    let err = q_err("from t group_by g having");
    assert_eq!(
        err.to_string(),
        "unexpected end of input, q!: expected a filter expression after `having`"
    );
}

/// Preservation test: also passes on the pre-fix code.
#[test]
fn rv_query_builder_macros_7e797992_empty_where_still_names_where() {
    let err = q_err("from t where");
    assert_eq!(
        err.to_string(),
        "unexpected end of input, q!: expected a filter expression after `where`"
    );
}

#[test]
fn rv_query_builder_macros_fb738525_duplicate_count_keys_rejected() {
    let err = q_err("from t group_by g select count(*), count(*)");
    assert_eq!(
        err.to_string(),
        "q!: duplicate select output key `count`; give each aggregate a distinct `as` alias"
    );
}

/// Preservation test: also passes on the pre-fix code.
#[test]
fn rv_query_builder_macros_fb738525_distinct_aliases_accepted() {
    assert!(parse_q("from t select count(*) as n, count(*) as m").is_ok());
}

#[test]
fn rv_query_builder_macros_e50875ab_unknown_select_fn_precise_error() {
    let err = q_err("from t select weird_fn(x)");
    assert_eq!(
        err.to_string(),
        "q!: unknown select function `weird_fn`; supported: count, sum, avg, min, max, agg_fn, func"
    );
}

#[test]
fn rv_query_builder_macros_2c459e8e_repo_qualified_call_parses() {
    assert!(parse_q("call my_repo.daily(1)").is_ok());
}
