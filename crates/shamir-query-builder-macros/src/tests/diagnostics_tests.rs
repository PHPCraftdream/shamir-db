//! Rejection-path diagnostics for `q!` / `filter!` lowering.
//!
//! This suite is the RV-QUERY-BUILDER-MACROS-A63844FD remediation: every
//! error path below asserts the EXACT `syn::Error` message so diagnostic
//! regressions cannot pass silently.

use crate::filter_lower;
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

fn lower_src(src: &str) -> syn::Result<TokenStream2> {
    let expr: syn::Expr = parse2(src.parse().unwrap()).unwrap();
    filter_lower::lower_expr(&expr)
}

/// Preservation test: also passes on the pre-fix code.
#[test]
fn rv_query_builder_macros_a63844fd_empty_inputs_error_not_panic() {
    for src in ["", "insert", "update", "delete", "upsert", "call"] {
        assert!(
            parse_q(src).is_err(),
            "parsing `{src}` should error, not panic"
        );
    }
}

#[test]
fn rv_query_builder_macros_e24d5f62_doc_map_trailing_tokens_message() {
    let err = q_err("insert into t values { \"a\" => 1 \"b\" => 2 }");
    assert_eq!(
        err.to_string(),
        "q!: unexpected trailing tokens in doc map; expected \"key\" => value pairs separated by `,`"
    );
}

#[test]
fn rv_query_builder_macros_e24d5f62_call_args_trailing_tokens_message() {
    let err = q_err("call p(1 2)");
    assert_eq!(
        err.to_string(),
        "q!(call ...): unexpected trailing tokens in argument list; separate arguments with `,`"
    );
}

/// Preservation test: also passes on the pre-fix code.
#[test]
fn rv_query_builder_macros_e24d5f62_dotted_ident_missing_field_message() {
    let err = q_err("from t select sum(a.) as x");
    assert_eq!(
        err.to_string(),
        "unexpected end of input, expected field name after `.`"
    );
}

#[test]
fn rv_query_builder_macros_a63844fd_unknown_first_keyword() {
    let err = q_err("bogus x");
    assert_eq!(
        err.to_string(),
        "q!: expected `from`, `insert`, `update`, `delete`, `upsert`, or `call`"
    );
}

#[test]
fn rv_query_builder_macros_a63844fd_aggregate_requires_as() {
    let err = q_err("from t select count(f)");
    assert_eq!(
        err.to_string(),
        "unexpected end of input, q!: aggregate `count(...)` requires `as <alias>` after it"
    );
}

#[test]
fn rv_query_builder_macros_a63844fd_order_by_requires_direction() {
    let err = q_err("from t order_by x");
    assert_eq!(
        err.to_string(),
        "unexpected end of input, order_by: expected `asc` or `desc` after field name"
    );
}

/// Preservation test: also passes on the pre-fix code.
#[test]
fn rv_query_builder_macros_a63844fd_trailing_tokens_after_read() {
    let err = q_err("from t limit 5 bogus");
    assert_eq!(
        err.to_string(),
        "q!: unexpected tokens after query; clauses must appear in order: \
         from, where, group_by, having, select, order_by, limit, offset"
    );
}

/// Preservation test: also passes on the pre-fix code (inputs updated per
/// F2 to reach the intended guards).
#[test]
fn rv_query_builder_macros_a63844fd_trailing_tokens_after_each_write_kind() {
    let cases = [
        (
            "insert into t values { \"a\" => 1 } extra",
            "q!(insert ...): unexpected tokens after values",
        ),
        (
            "update t set { \"a\" => 1 } where x == 1 limit 3",
            "q!(update ...): unexpected tokens after where clause",
        ),
        (
            "delete from t where x == 1 limit 3",
            "q!(delete ...): unexpected tokens after where clause",
        ),
        (
            "upsert t key { \"k\" => 1 } value { \"v\" => 2 } extra",
            "q!(upsert ...): unexpected tokens after value doc",
        ),
        (
            "call p(1) extra",
            "q!(call ...): unexpected tokens after arguments",
        ),
    ];
    for (src, expected) in cases {
        assert_eq!(q_err(src), expected, "src: {src}");
    }
}

#[test]
fn rv_query_builder_macros_a63844fd_unsupported_filter_operator() {
    let err = lower_src("a + b").unwrap_err();
    assert_eq!(
        err.to_string(),
        "filter!: unsupported operator; use ==, !=, >, >=, <, <=, &&, ||"
    );
}

#[test]
fn rv_query_builder_macros_a63844fd_unknown_predicate_name() {
    // Preservation pin: also passes on the pre-fix code.
    let err = lower_src("nope(a)").unwrap_err();
    assert_eq!(
        err.to_string(),
        "filter!: unknown predicate `nope`; supported predicates: like, ilike, regex, \
         is_null, is_not_null, exists, not_exists, contains, contains_any, contains_all, \
         in_, not_in, between, fts, vector_similarity, computed, computed_with_args"
    );
}

#[test]
fn rv_query_builder_macros_a63844fd_unsupported_expression_shape() {
    // Preservation pin: also passes on the pre-fix code.
    let err = lower_src("a?").unwrap_err();
    assert_eq!(
        err.to_string(),
        "filter!: unsupported expression; expected comparisons (==, !=, >, >=, <, <=), \
         logical operators (&&, ||, !), predicate calls (like, is_null, between, ...), \
         or parenthesised groups"
    );
}

#[test]
fn rv_query_builder_macros_a63844fd_predicate_arity_messages_exact() {
    let cases = [
        (
            "like(a)",
            "filter!: `like` expects 2 arguments: (field, pattern)",
        ),
        ("is_null(a, b)", "filter!: `is_null` expects 1 argument: (field)"),
        (
            "contains(a)",
            "filter!: `contains` expects 2 arguments: (field, value)",
        ),
        (
            "contains_any(a)",
            "filter!: `contains_any` expects 2 arguments: (field, [values...])",
        ),
        (
            "in_(a)",
            "filter!: `in_` expects 2 arguments: (field, [values...])",
        ),
        (
            "between(a, b)",
            "filter!: `between` expects 3 arguments: (field, lo, hi)",
        ),
        (
            "fts(a, b)",
            "filter!: `fts` expects 3 arguments: (field, query, mode)",
        ),
        (
            "vector_similarity(a, b)",
            "filter!: `vector_similarity` expects 3 arguments: (field, vec_expr, k)",
        ),
        (
            "computed(a, b, c)",
            "filter!: `computed` expects 4 arguments: (expr_op, field, cmp, value)",
        ),
        (
            "computed_with_args(a, b, c, d)",
            "filter!: `computed_with_args` expects 5 arguments: (expr_op, field, expr_args, cmp, value)",
        ),
    ];
    for (src, expected) in cases.iter() {
        let err = lower_src(src).unwrap_err();
        assert_eq!(err.to_string(), *expected, "src: {src}");
    }
}

#[test]
fn rv_query_builder_macros_e24d5f62_select_fn_trailing_tokens_exact() {
    for (src, func) in [
        ("from t select count(* extra) as n", "count"),
        ("from t select sum(a extra) as n", "sum"),
        ("from t select func(\"f\", [] extra) as n", "func"),
        ("from t select count(a extra) as n", "count"),
        ("from t select agg_fn(\"f\", a extra) as n", "agg_fn"),
    ] {
        let err = q_err(src);
        assert_eq!(
            err.to_string(),
            format!("q!: unexpected trailing tokens inside `{func}(...)`"),
            "src: {src}"
        );
    }
}

#[test]
fn rv_query_builder_macros_a63844fd_unsupported_unary_operator() {
    let err = lower_src("-a").unwrap_err();
    assert_eq!(
        err.to_string(),
        "filter!: only the `!` unary operator is supported (negation)"
    );
}

#[test]
fn rv_query_builder_macros_a63844fd_non_simple_predicate_callee() {
    let err = lower_src("foo::bar(x)").unwrap_err();
    assert_eq!(
        err.to_string(),
        "filter!: predicate call must be a simple name (like, ilike, regex, is_null, \
         is_not_null, exists, not_exists, contains, contains_any, contains_all, in_, \
         not_in, between, fts, vector_similarity, computed, computed_with_args)"
    );
}

#[test]
fn rv_query_builder_macros_a63844fd_delete_requires_where() {
    let err = q_err("delete from t");
    assert_eq!(
        err.to_string(),
        "unexpected end of input, q!(delete ...): `where` clause is required for delete statements"
    );
}

#[test]
fn rv_query_builder_macros_a63844fd_tuple_index_field_access() {
    let qm = parse_q("from t where t.0 == 1").expect("parse should succeed");
    let err = crate::query_parse_lower::render(&qm).unwrap_err();
    assert_eq!(
        err.to_string(),
        "filter!: tuple-index field access is not supported in field paths"
    );
}

#[test]
fn rv_query_builder_macros_a63844fd_predicate_wrong_arity() {
    let err = lower_src("like(a)").unwrap_err();
    assert_eq!(
        err.to_string(),
        "filter!: `like` expects 2 arguments: (field, pattern)"
    );
}
