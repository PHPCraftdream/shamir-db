//! Tests for the `filter!` proc-macro.

use serde_json::json;
use shamir_query_types::filter::Filter;

use crate::filter as filter_mod;
use crate::val::*;

// The `filter!` proc-macro is re-exported at the crate root.
// It resolves via `::shamir_query_builder::filter::*` paths internally,
// but we invoke it as `crate::filter!(...)` (auto-resolved by Rust's
// macro namespace) or via the explicit re-export.
use crate::filter;

// ── helpers ────────────────────────────────────────────────────────

/// Compare via wire JSON (order-insensitive for structural equality).
fn assert_same_wire(a: &Filter, b: &Filter) {
    let ja = serde_json::to_value(a).unwrap();
    let jb = serde_json::to_value(b).unwrap();
    assert_eq!(ja, jb, "wire JSON mismatch:\n  left:  {ja}\n  right: {jb}");
}

// ── basic comparisons ──────────────────────────────────────────────

#[test]
fn filter_eq_string() {
    let from_macro = filter!(status == "active");
    let from_builder = filter_mod::eq("status", "active");
    assert_same_wire(&from_macro, &from_builder);
}

#[test]
fn filter_ne() {
    let from_macro = filter!(role != "guest");
    let from_builder = filter_mod::ne("role", "guest");
    assert_same_wire(&from_macro, &from_builder);
}

#[test]
fn filter_gt() {
    let from_macro = filter!(age > 18);
    let from_builder = filter_mod::gt("age", 18);
    assert_same_wire(&from_macro, &from_builder);
}

#[test]
fn filter_gte() {
    let from_macro = filter!(score >= 90);
    let from_builder = filter_mod::gte("score", 90);
    assert_same_wire(&from_macro, &from_builder);
}

#[test]
fn filter_lt() {
    let from_macro = filter!(price < 100.0_f64);
    let from_builder = filter_mod::lt("price", 100.0_f64);
    assert_same_wire(&from_macro, &from_builder);
}

#[test]
fn filter_lte() {
    let from_macro = filter!(qty <= 5);
    let from_builder = filter_mod::lte("qty", 5);
    assert_same_wire(&from_macro, &from_builder);
}

// ── logical: && and || ─────────────────────────────────────────────

#[test]
fn filter_and_two() {
    let from_macro = filter!(status == "active" && age > 18);
    let from_builder = filter_mod::and([
        filter_mod::eq("status", "active"),
        filter_mod::gt("age", 18),
    ]);
    assert_same_wire(&from_macro, &from_builder);
}

#[test]
fn filter_or_two() {
    let from_macro = filter!(role == "admin" || vip == true);
    let from_builder =
        filter_mod::or([filter_mod::eq("role", "admin"), filter_mod::eq("vip", true)]);
    assert_same_wire(&from_macro, &from_builder);
}

// ── negation ───────────────────────────────────────────────────────

#[test]
fn filter_not() {
    let from_macro = filter!(!(status == "deleted"));
    let from_builder = filter_mod::not(filter_mod::eq("status", "deleted"));
    assert_same_wire(&from_macro, &from_builder);
}

// ── precedence: && binds tighter than || ───────────────────────────

#[test]
fn filter_precedence_or_and() {
    // `a || b && c` parses as `a || (b && c)` by Rust precedence.
    let from_macro = filter!(x == 1 || y == 2 && z == 3);
    let from_builder = filter_mod::or([
        filter_mod::eq("x", 1),
        filter_mod::and([filter_mod::eq("y", 2), filter_mod::eq("z", 3)]),
    ]);
    assert_same_wire(&from_macro, &from_builder);
}

#[test]
fn filter_precedence_parens_override() {
    // `(a || b) && c` — parens force OR to bind first.
    let from_macro = filter!((x == 1 || y == 2) && z == 3);
    let from_builder = filter_mod::and([
        filter_mod::or([filter_mod::eq("x", 1), filter_mod::eq("y", 2)]),
        filter_mod::eq("z", 3),
    ]);
    assert_same_wire(&from_macro, &from_builder);
}

// ── nested field path ──────────────────────────────────────────────

#[test]
fn filter_nested_field() {
    let from_macro = filter!(address.city == "NYC");
    let from_builder = filter_mod::eq(["address", "city"], "NYC");
    assert_same_wire(&from_macro, &from_builder);
}

// ── RHS expressions: func / col ────────────────────────────────────

#[test]
fn filter_func_rhs() {
    let from_macro = filter!(name == func("strings/lower", [lit("ALICE")]));
    let from_builder = filter_mod::eq("name", func("strings/lower", [lit("ALICE")]));
    assert_same_wire(&from_macro, &from_builder);
}

// ── complex composition from the spec ──────────────────────────────

#[test]
fn filter_complex_spec_example() {
    // status == "active" && (role == "admin" || vip == true) && age > 18
    let from_macro = filter!(status == "active" && (role == "admin" || vip == true) && age > 18);

    // The macro lowers `a && b && c` (left-assoc) as `and([and([a, b]), c])`.
    let expected = filter_mod::and([
        filter_mod::and([
            filter_mod::eq("status", "active"),
            filter_mod::or([filter_mod::eq("role", "admin"), filter_mod::eq("vip", true)]),
        ]),
        filter_mod::gt("age", 18),
    ]);
    assert_same_wire(&from_macro, &expected);
}

// ── wire JSON snapshot ─────────────────────────────────────────────

#[test]
fn filter_wire_json_snapshot() {
    let f = filter!(status == "active" && age > 18);
    let got = serde_json::to_value(&f).unwrap();
    let expected = json!({
        "op": "and",
        "filters": [
            {
                "op": "eq",
                "field": ["status"],
                "value": "active"
            },
            {
                "op": "gt",
                "field": ["age"],
                "value": 18
            }
        ]
    });
    assert_eq!(got, expected);
}

// ── predicate-call forms ──────────────────────────────────────────

#[test]
fn filter_like() {
    let from_macro = filter!(like(name, "Al%"));
    let from_builder = filter_mod::like("name", "Al%");
    assert_same_wire(&from_macro, &from_builder);
}

#[test]
fn filter_ilike() {
    let from_macro = filter!(ilike(name, "al%"));
    let from_builder = filter_mod::ilike("name", "al%");
    assert_same_wire(&from_macro, &from_builder);
}

#[test]
fn filter_regex_predicate() {
    let from_macro = filter!(regex(email, "^admin@"));
    let from_builder = filter_mod::regex("email", "^admin@");
    assert_same_wire(&from_macro, &from_builder);
}

#[test]
fn filter_is_null() {
    let from_macro = filter!(is_null(email));
    let from_builder = filter_mod::is_null("email");
    assert_same_wire(&from_macro, &from_builder);
}

#[test]
fn filter_is_not_null() {
    let from_macro = filter!(is_not_null(phone));
    let from_builder = filter_mod::is_not_null("phone");
    assert_same_wire(&from_macro, &from_builder);
}

#[test]
fn filter_exists_predicate() {
    let from_macro = filter!(exists(avatar));
    let from_builder = filter_mod::exists("avatar");
    assert_same_wire(&from_macro, &from_builder);
}

#[test]
fn filter_not_exists_predicate() {
    let from_macro = filter!(not_exists(deleted_at));
    let from_builder = filter_mod::not_exists("deleted_at");
    assert_same_wire(&from_macro, &from_builder);
}

#[test]
fn filter_contains_predicate() {
    let from_macro = filter!(contains(tags, "rust"));
    let from_builder = filter_mod::contains("tags", "rust");
    assert_same_wire(&from_macro, &from_builder);
}

#[test]
fn filter_contains_any_predicate() {
    let from_macro = filter!(contains_any(tags, ["rust", "python"]));
    let from_builder = filter_mod::contains_any("tags", ["rust", "python"]);
    assert_same_wire(&from_macro, &from_builder);
}

#[test]
fn filter_contains_all_predicate() {
    let from_macro = filter!(contains_all(perms, ["read", "write"]));
    let from_builder = filter_mod::contains_all("perms", ["read", "write"]);
    assert_same_wire(&from_macro, &from_builder);
}

#[test]
fn filter_in_predicate() {
    let from_macro = filter!(in_(role, ["admin", "mod"]));
    let from_builder = filter_mod::in_("role", ["admin", "mod"]);
    assert_same_wire(&from_macro, &from_builder);
}

#[test]
fn filter_not_in_predicate() {
    let from_macro = filter!(not_in(status, ["banned", "deleted"]));
    let from_builder = filter_mod::not_in("status", ["banned", "deleted"]);
    assert_same_wire(&from_macro, &from_builder);
}

#[test]
fn filter_between_predicate() {
    let from_macro = filter!(between(age, 18, 65));
    let from_builder = filter_mod::between("age", 18, 65);
    assert_same_wire(&from_macro, &from_builder);
}

#[test]
fn filter_fts_predicate() {
    let from_macro = filter!(fts(content, "hello world", "match"));
    let from_builder = filter_mod::fts("content", "hello world", "match");
    assert_same_wire(&from_macro, &from_builder);
}

#[test]
fn filter_vector_similarity_predicate() {
    let from_macro = filter!(vector_similarity(embedding, vec![0.1_f32, 0.2, 0.3], 10));
    let from_builder = filter_mod::vector_similarity("embedding", vec![0.1_f32, 0.2, 0.3], 10);
    assert_same_wire(&from_macro, &from_builder);
}

// ── dotted field in predicates ────────────────────────────────────

#[test]
fn filter_like_dotted_field() {
    let from_macro = filter!(like(address.street, "Main%"));
    let from_builder = filter_mod::like(["address", "street"], "Main%");
    assert_same_wire(&from_macro, &from_builder);
}

#[test]
fn filter_is_null_dotted_field() {
    let from_macro = filter!(is_null(profile.bio));
    let from_builder = filter_mod::is_null(["profile", "bio"]);
    assert_same_wire(&from_macro, &from_builder);
}

// ── predicates composed with &&/||/! ──────────────────────────────

#[test]
fn filter_predicate_composite() {
    let from_macro = filter!(
        like(name, "Al%")
            && (in_(role, ["admin", "mod"]) || is_null(email))
            && between(age, 18, 65)
    );
    let from_builder = filter_mod::and([
        filter_mod::and([
            filter_mod::like("name", "Al%"),
            filter_mod::or([
                filter_mod::in_("role", ["admin", "mod"]),
                filter_mod::is_null("email"),
            ]),
        ]),
        filter_mod::between("age", 18, 65),
    ]);
    assert_same_wire(&from_macro, &from_builder);
}

#[test]
fn filter_negated_predicate() {
    let from_macro = filter!(!is_null(email) && age > 18);
    let from_builder = filter_mod::and([
        filter_mod::not(filter_mod::is_null("email")),
        filter_mod::gt("age", 18),
    ]);
    assert_same_wire(&from_macro, &from_builder);
}

#[test]
fn filter_predicate_mixed_with_comparison() {
    let from_macro = filter!(like(name, "Al%") && age > 18 && !is_null(email));
    let from_builder = filter_mod::and([
        filter_mod::and([filter_mod::like("name", "Al%"), filter_mod::gt("age", 18)]),
        filter_mod::not(filter_mod::is_null("email")),
    ]);
    assert_same_wire(&from_macro, &from_builder);
}

// ── RHS func call stays as value expr (not predicate) ─────────────

#[test]
fn filter_func_rhs_not_treated_as_predicate() {
    // When a Call appears on the RHS of a comparison, it's a value expr.
    let from_macro = filter!(name == func("strings/lower", [lit("ALICE")]));
    let from_builder = filter_mod::eq("name", func("strings/lower", [lit("ALICE")]));
    assert_same_wire(&from_macro, &from_builder);
}

// ── computed predicates ────────────────────────────────────────────

#[test]
fn filter_computed() {
    let from_macro = filter!(computed("lower", email, "eq", "alice@foo.com"));
    let from_builder = filter_mod::computed("lower", "email", "eq", "alice@foo.com");
    assert_same_wire(&from_macro, &from_builder);
}

#[test]
fn filter_computed_dotted_field() {
    let from_macro = filter!(computed("lower", address.city, "eq", "ny"));
    let from_builder = filter_mod::computed("lower", ["address", "city"], "eq", "ny");
    assert_same_wire(&from_macro, &from_builder);
}

#[test]
fn filter_computed_with_args() {
    let from_macro = filter!(computed_with_args(
        "substring",
        name,
        [lit(0_i64), lit(3_i64)],
        "eq",
        "ali"
    ));
    let from_builder =
        filter_mod::computed_with_args("substring", "name", [lit(0_i64), lit(3_i64)], "eq", "ali");
    assert_same_wire(&from_macro, &from_builder);
}

#[test]
fn filter_computed_in_conjunction() {
    let from_macro = filter!(computed("lower", email, "eq", "alice@foo.com") && age > 18);
    let from_builder = filter_mod::and([
        filter_mod::computed("lower", "email", "eq", "alice@foo.com"),
        filter_mod::gt("age", 18),
    ]);
    assert_same_wire(&from_macro, &from_builder);
}

// ── fts with contains_any composition ─────────────────────────────

#[test]
fn filter_fts_and_contains_any() {
    let from_macro =
        filter!(fts(body, "rust async", "phrase") && contains_any(tags, ["tutorial", "guide"]));
    let from_builder = filter_mod::and([
        filter_mod::fts("body", "rust async", "phrase"),
        filter_mod::contains_any("tags", ["tutorial", "guide"]),
    ]);
    assert_same_wire(&from_macro, &from_builder);
}
