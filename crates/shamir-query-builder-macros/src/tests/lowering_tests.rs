//! Token-rendering tests for `q!` lowering and root-path hygiene.

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
fn rv_query_builder_macros_02993b77_all_forms_use_supplied_root() {
    for form in [
        "from r.t where x == 1 select x as y order_by x asc",
        "insert into r.t values { \"x\" => 1 }",
        "update r.t set { \"x\" => 1 } where x == 1",
        "delete from r.t where x == 1",
        "upsert r.t key { \"x\" => 1 } value { \"y\" => 2 }",
        "call r.p(1)",
    ] {
        let qm = parse_q(form).unwrap();
        let rendered = qm
            .to_tokens(&quote::quote! { ::alternate_root })
            .unwrap()
            .to_string();
        assert!(rendered.contains("alternate_root"), "{form}: {rendered}");
        assert!(
            !rendered.contains("shamir_query_builder"),
            "{form}: {rendered}"
        );
    }
}

#[test]
fn rv_query_builder_macros_a690bea8_predicate_lowering_avoids_temporary_collections() {
    let source = include_str!("../filter_lower.rs");
    let predicate = source
        .split_once("fn lower_predicate_call(")
        .unwrap()
        .1
        .split_once("// ── field-path extraction")
        .unwrap()
        .0;
    assert!(!predicate.contains("iter().collect()"));
    assert!(!predicate.contains("ident.to_string()"));
}

#[test]
fn rv_query_builder_macros_ec1b86b3_filter_parser_does_not_reparse_prefixes() {
    let source = include_str!("../query_parse.rs");
    let parser = source
        .split_once("fn parse_filter_expr(")
        .unwrap()
        .1
        .split_once("fn is_clause_keyword(")
        .unwrap()
        .0;
    assert!(!parser.contains("parse2::<Expr>"));
    assert!(!parser.contains("Group::new"));
}

#[test]
fn rv_query_builder_macros_002c0148_raw_select_function_matches_plain_count() {
    let plain = render_q("from t select count(*) as n");
    let raw = render_q("from t select r#count(*) as n");
    assert_eq!(raw, plain, "raw and plain builtin select lowering differ");
}

#[test]
fn rv_query_builder_macros_002c0148_raw_filter_identifier_normalizes() {
    let ts: TokenStream2 = "r#type == 1".parse().unwrap();
    let expr: syn::Expr = parse2(ts).unwrap();
    let rendered = filter_lower::lower_expr(&expr, &quote::quote! { ::shamir_query_builder })
        .unwrap()
        .to_string();
    assert!(rendered.contains("\"type\""));
    assert!(!rendered.contains("r#type"));
}

#[test]
fn rv_query_builder_macros_eee42189_generated_chain_retains_sequence() {
    let rendered = render_q("from t order_by a asc, b.c desc");
    assert!(rendered.contains("order_by_asc"));
    assert!(rendered.contains("order_by_desc"));
}

#[test]
fn rv_query_builder_macros_eee42189_loop_accumulates_fragments_without_reinterpolation() {
    let source = include_str!("../query_parse_lower.rs");
    let doc_map = source
        .split_once("fn lower_doc_map(doc: &DocMapAst, root: &TokenStream2)")
        .expect("doc-map lowering should exist")
        .1
        .split_once("/// Emit a table constructor prefix")
        .expect("doc-map lowering should end")
        .0;
    assert!(
        !doc_map.contains("#chain"),
        "doc-map lowering re-interpolates chain: {doc_map}"
    );
    assert!(doc_map.contains(".map(|(key, val)|"));
    assert!(doc_map.contains(".collect();"));
    assert!(doc_map.contains("#(#entries)*"));

    let insert_lowering = source
        .split_once("impl InsertMacro {")
        .expect("insert lowering should exist")
        .1
        .split_once("impl UpdateMacro {")
        .expect("insert lowering should end")
        .0;
    assert!(
        !insert_lowering.contains("#chain.row"),
        "insert lowering re-interpolates chain: {insert_lowering}"
    );
    assert!(insert_lowering.contains(".docs\n            .iter()"));
    assert!(insert_lowering.contains("quote! { .row(#doc_ts) }"));
    assert!(insert_lowering.contains("#chain #(#rows)* .build()"));

    let order_loop = source
        .split_once("for (field, dir) in &self.order_by_items {")
        .expect("order_by loop should exist")
        .1
        .split_once("\n        }")
        .expect("order_by loop should end")
        .0;
    assert!(
        !order_loop.contains("#chain"),
        "order_by loop re-interpolates chain: {order_loop}"
    );
    assert!(order_loop.contains("order_by.push(match dir"));
    assert!(source.contains("#chain #(#order_by)*"));
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
fn rv_query_builder_macros_2c459e8e_call_uses_new_constructor() {
    let rendered = render_q("call daily()");
    assert!(rendered.contains("CallOp :: new"), "expansion: {rendered}");
    assert!(rendered.contains("Vec :: < shamir_query_builder :: FilterValue > :: from"));

    let rendered = render_q("call daily(1, \"x\")");
    assert!(rendered.contains("CallOp :: new"), "expansion: {rendered}");
    assert!(rendered.contains("Into :: < shamir_query_builder :: FilterValue > :: into"));
}

#[test]
fn rv_query_builder_macros_2c459e8e_repo_qualified_call_uses_in_repo_constructor() {
    let rendered = render_q("call my_repo.daily(1)");
    assert!(
        rendered.contains("CallOp :: in_repo"),
        "expansion: {rendered}"
    );
    assert!(rendered.contains("\"daily\""), "expansion: {rendered}");
    assert!(rendered.contains("\"my_repo\""), "expansion: {rendered}");
}

#[test]
fn rv_query_builder_macros_2c459e8e_unqualified_call_uses_new_constructor() {
    let rendered = render_q("call daily(1)");
    assert!(rendered.contains("CallOp :: new"), "expansion: {rendered}");
    assert!(rendered.contains("\"daily\""), "expansion: {rendered}");
    assert!(!rendered.contains("in_repo"), "expansion: {rendered}");
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
    let err =
        filter_lower::lower_expr(&expr, &quote::quote! { ::shamir_query_builder }).unwrap_err();
    assert_eq!(
        err.to_string(),
        "filter!: expected a field name (ident) or dotted field path (e.g. `address.city`)"
    );
}
