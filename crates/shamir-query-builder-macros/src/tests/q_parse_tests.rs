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
